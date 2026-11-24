//! Runs the production Claude Code adapter and local detection against this
//! crate's binary, which acts as a fake `claude` CLI.

use maker_core::detect;
use maker_core::errors::ProviderError;
use maker_core::providers::claude_code::ClaudeCode;
use maker_core::providers::{Endpoints, Provider};
use std::path::PathBuf;
use std::time::Duration;
use tokio_util::sync::CancellationToken;

fn fake_cli() -> PathBuf {
    PathBuf::from(env!("CARGO_BIN_EXE_fake-provider"))
}

#[tokio::test]
async fn designs_through_the_cli_with_tools_disabled() {
    let c = ClaudeCode::new(fake_cli(), Some("sonnet".into()), Duration::from_secs(20));
    let v = c.verify().await.unwrap();
    assert_eq!(v.account_label.as_deref(), Some("Claude Code 9.9.9"));
    assert_eq!(v.default_model(), Some("default"));
    // The fake refuses to answer unless `--tools ""` was passed.
    let text = c
        .complete(
            &maker_core::design::system_prompt(),
            "a yellow toy forklift",
            &CancellationToken::new(),
        )
        .await
        .unwrap();
    let d = maker_core::design::parse(&text).unwrap();
    assert_eq!(d.name, "forklift");
}

#[tokio::test]
async fn detection_finds_the_overridden_cli() {
    let ep = Endpoints {
        claude_cli: Some(fake_cli()),
        ..Endpoints::default()
    };
    let found = detect::claude_code(&ep).await;
    assert!(found.available && found.installed, "{found:?}");
    assert!(found.detail.unwrap().starts_with("9.9.9"));

    let missing = Endpoints {
        claude_cli: Some(PathBuf::from("/nonexistent/claude")),
        ..Endpoints::default()
    };
    let none = detect::claude_code(&missing).await;
    assert!(!none.available && !none.installed && none.hint.is_some());
}

#[tokio::test]
async fn ollama_detection_reports_models_and_absence() {
    let srv = fake_provider::Fake::new().serve(0).await.unwrap();
    let ep = Endpoints::local_fake(&srv.base);
    let o = detect::ollama(&ep).await;
    assert!(o.available);
    assert_eq!(o.models, ["llama-test:latest"]);
    let closed = Endpoints {
        ollama_api: "http://127.0.0.1:9".into(),
        ..Endpoints::default()
    };
    let down = detect::ollama(&closed).await;
    assert!(!down.available && down.hint.is_some());
}

#[tokio::test]
async fn cancel_ends_the_cli_promptly() {
    // A Claude Code that never answers: `sleep` stands in on unix.
    #[cfg(unix)]
    {
        let dir = tempfile::tempdir().unwrap();
        let script = dir.path().join("claude");
        std::fs::write(&script, "#!/bin/sh\nsleep 30\n").unwrap();
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(&script, std::fs::Permissions::from_mode(0o755)).unwrap();
        let c = ClaudeCode::new(script, None, Duration::from_secs(60));
        let cancel = CancellationToken::new();
        let c2 = cancel.clone();
        tokio::spawn(async move {
            tokio::time::sleep(Duration::from_millis(200)).await;
            c2.cancel();
        });
        let started = std::time::Instant::now();
        let r = c.complete("s", "u", &cancel).await;
        assert_eq!(r, Err(ProviderError::Cancelled));
        assert!(started.elapsed() < Duration::from_secs(5));
    }
}

#[tokio::test]
async fn signed_out_cli_maps_to_sign_in_guidance() {
    // A wrapper sets the fake's signed-out mode only for its own process, so
    // concurrent tests are unaffected.
    #[cfg(unix)]
    {
        let dir = tempfile::tempdir().unwrap();
        let script = dir.path().join("claude");
        std::fs::write(
            &script,
            format!(
                "#!/bin/sh\nFAKE_CLAUDE_MODE=logged-out exec \"{}\" \"$@\"\n",
                fake_cli().display()
            ),
        )
        .unwrap();
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(&script, std::fs::Permissions::from_mode(0o755)).unwrap();
        let c = ClaudeCode::new(script, None, Duration::from_secs(20));
        let r = c.complete("s", "u", &CancellationToken::new()).await;
        assert_eq!(r, Err(ProviderError::InvalidCredentials));
    }
    let ue = ProviderError::InvalidCredentials.to_user(maker_core::model::ProviderId::ClaudeCode);
    assert!(ue.message.contains("sign in"));
}
