//! IPC commands. Every command returns `UserError` (plain-language, redacted)
//! on failure. Keys arrive once in `connect_provider` and never leave Rust.

use crate::{AppCtx, credential_account, keys};
use maker_core::catalog;
use maker_core::detect;
use maker_core::model::*;
use maker_core::providers::anthropic::Anthropic;
use maker_core::providers::claude_code::{self, ClaudeCode};
use maker_core::providers::codex::{self, Codex};
use maker_core::providers::openai::OpenAiCompat;
use maker_core::providers::{Provider, normalize_base};
use maker_core::redact::{Secret, mask_key};
use maker_core::secrets::Persistence;
use serde::{Deserialize, Serialize};
use std::path::PathBuf;
use tauri::State;

type Res<T> = Result<T, UserError>;

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct AppState {
    providers: Vec<ProviderInfo>,
    statuses: Vec<ProviderStatus>,
    local: Vec<LocalDetection>,
    selected_provider: Option<ProviderId>,
    credential_store_available: bool,
    active_job: Option<JobView>,
    recent: Vec<ProjectSummary>,
    data_dir: String,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ConnectRequest {
    provider: ProviderId,
    #[serde(default)]
    key: String,
    #[serde(default)]
    session_only: bool,
    endpoint: Option<String>,
    allow_loopback: Option<bool>,
    /// Preferred model; the provider's best default is used when absent.
    model: Option<String>,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ConnectResult {
    status: ProviderStatus,
    store_unavailable: bool,
}

fn status(ctx: &AppCtx, id: ProviderId) -> ProviderStatus {
    let s = |k: String| ctx.store.setting(&k);
    let verified_at: Option<i64> = s(keys::verified(id.as_str())).and_then(|v| v.parse().ok());
    let cred = ctx.creds.get(&credential_account(&ctx.store, id));
    let endpoint = (id == ProviderId::Custom)
        .then(|| ctx.store.setting(keys::CUSTOM_ENDPOINT))
        .flatten();
    let info = catalog::info(id);
    let credential = match &cred {
        Some((_, Persistence::Stored)) => CredentialState::Stored,
        Some((_, Persistence::Session)) => CredentialState::Session,
        None if !info.key_required => CredentialState::NotRequired,
        None => CredentialState::Missing,
    };
    let connected = verified_at.is_some()
        && match id {
            ProviderId::Anthropic | ProviderId::OpenAi => cred.is_some(),
            ProviderId::Custom => endpoint.is_some(),
            ProviderId::ClaudeCode | ProviderId::Codex | ProviderId::Ollama => true,
        };
    ProviderStatus {
        id,
        connected,
        credential,
        masked_key: cred.as_ref().map(|(k, _)| mask_key(k.expose())),
        account_label: connected.then(|| s(keys::account(id.as_str()))).flatten(),
        endpoint,
        model: s(keys::model(id.as_str())),
        models: s(keys::models(id.as_str()))
            .and_then(|j| serde_json::from_str(&j).ok())
            .unwrap_or_default(),
        verified_at: verified_at.filter(|_| connected),
    }
}

#[tauri::command]
pub async fn get_app_state(ctx: State<'_, AppCtx>) -> Res<AppState> {
    let selected = ctx
        .store
        .setting(keys::SELECTED)
        .and_then(|s| ProviderId::parse(&s));
    Ok(AppState {
        providers: catalog::providers(),
        statuses: ProviderId::ALL
            .into_iter()
            .map(|id| status(&ctx, id))
            .collect(),
        local: detect::all(&ctx.endpoints).await,
        selected_provider: selected,
        credential_store_available: ctx.store_available,
        active_job: ctx.engine.active_job(),
        recent: ctx.engine.summaries(),
        data_dir: ctx.engine.library().root().display().to_string(),
    })
}

/// Re-runs local detection (Claude Code, Codex, Ollama).
#[tauri::command]
pub async fn detect_providers(ctx: State<'_, AppCtx>) -> Res<Vec<LocalDetection>> {
    Ok(detect::all(&ctx.endpoints).await)
}

fn empty_key() -> UserError {
    UserError::new(
        ErrorKind::InvalidKey,
        "Paste your key",
        "The key field is empty.",
        NextAction::None,
    )
}

#[tauri::command]
pub async fn connect_provider(ctx: State<'_, AppCtx>, req: ConnectRequest) -> Res<ConnectResult> {
    let id = req.provider;
    let key = Secret::new(req.key.trim().to_string());
    drop(req.key);
    let to_user = |e: maker_core::errors::ProviderError| e.to_user(id);
    let ep = &ctx.endpoints;

    // 1. Verify with a free call before saving anything.
    let provider: Box<dyn Provider> = match id {
        ProviderId::Anthropic => {
            if key.is_empty() {
                return Err(empty_key());
            }
            Box::new(Anthropic::new(key.clone(), None, ep).map_err(to_user)?)
        }
        ProviderId::OpenAi => {
            if key.is_empty() {
                return Err(empty_key());
            }
            Box::new(OpenAiCompat::openai(key.clone(), None, ep).map_err(to_user)?)
        }
        ProviderId::Custom => {
            let loopback = req.allow_loopback.unwrap_or(false);
            let base =
                normalize_base(req.endpoint.as_deref().unwrap_or(""), loopback).map_err(to_user)?;
            let token = (!key.is_empty()).then(|| key.clone());
            let p = OpenAiCompat::custom(&base, token, None, loopback, ep).map_err(to_user)?;
            // Saved only after the check below succeeds.
            let v = p.verify().await.map_err(to_user)?;
            let _ = ctx.store.set_setting(keys::CUSTOM_ENDPOINT, Some(&base));
            let _ = ctx.store.set_setting(
                keys::CUSTOM_LOOPBACK,
                Some(if loopback { "1" } else { "0" }),
            );
            return finish_connect(&ctx, id, &key, req.session_only, req.model, v);
        }
        ProviderId::Ollama => Box::new(OpenAiCompat::ollama(None, ep).map_err(to_user)?),
        ProviderId::ClaudeCode => {
            let bin = claude_code::find(ep.claude_cli.as_deref()).ok_or_else(|| {
                maker_core::errors::ProviderError::Unavailable(
                    "Claude Code wasn't found on this computer. Install it and sign in once with `claude`.".into(),
                )
                .to_user(id)
            })?;
            Box::new(ClaudeCode::new(bin, None, ep.request_timeout))
        }
        ProviderId::Codex => {
            let bin = codex::find(ep.codex_cli.as_deref()).ok_or_else(|| {
                maker_core::errors::ProviderError::Unavailable(
                    "Codex wasn't found on this computer. Install it and sign in once with `codex login`.".into(),
                )
                .to_user(id)
            })?;
            Box::new(Codex::new(bin, None, ep.request_timeout))
        }
    };
    let v = provider.verify().await.map_err(to_user)?;
    finish_connect(&ctx, id, &key, req.session_only, req.model, v)
}

fn finish_connect(
    ctx: &AppCtx,
    id: ProviderId,
    key: &Secret,
    session_only: bool,
    wanted_model: Option<String>,
    v: maker_core::providers::Verified,
) -> Res<ConnectResult> {
    // 2. Save the key (OS store, or session-only when asked / unavailable).
    let account = credential_account(&ctx.store, id);
    let mut store_unavailable = false;
    if !key.is_empty() {
        if ctx.creds.save(&account, key, session_only).is_err() {
            store_unavailable = true;
        }
    } else {
        ctx.creds.remove(&account);
    }
    // 3. Remember the model list and choice; keep a still-valid earlier choice.
    let previous = ctx.store.setting(&keys::model(id.as_str()));
    let model = wanted_model
        .filter(|m| !m.trim().is_empty())
        .or_else(|| previous.filter(|m| v.models.contains(m)))
        .or_else(|| v.default_model().map(String::from));
    let set = |k: String, val: Option<&str>| {
        let _ = ctx.store.set_setting(&k, val);
    };
    set(keys::model(id.as_str()), model.as_deref());
    set(
        keys::models(id.as_str()),
        serde_json::to_string(&v.models).ok().as_deref(),
    );
    set(keys::account(id.as_str()), v.account_label.as_deref());
    set(keys::verified(id.as_str()), Some(&now_unix().to_string()));
    let _ = ctx.store.set_setting(keys::SELECTED, Some(id.as_str()));
    Ok(ConnectResult {
        status: status(ctx, id),
        store_unavailable,
    })
}

#[tauri::command]
pub async fn disconnect_provider(
    ctx: State<'_, AppCtx>,
    provider: ProviderId,
) -> Res<ProviderStatus> {
    ctx.creds.remove(&credential_account(&ctx.store, provider));
    for k in [
        keys::account(provider.as_str()),
        keys::verified(provider.as_str()),
        keys::models(provider.as_str()),
    ] {
        let _ = ctx.store.set_setting(&k, None);
    }
    if provider == ProviderId::Custom {
        let _ = ctx.store.set_setting(keys::CUSTOM_ENDPOINT, None);
    }
    if ctx.store.setting(keys::SELECTED).as_deref() == Some(provider.as_str()) {
        let _ = ctx.store.set_setting(keys::SELECTED, None);
    }
    Ok(status(&ctx, provider))
}

#[tauri::command]
pub async fn select_provider(ctx: State<'_, AppCtx>, provider: ProviderId) -> Res<()> {
    ctx.store
        .set_setting(keys::SELECTED, Some(provider.as_str()))
        .map_err(|e| UserError::internal(e.to_string()))
}

#[tauri::command]
pub async fn set_model(
    ctx: State<'_, AppCtx>,
    provider: ProviderId,
    model: String,
) -> Res<ProviderStatus> {
    let m = model.trim();
    if m.is_empty() || m.len() > 200 {
        return Err(UserError::internal("Choose a model name."));
    }
    ctx.store
        .set_setting(&keys::model(provider.as_str()), Some(m))
        .map_err(|e| UserError::internal(e.to_string()))?;
    Ok(status(&ctx, provider))
}

#[tauri::command]
pub async fn start_generation(ctx: State<'_, AppCtx>, req: GenerateRequest) -> Res<String> {
    ctx.engine.start_generation(req)
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AdjustRequest {
    project_id: String,
    instruction: String,
    provider: ProviderId,
    client_request_id: String,
}

/// Starts a prompt-driven adjustment; the result is a new object.
#[tauri::command]
pub async fn adjust_object(ctx: State<'_, AppCtx>, req: AdjustRequest) -> Res<String> {
    ctx.engine.adjust(
        &req.project_id,
        &req.instruction,
        req.provider,
        &req.client_request_id,
    )
}

#[tauri::command]
pub async fn cancel_generation(ctx: State<'_, AppCtx>, project_id: String) -> Res<CancelReport> {
    Ok(ctx.engine.cancel(&project_id).await)
}

#[tauri::command]
pub async fn retry_project(
    ctx: State<'_, AppCtx>,
    project_id: String,
    action: RetryAction,
) -> Res<()> {
    ctx.engine.retry(&project_id, action)
}

#[tauri::command]
pub async fn list_projects(ctx: State<'_, AppCtx>) -> Res<Vec<ProjectSummary>> {
    Ok(ctx.engine.summaries())
}

#[tauri::command]
pub async fn get_project(ctx: State<'_, AppCtx>, project_id: String) -> Res<ProjectDetail> {
    ctx.engine.detail(&project_id)
}

#[tauri::command]
pub async fn delete_project(ctx: State<'_, AppCtx>, project_id: String) -> Res<()> {
    ctx.engine.delete(&project_id)
}

fn slug(prompt: &str) -> String {
    let s: String = prompt
        .chars()
        .map(|c| {
            if c.is_alphanumeric() {
                c.to_ascii_lowercase()
            } else {
                '-'
            }
        })
        .collect::<String>()
        .split('-')
        .filter(|w| !w.is_empty())
        .take(6)
        .collect::<Vec<_>>()
        .join("-");
    if s.is_empty() { "splat".into() } else { s }
}

#[tauri::command]
pub async fn export_splat(
    app: tauri::AppHandle,
    ctx: State<'_, AppCtx>,
    project_id: String,
) -> Res<ExportResult> {
    let detail = ctx.engine.detail(&project_id)?;
    let name = format!("{}.ply", slug(&detail.summary.prompt));
    let dest: Option<PathBuf> = pick_save(&app, name).await;
    let Some(dest) = dest else {
        return Ok(ExportResult {
            saved: false,
            path: None,
            bytes: None,
            sha256: None,
        });
    };
    let engine = ctx.engine.clone();
    tauri::async_runtime::spawn_blocking(move || engine.export(&project_id, &dest))
        .await
        .map_err(|e| UserError::internal(e.to_string()))?
}

async fn pick_save(app: &tauri::AppHandle, name: String) -> Option<PathBuf> {
    #[cfg(feature = "e2e")]
    if let Some(p) = crate::e2e::export_path() {
        return Some(p);
    }
    use tauri_plugin_dialog::DialogExt;
    let app = app.clone();
    tauri::async_runtime::spawn_blocking(move || {
        app.dialog()
            .file()
            .set_title("Export splat")
            .set_file_name(&name)
            .add_filter("Gaussian splat (.ply)", &["ply"])
            .blocking_save_file()
            .and_then(|f| f.into_path().ok())
    })
    .await
    .ok()
    .flatten()
}

async fn pick_open(app: &tauri::AppHandle) -> Option<PathBuf> {
    #[cfg(feature = "e2e")]
    if let Some(p) = crate::e2e::import_path() {
        return Some(p);
    }
    use tauri_plugin_dialog::DialogExt;
    let app = app.clone();
    tauri::async_runtime::spawn_blocking(move || {
        app.dialog()
            .file()
            .set_title("Open a 3D object")
            .add_filter("3D objects (.ply, .obj, .stl)", &["ply", "obj", "stl"])
            .add_filter("Gaussian splat (.ply)", &["ply"])
            .add_filter("Mesh (.obj, .stl)", &["obj", "stl"])
            .blocking_pick_file()
            .and_then(|f| f.into_path().ok())
    })
    .await
    .ok()
    .flatten()
}

#[tauri::command]
pub async fn import_splat(app: tauri::AppHandle, ctx: State<'_, AppCtx>) -> Res<Option<String>> {
    match pick_open(&app).await {
        Some(p) => ctx.engine.import(p).await.map(Some),
        None => Ok(None),
    }
}

/// Raw PNG body (not JSON); project id in the `x-project-id` header.
#[tauri::command]
pub async fn save_thumbnail(ctx: State<'_, AppCtx>, request: tauri::ipc::Request<'_>) -> Res<()> {
    let id = request
        .headers()
        .get("x-project-id")
        .and_then(|v| v.to_str().ok())
        .map(String::from)
        .ok_or_else(|| UserError::internal("missing project id"))?;
    let tauri::ipc::InvokeBody::Raw(bytes) = request.body() else {
        return Err(UserError::internal("thumbnail must be sent as raw bytes"));
    };
    ctx.engine.save_thumbnail(&id, bytes)
}

#[tauri::command]
pub async fn report_viewer_issue(detail: String) -> Res<()> {
    tracing::warn!(
        "viewer issue: {}",
        maker_core::redact::redact(&detail.chars().take(500).collect::<String>())
    );
    Ok(())
}
