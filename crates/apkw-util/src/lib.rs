use std::{
    fs,
    io::{self, Write},
    net::SocketAddr,
    path::{Path, PathBuf},
    sync::atomic::{AtomicU64, Ordering},
    time::Duration,
};

use apkw_proto::apkw::v1::{
    job_service_client::JobServiceClient, Id, JobEvent, ListJobHistoryRequest, Pagination,
    Timestamp,
};
use apkw_telemetry as telemetry;
use fs2::FileExt;
use serde::Serialize;
use tonic::transport::{server::Router, Channel, Server};
use tracing::info;
use walkdir::WalkDir;
use zip::write::FileOptions;

#[cfg(unix)]
use std::os::unix::fs::PermissionsExt;

pub const DEFAULT_JOB_ADDR: &str = "127.0.0.1:50051";
pub const DEFAULT_TOOLCHAIN_ADDR: &str = "127.0.0.1:50052";
pub const DEFAULT_PROJECT_ADDR: &str = "127.0.0.1:50053";
pub const DEFAULT_BUILD_ADDR: &str = "127.0.0.1:50054";
pub const DEFAULT_TARGETS_ADDR: &str = "127.0.0.1:50055";
pub const DEFAULT_OBSERVE_ADDR: &str = "127.0.0.1:50056";
pub const DEFAULT_WORKFLOW_ADDR: &str = "127.0.0.1:50057";

static TMP_FILE_SEQ: AtomicU64 = AtomicU64::new(0);

pub fn env_var(key: &str) -> Option<String> {
    std::env::var(key).ok()
}

pub fn env_addr(key: &str, default: &str) -> String {
    env_var(key).unwrap_or_else(|| default.to_string())
}

pub fn promote_legacy_env() {}

fn preferred_data_dir_name() -> &'static str {
    "apkw"
}

fn home_data_dir(name: &str) -> PathBuf {
    if let Ok(home) = std::env::var("HOME") {
        PathBuf::from(home).join(format!(".local/share/{name}"))
    } else {
        PathBuf::from(format!("/tmp/{name}"))
    }
}

pub fn job_addr() -> String {
    env_addr("APKW_JOB_ADDR", DEFAULT_JOB_ADDR)
}

pub fn toolchain_addr() -> String {
    env_addr("APKW_TOOLCHAIN_ADDR", DEFAULT_TOOLCHAIN_ADDR)
}

pub fn project_addr() -> String {
    env_addr("APKW_PROJECT_ADDR", DEFAULT_PROJECT_ADDR)
}

pub fn build_addr() -> String {
    env_addr("APKW_BUILD_ADDR", DEFAULT_BUILD_ADDR)
}

pub fn targets_addr() -> String {
    env_addr("APKW_TARGETS_ADDR", DEFAULT_TARGETS_ADDR)
}

pub fn observe_addr() -> String {
    env_addr("APKW_OBSERVE_ADDR", DEFAULT_OBSERVE_ADDR)
}

pub fn workflow_addr() -> String {
    env_addr("APKW_WORKFLOW_ADDR", DEFAULT_WORKFLOW_ADDR)
}

pub fn data_dir() -> PathBuf {
    home_data_dir(preferred_data_dir_name())
}

pub fn state_dir() -> PathBuf {
    data_dir().join("state")
}

pub fn state_file_path(file_name: &str) -> PathBuf {
    state_dir().join(file_name)
}

pub fn expand_user(path: &str) -> PathBuf {
    if path == "~" || path.starts_with("~/") {
        if let Ok(home) = std::env::var("HOME") {
            let rest = path.strip_prefix("~/").unwrap_or("");
            return PathBuf::from(home).join(rest);
        }
    }
    PathBuf::from(path)
}

pub fn write_json_atomic<T: Serialize>(path: &Path, value: &T) -> io::Result<()> {
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)?;
    }
    let tmp = temp_write_path(path);
    let data = serde_json::to_vec_pretty(value).map_err(io::Error::other)?;
    {
        let mut file = fs::OpenOptions::new()
            .create_new(true)
            .write(true)
            .open(&tmp)?;
        file.write_all(&data)?;
        file.sync_all()?;
    }
    if let Err(err) = fs::rename(&tmp, path) {
        let _ = fs::remove_file(&tmp);
        return Err(err);
    }
    if let Some(parent) = path.parent() {
        sync_dir(parent)?;
    }
    Ok(())
}

pub fn now_millis() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis() as i64
}

pub fn now_ts() -> Timestamp {
    Timestamp {
        unix_millis: now_millis(),
    }
}

pub fn default_export_path(prefix: &str, job_id: &str) -> PathBuf {
    let ts = now_millis();
    state_dir().join(format!("{prefix}-{job_id}-{ts}.json"))
}

pub async fn collect_job_history(
    client: &mut JobServiceClient<Channel>,
    job_id: &str,
) -> Result<Vec<JobEvent>, Box<dyn std::error::Error>> {
    let mut events = Vec::new();
    let mut page_token = String::new();
    loop {
        let resp = client
            .list_job_history(ListJobHistoryRequest {
                job_id: Some(Id {
                    value: job_id.to_string(),
                }),
                page: Some(Pagination {
                    page_size: 200,
                    page_token: page_token.clone(),
                }),
                filter: None,
            })
            .await?
            .into_inner();
        events.extend(resp.events);
        let next_token = resp
            .page_info
            .map(|page_info| page_info.next_page_token)
            .unwrap_or_default();
        if next_token.is_empty() {
            break;
        }
        page_token = next_token;
    }
    Ok(events)
}

pub fn init_tracing() -> Result<(), Box<dyn std::error::Error>> {
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::from_default_env().add_directive("info".parse()?),
        )
        .init();
    Ok(())
}

pub fn init_service_telemetry(
    app_name: &'static str,
    app_version: &'static str,
    service_name: &str,
) {
    telemetry::init_with_env(app_name, app_version);
    telemetry::event("service.start", &[("service", service_name)]);
}

pub async fn serve_grpc<F>(
    app_name: &str,
    addr_env: &str,
    default_addr: &str,
    add_service: F,
) -> Result<(), Box<dyn std::error::Error>>
where
    F: FnOnce(&mut Server) -> Router,
{
    let addr_str = env_addr(addr_env, default_addr);
    let addr: SocketAddr = addr_str.parse()?;
    info!("{app_name} listening on {addr}");

    let mut server = Server::builder();
    add_service(&mut server).serve(addr).await?;
    Ok(())
}

pub async fn serve_grpc_with_telemetry<F>(
    app_name: &'static str,
    app_version: &'static str,
    service_name: &str,
    addr_env: &str,
    default_addr: &str,
    add_service: F,
) -> Result<(), Box<dyn std::error::Error>>
where
    F: FnOnce(&mut Server) -> Router,
{
    init_tracing()?;
    init_service_telemetry(app_name, app_version, service_name);
    serve_grpc(app_name, addr_env, default_addr, add_service).await
}

const STATE_EXPORTS_DIR: &str = "state-exports";
const STATE_OPS_DIR: &str = "state-ops";
const STATE_OP_LOCK_FILE: &str = "lock";
const STATE_OP_QUEUE_FILE: &str = "queue.txt";
const STATE_OP_ACTIVE_FILE: &str = "active.txt";
const STATE_OP_WAIT_MS: u64 = 200;
const STATE_OP_STALE_MS: i64 = 20 * 60 * 1000;

const LARGE_DIR_DOWNLOADS: &str = "downloads";
const LARGE_DIR_TOOLCHAINS: &str = "toolchains";
const LARGE_DIR_BUNDLES: &str = "bundles";
const LARGE_DIR_TELEMETRY: &str = "telemetry";

#[derive(Clone, Debug, Default)]
pub struct StateArchiveOptions {
    pub exclude_downloads: bool,
    pub exclude_toolchains: bool,
    pub exclude_bundles: bool,
    pub exclude_telemetry: bool,
}

#[derive(Clone, Debug)]
pub struct StateArchiveResult {
    pub output_path: PathBuf,
    pub file_count: u64,
    pub dir_count: u64,
    pub total_bytes: u64,
}

#[derive(Clone, Debug)]
pub struct StateOpenResult {
    pub restored_files: u64,
    pub restored_dirs: u64,
    pub preserved_dirs: Vec<String>,
}

pub struct StateOpGuard {
    token: String,
    lock_path: PathBuf,
    queue_path: PathBuf,
    active_path: PathBuf,
}

impl StateOpGuard {
    pub fn acquire(label: &str) -> io::Result<Self> {
        let ops_dir = state_ops_dir();
        fs::create_dir_all(&ops_dir)?;
        let lock_path = ops_dir.join(STATE_OP_LOCK_FILE);
        let queue_path = ops_dir.join(STATE_OP_QUEUE_FILE);
        let active_path = ops_dir.join(STATE_OP_ACTIVE_FILE);
        let token = format!("{}:{}:{}", label.trim(), std::process::id(), now_millis());

        with_state_ops_lock(&lock_path, || {
            let mut queue = read_state_ops_queue(&queue_path)?;
            if !queue.iter().any(|item| item == &token) {
                queue.push(token.clone());
                write_state_ops_queue(&queue_path, &queue)?;
            }
            Ok(())
        })?;

        loop {
            let mut acquired = false;
            with_state_ops_lock(&lock_path, || {
                let mut queue = read_state_ops_queue(&queue_path)?;
                let active = read_state_ops_active(&active_path)?;
                if let Some(active_token) = active.as_ref() {
                    if token_is_stale(active_token) {
                        clear_state_ops_active(&active_path)?;
                        queue.retain(|item| item != active_token);
                        write_state_ops_queue(&queue_path, &queue)?;
                    }
                }
                if active.is_none() && queue.first().map(|item| item == &token).unwrap_or(false) {
                    write_state_ops_active(&active_path, &token)?;
                    acquired = true;
                }
                Ok(())
            })?;
            if acquired {
                break;
            }
            std::thread::sleep(Duration::from_millis(STATE_OP_WAIT_MS));
        }

        Ok(Self {
            token,
            lock_path,
            queue_path,
            active_path,
        })
    }
}

impl Drop for StateOpGuard {
    fn drop(&mut self) {
        let _ = with_state_ops_lock(&self.lock_path, || {
            let mut queue = read_state_ops_queue(&self.queue_path)?;
            queue.retain(|item| item != &self.token);
            write_state_ops_queue(&self.queue_path, &queue)?;
            if read_state_ops_active(&self.active_path)?
                .as_ref()
                .map(|item| item == &self.token)
                .unwrap_or(false)
            {
                clear_state_ops_active(&self.active_path)?;
            }
            Ok(())
        });
    }
}

pub fn state_exports_dir() -> PathBuf {
    data_dir().join(STATE_EXPORTS_DIR)
}

pub fn state_export_path() -> PathBuf {
    state_exports_dir().join(format!("apkw-state-{}.zip", now_millis()))
}

pub fn save_state_archive(opts: &StateArchiveOptions) -> io::Result<StateArchiveResult> {
    save_state_archive_to(&state_export_path(), opts)
}

pub fn save_state_archive_to(
    output_path: &Path,
    opts: &StateArchiveOptions,
) -> io::Result<StateArchiveResult> {
    let base_dir = data_dir();
    if !base_dir.exists() {
        fs::create_dir_all(&base_dir)?;
    }
    if let Some(parent) = output_path.parent() {
        fs::create_dir_all(parent)?;
    }

    let output = fs::File::create(output_path)?;
    let mut zip = zip::ZipWriter::new(output);
    let options = FileOptions::default().compression_method(zip::CompressionMethod::Deflated);

    let mut file_count = 0;
    let mut dir_count = 0;
    let mut total_bytes = 0;

    let walker = WalkDir::new(&base_dir).follow_links(false).into_iter();
    for entry in walker.filter_entry(|entry| {
        if entry.path() == base_dir {
            return true;
        }
        let rel = match entry.path().strip_prefix(&base_dir) {
            Ok(rel) => rel,
            Err(_) => return false,
        };
        !should_exclude_rel(rel, opts)
    }) {
        let entry = entry.map_err(io::Error::other)?;
        let path = entry.path();
        if path == base_dir {
            continue;
        }
        let rel = path.strip_prefix(&base_dir).map_err(io::Error::other)?;
        if should_exclude_rel(rel, opts) {
            continue;
        }
        if entry.file_type().is_symlink() {
            continue;
        }
        let rel_str = rel.to_string_lossy().replace('\\', "/");
        if entry.file_type().is_dir() {
            zip.add_directory(format!("{rel_str}/"), options)
                .map_err(io::Error::other)?;
            dir_count += 1;
            continue;
        }
        let mut file = fs::File::open(path)?;
        zip.start_file(rel_str, options).map_err(io::Error::other)?;
        let copied = io::copy(&mut file, &mut zip)?;
        file_count += 1;
        total_bytes += copied;
    }

    zip.finish().map_err(io::Error::other)?;

    Ok(StateArchiveResult {
        output_path: output_path.to_path_buf(),
        file_count,
        dir_count,
        total_bytes,
    })
}

pub fn open_state_archive(path: &Path, opts: &StateArchiveOptions) -> io::Result<StateOpenResult> {
    if !path.is_file() {
        return Err(io::Error::new(
            io::ErrorKind::NotFound,
            "archive path does not exist",
        ));
    }

    let base_dir = data_dir();
    if !is_safe_data_dir(&base_dir) {
        return Err(io::Error::other(format!(
            "refusing to open state into unexpected path: {}",
            base_dir.display()
        )));
    }

    fs::create_dir_all(&base_dir)?;
    let stage_dir = state_ops_dir().join(format!("open-{}-{}", std::process::id(), now_millis()));
    let stage_payload_dir = stage_dir.join("payload");
    fs::create_dir_all(&stage_payload_dir)?;

    let file = fs::File::open(path)?;
    let mut archive = zip::ZipArchive::new(file).map_err(io::Error::other)?;

    let mut restored_files = 0;
    let mut restored_dirs = 0;

    for i in 0..archive.len() {
        let mut entry = archive.by_index(i).map_err(io::Error::other)?;
        let rel = match safe_archive_path(entry.name()) {
            Some(rel) => rel,
            None => continue,
        };
        if should_exclude_rel(&rel, opts) {
            continue;
        }
        let out_path = stage_payload_dir.join(&rel);
        if entry.is_dir() {
            fs::create_dir_all(&out_path)?;
            restored_dirs += 1;
            continue;
        }
        if let Some(parent) = out_path.parent() {
            fs::create_dir_all(parent)?;
        }
        let mut outfile = fs::File::create(&out_path)?;
        io::copy(&mut entry, &mut outfile)?;
        restored_files += 1;
        #[cfg(unix)]
        if let Some(mode) = entry.unix_mode() {
            let _ = fs::set_permissions(&out_path, fs::Permissions::from_mode(mode));
        }
    }

    if restored_files == 0 && restored_dirs == 0 {
        let _ = fs::remove_dir_all(&stage_dir);
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            "archive did not contain any restorable APKW state entries",
        ));
    }

    let preserved_dirs = preserve_dirs(opts);
    if base_dir.exists() {
        for entry in fs::read_dir(&base_dir)? {
            let entry = entry?;
            let name = entry.file_name();
            if preserved_dirs
                .iter()
                .any(|dir| dir == name.to_string_lossy().as_ref())
            {
                continue;
            }
            let path = entry.path();
            if path.is_dir() {
                fs::remove_dir_all(&path)?;
            } else {
                fs::remove_file(&path)?;
            }
        }
    } else {
        fs::create_dir_all(&base_dir)?;
    }

    for entry in fs::read_dir(&stage_payload_dir)? {
        let entry = entry?;
        let dest = base_dir.join(entry.file_name());
        fs::rename(entry.path(), dest)?;
    }

    fs::create_dir_all(state_exports_dir())?;
    let _ = fs::remove_dir_all(&stage_dir);

    Ok(StateOpenResult {
        restored_files,
        restored_dirs,
        preserved_dirs,
    })
}

fn state_ops_dir() -> PathBuf {
    data_dir().join(STATE_OPS_DIR)
}

fn temp_write_path(path: &Path) -> PathBuf {
    let file_name = path
        .file_name()
        .and_then(|name| name.to_str())
        .unwrap_or("state.json");
    let seq = TMP_FILE_SEQ.fetch_add(1, Ordering::Relaxed);
    path.parent()
        .unwrap_or_else(|| Path::new("."))
        .join(format!(
            ".{file_name}.tmp-{}-{}-{seq}",
            std::process::id(),
            now_millis()
        ))
}

#[cfg(unix)]
fn sync_dir(path: &Path) -> io::Result<()> {
    fs::File::open(path)?.sync_all()
}

#[cfg(not(unix))]
fn sync_dir(_path: &Path) -> io::Result<()> {
    Ok(())
}

fn with_state_ops_lock<T>(lock_path: &Path, op: impl FnOnce() -> io::Result<T>) -> io::Result<T> {
    let lock_file = fs::OpenOptions::new()
        .create(true)
        .read(true)
        .write(true)
        .truncate(false)
        .open(lock_path)?;
    lock_file.lock_exclusive()?;
    let result = op();
    let _ = lock_file.unlock();
    result
}

fn read_state_ops_queue(path: &Path) -> io::Result<Vec<String>> {
    match fs::read_to_string(path) {
        Ok(contents) => Ok(contents
            .lines()
            .map(|line| line.trim().to_string())
            .filter(|line| !line.is_empty())
            .collect()),
        Err(err) if err.kind() == io::ErrorKind::NotFound => Ok(Vec::new()),
        Err(err) => Err(err),
    }
}

fn write_state_ops_queue(path: &Path, queue: &[String]) -> io::Result<()> {
    if queue.is_empty() {
        if let Err(err) = fs::remove_file(path) {
            if err.kind() != io::ErrorKind::NotFound {
                return Err(err);
            }
        }
        return Ok(());
    }
    let mut contents = queue.join("\n");
    contents.push('\n');
    fs::write(path, contents)
}

fn read_state_ops_active(path: &Path) -> io::Result<Option<String>> {
    match fs::read_to_string(path) {
        Ok(contents) => {
            let token = contents.trim().to_string();
            if token.is_empty() {
                Ok(None)
            } else {
                Ok(Some(token))
            }
        }
        Err(err) if err.kind() == io::ErrorKind::NotFound => Ok(None),
        Err(err) => Err(err),
    }
}

fn write_state_ops_active(path: &Path, token: &str) -> io::Result<()> {
    fs::write(path, format!("{token}\n"))
}

fn clear_state_ops_active(path: &Path) -> io::Result<()> {
    if let Err(err) = fs::remove_file(path) {
        if err.kind() != io::ErrorKind::NotFound {
            return Err(err);
        }
    }
    Ok(())
}

fn token_is_stale(token: &str) -> bool {
    let Some((pid, ts)) = parse_state_ops_token(token) else {
        return true;
    };
    if !pid_is_alive(pid) {
        return true;
    }
    now_millis().saturating_sub(ts) > STATE_OP_STALE_MS
}

fn parse_state_ops_token(token: &str) -> Option<(u32, i64)> {
    let mut parts = token.splitn(3, ':');
    let _label = parts.next()?;
    let pid = parts.next()?.parse::<u32>().ok()?;
    let ts = parts.next()?.parse::<i64>().ok()?;
    Some((pid, ts))
}

fn pid_is_alive(pid: u32) -> bool {
    if pid == 0 {
        return false;
    }
    let res = unsafe { libc::kill(pid as i32, 0) };
    if res == 0 {
        return true;
    }
    let err = io::Error::last_os_error();
    matches!(err.raw_os_error(), Some(code) if code == libc::EPERM)
}

fn is_safe_data_dir(path: &Path) -> bool {
    path.file_name()
        .and_then(|name| name.to_str())
        .map(|name| name == "apkw")
        .unwrap_or(false)
}

fn safe_archive_path(name: &str) -> Option<PathBuf> {
    let path = Path::new(name);
    if path.is_absolute() {
        return None;
    }
    let mut out = PathBuf::new();
    for comp in path.components() {
        match comp {
            std::path::Component::Normal(part) => out.push(part),
            std::path::Component::CurDir => {}
            std::path::Component::ParentDir => return None,
            _ => return None,
        }
    }
    if out.as_os_str().is_empty() {
        return None;
    }
    Some(out)
}

fn should_exclude_rel(rel: &Path, opts: &StateArchiveOptions) -> bool {
    let Some(first) = rel.components().next() else {
        return false;
    };
    let std::path::Component::Normal(first) = first else {
        return true;
    };
    let name = first.to_string_lossy();
    if name == STATE_EXPORTS_DIR || name == STATE_OPS_DIR {
        return true;
    }
    if opts.exclude_downloads && name == LARGE_DIR_DOWNLOADS {
        return true;
    }
    if opts.exclude_toolchains && name == LARGE_DIR_TOOLCHAINS {
        return true;
    }
    if opts.exclude_bundles && name == LARGE_DIR_BUNDLES {
        return true;
    }
    if opts.exclude_telemetry && name == LARGE_DIR_TELEMETRY {
        return true;
    }
    false
}

fn preserve_dirs(opts: &StateArchiveOptions) -> Vec<String> {
    let mut dirs = vec![STATE_EXPORTS_DIR.to_string(), STATE_OPS_DIR.to_string()];
    if opts.exclude_downloads {
        dirs.push(LARGE_DIR_DOWNLOADS.to_string());
    }
    if opts.exclude_toolchains {
        dirs.push(LARGE_DIR_TOOLCHAINS.to_string());
    }
    if opts.exclude_bundles {
        dirs.push(LARGE_DIR_BUNDLES.to_string());
    }
    if opts.exclude_telemetry {
        dirs.push(LARGE_DIR_TELEMETRY.to_string());
    }
    dirs
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::{
        ffi::OsString,
        sync::{Mutex, OnceLock},
    };

    fn test_lock() -> &'static Mutex<()> {
        static LOCK: OnceLock<Mutex<()>> = OnceLock::new();
        LOCK.get_or_init(|| Mutex::new(()))
    }

    struct HomeGuard {
        previous: Option<OsString>,
    }

    impl Drop for HomeGuard {
        fn drop(&mut self) {
            if let Some(previous) = self.previous.take() {
                std::env::set_var("HOME", previous);
            } else {
                std::env::remove_var("HOME");
            }
        }
    }

    fn set_test_home(path: &Path) -> HomeGuard {
        let previous = std::env::var_os("HOME");
        std::env::set_var("HOME", path);
        HomeGuard { previous }
    }

    fn test_root(name: &str) -> PathBuf {
        let path = std::env::temp_dir().join(format!(
            "apkw-util-{name}-{}-{}",
            std::process::id(),
            now_millis()
        ));
        let _ = fs::remove_dir_all(&path);
        fs::create_dir_all(&path).unwrap();
        path
    }

    fn write_zip(path: &Path, entries: &[(&str, &[u8])]) {
        let file = fs::File::create(path).unwrap();
        let mut zip = zip::ZipWriter::new(file);
        let options = FileOptions::default().compression_method(zip::CompressionMethod::Deflated);
        for (name, contents) in entries {
            zip.start_file(*name, options).unwrap();
            zip.write_all(contents).unwrap();
        }
        zip.finish().unwrap();
    }

    #[test]
    fn open_state_archive_rejects_empty_archives_without_clearing_existing_state() {
        let _lock = test_lock().lock().unwrap();
        let home = test_root("empty-open");
        let _guard = set_test_home(&home);

        let base_dir = data_dir();
        fs::create_dir_all(base_dir.join("state")).unwrap();
        let existing_file = base_dir.join("state").join("projects.json");
        fs::write(&existing_file, br#"{"projects":[{"project_id":"demo"}]}"#).unwrap();

        let archive = home.join("empty.zip");
        let file = fs::File::create(&archive).unwrap();
        zip::ZipWriter::new(file).finish().unwrap();

        let err = open_state_archive(&archive, &StateArchiveOptions::default()).unwrap_err();
        assert_eq!(err.kind(), io::ErrorKind::InvalidData);
        assert!(existing_file.exists());

        let _ = fs::remove_dir_all(home);
    }

    #[test]
    fn open_state_archive_restores_state_from_same_filesystem_staging_area() {
        let _lock = test_lock().lock().unwrap();
        let home = test_root("roundtrip-open");
        let _guard = set_test_home(&home);

        let base_dir = data_dir();
        fs::create_dir_all(base_dir.join("state")).unwrap();
        fs::write(base_dir.join("state").join("projects.json"), b"before").unwrap();
        fs::write(base_dir.join("state").join("observe.json"), b"before").unwrap();

        let archive = home.join("state.zip");
        write_zip(
            &archive,
            &[
                (
                    "state/projects.json",
                    br#"{"projects":[{"project_id":"new"}]}"#,
                ),
                ("state/observe.json", br#"{"runs":[],"outputs":[]}"#),
            ],
        );

        let result = open_state_archive(&archive, &StateArchiveOptions::default()).unwrap();
        assert_eq!(result.restored_files, 2);
        assert_eq!(
            fs::read_to_string(base_dir.join("state").join("projects.json")).unwrap(),
            r#"{"projects":[{"project_id":"new"}]}"#
        );

        let _ = fs::remove_dir_all(home);
    }
}
