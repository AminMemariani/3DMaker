//! Wire types shared with the UI (mirrors `src/lib/ipc.ts`, camelCase JSON).

use crate::ply::Bounds;
use serde::{Deserialize, Serialize};

/// Where an object's design comes from. Cloud and custom providers need the
/// user's key; local ones (Claude Code, Codex, Ollama) are found on this computer.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum ProviderId {
    #[serde(rename = "anthropic")]
    Anthropic,
    #[serde(rename = "openai")]
    OpenAi,
    #[serde(rename = "custom")]
    Custom,
    #[serde(rename = "claude-code")]
    ClaudeCode,
    #[serde(rename = "codex")]
    Codex,
    #[serde(rename = "ollama")]
    Ollama,
}

impl ProviderId {
    pub const ALL: [ProviderId; 6] = [
        Self::Anthropic,
        Self::OpenAi,
        Self::Custom,
        Self::ClaudeCode,
        Self::Codex,
        Self::Ollama,
    ];
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Anthropic => "anthropic",
            Self::OpenAi => "openai",
            Self::Custom => "custom",
            Self::ClaudeCode => "claude-code",
            Self::Codex => "codex",
            Self::Ollama => "ollama",
        }
    }
    pub fn parse(s: &str) -> Option<Self> {
        Self::ALL.into_iter().find(|p| p.as_str() == s)
    }
    pub fn display_name(self) -> &'static str {
        match self {
            Self::Anthropic => "Anthropic",
            Self::OpenAi => "OpenAI",
            Self::Custom => "Custom provider",
            Self::ClaudeCode => "Claude Code",
            Self::Codex => "Codex",
            Self::Ollama => "Ollama",
        }
    }
    pub fn kind(self) -> ProviderKind {
        match self {
            Self::Anthropic | Self::OpenAi => ProviderKind::Cloud,
            Self::Custom => ProviderKind::Custom,
            Self::ClaudeCode | Self::Codex | Self::Ollama => ProviderKind::Local,
        }
    }
    /// Whether requests can cost the user money per call.
    pub fn bills_per_call(self) -> bool {
        matches!(self, Self::Anthropic | Self::OpenAi | Self::Custom)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum ProviderKind {
    Cloud,
    Custom,
    Local,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ProviderInfo {
    pub id: ProviderId,
    pub name: String,
    pub kind: ProviderKind,
    pub key_url: Option<String>,
    /// `None` when the provider needs no key.
    pub key_label: Option<String>,
    pub key_required: bool,
    pub verify_note: String,
    pub flow_summary: String,
    pub billing_note: String,
}

/// What auto-detection found for a local provider.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct LocalDetection {
    pub id: ProviderId,
    /// Ready to use right now.
    pub available: bool,
    /// Installed, even if not currently usable (e.g. Ollama not running).
    pub installed: bool,
    /// Version or path shown to the user.
    pub detail: Option<String>,
    pub models: Vec<String>,
    /// What to do when it isn't available.
    pub hint: Option<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum CredentialState {
    Missing,
    Stored,
    Session,
    NotRequired,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ProviderStatus {
    pub id: ProviderId,
    pub connected: bool,
    pub credential: CredentialState,
    pub masked_key: Option<String>,
    pub account_label: Option<String>,
    pub endpoint: Option<String>,
    pub model: Option<String>,
    pub models: Vec<String>,
    pub verified_at: Option<i64>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum ProjectState {
    Queued,
    Designing,
    Building,
    Saving,
    Ready,
    Failed,
    Cancelled,
    NeedsAttention,
}

impl ProjectState {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Queued => "queued",
            Self::Designing => "designing",
            Self::Building => "building",
            Self::Saving => "saving",
            Self::Ready => "ready",
            Self::Failed => "failed",
            Self::Cancelled => "cancelled",
            Self::NeedsAttention => "needsAttention",
        }
    }
    /// Unknown values (including states written by older versions) read as
    /// "needs attention" so they are never mistaken for finished objects.
    pub fn parse(s: &str) -> Self {
        match s {
            "queued" => Self::Queued,
            "designing" => Self::Designing,
            "building" => Self::Building,
            "saving" => Self::Saving,
            "ready" => Self::Ready,
            "failed" => Self::Failed,
            "cancelled" => Self::Cancelled,
            _ => Self::NeedsAttention,
        }
    }
    pub fn is_terminal(self) -> bool {
        matches!(
            self,
            Self::Ready | Self::Failed | Self::Cancelled | Self::NeedsAttention
        )
    }
}

/// The billable step of a generation, recorded with its model and attempt.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum StageKind {
    Design,
}

impl StageKind {
    pub fn as_str(self) -> &'static str {
        "design"
    }
    pub fn parse(s: &str) -> Option<Self> {
        (s == "design").then_some(Self::Design)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum ErrorKind {
    InvalidKey,
    NoCredit,
    RateLimited,
    Network,
    Unavailable,
    ProviderFailed,
    InvalidDesign,
    InvalidAsset,
    DiskFull,
    UnknownOutcome,
    Cancelled,
    Busy,
    Internal,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum NextAction {
    ReplaceKey,
    AddCredit,
    RetryLater,
    /// Ask the model again (may be billed).
    RetryAll,
    /// Rebuild the saved design locally (free).
    Rebuild,
    CheckConnection,
    /// Start or install a local provider.
    StartLocal,
    FreeSpace,
    None,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct UserError {
    pub kind: ErrorKind,
    pub title: String,
    pub message: String,
    pub action: NextAction,
    pub action_url: Option<String>,
}

impl UserError {
    pub fn new(
        kind: ErrorKind,
        title: impl Into<String>,
        message: impl Into<String>,
        action: NextAction,
    ) -> Self {
        Self {
            kind,
            title: title.into(),
            message: message.into(),
            action,
            action_url: None,
        }
    }
    pub fn busy() -> Self {
        Self::new(
            ErrorKind::Busy,
            "Already generating",
            "An object is already being generated. Wait for it to finish or cancel it first.",
            NextAction::None,
        )
    }
    pub fn internal(msg: impl Into<String>) -> Self {
        Self::new(
            ErrorKind::Internal,
            "Something went wrong",
            msg,
            NextAction::None,
        )
    }
}

impl std::fmt::Display for UserError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}: {}", self.title, self.message)
    }
}

impl std::error::Error for UserError {}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum CancelOutcome {
    /// Stopped here; a cloud provider may still bill for work already done.
    LocalOnly,
    /// Stopped, and nothing keeps running anywhere (local provider or local build).
    Confirmed,
    AlreadyFinished,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CancelReport {
    pub outcome: CancelOutcome,
    pub message: String,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct JobView {
    pub project_id: String,
    pub state: ProjectState,
    pub label: String,
    pub error: Option<UserError>,
    pub cancel: Option<CancelReport>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ProjectSummary {
    pub id: String,
    pub prompt: String,
    pub provider: String,
    pub state: ProjectState,
    pub created_at: i64,
    pub thumb_path: Option<String>,
    pub splat_count: Option<u64>,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, Default, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct RetryOptions {
    /// The design is saved: rebuild it locally without asking the model again.
    pub rebuild: bool,
    /// Ask the model for a new design.
    pub full: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ProjectDetail {
    #[serde(flatten)]
    pub summary: ProjectSummary,
    pub splat_path: Option<String>,
    pub splat_bytes: Option<u64>,
    pub splat_sha256: Option<String>,
    pub sh_degree: Option<u8>,
    pub bounds: Option<Bounds>,
    pub view_rotation: [f32; 3],
    /// Model that produced the design, when known.
    pub model: Option<String>,
    pub part_count: Option<usize>,
    pub error: Option<UserError>,
    pub retry: RetryOptions,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct GenerateRequest {
    pub prompt: String,
    pub provider: ProviderId,
    pub client_request_id: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum RetryAction {
    Rebuild,
    Full,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ExportResult {
    pub saved: bool,
    pub path: Option<String>,
    pub bytes: Option<u64>,
    pub sha256: Option<String>,
}

pub fn now_unix() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs() as i64)
        .unwrap_or(0)
}
