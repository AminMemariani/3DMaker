//! OpenAI Chat Completions, also used for any OpenAI-compatible server
//! (the custom provider: OpenRouter, LM Studio, vLLM, …) and for Ollama,
//! which serves the same API under `/v1`.

use super::{Endpoints, Provider, Verified};
use crate::errors::ProviderError;
use crate::model::ProviderId;
use crate::net::{self, AuthScheme, Backoff, ScopedClient};
use crate::redact::Secret;
use async_trait::async_trait;
use reqwest::Method;
use serde_json::{Value, json};
use tokio_util::sync::CancellationToken;

pub struct OpenAiCompat {
    id: ProviderId,
    /// API base including `/v1`.
    base: String,
    client: ScopedClient,
    model: Option<String>,
    /// Ask for a JSON object reply (`response_format`). Only sent to servers
    /// known to support it; arbitrary custom servers may reject the field.
    json_mode: bool,
}

impl OpenAiCompat {
    pub fn openai(
        key: Secret,
        model: Option<String>,
        endpoints: &Endpoints,
    ) -> Result<Self, ProviderError> {
        Self::build(
            ProviderId::OpenAi,
            &endpoints.openai_api,
            Some(key),
            model,
            endpoints.allow_loopback_http,
            endpoints,
            true,
        )
    }

    /// `base` must already be normalized (see [`super::normalize_base`]).
    pub fn custom(
        base: &str,
        key: Option<Secret>,
        model: Option<String>,
        allow_loopback_http: bool,
        endpoints: &Endpoints,
    ) -> Result<Self, ProviderError> {
        Self::build(
            ProviderId::Custom,
            base,
            key,
            model,
            allow_loopback_http,
            endpoints,
            false,
        )
    }

    pub fn ollama(model: Option<String>, endpoints: &Endpoints) -> Result<Self, ProviderError> {
        let base = format!("{}/v1", endpoints.ollama_api.trim_end_matches('/'));
        // Ollama is a local service: plain HTTP to loopback is expected.
        Self::build(
            ProviderId::Ollama,
            &base,
            None,
            model,
            true,
            endpoints,
            true,
        )
    }

    fn build(
        id: ProviderId,
        base: &str,
        key: Option<Secret>,
        model: Option<String>,
        allow_loopback_http: bool,
        endpoints: &Endpoints,
        json_mode: bool,
    ) -> Result<Self, ProviderError> {
        let base = base.trim_end_matches('/').to_string();
        let client = ScopedClient::new(
            key,
            AuthScheme::Bearer,
            &[&base],
            allow_loopback_http,
            endpoints.request_timeout,
        )?;
        Ok(Self {
            id,
            base,
            client,
            model: model.filter(|m| !m.trim().is_empty()),
            json_mode,
        })
    }
}

/// Keywords marking OpenAI models that can't hold a text chat.
const NOT_CHAT: [&str; 12] = [
    "audio",
    "realtime",
    "tts",
    "transcribe",
    "whisper",
    "image",
    "dall-e",
    "embedding",
    "moderation",
    "search",
    "instruct",
    "codex",
];

/// Orders a `/v1/models` listing for the picker. For OpenAI itself, chat
/// models come first, newest first; other servers keep their order.
pub fn rank_models(id: ProviderId, data: &[Value]) -> Vec<String> {
    let mut items: Vec<(String, i64)> = data
        .iter()
        .filter_map(|m| {
            Some((
                m["id"].as_str()?.to_string(),
                m["created"].as_i64().unwrap_or(0),
            ))
        })
        .collect();
    if id == ProviderId::OpenAi {
        items.retain(|(m, _)| !NOT_CHAT.iter().any(|k| m.contains(k)));
        items.sort_by_key(|(m, created)| (!m.starts_with("gpt-"), -created));
    }
    items.into_iter().map(|(m, _)| m).collect()
}

#[async_trait]
impl Provider for OpenAiCompat {
    fn id(&self) -> ProviderId {
        self.id
    }
    fn model(&self) -> String {
        self.model.clone().unwrap_or_default()
    }

    async fn verify(&self) -> Result<Verified, ProviderError> {
        if self.id == ProviderId::Ollama {
            return ollama_tags(&self.client, &self.base).await;
        }
        let url = format!("{}/models", self.base);
        let resp = net::send_safe(
            || self.client.request(Method::GET, &url),
            Backoff::default(),
            None,
        )
        .await?;
        let v = net::json_body(resp).await?;
        let data = v["data"].as_array().cloned().unwrap_or_default();
        Ok(Verified {
            account_label: None,
            models: rank_models(self.id, &data),
        })
    }

    async fn complete(
        &self,
        system: &str,
        user: &str,
        cancel: &CancellationToken,
    ) -> Result<String, ProviderError> {
        let model = self
            .model
            .clone()
            .ok_or_else(|| ProviderError::ModelNotFound("no model selected".into()))?;
        let mut body = json!({
            "model": model,
            "messages": [
                { "role": "system", "content": system },
                { "role": "user", "content": user },
            ],
        });
        if self.json_mode {
            body["response_format"] = json!({ "type": "json_object" });
        }
        let url = format!("{}/chat/completions", self.base);
        let resp = net::send_billable(
            || Ok(self.client.request(Method::POST, &url)?.json(&body)),
            Some(cancel),
        )
        .await
        .map_err(|e| match e {
            // Ollama answers 404 for a model that isn't pulled.
            ProviderError::BadResponse(m) if m.starts_with("HTTP 404") => {
                ProviderError::ModelNotFound(model.clone())
            }
            other => other,
        })?;
        let v = net::json_body(resp).await?;
        parse_completion(&v)
    }
}

/// Lists installed Ollama models through its native API.
async fn ollama_tags(client: &ScopedClient, v1_base: &str) -> Result<Verified, ProviderError> {
    let root = v1_base.trim_end_matches("/v1");
    let url = format!("{root}/api/tags");
    let resp = net::send_safe(
        || client.request(Method::GET, &url),
        Backoff {
            attempts: 1,
            ..Backoff::default()
        },
        None,
    )
    .await
    .map_err(|e| match e {
        ProviderError::Network(_) => ProviderError::Unavailable(
            "Ollama isn't running. Open the Ollama app or run `ollama serve`, then try again."
                .into(),
        ),
        other => other,
    })?;
    let v = net::json_body(resp).await?;
    let models = ollama_model_names(&v);
    if models.is_empty() {
        return Err(ProviderError::Unavailable(
            "Ollama is running but has no models. Pull one first, for example `ollama pull llama3.2`."
                .into(),
        ));
    }
    Ok(Verified {
        account_label: Some(format!("{} local model(s)", models.len())),
        models,
    })
}

pub fn ollama_model_names(tags: &Value) -> Vec<String> {
    tags["models"]
        .as_array()
        .map(|a| {
            a.iter()
                .filter_map(|m| m["name"].as_str().or_else(|| m["model"].as_str()))
                // Embedding-only models can't write designs.
                .filter(|n| !n.contains("embed"))
                .map(String::from)
                .collect()
        })
        .unwrap_or_default()
}

fn parse_completion(v: &Value) -> Result<String, ProviderError> {
    if let Some(msg) = v.pointer("/error/message").and_then(Value::as_str) {
        return Err(ProviderError::ProviderFailed(msg.into()));
    }
    let choice = v
        .pointer("/choices/0")
        .ok_or_else(|| ProviderError::BadResponse("the reply had no choices".into()))?;
    if let Some(r) = choice.pointer("/message/refusal").and_then(Value::as_str) {
        return Err(ProviderError::ProviderFailed(format!(
            "the model declined: {r}"
        )));
    }
    if choice["finish_reason"] == "length" {
        return Err(ProviderError::ProviderFailed(
            "the design was cut off because it ran past the length limit; try a simpler description".into(),
        ));
    }
    let text = choice
        .pointer("/message/content")
        .and_then(Value::as_str)
        .unwrap_or_default();
    if text.trim().is_empty() {
        return Err(ProviderError::BadResponse(
            "the reply contained no text".into(),
        ));
    }
    Ok(text.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn openai_models_ranked_chat_first_newest_first() {
        let data = vec![
            json!({"id":"text-embedding-3-small","created":5}),
            json!({"id":"gpt-old","created":1}),
            json!({"id":"o-reasoner","created":9}),
            json!({"id":"gpt-new","created":3}),
            json!({"id":"gpt-new-realtime","created":8}),
        ];
        assert_eq!(
            rank_models(ProviderId::OpenAi, &data),
            ["gpt-new", "gpt-old", "o-reasoner"]
        );
        // Other servers keep their own order and models.
        assert_eq!(rank_models(ProviderId::Custom, &data).len(), 5);
    }

    #[test]
    fn completion_parsing() {
        let ok = json!({"choices":[{"message":{"content":"{}"},"finish_reason":"stop"}]});
        assert_eq!(parse_completion(&ok).unwrap(), "{}");
        let cut = json!({"choices":[{"message":{"content":"{"},"finish_reason":"length"}]});
        assert!(parse_completion(&cut).is_err());
        let refused = json!({"choices":[{"message":{"content":null,"refusal":"no"}}]});
        assert!(
            matches!(parse_completion(&refused), Err(ProviderError::ProviderFailed(m)) if m.contains("declined"))
        );
    }

    #[test]
    fn ollama_names_skip_embedding_models() {
        let v = json!({"models":[{"name":"llama3.2:latest"},{"name":"nomic-embed-text:latest"},{"model":"qwen3:8b"}]});
        assert_eq!(ollama_model_names(&v), ["llama3.2:latest", "qwen3:8b"]);
    }
}
