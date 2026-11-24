//! Finds local language-model providers on this computer: the Claude Code
//! and Codex CLIs and a running Ollama server. Detection is read-only and
//! quick; it never sends a prompt.

use crate::model::{LocalDetection, ProviderId};
use crate::providers::{Endpoints, claude_code, codex, openai};
use std::time::Duration;

pub async fn codex(endpoints: &Endpoints) -> LocalDetection {
    let Some(bin) = codex::find(endpoints.codex_cli.as_deref()) else {
        return LocalDetection {
            id: ProviderId::Codex,
            available: false,
            installed: false,
            detail: None,
            models: vec![],
            hint: Some("Install the Codex CLI (`npm install -g @openai/codex`) and sign in once with `codex login`.".into()),
        };
    };
    match codex::version(&bin).await {
        Ok(v) => LocalDetection {
            id: ProviderId::Codex,
            available: true,
            installed: true,
            detail: Some(format!("{} at {}", codex::short_version(&v), bin.display())),
            models: vec!["default".to_string()],
            hint: None,
        },
        Err(e) => LocalDetection {
            id: ProviderId::Codex,
            available: false,
            installed: true,
            detail: Some(bin.display().to_string()),
            models: vec![],
            hint: Some(e.to_string()),
        },
    }
}

pub async fn claude_code(endpoints: &Endpoints) -> LocalDetection {
    let Some(bin) = claude_code::find(endpoints.claude_cli.as_deref()) else {
        return LocalDetection {
            id: ProviderId::ClaudeCode,
            available: false,
            installed: false,
            detail: None,
            models: vec![],
            hint: Some(
                "Install Claude Code from claude.com/code and sign in once with `claude`.".into(),
            ),
        };
    };
    match claude_code::version(&bin).await {
        Ok(v) => {
            let mut models = vec!["default".to_string()];
            models.extend(claude_code::MODEL_ALIASES.iter().map(|s| s.to_string()));
            LocalDetection {
                id: ProviderId::ClaudeCode,
                available: true,
                installed: true,
                detail: Some(format!(
                    "{} at {}",
                    v.trim_end_matches(" (Claude Code)"),
                    bin.display()
                )),
                models,
                hint: None,
            }
        }
        Err(e) => LocalDetection {
            id: ProviderId::ClaudeCode,
            available: false,
            installed: true,
            detail: Some(bin.display().to_string()),
            models: vec![],
            hint: Some(e.to_string()),
        },
    }
}

fn ollama_installed() -> bool {
    let exe = if cfg!(windows) {
        "ollama.exe"
    } else {
        "ollama"
    };
    let on_path = std::env::var_os("PATH")
        .map(|p| std::env::split_paths(&p).any(|d| d.join(exe).is_file()))
        .unwrap_or(false);
    on_path
        || [
            "/opt/homebrew/bin/ollama",
            "/usr/local/bin/ollama",
            "/Applications/Ollama.app",
            "/usr/bin/ollama",
        ]
        .iter()
        .any(|p| std::path::Path::new(p).exists())
}

pub async fn ollama(endpoints: &Endpoints) -> LocalDetection {
    let url = format!("{}/api/tags", endpoints.ollama_api.trim_end_matches('/'));
    let client = reqwest::Client::builder()
        .timeout(Duration::from_millis(1500))
        .redirect(reqwest::redirect::Policy::none())
        .build();
    let tags = match client {
        Ok(c) => match c.get(&url).send().await {
            Ok(r) if r.status().is_success() => r.json::<serde_json::Value>().await.ok(),
            _ => None,
        },
        Err(_) => None,
    };
    match tags {
        Some(v) => {
            let models = openai::ollama_model_names(&v);
            let has_models = !models.is_empty();
            LocalDetection {
                id: ProviderId::Ollama,
                available: has_models,
                installed: true,
                detail: Some(format!("running, {} model(s)", models.len())),
                models,
                hint: (!has_models).then(|| {
                    "Ollama is running but has no models. Pull one, for example `ollama pull llama3.2`.".into()
                }),
            }
        }
        None => {
            let installed = ollama_installed();
            LocalDetection {
                id: ProviderId::Ollama,
                available: false,
                installed,
                detail: None,
                models: vec![],
                hint: Some(if installed {
                    "Ollama is installed but not running. Open the Ollama app or run `ollama serve`.".into()
                } else {
                    "Install Ollama from ollama.com to make objects fully offline.".into()
                }),
            }
        }
    }
}

/// Detects every local provider concurrently.
pub async fn all(endpoints: &Endpoints) -> Vec<LocalDetection> {
    let (c, x, o) = tokio::join!(claude_code(endpoints), codex(endpoints), ollama(endpoints));
    vec![c, x, o]
}
