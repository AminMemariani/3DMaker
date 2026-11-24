//! Language-model providers behind one small interface.
//!
//! Every provider does the same thing: given the design instructions and the
//! user's description, return the model's text answer. Turning that answer
//! into a splat happens locally (see [`crate::design`]).

pub mod anthropic;
pub mod claude_code;
pub mod cli;
pub mod codex;
pub mod openai;

use crate::errors::ProviderError;
use crate::model::ProviderId;
use async_trait::async_trait;
use std::path::PathBuf;
use std::time::Duration;
use tokio_util::sync::CancellationToken;

/// Base URLs and local paths. Production values by default; tests and the
/// e2e build point them at a loopback fake server.
#[derive(Debug, Clone)]
pub struct Endpoints {
    pub anthropic_api: String,
    /// OpenAI API base including the `/v1` path.
    pub openai_api: String,
    /// Ollama server root (its OpenAI-compatible API lives under `/v1`).
    pub ollama_api: String,
    /// Use this Claude Code executable instead of searching for one.
    pub claude_cli: Option<PathBuf>,
    /// Use this Codex executable instead of searching for one.
    pub codex_cli: Option<PathBuf>,
    /// Permit plain HTTP to loopback for the cloud presets (tests only).
    pub allow_loopback_http: bool,
    /// Time limit for one design request. Models can take a while to write
    /// a detailed design, so this is generous.
    pub request_timeout: Duration,
}

impl Default for Endpoints {
    fn default() -> Self {
        Self {
            anthropic_api: "https://api.anthropic.com".into(),
            openai_api: "https://api.openai.com/v1".into(),
            ollama_api: "http://127.0.0.1:11434".into(),
            claude_cli: None,
            codex_cli: None,
            allow_loopback_http: false,
            request_timeout: Duration::from_secs(300),
        }
    }
}

impl Endpoints {
    /// All HTTP presets served by one local fake server (see `fake-provider`).
    pub fn local_fake(base: &str) -> Self {
        let b = base.trim_end_matches('/');
        Self {
            anthropic_api: format!("{b}/anthropic"),
            openai_api: format!("{b}/openai/v1"),
            ollama_api: format!("{b}/ollama"),
            claude_cli: None,
            codex_cli: None,
            allow_loopback_http: true,
            request_timeout: Duration::from_secs(30),
        }
    }
}

/// Result of a free connection check.
#[derive(Debug, Clone, Default)]
pub struct Verified {
    pub account_label: Option<String>,
    /// Models the user can pick from, best default first.
    pub models: Vec<String>,
}

impl Verified {
    pub fn default_model(&self) -> Option<&str> {
        self.models.first().map(String::as_str)
    }
}

#[async_trait]
pub trait Provider: Send + Sync {
    fn id(&self) -> ProviderId;
    /// The model this provider will use (for the object's record).
    fn model(&self) -> String;
    /// Non-billable check that the provider is reachable and the key works.
    async fn verify(&self) -> Result<Verified, ProviderError>;
    /// One design request. Billable on cloud providers.
    async fn complete(
        &self,
        system: &str,
        user: &str,
        cancel: &CancellationToken,
    ) -> Result<String, ProviderError>;
}

/// Room for a detailed design (dozens of parts) without truncation.
pub const MAX_OUTPUT_TOKENS: u32 = 8192;

/// Normalizes a user-entered OpenAI-compatible base URL: trims, drops a
/// trailing slash and a pasted `/chat/completions`, and appends `/v1` when no
/// path was given.
pub fn normalize_base(input: &str, allow_loopback_http: bool) -> Result<String, ProviderError> {
    let t = input.trim().trim_end_matches('/');
    let t = t
        .trim_end_matches("/chat/completions")
        .trim_end_matches('/');
    if t.is_empty() {
        return Err(ProviderError::Policy("enter the server's base URL".into()));
    }
    let mut u = url::Url::parse(t).map_err(|e| ProviderError::Policy(format!("bad URL: {e}")))?;
    crate::net::check_transport(&u, allow_loopback_http)?;
    if !u.username().is_empty() || u.password().is_some() {
        return Err(ProviderError::Policy(
            "put the access key in the key field, not in the URL".into(),
        ));
    }
    u.set_query(None);
    u.set_fragment(None);
    if u.path() == "/" || u.path().is_empty() {
        u.set_path("/v1");
    }
    Ok(u.as_str().trim_end_matches('/').to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn base_url_normalization() {
        assert_eq!(
            normalize_base(" https://openrouter.ai/api/v1/ ", false).unwrap(),
            "https://openrouter.ai/api/v1"
        );
        assert_eq!(
            normalize_base("https://example.com", false).unwrap(),
            "https://example.com/v1"
        );
        assert_eq!(
            normalize_base("http://localhost:1234/v1/chat/completions", true).unwrap(),
            "http://localhost:1234/v1"
        );
        assert!(normalize_base("http://example.com/v1", true).is_err());
        assert!(normalize_base("http://localhost:1234", false).is_err());
        assert!(normalize_base("https://user:pw@example.com", false).is_err());
        assert!(normalize_base("", false).is_err());
    }
}
