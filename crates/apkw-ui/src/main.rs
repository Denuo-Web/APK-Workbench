mod commands;
mod config;
mod models;
mod pages;
mod ui_events;
mod ui_state;
mod utils;
mod worker;

use std::{sync::Arc, thread};

use glib::prelude::*;
use gtk::gdk;
use gtk::gdk::prelude::{DisplayExt, MonitorExt};
use gtk::gio::prelude::ListModelExt;
use gtk::prelude::*;
use gtk4 as gtk;
use tokio::sync::mpsc;

use crate::utils::{infer_application_id_from_apk_path, infer_application_id_from_project};
use apkw_telemetry as telemetry;
use apkw_util::state_export_path;
use commands::{AppEvent, UiCommand};
use config::AppConfig;
use models::ActiveContext;
use pages::{
    page_console, page_evidence, page_home, page_jobs_history, page_projects, page_settings,
    page_targets, page_toolchains, page_workflow, select_project_path, select_zip_open_dialog,
    select_zip_save_dialog, BuildPage, EvidencePage, HomePage, JobsHistoryPage, Page, ProjectsPage,
    SettingsPage, TargetsPage, ToolchainsPage, WorkflowPage,
};
use ui_events::{UiEventQueue, DEFAULT_EVENT_QUEUE_SIZE};
use ui_state::UiState;
use worker::{handle_command, AppState};

fn main() {
    apkw_util::promote_legacy_env();
    let app = gtk::Application::builder()
        .application_id("dev.apkw.ui.full")
        .build();

    app.connect_activate(build_ui);
    app.run();
}

fn default_window_size() -> (i32, i32) {
    let base_width = 1100;
    let base_height = 700;
    let mut width = base_width;
    let mut height = base_height;

    if let Some(display) = gdk::Display::default() {
        let monitors = display.monitors();
        if let Some(item) = monitors.item(0) {
            if let Ok(monitor) = item.downcast::<gdk::Monitor>() {
                let geometry = monitor.geometry();
                let max_width = (geometry.width() as f32 * 0.9) as i32;
                let max_height = (geometry.height() as f32 * 0.9) as i32;
                if max_width > 0 {
                    width = width.min(max_width);
                }
                if max_height > 0 {
                    height = height.min(max_height);
                }
            }
        }
    }

    (width, height)
}

fn telemetry_env_override(name: &str) -> Option<bool> {
    match std::env::var(name) {
        Ok(value) => match value.to_ascii_lowercase().as_str() {
            "1" | "true" | "yes" | "on" => Some(true),
            "0" | "false" | "no" | "off" => Some(false),
            _ => None,
        },
        Err(_) => None,
    }
}

const APKW_APP_CSS: &str = r#"
window.apkw-app,
overlay.apkw-shell,
box.apkw-root,
stack.apkw-main-stack,
box.apkw-page-root,
box.apkw-page-body,
scrolledwindow.apkw-page-scroller,
scrolledwindow.apkw-page-scroller > viewport {
  background-color: %WINDOW_BG%;
}

box.apkw-solid-background {
  background-color: %WINDOW_BG%;
}

.apkw-left-rail {
  background-color: %WINDOW_BG%;
  background-image: linear-gradient(180deg, shade(%WINDOW_BG%, 1.02), shade(%WINDOW_BG%, 0.98));
  border-right: 1px solid alpha(@borders, 0.72);
}

box.apkw-brand,
frame.apkw-context-card > border,
box.apkw-page-intro,
frame.apkw-section > border,
box.apkw-log-panel {
  border-radius: 16px;
}

box.apkw-brand {
  margin: 8px;
  padding: 16px;
  background-image: linear-gradient(135deg, alpha(@accent_bg_color, 0.20), alpha(@accent_bg_color, 0.06));
  border: 1px solid alpha(@accent_bg_color, 0.22);
}

label.apkw-brand-title {
  font-weight: 700;
}

label.apkw-brand-copy {
  opacity: 0.88;
}

frame.apkw-context-card > border {
  background-color: %CARD_BG%;
  border-color: alpha(@borders, 0.72);
}

label.apkw-context-line {
  font-weight: 600;
}

button,
entry,
combobox,
dropdown {
  border-radius: 10px;
}

button.apkw-rail-button {
  min-height: 38px;
  border-radius: 12px;
  padding-left: 12px;
  padding-right: 12px;
}

button.apkw-primary-action,
button.apkw-destructive-action,
button.apkw-rail-button.suggested-action {
  font-weight: 700;
}

box.apkw-page-intro {
  padding: 16px;
  background-image: linear-gradient(180deg, shade(%CARD_BG%, 1.03), %CARD_BG%);
  border: 1px solid alpha(@borders, 0.72);
}

frame.apkw-section > border {
  background-color: %CARD_BG%;
  border-color: alpha(@borders, 0.72);
}

label.apkw-section-title,
label.apkw-log-title {
  font-weight: 700;
}

box.apkw-log-panel {
  padding: 12px;
  background-color: %VIEW_BG%;
  border: 1px solid alpha(@borders, 0.78);
}

scrolledwindow.apkw-log-scroller,
scrolledwindow.apkw-log-scroller > viewport,
textview.apkw-log-view,
textview.apkw-log-view text {
  background-color: %VIEW_BG%;
}

button.apkw-log-action {
  padding-left: 8px;
  padding-right: 8px;
}

stacksidebar.apkw-sidebar row {
  margin: 2px 8px;
  border-radius: 12px;
}

stacksidebar.apkw-sidebar row:selected,
stacksidebar.apkw-sidebar row:hover {
  background-color: alpha(@accent_bg_color, 0.12);
}

stacksidebar.apkw-sidebar label {
  font-weight: 600;
}
"#;

fn rgba_css(color: gdk::RGBA) -> String {
    let red = (color.red() * 255.0).round().clamp(0.0, 255.0) as u8;
    let green = (color.green() * 255.0).round().clamp(0.0, 255.0) as u8;
    let blue = (color.blue() * 255.0).round().clamp(0.0, 255.0) as u8;
    format!("rgba({red}, {green}, {blue}, 1.0)")
}

fn default_window_bg() -> gdk::RGBA {
    let prefer_dark = gtk::Settings::default()
        .map(|settings| settings.is_gtk_application_prefer_dark_theme())
        .unwrap_or(false);
    if prefer_dark {
        gdk::RGBA::new(0.12, 0.12, 0.13, 1.0)
    } else {
        gdk::RGBA::new(0.96, 0.96, 0.97, 1.0)
    }
}

#[allow(deprecated)]
fn lookup_opaque_theme_color(
    widget: &impl gtk::prelude::IsA<gtk::Widget>,
    names: &[&str],
    fallback: gdk::RGBA,
) -> gdk::RGBA {
    let style_context = widget.style_context();
    for name in names {
        if let Some(color) = style_context.lookup_color(name) {
            return color.with_alpha(1.0);
        }
    }
    fallback.with_alpha(1.0)
}

fn install_app_css(window: &gtk::ApplicationWindow) {
    let window_bg = lookup_opaque_theme_color(
        window,
        &["window_bg_color", "view_bg_color", "theme_bg_color"],
        default_window_bg(),
    );
    let card_bg = lookup_opaque_theme_color(
        window,
        &["card_bg_color", "window_bg_color", "view_bg_color"],
        window_bg,
    );
    let view_bg = lookup_opaque_theme_color(
        window,
        &["view_bg_color", "card_bg_color", "window_bg_color"],
        window_bg,
    );
    let css = APKW_APP_CSS
        .replace("%WINDOW_BG%", &rgba_css(window_bg))
        .replace("%CARD_BG%", &rgba_css(card_bg))
        .replace("%VIEW_BG%", &rgba_css(view_bg));
    let provider = gtk::CssProvider::new();
    provider.load_from_data(&css);
    if let Some(display) = gdk::Display::default() {
        gtk::style_context_add_provider_for_display(
            &display,
            &provider,
            gtk::STYLE_PROVIDER_PRIORITY_APPLICATION,
        );
    }
}

fn show_help_dialog(window: &gtk::ApplicationWindow) {
    let host_os = std::env::var("APKW_HOST_OS_PRETTY_NAME")
        .or_else(|_| std::env::var("APKW_HOST_OS_ID"))
        .unwrap_or_else(|_| "Unknown".into());
    let page_profile = std::env::var("APKW_HOST_PAGE_PROFILE").unwrap_or_else(|_| "unknown".into());
    let system_information = format!(
        "Host OS: {host_os}\nPage profile: {page_profile}\nJob service: {}",
        std::env::var("APKW_JOB_ADDR").unwrap_or_else(|_| "127.0.0.1:50051".into())
    );

    let dialog = gtk::AboutDialog::builder()
        .transient_for(window)
        .modal(true)
        .program_name("APK Workbench")
        .version(env!("CARGO_PKG_VERSION"))
        .comments(
            "GTK client for APK Workbench services.\n\nUse the sidebar pages to manage jobs, workflows, toolchains, projects, builds, targets, evidence, and settings.\nUse New Project and Open Project for project folders.\nUse Export Workspace and Import Workspace for APKW snapshots.",
        )
        .website(env!("CARGO_PKG_HOMEPAGE"))
        .website_label("Project homepage")
        .copyright("APK Workbench contributors")
        .license(env!("CARGO_PKG_LICENSE"))
        .wrap_license(true)
        .system_information(system_information)
        .build();
    dialog.set_authors(&["Denuo-Web contributors"]);
    dialog.present();
}

#[derive(Clone)]
struct ContextBar {
    project_label: gtk::Label,
    toolchain_label: gtk::Label,
    target_label: gtk::Label,
    run_label: gtk::Label,
}

impl ContextBar {
    fn set_context(&self, ctx: &ActiveContext) {
        let project_ref = if ctx.project_id.trim().is_empty() {
            ctx.project_path.trim()
        } else {
            ctx.project_id.trim()
        };
        self.project_label
            .set_text(&format!("Project: {}", format_context_value(project_ref)));
        self.toolchain_label.set_text(&format!(
            "Toolchain set: {}",
            format_context_value(ctx.toolchain_set_id.trim())
        ));
        self.target_label.set_text(&format!(
            "Target: {}",
            format_context_value(ctx.target_id.trim())
        ));
        self.run_label
            .set_text(&format!("Run: {}", format_context_value(ctx.run_id.trim())));
    }
}

fn format_context_value(value: &str) -> &str {
    if value.is_empty() {
        "-"
    } else {
        value
    }
}

const LEGACY_SAMPLE_APPLICATION_ID: &str = "com.example.sampleconsole";

fn apply_active_context(
    ctx: &ActiveContext,
    context_bar: &ContextBar,
    home: &HomePage,
    workflow: &WorkflowPage,
    projects: &ProjectsPage,
    targets: &TargetsPage,
    toolchains: &ToolchainsPage,
    console: &BuildPage,
) {
    context_bar.set_context(ctx);
    home.set_active_context(ctx);
    workflow.set_context(ctx);
    projects.set_active_context(ctx);
    targets.set_target_id(&ctx.target_id);
    toolchains.set_active_set_id(&ctx.toolchain_set_id);
    console.set_project_ref(&ctx.project_ref());
}

fn apply_projects_context_if_empty(projects: &ProjectsPage, ctx: &ActiveContext) {
    if projects.project_id_entry.text().trim().is_empty() && !ctx.project_id.trim().is_empty() {
        projects.project_id_entry.set_text(ctx.project_id.trim());
    }

    let current_toolchain = projects
        .toolchain_set_combo
        .active_id()
        .map(|id| id.to_string())
        .unwrap_or_default();
    if (current_toolchain.is_empty() || current_toolchain == "none")
        && !ctx.toolchain_set_id.trim().is_empty()
    {
        projects
            .toolchain_set_combo
            .set_active_id(Some(ctx.toolchain_set_id.trim()));
    }

    let current_target = projects
        .target_combo
        .active_id()
        .map(|id| id.to_string())
        .unwrap_or_default();
    if (current_target.is_empty() || current_target == "none") && !ctx.target_id.trim().is_empty() {
        projects
            .target_combo
            .set_active_id(Some(ctx.target_id.trim()));
    }
}

fn set_tooltip<W: gtk::prelude::IsA<gtk::Widget>>(widget: &W, text: &str) {
    widget.set_tooltip_text(Some(text));
}

fn set_page_text(page: &Page, text: &str) {
    page.buffer.set_text(text);
    let mut end = page.buffer.end_iter();
    page.textview.scroll_to_iter(&mut end, 0.0, false, 0.0, 0.0);
}

fn text_view_text(view: &gtk::TextView) -> String {
    let buffer = view.buffer();
    let start = buffer.start_iter();
    let end = buffer.end_iter();
    buffer.text(&start, &end, false).to_string()
}

fn combo_active_value(combo: &gtk::ComboBoxText) -> String {
    combo
        .active_id()
        .map(|id| id.to_string())
        .or_else(|| combo.active_text().map(|text| text.to_string()))
        .unwrap_or_default()
}

fn apply_dropdown_selection(dropdown: &gtk::DropDown, selected: u32) {
    let count = dropdown.model().map(|model| model.n_items()).unwrap_or(0);
    let value = if count == 0 {
        0
    } else if selected < count {
        selected
    } else {
        0
    };
    dropdown.set_selected(value);
}

#[allow(clippy::too_many_arguments)]
fn apply_ui_state(
    state: &UiState,
    home: &HomePage,
    workflow: &WorkflowPage,
    toolchains: &ToolchainsPage,
    projects: &ProjectsPage,
    targets: &TargetsPage,
    build: &BuildPage,
    jobs: &JobsHistoryPage,
    evidence: &EvidencePage,
    settings: &SettingsPage,
) {
    set_page_text(&home.page, &state.home.log);
    home.job_type_entry.set_text(&state.home.job_type);
    if !state.home.job_type.trim().is_empty() {
        home.job_type_combo
            .set_active_id(Some(state.home.job_type.as_str()));
    }
    home.params_view.buffer().set_text(&state.home.job_params);
    home.project_id_entry.set_text(&state.home.project_id);
    home.target_id_entry.set_text(&state.home.target_id);
    home.toolchain_id_entry
        .set_text(&state.home.toolchain_set_id);
    home.correlation_id_entry
        .set_text(&state.home.correlation_id);
    home.watch_entry.set_text(&state.home.watch_job_id);

    set_page_text(&workflow.page, &state.workflow.log);
    workflow.run_id_entry.set_text(&state.workflow.run_id);
    workflow
        .project_id_entry
        .set_text(&state.workflow.project_id);
    workflow
        .project_path_entry
        .set_text(&state.workflow.project_path);
    workflow
        .toolchain_set_entry
        .set_text(&state.workflow.toolchain_set_id);
    workflow.target_id_entry.set_text(&state.workflow.target_id);
    workflow
        .use_job_id_check
        .set_active(state.workflow.use_job_id);
    workflow.job_id_entry.set_text(&state.workflow.job_id);
    workflow
        .correlation_id_entry
        .set_text(&state.workflow.correlation_id);
    workflow
        .include_history_check
        .set_active(state.workflow.include_history);
    workflow
        .template_id_entry
        .set_text(&state.workflow.template_id);
    workflow
        .project_name_entry
        .set_text(&state.workflow.project_name);
    workflow
        .toolchain_id_entry
        .set_text(&state.workflow.toolchain_id);
    apply_dropdown_selection(&workflow.variant_combo, state.workflow.build_variant_index);
    workflow
        .variant_name_entry
        .set_text(&state.workflow.variant_name);
    workflow.module_entry.set_text(&state.workflow.module);
    workflow.tasks_entry.set_text(&state.workflow.tasks);
    workflow.apk_path_entry.set_text(&state.workflow.apk_path);
    workflow
        .application_id_entry
        .set_text(&state.workflow.application_id);
    workflow.activity_entry.set_text(&state.workflow.activity);
    workflow
        .auto_infer_check
        .set_active(state.workflow.auto_infer_steps);
    workflow.create_check.set_active(state.workflow.step_create);
    workflow.open_check.set_active(state.workflow.step_open);
    workflow.verify_check.set_active(state.workflow.step_verify);
    workflow.build_check.set_active(state.workflow.step_build);
    workflow
        .install_check
        .set_active(state.workflow.step_install);
    workflow.launch_check.set_active(state.workflow.step_launch);
    workflow
        .support_check
        .set_active(state.workflow.step_support);
    workflow
        .evidence_check
        .set_active(state.workflow.step_evidence);

    set_page_text(&toolchains.page, &state.toolchains.log);
    toolchains
        .use_job_id_check
        .set_active(state.toolchains.use_job_id);
    toolchains.job_id_entry.set_text(&state.toolchains.job_id);
    toolchains
        .correlation_id_entry
        .set_text(&state.toolchains.correlation_id);
    if !state.toolchains.sdk_version.trim().is_empty() {
        toolchains
            .sdk_version_combo
            .set_active_id(Some(state.toolchains.sdk_version.as_str()));
    }
    if !state.toolchains.ndk_version.trim().is_empty() {
        toolchains
            .ndk_version_combo
            .set_active_id(Some(state.toolchains.ndk_version.as_str()));
    }
    toolchains
        .toolchain_id_entry
        .set_text(&state.toolchains.toolchain_id);
    toolchains
        .update_version_entry
        .set_text(&state.toolchains.update_version);
    toolchains
        .verify_update_check
        .set_active(state.toolchains.verify_update);
    toolchains
        .remove_cached_check
        .set_active(state.toolchains.remove_cached);
    toolchains
        .force_uninstall_check
        .set_active(state.toolchains.force_uninstall);
    toolchains
        .dry_run_check
        .set_active(state.toolchains.dry_run);
    toolchains
        .remove_all_check
        .set_active(state.toolchains.remove_all);
    toolchains
        .sdk_set_entry
        .set_text(&state.toolchains.sdk_set_id);
    toolchains
        .ndk_set_entry
        .set_text(&state.toolchains.ndk_set_id);
    toolchains
        .display_name_entry
        .set_text(&state.toolchains.display_name);
    toolchains
        .active_set_entry
        .set_text(&state.toolchains.active_set_id);

    set_page_text(&projects.page, &state.projects.log);
    projects
        .use_job_id_check
        .set_active(state.projects.use_job_id);
    projects.job_id_entry.set_text(&state.projects.job_id);
    projects
        .correlation_id_entry
        .set_text(&state.projects.correlation_id);
    if !state.projects.template_id.trim().is_empty() {
        projects
            .template_combo
            .set_active_id(Some(state.projects.template_id.as_str()));
    }
    projects.name_entry.set_text(&state.projects.name);
    projects.path_entry.set_text(&state.projects.path);
    projects
        .project_id_entry
        .set_text(&state.projects.project_id);
    if !state.projects.toolchain_set_id.trim().is_empty() {
        projects
            .toolchain_set_combo
            .set_active_id(Some(state.projects.toolchain_set_id.as_str()));
    }
    if !state.projects.default_target_id.trim().is_empty() {
        projects
            .target_combo
            .set_active_id(Some(state.projects.default_target_id.as_str()));
    }

    set_page_text(&targets.page, &state.targets.log);
    targets
        .use_job_id_check
        .set_active(state.targets.use_job_id);
    targets.job_id_entry.set_text(&state.targets.job_id);
    targets
        .correlation_id_entry
        .set_text(&state.targets.correlation_id);
    targets
        .cuttlefish_branch_entry
        .set_text(&state.targets.cuttlefish_branch);
    targets
        .cuttlefish_target_entry
        .set_text(&state.targets.cuttlefish_target);
    targets
        .cuttlefish_build_entry
        .set_text(&state.targets.cuttlefish_build_id);
    targets.target_entry.set_text(&state.targets.target_id);
    targets.apk_entry.set_text(&state.targets.apk_path);
    targets.app_id_entry.set_text(&state.targets.application_id);
    targets.activity_entry.set_text(&state.targets.activity);

    set_page_text(&build.page, &state.build.log);
    build.project_entry.set_text(&state.build.project_ref);
    build.module_entry.set_text(&state.build.module);
    apply_dropdown_selection(&build.variant_combo, state.build.variant_index);
    build.variant_name_entry.set_text(&state.build.variant_name);
    build.tasks_entry.set_text(&state.build.tasks);
    build.args_entry.set_text(&state.build.gradle_args);
    build.clean_check.set_active(state.build.clean_first);
    build.use_job_id_check.set_active(state.build.use_job_id);
    build.job_id_entry.set_text(&state.build.job_id);
    build
        .correlation_id_entry
        .set_text(&state.build.correlation_id);
    build
        .artifact_modules_entry
        .set_text(&state.build.artifact_modules);
    build
        .artifact_variant_entry
        .set_text(&state.build.artifact_variant);
    build
        .artifact_types_entry
        .set_text(&state.build.artifact_types);
    build
        .artifact_name_entry
        .set_text(&state.build.artifact_name);
    build
        .artifact_path_entry
        .set_text(&state.build.artifact_path);

    set_page_text(&jobs.page, &state.jobs.log);
    jobs.job_types_entry.set_text(&state.jobs.job_types);
    jobs.states_entry.set_text(&state.jobs.states);
    jobs.created_after_entry.set_text(&state.jobs.created_after);
    jobs.created_before_entry
        .set_text(&state.jobs.created_before);
    jobs.finished_after_entry
        .set_text(&state.jobs.finished_after);
    jobs.finished_before_entry
        .set_text(&state.jobs.finished_before);
    jobs.correlation_id_entry
        .set_text(&state.jobs.correlation_id);
    jobs.page_size_entry.set_text(&state.jobs.page_size);
    jobs.page_token_entry.set_text(&state.jobs.page_token);
    jobs.job_id_entry.set_text(&state.jobs.job_id);
    jobs.kinds_entry.set_text(&state.jobs.kinds);
    jobs.after_entry.set_text(&state.jobs.after);
    jobs.before_entry.set_text(&state.jobs.before);
    jobs.history_page_size_entry
        .set_text(&state.jobs.history_page_size);
    jobs.history_page_token_entry
        .set_text(&state.jobs.history_page_token);
    jobs.output_path_entry.set_text(&state.jobs.output_path);

    set_page_text(&evidence.page, &state.evidence.log);
    evidence
        .use_job_id_check
        .set_active(state.evidence.use_job_id);
    evidence.job_id_entry.set_text(&state.evidence.job_id);
    evidence
        .correlation_id_entry
        .set_text(&state.evidence.correlation_id);
    evidence
        .job_log_output_path_entry
        .set_text(&state.evidence.job_log_output_path);
    evidence.run_id_entry.set_text(&state.evidence.run_id);
    let output_kind = if state.evidence.output_kind_index > 2 {
        0
    } else {
        state.evidence.output_kind_index
    };
    evidence.output_kind_combo.set_active(Some(output_kind));
    evidence
        .output_type_entry
        .set_text(&state.evidence.output_type);
    evidence
        .output_path_entry
        .set_text(&state.evidence.output_path);
    evidence
        .output_label_entry
        .set_text(&state.evidence.output_label);
    evidence
        .recent_limit_entry
        .set_text(&state.evidence.recent_limit);
    evidence
        .include_history_check
        .set_active(state.evidence.include_history);
    evidence
        .include_logs_check
        .set_active(state.evidence.include_logs);
    evidence
        .include_config_check
        .set_active(state.evidence.include_config);
    evidence
        .include_toolchain_check
        .set_active(state.evidence.include_toolchain);
    evidence
        .include_recent_check
        .set_active(state.evidence.include_recent);

    set_page_text(&settings.page, &state.settings.log);
    settings
        .exclude_downloads
        .set_active(state.settings.exclude_downloads);
    settings
        .exclude_toolchains
        .set_active(state.settings.exclude_toolchains);
    settings
        .exclude_bundles
        .set_active(state.settings.exclude_bundles);
    settings
        .exclude_telemetry
        .set_active(state.settings.exclude_telemetry);
    settings.save_entry.set_text(&state.settings.save_path);
    settings.open_entry.set_text(&state.settings.open_path);
}

#[allow(clippy::too_many_arguments)]
fn capture_ui_state(
    state: &mut UiState,
    home: &HomePage,
    workflow: &WorkflowPage,
    toolchains: &ToolchainsPage,
    projects: &ProjectsPage,
    targets: &TargetsPage,
    build: &BuildPage,
    jobs: &JobsHistoryPage,
    evidence: &EvidencePage,
    settings: &SettingsPage,
) {
    state.home.job_type = home.job_type_entry.text().to_string();
    state.home.job_params = text_view_text(&home.params_view);
    state.home.project_id = home.project_id_entry.text().to_string();
    state.home.target_id = home.target_id_entry.text().to_string();
    state.home.toolchain_set_id = home.toolchain_id_entry.text().to_string();
    state.home.correlation_id = home.correlation_id_entry.text().to_string();
    state.home.watch_job_id = home.watch_entry.text().to_string();

    state.workflow.run_id = workflow.run_id_entry.text().to_string();
    state.workflow.project_id = workflow.project_id_entry.text().to_string();
    state.workflow.project_path = workflow.project_path_entry.text().to_string();
    state.workflow.toolchain_set_id = workflow.toolchain_set_entry.text().to_string();
    state.workflow.target_id = workflow.target_id_entry.text().to_string();
    state.workflow.use_job_id = workflow.use_job_id_check.is_active();
    state.workflow.job_id = workflow.job_id_entry.text().to_string();
    state.workflow.correlation_id = workflow.correlation_id_entry.text().to_string();
    state.workflow.include_history = workflow.include_history_check.is_active();
    state.workflow.template_id = workflow.template_id_entry.text().to_string();
    state.workflow.project_name = workflow.project_name_entry.text().to_string();
    state.workflow.toolchain_id = workflow.toolchain_id_entry.text().to_string();
    state.workflow.build_variant_index = workflow.variant_combo.selected();
    state.workflow.variant_name = workflow.variant_name_entry.text().to_string();
    state.workflow.module = workflow.module_entry.text().to_string();
    state.workflow.tasks = workflow.tasks_entry.text().to_string();
    state.workflow.apk_path = workflow.apk_path_entry.text().to_string();
    state.workflow.application_id = workflow.application_id_entry.text().to_string();
    state.workflow.activity = workflow.activity_entry.text().to_string();
    state.workflow.auto_infer_steps = workflow.auto_infer_check.is_active();
    state.workflow.step_create = workflow.create_check.is_active();
    state.workflow.step_open = workflow.open_check.is_active();
    state.workflow.step_verify = workflow.verify_check.is_active();
    state.workflow.step_build = workflow.build_check.is_active();
    state.workflow.step_install = workflow.install_check.is_active();
    state.workflow.step_launch = workflow.launch_check.is_active();
    state.workflow.step_support = workflow.support_check.is_active();
    state.workflow.step_evidence = workflow.evidence_check.is_active();

    state.toolchains.use_job_id = toolchains.use_job_id_check.is_active();
    state.toolchains.job_id = toolchains.job_id_entry.text().to_string();
    state.toolchains.correlation_id = toolchains.correlation_id_entry.text().to_string();
    state.toolchains.sdk_version = combo_active_value(&toolchains.sdk_version_combo);
    state.toolchains.ndk_version = combo_active_value(&toolchains.ndk_version_combo);
    state.toolchains.toolchain_id = toolchains.toolchain_id_entry.text().to_string();
    state.toolchains.update_version = toolchains.update_version_entry.text().to_string();
    state.toolchains.verify_update = toolchains.verify_update_check.is_active();
    state.toolchains.remove_cached = toolchains.remove_cached_check.is_active();
    state.toolchains.force_uninstall = toolchains.force_uninstall_check.is_active();
    state.toolchains.dry_run = toolchains.dry_run_check.is_active();
    state.toolchains.remove_all = toolchains.remove_all_check.is_active();
    state.toolchains.sdk_set_id = toolchains.sdk_set_entry.text().to_string();
    state.toolchains.ndk_set_id = toolchains.ndk_set_entry.text().to_string();
    state.toolchains.display_name = toolchains.display_name_entry.text().to_string();
    state.toolchains.active_set_id = toolchains.active_set_entry.text().to_string();

    state.projects.use_job_id = projects.use_job_id_check.is_active();
    state.projects.job_id = projects.job_id_entry.text().to_string();
    state.projects.correlation_id = projects.correlation_id_entry.text().to_string();
    state.projects.template_id = combo_active_value(&projects.template_combo);
    state.projects.name = projects.name_entry.text().to_string();
    state.projects.path = projects.path_entry.text().to_string();
    state.projects.project_id = projects.project_id_entry.text().to_string();
    state.projects.toolchain_set_id = combo_active_value(&projects.toolchain_set_combo);
    state.projects.default_target_id = combo_active_value(&projects.target_combo);

    state.targets.use_job_id = targets.use_job_id_check.is_active();
    state.targets.job_id = targets.job_id_entry.text().to_string();
    state.targets.correlation_id = targets.correlation_id_entry.text().to_string();
    state.targets.cuttlefish_branch = targets.cuttlefish_branch_entry.text().to_string();
    state.targets.cuttlefish_target = targets.cuttlefish_target_entry.text().to_string();
    state.targets.cuttlefish_build_id = targets.cuttlefish_build_entry.text().to_string();
    state.targets.target_id = targets.target_entry.text().to_string();
    state.targets.apk_path = targets.apk_entry.text().to_string();
    state.targets.application_id = targets.app_id_entry.text().to_string();
    state.targets.activity = targets.activity_entry.text().to_string();

    state.build.project_ref = build.project_entry.text().to_string();
    state.build.module = build.module_entry.text().to_string();
    state.build.variant_index = build.variant_combo.selected();
    state.build.variant_name = build.variant_name_entry.text().to_string();
    state.build.tasks = build.tasks_entry.text().to_string();
    state.build.gradle_args = build.args_entry.text().to_string();
    state.build.clean_first = build.clean_check.is_active();
    state.build.use_job_id = build.use_job_id_check.is_active();
    state.build.job_id = build.job_id_entry.text().to_string();
    state.build.correlation_id = build.correlation_id_entry.text().to_string();
    state.build.artifact_modules = build.artifact_modules_entry.text().to_string();
    state.build.artifact_variant = build.artifact_variant_entry.text().to_string();
    state.build.artifact_types = build.artifact_types_entry.text().to_string();
    state.build.artifact_name = build.artifact_name_entry.text().to_string();
    state.build.artifact_path = build.artifact_path_entry.text().to_string();

    state.jobs.job_types = jobs.job_types_entry.text().to_string();
    state.jobs.states = jobs.states_entry.text().to_string();
    state.jobs.created_after = jobs.created_after_entry.text().to_string();
    state.jobs.created_before = jobs.created_before_entry.text().to_string();
    state.jobs.finished_after = jobs.finished_after_entry.text().to_string();
    state.jobs.finished_before = jobs.finished_before_entry.text().to_string();
    state.jobs.correlation_id = jobs.correlation_id_entry.text().to_string();
    state.jobs.page_size = jobs.page_size_entry.text().to_string();
    state.jobs.page_token = jobs.page_token_entry.text().to_string();
    state.jobs.job_id = jobs.job_id_entry.text().to_string();
    state.jobs.kinds = jobs.kinds_entry.text().to_string();
    state.jobs.after = jobs.after_entry.text().to_string();
    state.jobs.before = jobs.before_entry.text().to_string();
    state.jobs.history_page_size = jobs.history_page_size_entry.text().to_string();
    state.jobs.history_page_token = jobs.history_page_token_entry.text().to_string();
    state.jobs.output_path = jobs.output_path_entry.text().to_string();

    state.evidence.use_job_id = evidence.use_job_id_check.is_active();
    state.evidence.job_id = evidence.job_id_entry.text().to_string();
    state.evidence.correlation_id = evidence.correlation_id_entry.text().to_string();
    state.evidence.job_log_output_path = evidence.job_log_output_path_entry.text().to_string();
    state.evidence.run_id = evidence.run_id_entry.text().to_string();
    state.evidence.output_kind_index = evidence.output_kind_combo.active().unwrap_or(0);
    state.evidence.output_type = evidence.output_type_entry.text().to_string();
    state.evidence.output_path = evidence.output_path_entry.text().to_string();
    state.evidence.output_label = evidence.output_label_entry.text().to_string();
    state.evidence.recent_limit = evidence.recent_limit_entry.text().to_string();
    state.evidence.include_history = evidence.include_history_check.is_active();
    state.evidence.include_logs = evidence.include_logs_check.is_active();
    state.evidence.include_config = evidence.include_config_check.is_active();
    state.evidence.include_toolchain = evidence.include_toolchain_check.is_active();
    state.evidence.include_recent = evidence.include_recent_check.is_active();

    state.settings.exclude_downloads = settings.exclude_downloads.is_active();
    state.settings.exclude_toolchains = settings.exclude_toolchains.is_active();
    state.settings.exclude_bundles = settings.exclude_bundles.is_active();
    state.settings.exclude_telemetry = settings.exclude_telemetry.is_active();
    state.settings.save_path = settings.save_entry.text().to_string();
    state.settings.open_path = settings.open_entry.text().to_string();
}

#[allow(clippy::too_many_arguments)]
fn persist_ui_state_snapshot(
    ui_state: &Arc<std::sync::Mutex<UiState>>,
    last_saved_ui_state: &Arc<std::sync::Mutex<Option<UiState>>>,
    home: &HomePage,
    workflow: &WorkflowPage,
    toolchains: &ToolchainsPage,
    projects: &ProjectsPage,
    targets: &TargetsPage,
    build: &BuildPage,
    jobs: &JobsHistoryPage,
    evidence: &EvidencePage,
    settings: &SettingsPage,
) -> std::io::Result<bool> {
    let snapshot = {
        let mut state = ui_state.lock().unwrap();
        capture_ui_state(
            &mut state, home, workflow, toolchains, projects, targets, build, jobs, evidence,
            settings,
        );
        let last_saved = last_saved_ui_state.lock().unwrap();
        let should_save = last_saved
            .as_ref()
            .map(|saved| saved != &*state)
            .unwrap_or(true);
        if !should_save {
            return Ok(false);
        }
        state.clone()
    };

    snapshot.save()?;
    let mut last_saved = last_saved_ui_state.lock().unwrap();
    *last_saved = Some(snapshot);
    Ok(true)
}

fn build_ui(app: &gtk::Application) {
    let (default_width, default_height) = default_window_size();
    let window = gtk::ApplicationWindow::builder()
        .application(app)
        .title("APK Workbench")
        .default_width(default_width)
        .default_height(default_height)
        .resizable(true)
        .build();
    window.add_css_class("apkw-app");
    window.set_opacity(1.0);
    install_app_css(&window);

    let cfg = Arc::new(std::sync::Mutex::new(AppConfig::load()));
    let (initial_state, has_ui_state) = UiState::load_with_status();
    let ui_state = Arc::new(std::sync::Mutex::new(initial_state.clone()));
    let last_saved_ui_state = Arc::new(std::sync::Mutex::new(if has_ui_state {
        Some(initial_state.clone())
    } else {
        None
    }));
    let (usage_enabled, crash_enabled, install_id) = {
        let mut cfg = cfg.lock().unwrap();
        let usage_enabled =
            telemetry_env_override("APKW_TELEMETRY").unwrap_or(cfg.telemetry_usage_enabled);
        let crash_enabled =
            telemetry_env_override("APKW_TELEMETRY_CRASH").unwrap_or(cfg.telemetry_crash_enabled);
        cfg.telemetry_usage_enabled = usage_enabled;
        cfg.telemetry_crash_enabled = crash_enabled;
        if (usage_enabled || crash_enabled) && cfg.telemetry_install_id.trim().is_empty() {
            cfg.telemetry_install_id = telemetry::generate_install_id();
            if let Err(err) = cfg.save() {
                eprintln!("Failed to persist UI config: {err}");
            }
        }
        (
            usage_enabled,
            crash_enabled,
            cfg.telemetry_install_id.clone(),
        )
    };
    telemetry::init(telemetry::TelemetryOptions {
        app_name: "apkw-ui",
        app_version: env!("CARGO_PKG_VERSION"),
        usage_enabled,
        crash_enabled,
        install_id: if install_id.trim().is_empty() {
            None
        } else {
            Some(install_id)
        },
    });
    telemetry::event("app.start", &[]);
    let state = Arc::new(std::sync::Mutex::new(AppState::default()));
    let pending_project_prompt = Arc::new(std::sync::Mutex::new(false));

    let (cmd_tx, cmd_rx) = mpsc::channel::<UiCommand>(128);
    let (event_queue, mut notify_rx) = UiEventQueue::new(DEFAULT_EVENT_QUEUE_SIZE);
    let ui_events = event_queue.sender();

    // Background thread with tokio runtime; holds a private copy of AppState for worker actions.
    // State mutations are pushed to GTK via AppEvent.
    let mut cmd_rx = cmd_rx;
    thread::spawn(move || {
        let rt = tokio::runtime::Builder::new_multi_thread()
            .enable_all()
            .build()
            .expect("failed to build tokio runtime");

        rt.block_on(async move {
            let mut worker_state = AppState::default();
            let mut stream_tasks = tokio::task::JoinSet::new();

            loop {
                tokio::select! {
                    cmd = cmd_rx.recv() => {
                        let Some(cmd) = cmd else { break };
                        let cmd_name = cmd.name();
                        telemetry::event("ui.command.start", &[("command", cmd_name)]);
                        let result = handle_command(
                            cmd,
                            &mut worker_state,
                            ui_events.clone(),
                            &mut stream_tasks,
                        )
                        .await;
                        match result {
                            Ok(()) => {
                                telemetry::event(
                                    "ui.command.result",
                                    &[("command", cmd_name), ("result", "ok")],
                                );
                            }
                            Err(err) => {
                                telemetry::event(
                                    "ui.command.result",
                                    &[("command", cmd_name), ("result", "err")],
                                );
                                eprintln!("worker error: {err}");
                            }
                        }
                    }
                    Some(result) = stream_tasks.join_next(), if !stream_tasks.is_empty() => {
                        if let Err(err) = result {
                            if !err.is_cancelled() {
                                eprintln!("stream task error: {err}");
                            }
                        }
                    }
                }
            }
        });
    });

    // Layout: context + actions + sidebar + stack
    let root = gtk::Box::new(gtk::Orientation::Horizontal, 0);
    root.add_css_class("apkw-root");
    root.set_opacity(1.0);
    let sidebar_width = 94;
    let left_column = gtk::Box::new(gtk::Orientation::Vertical, 6);
    left_column.add_css_class("apkw-left-rail");
    left_column.set_width_request(sidebar_width);
    left_column.set_hexpand(false);
    left_column.set_vexpand(true);
    left_column.set_opacity(1.0);

    let stack = gtk::Stack::builder()
        .transition_type(gtk::StackTransitionType::SlideLeftRight)
        .hexpand(true)
        .vexpand(true)
        .build();
    stack.add_css_class("apkw-main-stack");
    stack.set_opacity(1.0);
    stack.set_margin_top(8);
    stack.set_margin_bottom(8);
    stack.set_margin_start(8);
    stack.set_margin_end(10);

    let sidebar = gtk::StackSidebar::builder()
        .stack(&stack)
        .width_request(sidebar_width)
        .build();
    sidebar.add_css_class("apkw-sidebar");
    sidebar.set_vexpand(true);

    let brand_box = gtk::Box::new(gtk::Orientation::Vertical, 4);
    brand_box.add_css_class("apkw-brand");
    let brand_title = gtk::Label::builder()
        .label("APK Workbench")
        .xalign(0.0)
        .css_classes(vec!["title-3"])
        .build();
    brand_title.add_css_class("apkw-brand-title");
    brand_title.set_wrap(true);
    brand_title.set_max_width_chars(10);
    let brand_copy = gtk::Label::builder()
        .label("Projects, builds, targets, and run evidence in one GTK workspace.")
        .xalign(0.0)
        .wrap(true)
        .build();
    brand_copy.add_css_class("dim-label");
    brand_copy.add_css_class("apkw-brand-copy");
    brand_copy.set_width_chars(14);
    brand_copy.set_max_width_chars(14);
    brand_box.set_hexpand(true);
    brand_box.set_halign(gtk::Align::Fill);
    brand_box.set_width_request(116);
    brand_box.append(&brand_title);
    brand_box.append(&brand_copy);

    let context_frame = gtk::Frame::builder().label("Active context").build();
    context_frame.add_css_class("apkw-context-card");
    context_frame.set_margin_top(8);
    context_frame.set_margin_bottom(6);
    context_frame.set_margin_start(8);
    context_frame.set_margin_end(8);

    let context_grid = gtk::Grid::builder()
        .row_spacing(4)
        .column_spacing(0)
        .build();
    let project_label = gtk::Label::builder()
        .label("Project: -")
        .xalign(0.0)
        .build();
    let toolchain_label = gtk::Label::builder()
        .label("Toolchain set: -")
        .xalign(0.0)
        .build();
    let target_label = gtk::Label::builder().label("Target: -").xalign(0.0).build();
    let run_label = gtk::Label::builder().label("Run: -").xalign(0.0).build();
    for label in [&project_label, &toolchain_label, &target_label, &run_label] {
        label.add_css_class("apkw-context-line");
        label.set_wrap(false);
        label.set_selectable(true);
    }
    let context_bar = ContextBar {
        project_label: project_label.clone(),
        toolchain_label: toolchain_label.clone(),
        target_label: target_label.clone(),
        run_label: run_label.clone(),
    };

    let header_row = gtk::Box::new(gtk::Orientation::Horizontal, 8);
    header_row.set_margin_end(8);
    header_row.set_margin_bottom(4);
    header_row.set_halign(gtk::Align::Fill);
    header_row.set_hexpand(true);
    let project_action_row = gtk::Box::new(gtk::Orientation::Vertical, 6);
    project_action_row.set_margin_top(8);
    project_action_row.set_margin_bottom(8);
    project_action_row.set_halign(gtk::Align::End);
    project_action_row.set_width_request(96);
    let help_btn = gtk::Button::with_label("Help");
    let new_project_btn = gtk::Button::with_label("New\nProject");
    let open_project_btn = gtk::Button::with_label("Open\nProject");
    for btn in [&new_project_btn, &open_project_btn, &help_btn] {
        btn.set_halign(gtk::Align::Fill);
        btn.set_hexpand(true);
        btn.set_width_request(96);
        btn.add_css_class("apkw-rail-button");
    }
    new_project_btn.add_css_class("suggested-action");
    set_tooltip(
        &new_project_btn,
        "What: Start a new project. Why: reset local UI/workspace state and pick a fresh project folder. How: confirm reset, then choose a project folder.",
    );
    set_tooltip(
        &open_project_btn,
        "What: Open an existing APKW project folder. Why: switch to an already created project without resetting workspace snapshots. How: choose a folder containing .apkw/project.json.",
    );
    set_tooltip(
        &help_btn,
        "What: Show application help and about details. Why: review version, project links, and basic navigation guidance. How: open the standard GTK about dialog.",
    );
    project_action_row.append(&new_project_btn);
    project_action_row.append(&open_project_btn);
    project_action_row.append(&help_btn);

    let workspace_frame = gtk::Frame::builder().label("Workspace").build();
    workspace_frame.add_css_class("apkw-context-card");
    workspace_frame.set_margin_top(6);
    workspace_frame.set_margin_bottom(6);
    workspace_frame.set_margin_start(8);
    workspace_frame.set_margin_end(8);
    let workspace_action_row = gtk::Box::new(gtk::Orientation::Horizontal, 6);
    workspace_action_row.set_margin_top(12);
    workspace_action_row.set_margin_bottom(12);
    workspace_action_row.set_margin_start(12);
    workspace_action_row.set_margin_end(12);
    workspace_action_row.set_homogeneous(true);
    let export_workspace_btn = gtk::Button::with_label("Export\nWorkspace");
    let import_workspace_btn = gtk::Button::with_label("Import\nWorkspace");
    for btn in [&export_workspace_btn, &import_workspace_btn] {
        btn.set_halign(gtk::Align::Fill);
        btn.set_hexpand(true);
        btn.set_width_request(0);
        btn.add_css_class("apkw-rail-button");
    }
    export_workspace_btn.add_css_class("suggested-action");
    set_tooltip(
        &export_workspace_btn,
        "What: Export the current APKW workspace snapshot. Why: back up APKW state separately from project folders. How: choose a zip archive path (exclusions from Settings apply).",
    );
    set_tooltip(
        &import_workspace_btn,
        "What: Import an APKW workspace snapshot. Why: restore APKW state separately from project folders. How: choose a zip archive (exclusions from Settings apply).",
    );
    workspace_action_row.append(&export_workspace_btn);
    workspace_action_row.append(&import_workspace_btn);
    workspace_frame.set_child(Some(&workspace_action_row));

    context_grid.attach(&project_label, 0, 0, 1, 1);
    context_grid.attach(&toolchain_label, 0, 1, 1, 1);
    context_grid.attach(&target_label, 0, 2, 1, 1);
    context_grid.attach(&run_label, 0, 3, 1, 1);

    let context_scroller = gtk::ScrolledWindow::builder()
        .hexpand(true)
        .vexpand(false)
        .hscrollbar_policy(gtk::PolicyType::Automatic)
        .vscrollbar_policy(gtk::PolicyType::Never)
        .build();
    context_scroller.set_propagate_natural_width(false);
    context_scroller.set_min_content_width(0);
    context_scroller.set_child(Some(&context_grid));
    context_scroller.set_has_frame(false);

    context_frame.set_child(Some(&context_scroller));

    header_row.append(&brand_box);
    header_row.append(&project_action_row);

    left_column.append(&header_row);
    left_column.append(&context_frame);
    left_column.append(&workspace_frame);
    left_column.append(&sidebar);

    root.append(&left_column);
    root.append(&stack);

    // Pages
    let home = page_home(cfg.clone(), cmd_tx.clone());
    let workflow = page_workflow(cfg.clone(), cmd_tx.clone(), &window);
    let jobs_history = page_jobs_history(cfg.clone(), cmd_tx.clone());
    let toolchains = page_toolchains(cfg.clone(), cmd_tx.clone());
    let projects = page_projects(cfg.clone(), cmd_tx.clone(), &window);
    let targets = page_targets(cfg.clone(), cmd_tx.clone(), &window);
    let evidence = page_evidence(cfg.clone(), cmd_tx.clone());
    let console = page_console(cfg.clone(), cmd_tx.clone(), &window);
    let settings = page_settings(cfg.clone(), cmd_tx.clone(), &window);

    {
        let cfg = cfg.lock().unwrap().clone();
        cmd_tx
            .try_send(UiCommand::ProjectListTemplates { cfg })
            .ok();
    }

    let stack_for_new = stack.clone();
    let cfg_reset = cfg.clone();
    let cmd_tx_reset = cmd_tx.clone();
    let window_reset = window.clone();
    let pending_project_prompt_reset = pending_project_prompt.clone();
    new_project_btn.connect_clicked(move |_| {
        stack_for_new.set_visible_child_name("projects");
        let dialog = gtk::MessageDialog::builder()
            .transient_for(&window_reset)
            .modal(true)
            .message_type(gtk::MessageType::Warning)
            .text("Reset all local APKW state before starting a new project?")
            .secondary_text("This deletes cached state, job history, toolchains, downloads, bundles, and UI selections. Running jobs will keep going.")
            .build();
        dialog.add_buttons(&[
            ("Cancel", gtk::ResponseType::Cancel),
            ("Reset and continue", gtk::ResponseType::Accept),
        ]);
        let cmd_tx_confirm = cmd_tx_reset.clone();
        let cfg_confirm = cfg_reset.clone();
        let pending_confirm = pending_project_prompt_reset.clone();
        let window_confirm = window_reset.clone();
        dialog.connect_response(move |dialog, response| {
            if response == gtk::ResponseType::Accept {
                let cfg = cfg_confirm.lock().unwrap().clone();
                if cmd_tx_confirm
                    .try_send(UiCommand::ResetAllState { cfg: cfg.clone() })
                    .is_ok()
                {
                    *pending_confirm.lock().unwrap() = true;
                } else {
                    *pending_confirm.lock().unwrap() = true;
                    let cmd_tx_async = cmd_tx_confirm.clone();
                    let pending_async = pending_confirm.clone();
                    let window_async = window_confirm.clone();
                    glib::MainContext::default().spawn_local(async move {
                        if cmd_tx_async
                            .send(UiCommand::ResetAllState { cfg })
                            .await
                            .is_err()
                        {
                            *pending_async.lock().unwrap() = false;
                            let error_dialog = gtk::MessageDialog::builder()
                                .transient_for(&window_async)
                                .modal(true)
                                .message_type(gtk::MessageType::Error)
                                .text("Failed to queue reset request")
                                .secondary_text(
                                    "The UI command queue is unavailable. Try restarting APK Workbench.",
                                )
                                .build();
                            error_dialog.add_button("OK", gtk::ResponseType::Close);
                            error_dialog.connect_response(|dialog, _| dialog.close());
                            error_dialog.show();
                        }
                    });
                }
            }
            dialog.close();
        });
        dialog.show();
    });

    let stack_for_open_project = stack.clone();
    let window_open_project = window.clone();
    let projects_path_entry_open = projects.path_entry.clone();
    let cfg_open_project = cfg.clone();
    let cmd_tx_open_project = cmd_tx.clone();
    open_project_btn.connect_clicked(move |_| {
        stack_for_open_project.set_visible_child_name("projects");
        select_project_path(
            &window_open_project,
            &projects_path_entry_open,
            &cfg_open_project,
            &cmd_tx_open_project,
        );
    });

    let cfg_state_save = cfg.clone();
    let cmd_tx_state_save = cmd_tx.clone();
    let exclude_downloads_save = settings.exclude_downloads.clone();
    let exclude_toolchains_save = settings.exclude_toolchains.clone();
    let exclude_bundles_save = settings.exclude_bundles.clone();
    let exclude_telemetry_save = settings.exclude_telemetry.clone();
    let save_entry_state = settings.save_entry.clone();
    let window_state_save = window.clone();
    let ui_state_state_save = ui_state.clone();
    let last_saved_ui_state_state_save = last_saved_ui_state.clone();
    let home_state_save = home.clone();
    let workflow_state_save = workflow.clone();
    let toolchains_state_save = toolchains.clone();
    let projects_state_save = projects.clone();
    let targets_state_save = targets.clone();
    let console_state_save = console.clone();
    let jobs_history_state_save = jobs_history.clone();
    let evidence_state_save = evidence.clone();
    let settings_state_save = settings.clone();
    export_workspace_btn.connect_clicked(move |_| {
        let cfg_state_save = cfg_state_save.clone();
        let cmd_tx_state_save = cmd_tx_state_save.clone();
        let exclude_downloads_save = exclude_downloads_save.clone();
        let exclude_toolchains_save = exclude_toolchains_save.clone();
        let exclude_bundles_save = exclude_bundles_save.clone();
        let exclude_telemetry_save = exclude_telemetry_save.clone();
        let save_entry_dialog = save_entry_state.clone();
        let ui_state_state_save = ui_state_state_save.clone();
        let last_saved_ui_state_state_save = last_saved_ui_state_state_save.clone();
        let home_state_save = home_state_save.clone();
        let workflow_state_save = workflow_state_save.clone();
        let toolchains_state_save = toolchains_state_save.clone();
        let projects_state_save = projects_state_save.clone();
        let targets_state_save = targets_state_save.clone();
        let console_state_save = console_state_save.clone();
        let jobs_history_state_save = jobs_history_state_save.clone();
        let evidence_state_save = evidence_state_save.clone();
        let settings_state_save = settings_state_save.clone();
        let default_name = state_export_path()
            .file_name()
            .and_then(|name| name.to_str())
            .unwrap_or("apkw-state.zip")
            .to_string();
        select_zip_save_dialog(
            &window_state_save,
            &save_entry_dialog,
            "Export APKW Workspace Archive",
            Some(default_name),
            Some(Box::new(move |path| {
                if let Err(err) = persist_ui_state_snapshot(
                    &ui_state_state_save,
                    &last_saved_ui_state_state_save,
                    &home_state_save,
                    &workflow_state_save,
                    &toolchains_state_save,
                    &projects_state_save,
                    &targets_state_save,
                    &console_state_save,
                    &jobs_history_state_save,
                    &evidence_state_save,
                    &settings_state_save,
                ) {
                    eprintln!("Failed to persist UI state before archive save: {err}");
                }
                let cfg = cfg_state_save.lock().unwrap().clone();
                cmd_tx_state_save
                    .try_send(UiCommand::StateSave {
                        cfg,
                        output_path: path,
                        exclude_downloads: exclude_downloads_save.is_active(),
                        exclude_toolchains: exclude_toolchains_save.is_active(),
                        exclude_bundles: exclude_bundles_save.is_active(),
                        exclude_telemetry: exclude_telemetry_save.is_active(),
                    })
                    .ok();
            })),
        );
    });

    let cfg_state_open = cfg.clone();
    let cmd_tx_state_open = cmd_tx.clone();
    let exclude_downloads_open = settings.exclude_downloads.clone();
    let exclude_toolchains_open = settings.exclude_toolchains.clone();
    let exclude_bundles_open = settings.exclude_bundles.clone();
    let exclude_telemetry_open = settings.exclude_telemetry.clone();
    let open_entry_state = settings.open_entry.clone();
    let window_state_open = window.clone();
    import_workspace_btn.connect_clicked(move |_| {
        let cfg_state_open = cfg_state_open.clone();
        let cmd_tx_state_open = cmd_tx_state_open.clone();
        let exclude_downloads_open = exclude_downloads_open.clone();
        let exclude_toolchains_open = exclude_toolchains_open.clone();
        let exclude_bundles_open = exclude_bundles_open.clone();
        let exclude_telemetry_open = exclude_telemetry_open.clone();
        let open_entry_dialog = open_entry_state.clone();
        select_zip_open_dialog(
            &window_state_open,
            &open_entry_dialog,
            "Import APKW Workspace Archive",
            Some(Box::new(move |path| {
                let cfg = cfg_state_open.lock().unwrap().clone();
                cmd_tx_state_open
                    .try_send(UiCommand::StateOpen {
                        cfg,
                        archive_path: path,
                        exclude_downloads: exclude_downloads_open.is_active(),
                        exclude_toolchains: exclude_toolchains_open.is_active(),
                        exclude_bundles: exclude_bundles_open.is_active(),
                        exclude_telemetry: exclude_telemetry_open.is_active(),
                    })
                    .ok();
            })),
        );
    });

    let window_help = window.clone();
    help_btn.connect_clicked(move |_| {
        show_help_dialog(&window_help);
    });

    {
        let cfg = cfg.lock().unwrap().clone();
        let ctx = cfg.active_context();
        apply_active_context(
            &ctx,
            &context_bar,
            &home,
            &workflow,
            &projects,
            &targets,
            &toolchains,
            &console,
        );
    }
    if has_ui_state {
        apply_ui_state(
            &initial_state,
            &home,
            &workflow,
            &toolchains,
            &projects,
            &targets,
            &console,
            &jobs_history,
            &evidence,
            &settings,
        );
    }

    {
        let ui_state = ui_state.clone();
        let last_saved_ui_state = last_saved_ui_state.clone();
        let home = home.clone();
        let workflow = workflow.clone();
        let toolchains = toolchains.clone();
        let projects = projects.clone();
        let targets = targets.clone();
        let console = console.clone();
        let jobs_history = jobs_history.clone();
        let evidence = evidence.clone();
        let settings = settings.clone();
        glib::timeout_add_local(std::time::Duration::from_secs(1), move || {
            if let Err(err) = persist_ui_state_snapshot(
                &ui_state,
                &last_saved_ui_state,
                &home,
                &workflow,
                &toolchains,
                &projects,
                &targets,
                &console,
                &jobs_history,
                &evidence,
                &settings,
            ) {
                eprintln!("Failed to persist UI state: {err}");
            }
            glib::ControlFlow::Continue
        });
    }

    stack.add_titled(&home.page.root, Some("home"), "Job Control");
    stack.add_titled(&workflow.page.root, Some("workflow"), "Workflow");
    stack.add_titled(&toolchains.page.root, Some("toolchains"), "Toolchains");
    stack.add_titled(&projects.page.root, Some("projects"), "Projects");
    stack.add_titled(&console.page.root, Some("console"), "Build");
    stack.add_titled(&targets.page.root, Some("targets"), "Targets");
    stack.add_titled(&jobs_history.page.root, Some("jobs"), "Job History");
    stack.add_titled(&evidence.page.root, Some("evidence"), "Evidence");
    stack.add_titled(&settings.page.root, Some("settings"), "Settings");

    let cfg_for_stack = cfg.clone();
    let cmd_tx_for_stack = cmd_tx.clone();
    stack.connect_visible_child_notify(move |stack| {
        if let Some(name) = stack.visible_child_name() {
            telemetry::event("ui.page.view", &[("page", name.as_str())]);
            if name.as_str() == "projects" {
                let cfg = cfg_for_stack.lock().unwrap().clone();
                cmd_tx_for_stack
                    .try_send(UiCommand::ProjectListTemplates { cfg })
                    .ok();
            }
        }
    });

    // Clone page handles for event routing closure.
    let home_page_for_events = home.clone();
    let workflow_for_events = workflow.clone();
    let jobs_for_events = jobs_history.clone();
    let toolchains_for_events = toolchains.clone();
    let projects_for_events = projects.clone();
    let targets_for_events = targets.clone();
    let console_for_events = console.clone();
    let evidence_for_events = evidence.clone();
    let settings_for_events = settings.clone();
    let context_bar_for_events = context_bar.clone();
    let cfg_for_events = cfg.clone();
    let ui_state_for_events = ui_state.clone();
    let last_saved_ui_state_for_events = last_saved_ui_state.clone();
    let pending_project_prompt_for_events = pending_project_prompt.clone();
    let window_for_events = window.clone();
    let cmd_tx_for_events = cmd_tx.clone();

    // Event routing: drain worker events on the GTK thread.
    let state_for_events = state.clone();
    let event_queue_for_events = event_queue.clone();
    glib::MainContext::default().spawn_local(async move {
        while notify_rx.recv().await.is_some() {
            for ev in event_queue_for_events.drain() {
                match ev {
                    AppEvent::Log { page, line } => {
                        match page {
                            "home" => home_page_for_events.append(&line),
                            "workflow" => workflow_for_events.append(&line),
                            "jobs" => jobs_for_events.append(&line),
                            "toolchains" => toolchains_for_events.append(&line),
                            "projects" => projects_for_events.append(&line),
                            "targets" => targets_for_events.append(&line),
                            "console" => console_for_events.append(&line),
                            "evidence" => evidence_for_events.append(&line),
                            "settings" => settings_for_events.append(&line),
                            _ => {}
                        }
                        ui_state_for_events.lock().unwrap().append_log(page, &line);
                    }
                    AppEvent::SetCurrentJob { job_id } => {
                        let mut state = state_for_events.lock().unwrap();
                        state.current_job_id = job_id;
                        let job_id = state.current_job_id.clone();
                        drop(state);
                        home_page_for_events.set_job_id(job_id.as_deref());
                        let mut cfg = cfg_for_events.lock().unwrap();
                        cfg.last_job_id = job_id.unwrap_or_default();
                        if let Err(err) = cfg.save() {
                            eprintln!("Failed to persist UI config: {err}");
                        }
                    }
                    AppEvent::HomeResetStatus => {
                        home_page_for_events.reset_status();
                    }
                    AppEvent::HomeState { state } => {
                        home_page_for_events.set_state(&state);
                    }
                    AppEvent::HomeProgress { progress } => {
                        home_page_for_events.set_progress(&progress);
                    }
                    AppEvent::HomeResult { result } => {
                        home_page_for_events.set_result(&result);
                    }
                    AppEvent::SetLastBuildApk { apk_path } => {
                        targets_for_events.set_apk_path(&apk_path);
                        workflow_for_events.set_apk_path(&apk_path);
                        let current_app_id = targets_for_events.app_id_entry.text().to_string();
                        if current_app_id.trim().is_empty() {
                            if let Some(inferred) =
                                infer_application_id_from_apk_path(apk_path.as_str())
                            {
                                targets_for_events.set_application_id(&inferred);
                                let mut state = ui_state_for_events.lock().unwrap();
                                state.targets.application_id = inferred;
                            }
                        }
                        let workflow_app_id =
                            workflow_for_events.application_id_entry.text().to_string();
                        if workflow_app_id.trim().is_empty() {
                            if let Some(inferred) =
                                infer_application_id_from_apk_path(apk_path.as_str())
                            {
                                workflow_for_events.set_application_id(&inferred);
                                let mut state = ui_state_for_events.lock().unwrap();
                                state.workflow.application_id = inferred;
                            }
                        }
                    }
                    AppEvent::SetCuttlefishBuildId { build_id } => {
                        targets_for_events.set_cuttlefish_build_id(&build_id);
                    }
                    AppEvent::TargetsCuttlefishState {
                        state,
                        adb_serial,
                        webrtc_url,
                    } => {
                        targets_for_events
                            .set_cuttlefish_state(&state, &adb_serial, webrtc_url.as_deref());
                    }
                    AppEvent::ToolchainAvailable {
                        provider_id,
                        versions,
                    } => {
                        let preferred = {
                            let state = ui_state_for_events.lock().unwrap();
                            match provider_id.as_str() {
                                pages::PROVIDER_SDK_ID => {
                                    Some(state.toolchains.sdk_version.clone())
                                }
                                pages::PROVIDER_NDK_ID => {
                                    Some(state.toolchains.ndk_version.clone())
                                }
                                _ => None,
                            }
                        };
                        toolchains_for_events.set_available_versions(
                            &provider_id,
                            &versions,
                            preferred.as_deref(),
                        );
                    }
                    AppEvent::ProjectTemplates { templates } => {
                        let preferred = {
                            ui_state_for_events
                                .lock()
                                .unwrap()
                                .projects
                                .template_id
                                .clone()
                        };
                        projects_for_events.set_templates(
                            &templates,
                            if preferred.trim().is_empty() {
                                None
                            } else {
                                Some(preferred.as_str())
                            },
                        );
                    }
                    AppEvent::ProjectToolchainSets { sets } => {
                        let preferred = {
                            ui_state_for_events
                                .lock()
                                .unwrap()
                                .projects
                                .toolchain_set_id
                                .clone()
                        };
                        projects_for_events.set_toolchain_sets(
                            &sets,
                            if preferred.trim().is_empty() {
                                None
                            } else {
                                Some(preferred.as_str())
                            },
                        );
                        let ctx = cfg_for_events.lock().unwrap().active_context();
                        apply_projects_context_if_empty(&projects_for_events, &ctx);
                    }
                    AppEvent::ProjectTargets { targets } => {
                        let preferred = {
                            ui_state_for_events
                                .lock()
                                .unwrap()
                                .projects
                                .default_target_id
                                .clone()
                        };
                        projects_for_events.set_targets(
                            &targets,
                            if preferred.trim().is_empty() {
                                None
                            } else {
                                Some(preferred.as_str())
                            },
                        );
                        let ctx = cfg_for_events.lock().unwrap().active_context();
                        apply_projects_context_if_empty(&projects_for_events, &ctx);
                    }
                    AppEvent::ProjectSelected {
                        project_id,
                        project_path,
                        opened_existing,
                    } => {
                        if opened_existing {
                            projects_for_events.set_template_none();
                            let mut state = ui_state_for_events.lock().unwrap();
                            state.projects.template_id = "none".into();
                        }
                        let ctx = {
                            let mut cfg = cfg_for_events.lock().unwrap();
                            if !project_id.trim().is_empty() {
                                cfg.active_project_id = project_id.clone();
                            }
                            if !project_path.trim().is_empty() || !project_id.trim().is_empty() {
                                cfg.active_project_path = project_path.clone();
                            }
                            let project_ref = if cfg.active_project_id.trim().is_empty() {
                                cfg.active_project_path.clone()
                            } else {
                                cfg.active_project_id.clone()
                            };
                            if !project_ref.trim().is_empty() {
                                cfg.last_job_project_id = project_ref;
                            }
                            if let Err(err) = cfg.save() {
                                eprintln!("Failed to persist UI config: {err}");
                            }
                            cfg.active_context()
                        };
                        apply_active_context(
                            &ctx,
                            &context_bar_for_events,
                            &home_page_for_events,
                            &workflow_for_events,
                            &projects_for_events,
                            &targets_for_events,
                            &toolchains_for_events,
                            &console_for_events,
                        );
                        let current_app_id = targets_for_events.app_id_entry.text().to_string();
                        let current_trimmed = current_app_id.trim();
                        if current_trimmed.is_empty()
                            || current_trimmed == LEGACY_SAMPLE_APPLICATION_ID
                        {
                            if let Some(inferred) =
                                infer_application_id_from_project(project_path.as_str())
                            {
                                targets_for_events.set_application_id(&inferred);
                                let mut state = ui_state_for_events.lock().unwrap();
                                state.targets.application_id = inferred;
                            } else if current_trimmed == LEGACY_SAMPLE_APPLICATION_ID {
                                targets_for_events.set_application_id("");
                                let mut state = ui_state_for_events.lock().unwrap();
                                state.targets.application_id.clear();
                            }
                        }
                    }
                    AppEvent::UpdateActiveContext {
                        project_id,
                        project_path,
                        toolchain_set_id,
                        target_id,
                        run_id,
                    } => {
                        let ctx = {
                            let mut cfg = cfg_for_events.lock().unwrap();
                            let mut project_updated = false;
                            if let Some(value) = project_id {
                                cfg.active_project_id = value;
                                project_updated = true;
                            }
                            if let Some(value) = project_path {
                                cfg.active_project_path = value;
                                project_updated = true;
                            }
                            if let Some(value) = toolchain_set_id {
                                cfg.active_toolchain_set_id = value.clone();
                                cfg.last_job_toolchain_set_id = value;
                            }
                            if let Some(value) = target_id {
                                cfg.active_target_id = value.clone();
                                cfg.last_job_target_id = value;
                            }
                            if let Some(value) = run_id {
                                cfg.active_run_id = value;
                            }
                            if project_updated {
                                let project_ref = if cfg.active_project_id.trim().is_empty() {
                                    cfg.active_project_path.clone()
                                } else {
                                    cfg.active_project_id.clone()
                                };
                                cfg.last_job_project_id = project_ref;
                            }
                            if let Err(err) = cfg.save() {
                                eprintln!("Failed to persist UI config: {err}");
                            }
                            cfg.active_context()
                        };
                        apply_active_context(
                            &ctx,
                            &context_bar_for_events,
                            &home_page_for_events,
                            &workflow_for_events,
                            &projects_for_events,
                            &targets_for_events,
                            &toolchains_for_events,
                            &console_for_events,
                        );
                    }
                    AppEvent::ResetAllStateComplete { ok } => {
                        let was_pending = {
                            let mut pending = pending_project_prompt_for_events.lock().unwrap();
                            let was_pending = *pending;
                            *pending = false;
                            was_pending
                        };
                        let should_clear = ok || was_pending;
                        if should_clear {
                            let ctx = {
                                let mut cfg = cfg_for_events.lock().unwrap();
                                cfg.clear_cached_state();
                                if let Err(err) = cfg.save() {
                                    eprintln!("Failed to persist UI config: {err}");
                                }
                                cfg.active_context()
                            };
                            {
                                let mut state = state_for_events.lock().unwrap();
                                state.current_job_id = None;
                            }
                            home_page_for_events.reset_status();
                            apply_active_context(
                                &ctx,
                                &context_bar_for_events,
                                &home_page_for_events,
                                &workflow_for_events,
                                &projects_for_events,
                                &targets_for_events,
                                &toolchains_for_events,
                                &console_for_events,
                            );
                            home_page_for_events.page.clear();
                            workflow_for_events.page.clear();
                            jobs_for_events.clear();
                            toolchains_for_events.page.clear();
                            projects_for_events.page.clear();
                            targets_for_events.page.clear();
                            console_for_events.page.clear();
                            evidence_for_events.clear();
                            settings_for_events.clear();
                            let default_state = UiState::default();
                            {
                                let mut state = ui_state_for_events.lock().unwrap();
                                *state = default_state.clone();
                            }
                            {
                                let mut last_saved =
                                    last_saved_ui_state_for_events.lock().unwrap();
                                *last_saved = None;
                            }
                            apply_ui_state(
                                &default_state,
                                &home_page_for_events,
                                &workflow_for_events,
                                &toolchains_for_events,
                                &projects_for_events,
                                &targets_for_events,
                                &console_for_events,
                                &jobs_for_events,
                                &evidence_for_events,
                                &settings_for_events,
                            );
                            if let Err(err) = UiState::clear_file() {
                                eprintln!("Failed to clear UI state file: {err}");
                            }
                        }
                        if !ok && was_pending {
                            let warning = "Reset did not complete; continuing to select a project folder. Check Settings for reset errors.\n";
                            projects_for_events.append(warning);
                            settings_for_events.append(warning);
                        }
                        if was_pending {
                            let projects_prompt = projects_for_events.clone();
                            let window_prompt = window_for_events.clone();
                            let cfg_prompt = cfg_for_events.clone();
                            let cmd_tx_prompt = cmd_tx_for_events.clone();
                            glib::idle_add_local(move || {
                                projects_prompt
                                    .prompt_project_path(&window_prompt, &cfg_prompt, &cmd_tx_prompt);
                                glib::ControlFlow::Break
                            });
                        }
                    }
                    AppEvent::ConfigReloaded { cfg } => {
                        let cfg = (*cfg).clone();
                        {
                            let mut cfg_guard = cfg_for_events.lock().unwrap();
                            *cfg_guard = cfg.clone();
                        }
                        let ctx = cfg.active_context();
                        apply_active_context(
                            &ctx,
                            &context_bar_for_events,
                            &home_page_for_events,
                            &workflow_for_events,
                            &projects_for_events,
                            &targets_for_events,
                            &toolchains_for_events,
                            &console_for_events,
                        );
                        let (state, loaded) = UiState::load_with_status();
                        {
                            let mut state_guard = ui_state_for_events.lock().unwrap();
                            *state_guard = state.clone();
                        }
                        {
                            let mut last_saved = last_saved_ui_state_for_events.lock().unwrap();
                            *last_saved = if loaded { Some(state.clone()) } else { None };
                        }
                        if loaded {
                            apply_ui_state(
                                &state,
                                &home_page_for_events,
                                &workflow_for_events,
                                &toolchains_for_events,
                                &projects_for_events,
                                &targets_for_events,
                                &console_for_events,
                                &jobs_for_events,
                                &evidence_for_events,
                                &settings_for_events,
                            );
                        }
                        settings_for_events.apply_config(&cfg);
                    }
                }
            }
        }
    });

    // Hook cancel button to current_job_id from GTK-side state
    {
        let cfg = cfg.clone();
        let state = state.clone();
        let cmd_tx = cmd_tx.clone();
        home.cancel_btn.connect_clicked(move |_| {
            let cfg = cfg.lock().unwrap().clone();
            // If no current job, do nothing (silent).
            if state.lock().unwrap().current_job_id.is_some() {
                cmd_tx.try_send(UiCommand::HomeCancelCurrent { cfg }).ok();
            }
        });
    }

    {
        let ui_state = ui_state.clone();
        let last_saved_ui_state = last_saved_ui_state.clone();
        let home = home.clone();
        let workflow = workflow.clone();
        let toolchains = toolchains.clone();
        let projects = projects.clone();
        let targets = targets.clone();
        let console = console.clone();
        let jobs_history = jobs_history.clone();
        let evidence = evidence.clone();
        let settings = settings.clone();
        window.connect_close_request(move |_| {
            if let Err(err) = persist_ui_state_snapshot(
                &ui_state,
                &last_saved_ui_state,
                &home,
                &workflow,
                &toolchains,
                &projects,
                &targets,
                &console,
                &jobs_history,
                &evidence,
                &settings,
            ) {
                eprintln!("Failed to persist UI state: {err}");
            }
            glib::Propagation::Proceed
        });
    }

    let background_layer = gtk::Box::new(gtk::Orientation::Vertical, 0);
    background_layer.add_css_class("apkw-solid-background");
    background_layer.set_hexpand(true);
    background_layer.set_vexpand(true);
    background_layer.set_opacity(1.0);

    let shell = gtk::Overlay::new();
    shell.add_css_class("apkw-shell");
    shell.set_hexpand(true);
    shell.set_vexpand(true);
    shell.set_opacity(1.0);
    shell.set_child(Some(&background_layer));
    shell.add_overlay(&root);
    shell.set_measure_overlay(&root, true);

    window.set_child(Some(&shell));
    window.present();
}
