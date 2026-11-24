//! User-facing provider descriptions shown in Connect and Settings.

use crate::model::{ProviderId, ProviderInfo};

const FLOW: &str = "The model designs your object as a set of coloured shapes; 3D Maker turns that design into a Gaussian splat on this computer.";

pub fn providers() -> Vec<ProviderInfo> {
    ProviderId::ALL.into_iter().map(info).collect()
}

pub fn info(id: ProviderId) -> ProviderInfo {
    let (key_url, key_label, key_required, verify_note, billing_note) = match id {
        ProviderId::Anthropic => (
            Some("https://console.anthropic.com/settings/keys"),
            Some("API key"),
            true,
            "Connect checks the key with a free model-list request.",
            "Anthropic bills your account for one model request per object.",
        ),
        ProviderId::OpenAi => (
            Some("https://platform.openai.com/api-keys"),
            Some("API key"),
            true,
            "Connect checks the key with a free model-list request.",
            "OpenAI bills your account for one model request per object.",
        ),
        ProviderId::Custom => (
            None,
            Some("API key (optional)"),
            false,
            "Any server with an OpenAI-compatible Chat Completions API (OpenRouter, LM Studio, vLLM, …). Connect reads its model list.",
            "Any charges are set by whoever runs the server.",
        ),
        ProviderId::ClaudeCode => (
            None,
            None,
            false,
            "Uses the Claude Code app already installed and signed in on this computer. No key is needed.",
            "Requests count against your Claude plan's usage limits.",
        ),
        ProviderId::Codex => (
            None,
            None,
            false,
            "Uses the OpenAI Codex CLI already installed and signed in on this computer. No key is needed.",
            "Requests count against your ChatGPT plan's Codex limits, or your OpenAI account if Codex signs in with a key.",
        ),
        ProviderId::Ollama => (
            None,
            None,
            false,
            "Uses models running in Ollama on this computer. Works fully offline.",
            "Free: everything runs on your own hardware.",
        ),
    };
    ProviderInfo {
        id,
        name: id.display_name().into(),
        kind: id.kind(),
        key_url: key_url.map(String::from),
        key_label: key_label.map(String::from),
        key_required,
        verify_note: verify_note.into(),
        flow_summary: FLOW.into(),
        billing_note: billing_note.into(),
    }
}
