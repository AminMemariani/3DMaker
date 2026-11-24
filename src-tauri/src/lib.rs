//! 3D Maker desktop shell: wires the Tauri window to `maker-core`.

mod commands;
#[cfg(feature = "e2e")]
mod e2e;

use maker_core::engine::{Engine, EngineConfig, EventSink, ProviderFactory};
use maker_core::errors::ProviderError;
use maker_core::library::Library;
use maker_core::model::*;
use maker_core::providers::anthropic::Anthropic;
use maker_core::providers::claude_code::{self, ClaudeCode};
use maker_core::providers::codex::{self, Codex};
use maker_core::providers::openai::OpenAiCompat;
use maker_core::providers::{Endpoints, Provider};
use maker_core::secrets::{CredentialStore, Credentials, KeyringStore, SERVICE, custom_account};
use maker_core::store::Store;
use std::sync::Arc;
use tauri::{Emitter, Manager};

pub struct AppCtx {
    pub engine: Arc<Engine>,
    pub creds: Arc<Credentials>,
    pub store: Arc<Store>,
    pub endpoints: Endpoints,
    pub store_available: bool,
}

struct TauriEvents(tauri::AppHandle);

impl EventSink for TauriEvents {
    fn job(&self, view: &JobView) {
        let _ = self.0.emit("job-update", view);
    }
    fn projects(&self, list: &[ProjectSummary]) {
        let _ = self.0.emit("projects-update", list);
    }
}

/// Settings keys (non-secret).
pub mod keys {
    pub const SELECTED: &str = "selected_provider";
    pub const CUSTOM_ENDPOINT: &str = "custom.endpoint";
    pub const CUSTOM_LOOPBACK: &str = "custom.allow_loopback";
    pub fn model(id: &str) -> String {
        format!("model.{id}")
    }
    pub fn models(id: &str) -> String {
        format!("status.{id}.models")
    }
    pub fn account(id: &str) -> String {
        format!("status.{id}.account")
    }
    pub fn verified(id: &str) -> String {
        format!("status.{id}.verified_at")
    }
}

/// Credential-store account for a provider's key. Custom keys are scoped to
/// the server's origin, so changing the URL never reuses an old key.
pub fn credential_account(store: &Store, id: ProviderId) -> String {
    match id {
        ProviderId::Custom => {
            let origin = store
                .setting(keys::CUSTOM_ENDPOINT)
                .and_then(|e| url::Url::parse(&e).ok())
                .map(|u| u.origin().ascii_serialization())
                .unwrap_or_default();
            custom_account(&origin)
        }
        other => other.as_str().to_string(),
    }
}

fn not_connected(id: ProviderId) -> UserError {
    UserError::new(
        ErrorKind::InvalidKey,
        format!("Connect {} first", id.display_name()),
        format!(
            "Set up {} in Settings to generate objects. Saved objects stay available without it.",
            id.display_name()
        ),
        NextAction::ReplaceKey,
    )
}

/// Builds the provider for a request from saved settings and credentials.
pub fn build_provider(
    id: ProviderId,
    creds: &Credentials,
    store: &Store,
    endpoints: &Endpoints,
) -> Result<Arc<dyn Provider>, UserError> {
    let to_user = |e: ProviderError| e.to_user(id);
    let secret = creds.get(&credential_account(store, id)).map(|(s, _)| s);
    let model = store.setting(&keys::model(id.as_str()));
    Ok(match id {
        ProviderId::Anthropic => Arc::new(
            Anthropic::new(secret.ok_or_else(|| not_connected(id))?, model, endpoints)
                .map_err(to_user)?,
        ),
        ProviderId::OpenAi => Arc::new(
            OpenAiCompat::openai(secret.ok_or_else(|| not_connected(id))?, model, endpoints)
                .map_err(to_user)?,
        ),
        ProviderId::Custom => {
            let base = store
                .setting(keys::CUSTOM_ENDPOINT)
                .ok_or_else(|| not_connected(id))?;
            let loopback = store.setting(keys::CUSTOM_LOOPBACK).as_deref() == Some("1");
            Arc::new(
                OpenAiCompat::custom(&base, secret, model, loopback, endpoints).map_err(to_user)?,
            )
        }
        ProviderId::Ollama => Arc::new(OpenAiCompat::ollama(model, endpoints).map_err(to_user)?),
        ProviderId::ClaudeCode => {
            let bin = claude_code::find(endpoints.claude_cli.as_deref()).ok_or_else(|| {
                ProviderError::Unavailable(
                    "Claude Code wasn't found on this computer. Install it and sign in once with `claude`.".into(),
                )
                .to_user(id)
            })?;
            Arc::new(ClaudeCode::new(bin, model, endpoints.request_timeout))
        }
        ProviderId::Codex => {
            let bin = codex::find(endpoints.codex_cli.as_deref()).ok_or_else(|| {
                ProviderError::Unavailable(
                    "Codex wasn't found on this computer. Install it and sign in once with `codex login`.".into(),
                )
                .to_user(id)
            })?;
            Arc::new(Codex::new(bin, model, endpoints.request_timeout))
        }
    })
}

fn factory(creds: Arc<Credentials>, store: Arc<Store>, endpoints: Endpoints) -> ProviderFactory {
    Arc::new(move |id| build_provider(id, &creds, &store, &endpoints))
}

fn install_panic_redaction() {
    std::panic::set_hook(Box::new(move |info| {
        let msg = info
            .payload()
            .downcast_ref::<&str>()
            .map(|s| s.to_string())
            .or_else(|| info.payload().downcast_ref::<String>().cloned())
            .unwrap_or_default();
        eprintln!(
            "3D Maker crashed: {} at {:?}",
            maker_core::redact::redact(&msg),
            info.location()
        );
    }));
}

fn data_dir(app: &tauri::App) -> std::path::PathBuf {
    #[cfg(feature = "e2e")]
    if let Some(d) = e2e::data_dir() {
        return d;
    }
    app.path().app_data_dir().expect("no app data directory")
}

fn credential_backend(_root: &std::path::Path) -> Box<dyn CredentialStore> {
    #[cfg(feature = "e2e")]
    if let Some(s) = e2e::credential_store(_root) {
        return s;
    }
    Box::new(KeyringStore::new(SERVICE))
}

fn endpoints() -> Endpoints {
    #[cfg(feature = "e2e")]
    if let Some(e) = e2e::endpoints() {
        return e;
    }
    Endpoints::default()
}

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    install_panic_redaction();
    let _ = tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_env("THREEDMAKER_LOG")
                .unwrap_or_else(|_| "warn".into()),
        )
        .with_writer(std::io::stderr)
        .try_init();

    let builder = tauri::Builder::default()
        .plugin(tauri_plugin_dialog::init())
        .setup(|app| {
            let root = data_dir(app);
            let lib = Library::new(&root)?;
            // Only the object library is reachable through the asset protocol.
            app.asset_protocol_scope()
                .allow_directory(lib.objects_dir(), true)?;
            let store = Arc::new(Store::open(&lib.db_path())?);
            let creds = Arc::new(Credentials::new(credential_backend(&root)));
            let store_available = creds.persistent_available();
            let endpoints = endpoints();
            let engine = Engine::new(
                store.clone(),
                lib,
                factory(creds.clone(), store.clone(), endpoints.clone()),
                Arc::new(TauriEvents(app.handle().clone())),
                EngineConfig::default(),
            );
            let recover = engine.clone();
            tauri::async_runtime::spawn(async move {
                recover.recover();
            });
            app.manage(AppCtx {
                engine,
                creds,
                store,
                endpoints,
                store_available,
            });
            #[cfg(feature = "e2e")]
            {
                e2e::setup_window(app);
                e2e::start_watchdog(app.handle().clone());
            }
            Ok(())
        });

    #[cfg(not(feature = "e2e"))]
    let builder = builder.invoke_handler(tauri::generate_handler![
        commands::get_app_state,
        commands::detect_providers,
        commands::connect_provider,
        commands::disconnect_provider,
        commands::select_provider,
        commands::set_model,
        commands::start_generation,
        commands::adjust_object,
        commands::cancel_generation,
        commands::retry_project,
        commands::list_projects,
        commands::get_project,
        commands::delete_project,
        commands::export_splat,
        commands::import_splat,
        commands::save_thumbnail,
        commands::report_viewer_issue,
    ]);
    #[cfg(feature = "e2e")]
    let builder = builder.invoke_handler(tauri::generate_handler![
        commands::get_app_state,
        commands::detect_providers,
        commands::connect_provider,
        commands::disconnect_provider,
        commands::select_provider,
        commands::set_model,
        commands::start_generation,
        commands::adjust_object,
        commands::cancel_generation,
        commands::retry_project,
        commands::list_projects,
        commands::get_project,
        commands::delete_project,
        commands::export_splat,
        commands::import_splat,
        commands::save_thumbnail,
        commands::report_viewer_issue,
        e2e::e2e_config,
        e2e::e2e_report,
    ]);

    builder
        .run(tauri::generate_context!())
        .expect("error while running 3D Maker");
}
