//! Local stand-in for the Anthropic Messages API, the OpenAI Chat
//! Completions API and Ollama, answering with canned shape designs.
//! Test tooling only; never bundled.
//!
//! Faults are configured through [`FaultConfig`] (in-process) or
//! `POST /__admin/config` (from another process). `GET /__admin/stats`
//! returns request counters.

use axum::extract::{Path as AxPath, State};
use axum::http::{HeaderMap, StatusCode, header};
use axum::response::{IntoResponse, Response};
use axum::routing::{get, post};
use axum::{Json, Router};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use std::collections::HashMap;
use std::sync::{Arc, Mutex};

pub const VALID_KEY: &str = "test-key-ok";
pub const NOCREDIT_KEY: &str = "test-key-nocredit";

pub const ANTHROPIC_MODELS: [&str; 2] = ["claude-test-opus", "claude-test-sonnet"];
pub const OPENAI_MODELS: [&str; 2] = ["gpt-test", "gpt-test-mini"];

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct FaultConfig {
    /// Answer with prose instead of JSON, once, for prompts containing this text.
    pub bad_design_once_for: Option<String>,
    /// Answer HTTP 500, once, for prompts containing this text.
    pub server_error_once_for: Option<String>,
    /// Answer the next N design requests with HTTP 429.
    pub rate_limit_next: u32,
    /// Wait this long before answering a design request.
    pub delay_ms: u64,
    /// Ollama's installed models; empty means "running with no models".
    pub ollama_models: Vec<String>,
    /// When false, every Ollama route answers 503 as if nothing listened.
    pub ollama_running: bool,
}

impl Default for FaultConfig {
    fn default() -> Self {
        Self {
            bad_design_once_for: None,
            server_error_once_for: None,
            rate_limit_next: 0,
            delay_ms: 0,
            ollama_models: vec!["llama-test:latest".into(), "nomic-embed-text:latest".into()],
            ollama_running: true,
        }
    }
}

#[derive(Default)]
struct Inner {
    cfg: FaultConfig,
    counters: HashMap<String, u64>,
    requests: Vec<Value>,
    bad_used: bool,
    error_used: bool,
}

#[derive(Clone, Default)]
pub struct Fake {
    inner: Arc<Mutex<Inner>>,
}

pub struct Running {
    pub base: String,
    pub fake: Fake,
    handle: tokio::task::JoinHandle<()>,
}

impl Running {
    pub fn stop(self) {
        self.handle.abort();
    }
    pub fn count(&self, key: &str) -> u64 {
        self.fake.count(key)
    }
}

impl Fake {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn configure(&self, f: impl FnOnce(&mut FaultConfig)) {
        f(&mut self.inner.lock().unwrap().cfg);
    }

    pub fn count(&self, key: &str) -> u64 {
        *self.inner.lock().unwrap().counters.get(key).unwrap_or(&0)
    }

    /// Recorded design-request bodies whose path contains `path_contains`.
    pub fn requests(&self, path_contains: &str) -> Vec<Value> {
        self.inner
            .lock()
            .unwrap()
            .requests
            .iter()
            .filter(|r| {
                r["path"]
                    .as_str()
                    .is_some_and(|p| p.contains(path_contains))
            })
            .cloned()
            .collect()
    }

    pub async fn serve(self, port: u16) -> std::io::Result<Running> {
        let listener = tokio::net::TcpListener::bind(("127.0.0.1", port)).await?;
        let base = format!("http://{}", listener.local_addr()?);
        let app = router(self.clone());
        let handle = tokio::spawn(async move {
            let _ = axum::serve(listener, app).await;
        });
        Ok(Running {
            base,
            fake: self,
            handle,
        })
    }

    fn bump(&self, key: &str) {
        *self
            .inner
            .lock()
            .unwrap()
            .counters
            .entry(key.to_string())
            .or_default() += 1;
    }

    fn cfg(&self) -> FaultConfig {
        self.inner.lock().unwrap().cfg.clone()
    }

    /// Applies scripted faults for one design request.
    fn fault_for(&self, prompt: &str) -> Option<Fault> {
        let mut g = self.inner.lock().unwrap();
        if g.cfg.rate_limit_next > 0 {
            g.cfg.rate_limit_next -= 1;
            return Some(Fault::RateLimit);
        }
        let hit = |t: &Option<String>| {
            t.as_ref()
                .is_some_and(|t| prompt.to_lowercase().contains(&t.to_lowercase()))
        };
        if !g.error_used && hit(&g.cfg.server_error_once_for) {
            g.error_used = true;
            return Some(Fault::ServerError);
        }
        if !g.bad_used && hit(&g.cfg.bad_design_once_for) {
            g.bad_used = true;
            return Some(Fault::BadDesign);
        }
        None
    }
}

enum Fault {
    RateLimit,
    ServerError,
    BadDesign,
}

fn err(status: StatusCode, body: Value) -> Response {
    (status, Json(body)).into_response()
}

/// A canned design for the prompt. Teapot and forklift match the
/// acceptance prompts; anything else gets a simple two-part object.
pub fn design_for(prompt: &str) -> Value {
    let p = prompt.to_lowercase();
    if p.contains("teapot") {
        json!({"name": "teapot", "parts": [
            {"label": "body", "shape": "ellipsoid", "center": [0, 0.42, 0], "size": [0.9, 0.72, 0.9], "color": "#1f4fd1"},
            {"label": "foot", "shape": "cylinder", "center": [0, 0.05, 0], "size": [0.5, 0.1, 0.5], "color": "#1f4fd1"},
            {"label": "lid", "shape": "cylinder", "center": [0, 0.8, 0], "size": [0.38, 0.06, 0.38], "color": "#b5892e"},
            {"label": "knob", "shape": "ellipsoid", "center": [0, 0.87, 0], "size": [0.1, 0.08, 0.1], "color": "#b5892e"},
            {"label": "spout", "shape": "cone", "center": [0.55, 0.5, 0], "size": [0.14, 0.5, 0.14], "rotation": [0, 0, -55], "color": "#1f4fd1"},
            {"label": "handle", "shape": "torus", "center": [-0.5, 0.45, 0], "size": [0.42, 0.06, 0.42], "rotation": [90, 0, 0], "color": "#1f4fd1"}
        ]})
    } else if p.contains("forklift") {
        let wheel = |x: f32, z: f32| json!({"label": "wheel", "shape": "cylinder", "center": [x, 0.15, z], "size": [0.3, 0.12, 0.3], "rotation": [90, 0, 0], "color": "#111111"});
        json!({"name": "forklift", "parts": [
            {"label": "chassis", "shape": "box", "center": [0, 0.35, 0], "size": [1.0, 0.35, 0.6], "color": "#f2c200"},
            {"label": "seat", "shape": "box", "center": [-0.15, 0.6, 0], "size": [0.3, 0.15, 0.35], "color": "#222222"},
            {"label": "roof", "shape": "box", "center": [-0.1, 1.1, 0], "size": [0.6, 0.05, 0.55], "color": "#f2c200"},
            {"label": "mast", "shape": "box", "center": [0.52, 0.75, 0], "size": [0.06, 1.2, 0.45], "color": "#333333"},
            {"label": "fork", "shape": "box", "center": [0.75, 0.08, 0.15], "size": [0.45, 0.04, 0.08], "color": "#333333"},
            {"label": "fork", "shape": "box", "center": [0.75, 0.08, -0.15], "size": [0.45, 0.04, 0.08], "color": "#333333"},
            wheel(0.32, 0.32), wheel(0.32, -0.32), wheel(-0.32, 0.32), wheel(-0.32, -0.32)
        ]})
    } else {
        json!({"name": "object", "parts": [
            {"label": "base", "shape": "box", "center": [0, 0.2, 0], "size": [0.8, 0.4, 0.8], "color": "#888888"},
            {"label": "top", "shape": "sphere", "center": [0, 0.65, 0], "size": 0.5, "color": "#cc4444"}
        ]})
    }
}

const PROSE: &str = "I'd love to help design that! It would have a body, a lid and a handle.";

fn router(f: Fake) -> Router {
    Router::new()
        .route("/anthropic/v1/models", get(anthropic_models))
        .route("/anthropic/v1/messages", post(anthropic_messages))
        .route("/openai/v1/models", get(openai_models))
        .route("/openai/v1/chat/completions", post(openai_chat))
        .route("/ollama/api/tags", get(ollama_tags))
        .route("/ollama/v1/chat/completions", post(ollama_chat))
        .route("/__admin/stats", get(admin_stats))
        .route("/__admin/config", post(admin_config))
        .route("/__admin/requests/{part}", get(admin_requests))
        .with_state(f)
}

// ---------- Anthropic ----------

fn anthropic_key(h: &HeaderMap) -> &str {
    h.get("x-api-key")
        .and_then(|v| v.to_str().ok())
        .unwrap_or("")
}

fn anthropic_auth(h: &HeaderMap) -> Option<Response> {
    if h.get("anthropic-version").is_none() {
        return Some(err(
            StatusCode::BAD_REQUEST,
            json!({"type":"error","error":{"type":"invalid_request_error","message":"anthropic-version header is required"}}),
        ));
    }
    let k = anthropic_key(h);
    (k != VALID_KEY && k != NOCREDIT_KEY).then(|| {
        err(StatusCode::UNAUTHORIZED, json!({"type":"error","error":{"type":"authentication_error","message":"invalid x-api-key"}}))
    })
}

async fn anthropic_models(State(f): State<Fake>, h: HeaderMap) -> Response {
    f.bump("anthropic.models");
    if let Some(r) = anthropic_auth(&h) {
        return r;
    }
    let data: Vec<Value> = ANTHROPIC_MODELS
        .iter()
        .map(|m| json!({"type": "model", "id": m, "display_name": m}))
        .collect();
    Json(json!({"data": data, "has_more": false})).into_response()
}

async fn anthropic_messages(
    State(f): State<Fake>,
    h: HeaderMap,
    Json(body): Json<Value>,
) -> Response {
    f.bump("anthropic.messages");
    record(&f, "/anthropic/v1/messages", &body);
    if let Some(r) = anthropic_auth(&h) {
        return r;
    }
    if anthropic_key(&h) == NOCREDIT_KEY {
        return err(
            StatusCode::BAD_REQUEST,
            json!({"type":"error","error":{"type":"invalid_request_error","message":"Your credit balance is too low to access the Anthropic API. Please go to Plans & Billing to upgrade or purchase credits."}}),
        );
    }
    let model = body["model"].as_str().unwrap_or("");
    if !ANTHROPIC_MODELS.contains(&model) {
        return err(
            StatusCode::NOT_FOUND,
            json!({"type":"error","error":{"type":"not_found_error","message":format!("model: {model}")}}),
        );
    }
    let prompt = body
        .pointer("/messages/0/content")
        .and_then(Value::as_str)
        .unwrap_or("");
    let system = body["system"].as_str().unwrap_or("");
    match answer(&f, system, prompt).await {
        Ok(text) => Json(json!({
            "id": "msg_fake", "type": "message", "role": "assistant", "model": model,
            "content": [{"type": "text", "text": text}],
            "stop_reason": "end_turn",
            "usage": {"input_tokens": 1, "output_tokens": 1}
        }))
        .into_response(),
        Err(r) => r,
    }
}

// ---------- OpenAI / Ollama ----------

fn bearer(h: &HeaderMap) -> &str {
    h.get(header::AUTHORIZATION)
        .and_then(|v| v.to_str().ok())
        .and_then(|v| v.strip_prefix("Bearer "))
        .unwrap_or("")
}

async fn openai_models(State(f): State<Fake>, h: HeaderMap) -> Response {
    f.bump("openai.models");
    let k = bearer(&h);
    if k != VALID_KEY && k != NOCREDIT_KEY {
        return err(
            StatusCode::UNAUTHORIZED,
            json!({"error":{"message":"Incorrect API key provided.","type":"invalid_request_error","code":"invalid_api_key"}}),
        );
    }
    let data: Vec<Value> = OPENAI_MODELS
        .iter()
        .enumerate()
        .map(|(i, m)| json!({"id": m, "object": "model", "created": 100 - i as i64}))
        .chain([json!({"id": "text-embedding-test", "object": "model", "created": 500})])
        .collect();
    Json(json!({"object": "list", "data": data})).into_response()
}

async fn openai_chat(State(f): State<Fake>, h: HeaderMap, Json(body): Json<Value>) -> Response {
    f.bump("openai.chat");
    record(&f, "/openai/v1/chat/completions", &body);
    let k = bearer(&h);
    if k == NOCREDIT_KEY {
        return err(
            StatusCode::TOO_MANY_REQUESTS,
            json!({"error":{"message":"You exceeded your current quota, please check your plan and billing details.","type":"insufficient_quota","code":"insufficient_quota"}}),
        );
    }
    if k != VALID_KEY {
        return err(
            StatusCode::UNAUTHORIZED,
            json!({"error":{"message":"Incorrect API key provided.","code":"invalid_api_key"}}),
        );
    }
    chat(&f, &body, &OPENAI_MODELS.map(String::from)).await
}

async fn ollama_tags(State(f): State<Fake>) -> Response {
    f.bump("ollama.tags");
    let cfg = f.cfg();
    if !cfg.ollama_running {
        return StatusCode::SERVICE_UNAVAILABLE.into_response();
    }
    let models: Vec<Value> = cfg
        .ollama_models
        .iter()
        .map(|m| json!({"name": m, "model": m}))
        .collect();
    Json(json!({"models": models})).into_response()
}

async fn ollama_chat(State(f): State<Fake>, Json(body): Json<Value>) -> Response {
    f.bump("ollama.chat");
    record(&f, "/ollama/v1/chat/completions", &body);
    let cfg = f.cfg();
    if !cfg.ollama_running {
        return StatusCode::SERVICE_UNAVAILABLE.into_response();
    }
    chat(&f, &body, &cfg.ollama_models).await
}

async fn chat(f: &Fake, body: &Value, models: &[String]) -> Response {
    let model = body["model"].as_str().unwrap_or("");
    if !models.iter().any(|m| m == model) {
        return err(
            StatusCode::NOT_FOUND,
            json!({"error":{"message":format!("The model `{model}` does not exist"),"type":"invalid_request_error","code":"model_not_found"}}),
        );
    }
    let prompt = body["messages"]
        .as_array()
        .and_then(|m| m.iter().find(|m| m["role"] == "user"))
        .and_then(|m| m["content"].as_str())
        .unwrap_or("");
    let system = body["messages"]
        .as_array()
        .and_then(|m| m.iter().find(|m| m["role"] == "system"))
        .and_then(|m| m["content"].as_str())
        .unwrap_or("");
    match answer(f, system, prompt).await {
        Ok(text) => Json(json!({
            "id": "chatcmpl-fake", "object": "chat.completion", "model": model,
            "choices": [{"index": 0, "message": {"role": "assistant", "content": text}, "finish_reason": "stop"}]
        }))
        .into_response(),
        Err(r) => r,
    }
}

/// The design text for one request, after any scripted fault.
#[allow(clippy::result_large_err)] // the error is an HTTP response returned once
async fn answer(f: &Fake, system: &str, prompt: &str) -> Result<String, Response> {
    let text = design_answer(f, prompt).await?;
    Ok(respond(system, prompt).unwrap_or(text))
}

/// Canned answers for adjustment requests: a design edit turns the first
/// part red; a splat edit recolours blue splats red.
pub fn respond(system: &str, prompt: &str) -> Option<String> {
    if system.contains("3D splat editor") {
        return Some(
            json!({"summary": "made the blue parts red", "operations": [
                {"op": "color", "color": "#d01010", "where": {"color": "#1f4fd1", "tolerance": 0.35}}
            ]})
            .to_string(),
        );
    }
    if system.contains("editing an existing design") {
        let start = prompt.find('{')?;
        let end = prompt.rfind("\n\nChange request:")?;
        let mut d: Value = serde_json::from_str(prompt[start..end].trim()).ok()?;
        // Designs are stored with numeric colours; answer in the model's hex style.
        d["parts"][0]["color"] = json!("#d01010");
        return Some(d.to_string());
    }
    None
}

#[allow(clippy::result_large_err)] // the error is an HTTP response returned once
async fn design_answer(f: &Fake, prompt: &str) -> Result<String, Response> {
    let delay = f.cfg().delay_ms;
    if delay > 0 {
        tokio::time::sleep(std::time::Duration::from_millis(delay)).await;
    }
    match f.fault_for(prompt) {
        Some(Fault::RateLimit) => Err((
            StatusCode::TOO_MANY_REQUESTS,
            [(header::RETRY_AFTER, "1")],
            Json(json!({"error":{"message":"Rate limit reached","type":"rate_limit_error"}})),
        )
            .into_response()),
        Some(Fault::ServerError) => Err(err(
            StatusCode::INTERNAL_SERVER_ERROR,
            json!({"error":{"message":"internal error"}}),
        )),
        Some(Fault::BadDesign) => Ok(PROSE.into()),
        None => Ok(design_for(prompt).to_string()),
    }
}

fn record(f: &Fake, path: &str, body: &Value) {
    f.inner
        .lock()
        .unwrap()
        .requests
        .push(json!({ "path": path, "body": body }));
}

// ---------- admin ----------

async fn admin_stats(State(f): State<Fake>) -> Json<Value> {
    Json(json!(f.inner.lock().unwrap().counters))
}

async fn admin_config(State(f): State<Fake>, Json(cfg): Json<FaultConfig>) -> StatusCode {
    let mut g = f.inner.lock().unwrap();
    g.cfg = cfg;
    g.bad_used = false;
    g.error_used = false;
    StatusCode::NO_CONTENT
}

async fn admin_requests(State(f): State<Fake>, AxPath(part): AxPath<String>) -> Json<Value> {
    Json(json!(f.requests(&part)))
}

// ---------- fake Claude Code CLI ----------

/// Behaves like `claude -p --output-format json` for tests: reads the user
/// prompt from stdin and prints a result object with a canned design.
/// `FAKE_CLAUDE_MODE=logged-out` simulates a signed-out CLI.
pub fn run_fake_claude(args: &[String]) -> i32 {
    if args.iter().any(|a| a == "--version") {
        println!("9.9.9 (Claude Code)");
        return 0;
    }
    let mut prompt = String::new();
    let _ = std::io::Read::read_to_string(&mut std::io::stdin(), &mut prompt);
    let has_tools_off = args
        .windows(2)
        .any(|w| w[0] == "--tools" && w[1].is_empty());
    let result = if std::env::var("FAKE_CLAUDE_MODE").as_deref() == Ok("logged-out") {
        json!({"type":"result","subtype":"success","is_error":true,"result":"Invalid API key · Please run /login"})
    } else if !has_tools_off {
        json!({"type":"result","subtype":"success","is_error":true,"result":"fake claude: tools were not disabled"})
    } else {
        let system = args
            .iter()
            .position(|a| a == "--system-prompt")
            .and_then(|i| args.get(i + 1))
            .map(String::as_str)
            .unwrap_or("");
        let text = respond(system, &prompt).unwrap_or_else(|| design_for(&prompt).to_string());
        json!({"type":"result","subtype":"success","is_error":false,"result": text})
    };
    println!("{result}");
    0
}

// ---------- fake Codex CLI ----------

/// Mirror of `maker_core::providers::codex::PROMPT_SEPARATOR` (this crate
/// can't depend on maker-core outside tests; `tests/codex_cli.rs` checks they match).
pub const CODEX_PROMPT_SEPARATOR: &str = "\n\n=== REQUEST ===\n\n";

/// Behaves like `codex exec … -` and `codex login status` for tests. Reads
/// the whole prompt from stdin, splits off the design instructions, and
/// prints only the final answer, like Codex does.
/// `FAKE_CODEX_MODE=logged-out` simulates a signed-out CLI.
pub fn run_fake_codex(args: &[String]) -> i32 {
    let logged_out = std::env::var("FAKE_CODEX_MODE").as_deref() == Ok("logged-out");
    if args.iter().any(|a| a == "--version") {
        println!("codex-cli 9.9.9");
        return 0;
    }
    if args.get(1).map(String::as_str) == Some("login") {
        if logged_out {
            eprintln!("Not logged in");
            return 1;
        }
        println!("Logged in using ChatGPT");
        return 0;
    }
    if args.get(1).map(String::as_str) != Some("exec") {
        eprintln!("fake codex: unsupported command");
        return 2;
    }
    let mut prompt = String::new();
    let _ = std::io::Read::read_to_string(&mut std::io::stdin(), &mut prompt);
    if logged_out {
        eprintln!("Error: Not logged in. Run `codex login`.");
        return 1;
    }
    let read_only = args
        .windows(2)
        .any(|w| w[0] == "--sandbox" && w[1] == "read-only");
    let safe = read_only
        && [
            "--ephemeral",
            "--skip-git-repo-check",
            "--ignore-user-config",
        ]
        .iter()
        .all(|f| args.iter().any(|a| a == f))
        && args.last().map(String::as_str) == Some("-");
    if !safe {
        eprintln!("fake codex: not run read-only, ephemeral and from stdin");
        return 3;
    }
    let (system, user) = prompt
        .split_once(CODEX_PROMPT_SEPARATOR)
        .unwrap_or(("", prompt.as_str()));
    eprintln!("[fake codex] thinking…");
    let text = respond(system, user).unwrap_or_else(|| design_for(user).to_string());
    println!("{text}");
    0
}

/// Whether this process was started under a name ending in `codex`, so one
/// binary can stand in for both CLIs.
pub fn invoked_as_codex(argv0: &str) -> bool {
    std::path::Path::new(argv0)
        .file_stem()
        .and_then(|s| s.to_str())
        .is_some_and(|s| s.ends_with("codex"))
}
