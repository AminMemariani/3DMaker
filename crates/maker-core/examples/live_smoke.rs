//! LIVE smoke test against a real provider (opt-in; cloud providers bill).
//!
//!   ANTHROPIC_API_KEY=... cargo run -p maker-core --example live_smoke -- --provider anthropic --model <id> --yes-spend
//!   OPENAI_API_KEY=...    cargo run -p maker-core --example live_smoke -- --provider openai --model <id> --yes-spend
//!   cargo run -p maker-core --example live_smoke -- --provider claude-code          (your Claude plan)
//!   cargo run -p maker-core --example live_smoke -- --provider codex                (your ChatGPT plan)
//!   cargo run -p maker-core --example live_smoke -- --provider ollama --model <name> (free, local)
//!
//! Runs at most two generations (the teapot and forklift acceptance prompts;
//! `--only-teapot` runs one), i.e. at most two model requests, through the
//! same engine the app uses. Results are saved under
//! tests/output/live-<provider>-<time>/ with a JSON record. Without
//! `--model`, the first model from the provider's free model list is used.
//! Inspect the PLYs in the app (Open .ply…) before claiming live validation.

use maker_core::engine::{Engine, EngineConfig, NoEvents, ProviderFactory};
use maker_core::library::Library;
use maker_core::model::*;
use maker_core::providers::anthropic::Anthropic;
use maker_core::providers::claude_code::{self, ClaudeCode};
use maker_core::providers::codex::{self, Codex};
use maker_core::providers::openai::OpenAiCompat;
use maker_core::providers::{Endpoints, Provider};
use maker_core::redact::Secret;
use maker_core::store::Store;
use std::sync::Arc;
use std::time::{Duration, Instant};

const PROMPTS: [&str; 2] = [
    "A cobalt-blue ceramic teapot with one curved spout, a round brass lid, and a large loop handle.",
    "a yellow toy forklift with black wheels",
];

fn build(
    id: ProviderId,
    model: Option<String>,
    ep: &Endpoints,
) -> Result<Arc<dyn Provider>, String> {
    let key = |var: &str| {
        std::env::var(var)
            .map(Secret::new)
            .map_err(|_| format!("set {var} first"))
    };
    let e = |err: maker_core::errors::ProviderError| err.to_string();
    Ok(match id {
        ProviderId::Anthropic => {
            Arc::new(Anthropic::new(key("ANTHROPIC_API_KEY")?, model, ep).map_err(e)?)
        }
        ProviderId::OpenAi => {
            Arc::new(OpenAiCompat::openai(key("OPENAI_API_KEY")?, model, ep).map_err(e)?)
        }
        ProviderId::Ollama => Arc::new(OpenAiCompat::ollama(model, ep).map_err(e)?),
        ProviderId::ClaudeCode => {
            let bin = claude_code::find(None).ok_or("Claude Code not found")?;
            Arc::new(ClaudeCode::new(bin, model, ep.request_timeout))
        }
        ProviderId::Codex => {
            let bin = codex::find(None).ok_or("Codex not found")?;
            Arc::new(Codex::new(bin, model, ep.request_timeout))
        }
        ProviderId::Custom => return Err("use the app for custom providers".into()),
    })
}

#[tokio::main]
async fn main() {
    let args: Vec<String> = std::env::args().collect();
    let has = |f: &str| args.iter().any(|a| a == f);
    let arg = |f: &str| {
        args.iter()
            .position(|a| a == f)
            .and_then(|i| args.get(i + 1))
            .cloned()
    };
    let Some(id) = arg("--provider").and_then(|p| ProviderId::parse(&p)) else {
        eprintln!(
            "usage: --provider anthropic|openai|claude-code|codex|ollama [--model ID] [--only-teapot] [--yes-spend]"
        );
        std::process::exit(2);
    };
    if id.bills_per_call() && !has("--yes-spend") {
        eprintln!(
            "{} bills per request. Re-run with --yes-spend to confirm (at most 2 requests).",
            id.display_name()
        );
        std::process::exit(2);
    }
    let ep = Endpoints::default();
    let mut model = arg("--model");
    if model.is_none() {
        let probe = build(id, None, &ep).unwrap_or_else(|e| panic!("{e}"));
        let v = probe
            .verify()
            .await
            .unwrap_or_else(|e| panic!("connection check failed: {e}"));
        model = v.default_model().map(String::from);
        eprintln!("using model {}", model.as_deref().unwrap_or("(default)"));
    }
    let provider = build(id, model, &ep).unwrap_or_else(|e| panic!("{e}"));
    let factory: ProviderFactory = {
        let p = provider.clone();
        Arc::new(move |_| Ok(p.clone()))
    };
    let stamp = now_unix();
    let out = std::path::PathBuf::from(format!("tests/output/live-{}-{stamp}", id.as_str()));
    let lib = Library::new(&out).unwrap();
    let store = Arc::new(Store::open(&lib.db_path()).unwrap());
    let engine = Engine::new(
        store,
        lib,
        factory,
        Arc::new(NoEvents),
        EngineConfig::default(),
    );
    let prompts = if has("--only-teapot") {
        &PROMPTS[..1]
    } else {
        &PROMPTS[..]
    };
    let mut record = vec![];
    let mut ok = true;
    for prompt in prompts {
        let started = Instant::now();
        let pid = engine
            .start_generation(GenerateRequest {
                prompt: prompt.to_string(),
                provider: id,
                client_request_id: uuid::Uuid::new_v4().to_string(),
            })
            .unwrap();
        let d = loop {
            tokio::time::sleep(Duration::from_millis(250)).await;
            let d = engine.detail(&pid).unwrap();
            if d.summary.state.is_terminal() && !engine.is_busy() {
                break d;
            }
        };
        ok &= d.summary.state == ProjectState::Ready;
        eprintln!(
            "{:?} in {:.1}s: {} parts, {} splats {}",
            d.summary.state,
            started.elapsed().as_secs_f32(),
            d.part_count.unwrap_or(0),
            d.summary.splat_count.unwrap_or(0),
            d.error.as_ref().map(|e| e.to_string()).unwrap_or_default()
        );
        record.push(serde_json::json!({
            "prompt": prompt, "state": d.summary.state, "model": d.model, "parts": d.part_count,
            "splats": d.summary.splat_count, "splatPath": d.splat_path, "sha256": d.splat_sha256,
            "seconds": started.elapsed().as_secs_f32(), "error": d.error,
        }));
    }
    let rec = out.join("live-record.json");
    std::fs::write(&rec, serde_json::to_vec_pretty(&record).unwrap()).unwrap();
    eprintln!("record: {}", rec.display());
    std::process::exit(if ok { 0 } else { 1 });
}
