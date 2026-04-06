use std::{
    collections::HashMap,
    fs, io,
    path::{Path, PathBuf},
    process::Stdio,
    time::Duration,
    time::SystemTime,
};

use apkw_proto::apkw::v1::{
    job_service_client::JobServiceClient, ErrorCode, ErrorDetail, Id, JobState, KeyValue, Target,
    TargetKind,
};
use serde::Deserialize;
use tokio::process::Command;
use tonic::{transport::Channel, Status};
use tracing::warn;

use crate::adb::{
    adb_connect, adb_failure_message, adb_find_device_serial, adb_get_prop, adb_get_prop_timeout,
    adb_get_state, adb_path, adb_shell, format_adb_output, health_state_from_adb_state,
    wait_for_adb_device,
};
use crate::ids::{canonicalize_adb_serial, normalize_target_id, normalize_target_id_for_compare};
use crate::jobs::{
    cancel_requested, connect_job, job_error_detail, job_is_cancelled, metric, publish_completed,
    publish_failed, publish_log, publish_progress, publish_state, spawn_cancel_watcher,
};
use crate::state::data_dir;

#[derive(Default)]
pub(crate) struct CuttlefishStatus {
    pub(crate) adb_serial: String,
    pub(crate) running: bool,
    pub(crate) raw: String,
    pub(crate) details: Vec<(String, String)>,
}

#[derive(Debug)]
pub(crate) enum CuttlefishStatusError {
    NotInstalled,
    Failed(String),
}

const CUTTLEFISH_CAPABILITY_QUERY_DEFAULT_PATH: &str =
    "/usr/lib/cuttlefish-common/bin/capability_query.py";

fn cuttlefish_enabled() -> bool {
    match std::env::var("APKW_CUTTLEFISH_ENABLE") {
        Ok(val) => !(val == "0" || val.eq_ignore_ascii_case("false")),
        Err(_) => true,
    }
}

fn cuttlefish_page_size_check_enabled() -> bool {
    match std::env::var("APKW_CUTTLEFISH_PAGE_SIZE_CHECK") {
        Ok(val) => !(val == "0" || val.eq_ignore_ascii_case("false")),
        Err(_) => true,
    }
}

fn cuttlefish_kvm_check_enabled() -> bool {
    match std::env::var("APKW_CUTTLEFISH_KVM_CHECK") {
        Ok(val) => !(val == "0" || val.eq_ignore_ascii_case("false")),
        Err(_) => true,
    }
}

fn local_display_available() -> bool {
    let wayland = std::env::var("WAYLAND_DISPLAY")
        .ok()
        .map(|value| !value.trim().is_empty())
        .unwrap_or(false);
    let x11 = std::env::var("DISPLAY")
        .ok()
        .map(|value| !value.trim().is_empty())
        .unwrap_or(false);
    wayland || x11
}

fn cuttlefish_connect_enabled() -> bool {
    match std::env::var("APKW_CUTTLEFISH_CONNECT") {
        Ok(val) => !(val == "0" || val.eq_ignore_ascii_case("false")),
        Err(_) => true,
    }
}

pub(crate) fn cuttlefish_adb_serial() -> String {
    std::env::var("APKW_CUTTLEFISH_ADB_SERIAL").unwrap_or_else(|_| "127.0.0.1:6520".into())
}

fn cuttlefish_web_url() -> String {
    std::env::var("APKW_CUTTLEFISH_WEBRTC_URL").unwrap_or_else(|_| "https://localhost:8443".into())
}

fn cuttlefish_env_url() -> String {
    std::env::var("APKW_CUTTLEFISH_ENV_URL").unwrap_or_else(|_| "https://localhost:1443".into())
}

fn cuttlefish_cvd_bin() -> String {
    std::env::var("APKW_CVD_BIN").unwrap_or_else(|_| "cvd".into())
}

fn cuttlefish_launch_bin() -> String {
    std::env::var("APKW_LAUNCH_CVD_BIN").unwrap_or_else(|_| "launch_cvd".into())
}

fn cuttlefish_stop_bin() -> String {
    std::env::var("APKW_STOP_CVD_BIN").unwrap_or_else(|_| "stop_cvd".into())
}

fn cuttlefish_capability_query_path() -> Option<PathBuf> {
    if let Some(path) = read_env_trimmed("APKW_CUTTLEFISH_CAPABILITY_QUERY") {
        let candidate = PathBuf::from(path);
        return candidate.is_file().then_some(candidate);
    }
    let candidate = PathBuf::from(CUTTLEFISH_CAPABILITY_QUERY_DEFAULT_PATH);
    candidate.is_file().then_some(candidate)
}

fn cuttlefish_host_tools_issue_for_detection(
    has_custom_start_cmd: bool,
    has_cvd: bool,
    has_launch: bool,
    has_capability_query: bool,
) -> Option<String> {
    if has_custom_start_cmd {
        return None;
    }
    if !has_cvd && !has_launch {
        return Some(
            "launch_cvd/cvd host tools not found; install Cuttlefish host tools or set \
             APKW_CUTTLEFISH_START_CMD"
                .into(),
        );
    }
    if !has_capability_query {
        return Some(format!(
            "missing {}; reinstall cuttlefish-base/cuttlefish-user or set \
             APKW_CUTTLEFISH_START_CMD",
            CUTTLEFISH_CAPABILITY_QUERY_DEFAULT_PATH
        ));
    }
    None
}

fn cuttlefish_host_tools_issue(page_size: Option<usize>) -> Option<String> {
    cuttlefish_host_tools_issue_for_detection(
        read_env_trimmed("APKW_CUTTLEFISH_START_CMD").is_some(),
        cuttlefish_cvd_path().is_some(),
        cuttlefish_launch_path(page_size).is_some(),
        cuttlefish_capability_query_path().is_some(),
    )
}

fn cuttlefish_host_tools_ready(page_size: Option<usize>) -> bool {
    cuttlefish_host_tools_issue(page_size).is_none()
}

fn cuttlefish_gpu_mode() -> Option<String> {
    read_env_trimmed("APKW_CUTTLEFISH_GPU_MODE")
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum CuttlefishTapMode {
    Auto,
    Enabled,
    Disabled,
}

#[derive(Clone, Copy, Debug, Default)]
struct CuttlefishResourceLimits {
    cpus: Option<u32>,
    memory_mb: Option<u32>,
    x_res: Option<u32>,
    y_res: Option<u32>,
    dpi: Option<u32>,
}

fn parse_env_bool(raw: &str) -> Option<bool> {
    match raw.trim().to_ascii_lowercase().as_str() {
        "1" | "true" | "yes" | "on" => Some(true),
        "0" | "false" | "no" | "off" => Some(false),
        _ => None,
    }
}

fn cuttlefish_tap_mode() -> CuttlefishTapMode {
    if let Some(mode) = read_env_trimmed("APKW_CUTTLEFISH_TAP_MODE") {
        match mode.to_ascii_lowercase().as_str() {
            "enabled" | "on" | "true" | "1" => return CuttlefishTapMode::Enabled,
            "disabled" | "off" | "false" | "0" => return CuttlefishTapMode::Disabled,
            _ => {}
        }
    }
    if let Some(value) = read_env_trimmed("APKW_CUTTLEFISH_ENABLE_TAP") {
        if let Some(enabled) = parse_env_bool(&value) {
            return if enabled {
                CuttlefishTapMode::Enabled
            } else {
                CuttlefishTapMode::Disabled
            };
        }
    }
    CuttlefishTapMode::Auto
}

fn parse_env_u32(key: &str) -> Option<u32> {
    read_env_trimmed(key)?
        .parse::<u32>()
        .ok()
        .filter(|value| *value > 0)
}

fn cuttlefish_auto_resources_enabled() -> bool {
    match read_env_trimmed("APKW_CUTTLEFISH_AUTO_RESOURCES") {
        Some(value) => parse_env_bool(&value).unwrap_or(true),
        None => true,
    }
}

fn cuttlefish_auto_display_enabled() -> bool {
    match read_env_trimmed("APKW_CUTTLEFISH_AUTO_DISPLAY") {
        Some(value) => parse_env_bool(&value).unwrap_or(true),
        None => true,
    }
}

fn host_total_memory_mb() -> Option<u64> {
    let raw = fs::read_to_string("/proc/meminfo").ok()?;
    for line in raw.lines() {
        let line = line.trim();
        if !line.starts_with("MemTotal:") {
            continue;
        }
        let kb = line
            .split_whitespace()
            .nth(1)
            .and_then(|value| value.parse::<u64>().ok())?;
        return Some(kb / 1024);
    }
    None
}

fn host_cpu_count() -> usize {
    std::thread::available_parallelism()
        .map(|value| value.get())
        .unwrap_or(1)
}

fn recommended_cuttlefish_cpus(host_cpus: usize) -> Option<u32> {
    if host_cpus <= 2 {
        Some(1)
    } else if host_cpus <= 6 {
        Some(2)
    } else {
        None
    }
}

fn recommended_cuttlefish_memory_mb(host_memory_mb: u64) -> Option<u32> {
    if host_memory_mb <= 6 * 1024 {
        Some(2048)
    } else if host_memory_mb <= 12 * 1024 {
        Some(3072)
    } else {
        None
    }
}

fn recommended_cuttlefish_display(
    host_cpus: usize,
    host_memory_mb: u64,
) -> Option<(u32, u32, u32)> {
    if host_cpus <= 2 || host_memory_mb <= 6 * 1024 {
        Some((540, 960, 240))
    } else if host_cpus <= 4 || host_memory_mb <= 8 * 1024 {
        Some((720, 1280, 280))
    } else {
        None
    }
}

fn resolve_cuttlefish_resource_limits(
    start_args: &str,
) -> (CuttlefishResourceLimits, Option<String>) {
    let has_cpu_arg = args_has_flag(start_args, "--cpus");
    let has_mem_arg =
        args_has_flag(start_args, "--memory_mb") || args_has_flag(start_args, "--mem");
    let has_x_res_arg = args_has_flag(start_args, "--x_res");
    let has_y_res_arg = args_has_flag(start_args, "--y_res");
    let has_dpi_arg = args_has_flag(start_args, "--dpi");
    let mut limits = CuttlefishResourceLimits::default();
    let mut notes = Vec::new();

    if !has_cpu_arg {
        if let Some(cpus) = parse_env_u32("APKW_CUTTLEFISH_CPUS") {
            limits.cpus = Some(cpus);
            notes.push(format!("--cpus={cpus} (env)"));
        }
    }
    if !has_mem_arg {
        if let Some(memory_mb) = parse_env_u32("APKW_CUTTLEFISH_MEMORY_MB") {
            limits.memory_mb = Some(memory_mb);
            notes.push(format!("--memory_mb={memory_mb} (env)"));
        }
    }
    if !has_x_res_arg {
        if let Some(x_res) = parse_env_u32("APKW_CUTTLEFISH_X_RES") {
            limits.x_res = Some(x_res);
            notes.push(format!("--x_res={x_res} (env)"));
        }
    }
    if !has_y_res_arg {
        if let Some(y_res) = parse_env_u32("APKW_CUTTLEFISH_Y_RES") {
            limits.y_res = Some(y_res);
            notes.push(format!("--y_res={y_res} (env)"));
        }
    }
    if !has_dpi_arg {
        if let Some(dpi) = parse_env_u32("APKW_CUTTLEFISH_DPI") {
            limits.dpi = Some(dpi);
            notes.push(format!("--dpi={dpi} (env)"));
        }
    }

    let host_cpus = host_cpu_count();
    let host_memory_mb = host_total_memory_mb();
    if cuttlefish_auto_resources_enabled() {
        if limits.cpus.is_none() && !has_cpu_arg {
            if let Some(cpus) = recommended_cuttlefish_cpus(host_cpus) {
                limits.cpus = Some(cpus);
                notes.push(format!("--cpus={cpus} (auto)"));
            }
        }
        if limits.memory_mb.is_none() && !has_mem_arg {
            if let Some(memory_mb) = host_memory_mb.and_then(recommended_cuttlefish_memory_mb) {
                limits.memory_mb = Some(memory_mb);
                notes.push(format!("--memory_mb={memory_mb} (auto)"));
            }
        }
    }
    if cuttlefish_auto_display_enabled() {
        if let Some((x_res, y_res, dpi)) =
            host_memory_mb.and_then(|memory| recommended_cuttlefish_display(host_cpus, memory))
        {
            if limits.x_res.is_none() && !has_x_res_arg {
                limits.x_res = Some(x_res);
                notes.push(format!("--x_res={x_res} (auto)"));
            }
            if limits.y_res.is_none() && !has_y_res_arg {
                limits.y_res = Some(y_res);
                notes.push(format!("--y_res={y_res} (auto)"));
            }
            if limits.dpi.is_none() && !has_dpi_arg {
                limits.dpi = Some(dpi);
                notes.push(format!("--dpi={dpi} (auto)"));
            }
        }
    }

    let note = if notes.is_empty() {
        None
    } else {
        let host_mem_note = host_memory_mb
            .map(|value| value.to_string())
            .unwrap_or_else(|| "unknown".into());
        Some(format!(
            "Applying Cuttlefish resource limits: {} (host_cpus={host_cpus}, host_memory_mb={host_mem_note})",
            notes.join(", ")
        ))
    };
    (limits, note)
}

fn tap_probe_name() -> String {
    // Linux interface names are limited to 15 bytes; keep this short and alphanumeric.
    let pid = std::process::id() as u64;
    let suffix = SystemTime::now()
        .duration_since(SystemTime::UNIX_EPOCH)
        .map(|d| d.as_micros() as u64)
        .unwrap_or(0);
    format!("acf{:04x}{:04x}", pid & 0xffff, suffix & 0xffff)
}

fn tap_permission_error(output: &str) -> bool {
    let lower = output.to_ascii_lowercase();
    lower.contains("operation not permitted")
        || lower.contains("permission denied")
        || lower.contains("tunsetiff")
        || lower.contains("no such file or directory")
        || lower.contains("device not found")
        || lower.contains("cannot find device")
}

async fn host_supports_tap_devices() -> bool {
    if !Path::new("/dev/net/tun").exists() {
        return false;
    }
    let Some(ip_path) = find_command("ip") else {
        return false;
    };
    let probe = tap_probe_name();
    let ip = shell_escape(&ip_path.display().to_string());
    let command =
        format!("{ip} tuntap add dev {probe} mode tap && {ip} tuntap del dev {probe} mode tap");
    let (success, _, stdout, stderr) = match run_shell_command_raw(&command).await {
        Ok(output) => output,
        Err(err) => {
            warn!("failed to probe TAP networking support: {}", err);
            return false;
        }
    };
    if success {
        true
    } else {
        let combined = format!("{stdout}\n{stderr}");
        if !tap_permission_error(&combined) {
            warn!(
                "TAP probe failed with unexpected output; assuming unavailable: {}",
                combined.trim()
            );
        }
        false
    }
}

fn find_command(cmd: &str) -> Option<PathBuf> {
    if cmd.contains('/') {
        let path = PathBuf::from(cmd);
        return path.is_file().then_some(path);
    }

    let mut candidates = Vec::new();
    if let Some(paths) = std::env::var_os("PATH") {
        candidates.extend(std::env::split_paths(&paths));
    }
    candidates.extend([
        PathBuf::from("/usr/bin"),
        PathBuf::from("/bin"),
        PathBuf::from("/usr/sbin"),
        PathBuf::from("/sbin"),
    ]);

    for dir in candidates {
        let candidate = dir.join(cmd);
        if candidate.is_file() {
            return Some(candidate);
        }
    }
    None
}

fn read_os_release() -> HashMap<String, String> {
    let mut values = HashMap::new();
    let mut raw = None;
    for path in ["/etc/os-release", "/usr/lib/os-release"] {
        if let Ok(contents) = fs::read_to_string(path) {
            raw = Some(contents);
            break;
        }
    }
    let Some(raw) = raw else {
        return values;
    };
    for line in raw.lines() {
        let line = line.trim();
        if line.is_empty() || line.starts_with('#') {
            continue;
        }
        let mut parts = line.splitn(2, '=');
        let key = match parts.next() {
            Some(key) if !key.trim().is_empty() => key.trim(),
            _ => continue,
        };
        let value = match parts.next() {
            Some(value) => value.trim(),
            None => continue,
        };
        let value = value.trim_matches('"').trim_matches('\'').to_string();
        values.insert(key.to_string(), value);
    }
    values
}

fn os_release_tokens(value: &str) -> Vec<String> {
    value
        .split_whitespace()
        .map(|token| {
            token
                .trim_matches('"')
                .trim_matches('\'')
                .to_ascii_lowercase()
        })
        .filter(|token| !token.is_empty())
        .collect()
}

fn linux_distro_summary() -> String {
    let values = read_os_release();
    let mut parts = Vec::new();
    if let Some(id) = values.get("ID") {
        if !id.trim().is_empty() {
            parts.push(format!("id={}", id.trim()));
        }
    }
    if let Some(like) = values.get("ID_LIKE") {
        if !like.trim().is_empty() {
            parts.push(format!("id_like={}", like.trim()));
        }
    }
    if let Some(name) = values.get("NAME") {
        if !name.trim().is_empty() {
            parts.push(format!("name={}", name.trim()));
        }
    }
    if parts.is_empty() {
        "id=unknown".into()
    } else {
        parts.join(" ")
    }
}

fn is_debian_like() -> bool {
    let values = read_os_release();
    let mut tokens = Vec::new();
    if let Some(id) = values.get("ID") {
        tokens.extend(os_release_tokens(id));
    }
    if let Some(like) = values.get("ID_LIKE") {
        tokens.extend(os_release_tokens(like));
    }
    tokens.iter().any(|token| {
        matches!(
            token.as_str(),
            "debian" | "ubuntu" | "raspbian" | "linuxmint" | "pop"
        )
    })
}

pub(crate) fn host_page_size() -> Option<usize> {
    let size = unsafe { libc::sysconf(libc::_SC_PAGESIZE) };
    if size > 0 {
        Some(size as usize)
    } else {
        None
    }
}

#[derive(Debug)]
struct KvmStatus {
    present: bool,
    accessible: bool,
    detail: Option<String>,
}

fn kvm_status() -> KvmStatus {
    let path = Path::new("/dev/kvm");
    if !path.exists() {
        return KvmStatus {
            present: false,
            accessible: false,
            detail: Some("missing /dev/kvm".into()),
        };
    }

    match fs::OpenOptions::new().read(true).write(true).open(path) {
        Ok(_) => KvmStatus {
            present: true,
            accessible: true,
            detail: None,
        },
        Err(err) => KvmStatus {
            present: true,
            accessible: false,
            detail: Some(err.to_string()),
        },
    }
}

fn is_root_user() -> bool {
    if let Ok(value) = std::env::var("EUID") {
        if value == "0" {
            return true;
        }
    }
    if let Ok(value) = std::env::var("UID") {
        if value == "0" {
            return true;
        }
    }
    matches!(std::env::var("USER"), Ok(value) if value == "root")
}

fn pkexec_path() -> Option<PathBuf> {
    find_command("pkexec")
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum PrivilegedShellMode {
    Direct,
    Sudo,
    Pkexec,
}

impl PrivilegedShellMode {
    fn description(self) -> &'static str {
        match self {
            Self::Direct => "direct (already root)",
            Self::Sudo => "sudo -n",
            Self::Pkexec => "pkexec",
        }
    }
}

async fn privileged_shell_command(command: &str) -> Result<(String, PrivilegedShellMode), String> {
    if is_root_user() {
        return Ok((command.to_string(), PrivilegedShellMode::Direct));
    }

    if let Some(sudo) = find_command("sudo") {
        let sudo_path = sudo.display().to_string();
        let probe = format!("{} -n true", shell_escape(&sudo_path));
        match run_shell_command_raw(&probe).await {
            Ok((true, _, _, _)) => {
                return Ok((
                    format!(
                        "{} -n /bin/sh -lc {}",
                        shell_escape(&sudo_path),
                        shell_escape(command)
                    ),
                    PrivilegedShellMode::Sudo,
                ));
            }
            Ok(_) | Err(_) => {}
        }
    }

    if let Some(pkexec) = pkexec_path() {
        if local_display_available() {
            let pkexec_path = pkexec.display().to_string();
            return Ok((
                format!(
                    "{} /bin/sh -lc {}",
                    shell_escape(&pkexec_path),
                    shell_escape(command)
                ),
                PrivilegedShellMode::Pkexec,
            ));
        }
    }

    let mut reasons = Vec::new();
    if find_command("sudo").is_some() {
        reasons.push("passwordless sudo unavailable".to_string());
    } else {
        reasons.push("sudo not installed".to_string());
    }
    if pkexec_path().is_some() {
        if !local_display_available() {
            reasons.push("no local display for pkexec authentication".to_string());
        }
    } else {
        reasons.push("pkexec not installed".to_string());
    }
    reasons.push(
        "run apkw-ui in a graphical session, run apkw-targets as root, or set APKW_CUTTLEFISH_INSTALL_CMD"
            .to_string(),
    );
    Err(reasons.join("; "))
}

fn cuttlefish_env_for_page_size(base: &str, page_size: Option<usize>) -> Option<String> {
    if let Some(size) = page_size {
        if size > 4096 {
            return read_env_trimmed(&format!("{base}_16K"));
        }
        if let Some(value) = read_env_trimmed(&format!("{base}_4K")) {
            return Some(value);
        }
    }
    read_env_trimmed(base)
}

fn cuttlefish_home_dir(page_size: Option<usize>) -> PathBuf {
    if let Some(path) = cuttlefish_env_for_page_size("APKW_CUTTLEFISH_HOME", page_size) {
        return PathBuf::from(path);
    }
    let base = data_dir().join("cuttlefish");
    let suffix = page_size.map(page_size_label).unwrap_or("default");
    base.join(suffix.to_lowercase())
}

fn cuttlefish_images_dir(page_size: Option<usize>) -> PathBuf {
    if let Some(path) = cuttlefish_env_for_page_size("APKW_CUTTLEFISH_IMAGES_DIR", page_size) {
        return PathBuf::from(path);
    }
    cuttlefish_home_dir(page_size)
}

pub(crate) fn cuttlefish_host_dir(page_size: Option<usize>) -> PathBuf {
    if let Some(path) = cuttlefish_env_for_page_size("APKW_CUTTLEFISH_HOST_DIR", page_size) {
        return PathBuf::from(path);
    }
    cuttlefish_home_dir(page_size)
}

fn cuttlefish_branch(page_size: Option<usize>) -> String {
    if let Some(branch) = cuttlefish_env_for_page_size("APKW_CUTTLEFISH_BRANCH", page_size) {
        return branch;
    }
    if page_size.unwrap_or(0) > 4096 {
        return "main-16k-with-phones".into();
    }
    "aosp-android-latest-release".into()
}

fn cuttlefish_target(page_size: Option<usize>) -> String {
    if let Some(target) = cuttlefish_env_for_page_size("APKW_CUTTLEFISH_TARGET", page_size) {
        return target;
    }
    if page_size.unwrap_or(0) > 4096 {
        return match std::env::consts::ARCH {
            "aarch64" => "aosp_cf_arm64".into(),
            _ => "aosp_cf_x86_64".into(),
        };
    }
    match std::env::consts::ARCH {
        "aarch64" => "aosp_cf_arm64_only_phone-userdebug".into(),
        "riscv64" => "aosp_cf_riscv64_phone-userdebug".into(),
        _ => "aosp_cf_x86_64_only_phone-userdebug".into(),
    }
}

fn cuttlefish_fallback_branch_target(_page_size: Option<usize>) -> Option<(String, String)> {
    match std::env::consts::ARCH {
        "aarch64" => Some((
            "aosp-main-throttled".into(),
            "aosp_cf_arm64_only_phone-trunk_staging-userdebug".into(),
        )),
        "riscv64" => Some((
            "aosp-main".into(),
            "aosp_cf_riscv64_phone-trunk_staging-userdebug".into(),
        )),
        _ => Some((
            "aosp-main".into(),
            "aosp_cf_x86_64_phone-trunk_staging-userdebug".into(),
        )),
    }
}

fn cuttlefish_build_id_override() -> Option<String> {
    read_env_trimmed("APKW_CUTTLEFISH_BUILD_ID")
}

fn cuttlefish_images_ready(images_dir: &Path) -> bool {
    images_dir.join("system.img").exists()
        || images_dir.join("super.img").exists()
        || images_dir.join("boot.img").exists()
}

fn resolve_cuttlefish_images_dir(page_size: Option<usize>) -> (PathBuf, Option<PathBuf>) {
    let configured = cuttlefish_images_dir(page_size);
    if cuttlefish_images_ready(&configured) {
        return (configured, None);
    }
    if let Some(size) = page_size {
        let suffix = page_size_label(size).to_lowercase();
        let nested = configured.join(&suffix);
        if nested != configured && cuttlefish_images_ready(&nested) {
            return (nested, Some(configured));
        }
    }
    let default_dir = cuttlefish_home_dir(page_size);
    if default_dir != configured && cuttlefish_images_ready(&default_dir) {
        return (default_dir, Some(configured));
    }
    (configured, None)
}

fn read_env_trimmed(key: &str) -> Option<String> {
    let value = std::env::var(key).ok()?;
    let trimmed = value.trim();
    if trimmed.is_empty() {
        None
    } else {
        Some(trimmed.to_string())
    }
}

fn page_size_label(size: usize) -> &'static str {
    if size > 4096 {
        "16K"
    } else {
        "4K"
    }
}

fn shell_escape(value: &str) -> String {
    if value.is_empty() {
        "''".to_string()
    } else {
        format!("'{}'", value.replace('\'', "'\"'\"'"))
    }
}

fn cuttlefish_product_from_target(target: &str) -> String {
    target.split('-').next().unwrap_or(target).to_string()
}

fn cuttlefish_branch_grid_url(branch: &str) -> String {
    format!("https://ci.android.com/builds/branches/{branch}/grid")
}

fn extract_js_variables_payload(html: &str) -> Option<String> {
    let marker = "var JSVariables = ";
    let start = html.find(marker)? + marker.len();
    let mut depth = 0;
    let mut in_string = false;
    let mut escape = false;

    for (idx, ch) in html[start..].char_indices() {
        if in_string {
            if escape {
                escape = false;
            } else if ch == '\\' {
                escape = true;
            } else if ch == '"' {
                in_string = false;
            }
            continue;
        }

        match ch {
            '"' => in_string = true,
            '{' => depth += 1,
            '}' => {
                if depth > 0 {
                    depth -= 1;
                }
                if depth == 0 {
                    return Some(html[start..start + idx + 1].to_string());
                }
            }
            _ => {}
        }
    }
    None
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct CiBranchGrid {
    builds: Vec<CiBuild>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct CiBuild {
    build_id: String,
    targets: Vec<CiBuildTarget>,
}

#[derive(Debug, Deserialize)]
struct CiBuildTarget {
    target: CiTargetInfo,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct CiTargetInfo {
    name: String,
    build_commands: Vec<String>,
    product: Option<String>,
}

fn parse_target_product_from_commands(commands: &[String]) -> Option<String> {
    for command in commands {
        if let Some(idx) = command.find("TARGET_PRODUCT=") {
            let rest = &command[idx + "TARGET_PRODUCT=".len()..];
            let end = rest
                .find(|ch: char| ch.is_whitespace())
                .unwrap_or(rest.len());
            let candidate = rest[..end].trim_matches('"').trim();
            if !candidate.is_empty() {
                return Some(candidate.to_string());
            }
        }
    }
    None
}

async fn fetch_branch_grid(branch: &str) -> Result<CiBranchGrid, String> {
    let url = cuttlefish_branch_grid_url(branch);
    let cmd = format!("curl -fsSL {}", shell_escape(&url));
    let (success, code, stdout, stderr) = run_shell_command_raw(&cmd)
        .await
        .map_err(|e| e.to_string())?;
    if !success {
        let detail = if stderr.trim().is_empty() {
            format!("exit_code={code}")
        } else {
            stderr.trim().to_string()
        };
        return Err(format!("failed to query CI grid: {detail}"));
    }
    let payload = extract_js_variables_payload(&stdout)
        .ok_or_else(|| "failed to locate JSVariables in CI grid".to_string())?;
    serde_json::from_str(&payload).map_err(|e| format!("invalid CI grid payload: {e}"))
}

pub(crate) struct CuttlefishBuildInfo {
    pub(crate) build_id: String,
    pub(crate) product: String,
}

#[derive(Clone, Debug)]
pub(crate) struct CuttlefishInstallOptions {
    pub(crate) force: bool,
    pub(crate) branch: Option<String>,
    pub(crate) target: Option<String>,
    pub(crate) build_id: Option<String>,
}

pub(crate) struct CuttlefishRequestConfig {
    pub(crate) branch: String,
    pub(crate) target: String,
    pub(crate) build_id_override: Option<String>,
    pub(crate) has_branch_override: bool,
    pub(crate) has_target_override: bool,
}

fn normalize_override(value: Option<String>) -> Option<String> {
    value
        .map(|value| value.trim().to_string())
        .filter(|value| !value.is_empty())
}

pub(crate) fn resolve_cuttlefish_request_config(
    page_size: Option<usize>,
    branch_override: Option<String>,
    target_override: Option<String>,
    build_id_override: Option<String>,
) -> CuttlefishRequestConfig {
    let env_branch_override =
        cuttlefish_env_for_page_size("APKW_CUTTLEFISH_BRANCH", page_size).is_some();
    let env_target_override =
        cuttlefish_env_for_page_size("APKW_CUTTLEFISH_TARGET", page_size).is_some();

    let branch_override = normalize_override(branch_override);
    let target_override = normalize_override(target_override);
    let build_id_override =
        normalize_override(build_id_override).or_else(cuttlefish_build_id_override);

    let branch = branch_override
        .clone()
        .unwrap_or_else(|| cuttlefish_branch(page_size));
    let target = target_override
        .clone()
        .unwrap_or_else(|| cuttlefish_target(page_size));

    CuttlefishRequestConfig {
        branch,
        target,
        build_id_override,
        has_branch_override: branch_override.is_some() || env_branch_override,
        has_target_override: target_override.is_some() || env_target_override,
    }
}

pub(crate) async fn resolve_build_info(
    branch: &str,
    target: &str,
    build_id_override: Option<String>,
) -> Result<CuttlefishBuildInfo, String> {
    if let Some(build_id) = build_id_override {
        let mut resolved_product = None;
        if let Ok(grid) = fetch_branch_grid(branch).await {
            for build in grid.builds {
                if build.build_id != build_id {
                    continue;
                }
                if let Some(target_info) = build
                    .targets
                    .iter()
                    .find(|entry| entry.target.name == target)
                {
                    resolved_product =
                        parse_target_product_from_commands(&target_info.target.build_commands)
                            .or_else(|| target_info.target.product.clone());
                }
                break;
            }
        }
        let product = resolved_product.unwrap_or_else(|| cuttlefish_product_from_target(target));
        return Ok(CuttlefishBuildInfo { build_id, product });
    }

    let grid = fetch_branch_grid(branch).await?;
    let mut last_err = None;

    for build in grid.builds {
        let Some(target_info) = build
            .targets
            .iter()
            .find(|entry| entry.target.name == target)
        else {
            continue;
        };

        let product = parse_target_product_from_commands(&target_info.target.build_commands)
            .or_else(|| target_info.target.product.clone())
            .unwrap_or_else(|| cuttlefish_product_from_target(target));

        let target_paths = candidate_target_paths(target, &product);
        let img_candidates =
            cuttlefish_image_artifact_candidates(&product, target, &build.build_id);
        let host_candidates = cuttlefish_host_artifact_candidates(&build.build_id);

        if let Err(err) =
            resolve_artifact_url_for_targets(&build.build_id, &target_paths, &img_candidates).await
        {
            last_err = Some(err);
            continue;
        }

        if let Err(err) =
            resolve_artifact_url_for_targets(&build.build_id, &target_paths, &host_candidates).await
        {
            last_err = Some(err);
            continue;
        }

        return Ok(CuttlefishBuildInfo {
            build_id: build.build_id,
            product,
        });
    }

    if let Some(err) = last_err {
        return Err(err);
    }

    Err(format!(
        "no builds found for target {target} on branch {branch}"
    ))
}

fn cuttlefish_image_artifact_candidates(
    product: &str,
    target: &str,
    build_id: &str,
) -> Vec<String> {
    vec![
        format!("{product}-img-{build_id}.zip"),
        format!("{target}-img-{build_id}.zip"),
        format!("{product}-{build_id}.zip"),
        format!("{target}-{build_id}.zip"),
    ]
}

fn cuttlefish_host_artifact_candidates(build_id: &str) -> Vec<String> {
    vec![
        "cvd-host_package.tar.gz".to_string(),
        format!("cvd-host_package-{build_id}.tar.gz"),
    ]
}

fn candidate_target_paths(target: &str, product: &str) -> Vec<String> {
    let mut out = Vec::new();
    let target = target.trim();
    if !target.is_empty() {
        out.push(target.to_string());
    }
    let product = product.trim();
    if !product.is_empty() && product != target {
        out.push(product.to_string());
    }
    out
}

fn headers_look_like_html(headers: &str) -> bool {
    let lower = headers.to_ascii_lowercase();
    lower.contains("content-type: text/html")
        || lower.contains("content-type: text/plain")
        || lower.contains("content-type: application/json")
}

fn body_looks_like_html(body: &str) -> bool {
    let trimmed = body.trim_start().to_ascii_lowercase();
    trimmed.starts_with("<!doctype html") || trimmed.starts_with("<html")
}

async fn artifact_url_is_downloadable(url: &str) -> bool {
    let head_cmd = format!("curl -fsSIL {}", shell_escape(url));
    if let Ok((true, _, stdout, _)) = run_shell_command_raw(&head_cmd).await {
        if headers_look_like_html(&stdout) {
            return false;
        }
    }

    let range_cmd = format!("curl -fsSL --range 0-200 {}", shell_escape(url));
    if let Ok((true, _, stdout, _)) = run_shell_command_raw(&range_cmd).await {
        return !body_looks_like_html(&stdout);
    }

    false
}

#[derive(Debug, Deserialize)]
struct ArtifactViewerVariables {
    #[serde(rename = "artifactUrl")]
    artifact_url: Option<String>,
}

fn extract_artifact_url_from_viewer(html: &str) -> Option<String> {
    let payload = extract_js_variables_payload(html)?;
    let parsed: ArtifactViewerVariables = serde_json::from_str(&payload).ok()?;
    parsed.artifact_url
}

async fn resolve_artifact_url_via_viewer(
    build_id: &str,
    target: &str,
    artifact: &str,
) -> Option<String> {
    let url =
        format!("https://ci.android.com/builds/submitted/{build_id}/{target}/latest/{artifact}");
    let cmd = format!("curl -fsSL {}", shell_escape(&url));
    let (ok, _, stdout, _) = run_shell_command_raw(&cmd).await.ok()?;
    if !ok {
        return None;
    }
    let artifact_url = extract_artifact_url_from_viewer(&stdout)?;
    let trimmed = artifact_url.trim();
    if trimmed.is_empty() {
        return None;
    }
    if artifact_url_is_downloadable(trimmed).await {
        return Some(trimmed.to_string());
    }
    None
}

async fn resolve_artifact_url(
    build_id: &str,
    target: &str,
    artifacts: &[String],
) -> Result<String, String> {
    let bases = [
        format!(
            "https://android-ci.googleusercontent.com/builds/submitted/{build_id}/{target}/latest/raw/"
        ),
        format!(
            "https://android-ci.googleusercontent.com/builds/submitted/{build_id}/{target}/latest/"
        ),
        format!("https://ci.android.com/builds/submitted/{build_id}/{target}/latest/raw/"),
        format!("https://ci.android.com/builds/submitted/{build_id}/{target}/latest/"),
    ];

    for artifact in artifacts {
        for base in &bases {
            let url = format!("{base}{artifact}");
            if artifact_url_is_downloadable(&url).await {
                return Ok(url);
            }
        }
        if let Some(url) = resolve_artifact_url_via_viewer(build_id, target, artifact).await {
            return Ok(url);
        }
    }

    Err(format!(
        "unable to resolve artifact url for build_id={build_id}, target={target}"
    ))
}

async fn resolve_artifact_url_for_targets(
    build_id: &str,
    target_paths: &[String],
    artifacts: &[String],
) -> Result<String, String> {
    let mut last_err = None;
    for target in target_paths {
        match resolve_artifact_url(build_id, target, artifacts).await {
            Ok(url) if !url.trim().is_empty() => return Ok(url),
            Ok(_) => {
                last_err = Some(format!(
                    "empty artifact url for build_id={build_id}, target={target}"
                ));
            }
            Err(err) => last_err = Some(err),
        }
    }
    Err(last_err
        .unwrap_or_else(|| format!("unable to resolve artifact url for build_id={build_id}")))
}

fn cuttlefish_cvd_path() -> Option<PathBuf> {
    find_command(&cuttlefish_cvd_bin())
}

fn cuttlefish_launch_path(page_size: Option<usize>) -> Option<PathBuf> {
    if let Some(path) = find_command(&cuttlefish_launch_bin()) {
        return Some(path);
    }
    let candidate = cuttlefish_host_dir(page_size)
        .join("bin")
        .join(cuttlefish_launch_bin());
    candidate.is_file().then_some(candidate)
}

fn cuttlefish_stop_path(page_size: Option<usize>) -> Option<PathBuf> {
    if let Some(path) = find_command(&cuttlefish_stop_bin()) {
        return Some(path);
    }
    let candidate = cuttlefish_host_dir(page_size)
        .join("bin")
        .join(cuttlefish_stop_bin());
    candidate.is_file().then_some(candidate)
}

fn cuttlefish_home_env_prefix(home: &Path) -> String {
    let home_str = home.to_string_lossy();
    format!("HOME={} ", shell_escape(home_str.as_ref()))
}

fn webrtc_custom_css_path(images_dir: &Path) -> PathBuf {
    images_dir
        .join("usr")
        .join("share")
        .join("webrtc")
        .join("assets")
        .join("custom.css")
}

fn ensure_nonempty_webrtc_custom_css(images_dir: &Path) -> io::Result<bool> {
    let path = webrtc_custom_css_path(images_dir);
    let metadata = match fs::metadata(&path) {
        Ok(metadata) => metadata,
        Err(err) if err.kind() == io::ErrorKind::NotFound => return Ok(false),
        Err(err) => return Err(err),
    };
    if !metadata.is_file() || metadata.len() > 0 {
        return Ok(false);
    }
    fs::write(
        &path,
        "/* APKW: avoid empty custom.css TLS EOF responses */\n",
    )?;
    Ok(true)
}

fn proc_state(pid: u32) -> Option<char> {
    let stat = fs::read_to_string(format!("/proc/{pid}/stat")).ok()?;
    stat.split_whitespace().nth(2)?.chars().next()
}

fn proc_cmdline(pid: u32) -> Option<String> {
    let raw = fs::read(format!("/proc/{pid}/cmdline")).ok()?;
    if raw.is_empty() {
        return None;
    }
    let parts: Vec<String> = raw
        .split(|b| *b == 0)
        .filter(|part| !part.is_empty())
        .map(|part| String::from_utf8_lossy(part).to_string())
        .collect();
    if parts.is_empty() {
        None
    } else {
        Some(parts.join(" "))
    }
}

fn cuttlefish_runtime_processes(system_image_dir: &Path) -> Vec<u32> {
    let mut pids = Vec::new();
    let image_dir = system_image_dir.display().to_string();
    let entries = match fs::read_dir("/proc") {
        Ok(entries) => entries,
        Err(_) => return pids,
    };
    for entry in entries.flatten() {
        let Some(pid_str) = entry.file_name().to_str().map(|value| value.to_string()) else {
            continue;
        };
        let Ok(pid) = pid_str.parse::<u32>() else {
            continue;
        };
        if matches!(proc_state(pid), Some('Z')) {
            continue;
        }
        let Some(cmdline) = proc_cmdline(pid) else {
            continue;
        };
        if !(cmdline.contains("run_cvd") || cmdline.contains("launch_cvd")) {
            continue;
        }
        if !cmdline.contains(&image_dir) {
            continue;
        }
        pids.push(pid);
    }
    pids.sort_unstable();
    pids
}

fn format_pid_list(pids: &[u32], max_items: usize) -> String {
    let shown = pids
        .iter()
        .take(max_items)
        .map(|pid| pid.to_string())
        .collect::<Vec<_>>();
    if pids.len() > max_items {
        format!("{},+{}", shown.join(","), pids.len() - max_items)
    } else {
        shown.join(",")
    }
}

pub(crate) async fn cuttlefish_status() -> Result<CuttlefishStatus, CuttlefishStatusError> {
    let page_size = host_page_size();
    let home_dir = cuttlefish_home_dir(page_size);
    let (images_dir, _) = resolve_cuttlefish_images_dir(page_size);
    let runtime_pids = cuttlefish_runtime_processes(&images_dir);
    let cvd_path = cuttlefish_cvd_path();
    let launch_path = cuttlefish_launch_path(page_size);
    let host_tools_issue = cuttlefish_host_tools_issue(page_size);
    if cvd_path.is_none() && launch_path.is_none() {
        return Err(CuttlefishStatusError::NotInstalled);
    }

    let mut status = CuttlefishStatus {
        adb_serial: cuttlefish_adb_serial(),
        ..Default::default()
    };
    if !runtime_pids.is_empty() {
        status.running = true;
        status.details.push((
            "process_probe".into(),
            "run_cvd/launch_cvd process detected".into(),
        ));
        status
            .details
            .push(("process_pids".into(), format_pid_list(&runtime_pids, 8)));
    }
    if let Some(issue) = host_tools_issue.as_ref() {
        status
            .details
            .push(("host_tools_issue".into(), issue.clone()));
        if runtime_pids.is_empty() {
            return Err(CuttlefishStatusError::Failed(format!(
                "host tools incomplete: {issue}"
            )));
        }
    }

    let Some(cvd_path) = cvd_path else {
        return Ok(status);
    };

    let host_dir = cuttlefish_host_dir(page_size);
    let mut cmd = Command::new(&cvd_path);
    cmd.arg("status")
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .env("HOME", &home_dir);
    if host_dir.is_dir() {
        cmd.current_dir(&host_dir);
    }
    let output = cmd.output().await.map_err(|e| {
        if e.kind() == io::ErrorKind::NotFound {
            CuttlefishStatusError::NotInstalled
        } else {
            CuttlefishStatusError::Failed(e.to_string())
        }
    })?;

    if !output.status.success() {
        let stdout = String::from_utf8_lossy(&output.stdout).trim().to_string();
        let stderr = String::from_utf8_lossy(&output.stderr).trim().to_string();
        let exit_code = output.status.code().unwrap_or(-1);
        if stderr.to_lowercase().contains("not applicable: no device") {
            return Ok(status);
        }
        let mut message = String::new();
        if !stdout.is_empty() {
            message.push_str("stdout:\n");
            message.push_str(&stdout);
            message.push('\n');
        }
        if !stderr.is_empty() {
            message.push_str("stderr:\n");
            message.push_str(&stderr);
            message.push('\n');
        }
        if message.trim().is_empty() {
            message = format!("exit_code={exit_code}");
        }
        return Err(CuttlefishStatusError::Failed(message.trim().to_string()));
    }

    let stdout = String::from_utf8_lossy(&output.stdout);
    status.raw = stdout.trim().to_string();

    for line in stdout.lines() {
        let line = line.trim();
        if line.is_empty() {
            continue;
        }
        let lower = line.to_lowercase();
        if lower.contains("running") && !lower.contains("not running") {
            status.running = true;
        }
        if let Some((key, value)) = line.split_once(':') {
            let key = key.trim().to_lowercase().replace(' ', "_");
            let value = value.trim().to_string();
            if key == "adb" || key == "adb_serial" || key == "adb_address" {
                status.adb_serial = value.clone();
            }
            status.details.push((key, value));
        }
    }

    Ok(status)
}

pub(crate) async fn maybe_cuttlefish_target(
    adb_targets: &mut Vec<Target>,
    include_offline: bool,
) -> Result<Option<Target>, Status> {
    if !cuttlefish_enabled() {
        return Ok(None);
    }

    let mut status = CuttlefishStatus::default();
    let mut status_error = None;
    match cuttlefish_status().await {
        Ok(found) => status = found,
        Err(CuttlefishStatusError::NotInstalled) => return Ok(None),
        Err(CuttlefishStatusError::Failed(err)) => {
            warn!("Cuttlefish status failed: {}", err);
            status_error = Some(err);
        }
    };

    let page_size = host_page_size();
    let adb_serial_config = cuttlefish_adb_serial();
    let adb_serial_config_normalized = normalize_target_id(&adb_serial_config);
    let normalized_config = normalize_target_id_for_compare(&adb_serial_config);
    let mut adb_serial = if status.adb_serial.is_empty() {
        adb_serial_config.clone()
    } else {
        status.adb_serial.clone()
    };

    let mut adb_entry = None;
    if let Some(index) = adb_targets.iter().position(|t| {
        t.target_id
            .as_ref()
            .map(|i| normalize_target_id_for_compare(&i.value) == normalized_config)
            .unwrap_or(false)
    }) {
        adb_entry = Some(adb_targets.remove(index));
        if let Some(id) = adb_entry
            .as_ref()
            .and_then(|entry| entry.target_id.as_ref())
        {
            adb_serial = id.value.clone();
        }
    }
    let adb_serial_normalized = normalize_target_id(&adb_serial);
    let adb_serial_canonical = canonicalize_adb_serial(&adb_serial_normalized);

    let status_running = status_error.is_none() && status.running;
    let connect_enabled = cuttlefish_connect_enabled();

    let mut details = vec![
        KeyValue {
            key: "cvd_bin".into(),
            value: cuttlefish_cvd_bin(),
        },
        KeyValue {
            key: "launch_cvd_bin".into(),
            value: cuttlefish_launch_bin(),
        },
        KeyValue {
            key: "adb_path".into(),
            value: adb_path().display().to_string(),
        },
        KeyValue {
            key: "adb_serial".into(),
            value: adb_serial_normalized.clone(),
        },
    ];
    if adb_serial_normalized != adb_serial_config_normalized {
        details.push(KeyValue {
            key: "adb_serial_config".into(),
            value: adb_serial_config_normalized.clone(),
        });
    }
    if let Some(size) = page_size {
        details.push(KeyValue {
            key: "host_page_size".into(),
            value: size.to_string(),
        });
    }
    let kvm = kvm_status();
    details.push(KeyValue {
        key: "kvm_present".into(),
        value: kvm.present.to_string(),
    });
    details.push(KeyValue {
        key: "kvm_access".into(),
        value: kvm.accessible.to_string(),
    });
    details.push(KeyValue {
        key: "kvm_check_enabled".into(),
        value: cuttlefish_kvm_check_enabled().to_string(),
    });
    if let Some(detail) = kvm.detail {
        details.push(KeyValue {
            key: "kvm_detail".into(),
            value: detail,
        });
    }
    details.push(KeyValue {
        key: "cuttlefish_running".into(),
        value: status_running.to_string(),
    });
    details.push(KeyValue {
        key: "cuttlefish_connect_enabled".into(),
        value: connect_enabled.to_string(),
    });
    details.push(KeyValue {
        key: "cuttlefish_home".into(),
        value: cuttlefish_home_dir(page_size).display().to_string(),
    });
    details.push(KeyValue {
        key: "cuttlefish_images_dir".into(),
        value: cuttlefish_images_dir(page_size).display().to_string(),
    });
    details.push(KeyValue {
        key: "cuttlefish_host_dir".into(),
        value: cuttlefish_host_dir(page_size).display().to_string(),
    });
    details.push(KeyValue {
        key: "cuttlefish_host_tools_ready".into(),
        value: cuttlefish_host_tools_ready(page_size).to_string(),
    });
    if let Some(path) = cuttlefish_capability_query_path() {
        details.push(KeyValue {
            key: "cuttlefish_capability_query_path".into(),
            value: path.display().to_string(),
        });
    }
    if let Some(issue) = cuttlefish_host_tools_issue(page_size) {
        details.push(KeyValue {
            key: "cuttlefish_host_tools_issue".into(),
            value: issue,
        });
    }
    details.push(KeyValue {
        key: "cuttlefish_branch".into(),
        value: cuttlefish_branch(page_size),
    });
    details.push(KeyValue {
        key: "cuttlefish_target".into(),
        value: cuttlefish_target(page_size),
    });
    if let Some(mode) = cuttlefish_gpu_mode() {
        details.push(KeyValue {
            key: "cuttlefish_gpu_mode".into(),
            value: mode,
        });
    }
    if let Some(build_id) = cuttlefish_build_id_override() {
        details.push(KeyValue {
            key: "cuttlefish_build_id".into(),
            value: build_id,
        });
    }
    details.push(KeyValue {
        key: "cuttlefish_webrtc_url".into(),
        value: cuttlefish_web_url(),
    });
    details.push(KeyValue {
        key: "cuttlefish_env_url".into(),
        value: cuttlefish_env_url(),
    });
    for (key, value) in &status.details {
        details.push(KeyValue {
            key: format!("cuttlefish_{key}"),
            value: value.clone(),
        });
    }
    if let Some(err) = status_error.as_ref() {
        details.push(KeyValue {
            key: "cuttlefish_status_error".into(),
            value: err.clone(),
        });
    }
    if !status.raw.is_empty() {
        details.push(KeyValue {
            key: "cuttlefish_status_raw".into(),
            value: status.raw.clone(),
        });
    }

    let should_connect = status_running
        && adb_entry.is_none()
        && connect_enabled
        && adb_serial_canonical.contains(':');
    if should_connect {
        if let Some(msg) = adb_connect(&adb_serial_canonical).await {
            details.push(KeyValue {
                key: "adb_connect".into(),
                value: msg,
            });
        }
    } else if status_error.is_some() {
        details.push(KeyValue {
            key: "adb_connect_status".into(),
            value: "skipped (cuttlefish status error)".into(),
        });
    } else if !connect_enabled {
        details.push(KeyValue {
            key: "adb_connect_status".into(),
            value: "skipped (APKW_CUTTLEFISH_CONNECT=0)".into(),
        });
    } else if adb_entry.is_some() {
        details.push(KeyValue {
            key: "adb_connect_status".into(),
            value: "skipped (already listed)".into(),
        });
    } else {
        details.push(KeyValue {
            key: "adb_connect_status".into(),
            value: "skipped (cuttlefish not running)".into(),
        });
    }

    let mut api_level = String::new();
    let mut release = String::new();
    let adb_state = if let Some(entry) = &adb_entry {
        details.extend(entry.details.clone());
        details.push(KeyValue {
            key: "adb_state".into(),
            value: entry.state.clone(),
        });
        Some(entry.state.clone())
    } else if connect_enabled {
        match adb_get_state(&adb_serial_canonical).await {
            Ok(state) => {
                details.push(KeyValue {
                    key: "adb_state".into(),
                    value: state.clone(),
                });
                Some(state)
            }
            Err(err) => {
                details.push(KeyValue {
                    key: "adb_state_error".into(),
                    value: adb_failure_message(&err),
                });
                None
            }
        }
    } else {
        None
    };

    if adb_state.as_deref() == Some("device") {
        if let Ok(value) = adb_get_prop(&adb_serial_canonical, "ro.build.version.sdk").await {
            api_level = value.clone();
            details.push(KeyValue {
                key: "api_level".into(),
                value,
            });
        }
        if let Ok(value) = adb_get_prop(&adb_serial_canonical, "ro.build.version.release").await {
            release = value.clone();
            details.push(KeyValue {
                key: "android_release".into(),
                value,
            });
        }
        if let Some(abi) = adb_get_prop_timeout(&adb_serial_canonical, "ro.product.cpu.abi").await {
            details.push(KeyValue {
                key: "abi".into(),
                value: abi,
            });
        }
        if let Some(abi_list) =
            adb_get_prop_timeout(&adb_serial_canonical, "ro.product.cpu.abilist").await
        {
            details.push(KeyValue {
                key: "abi_list".into(),
                value: abi_list,
            });
        }
    }

    let mut state = if let Some(state) = adb_state {
        state
    } else if status_running {
        "running".into()
    } else if status_error.is_some() {
        "error".into()
    } else {
        "stopped".into()
    };
    if !include_offline && state != "device" && state != "running" {
        return Ok(None);
    }

    if state == "device" && release.is_empty() && !status_running {
        state = "offline".into();
    }

    let health_state = if state == "device" {
        "online"
    } else if state == "running" {
        "booting"
    } else if state == "error" {
        "error"
    } else if state == "offline" || state == "unauthorized" {
        health_state_from_adb_state(&state)
    } else {
        "stopped"
    };
    details.push(KeyValue {
        key: "health_state".into(),
        value: health_state.to_string(),
    });

    let display_name = adb_entry
        .as_ref()
        .map(|entry| entry.display_name.clone())
        .unwrap_or_else(|| "Cuttlefish (local)".into());

    Ok(Some(Target {
        target_id: Some(Id {
            value: adb_serial_normalized.clone(),
        }),
        kind: TargetKind::Emulatorlike as i32,
        display_name,
        provider: "cuttlefish".into(),
        address: adb_serial_normalized,
        api_level,
        state,
        details,
    }))
}

fn is_cuttlefish_package_missing(detail: &str) -> bool {
    let lower = detail.to_lowercase();
    lower.contains("unable to locate package cuttlefish")
        || lower.contains("cuttlefish-base")
        || lower.contains("cuttlefish-user")
        || lower.contains("no installation candidate")
}

fn classify_install_error(detail: &str) -> ErrorCode {
    if is_cuttlefish_package_missing(detail) {
        return ErrorCode::NotFound;
    }
    let lower = detail.to_lowercase();
    if lower.contains("permission denied")
        || lower.contains("a password is required")
        || (lower.contains("sudo") && lower.contains("password"))
        || lower.contains("not authorized")
        || lower.contains("authentication failed")
    {
        ErrorCode::PermissionDenied
    } else {
        ErrorCode::Internal
    }
}

async fn run_shell_command(command: &str) -> Result<(bool, i32, String), io::Error> {
    run_shell_command_with_timeout(command, None).await
}

async fn run_shell_command_with_timeout(
    command: &str,
    timeout: Option<Duration>,
) -> Result<(bool, i32, String), io::Error> {
    let (success, code, stdout, stderr) = run_shell_command_inner(command, None, timeout).await?;
    let log = format_adb_output(&stdout, &stderr);
    Ok((success, code, log))
}

async fn run_shell_command_in_dir(
    command: &str,
    dir: &Path,
) -> Result<(bool, i32, String), io::Error> {
    run_shell_command_in_dir_with_timeout(command, dir, None).await
}

async fn run_shell_command_in_dir_with_timeout(
    command: &str,
    dir: &Path,
    timeout: Option<Duration>,
) -> Result<(bool, i32, String), io::Error> {
    let (success, code, stdout, stderr) =
        run_shell_command_inner(command, Some(dir), timeout).await?;
    let log = format_adb_output(&stdout, &stderr);
    Ok((success, code, log))
}

async fn run_shell_command_raw(command: &str) -> Result<(bool, i32, String, String), io::Error> {
    run_shell_command_inner(command, None, None).await
}

async fn run_shell_command_inner(
    command: &str,
    dir: Option<&Path>,
    timeout: Option<Duration>,
) -> Result<(bool, i32, String, String), io::Error> {
    let mut cmd = Command::new("sh");
    cmd.arg("-lc")
        .arg(command)
        .env("DEBIAN_FRONTEND", "noninteractive")
        .env("APT_LISTCHANGES_FRONTEND", "none")
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    if timeout.is_some() {
        cmd.kill_on_drop(true);
    }
    if let Some(dir) = dir {
        cmd.current_dir(dir);
    }
    let output = match timeout {
        Some(timeout) => match tokio::time::timeout(timeout, cmd.output()).await {
            Ok(output) => output?,
            Err(_) => {
                return Ok((
                    false,
                    124,
                    String::new(),
                    format!("command timed out after {}s", timeout.as_secs()),
                ));
            }
        },
        None => cmd.output().await?,
    };

    let stdout = String::from_utf8_lossy(&output.stdout).to_string();
    let stderr = String::from_utf8_lossy(&output.stderr).to_string();
    let code = output.status.code().unwrap_or(-1);
    Ok((output.status.success(), code, stdout, stderr))
}

async fn current_user_groups() -> Option<Vec<String>> {
    let (ok, _, stdout, _) = run_shell_command_raw("id -Gn").await.ok()?;
    if !ok {
        return None;
    }
    let groups: Vec<String> = stdout
        .split_whitespace()
        .map(|group| group.trim().to_string())
        .filter(|group| !group.is_empty())
        .collect();
    if groups.is_empty() {
        None
    } else {
        Some(groups)
    }
}

fn missing_groups(groups: &[String], required: &[&str]) -> Vec<String> {
    required
        .iter()
        .filter(|required| !groups.iter().any(|group| group == *required))
        .map(|group| group.to_string())
        .collect()
}

async fn run_diag_command(label: &str, command: &str) -> Option<String> {
    match run_shell_command(command).await {
        Ok((_, _, log)) => {
            if log.trim().is_empty() {
                None
            } else {
                Some(format!("{label}:\n{log}"))
            }
        }
        Err(_) => None,
    }
}

async fn collect_cuttlefish_diagnostics() -> String {
    let mut out = String::new();
    let page_size = host_page_size();
    if let Some(size) = page_size {
        out.push_str(&format!("host page size: {size}\n"));
    }
    out.push_str(&format!(
        "cuttlefish_home: {}\n",
        cuttlefish_home_dir(page_size).display()
    ));
    out.push_str(&format!(
        "cuttlefish_images_dir: {}\n",
        cuttlefish_images_dir(page_size).display()
    ));
    out.push_str(&format!(
        "cuttlefish_host_dir: {}\n\n",
        cuttlefish_host_dir(page_size).display()
    ));
    let kvm = kvm_status();
    out.push_str(&format!("kvm_present: {}\n", kvm.present));
    out.push_str(&format!("kvm_access: {}\n", kvm.accessible));
    if let Some(detail) = kvm.detail {
        out.push_str(&format!("kvm_detail: {}\n", detail.trim()));
    }
    out.push('\n');

    if let Some(cvd_path) = cuttlefish_cvd_path() {
        let cmd = format!("{} status", cvd_path.display());
        let host_dir = cuttlefish_host_dir(page_size);
        let result = if host_dir.is_dir() {
            run_shell_command_in_dir(&cmd, &host_dir).await
        } else {
            run_shell_command(&cmd).await
        };
        if let Ok((_, _, log)) = result {
            if !log.trim().is_empty() {
                out.push_str("cvd status:\n");
                out.push_str(&log);
                out.push_str("\n\n");
            }
        }
    }

    let adb_cmd = format!("{} devices -l", adb_path().display());
    if let Some(section) = run_diag_command("adb devices", &adb_cmd).await {
        out.push_str(&section);
        out.push_str("\n\n");
    }

    if let Some(section) = run_diag_command("kvm device", "ls -l /dev/kvm").await {
        out.push_str(&section);
        out.push_str("\n\n");
    }

    if let Some(section) = run_diag_command("groups", "id -nG").await {
        out.push_str(&section);
        out.push_str("\n\n");
    }

    if let Some(section) = run_diag_command("uname -a", "uname -a").await {
        out.push_str(&section);
        out.push('\n');
    }

    out.trim().to_string()
}

async fn append_cuttlefish_diagnostics(detail: &mut String) {
    let diagnostics = collect_cuttlefish_diagnostics().await;
    if diagnostics.is_empty() {
        return;
    }
    detail.push_str("\n\nDiagnostics:\n");
    detail.push_str(&diagnostics);
}

struct CuttlefishCommandOutcome {
    success: bool,
    exit_code: i32,
    log: String,
    timed_out: bool,
}

async fn run_cuttlefish_command(
    job_client: &mut JobServiceClient<Channel>,
    job_id: &str,
    command: &str,
    phase: &str,
    percent: u32,
    cwd: Option<&Path>,
    timeout: Option<Duration>,
) -> Result<CuttlefishCommandOutcome, ErrorDetail> {
    let mut metrics = vec![metric("command", command)];
    if let Some(dir) = cwd {
        metrics.push(metric("cwd", dir.display()));
    }
    if let Some(timeout) = timeout {
        metrics.push(metric("timeout_secs", timeout.as_secs()));
    }
    let _ = publish_progress(job_client, job_id, percent, phase, metrics).await;
    let _ = publish_log(job_client, job_id, &format!("Running: {command}\n")).await;

    let result = match cwd {
        Some(dir) => run_shell_command_in_dir_with_timeout(command, dir, timeout).await,
        None => run_shell_command_with_timeout(command, timeout).await,
    };
    match result {
        Ok((success, exit_code, log)) => {
            if !log.is_empty() {
                let _ = publish_log(job_client, job_id, &log).await;
            }
            Ok(CuttlefishCommandOutcome {
                success,
                exit_code,
                log,
                timed_out: exit_code == 124,
            })
        }
        Err(err) => {
            let code = if err.kind() == io::ErrorKind::NotFound {
                ErrorCode::NotFound
            } else {
                ErrorCode::Internal
            };
            Err(job_error_detail(
                code,
                "failed to run cuttlefish command",
                err.to_string(),
                job_id,
            ))
        }
    }
}

struct CuttlefishRuntime {
    page_size: Option<usize>,
    home_dir: PathBuf,
    images_dir: PathBuf,
    host_dir: PathBuf,
}

fn cleanup_cuttlefish_temp() {
    // Cuttlefish can leave stale vsock and instance sockets under /tmp if a previous run failed.
    // A subsequent launch may fail with "IsDirectoryEmpty test failed" unless these are removed.
    for path in ["/tmp/vsock_3_1000", "/tmp/cf_avd_1000"] {
        if let Ok(meta) = std::fs::metadata(path) {
            if meta.is_dir() {
                if let Err(err) = std::fs::remove_dir_all(path) {
                    warn!("failed to remove stale dir {}: {}", path, err);
                }
            } else if let Err(err) = std::fs::remove_file(path) {
                warn!("failed to remove stale file {}: {}", path, err);
            }
        }
    }
}

async fn enable_guest_bluetooth(adb_serial: &str) {
    let cmds = [
        "cmd bluetooth_manager enable",
        "settings put global bluetooth_on 1",
    ];
    for cmd in cmds {
        let _ = adb_shell(adb_serial, cmd).await;
    }
}

fn should_recover_bluetooth(log: &str) -> bool {
    let lower = log.to_lowercase();
    lower.contains("bluetooth")
        && (lower.contains("boot_failed")
            || lower.contains("boot pending")
            || lower.contains("dependencies not ready"))
}

fn should_recover_stale_instance(log: &str) -> bool {
    let lower = log.to_lowercase();
    lower.contains("instance directory files in use")
        || lower.contains("try `cvd reset`")
        || lower.contains("failed to clean prior files")
        || lower.contains("cleanpriorfiles")
}

fn env_timeout_secs(name: &str, default_secs: u64) -> Duration {
    read_env_trimmed(name)
        .and_then(|value| value.parse::<u64>().ok())
        .filter(|secs| *secs > 0)
        .map(Duration::from_secs)
        .unwrap_or_else(|| Duration::from_secs(default_secs))
}

fn cuttlefish_start_timeout() -> Duration {
    env_timeout_secs("APKW_CUTTLEFISH_START_CMD_TIMEOUT_SECS", 120)
}

fn cuttlefish_stop_timeout() -> Duration {
    env_timeout_secs("APKW_CUTTLEFISH_STOP_CMD_TIMEOUT_SECS", 60)
}

fn args_has_flag(args: &str, flag: &str) -> bool {
    let prefix = format!("{flag}=");
    args.split_whitespace()
        .any(|part| part == flag || part.starts_with(&prefix))
}

fn append_arg_once(mut command: String, arg: &str) -> String {
    if command.contains(arg) {
        return command;
    }
    if !command.is_empty() {
        command.push(' ');
    }
    command.push_str(arg);
    command
}

async fn cuttlefish_preflight(
    job_client: &mut JobServiceClient<Channel>,
    job_id: &str,
    require_kvm: bool,
    require_images: bool,
) -> Result<CuttlefishRuntime, ErrorDetail> {
    let page_size = host_page_size();
    let home_dir = cuttlefish_home_dir(page_size);
    let (images_dir, images_fallback) = resolve_cuttlefish_images_dir(page_size);
    let host_dir = cuttlefish_host_dir(page_size);
    let images_ready = cuttlefish_images_ready(&images_dir);

    if require_images {
        if let Some(original) = images_fallback.as_ref() {
            let _ = publish_log(
                job_client,
                job_id,
                &format!(
                    "Cuttlefish images dir fallback: {} -> {}\n",
                    original.display(),
                    images_dir.display()
                ),
            )
            .await;
        }
    }

    if let Some(size) = page_size {
        let _ = publish_log(job_client, job_id, &format!("Host page size: {size}\n")).await;
        if require_images && size > 4096 {
            if cuttlefish_page_size_check_enabled() {
                if !images_ready {
                    let _ = publish_log(
                        job_client,
                        job_id,
                        &format!(
                            "Cuttlefish images missing under {} (expected boot.img/super.img/system.img)\n",
                            images_dir.display()
                        ),
                    )
                    .await;
                    return Err(job_error_detail(
                        ErrorCode::Unavailable,
                        "missing 16K Cuttlefish images",
                        format!(
                            "page_size={size}; run Install Cuttlefish or set APKW_CUTTLEFISH_IMAGES_DIR_16K"
                        ),
                        job_id,
                    ));
                }
            } else {
                let _ = publish_log(
                    job_client,
                    job_id,
                    "Skipping 16K image check (APKW_CUTTLEFISH_PAGE_SIZE_CHECK=0)\n",
                )
                .await;
            }
        }
    }

    if require_kvm {
        if cuttlefish_kvm_check_enabled() {
            let status = kvm_status();
            let _ = publish_log(
                job_client,
                job_id,
                &format!(
                    "KVM check: present={} accessible={}\n",
                    status.present, status.accessible
                ),
            )
            .await;
            if !status.present {
                return Err(job_error_detail(
                    ErrorCode::Unavailable,
                    "KVM not available",
                    "missing /dev/kvm; enable virtualization or nested virtualization".into(),
                    job_id,
                ));
            }
            if !status.accessible {
                let detail = status
                    .detail
                    .unwrap_or_else(|| "failed to open /dev/kvm".into());
                return Err(job_error_detail(
                    ErrorCode::PermissionDenied,
                    "KVM access denied",
                    format!(
                        "{}; add the user to the kvm group and re-login",
                        detail.trim()
                    ),
                    job_id,
                ));
            }
        } else {
            let _ = publish_log(
                job_client,
                job_id,
                "Skipping KVM check (APKW_CUTTLEFISH_KVM_CHECK=0)\n",
            )
            .await;
        }
    }

    if require_images && !images_ready {
        let _ = publish_log(
            job_client,
            job_id,
            &format!(
                "Cuttlefish images missing under {} (expected boot.img/super.img/system.img)\n",
                images_dir.display()
            ),
        )
        .await;
        return Err(job_error_detail(
            ErrorCode::NotFound,
            "Cuttlefish images not found",
            format!(
                "missing images under {}; run Install Cuttlefish or set APKW_CUTTLEFISH_IMAGES_DIR",
                images_dir.display()
            ),
            job_id,
        ));
    }

    if let Some(issue) = cuttlefish_host_tools_issue(page_size) {
        let _ = publish_log(
            job_client,
            job_id,
            &format!("Cuttlefish host tools not ready: {issue}\n"),
        )
        .await;
        return Err(job_error_detail(
            ErrorCode::NotFound,
            "Cuttlefish host tools not ready",
            issue,
            job_id,
        ));
    }

    Ok(CuttlefishRuntime {
        page_size,
        home_dir,
        images_dir,
        host_dir,
    })
}

fn cuttlefish_start_command(
    runtime: &CuttlefishRuntime,
    show_full_ui: bool,
    disable_tap_devices: bool,
    resource_limits: CuttlefishResourceLimits,
    job_id: &str,
) -> Result<String, ErrorDetail> {
    if let Some(cmd) = read_env_trimmed("APKW_CUTTLEFISH_START_CMD") {
        return Ok(cmd);
    }

    let start_args = read_env_trimmed("APKW_CUTTLEFISH_START_ARGS");
    let mut extra_args = start_args.unwrap_or_default();
    if let Some(mode) = cuttlefish_gpu_mode() {
        if !args_has_flag(&extra_args, "--gpu_mode") {
            if !extra_args.is_empty() {
                extra_args.push(' ');
            }
            extra_args.push_str("--gpu_mode=");
            extra_args.push_str(&mode);
        }
    }
    if !args_has_flag(&extra_args, "--start_webrtc") {
        let start_webrtc = show_full_ui || !local_display_available();
        if !extra_args.is_empty() {
            extra_args.push(' ');
        }
        extra_args.push_str("--start_webrtc=");
        extra_args.push_str(if start_webrtc { "true" } else { "false" });
    }
    let include_usage_stats = !extra_args.contains("report_anonymous_usage_stats");
    if std::env::consts::ARCH == "aarch64" && !extra_args.contains("enable_host_bluetooth") {
        if !extra_args.is_empty() {
            extra_args.push(' ');
        }
        extra_args.push_str("--enable_host_bluetooth=true");
    }
    if disable_tap_devices && !args_has_flag(&extra_args, "--enable_tap_devices") {
        if !extra_args.is_empty() {
            extra_args.push(' ');
        }
        extra_args.push_str("--enable_tap_devices=false");
    }
    if let Some(cpus) = resource_limits.cpus {
        if !args_has_flag(&extra_args, "--cpus") {
            if !extra_args.is_empty() {
                extra_args.push(' ');
            }
            extra_args.push_str("--cpus=");
            extra_args.push_str(&cpus.to_string());
        }
    }
    if let Some(memory_mb) = resource_limits.memory_mb {
        if !args_has_flag(&extra_args, "--memory_mb") && !args_has_flag(&extra_args, "--mem") {
            if !extra_args.is_empty() {
                extra_args.push(' ');
            }
            extra_args.push_str("--memory_mb=");
            extra_args.push_str(&memory_mb.to_string());
        }
    }
    if let Some(x_res) = resource_limits.x_res {
        if !args_has_flag(&extra_args, "--x_res") {
            if !extra_args.is_empty() {
                extra_args.push(' ');
            }
            extra_args.push_str("--x_res=");
            extra_args.push_str(&x_res.to_string());
        }
    }
    if let Some(y_res) = resource_limits.y_res {
        if !args_has_flag(&extra_args, "--y_res") {
            if !extra_args.is_empty() {
                extra_args.push(' ');
            }
            extra_args.push_str("--y_res=");
            extra_args.push_str(&y_res.to_string());
        }
    }
    if let Some(dpi) = resource_limits.dpi {
        if !args_has_flag(&extra_args, "--dpi") {
            if !extra_args.is_empty() {
                extra_args.push(' ');
            }
            extra_args.push_str("--dpi=");
            extra_args.push_str(&dpi.to_string());
        }
    }
    if let Some(launch_path) = cuttlefish_launch_path(runtime.page_size) {
        let mut command = format!(
            "{}{} --daemon",
            cuttlefish_home_env_prefix(&runtime.home_dir),
            shell_escape(&launch_path.display().to_string())
        );
        command.push_str(&format!(
            " --system_image_dir={}",
            shell_escape(&runtime.images_dir.display().to_string())
        ));
        if include_usage_stats {
            command.push_str(" --report_anonymous_usage_stats=n");
        }
        if !extra_args.is_empty() {
            command.push(' ');
            command.push_str(&extra_args);
        }
        return Ok(command);
    }

    if let Some(cvd_path) = cuttlefish_cvd_path() {
        let mut command = format!(
            "{}{} create --host_path={} --product_path={}",
            cuttlefish_home_env_prefix(&runtime.home_dir),
            shell_escape(&cvd_path.display().to_string()),
            shell_escape(&runtime.host_dir.display().to_string()),
            shell_escape(&runtime.images_dir.display().to_string())
        );
        if include_usage_stats {
            command.push_str(" --report_anonymous_usage_stats=n");
        }
        if !extra_args.is_empty() {
            command.push(' ');
            command.push_str(&extra_args);
        }
        return Ok(command);
    }

    Err(job_error_detail(
        ErrorCode::NotFound,
        "no cuttlefish start command available",
        "set APKW_CUTTLEFISH_START_CMD or install Cuttlefish host tools".into(),
        job_id,
    ))
}

fn cuttlefish_stop_command(
    runtime: &CuttlefishRuntime,
    job_id: &str,
) -> Result<String, ErrorDetail> {
    if let Some(cmd) = read_env_trimmed("APKW_CUTTLEFISH_STOP_CMD") {
        return Ok(cmd);
    }

    if let Some(stop_path) = cuttlefish_stop_path(runtime.page_size) {
        return Ok(format!(
            "{}{}",
            cuttlefish_home_env_prefix(&runtime.home_dir),
            shell_escape(&stop_path.display().to_string())
        ));
    }

    if let Some(cvd_path) = cuttlefish_cvd_path() {
        return Ok(format!(
            "{}{} stop",
            cuttlefish_home_env_prefix(&runtime.home_dir),
            shell_escape(&cvd_path.display().to_string())
        ));
    }

    Err(job_error_detail(
        ErrorCode::NotFound,
        "no cuttlefish stop command available",
        "set APKW_CUTTLEFISH_STOP_CMD or install Cuttlefish host tools".into(),
        job_id,
    ))
}

fn cuttlefish_reset_command(
    runtime: &CuttlefishRuntime,
    job_id: &str,
) -> Result<String, ErrorDetail> {
    if let Some(cmd) = read_env_trimmed("APKW_CUTTLEFISH_RESET_CMD") {
        return Ok(cmd);
    }

    if let Some(cvd_path) = cuttlefish_cvd_path() {
        return Ok(format!(
            "{}{} reset -y",
            cuttlefish_home_env_prefix(&runtime.home_dir),
            shell_escape(&cvd_path.display().to_string())
        ));
    }

    Err(job_error_detail(
        ErrorCode::NotFound,
        "no cuttlefish reset command available",
        "set APKW_CUTTLEFISH_RESET_CMD or install cvd".into(),
        job_id,
    ))
}

pub(crate) async fn run_cuttlefish_start_job(job_id: String, show_full_ui: bool) {
    let mut job_client = match connect_job().await {
        Ok(client) => client,
        Err(err) => {
            warn!("cuttlefish job {job_id}: failed to connect job service: {err}");
            return;
        }
    };

    let cancel_rx = spawn_cancel_watcher(job_id.clone()).await;
    if job_is_cancelled(&mut job_client, &job_id).await {
        let _ = publish_log(
            &mut job_client,
            &job_id,
            "Cuttlefish start cancelled before launch\n",
        )
        .await;
        return;
    }

    let _ = publish_state(&mut job_client, &job_id, JobState::Running).await;
    let _ = publish_log(&mut job_client, &job_id, "Starting Cuttlefish\n").await;
    let mut need_start = true;
    let mut adb_serial = cuttlefish_adb_serial();
    let _ = publish_progress(
        &mut job_client,
        &job_id,
        5,
        "checking cuttlefish",
        vec![
            metric("show_full_ui", show_full_ui),
            metric("adb_serial_hint", &adb_serial),
        ],
    )
    .await;
    match cuttlefish_status().await {
        Ok(status) => {
            if !status.adb_serial.is_empty() {
                adb_serial = status.adb_serial;
            }
            if status.running {
                need_start = false;
                let _ = publish_log(&mut job_client, &job_id, "Cuttlefish already running\n").await;
            }
        }
        Err(CuttlefishStatusError::NotInstalled) => {
            let detail = job_error_detail(
                ErrorCode::NotFound,
                "cuttlefish not installed",
                "install Cuttlefish host tools or set APKW_CUTTLEFISH_INSTALL_CMD and run Install Cuttlefish".into(),
                &job_id,
            );
            let _ = publish_failed(&mut job_client, &job_id, detail).await;
            return;
        }
        Err(CuttlefishStatusError::Failed(err)) => {
            let _ = publish_log(
                &mut job_client,
                &job_id,
                &format!("cuttlefish status failed (continuing): {err}\n"),
            )
            .await;
        }
    }
    if need_start && !adb_serial.trim().is_empty() {
        if adb_serial.contains(':') {
            let _ = adb_connect(&adb_serial).await;
        }
        if let Ok(state) = adb_get_state(&adb_serial).await {
            let normalized = state.trim();
            if normalized == "device" {
                need_start = false;
                let _ = publish_log(
                    &mut job_client,
                    &job_id,
                    &format!("adb state={normalized}; skipping start\n"),
                )
                .await;
            }
        }
    }

    if cancel_requested(&cancel_rx) {
        let _ = publish_log(&mut job_client, &job_id, "Cuttlefish start cancelled\n").await;
        return;
    }

    let runtime = match cuttlefish_preflight(&mut job_client, &job_id, true, true).await {
        Ok(runtime) => runtime,
        Err(detail) => {
            let _ = publish_failed(&mut job_client, &job_id, detail).await;
            return;
        }
    };

    match ensure_nonempty_webrtc_custom_css(&runtime.images_dir) {
        Ok(true) => {
            let _ = publish_log(
                &mut job_client,
                &job_id,
                "Patched empty Cuttlefish custom.css to avoid Web UI stylesheet dropouts\n",
            )
            .await;
        }
        Ok(false) => {}
        Err(err) => {
            let _ = publish_log(
                &mut job_client,
                &job_id,
                &format!("Unable to patch Cuttlefish custom.css: {err}\n"),
            )
            .await;
        }
    }

    let start_args = read_env_trimmed("APKW_CUTTLEFISH_START_ARGS").unwrap_or_default();
    if !show_full_ui {
        if !args_has_flag(&start_args, "--start_webrtc") && !local_display_available() {
            let _ = publish_log(
                &mut job_client,
                &job_id,
                "No local display detected; forcing --start_webrtc=true (override with APKW_CUTTLEFISH_START_ARGS=--start_webrtc=false)\n",
            )
            .await;
        }
    }

    if cancel_requested(&cancel_rx) {
        let _ = publish_log(&mut job_client, &job_id, "Cuttlefish start cancelled\n").await;
        return;
    }

    let mut start_cmd = String::new();
    let start_timeout = cuttlefish_start_timeout();
    let stop_timeout = cuttlefish_stop_timeout();
    let (resource_limits, resource_note) = resolve_cuttlefish_resource_limits(&start_args);
    if let Some(note) = resource_note {
        let _ = publish_log(&mut job_client, &job_id, &format!("{note}\n")).await;
    }
    let disable_tap_devices = if args_has_flag(&start_args, "--enable_tap_devices") {
        false
    } else {
        match cuttlefish_tap_mode() {
            CuttlefishTapMode::Enabled => false,
            CuttlefishTapMode::Disabled => true,
            CuttlefishTapMode::Auto => !host_supports_tap_devices().await,
        }
    };
    if disable_tap_devices {
        let _ = publish_log(
            &mut job_client,
            &job_id,
            "Host TAP networking is unavailable; launching Cuttlefish with --enable_tap_devices=false\n",
        )
        .await;
    }
    if need_start {
        cleanup_cuttlefish_temp();
        let command = match cuttlefish_start_command(
            &runtime,
            show_full_ui,
            disable_tap_devices,
            resource_limits,
            &job_id,
        ) {
            Ok(command) => command,
            Err(detail) => {
                let _ = publish_failed(&mut job_client, &job_id, detail).await;
                return;
            }
        };
        start_cmd = command.clone();
        if cancel_requested(&cancel_rx) {
            let _ = publish_log(&mut job_client, &job_id, "Cuttlefish start cancelled\n").await;
            return;
        }
        let outcome = match run_cuttlefish_command(
            &mut job_client,
            &job_id,
            &command,
            "starting",
            40,
            Some(&runtime.host_dir),
            Some(start_timeout),
        )
        .await
        {
            Ok(outcome) => outcome,
            Err(detail) => {
                let _ = publish_failed(&mut job_client, &job_id, detail).await;
                return;
            }
        };

        if !outcome.success {
            if outcome.timed_out {
                let _ = publish_log(
                    &mut job_client,
                    &job_id,
                    &format!(
                        "Start command exceeded {}s timeout; continuing with device readiness checks\n",
                        start_timeout.as_secs()
                    ),
                )
                .await;
            } else {
                let mut recovered = false;
                if should_recover_stale_instance(&outcome.log) {
                    let _ = publish_log(
                        &mut job_client,
                        &job_id,
                        "Cuttlefish instance files are busy; attempting reset recovery\n",
                    )
                    .await;

                    if let Ok(stop_cmd) = cuttlefish_stop_command(&runtime, &job_id) {
                        let _ = run_cuttlefish_command(
                            &mut job_client,
                            &job_id,
                            &stop_cmd,
                            "cleanup",
                            45,
                            Some(&runtime.host_dir),
                            Some(stop_timeout),
                        )
                        .await;
                    }

                    if let Ok(reset_cmd) = cuttlefish_reset_command(&runtime, &job_id) {
                        let _ = run_cuttlefish_command(
                            &mut job_client,
                            &job_id,
                            &reset_cmd,
                            "resetting",
                            50,
                            Some(&runtime.host_dir),
                            Some(stop_timeout),
                        )
                        .await;
                    } else {
                        let _ = publish_log(
                            &mut job_client,
                            &job_id,
                            "No reset command available; retrying start after cleanup\n",
                        )
                        .await;
                    }

                    cleanup_cuttlefish_temp();
                    let retry_outcome = run_cuttlefish_command(
                        &mut job_client,
                        &job_id,
                        &command,
                        "restarting",
                        60,
                        Some(&runtime.host_dir),
                        Some(start_timeout),
                    )
                    .await;
                    recovered = matches!(retry_outcome, Ok(outcome) if outcome.success);
                } else if should_recover_bluetooth(&outcome.log) && command.contains("launch_cvd") {
                    let _ = publish_log(
                        &mut job_client,
                        &job_id,
                        "Cuttlefish boot blocked by Bluetooth; attempting recovery\n",
                    )
                    .await;
                    let recovery_cmd = append_arg_once(command.clone(), "--fail_fast=false");
                    let _ = run_cuttlefish_command(
                        &mut job_client,
                        &job_id,
                        &recovery_cmd,
                        "recovering",
                        45,
                        Some(&runtime.host_dir),
                        Some(start_timeout),
                    )
                    .await;

                    if let Some(serial) =
                        wait_for_adb_device(60, std::time::Duration::from_secs(2)).await
                    {
                        let _ = publish_log(
                            &mut job_client,
                            &job_id,
                            &format!("Enabling Bluetooth in guest via {serial}\n"),
                        )
                        .await;
                        enable_guest_bluetooth(&serial).await;
                    } else {
                        let _ = publish_log(
                            &mut job_client,
                            &job_id,
                            "Bluetooth recovery could not find an ADB device\n",
                        )
                        .await;
                    }

                    if let Ok(stop_cmd) = cuttlefish_stop_command(&runtime, &job_id) {
                        let _ = run_cuttlefish_command(
                            &mut job_client,
                            &job_id,
                            &stop_cmd,
                            "stopping",
                            55,
                            Some(&runtime.host_dir),
                            Some(stop_timeout),
                        )
                        .await;
                    }

                    cleanup_cuttlefish_temp();
                    let retry_outcome = run_cuttlefish_command(
                        &mut job_client,
                        &job_id,
                        &command,
                        "restarting",
                        60,
                        Some(&runtime.host_dir),
                        Some(start_timeout),
                    )
                    .await;
                    recovered = matches!(retry_outcome, Ok(outcome) if outcome.success);
                }

                if recovered {
                    let _ =
                        publish_log(&mut job_client, &job_id, "Cuttlefish recovery succeeded\n")
                            .await;
                } else {
                    let mut detail = if outcome.log.is_empty() {
                        format!("exit_code={}", outcome.exit_code)
                    } else {
                        format!("exit_code={}\n{}", outcome.exit_code, outcome.log)
                    };
                    append_cuttlefish_diagnostics(&mut detail).await;
                    let error = job_error_detail(
                        ErrorCode::Internal,
                        "cuttlefish start failed",
                        detail,
                        &job_id,
                    );
                    let _ = publish_failed(&mut job_client, &job_id, error).await;
                    return;
                }
            }
        }
    }

    if adb_serial.contains(':') {
        let _ = adb_connect(&adb_serial).await;
    }

    let mut running = false;
    let max_attempts = 40; // ~80s total
    for attempt in 0..max_attempts {
        if let Some(serial) = adb_find_device_serial().await {
            adb_serial = serial;
            running = true;
            enable_guest_bluetooth(&adb_serial).await;
            break;
        }
        match adb_get_state(&adb_serial).await {
            Ok(state) => {
                let normalized = state.trim();
                if normalized == "device" {
                    running = true;
                    // Try to force-enable Bluetooth as soon as the guest is reachable.
                    enable_guest_bluetooth(&adb_serial).await;
                    break;
                }
                let _ = publish_log(
                    &mut job_client,
                    &job_id,
                    &format!("adb state={normalized} (attempt {})\n", attempt + 1),
                )
                .await;
            }
            Err(err) => {
                let _ = publish_log(
                    &mut job_client,
                    &job_id,
                    &format!("adb get-state failed: {}\n", adb_failure_message(&err)),
                )
                .await;
            }
        }

        if attempt < max_attempts - 1 {
            let _ = publish_progress(
                &mut job_client,
                &job_id,
                70,
                "waiting for device",
                vec![
                    metric("attempt", attempt + 1),
                    metric("max_attempts", max_attempts),
                    metric("adb_serial", &adb_serial),
                ],
            )
            .await;
            tokio::time::sleep(std::time::Duration::from_secs(2)).await;
        }
    }

    if !running {
        let mut detail = format!("adb_serial={adb_serial}");
        append_cuttlefish_diagnostics(&mut detail).await;
        let error = job_error_detail(
            ErrorCode::TargetNotReachable,
            "cuttlefish not reachable via adb",
            detail,
            &job_id,
        );
        let _ = publish_failed(&mut job_client, &job_id, error).await;
        return;
    }

    let mut outputs = vec![
        KeyValue {
            key: "adb_serial".into(),
            value: adb_serial,
        },
        KeyValue {
            key: "show_full_ui".into(),
            value: show_full_ui.to_string(),
        },
        KeyValue {
            key: "webrtc_url".into(),
            value: cuttlefish_web_url(),
        },
        KeyValue {
            key: "env_url".into(),
            value: cuttlefish_env_url(),
        },
        KeyValue {
            key: "home_dir".into(),
            value: runtime.home_dir.display().to_string(),
        },
        KeyValue {
            key: "images_dir".into(),
            value: runtime.images_dir.display().to_string(),
        },
        KeyValue {
            key: "host_dir".into(),
            value: runtime.host_dir.display().to_string(),
        },
    ];
    if let Some(mode) = cuttlefish_gpu_mode() {
        outputs.push(KeyValue {
            key: "gpu_mode".into(),
            value: mode,
        });
    }
    if !start_cmd.is_empty() {
        outputs.push(KeyValue {
            key: "start_command".into(),
            value: start_cmd,
        });
    }

    if cancel_requested(&cancel_rx) {
        let _ = publish_log(&mut job_client, &job_id, "Cuttlefish start cancelled\n").await;
        return;
    }

    let _ = publish_completed(&mut job_client, &job_id, "Cuttlefish ready", outputs).await;
}

pub(crate) async fn run_cuttlefish_stop_job(job_id: String) {
    let mut job_client = match connect_job().await {
        Ok(client) => client,
        Err(err) => {
            warn!("cuttlefish stop {job_id}: failed to connect job service: {err}");
            return;
        }
    };

    let cancel_rx = spawn_cancel_watcher(job_id.clone()).await;
    if job_is_cancelled(&mut job_client, &job_id).await {
        let _ = publish_log(
            &mut job_client,
            &job_id,
            "Cuttlefish stop cancelled before start\n",
        )
        .await;
        return;
    }

    let _ = publish_state(&mut job_client, &job_id, JobState::Running).await;
    let _ = publish_log(&mut job_client, &job_id, "Stopping Cuttlefish\n").await;
    let adb_serial = cuttlefish_adb_serial();
    let _ = publish_progress(
        &mut job_client,
        &job_id,
        10,
        "stopping",
        vec![metric("adb_serial_hint", adb_serial)],
    )
    .await;

    if cancel_requested(&cancel_rx) {
        let _ = publish_log(&mut job_client, &job_id, "Cuttlefish stop cancelled\n").await;
        return;
    }

    let runtime = match cuttlefish_preflight(&mut job_client, &job_id, false, false).await {
        Ok(runtime) => runtime,
        Err(detail) => {
            let _ = publish_failed(&mut job_client, &job_id, detail).await;
            return;
        }
    };

    if cancel_requested(&cancel_rx) {
        let _ = publish_log(&mut job_client, &job_id, "Cuttlefish stop cancelled\n").await;
        return;
    }

    let command = match cuttlefish_stop_command(&runtime, &job_id) {
        Ok(command) => command,
        Err(detail) => {
            let _ = publish_failed(&mut job_client, &job_id, detail).await;
            return;
        }
    };

    if cancel_requested(&cancel_rx) {
        let _ = publish_log(&mut job_client, &job_id, "Cuttlefish stop cancelled\n").await;
        return;
    }

    let outcome = match run_cuttlefish_command(
        &mut job_client,
        &job_id,
        &command,
        "stopping",
        40,
        Some(&runtime.host_dir),
        Some(cuttlefish_stop_timeout()),
    )
    .await
    {
        Ok(outcome) => outcome,
        Err(detail) => {
            let _ = publish_failed(&mut job_client, &job_id, detail).await;
            return;
        }
    };

    if !outcome.success {
        let mut detail = if outcome.log.is_empty() {
            format!("exit_code={}", outcome.exit_code)
        } else {
            format!("exit_code={}\n{}", outcome.exit_code, outcome.log)
        };
        append_cuttlefish_diagnostics(&mut detail).await;
        let error = job_error_detail(
            ErrorCode::Internal,
            "cuttlefish stop failed",
            detail,
            &job_id,
        );
        let _ = publish_failed(&mut job_client, &job_id, error).await;
        return;
    }

    let stop_log_lower = outcome.log.to_ascii_lowercase();
    if stop_log_lower.contains("unable to connect to launcher monitor")
        || stop_log_lower.contains("clean stop failed")
    {
        let _ = publish_log(
            &mut job_client,
            &job_id,
            "stop_cvd reported launcher monitor unavailable; treating stop as complete (launcher already exited)\n",
        )
        .await;
    }

    let outputs = vec![
        KeyValue {
            key: "stop_command".into(),
            value: command,
        },
        KeyValue {
            key: "home_dir".into(),
            value: runtime.home_dir.display().to_string(),
        },
    ];

    if cancel_requested(&cancel_rx) {
        let _ = publish_log(&mut job_client, &job_id, "Cuttlefish stop cancelled\n").await;
        return;
    }

    let _ = publish_completed(&mut job_client, &job_id, "Cuttlefish stopped", outputs).await;
}

pub(crate) async fn run_cuttlefish_install_job(job_id: String, options: CuttlefishInstallOptions) {
    let mut job_client = match connect_job().await {
        Ok(client) => client,
        Err(err) => {
            warn!("cuttlefish install {job_id}: failed to connect job service: {err}");
            return;
        }
    };

    let cancel_rx = spawn_cancel_watcher(job_id.clone()).await;
    if job_is_cancelled(&mut job_client, &job_id).await {
        let _ = publish_log(
            &mut job_client,
            &job_id,
            "Cuttlefish install cancelled before start\n",
        )
        .await;
        return;
    }

    let _ = publish_state(&mut job_client, &job_id, JobState::Running).await;
    let _ = publish_log(&mut job_client, &job_id, "Installing Cuttlefish\n").await;

    let page_size = host_page_size();
    let home_dir = cuttlefish_home_dir(page_size);
    let images_dir = cuttlefish_images_dir(page_size);
    let host_dir = cuttlefish_host_dir(page_size);

    let host_tools_issue = cuttlefish_host_tools_issue(page_size);
    let host_installed = host_tools_issue.is_none();
    let images_ready = cuttlefish_images_ready(&images_dir);
    let kvm_status = kvm_status();
    let mut environment_metrics = vec![
        metric("page_size", page_size.unwrap_or_default()),
        metric("home_dir", home_dir.display()),
        metric("images_dir", images_dir.display()),
        metric("host_dir", host_dir.display()),
        metric("host_installed", host_installed),
        metric("images_ready", images_ready),
        metric("kvm_present", kvm_status.present),
        metric("kvm_accessible", kvm_status.accessible),
    ];
    if let Some(issue) = host_tools_issue.as_ref() {
        environment_metrics.push(metric("host_issue", issue));
    }
    let _ = publish_progress(
        &mut job_client,
        &job_id,
        5,
        "checking environment",
        environment_metrics,
    )
    .await;
    if let Some(issue) = host_tools_issue.as_ref() {
        let _ = publish_log(
            &mut job_client,
            &job_id,
            &format!("Host tools incomplete: {issue}\n"),
        )
        .await;
    }

    if cuttlefish_kvm_check_enabled() {
        let _ = publish_log(
            &mut job_client,
            &job_id,
            &format!(
                "KVM check: present={} accessible={}\n",
                kvm_status.present, kvm_status.accessible
            ),
        )
        .await;
        if !kvm_status.present {
            let _ = publish_log(
                &mut job_client,
                &job_id,
                "KVM device not found (/dev/kvm). Cuttlefish will not run until virtualization is enabled.\n",
            )
            .await;
        } else if !kvm_status.accessible {
            let detail = kvm_status
                .detail
                .as_deref()
                .unwrap_or("failed to open /dev/kvm");
            let _ = publish_log(
                &mut job_client,
                &job_id,
                &format!(
                    "KVM access denied: {detail}. Add the user to the kvm group and re-login.\n"
                ),
            )
            .await;
        }
    } else {
        let _ = publish_log(
            &mut job_client,
            &job_id,
            "Skipping KVM check (APKW_CUTTLEFISH_KVM_CHECK=0)\n",
        )
        .await;
    }

    if cancel_requested(&cancel_rx) {
        let _ = publish_log(&mut job_client, &job_id, "Cuttlefish install cancelled\n").await;
        return;
    }

    let install_host = match std::env::var("APKW_CUTTLEFISH_INSTALL_HOST") {
        Ok(val) => !(val == "0" || val.eq_ignore_ascii_case("false")),
        Err(_) => true,
    };
    let install_images = match std::env::var("APKW_CUTTLEFISH_INSTALL_IMAGES") {
        Ok(val) => !(val == "0" || val.eq_ignore_ascii_case("false")),
        Err(_) => true,
    };
    let add_groups = match std::env::var("APKW_CUTTLEFISH_ADD_GROUPS") {
        Ok(val) => !(val == "0" || val.eq_ignore_ascii_case("false")),
        Err(_) => true,
    };

    if install_host && (!host_installed || options.force) {
        let mut used_default = false;
        let mut privilege_mode = None;
        let install_cmd = if let Some(cmd) = read_env_trimmed("APKW_CUTTLEFISH_INSTALL_CMD") {
            cmd
        } else {
            if std::env::consts::OS != "linux" {
                let detail = job_error_detail(
                    ErrorCode::InvalidArgument,
                    "cuttlefish install not supported on this host",
                    format!(
                        "host_os={}; set APKW_CUTTLEFISH_INSTALL_CMD for a custom installer",
                        std::env::consts::OS
                    ),
                    &job_id,
                );
                let _ = publish_failed(&mut job_client, &job_id, detail).await;
                return;
            }
            if !is_debian_like() {
                let detail = job_error_detail(
                    ErrorCode::InvalidArgument,
                    "cuttlefish install not configured for this distro",
                    format!(
                        "host_os=linux {}; set APKW_CUTTLEFISH_INSTALL_CMD for a custom installer",
                        linux_distro_summary()
                    ),
                    &job_id,
                );
                let _ = publish_failed(&mut job_client, &job_id, detail).await;
                return;
            }
            let installer_path = if let Some(path) = find_command("apt-get") {
                path
            } else if let Some(path) = find_command("apt") {
                path
            } else {
                let detail = job_error_detail(
                    ErrorCode::NotFound,
                    "no supported package manager found",
                    "install cuttlefish manually or set APKW_CUTTLEFISH_INSTALL_CMD".into(),
                    &job_id,
                );
                let _ = publish_failed(&mut job_client, &job_id, detail).await;
                return;
            };
            used_default = true;
            let installer = shell_escape(&installer_path.display().to_string());
            let repo_key = "https://us-apt.pkg.dev/doc/repo-signing-key.gpg";
            let repo_line = "deb https://us-apt.pkg.dev/projects/android-cuttlefish-artifacts android-cuttlefish main";
            let install_script = format!(
                "{installer} update && {installer} install -y curl ca-certificates && curl -fsSL {} -o /etc/apt/trusted.gpg.d/artifact-registry.asc && chmod a+r /etc/apt/trusted.gpg.d/artifact-registry.asc && sh -lc \"echo {} > /etc/apt/sources.list.d/artifact-registry.list\" && {installer} update && {installer} install -y cuttlefish-base cuttlefish-user",
                shell_escape(repo_key),
                shell_escape(repo_line),
            );
            match privileged_shell_command(&install_script).await {
                Ok((command, mode)) => {
                    privilege_mode = Some(mode);
                    command
                }
                Err(detail) => {
                    let error = job_error_detail(
                        ErrorCode::PermissionDenied,
                        "privileged install unavailable",
                        detail,
                        &job_id,
                    );
                    let _ = publish_failed(&mut job_client, &job_id, error).await;
                    return;
                }
            }
        };

        if used_default {
            let _ = publish_log(
                &mut job_client,
                &job_id,
                "Using Debian/Ubuntu apt install from android-cuttlefish README\n",
            )
            .await;
            if let Some(mode) = privilege_mode {
                let _ = publish_log(
                    &mut job_client,
                    &job_id,
                    &format!("Privilege escalation: {}\n", mode.description()),
                )
                .await;
                if mode == PrivilegedShellMode::Pkexec {
                    let _ = publish_log(
                        &mut job_client,
                        &job_id,
                        "Approve the desktop authentication prompt if shown\n",
                    )
                    .await;
                }
            }
        }
        let _ = publish_log(
            &mut job_client,
            &job_id,
            &format!("Install command: {install_cmd}\n"),
        )
        .await;
        let _ = publish_progress(
            &mut job_client,
            &job_id,
            30,
            "installing host tools",
            vec![
                metric("force", options.force),
                metric("install_cmd", &install_cmd),
            ],
        )
        .await;

        if cancel_requested(&cancel_rx) {
            let _ = publish_log(&mut job_client, &job_id, "Cuttlefish install cancelled\n").await;
            return;
        }

        match run_shell_command(&install_cmd).await {
            Ok((true, _, log)) => {
                if !log.is_empty() {
                    let _ = publish_log(&mut job_client, &job_id, &log).await;
                }
            }
            Ok((false, code, log)) => {
                if !log.is_empty() {
                    let _ = publish_log(&mut job_client, &job_id, &log).await;
                }
                let detail = if log.is_empty() {
                    format!("exit_code={code}")
                } else {
                    format!("exit_code={code}\n{log}")
                };
                let error = job_error_detail(
                    classify_install_error(&detail),
                    "Cuttlefish install failed",
                    detail,
                    &job_id,
                );
                let _ = publish_failed(&mut job_client, &job_id, error).await;
                return;
            }
            Err(err) => {
                let error = job_error_detail(
                    ErrorCode::Internal,
                    "failed to run install command",
                    err.to_string(),
                    &job_id,
                );
                let _ = publish_failed(&mut job_client, &job_id, error).await;
                return;
            }
        }
    } else if !install_host {
        let _ = publish_log(
            &mut job_client,
            &job_id,
            "Host install disabled (APKW_CUTTLEFISH_INSTALL_HOST=0)\n",
        )
        .await;
    } else {
        let _ = publish_log(
            &mut job_client,
            &job_id,
            "Host tools already installed; skipping host install\n",
        )
        .await;
    }

    let host_tools_issue = cuttlefish_host_tools_issue(page_size);
    if let Some(issue) = host_tools_issue.as_ref() {
        let _ = publish_log(
            &mut job_client,
            &job_id,
            &format!("Host tools still incomplete after install: {issue}\n"),
        )
        .await;
        if install_host {
            let error = job_error_detail(
                ErrorCode::Unavailable,
                "Cuttlefish host tools incomplete",
                issue.clone(),
                &job_id,
            );
            let _ = publish_failed(&mut job_client, &job_id, error).await;
            return;
        }
    }

    let required_groups = ["kvm", "cvdnetwork", "render"];
    let missing = match current_user_groups().await {
        Some(groups) => {
            let missing = missing_groups(&groups, &required_groups);
            if missing.is_empty() {
                let _ = publish_log(
                    &mut job_client,
                    &job_id,
                    "User already in kvm/cvdnetwork/render groups\n",
                )
                .await;
            } else {
                let _ = publish_log(
                    &mut job_client,
                    &job_id,
                    &format!("Missing groups: {}\n", missing.join(",")),
                )
                .await;
            }
            missing
        }
        None => {
            let _ = publish_log(
                &mut job_client,
                &job_id,
                "Unable to determine user groups; assuming group setup is needed\n",
            )
            .await;
            required_groups.iter().map(|g| g.to_string()).collect()
        }
    };

    if cancel_requested(&cancel_rx) {
        let _ = publish_log(&mut job_client, &job_id, "Cuttlefish install cancelled\n").await;
        return;
    }

    if add_groups {
        if missing.is_empty() {
            let _ = publish_log(
                &mut job_client,
                &job_id,
                "Group setup not required; skipping usermod\n",
            )
            .await;
        } else if let Ok(user) = std::env::var("USER") {
            if !user.trim().is_empty() {
                if let Some(usermod_path) = find_command("usermod") {
                    let group_script = format!(
                        "{} -aG kvm,cvdnetwork,render {}",
                        shell_escape(&usermod_path.display().to_string()),
                        shell_escape(user.trim()),
                    );
                    match privileged_shell_command(&group_script).await {
                        Ok((group_cmd, mode)) => {
                            let _ = publish_log(
                                &mut job_client,
                                &job_id,
                                &format!(
                                    "Adding user to kvm/cvdnetwork/render groups via {}\n",
                                    mode.description()
                                ),
                            )
                            .await;
                            match run_shell_command(&group_cmd).await {
                                Ok((true, _, log)) => {
                                    if !log.is_empty() {
                                        let _ = publish_log(&mut job_client, &job_id, &log).await;
                                    }
                                    let _ = publish_log(
                                        &mut job_client,
                                        &job_id,
                                        "Re-login or reboot may be required for group changes to take effect\n",
                                    )
                                    .await;
                                }
                                Ok((false, _, log)) => {
                                    if !log.is_empty() {
                                        let _ = publish_log(&mut job_client, &job_id, &log).await;
                                    }
                                    let _ = publish_log(
                                        &mut job_client,
                                        &job_id,
                                        "Group setup failed; add the user to kvm/cvdnetwork/render manually if needed\n",
                                    )
                                    .await;
                                }
                                Err(err) => {
                                    let _ = publish_log(
                                        &mut job_client,
                                        &job_id,
                                        &format!(
                                            "Group setup failed to start: {err}; add the user to kvm/cvdnetwork/render manually if needed\n"
                                        ),
                                    )
                                    .await;
                                }
                            }
                        }
                        Err(detail) => {
                            let _ = publish_log(
                                &mut job_client,
                                &job_id,
                                &format!("Skipping group setup: {detail}\n"),
                            )
                            .await;
                        }
                    }
                } else {
                    let _ = publish_log(
                        &mut job_client,
                        &job_id,
                        "Skipping group setup; usermod not found\n",
                    )
                    .await;
                }
            }
        } else {
            let _ = publish_log(
                &mut job_client,
                &job_id,
                "Skipping group setup; USER is not set\n",
            )
            .await;
        }
    } else if !missing.is_empty() {
        let _ = publish_log(
            &mut job_client,
            &job_id,
            "Group setup disabled (APKW_CUTTLEFISH_ADD_GROUPS=0)\n",
        )
        .await;
    }

    if install_images && (!images_ready || options.force) {
        if cancel_requested(&cancel_rx) {
            let _ = publish_log(&mut job_client, &job_id, "Cuttlefish install cancelled\n").await;
            return;
        }
        let config = resolve_cuttlefish_request_config(
            page_size,
            options.branch.clone(),
            options.target.clone(),
            options.build_id.clone(),
        );
        let branch = config.branch;
        let target = config.target;
        let branch_override = config.has_branch_override;
        let target_override = config.has_target_override;
        let build_id_override = config.build_id_override;

        let mut candidates = vec![(branch.clone(), target.clone())];
        if build_id_override.is_none() && !branch_override && !target_override {
            if let Some((fallback_branch, fallback_target)) =
                cuttlefish_fallback_branch_target(page_size)
            {
                if fallback_branch != branch || fallback_target != target {
                    candidates.push((fallback_branch, fallback_target));
                }
            }
        }

        let mut resolved_branch = None;
        let mut resolved_target = None;
        let mut build_info = None;

        for (candidate_branch, candidate_target) in candidates {
            match resolve_build_info(
                &candidate_branch,
                &candidate_target,
                build_id_override.clone(),
            )
            .await
            {
                Ok(info) => {
                    resolved_branch = Some(candidate_branch);
                    resolved_target = Some(candidate_target);
                    build_info = Some(info);
                    break;
                }
                Err(err) => {
                    let _ = publish_log(
                        &mut job_client,
                        &job_id,
                        &format!(
                            "Cuttlefish build not available for branch={candidate_branch} target={candidate_target}: {err}\n"
                        ),
                    )
                    .await;
                }
            }
        }

        let Some(build_info) = build_info else {
            let error = job_error_detail(
                ErrorCode::Internal,
                "failed to resolve Cuttlefish build",
                "no viable build artifacts found".to_string(),
                &job_id,
            );
            let _ = publish_failed(&mut job_client, &job_id, error).await;
            return;
        };

        let branch = resolved_branch.unwrap_or(branch);
        let target = resolved_target.unwrap_or(target);

        let build_id = build_info.build_id;
        let product = build_info.product;

        let _ = publish_log(
            &mut job_client,
            &job_id,
            &format!(
                "Resolved build: branch={branch} target={target} build_id={build_id} product={product}\n"
            ),
        )
        .await;

        let img_candidates = cuttlefish_image_artifact_candidates(&product, &target, &build_id);
        let host_candidates = cuttlefish_host_artifact_candidates(&build_id);
        let target_paths = candidate_target_paths(&target, &product);

        let img_url =
            match resolve_artifact_url_for_targets(&build_id, &target_paths, &img_candidates).await
            {
                Ok(url) => url,
                Err(err) => {
                    let error = job_error_detail(
                        ErrorCode::NotFound,
                        "failed to locate Cuttlefish image artifact",
                        err,
                        &job_id,
                    );
                    let _ = publish_failed(&mut job_client, &job_id, error).await;
                    return;
                }
            };

        let host_url = match resolve_artifact_url_for_targets(
            &build_id,
            &target_paths,
            &host_candidates,
        )
        .await
        {
            Ok(url) => url,
            Err(err) => {
                let error = job_error_detail(
                    ErrorCode::NotFound,
                    "failed to locate Cuttlefish host package",
                    err,
                    &job_id,
                );
                let _ = publish_failed(&mut job_client, &job_id, error).await;
                return;
            }
        };

        let _ = publish_log(
            &mut job_client,
            &job_id,
            &format!("Image artifact: {img_url}\nHost artifact: {host_url}\n"),
        )
        .await;

        let downloads_dir = data_dir()
            .join("cuttlefish")
            .join("downloads")
            .join(&build_id);
        let _ = std::fs::create_dir_all(&downloads_dir);

        let img_artifact = img_url
            .split('/')
            .next_back()
            .unwrap_or("cuttlefish-img.zip")
            .to_string();
        let host_artifact = host_url
            .split('/')
            .next_back()
            .unwrap_or("cvd-host_package.tar.gz")
            .to_string();

        let img_path = downloads_dir.join(&img_artifact);
        let host_path = downloads_dir.join(&host_artifact);

        let _ = publish_progress(
            &mut job_client,
            &job_id,
            55,
            "downloading images",
            vec![
                metric("branch", &branch),
                metric("target", &target),
                metric("build_id", &build_id),
                metric("product", &product),
                metric("image_url", &img_url),
                metric("image_path", img_path.display()),
            ],
        )
        .await;
        let img_cmd = format!(
            "curl -fL {} -o {}",
            shell_escape(&img_url),
            shell_escape(&img_path.display().to_string())
        );
        if cancel_requested(&cancel_rx) {
            let _ = publish_log(&mut job_client, &job_id, "Cuttlefish install cancelled\n").await;
            return;
        }
        if let Err(err) = run_shell_command(&img_cmd).await {
            let error = job_error_detail(
                ErrorCode::Internal,
                "failed to download system images",
                err.to_string(),
                &job_id,
            );
            let _ = publish_failed(&mut job_client, &job_id, error).await;
            return;
        }

        let _ = publish_progress(
            &mut job_client,
            &job_id,
            60,
            "downloading host package",
            vec![
                metric("branch", &branch),
                metric("target", &target),
                metric("build_id", &build_id),
                metric("product", &product),
                metric("host_url", &host_url),
                metric("host_path", host_path.display()),
            ],
        )
        .await;
        let host_cmd = format!(
            "curl -fL {} -o {}",
            shell_escape(&host_url),
            shell_escape(&host_path.display().to_string())
        );
        if cancel_requested(&cancel_rx) {
            let _ = publish_log(&mut job_client, &job_id, "Cuttlefish install cancelled\n").await;
            return;
        }
        if let Err(err) = run_shell_command(&host_cmd).await {
            let error = job_error_detail(
                ErrorCode::Internal,
                "failed to download host package",
                err.to_string(),
                &job_id,
            );
            let _ = publish_failed(&mut job_client, &job_id, error).await;
            return;
        }

        let _ = std::fs::create_dir_all(&images_dir);
        let _ = std::fs::create_dir_all(&host_dir);

        let _ = publish_progress(
            &mut job_client,
            &job_id,
            70,
            "extracting images",
            vec![
                metric("image_path", img_path.display()),
                metric("images_dir", images_dir.display()),
            ],
        )
        .await;
        let unzip_cmd = format!(
            "unzip -o {} -d {}",
            shell_escape(&img_path.display().to_string()),
            shell_escape(&images_dir.display().to_string())
        );
        if cancel_requested(&cancel_rx) {
            let _ = publish_log(&mut job_client, &job_id, "Cuttlefish install cancelled\n").await;
            return;
        }
        match run_shell_command(&unzip_cmd).await {
            Ok((true, _, log)) => {
                if !log.is_empty() {
                    let _ = publish_log(&mut job_client, &job_id, &log).await;
                }
            }
            Ok((false, code, log)) => {
                let detail = if log.is_empty() {
                    format!("exit_code={code}")
                } else {
                    format!("exit_code={code}\n{log}")
                };
                let error = job_error_detail(
                    ErrorCode::Internal,
                    "failed to extract images",
                    detail,
                    &job_id,
                );
                let _ = publish_failed(&mut job_client, &job_id, error).await;
                return;
            }
            Err(err) => {
                let error = job_error_detail(
                    ErrorCode::Internal,
                    "failed to extract images",
                    err.to_string(),
                    &job_id,
                );
                let _ = publish_failed(&mut job_client, &job_id, error).await;
                return;
            }
        }

        let _ = publish_progress(
            &mut job_client,
            &job_id,
            80,
            "extracting host tools",
            vec![
                metric("host_path", host_path.display()),
                metric("host_dir", host_dir.display()),
            ],
        )
        .await;
        let tar_cmd = format!(
            "tar -xzf {} -C {}",
            shell_escape(&host_path.display().to_string()),
            shell_escape(&host_dir.display().to_string())
        );
        if cancel_requested(&cancel_rx) {
            let _ = publish_log(&mut job_client, &job_id, "Cuttlefish install cancelled\n").await;
            return;
        }
        match run_shell_command(&tar_cmd).await {
            Ok((true, _, log)) => {
                if !log.is_empty() {
                    let _ = publish_log(&mut job_client, &job_id, &log).await;
                }
            }
            Ok((false, code, log)) => {
                let detail = if log.is_empty() {
                    format!("exit_code={code}")
                } else {
                    format!("exit_code={code}\n{log}")
                };
                let error = job_error_detail(
                    ErrorCode::Internal,
                    "failed to extract host tools",
                    detail,
                    &job_id,
                );
                let _ = publish_failed(&mut job_client, &job_id, error).await;
                return;
            }
            Err(err) => {
                let error = job_error_detail(
                    ErrorCode::Internal,
                    "failed to extract host tools",
                    err.to_string(),
                    &job_id,
                );
                let _ = publish_failed(&mut job_client, &job_id, error).await;
                return;
            }
        }
    } else if !install_images {
        let _ = publish_log(
            &mut job_client,
            &job_id,
            "Image install disabled (APKW_CUTTLEFISH_INSTALL_IMAGES=0)\n",
        )
        .await;
    } else {
        let _ = publish_log(
            &mut job_client,
            &job_id,
            "Images already present; skipping download\n",
        )
        .await;
    }

    if cancel_requested(&cancel_rx) {
        let _ = publish_log(&mut job_client, &job_id, "Cuttlefish install cancelled\n").await;
        return;
    }

    let mut outputs = vec![
        KeyValue {
            key: "force".into(),
            value: options.force.to_string(),
        },
        KeyValue {
            key: "home_dir".into(),
            value: home_dir.display().to_string(),
        },
        KeyValue {
            key: "images_dir".into(),
            value: images_dir.display().to_string(),
        },
        KeyValue {
            key: "host_dir".into(),
            value: host_dir.display().to_string(),
        },
        KeyValue {
            key: "install_host".into(),
            value: install_host.to_string(),
        },
        KeyValue {
            key: "install_images".into(),
            value: install_images.to_string(),
        },
        KeyValue {
            key: "kvm_present".into(),
            value: kvm_status.present.to_string(),
        },
        KeyValue {
            key: "kvm_access".into(),
            value: kvm_status.accessible.to_string(),
        },
        KeyValue {
            key: "host_installed".into(),
            value: cuttlefish_host_tools_ready(page_size).to_string(),
        },
    ];
    if let Some(path) = cuttlefish_capability_query_path() {
        outputs.push(KeyValue {
            key: "capability_query_path".into(),
            value: path.display().to_string(),
        });
    }
    if let Some(issue) = host_tools_issue {
        outputs.push(KeyValue {
            key: "host_issue".into(),
            value: issue,
        });
    }
    if let Some(detail) = kvm_status.detail {
        outputs.push(KeyValue {
            key: "kvm_detail".into(),
            value: detail,
        });
    }

    let _ = publish_completed(&mut job_client, &job_id, "Cuttlefish installed", outputs).await;
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn tap_probe_name_fits_linux_ifname_limit() {
        let name = tap_probe_name();
        assert!(!name.is_empty());
        assert!(name.len() <= 15, "ifname too long: {name}");
        assert!(
            name.chars().all(|ch| ch.is_ascii_alphanumeric()),
            "ifname should be alphanumeric: {name}"
        );
    }

    #[test]
    fn tap_permission_error_matches_known_messages() {
        assert!(tap_permission_error(
            "ioctl(TUNSETIFF): Operation not permitted"
        ));
        assert!(tap_permission_error(
            "open: /dev/net/tun: No such file or directory"
        ));
        assert!(!tap_permission_error("invalid argument"));
    }

    #[test]
    fn host_tools_require_capability_query_when_using_default_launchers() {
        let issue = cuttlefish_host_tools_issue_for_detection(false, false, true, false)
            .expect("expected missing capability query to be reported");
        assert!(issue.contains("capability_query.py"));
    }

    #[test]
    fn host_tools_allow_custom_start_command_without_capability_query() {
        assert!(cuttlefish_host_tools_issue_for_detection(true, false, false, false).is_none());
    }

    #[test]
    fn recommended_resource_limits_match_host_tiers() {
        assert_eq!(recommended_cuttlefish_cpus(2), Some(1));
        assert_eq!(recommended_cuttlefish_cpus(4), Some(2));
        assert_eq!(recommended_cuttlefish_cpus(6), Some(2));
        assert_eq!(recommended_cuttlefish_cpus(8), None);
        assert_eq!(recommended_cuttlefish_memory_mb(6 * 1024), Some(2048));
        assert_eq!(recommended_cuttlefish_memory_mb(8 * 1024), Some(3072));
        assert_eq!(recommended_cuttlefish_memory_mb(10 * 1024), Some(3072));
        assert_eq!(recommended_cuttlefish_memory_mb(16 * 1024), None);
        assert_eq!(
            recommended_cuttlefish_display(2, 6 * 1024),
            Some((540, 960, 240))
        );
        assert_eq!(
            recommended_cuttlefish_display(4, 8 * 1024),
            Some((720, 1280, 280))
        );
        assert_eq!(recommended_cuttlefish_display(8, 16 * 1024), None);
    }

    #[test]
    fn ensure_nonempty_webrtc_custom_css_fills_empty_file() {
        let unique = format!(
            "apkw-cf-css-{}-{}",
            std::process::id(),
            SystemTime::now()
                .duration_since(SystemTime::UNIX_EPOCH)
                .map(|d| d.as_nanos())
                .unwrap_or(0)
        );
        let root = std::env::temp_dir().join(unique);
        let assets = root.join("usr/share/webrtc/assets");
        fs::create_dir_all(&assets).expect("create assets dir");
        let css = assets.join("custom.css");
        fs::write(&css, "").expect("create empty css");

        let patched = ensure_nonempty_webrtc_custom_css(&root).expect("patch css");
        assert!(patched);
        let data = fs::read_to_string(&css).expect("read patched css");
        assert!(data.contains("APKW"));

        let patched_again = ensure_nonempty_webrtc_custom_css(&root).expect("second patch css");
        assert!(!patched_again);

        let _ = fs::remove_dir_all(root);
    }
}
