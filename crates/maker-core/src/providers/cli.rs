//! Shared helpers for local command-line providers (Claude Code, Codex):
//! finding the executable, reading its version, and running one request.

use crate::errors::ProviderError;
use std::path::{Path, PathBuf};
use std::process::{Output, Stdio};
use std::time::Duration;
use tokio::io::AsyncWriteExt;
use tokio_util::sync::CancellationToken;

/// Where a CLI named `name` is usually installed. Desktop apps on macOS don't
/// inherit the shell's PATH, so common locations are checked explicitly.
/// `extra` lists tool-specific places (for example inside an app bundle).
pub fn candidates(name: &str, extra: &[PathBuf]) -> Vec<PathBuf> {
    let exe = if cfg!(windows) {
        format!("{name}.exe")
    } else {
        name.to_string()
    };
    let mut out = Vec::new();
    if let Some(path) = std::env::var_os("PATH") {
        out.extend(std::env::split_paths(&path).map(|d| d.join(&exe)));
        if cfg!(windows) {
            out.extend(std::env::split_paths(&path).map(|d| d.join(format!("{name}.cmd"))));
        }
    }
    if let Some(home) = std::env::var_os(if cfg!(windows) { "USERPROFILE" } else { "HOME" }) {
        let home = PathBuf::from(home);
        out.push(home.join(".local/bin").join(&exe));
        out.push(home.join(".npm-global/bin").join(&exe));
        out.push(home.join(".bun/bin").join(&exe));
        out.push(home.join(".volta/bin").join(&exe));
        if cfg!(windows) {
            out.push(home.join(format!("AppData/Roaming/npm/{name}.cmd")));
        }
    }
    out.extend(extra.iter().cloned());
    if !cfg!(windows) {
        out.push(PathBuf::from(format!("/opt/homebrew/bin/{name}")));
        out.push(PathBuf::from(format!("/usr/local/bin/{name}")));
    }
    out
}

/// Finds an executable, preferring an explicit override.
pub fn find(override_path: Option<&Path>, candidates: Vec<PathBuf>) -> Option<PathBuf> {
    if let Some(p) = override_path {
        return p.is_file().then(|| p.to_path_buf());
    }
    candidates.into_iter().find(|p| p.is_file())
}

/// Runs `<bin> <args>` with a short time limit and returns trimmed stdout.
/// `label` names the tool in errors ("Claude Code", "Codex").
pub async fn quick(bin: &Path, args: &[&str], label: &str) -> Result<String, ProviderError> {
    let out = tokio::time::timeout(
        Duration::from_secs(15),
        tokio::process::Command::new(bin)
            .args(args)
            .stdin(Stdio::null())
            .kill_on_drop(true)
            .output(),
    )
    .await
    .map_err(|_| ProviderError::Unavailable(format!("{label} didn't respond")))?
    .map_err(|e| ProviderError::Unavailable(format!("{label} couldn't be started: {e}")))?;
    if !out.status.success() {
        let err = String::from_utf8_lossy(&out.stderr);
        let first = err
            .lines()
            .find(|l| !l.trim().is_empty())
            .unwrap_or("")
            .trim();
        return Err(ProviderError::Unavailable(if first.is_empty() {
            format!("{label} is installed but didn't run correctly")
        } else {
            format!("{label} is installed but didn't run correctly: {first}")
        }));
    }
    Ok(String::from_utf8_lossy(&out.stdout).trim().to_string())
}

/// Runs one request: writes `input` to stdin, waits for exit within
/// `timeout`, and ends the process if `cancel` fires. Runs in an empty
/// scratch directory so the tool sees no project files.
pub async fn run(
    bin: &Path,
    args: &[String],
    input: &str,
    timeout: Duration,
    cancel: &CancellationToken,
    label: &str,
) -> Result<Output, ProviderError> {
    let scratch = tempfile::tempdir().map_err(|e| ProviderError::from_io(&e))?;
    let mut child = tokio::process::Command::new(bin)
        .args(args)
        .current_dir(scratch.path())
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        // Cancelling drops the future, which kills the process.
        .kill_on_drop(true)
        .spawn()
        .map_err(|e| ProviderError::Unavailable(format!("{label} couldn't be started: {e}")))?;
    let mut stdin = child.stdin.take().expect("piped stdin");
    let input = input.to_string();
    let run = async move {
        stdin
            .write_all(input.as_bytes())
            .await
            .map_err(|e| ProviderError::Io(e.to_string()))?;
        drop(stdin);
        child
            .wait_with_output()
            .await
            .map_err(|e| ProviderError::Io(e.to_string()))
    };
    tokio::select! {
        r = tokio::time::timeout(timeout, run) => r.map_err(|_| {
            ProviderError::ProviderFailed(format!("{label} didn't finish within the time limit"))
        })?,
        _ = cancel.cancelled() => Err(ProviderError::Cancelled),
    }
}
