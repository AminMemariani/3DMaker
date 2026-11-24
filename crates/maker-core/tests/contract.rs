//! Provider contract tests: the production adapters and job engine run
//! against a local fake of the Anthropic, OpenAI and Ollama APIs
//! (`fake-provider`). No keys, no paid calls. Request counters prove that
//! double clicks, failures and restarts never silently repeat a model request.

use fake_provider::{Fake, NOCREDIT_KEY, Running, VALID_KEY};
use maker_core::engine::{
    DESIGN_FILE, Engine, EngineConfig, EventSink, ProviderFactory, SPLAT_FILE,
};
use maker_core::errors::ProviderError;
use maker_core::library::{Library, file_sha256};
use maker_core::model::*;
use maker_core::ply::{PlyLimits, validate_file};
use maker_core::providers::anthropic::Anthropic;
use maker_core::providers::openai::OpenAiCompat;
use maker_core::providers::{Endpoints, Provider};
use maker_core::redact::Secret;
use maker_core::store::Store;
use std::path::PathBuf;
use std::sync::{Arc, Mutex};
use std::time::Duration;
use tokio_util::sync::CancellationToken;

const TEAPOT: &str = "A cobalt-blue ceramic teapot with one curved spout, a round brass lid, and a large loop handle.";
const FORKLIFT: &str = "a yellow toy forklift with black wheels";

async fn server() -> Running {
    Fake::new().serve(0).await.unwrap()
}

fn endpoints(base: &str) -> Endpoints {
    let mut e = Endpoints::local_fake(base);
    e.request_timeout = Duration::from_secs(10);
    e
}

#[derive(Default)]
struct Events {
    jobs: Mutex<Vec<JobView>>,
}
impl EventSink for Events {
    fn job(&self, v: &JobView) {
        self.jobs.lock().unwrap().push(v.clone());
    }
    fn projects(&self, _: &[ProjectSummary]) {}
}

struct Harness {
    engine: Arc<Engine>,
    events: Arc<Events>,
    root: PathBuf,
    _dir: Option<tempfile::TempDir>,
}

/// Builds providers the way the app does, with fixed models.
fn factory(base: &str, key: &str) -> ProviderFactory {
    let ep = endpoints(base);
    let key = key.to_string();
    let custom_base = format!("{base}/openai/v1");
    Arc::new(move |id| -> Result<Arc<dyn Provider>, UserError> {
        let e = |err: ProviderError| err.to_user(id);
        Ok(match id {
            ProviderId::Anthropic => Arc::new(
                Anthropic::new(
                    Secret::new(key.clone()),
                    Some("claude-test-sonnet".into()),
                    &ep,
                )
                .map_err(e)?,
            ),
            ProviderId::OpenAi => Arc::new(
                OpenAiCompat::openai(Secret::new(key.clone()), Some("gpt-test".into()), &ep)
                    .map_err(e)?,
            ),
            ProviderId::Custom => Arc::new(
                OpenAiCompat::custom(
                    &custom_base,
                    Some(Secret::new(key.clone())),
                    Some("gpt-test-mini".into()),
                    true,
                    &ep,
                )
                .map_err(e)?,
            ),
            ProviderId::Ollama => {
                Arc::new(OpenAiCompat::ollama(Some("llama-test:latest".into()), &ep).map_err(e)?)
            }
            ProviderId::ClaudeCode | ProviderId::Codex => {
                return Err(UserError::internal(
                    "CLI providers are covered by fake-provider's tests",
                ));
            }
        })
    })
}

fn small() -> EngineConfig {
    EngineConfig {
        splat_budget: 6_000,
        ..Default::default()
    }
}

fn harness_at(root: PathBuf, dir: Option<tempfile::TempDir>, f: ProviderFactory) -> Harness {
    let lib = Library::new(&root).unwrap();
    let store = Arc::new(Store::open(&lib.db_path()).unwrap());
    let events = Arc::new(Events::default());
    let engine = Engine::new(store, lib, f, events.clone(), small());
    Harness {
        engine,
        events,
        root,
        _dir: dir,
    }
}

fn harness_with(base: &str, key: &str) -> Harness {
    let dir = tempfile::tempdir().unwrap();
    harness_at(dir.path().to_path_buf(), Some(dir), factory(base, key))
}

fn harness(base: &str) -> Harness {
    harness_with(base, VALID_KEY)
}

fn req(prompt: &str, provider: ProviderId) -> GenerateRequest {
    GenerateRequest {
        prompt: prompt.into(),
        provider,
        client_request_id: uuid::Uuid::new_v4().to_string(),
    }
}

async fn wait_done(h: &Harness, id: &str) -> ProjectDetail {
    for _ in 0..1500 {
        let d = h.engine.detail(id).unwrap();
        if d.summary.state.is_terminal() && !h.engine.is_busy() {
            return d;
        }
        tokio::time::sleep(Duration::from_millis(10)).await;
    }
    panic!(
        "timed out; last state {:?}",
        h.engine.detail(id).unwrap().summary.state
    );
}

async fn wait_state(h: &Harness, id: &str, s: ProjectState) {
    for _ in 0..1000 {
        if h.engine.detail(id).unwrap().summary.state == s {
            return;
        }
        tokio::time::sleep(Duration::from_millis(5)).await;
    }
    panic!("never reached {s:?}");
}

fn project_dir(h: &Harness, id: &str) -> PathBuf {
    h.engine.library().project_dir(id)
}

// ---------- connection checks ----------

#[tokio::test]
async fn anthropic_verify_lists_models_sonnet_first_and_rejects_bad_keys() {
    let srv = server().await;
    let ep = endpoints(&srv.base);
    let ok = Anthropic::new(Secret::new(VALID_KEY), None, &ep)
        .unwrap()
        .verify()
        .await
        .unwrap();
    assert_eq!(ok.default_model(), Some("claude-test-sonnet"));
    assert_eq!(ok.models.len(), 2);
    let bad = Anthropic::new(Secret::new("nope"), None, &ep)
        .unwrap()
        .verify()
        .await;
    assert_eq!(bad.unwrap_err(), ProviderError::InvalidCredentials);
    // The check is free: no design request was sent.
    assert_eq!(srv.count("anthropic.messages"), 0);
}

#[tokio::test]
async fn openai_verify_filters_non_chat_models() {
    let srv = server().await;
    let ep = endpoints(&srv.base);
    let v = OpenAiCompat::openai(Secret::new(VALID_KEY), None, &ep)
        .unwrap()
        .verify()
        .await
        .unwrap();
    assert_eq!(v.models, ["gpt-test", "gpt-test-mini"]);
    let bad = OpenAiCompat::openai(Secret::new("nope"), None, &ep)
        .unwrap()
        .verify()
        .await;
    assert_eq!(bad.unwrap_err(), ProviderError::InvalidCredentials);
}

#[tokio::test]
async fn ollama_verify_lists_local_models_and_reports_when_not_running() {
    let srv = server().await;
    let ep = endpoints(&srv.base);
    let v = OpenAiCompat::ollama(None, &ep)
        .unwrap()
        .verify()
        .await
        .unwrap();
    assert_eq!(
        v.models,
        ["llama-test:latest"],
        "embedding models are skipped"
    );

    srv.fake.configure(|c| c.ollama_models.clear());
    let none = OpenAiCompat::ollama(None, &ep)
        .unwrap()
        .verify()
        .await
        .unwrap_err();
    assert!(matches!(none, ProviderError::Unavailable(m) if m.contains("no models")));

    // Nothing listening at all.
    let mut closed = ep.clone();
    closed.ollama_api = "http://127.0.0.1:9".into();
    let down = OpenAiCompat::ollama(None, &closed)
        .unwrap()
        .verify()
        .await
        .unwrap_err();
    assert!(matches!(down, ProviderError::Unavailable(m) if m.contains("isn't running")));
}

#[tokio::test]
async fn credentials_are_never_sent_to_another_origin() {
    let srv = server().await;
    let ep = endpoints(&srv.base);
    // A custom server on plain http that isn't loopback is refused outright.
    assert!(matches!(
        maker_core::providers::normalize_base("http://example.com/v1", true),
        Err(ProviderError::Policy(_))
    ));
    // Cloud presets refuse plain http unless explicitly allowed (tests only).
    let mut strict = ep.clone();
    strict.allow_loopback_http = false;
    let err = Anthropic::new(Secret::new(VALID_KEY), None, &strict)
        .unwrap()
        .verify()
        .await;
    assert!(matches!(err, Err(ProviderError::Policy(_))));
}

// ---------- happy paths ----------

#[tokio::test(flavor = "multi_thread")]
async fn anthropic_teapot_end_to_end_with_exact_inputs() {
    let srv = server().await;
    let h = harness(&srv.base);
    let id = h
        .engine
        .start_generation(req(TEAPOT, ProviderId::Anthropic))
        .unwrap();
    let d = wait_done(&h, &id).await;
    assert_eq!(d.summary.state, ProjectState::Ready, "{:?}", d.error);
    assert_eq!(srv.count("anthropic.messages"), 1);

    let body = &srv.fake.requests("/anthropic/v1/messages")[0]["body"];
    assert_eq!(body["model"], "claude-test-sonnet");
    let user = body["messages"][0]["content"].as_str().unwrap();
    assert!(user.contains(TEAPOT), "the description is sent verbatim");
    assert!(
        body["system"]
            .as_str()
            .unwrap()
            .contains("single JSON object")
    );

    // Design and splat saved; the splat validates and matches its record.
    assert_eq!(d.part_count, Some(6));
    assert_eq!(d.model.as_deref(), Some("claude-test-sonnet"));
    assert_eq!(d.view_rotation, [0.0; 3]);
    let splat = PathBuf::from(d.splat_path.clone().unwrap());
    let s = validate_file(&splat, &PlyLimits::default()).unwrap();
    assert!(s.count >= 5_900, "{}", s.count);
    assert_eq!(
        d.splat_sha256.as_deref(),
        Some(file_sha256(&splat).unwrap().as_str())
    );
    assert!(project_dir(&h, &id).join(DESIGN_FILE).exists());

    // Export is byte-identical.
    let out = h.root.join("export.ply");
    let ex = h.engine.export(&id, &out).unwrap();
    assert_eq!(std::fs::read(&out).unwrap(), std::fs::read(&splat).unwrap());
    assert_eq!(ex.sha256, d.splat_sha256);

    // Progress went designing → building → saving → ready.
    let states: Vec<_> = h
        .events
        .jobs
        .lock()
        .unwrap()
        .iter()
        .map(|j| j.state)
        .collect();
    for s in [
        ProjectState::Designing,
        ProjectState::Building,
        ProjectState::Saving,
        ProjectState::Ready,
    ] {
        assert!(states.contains(&s), "missing {s:?} in {states:?}");
    }
}

#[tokio::test(flavor = "multi_thread")]
async fn openai_forklift_asks_for_json_and_custom_does_not() {
    let srv = server().await;
    let h = harness(&srv.base);
    let id = h
        .engine
        .start_generation(req(FORKLIFT, ProviderId::OpenAi))
        .unwrap();
    let d = wait_done(&h, &id).await;
    assert_eq!(d.summary.state, ProjectState::Ready, "{:?}", d.error);
    assert_eq!(d.part_count, Some(10));
    let body = &srv.fake.requests("/openai/v1/chat/completions")[0]["body"];
    assert_eq!(body["response_format"]["type"], "json_object");
    assert_eq!(body["messages"][0]["role"], "system");
    assert!(
        body["messages"][1]["content"]
            .as_str()
            .unwrap()
            .contains(FORKLIFT)
    );

    // The custom provider talks to an arbitrary server: no response_format.
    let id2 = h
        .engine
        .start_generation(req(FORKLIFT, ProviderId::Custom))
        .unwrap();
    assert_eq!(wait_done(&h, &id2).await.summary.state, ProjectState::Ready);
    let body2 = &srv.fake.requests("/openai/v1/chat/completions")[1]["body"];
    assert_eq!(body2["model"], "gpt-test-mini");
    assert!(body2.get("response_format").is_none());
}

#[tokio::test(flavor = "multi_thread")]
async fn ollama_generates_without_a_key() {
    let srv = server().await;
    let h = harness(&srv.base);
    let id = h
        .engine
        .start_generation(req(TEAPOT, ProviderId::Ollama))
        .unwrap();
    let d = wait_done(&h, &id).await;
    assert_eq!(d.summary.state, ProjectState::Ready, "{:?}", d.error);
    assert_eq!(srv.count("ollama.chat"), 1);
}

// ---------- failures and retries ----------

#[tokio::test(flavor = "multi_thread")]
async fn unusable_design_fails_clearly_and_ask_again_succeeds() {
    let srv = server().await;
    srv.fake
        .configure(|c| c.bad_design_once_for = Some("forklift".into()));
    let h = harness(&srv.base);
    let id = h
        .engine
        .start_generation(req(FORKLIFT, ProviderId::Anthropic))
        .unwrap();
    let d = wait_done(&h, &id).await;
    assert_eq!(d.summary.state, ProjectState::Failed);
    let e = d.error.unwrap();
    assert_eq!(
        (e.kind, e.action),
        (ErrorKind::InvalidDesign, NextAction::RetryAll)
    );
    assert!(e.message.contains("no JSON"), "{}", e.message);
    assert!(
        e.message.contains("billed again"),
        "cloud retries mention billing"
    );
    assert_eq!(
        d.retry,
        RetryOptions {
            rebuild: false,
            full: true
        }
    );
    // The raw answer is kept for troubleshooting.
    assert!(project_dir(&h, &id).join("answer.txt").exists());

    h.engine.retry(&id, RetryAction::Full).unwrap();
    let d = wait_done(&h, &id).await;
    assert_eq!(d.summary.state, ProjectState::Ready, "{:?}", d.error);
    assert_eq!(
        srv.count("anthropic.messages"),
        2,
        "exactly one extra, explicit request"
    );
}

#[tokio::test(flavor = "multi_thread")]
async fn server_error_after_sending_is_never_retried_automatically() {
    let srv = server().await;
    srv.fake
        .configure(|c| c.server_error_once_for = Some("teapot".into()));
    let h = harness(&srv.base);
    let id = h
        .engine
        .start_generation(req(TEAPOT, ProviderId::OpenAi))
        .unwrap();
    let d = wait_done(&h, &id).await;
    assert_eq!(d.summary.state, ProjectState::Failed);
    assert_eq!(d.error.unwrap().kind, ErrorKind::UnknownOutcome);
    assert_eq!(srv.count("openai.chat"), 1);
}

#[tokio::test(flavor = "multi_thread")]
async fn rate_limit_is_waited_out_because_the_request_was_refused() {
    let srv = server().await;
    srv.fake.configure(|c| c.rate_limit_next = 1);
    let h = harness(&srv.base);
    let id = h
        .engine
        .start_generation(req(TEAPOT, ProviderId::Anthropic))
        .unwrap();
    let d = wait_done(&h, &id).await;
    assert_eq!(d.summary.state, ProjectState::Ready, "{:?}", d.error);
    assert_eq!(srv.count("anthropic.messages"), 2);
}

#[tokio::test(flavor = "multi_thread")]
async fn billing_and_key_errors_are_plain_and_actionable() {
    let srv = server().await;
    let h = harness_with(&srv.base, NOCREDIT_KEY);
    let a = h
        .engine
        .start_generation(req(TEAPOT, ProviderId::Anthropic))
        .unwrap();
    let e = wait_done(&h, &a).await.error.unwrap();
    assert_eq!(
        (e.kind, e.action),
        (ErrorKind::NoCredit, NextAction::AddCredit)
    );
    assert!(e.action_url.unwrap().contains("anthropic.com"));
    let o = h
        .engine
        .start_generation(req(TEAPOT, ProviderId::OpenAi))
        .unwrap();
    assert_eq!(
        wait_done(&h, &o).await.error.unwrap().kind,
        ErrorKind::NoCredit
    );

    let h = harness_with(&srv.base, "wrong");
    let w = h
        .engine
        .start_generation(req(TEAPOT, ProviderId::OpenAi))
        .unwrap();
    let e = wait_done(&h, &w).await.error.unwrap();
    assert_eq!(
        (e.kind, e.action),
        (ErrorKind::InvalidKey, NextAction::ReplaceKey)
    );
}

#[tokio::test]
async fn unknown_model_is_reported_as_such() {
    let srv = server().await;
    let ep = endpoints(&srv.base);
    let p = Anthropic::new(Secret::new(VALID_KEY), Some("claude-nope".into()), &ep).unwrap();
    let e = p
        .complete("s", "u", &CancellationToken::new())
        .await
        .unwrap_err();
    assert!(matches!(e, ProviderError::ModelNotFound(_)), "{e:?}");
    let o = OpenAiCompat::ollama(Some("missing:latest".into()), &ep).unwrap();
    let e = o
        .complete("s", "u", &CancellationToken::new())
        .await
        .unwrap_err();
    assert!(matches!(e, ProviderError::ModelNotFound(_)), "{e:?}");
}

// ---------- concurrency, cancellation, recovery ----------

#[tokio::test(flavor = "multi_thread")]
async fn double_click_and_concurrent_generate_send_one_request() {
    let srv = server().await;
    srv.fake.configure(|c| c.delay_ms = 300);
    let h = harness(&srv.base);
    let r = req(TEAPOT, ProviderId::Anthropic);
    let a = h.engine.start_generation(r.clone()).unwrap();
    let b = h.engine.start_generation(r).unwrap();
    assert_eq!(a, b, "same click id → same object");
    let busy = h
        .engine
        .start_generation(req(FORKLIFT, ProviderId::Anthropic))
        .unwrap_err();
    assert_eq!(busy.kind, ErrorKind::Busy);
    wait_done(&h, &a).await;
    assert_eq!(srv.count("anthropic.messages"), 1);
}

#[tokio::test(flavor = "multi_thread")]
async fn cancel_reports_billing_risk_for_cloud_but_not_local() {
    let srv = server().await;
    srv.fake.configure(|c| c.delay_ms = 3_000);
    let h = harness(&srv.base);

    let id = h
        .engine
        .start_generation(req(TEAPOT, ProviderId::Anthropic))
        .unwrap();
    wait_state(&h, &id, ProjectState::Designing).await;
    let r = h.engine.cancel(&id).await;
    assert_eq!(r.outcome, CancelOutcome::LocalOnly);
    assert!(r.message.contains("bill"), "{}", r.message);
    assert_eq!(
        wait_done(&h, &id).await.summary.state,
        ProjectState::Cancelled
    );

    let id = h
        .engine
        .start_generation(req(TEAPOT, ProviderId::Ollama))
        .unwrap();
    wait_state(&h, &id, ProjectState::Designing).await;
    let r = h.engine.cancel(&id).await;
    assert_eq!(r.outcome, CancelOutcome::Confirmed);
    assert!(!r.message.contains("bill"));
    let d = wait_done(&h, &id).await;
    assert_eq!(
        d.retry,
        RetryOptions {
            rebuild: false,
            full: true
        }
    );

    let r = h.engine.cancel(&id).await;
    assert_eq!(r.outcome, CancelOutcome::AlreadyFinished);
}

#[tokio::test(flavor = "multi_thread")]
async fn restart_after_interrupted_build_rebuilds_for_free() {
    let srv = server().await;
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path().to_path_buf();
    let id = {
        let h = harness_at(root.clone(), None, factory(&srv.base, VALID_KEY));
        let id = h
            .engine
            .start_generation(req(TEAPOT, ProviderId::Anthropic))
            .unwrap();
        assert_eq!(wait_done(&h, &id).await.summary.state, ProjectState::Ready);
        // Simulate a crash mid-build: the design exists, the splat doesn't.
        std::fs::remove_file(project_dir(&h, &id).join(SPLAT_FILE)).unwrap();
        h.engine
            .store()
            .set_state(&id, ProjectState::Building, None)
            .unwrap();
        id
    };
    let h = harness_at(root, Some(dir), factory(&srv.base, VALID_KEY));
    assert_eq!(h.engine.recover(), 1);
    let d = h.engine.detail(&id).unwrap();
    assert_eq!(d.summary.state, ProjectState::NeedsAttention);
    assert_eq!(d.error.unwrap().action, NextAction::Rebuild);
    assert_eq!(
        d.retry,
        RetryOptions {
            rebuild: true,
            full: true
        }
    );

    h.engine.retry(&id, RetryAction::Rebuild).unwrap();
    let d = wait_done(&h, &id).await;
    assert_eq!(d.summary.state, ProjectState::Ready, "{:?}", d.error);
    assert_eq!(
        srv.count("anthropic.messages"),
        1,
        "rebuild never asks the model"
    );
}

#[tokio::test(flavor = "multi_thread")]
async fn restart_during_design_sends_nothing_until_asked() {
    let srv = server().await;
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path().to_path_buf();
    let id = {
        let h = harness_at(root.clone(), None, factory(&srv.base, VALID_KEY));
        let mut row = maker_core::store::ProjectRow::new("p1".into(), TEAPOT.into(), "openai");
        row.state = ProjectState::Designing;
        h.engine.store().insert_project(&row).unwrap();
        h.engine
            .store()
            .begin_stage("p1", StageKind::Design, "gpt-test")
            .unwrap();
        "p1".to_string()
    };
    let h = harness_at(root, Some(dir), factory(&srv.base, VALID_KEY));
    h.engine.recover();
    tokio::time::sleep(Duration::from_millis(100)).await;
    let d = h.engine.detail(&id).unwrap();
    assert_eq!(d.summary.state, ProjectState::NeedsAttention);
    assert_eq!(d.error.unwrap().action, NextAction::RetryAll);
    assert_eq!(
        d.retry,
        RetryOptions {
            rebuild: false,
            full: true
        }
    );
    assert_eq!(srv.count("openai.chat"), 0);
}

// ---------- files and secrets ----------

#[tokio::test(flavor = "multi_thread")]
async fn imports_keep_their_orientation_convention() {
    let srv = server().await;
    let h = harness(&srv.base);
    let id = h
        .engine
        .start_generation(req(TEAPOT, ProviderId::Anthropic))
        .unwrap();
    wait_done(&h, &id).await;
    let own = h.root.join("own.ply");
    h.engine.export(&id, &own).unwrap();
    let a = h.engine.import(own).await.unwrap();
    assert_eq!(h.engine.detail(&a).unwrap().view_rotation, [0.0; 3]);

    let foreign = h.root.join("foreign.ply");
    std::fs::write(
        &foreign,
        maker_core::fixtures::encode(&maker_core::fixtures::teapot(500, 3), &Default::default()),
    )
    .unwrap();
    let b = h.engine.import(foreign).await.unwrap();
    let d = h.engine.detail(&b).unwrap();
    assert_eq!(d.view_rotation, [180.0, 0.0, 0.0]);
    assert_eq!(
        d.retry,
        RetryOptions::default(),
        "imports can't be regenerated"
    );
}

#[tokio::test(flavor = "multi_thread")]
async fn keys_never_reach_disk_or_errors() {
    let srv = server().await;
    let h = harness_with(&srv.base, NOCREDIT_KEY);
    let id = h
        .engine
        .start_generation(req(TEAPOT, ProviderId::Anthropic))
        .unwrap();
    let d = wait_done(&h, &id).await;
    let err = serde_json::to_string(&d.error).unwrap();
    assert!(!err.contains(NOCREDIT_KEY));
    for entry in walk(&h.root) {
        let bytes = std::fs::read(&entry).unwrap();
        assert!(
            !bytes
                .windows(NOCREDIT_KEY.len())
                .any(|w| w == NOCREDIT_KEY.as_bytes()),
            "key found in {}",
            entry.display()
        );
    }
}

// ---------- user files and prompt adjustments ----------

fn adjust_req() -> String {
    uuid::Uuid::new_v4().to_string()
}

#[tokio::test(flavor = "multi_thread")]
async fn adjusting_a_designed_object_edits_its_design_into_a_new_object() {
    let srv = server().await;
    let h = harness(&srv.base);
    let src = h
        .engine
        .start_generation(req(TEAPOT, ProviderId::Anthropic))
        .unwrap();
    let before = wait_done(&h, &src).await;
    let id = h
        .engine
        .adjust(
            &src,
            "make the body red",
            ProviderId::Anthropic,
            &adjust_req(),
        )
        .unwrap();
    let d = wait_done(&h, &id).await;
    assert_eq!(d.summary.state, ProjectState::Ready, "{:?}", d.error);
    assert_ne!(id, src);
    assert!(d.summary.prompt.ends_with("→ make the body red"));
    // The model got the current design and the request.
    let body = &srv.fake.requests("/anthropic/v1/messages")[1]["body"];
    assert!(
        body["system"]
            .as_str()
            .unwrap()
            .contains("editing an existing design")
    );
    let user = body["messages"][0]["content"].as_str().unwrap();
    assert!(user.contains("\"parts\"") && user.contains("make the body red"));
    // New design: first part red, the rest unchanged; the source is untouched.
    let new: maker_core::design::Design = serde_json::from_str(
        &std::fs::read_to_string(project_dir(&h, &id).join(DESIGN_FILE)).unwrap(),
    )
    .unwrap();
    assert!(new.parts[0].color[0] > 0.8 && new.parts[0].color[2] < 0.1);
    assert_eq!(new.parts.len(), 6);
    assert_eq!(
        h.engine.detail(&src).unwrap().splat_sha256,
        before.splat_sha256
    );
}

#[tokio::test(flavor = "multi_thread")]
async fn adjusting_an_imported_mesh_applies_operations_and_rebuilds_for_free() {
    let srv = server().await;
    let h = harness(&srv.base);
    // A blue square, as an OBJ with vertex colours.
    let obj = h.root.join("square.obj");
    std::fs::write(&obj, "v 0 0 0 0.12 0.31 0.82\nv 1 0 0 0.12 0.31 0.82\nv 1 1 0 0.12 0.31 0.82\nv 0 1 0 0.12 0.31 0.82\nf 1 2 3 4\n").unwrap();
    let src = h.engine.import(obj).await.unwrap();
    let s = h.engine.detail(&src).unwrap();
    assert_eq!(
        (s.summary.state, s.summary.provider.as_str()),
        (ProjectState::Ready, "import")
    );
    assert!(s.summary.splat_count.unwrap() > 5_000);

    let id = h
        .engine
        .adjust(&src, "make it red", ProviderId::OpenAi, &adjust_req())
        .unwrap();
    let d = wait_done(&h, &id).await;
    assert_eq!(d.summary.state, ProjectState::Ready, "{:?}", d.error);
    assert_eq!(d.view_rotation, [0.0; 3], "orientation is baked in");
    let body = &srv.fake.requests("/openai/v1/chat/completions")[0]["body"];
    assert!(
        body["messages"][0]["content"]
            .as_str()
            .unwrap()
            .contains("3D splat editor")
    );
    let user = body["messages"][1]["content"].as_str().unwrap();
    assert!(
        user.contains("splats") && user.contains("make it red"),
        "{user}"
    );
    let t = maker_core::ply::load_table(
        std::path::Path::new(d.splat_path.as_ref().unwrap()),
        &PlyLimits::default(),
    )
    .unwrap();
    let r = t.data[t.col("f_dc_0").unwrap()];
    let b = t.data[t.col("f_dc_2").unwrap()];
    assert!(r > b, "the square is now red");

    // A lost result rebuilds from the saved operations without asking again.
    std::fs::remove_file(project_dir(&h, &id).join(SPLAT_FILE)).unwrap();
    h.engine
        .store()
        .set_state(&id, ProjectState::Building, None)
        .unwrap();
    h.engine.recover();
    assert_eq!(
        h.engine.detail(&id).unwrap().retry,
        RetryOptions {
            rebuild: true,
            full: true
        }
    );
    h.engine.retry(&id, RetryAction::Rebuild).unwrap();
    assert_eq!(wait_done(&h, &id).await.summary.state, ProjectState::Ready);
    assert_eq!(srv.count("openai.chat"), 1);
}

#[tokio::test(flavor = "multi_thread")]
async fn bad_mesh_files_are_refused_plainly() {
    let srv = server().await;
    let h = harness(&srv.base);
    let bad = h.root.join("broken.stl");
    std::fs::write(&bad, "this is not a model").unwrap();
    let e = h.engine.import(bad).await.unwrap_err();
    assert_eq!(e.kind, ErrorKind::InvalidAsset);
    assert!(
        h.engine.summaries().is_empty(),
        "nothing half-imported is left behind"
    );
}

fn walk(dir: &std::path::Path) -> Vec<PathBuf> {
    let mut out = vec![];
    for e in std::fs::read_dir(dir).unwrap().flatten() {
        let p = e.path();
        if p.is_dir() {
            out.extend(walk(&p));
        } else {
            out.push(p);
        }
    }
    out
}
