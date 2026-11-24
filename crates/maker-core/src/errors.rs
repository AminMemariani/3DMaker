//! Provider-level errors and their translation into plain-language UI errors.

use crate::model::{ErrorKind, NextAction, ProviderId, UserError};
use std::time::Duration;

#[derive(Debug, Clone, thiserror::Error, PartialEq)]
pub enum ProviderError {
    #[error("credentials rejected")]
    InvalidCredentials,
    #[error("insufficient credit")]
    NoCredit,
    #[error("rate limited")]
    RateLimited { retry_after: Option<Duration> },
    #[error("network error: {0}")]
    Network(String),
    /// A request may or may not have been processed (connection lost after sending).
    #[error("request outcome unknown: {0}")]
    UnknownOutcome(String),
    /// A local provider isn't installed, running or signed in.
    #[error("not available: {0}")]
    Unavailable(String),
    #[error("model not found: {0}")]
    ModelNotFound(String),
    #[error("provider failed: {0}")]
    ProviderFailed(String),
    #[error("unexpected response: {0}")]
    BadResponse(String),
    #[error("blocked by network policy: {0}")]
    Policy(String),
    #[error("disk full")]
    DiskFull,
    #[error("file error: {0}")]
    Io(String),
    #[error("cancelled")]
    Cancelled,
}

impl ProviderError {
    /// Whether retrying the same *safe* (non-billable) operation may help.
    pub fn is_transient(&self) -> bool {
        matches!(self, Self::Network(_) | Self::RateLimited { .. })
    }

    pub fn from_io(e: &std::io::Error) -> Self {
        if is_disk_full(e) {
            Self::DiskFull
        } else {
            Self::Io(e.to_string())
        }
    }

    pub fn to_user(&self, provider: ProviderId) -> UserError {
        let name = provider.display_name();
        let billing_url = match provider {
            ProviderId::Anthropic => Some("https://console.anthropic.com/settings/billing"),
            ProviderId::OpenAi => Some("https://platform.openai.com/settings/organization/billing"),
            _ => None,
        };
        let billed = if provider.bills_per_call() {
            " Nothing new is charged until you click a retry button."
        } else {
            ""
        };
        let mut e = match self {
            Self::InvalidCredentials if provider == ProviderId::ClaudeCode => UserError::new(
                ErrorKind::InvalidKey,
                "Claude Code isn't signed in",
                "Open a terminal, run `claude`, and sign in. Then try again.",
                NextAction::StartLocal,
            ),
            Self::InvalidCredentials if provider == ProviderId::Codex => UserError::new(
                ErrorKind::InvalidKey,
                "Codex isn't signed in",
                "Open a terminal, run `codex login`, and sign in. Then try again.",
                NextAction::StartLocal,
            ),
            Self::InvalidCredentials => UserError::new(
                ErrorKind::InvalidKey,
                format!("{name} rejected your key"),
                format!(
                    "The key may be mistyped, revoked or expired. Paste a current key from your {name} account in Settings."
                ),
                NextAction::ReplaceKey,
            ),
            Self::NoCredit => {
                let mut u = UserError::new(
                    ErrorKind::NoCredit,
                    format!("Your {name} account is out of credit"),
                    format!(
                        "{name} declined the request for billing reasons. Add credit or a payment method, then try again."
                    ),
                    NextAction::AddCredit,
                );
                u.action_url = billing_url.map(String::from);
                u
            }
            Self::RateLimited { retry_after } => UserError::new(
                ErrorKind::RateLimited,
                format!("{name} is limiting requests"),
                match retry_after {
                    Some(d) => format!(
                        "Too many requests right now. Try again in about {} seconds.",
                        d.as_secs().max(1)
                    ),
                    None => "Too many requests right now. Wait a minute and try again.".into(),
                },
                NextAction::RetryLater,
            ),
            Self::Network(detail) => UserError::new(
                ErrorKind::Network,
                format!("Couldn't reach {name}"),
                format!(
                    "Check the connection, then retry.{billed} Details: {}",
                    short(detail)
                ),
                NextAction::CheckConnection,
            ),
            Self::UnknownOutcome(detail) => UserError::new(
                ErrorKind::UnknownOutcome,
                "The answer was lost on the way",
                format!(
                    "The connection dropped after the request was sent, so {name} may have processed{} it. 3D Maker won't ask again on its own; retry when you're ready. Details: {}",
                    if provider.bills_per_call() {
                        " (and billed)"
                    } else {
                        ""
                    },
                    short(detail)
                ),
                NextAction::RetryAll,
            ),
            Self::Unavailable(msg) => UserError::new(
                ErrorKind::Unavailable,
                format!("{name} isn't available"),
                short(msg),
                NextAction::StartLocal,
            ),
            Self::ModelNotFound(model) => UserError::new(
                ErrorKind::ProviderFailed,
                format!("{name} doesn't have that model"),
                format!(
                    "The model \u{201c}{}\u{201d} isn't available on this account or server. Pick another model in Settings.",
                    short(model)
                ),
                NextAction::ReplaceKey,
            ),
            Self::ProviderFailed(msg) => UserError::new(
                ErrorKind::ProviderFailed,
                format!("{name} reported a failure"),
                format!("{name} said: {}", short(msg)),
                NextAction::RetryAll,
            ),
            Self::BadResponse(msg) => UserError::new(
                ErrorKind::ProviderFailed,
                format!("{name} sent an unexpected response"),
                format!("{}.{billed}", short(msg).trim_end_matches('.')),
                NextAction::RetryAll,
            ),
            Self::Policy(msg) => UserError::new(
                ErrorKind::Internal,
                "Blocked an unsafe connection",
                format!(
                    "For your security the app refused a request: {}",
                    short(msg)
                ),
                NextAction::None,
            ),
            Self::DiskFull => UserError::new(
                ErrorKind::DiskFull,
                "Your disk is full",
                "Free some disk space, then rebuild. The design is kept, so nothing is asked of the model again.",
                NextAction::FreeSpace,
            ),
            Self::Io(msg) => UserError::new(
                ErrorKind::Internal,
                "Couldn't save the file",
                short(msg),
                NextAction::Rebuild,
            ),
            Self::Cancelled => UserError::new(
                ErrorKind::Cancelled,
                "Cancelled",
                "Generation was cancelled.",
                NextAction::None,
            ),
        };
        e.message = crate::redact::redact(&e.message);
        e
    }
}

pub fn short(s: &str) -> String {
    let s = crate::redact::redact(s.trim());
    if s.chars().count() > 280 {
        format!("{}…", s.chars().take(280).collect::<String>())
    } else {
        s
    }
}

pub fn is_disk_full(e: &std::io::Error) -> bool {
    if e.kind() == std::io::ErrorKind::StorageFull {
        return true;
    }
    // ENOSPC (unix) / ERROR_DISK_FULL, ERROR_HANDLE_DISK_FULL (windows)
    matches!(e.raw_os_error(), Some(28) | Some(112) | Some(39))
}
