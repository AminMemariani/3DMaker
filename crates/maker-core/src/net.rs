//! HTTP plumbing shared by the provider adapters: credential scoping,
//! transport policy, bounded retries with Retry-After, and atomic commits.

use crate::errors::ProviderError;
use crate::redact::{Secret, redact_url};
use reqwest::{Method, RequestBuilder, Response, StatusCode};
use std::path::Path;
use std::time::Duration;
use tokio_util::sync::CancellationToken;
use url::Url;

/// How the key is attached to requests.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AuthScheme {
    /// `Authorization: Bearer <key>` (OpenAI and OpenAI-compatible servers)
    Bearer,
    /// `x-api-key: <key>` (Anthropic)
    XApiKey,
}

/// An authenticated client bound to the origin the key belongs to.
/// Requests to any other origin are refused before credentials are attached,
/// and redirects are never followed.
#[derive(Clone)]
pub struct ScopedClient {
    http: reqwest::Client,
    secret: Option<Secret>,
    scheme: AuthScheme,
    origins: Vec<url::Origin>,
    allow_loopback_http: bool,
}

pub fn origin_of(u: &str) -> Result<url::Origin, ProviderError> {
    Url::parse(u)
        .map(|u| u.origin())
        .map_err(|e| ProviderError::Policy(format!("bad URL: {e}")))
}

pub fn is_loopback(u: &Url) -> bool {
    match u.host() {
        Some(url::Host::Domain(d)) => d.eq_ignore_ascii_case("localhost"),
        Some(url::Host::Ipv4(ip)) => ip.is_loopback(),
        Some(url::Host::Ipv6(ip)) => ip.is_loopback(),
        None => false,
    }
}

/// HTTPS is required everywhere; plain HTTP only to an explicitly allowed loopback service.
pub fn check_transport(u: &Url, allow_loopback_http: bool) -> Result<(), ProviderError> {
    match u.scheme() {
        "https" => Ok(()),
        "http" if allow_loopback_http && is_loopback(u) => Ok(()),
        "http" => Err(ProviderError::Policy(format!(
            "{} uses unencrypted http; only https (or an explicitly allowed local service) is permitted",
            redact_url(u)
        ))),
        other => Err(ProviderError::Policy(format!(
            "unsupported URL scheme `{other}`"
        ))),
    }
}

impl ScopedClient {
    pub fn new(
        secret: Option<Secret>,
        scheme: AuthScheme,
        origins: &[&str],
        allow_loopback_http: bool,
        request_timeout: Duration,
    ) -> Result<Self, ProviderError> {
        let http = reqwest::Client::builder()
            .redirect(reqwest::redirect::Policy::none())
            .connect_timeout(Duration::from_secs(10))
            .timeout(request_timeout)
            .user_agent(concat!("3DMaker/", env!("CARGO_PKG_VERSION")))
            .build()
            .map_err(|e| ProviderError::Network(e.to_string()))?;
        let origins = origins
            .iter()
            .map(|o| origin_of(o))
            .collect::<Result<Vec<_>, _>>()?;
        Ok(Self {
            http,
            secret,
            scheme,
            origins,
            allow_loopback_http,
        })
    }

    pub fn is_in_scope(&self, u: &Url) -> bool {
        self.origins.iter().any(|o| *o == u.origin())
    }

    /// Builds a request; refuses URLs outside the credential's origins.
    pub fn request(&self, method: Method, url: &str) -> Result<RequestBuilder, ProviderError> {
        let u = Url::parse(url).map_err(|e| ProviderError::Policy(format!("bad URL: {e}")))?;
        check_transport(&u, self.allow_loopback_http)?;
        if !self.is_in_scope(&u) {
            return Err(ProviderError::Policy(format!(
                "refusing to send credentials to {}, which is not the configured provider",
                redact_url(&u)
            )));
        }
        let mut rb = self.http.request(method, u);
        if let Some(s) = self.secret.as_ref().filter(|s| !s.is_empty()) {
            let (name, value) = match self.scheme {
                AuthScheme::Bearer => (
                    reqwest::header::AUTHORIZATION,
                    format!("Bearer {}", s.expose()),
                ),
                AuthScheme::XApiKey => (
                    reqwest::header::HeaderName::from_static("x-api-key"),
                    s.expose().to_string(),
                ),
            };
            let mut hv = reqwest::header::HeaderValue::from_str(&value)
                .map_err(|_| ProviderError::InvalidCredentials)?;
            hv.set_sensitive(true);
            rb = rb.header(name, hv);
        }
        Ok(rb)
    }

    pub fn has_secret(&self) -> bool {
        self.secret.as_ref().is_some_and(|s| !s.is_empty())
    }
}

/// The most specific cause of a transport error (e.g. "dns error: failed to
/// lookup address", "Connection refused", "invalid peer certificate"), so the
/// user sees why the provider couldn't be reached.
pub fn describe(e: &reqwest::Error) -> String {
    let mut msg = e.to_string();
    let mut src = std::error::Error::source(e);
    while let Some(s) = src {
        msg = s.to_string();
        src = s.source();
    }
    let kind = if e.is_timeout() {
        "timed out"
    } else if e.is_connect() {
        "connection failed"
    } else {
        "request failed"
    };
    crate::redact::redact(&format!("{kind}: {msg}"))
}

/// Parses `Retry-After` given in seconds (HTTP-date form falls back to None).
pub fn retry_after(resp: &Response) -> Option<Duration> {
    resp.headers()
        .get(reqwest::header::RETRY_AFTER)
        .and_then(|v| v.to_str().ok())
        .and_then(|s| s.trim().parse::<u64>().ok())
        .map(|s| Duration::from_secs(s.min(300)))
}

/// Maps common HTTP status codes to provider errors.
pub async fn status_error(resp: Response) -> ProviderError {
    let status = resp.status();
    let ra = retry_after(&resp);
    let body = resp.text().await.unwrap_or_default();
    let body = crate::redact::redact(&body);
    let lower = body.to_ascii_lowercase();
    let billing = ["balance", "billing", "credit", "quota", "payment"]
        .iter()
        .any(|w| lower.contains(w));
    match status {
        StatusCode::UNAUTHORIZED => ProviderError::InvalidCredentials,
        StatusCode::FORBIDDEN if billing => ProviderError::NoCredit,
        StatusCode::FORBIDDEN => ProviderError::InvalidCredentials,
        StatusCode::PAYMENT_REQUIRED => ProviderError::NoCredit,
        // OpenAI reports an exhausted quota as 429 `insufficient_quota`.
        StatusCode::TOO_MANY_REQUESTS if lower.contains("insufficient_quota") => {
            ProviderError::NoCredit
        }
        StatusCode::TOO_MANY_REQUESTS => ProviderError::RateLimited { retry_after: ra },
        StatusCode::BAD_REQUEST if billing && lower.contains("credit") => ProviderError::NoCredit,
        StatusCode::NOT_FOUND if lower.contains("model") => {
            ProviderError::ModelNotFound(api_message(&body))
        }
        s if s.is_redirection() => {
            ProviderError::Policy(format!("unexpected redirect ({s}) from the provider API"))
        }
        // 529 is Anthropic's "overloaded".
        s if s.is_server_error() || s.as_u16() == 529 => {
            ProviderError::Network(format!("provider returned {s}"))
        }
        s => ProviderError::BadResponse(format!("HTTP {s}: {}", api_message(&body))),
    }
}

/// Pulls the human-readable message out of a typical JSON error body.
fn api_message(body: &str) -> String {
    let v: Option<serde_json::Value> = serde_json::from_str(body).ok();
    let msg = v.as_ref().and_then(|v| {
        v.pointer("/error/message")
            .or_else(|| v.get("error"))
            .or_else(|| v.get("message"))
            .and_then(|m| m.as_str())
            .map(String::from)
    });
    msg.unwrap_or_else(|| body.chars().take(300).collect())
}

#[derive(Debug, Clone, Copy)]
pub struct Backoff {
    pub attempts: u32,
    pub base: Duration,
    pub max: Duration,
}

impl Default for Backoff {
    fn default() -> Self {
        Self {
            attempts: 4,
            base: Duration::from_millis(500),
            max: Duration::from_secs(20),
        }
    }
}

impl Backoff {
    pub fn delay(&self, attempt: u32) -> Duration {
        let exp = self.base.saturating_mul(1u32 << attempt.min(10));
        // deterministic jitter (no RNG dependency): +0..25%
        let jitter = exp / 4 * ((attempt * 7919) % 4) / 3;
        (exp + jitter).min(self.max)
    }
}

async fn sleep_or_cancel(
    d: Duration,
    cancel: Option<&CancellationToken>,
) -> Result<(), ProviderError> {
    match cancel {
        Some(c) => tokio::select! {
            _ = tokio::time::sleep(d) => Ok(()),
            _ = c.cancelled() => Err(ProviderError::Cancelled),
        },
        None => {
            tokio::time::sleep(d).await;
            Ok(())
        }
    }
}

/// Sends an idempotent request (model lists, health checks) with bounded
/// retries on network errors, 5xx and 429, honouring Retry-After.
pub async fn send_safe(
    build: impl Fn() -> Result<RequestBuilder, ProviderError>,
    backoff: Backoff,
    cancel: Option<&CancellationToken>,
) -> Result<Response, ProviderError> {
    let mut attempt = 0;
    loop {
        let result = build()?.send().await;
        let err = match result {
            Ok(r) if r.status().is_success() => return Ok(r),
            Ok(r) => {
                let e = status_error(r).await;
                if !e.is_transient() {
                    return Err(e);
                }
                e
            }
            Err(e) => ProviderError::Network(describe(&e)),
        };
        attempt += 1;
        if attempt >= backoff.attempts {
            return Err(err);
        }
        let wait = match &err {
            ProviderError::RateLimited {
                retry_after: Some(d),
            } => *d,
            _ => backoff.delay(attempt),
        };
        sleep_or_cancel(wait, cancel).await?;
    }
}

/// Sends a potentially billable request. It is retried only when it provably
/// did not reach the provider (connection refused before sending, or an
/// explicit 429). A failure after sending is reported as `UnknownOutcome`
/// and never retried automatically.
pub async fn send_billable(
    build: impl Fn() -> Result<RequestBuilder, ProviderError>,
    cancel: Option<&CancellationToken>,
) -> Result<Response, ProviderError> {
    let backoff = Backoff {
        attempts: 3,
        ..Backoff::default()
    };
    let mut attempt = 0;
    loop {
        let err = match build()?.send().await {
            Ok(r) if r.status().is_success() => return Ok(r),
            Ok(r) => match status_error(r).await {
                e @ ProviderError::RateLimited { .. } => e,
                // A 5xx is ambiguous: the provider may have done the work.
                ProviderError::Network(msg) => return Err(ProviderError::UnknownOutcome(msg)),
                other => return Err(other),
            },
            Err(e) if e.is_connect() => ProviderError::Network(describe(&e)),
            Err(e) => return Err(ProviderError::UnknownOutcome(describe(&e))),
        };
        attempt += 1;
        if attempt >= backoff.attempts {
            return Err(err);
        }
        let wait = match &err {
            ProviderError::RateLimited {
                retry_after: Some(d),
            } => *d,
            _ => backoff.delay(attempt),
        };
        sleep_or_cancel(wait, cancel).await?;
    }
}

/// Reads a JSON body, mapping failures to `BadResponse`.
pub async fn json_body(resp: Response) -> Result<serde_json::Value, ProviderError> {
    resp.json()
        .await
        .map_err(|e| ProviderError::BadResponse(format!("the reply was not JSON ({e})")))
}

/// Atomically moves a fully written temp file to `dest` (same directory tree),
/// then syncs the directory so the rename survives a crash.
pub fn commit(temp: tempfile::TempPath, dest: &Path) -> Result<(), ProviderError> {
    temp.persist(dest)
        .map_err(|e| ProviderError::from_io(&e.error))?;
    #[cfg(unix)]
    if let Some(parent) = dest.parent() {
        if let Ok(d) = std::fs::File::open(parent) {
            let _ = d.sync_all();
        }
    }
    Ok(())
}
