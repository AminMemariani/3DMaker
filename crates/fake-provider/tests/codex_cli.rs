//! Runs the production Codex adapter and local detection against this
//! crate's binary, started through a symlink named `codex` so it acts as a
//! fake Codex CLI.
#![cfg(unix)]

use maker_core::detect;
use maker_core::errors::ProviderError;
use maker_core::model::ProviderId;
use maker_core::providers::codex::{Codex, PROMPT_SEPARATOR};
use maker_core::providers::{Endpoints, Provider};
use std::path::PathBuf;
use std::time::Duration;
use tokio_util::sync::CancellationToken;

/// A temp dir holding `codex` -> fake-provider (kept alive by the caller).
fn fake_codex() -> (tempfile::TempDir, PathBuf) {
    let dir = tempfile::tempdir().unwrap();
    let link = dir.path().join("codex");
    std::os::unix::fs::symlink(env!("CARGO_BIN_EXE_fake-provider"), &link).unwrap();
    (dir, link)
}

/// A wrapper script named `codex` that runs the fake signed out.
fn signed_out_codex() -> (tempfile::TempDir, PathBuf) {
    let (dir, link) = fake_codex();
    let script = dir.path().join("signed-out-codex");
    std::fs::write(
        &script,
        format!(
            "#!/bin/sh\nFAKE_CODEX_MODE=logged-out exec \"{}\" \"$@\"\n",
            link.display()
        ),
    )
    .unwrap();
    use std::os::unix::fs::PermissionsExt;
    std::fs::set_permissions(&script, std::fs::Permissions::from_mode(0o755)).unwrap();
    (dir, script)
}

#[test]
fn fake_uses_the_same_prompt_separator() {
    assert_eq!(fake_provider::CODEX_PROMPT_SEPARATOR, PROMPT_SEPARATOR);
}

#[tokio::test]
async fn designs_through_codex_read_only() {
    let (_d, bin) = fake_codex();
    let c = Codex::new(bin, None, Duration::from_secs(20));
    let v = c.verify().await.unwrap();
    assert_eq!(v.account_label.as_deref(), Some("Codex 9.9.9"));
    assert_eq!(v.default_model(), Some("default"));
    // The fake refuses unless run read-only, ephemeral, outside git checks and from stdin.
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
async fn detection_finds_the_overridden_codex() {
    let (_d, bin) = fake_codex();
    let ep = Endpoints {
        codex_cli: Some(bin),
        ..Endpoints::default()
    };
    let found = detect::codex(&ep).await;
    assert!(found.available && found.installed, "{found:?}");
    assert!(found.detail.unwrap().starts_with("9.9.9 at "));

    let missing = Endpoints {
        codex_cli: Some(PathBuf::from("/nonexistent/codex")),
        ..Endpoints::default()
    };
    let none = detect::codex(&missing).await;
    assert!(!none.available && !none.installed && none.hint.is_some());

    // detect::all reports every local provider, Codex included.
    let ids: Vec<_> = detect::all(&missing)
        .await
        .into_iter()
        .map(|d| d.id)
        .collect();
    assert_eq!(
        ids,
        [
            ProviderId::ClaudeCode,
            ProviderId::Codex,
            ProviderId::Ollama
        ]
    );
}

#[tokio::test]
async fn signed_out_codex_maps_to_sign_in_guidance() {
    let (_d, bin) = signed_out_codex();
    let c = Codex::new(bin, None, Duration::from_secs(20));
    assert_eq!(
        c.verify().await.unwrap_err(),
        ProviderError::InvalidCredentials
    );
    let r = c.complete("s", "u", &CancellationToken::new()).await;
    assert_eq!(r, Err(ProviderError::InvalidCredentials));
    let ue = ProviderError::InvalidCredentials.to_user(ProviderId::Codex);
    assert!(ue.message.contains("codex login"), "{}", ue.message);
}

#[tokio::test]
async fn cancel_ends_codex_promptly() {
    let dir = tempfile::tempdir().unwrap();
    let script = dir.path().join("codex");
    std::fs::write(&script, "#!/bin/sh\nsleep 30\n").unwrap();
    use std::os::unix::fs::PermissionsExt;
    std::fs::set_permissions(&script, std::fs::Permissions::from_mode(0o755)).unwrap();
    let c = Codex::new(script, None, Duration::from_secs(60));
    let cancel = CancellationToken::new();
    let c2 = cancel.clone();
    tokio::spawn(async move {
        tokio::time::sleep(Duration::from_millis(200)).await;
        c2.cancel();
    });
    let started = std::time::Instant::now();
    assert_eq!(
        c.complete("s", "u", &cancel).await,
        Err(ProviderError::Cancelled)
    );
    assert!(started.elapsed() < Duration::from_secs(5));
}
