//! OpenAI Codex CLI as a local provider. Uses the user's existing Codex
//! login (ChatGPT plan or API key), so no key is handled by 3D Maker.
//!
//! Runs `codex exec` non-interactively in a read-only sandbox, without saving
//! a session, without the user's config (so no MCP servers start), in an
//! empty scratch directory, with the prompt on stdin. Codex prints only its
//! final message to stdout.

use super::{Provider, Verified, cli};
use crate::errors::ProviderError;
use crate::model::ProviderId;
use async_trait::async_trait;
use std::path::{Path, PathBuf};
use std::time::Duration;
use tokio_util::sync::CancellationToken;

/// Separates the design instructions from the user's request in the single
/// prompt Codex receives (it has no separate system-prompt flag).
pub const PROMPT_SEPARATOR: &str = "\n\n=== REQUEST ===\n\n";

pub struct Codex {
    bin: PathBuf,
    model: Option<String>,
    timeout: Duration,
}

/// Finds the Codex executable, preferring an explicit override. Also checks
/// the copy bundled inside the Codex desktop app.
pub fn find(override_path: Option<&Path>) -> Option<PathBuf> {
    let mut extra = Vec::new();
    if cfg!(target_os = "macos") {
        extra.push(PathBuf::from(
            "/Applications/Codex.app/Contents/Resources/codex",
        ));
        if let Some(h) = std::env::var_os("HOME") {
            extra.push(PathBuf::from(h).join("Applications/Codex.app/Contents/Resources/codex"));
        }
    }
    cli::find(override_path, cli::candidates("codex", &extra))
}

/// `codex --version`, e.g. "codex-cli 0.98.0".
pub async fn version(bin: &Path) -> Result<String, ProviderError> {
    cli::quick(bin, &["--version"], "Codex").await
}

/// Version number without the "codex-cli" prefix.
pub fn short_version(v: &str) -> &str {
    v.trim().trim_start_matches("codex-cli").trim()
}

impl Codex {
    pub fn new(bin: PathBuf, model: Option<String>, timeout: Duration) -> Self {
        Self {
            bin,
            model: model.filter(|m| !m.trim().is_empty() && m != "default"),
            timeout,
        }
    }

    pub fn args(&self) -> Vec<String> {
        let mut a: Vec<String> = [
            "exec",
            "--ephemeral",
            "--skip-git-repo-check",
            "--sandbox",
            "read-only",
            "--ignore-user-config",
            "--color",
            "never",
        ]
        .iter()
        .map(|s| s.to_string())
        .collect();
        if let Some(m) = &self.model {
            a.push("--model".into());
            a.push(m.clone());
        }
        // Read the whole prompt from stdin.
        a.push("-".into());
        a
    }
}

#[async_trait]
impl Provider for Codex {
    fn id(&self) -> ProviderId {
        ProviderId::Codex
    }
    fn model(&self) -> String {
        self.model.clone().unwrap_or_else(|| "Codex default".into())
    }

    async fn verify(&self) -> Result<Verified, ProviderError> {
        let v = version(&self.bin).await?;
        // `codex login status` exits non-zero when nobody is signed in.
        if cli::quick(&self.bin, &["login", "status"], "Codex")
            .await
            .is_err()
        {
            return Err(ProviderError::InvalidCredentials);
        }
        Ok(Verified {
            account_label: Some(format!("Codex {}", short_version(&v))),
            models: vec!["default".to_string()],
        })
    }

    async fn complete(
        &self,
        system: &str,
        user: &str,
        cancel: &CancellationToken,
    ) -> Result<String, ProviderError> {
        let prompt = format!("{system}{PROMPT_SEPARATOR}{user}");
        let out = cli::run(
            &self.bin,
            &self.args(),
            &prompt,
            self.timeout,
            cancel,
            "Codex",
        )
        .await?;
        parse_output(&out.stdout, &out.stderr, out.status.success())
    }
}

fn parse_output(stdout: &[u8], stderr: &[u8], ok: bool) -> Result<String, ProviderError> {
    let text = String::from_utf8_lossy(stdout).trim().to_string();
    if ok && !text.is_empty() {
        return Ok(text);
    }
    let err = String::from_utf8_lossy(stderr);
    // Codex streams progress on stderr; the cause is usually near the end.
    let detail = err
        .lines()
        .rev()
        .map(str::trim)
        .find(|l| !l.is_empty())
        .unwrap_or("")
        .to_string();
    if ok {
        return Err(ProviderError::BadResponse("Codex returned no text".into()));
    }
    Err(classify(&err, &detail))
}

fn classify(all: &str, detail: &str) -> ProviderError {
    let l = all.to_ascii_lowercase();
    if l.contains("not logged in")
        || l.contains("codex login")
        || l.contains("401")
        || l.contains("unauthorized")
    {
        ProviderError::InvalidCredentials
    } else if l.contains("rate limit") || l.contains("usage limit") || l.contains("429") {
        ProviderError::RateLimited { retry_after: None }
    } else if l.contains("unexpected argument") || l.contains("unrecognized") {
        ProviderError::Unavailable(
            "This Codex version is too old for 3D Maker. Update it (for example `npm install -g @openai/codex`) and try again.".into(),
        )
    } else {
        ProviderError::ProviderFailed(if detail.is_empty() {
            "Codex exited without an answer".into()
        } else {
            detail.to_string()
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn args_are_read_only_ephemeral_and_read_stdin() {
        let c = Codex::new("codex".into(), Some("gpt-5".into()), Duration::from_secs(1));
        let a = c.args();
        assert_eq!(a[0], "exec");
        let i = a.iter().position(|x| x == "--sandbox").unwrap();
        assert_eq!(a[i + 1], "read-only");
        for f in [
            "--ephemeral",
            "--skip-git-repo-check",
            "--ignore-user-config",
        ] {
            assert!(a.contains(&f.to_string()), "{f}");
        }
        assert_eq!(
            a[a.len() - 3..],
            ["--model".to_string(), "gpt-5".to_string(), "-".to_string()]
        );
        let d = Codex::new(
            "codex".into(),
            Some("default".into()),
            Duration::from_secs(1),
        );
        assert!(!d.args().contains(&"--model".to_string()));
    }

    #[test]
    fn output_parsing() {
        assert_eq!(
            parse_output(b"{\"parts\":[]}\n", b"progress", true).unwrap(),
            "{\"parts\":[]}"
        );
        assert_eq!(
            parse_output(b"", b"Error: Not logged in. Run `codex login`.", false),
            Err(ProviderError::InvalidCredentials)
        );
        assert!(matches!(
            parse_output(
                b"",
                b"error: unexpected argument '--ephemeral' found",
                false
            ),
            Err(ProviderError::Unavailable(_))
        ));
        assert!(
            matches!(parse_output(b"", b"step\nboom", false), Err(ProviderError::ProviderFailed(m)) if m == "boom")
        );
        assert_eq!(short_version("codex-cli 0.98.0"), "0.98.0");
    }
}
