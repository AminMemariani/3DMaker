//! Anthropic Messages API.

use super::{Endpoints, MAX_OUTPUT_TOKENS, Provider, Verified};
use crate::errors::ProviderError;
use crate::model::ProviderId;
use crate::net::{self, AuthScheme, Backoff, ScopedClient};
use crate::redact::Secret;
use async_trait::async_trait;
use reqwest::Method;
use serde_json::{Value, json};
use tokio_util::sync::CancellationToken;

const API_VERSION: &str = "2023-06-01";

pub struct Anthropic {
    base: String,
    client: ScopedClient,
    model: Option<String>,
}

impl Anthropic {
    pub fn new(
        key: Secret,
        model: Option<String>,
        endpoints: &Endpoints,
    ) -> Result<Self, ProviderError> {
        let base = endpoints.anthropic_api.trim_end_matches('/').to_string();
        let client = ScopedClient::new(
            Some(key),
            AuthScheme::XApiKey,
            &[&base],
            endpoints.allow_loopback_http,
            endpoints.request_timeout,
        )?;
        Ok(Self {
            base,
            client,
            model: model.filter(|m| !m.trim().is_empty()),
        })
    }

    fn req(&self, method: Method, path: &str) -> Result<reqwest::RequestBuilder, ProviderError> {
        Ok(self
            .client
            .request(method, &format!("{}{path}", self.base))?
            .header("anthropic-version", API_VERSION))
    }
}

/// Orders model ids so a capable, mid-priced model comes first. The API
/// lists newest first; prefer the newest Sonnet, then anything else.
pub fn rank_models(ids: Vec<String>) -> Vec<String> {
    let (mut sonnet, rest): (Vec<_>, Vec<_>) = ids.into_iter().partition(|m| m.contains("sonnet"));
    sonnet.extend(rest);
    sonnet
}

#[async_trait]
impl Provider for Anthropic {
    fn id(&self) -> ProviderId {
        ProviderId::Anthropic
    }
    fn model(&self) -> String {
        self.model.clone().unwrap_or_default()
    }

    async fn verify(&self) -> Result<Verified, ProviderError> {
        let resp = net::send_safe(
            || self.req(Method::GET, "/v1/models?limit=100"),
            Backoff::default(),
            None,
        )
        .await?;
        let v = net::json_body(resp).await?;
        let ids = v["data"]
            .as_array()
            .map(|a| {
                a.iter()
                    .filter_map(|m| m["id"].as_str().map(String::from))
                    .collect::<Vec<_>>()
            })
            .unwrap_or_default();
        Ok(Verified {
            account_label: None,
            models: rank_models(ids),
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
        let body = json!({
            "model": model,
            "max_tokens": MAX_OUTPUT_TOKENS,
            "system": system,
            "messages": [{ "role": "user", "content": user }],
        });
        let resp = net::send_billable(
            || Ok(self.req(Method::POST, "/v1/messages")?.json(&body)),
            Some(cancel),
        )
        .await?;
        let v = net::json_body(resp).await?;
        parse_message(&v)
    }
}

fn parse_message(v: &Value) -> Result<String, ProviderError> {
    if v["type"] == "error" {
        return Err(ProviderError::ProviderFailed(
            v.pointer("/error/message")
                .and_then(Value::as_str)
                .unwrap_or("unknown error")
                .into(),
        ));
    }
    let text: String = v["content"]
        .as_array()
        .ok_or_else(|| ProviderError::BadResponse("the reply had no content".into()))?
        .iter()
        .filter(|b| b["type"] == "text")
        .filter_map(|b| b["text"].as_str())
        .collect();
    match v["stop_reason"].as_str() {
        Some("max_tokens") => Err(ProviderError::ProviderFailed(
            "the design was cut off because it ran past the length limit; try a simpler description".into(),
        )),
        Some("refusal") => Err(ProviderError::ProviderFailed(
            "the model declined to design this object".into(),
        )),
        _ if text.trim().is_empty() => {
            Err(ProviderError::BadResponse("the reply contained no text".into()))
        }
        _ => Ok(text),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_text_blocks_and_stop_reasons() {
        let ok = json!({"type":"message","content":[{"type":"text","text":"{\"a\":"},{"type":"text","text":"1}"}],"stop_reason":"end_turn"});
        assert_eq!(parse_message(&ok).unwrap(), "{\"a\":1}");
        let cut = json!({"type":"message","content":[{"type":"text","text":"{"}],"stop_reason":"max_tokens"});
        assert!(
            matches!(parse_message(&cut), Err(ProviderError::ProviderFailed(m)) if m.contains("cut off"))
        );
        let err =
            json!({"type":"error","error":{"type":"overloaded_error","message":"Overloaded"}});
        assert_eq!(
            parse_message(&err),
            Err(ProviderError::ProviderFailed("Overloaded".into()))
        );
    }

    #[test]
    fn sonnet_ranked_first() {
        let r = rank_models(vec![
            "claude-opus-x".into(),
            "claude-sonnet-y".into(),
            "claude-haiku-z".into(),
        ]);
        assert_eq!(r[0], "claude-sonnet-y");
        assert_eq!(r.len(), 3);
    }
}
