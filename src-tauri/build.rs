// Declares the app's IPC commands so each one needs an explicit permission
// in a capability file (nothing is callable by default).
const COMMANDS: &[&str] = &[
    "get_app_state",
    "detect_providers",
    "set_model",
    "connect_provider",
    "disconnect_provider",
    "select_provider",
    "start_generation",
    "adjust_object",
    "cancel_generation",
    "retry_project",
    "list_projects",
    "get_project",
    "delete_project",
    "export_splat",
    "import_splat",
    "save_thumbnail",
    "report_viewer_issue",
    // Test-driver commands: handlers are compiled only with `--features e2e`,
    // and only the e2e config enables the capability that grants them.
    "e2e_config",
    "e2e_report",
];

fn main() {
    tauri_build::try_build(
        tauri_build::Attributes::new()
            .app_manifest(tauri_build::AppManifest::new().commands(COMMANDS)),
    )
    .expect("failed to run tauri-build");
}
