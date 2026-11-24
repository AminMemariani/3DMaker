//! Test-driver hooks, compiled only with `--features e2e` (never in release
//! bundles). Driven by environment variables set by tests/e2e-native/run.mjs.

use maker_core::providers::Endpoints;
use maker_core::redact::Secret;
use maker_core::secrets::{CredentialStore, StoreError};
use serde_json::{Value, json};
use std::path::{Path, PathBuf};
use std::sync::Mutex;
use tauri::Manager;

fn env(k: &str) -> Option<String> {
    std::env::var(k).ok().filter(|v| !v.is_empty())
}

pub fn data_dir() -> Option<PathBuf> {
    env("MAKER_DATA_DIR").map(PathBuf::from)
}

pub fn endpoints() -> Option<Endpoints> {
    env("MAKER_TEST_PROVIDER_BASE").map(|b| Endpoints {
        claude_cli: env("MAKER_TEST_CLAUDE_CLI").map(PathBuf::from),
        codex_cli: env("MAKER_TEST_CODEX_CLI").map(PathBuf::from),
        ..Endpoints::local_fake(&b)
    })
}

pub fn export_path() -> Option<PathBuf> {
    env("MAKER_E2E_EXPORT_PATH").map(PathBuf::from)
}

pub fn import_path() -> Option<PathBuf> {
    env("MAKER_E2E_PERF_PLY").map(PathBuf::from)
}

/// File-backed credential store inside the throwaway profile, so the test
/// never touches the real OS keychain but credentials survive a relaunch.
struct FileStore {
    path: PathBuf,
    lock: Mutex<()>,
}

impl FileStore {
    fn read(&self) -> serde_json::Map<String, Value> {
        std::fs::read(&self.path)
            .ok()
            .and_then(|b| serde_json::from_slice(&b).ok())
            .unwrap_or_default()
    }
}

impl CredentialStore for FileStore {
    fn get(&self, account: &str) -> Result<Option<Secret>, StoreError> {
        let _g = self.lock.lock().unwrap();
        Ok(self
            .read()
            .get(account)
            .and_then(|v| v.as_str())
            .map(Secret::new))
    }
    fn set(&self, account: &str, secret: &Secret) -> Result<(), StoreError> {
        let _g = self.lock.lock().unwrap();
        let mut m = self.read();
        m.insert(account.into(), json!(secret.expose()));
        std::fs::write(&self.path, serde_json::to_vec(&m).unwrap())
            .map_err(|e| StoreError::Unavailable(e.to_string()))
    }
    fn delete(&self, account: &str) -> Result<(), StoreError> {
        let _g = self.lock.lock().unwrap();
        let mut m = self.read();
        m.remove(account);
        std::fs::write(&self.path, serde_json::to_vec(&m).unwrap())
            .map_err(|e| StoreError::Unavailable(e.to_string()))
    }
}

pub fn credential_store(root: &Path) -> Option<Box<dyn CredentialStore>> {
    data_dir()?;
    Some(Box::new(FileStore {
        path: root.join("e2e-credentials.json"),
        lock: Mutex::new(()),
    }))
}

pub fn setup_window(app: &tauri::App) {
    // A window launched from a background test process may start occluded or
    // unfocused, and WebKit then pauses requestAnimationFrame entirely
    // (document.visibilityState = "hidden"). Keep it visible and on top.
    #[cfg(target_os = "macos")]
    {
        let _ = app
            .handle()
            .set_activation_policy(tauri::ActivationPolicy::Regular);
    }
    if let Some(w) = app.get_webview_window("main") {
        let _ = w.show();
        let _ = w.unminimize();
        let _ = w.set_always_on_top(true);
        let _ = w.center();
        let _ = w.set_focus();
        let w2 = w.clone();
        tauri::async_runtime::spawn(async move {
            for _ in 0..10 {
                tokio::time::sleep(std::time::Duration::from_millis(500)).await;
                let _ = w2.set_focus();
            }
        });
    }
    if let (Some(size), Some(w)) = (env("MAKER_E2E_WINDOW"), app.get_webview_window("main")) {
        if let Some((a, b)) = size.split_once('x') {
            if let (Ok(width), Ok(height)) = (a.parse::<f64>(), b.parse::<f64>()) {
                let _ = w.set_size(tauri::LogicalSize::new(width, height));
            }
        }
    }
}

/// Writes a failure report and exits if the driver never reports (so a stuck
/// run is diagnosable instead of silently timing out).
pub fn start_watchdog(app: tauri::AppHandle) {
    let secs: u64 = env("MAKER_E2E_WATCHDOG_SECS")
        .and_then(|s| s.parse().ok())
        .unwrap_or(190);
    std::thread::spawn(move || {
        std::thread::sleep(std::time::Duration::from_secs(secs));
        eprintln!(
            "e2e: watchdog fired after {secs}s (driver started: {})",
            DRIVER_STARTED.load(std::sync::atomic::Ordering::SeqCst)
        );
        if let Some(path) = env("MAKER_E2E_REPORT") {
            // Keep the driver's last checkpoint, marked as failed by the watchdog.
            let mut r: Value = std::fs::read(&path)
                .ok()
                .and_then(|b| serde_json::from_slice(&b).ok())
                .unwrap_or_else(|| json!({"steps": []}));
            r["ok"] = json!(false);
            r["error"] = json!(format!(
                "watchdog: no final driver report after {secs}s (last step: {})",
                r["current"].as_str().unwrap_or("none")
            ));
            r["driverStarted"] = json!(DRIVER_STARTED.load(std::sync::atomic::Ordering::SeqCst));
            let _ = std::fs::write(path, serde_json::to_vec_pretty(&r).unwrap());
        }
        app.exit(3);
    });
}

static DRIVER_STARTED: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);

#[tauri::command]
pub async fn e2e_config() -> Result<Value, String> {
    DRIVER_STARTED.store(true, std::sync::atomic::Ordering::SeqCst);
    eprintln!(
        "e2e: driver started (phase {})",
        env("MAKER_E2E_PHASE").unwrap_or_default()
    );
    Ok(json!({
        "phase": env("MAKER_E2E_PHASE").unwrap_or_else(|| "first".into()),
        "testKey": fake_key("test-key-ok"),
        "wrongKey": "sk-ant-this-key-is-wrong-000000",
        "teapotPrompt": "A cobalt-blue ceramic teapot with one curved spout, a round brass lid, and a large loop handle.",
        "secondPrompt": "a yellow toy forklift with black wheels",
        "perfPlyPath": import_path().map(|p| p.display().to_string()),
    }))
}

fn fake_key(k: &str) -> String {
    k.to_string()
}

#[tauri::command]
pub async fn e2e_report(app: tauri::AppHandle, report: Value, quit: bool) -> Result<(), String> {
    if let Some(path) = env("MAKER_E2E_REPORT") {
        std::fs::write(&path, serde_json::to_vec_pretty(&report).unwrap())
            .map_err(|e| e.to_string())?;
    }
    if quit {
        app.exit(0);
    }
    Ok(())
}
