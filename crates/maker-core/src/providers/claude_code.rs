//! Claude Code CLI as a local provider. Uses the user's existing Claude
//! login, so no key is handled by 3D Maker.
//!
//! The CLI runs in print mode with every tool disabled, no MCP servers, no
//! saved session, and an empty scratch directory as its working directory,
//! so it can only answer with text.

use super::{Provider, Verified, cli};
use crate::errors::ProviderError;
use crate::model::ProviderId;
use async_trait::async_trait;
use serde_json::Value;
use std::path::{Path, PathBuf};
use std::time::Duration;
use tokio_util::sync::CancellationToken;

/// Model aliases accepted by `--model`. Empty means the CLI's own default.
pub const MODEL_ALIASES: [&str; 3] = ["sonnet", "opus", "haiku"];

pub struct ClaudeCode {
    bin: PathBuf,
    model: Option<String>,
    timeout: Duration,
}

/// Finds the Claude Code executable, preferring an explicit override.
pub fn find(override_path: Option<&Path>) -> Option<PathBuf> {
    let extra: Vec<PathBuf> = std::env::var_os(if cfg!(windows) { "USERPROFILE" } else { "HOME" })
        .map(|h| {
            let exe = if cfg!(windows) {
                "claude.exe"
            } else {
                "claude"
            };
            vec![PathBuf::from(h).join(".claude/local").join(exe)]
        })
        .unwrap_or_default();
    cli::find(override_path, cli::candidates("claude", &extra))
}

/// `claude --version`, e.g. "2.1.314 (Claude Code)".
pub async fn version(bin: &Path) -> Result<String, ProviderError> {
    cli::quick(bin, &["--version"], "Claude Code").await
}

impl ClaudeCode {
    pub fn new(bin: PathBuf, model: Option<String>, timeout: Duration) -> Self {
        Self {
            bin,
            model: model.filter(|m| !m.trim().is_empty() && m != "default"),
            timeout,
        }
    }

    pub fn args(&self, system: &str) -> Vec<String> {
        let mut a: Vec<String> = [
            "-p",
            "--output-format",
            "json",
            "--no-session-persistence",
            "--strict-mcp-config",
            "--system-prompt",
            system,
            "--tools",
            "",
        ]
        .iter()
        .map(|s| s.to_string())
        .collect();
        if let Some(m) = &self.model {
            a.push("--model".into());
            a.push(m.clone());
        }
        a
    }
}

#[async_trait]
impl Provider for ClaudeCode {
    fn id(&self) -> ProviderId {
        ProviderId::ClaudeCode
    }
    fn model(&self) -> String {
        self.model
            .clone()
            .unwrap_or_else(|| "Claude Code default".into())
    }

    async fn verify(&self) -> Result<Verified, ProviderError> {
        let v = version(&self.bin).await?;
        let mut models = vec!["default".to_string()];
        models.extend(MODEL_ALIASES.iter().map(|s| s.to_string()));
        Ok(Verified {
            account_label: Some(format!(
                "Claude Code {}",
                v.trim_end_matches(" (Claude Code)")
            )),
            models,
        })
    }

    async fn complete(
        &self,
        system: &str,
        user: &str,
        cancel: &CancellationToken,
    ) -> Result<String, ProviderError> {
        let out = cli::run(
            &self.bin,
            &self.args(system),
            user,
            self.timeout,
            cancel,
            "Claude Code",
        )
        .await?;
        parse_output(&out.stdout, &out.stderr, out.status.success())
    }
}

fn parse_output(stdout: &[u8], stderr: &[u8], ok: bool) -> Result<String, ProviderError> {
    let text = String::from_utf8_lossy(stdout);
    let err = String::from_utf8_lossy(stderr);
    let v: Option<Value> = serde_json::from_str(text.trim()).ok();
    let Some(v) = v else {
        let detail = if err.trim().is_empty() {
            text.trim()
        } else {
            err.trim()
        };
        return Err(classify(detail, ok));
    };
    let result = v["result"].as_str().unwrap_or_default().to_string();
    if v["is_error"].as_bool().unwrap_or(false) || v["subtype"] != "success" {
        let detail = if result.is_empty() {
            v["subtype"].as_str().unwrap_or("unknown error").to_string()
        } else {
            result
        };
        return Err(classify(&detail, false));
    }
    if result.trim().is_empty() {
        return Err(ProviderError::BadResponse(
            "Claude Code returned no text".into(),
        ));
    }
    Ok(result)
}

fn classify(detail: &str, _ok: bool) -> ProviderError {
    let l = detail.to_ascii_lowercase();
    if l.contains("log in")
        || l.contains("login")
        || l.contains("not authenticated")
        || l.contains("invalid api key")
    {
        ProviderError::InvalidCredentials
    } else if l.contains("rate limit") || l.contains("usage limit") {
        ProviderError::RateLimited { retry_after: None }
    } else {
        ProviderError::ProviderFailed(if detail.is_empty() {
            "Claude Code exited without an answer".into()
        } else {
            detail.to_string()
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn args_disable_tools_and_sessions() {
        let c = ClaudeCode::new(
            "claude".into(),
            Some("sonnet".into()),
            Duration::from_secs(1),
        );
        let a = c.args("SYS");
        let i = a.iter().position(|x| x == "--tools").unwrap();
        assert_eq!(a[i + 1], "");
        assert!(a.contains(&"--no-session-persistence".to_string()));
        assert!(a.contains(&"--strict-mcp-config".to_string()));
        assert_eq!(
            a[a.len() - 2..],
            ["--model".to_string(), "sonnet".to_string()]
        );
        let d = ClaudeCode::new(
            "claude".into(),
            Some("default".into()),
            Duration::from_secs(1),
        );
        assert!(!d.args("S").contains(&"--model".to_string()));
    }

    #[test]
    fn output_parsing() {
        let ok =
            br#"{"type":"result","subtype":"success","is_error":false,"result":"{\"parts\":[]}"}"#;
        assert_eq!(parse_output(ok, b"", true).unwrap(), "{\"parts\":[]}");
        let auth = br#"{"type":"result","subtype":"success","is_error":true,"result":"Invalid API key \u00b7 Please run /login"}"#;
        assert_eq!(
            parse_output(auth, b"", true),
            Err(ProviderError::InvalidCredentials)
        );
        assert!(
            matches!(parse_output(b"", b"boom", false), Err(ProviderError::ProviderFailed(m)) if m == "boom")
        );
    }
}
