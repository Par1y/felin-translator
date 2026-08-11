//! OpenAI-compatible chat client with retry/backoff.

use crate::error::{Error, Result};
use crate::llm::{ChatMessage, LlmConfig, TranslateRequest};
use serde::Deserialize;
use std::sync::Arc;
use std::time::Duration;
use tokio::sync::Semaphore;

/// A configured LLM client.
pub struct LlmClient {
    config: LlmConfig,
    http: reqwest::Client,
    /// Global concurrency limiter shared across every LLM feature (translation
    /// workers, extraction, auto-tag, connection test). `Arc` so several
    /// clients — one per run — share the same cap instead of each getting its
    /// own. See `docs/data-contract.md` §6.
    limiter: Arc<Semaphore>,
}

/// Internal per-attempt failure classification.
enum CallError {
    /// Do not retry (client misuse / auth / bad request).
    Fatal(String),
    /// Transient; retry after `retry_after` (or computed backoff).
    Retryable { msg: String, retry_after: Option<Duration> },
}

#[derive(Deserialize)]
struct ChatResponse {
    choices: Vec<Choice>,
}
#[derive(Deserialize)]
struct Choice {
    message: RespMsg,
}
#[derive(Deserialize)]
struct RespMsg {
    #[serde(default)]
    content: String,
}

/// Build the chat-completions URL from a configured endpoint.
///
/// The endpoint may be either a bare base (`https://api.stepfun.com/v1`) or a
/// full chat-completions URL already (`https://api.stepfun.com/step_plan/v1/
/// chat/completions`, as ocr-router's config uses). Blindly appending the
/// suffix to the latter would double it and yield an HTTP 404.
fn chat_url(endpoint: &str) -> String {
    let e = endpoint.trim();
    if e.ends_with("/chat/completions") {
        e.to_string()
    } else {
        format!("{}/chat/completions", e.trim_end_matches('/'))
    }
}

impl LlmClient {
    /// Build a client from `config`, with its own private concurrency limiter
    /// sized by `config.concurrency` (tests / one-off callers).
    pub fn new(config: LlmConfig) -> Result<Self> {
        let permits = (config.concurrency.clamp(1, 16)) as usize;
        Self::with_limiter(config, Arc::new(Semaphore::new(permits)))
    }

    /// Build a client that shares `limiter` (the app-wide one) instead of
    /// creating its own — the unified-concurrency path: every feature's client
    /// is constructed here so all LLM calls queue on one global cap.
    pub fn with_limiter(config: LlmConfig, limiter: Arc<Semaphore>) -> Result<Self> {
        let http = reqwest::Client::builder()
            .timeout(config.timeout)
            .build()
            .map_err(|e| Error::llm(format!("failed to build HTTP client: {e}")))?;
        Ok(Self { config, http, limiter })
    }

    pub fn config(&self) -> &LlmConfig {
        &self.config
    }

    /// Translate under the plaintext contract; returns the model's raw text
    /// (kept verbatim by the caller — the model is assumed fallible).
    pub async fn translate(&self, req: &TranslateRequest) -> Result<String> {
        self.chat(&crate::llm::build_messages(req)).await
    }

    /// One chat completion, with retry/backoff on transient failures. A permit
    /// is acquired **per network attempt** and released before the backoff
    /// sleep, so the shared cap bounds in-flight requests without letting one
    /// stalled call hog a permit (and thus block every other LLM feature) for
    /// the whole retry window.
    pub async fn chat(&self, messages: &[ChatMessage]) -> Result<String> {
        let url = chat_url(&self.config.endpoint);
        let mut body = serde_json::json!({
            "model": self.config.model,
            "messages": messages,
            "stream": false,
        });
        if let Some(t) = self.config.temperature {
            body["temperature"] = t.into();
        }
        if let Some(m) = self.config.max_tokens {
            body["max_tokens"] = m.into();
        }

        let mut attempt = 0u32;
        loop {
            // Scope the permit to a single network attempt: it is released
            // before the backoff sleep below, so a stalled call never holds a
            // permit (and thus never blocks every other LLM feature) across the
            // whole retry window.
            let outcome = {
                let _permit = self
                    .limiter
                    .acquire()
                    .await
                    .map_err(|e| Error::llm(format!("rate limiter closed: {e}")))?;
                self.try_once(&url, &body).await
            };
            match outcome {
                Ok(text) => return Ok(text),
                Err(CallError::Fatal(msg)) => return Err(Error::llm(msg)),
                Err(CallError::Retryable { msg, retry_after }) => {
                    if attempt >= self.config.max_retries {
                        return Err(Error::llm(format!(
                            "giving up after {} attempt(s): {msg}",
                            attempt + 1
                        )));
                    }
                    let delay = retry_after.unwrap_or_else(|| self.backoff(attempt));
                    tracing::warn!(attempt, ?delay, error = %msg, "LLM call failed; retrying");
                    tokio::time::sleep(delay).await;
                    attempt += 1;
                }
            }
        }
    }

    async fn try_once(
        &self,
        url: &str,
        body: &serde_json::Value,
    ) -> std::result::Result<String, CallError> {
        let resp = match self.http.post(url).bearer_auth(&self.config.api_key).json(body).send().await {
            Ok(r) => r,
            // Connect / timeout / DNS — transient.
            Err(e) => {
                return Err(CallError::Retryable { msg: format!("request error: {e}"), retry_after: None })
            }
        };

        let status = resp.status();
        if status.is_success() {
            let parsed: ChatResponse =
                resp.json().await.map_err(|e| CallError::Fatal(format!("bad response JSON: {e}")))?;
            return parsed
                .choices
                .into_iter()
                .next()
                .map(|c| c.message.content)
                .ok_or_else(|| CallError::Fatal("response contained no choices".into()));
        }

        let retry_after = parse_retry_after(resp.headers());
        let code = status.as_u16();
        let body = truncate(&resp.text().await.unwrap_or_default(), 500);
        let mut detail = format!("HTTP {code}: {body}");
        if code == 404 {
            // The most common cause is an endpoint that already ends in
            // /chat/completions (as ocr-router's config uses); a wrong model
            // name or an account without that model also 404s here.
            detail.push_str(
                "（endpoint URI错误；\
                 或模型名/路径不对，账号无权访问该模型）",
            );
        }
        // 429 / 408 / 5xx are transient; other 4xx (400/401/403/404) fast-fail.
        if code == 429 || code == 408 || status.is_server_error() {
            Err(CallError::Retryable { msg: detail, retry_after })
        } else {
            Err(CallError::Fatal(detail))
        }
    }

    /// Exponential backoff (base·2^attempt, capped) plus a little clock-derived
    /// jitter (no RNG dependency).
    fn backoff(&self, attempt: u32) -> Duration {
        let base = self.config.base_delay.as_millis() as u64;
        let exp = base.saturating_mul(1u64 << attempt.min(16));
        let capped = exp.min(self.config.max_delay.as_millis() as u64);
        let jitter = now_nanos() % base.max(1);
        Duration::from_millis(capped.saturating_add(jitter))
    }
}

fn now_nanos() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.subsec_nanos() as u64)
        .unwrap_or(0)
}

fn parse_retry_after(headers: &reqwest::header::HeaderMap) -> Option<Duration> {
    // Only the delta-seconds form is honored; the HTTP-date form is ignored.
    let v = headers.get(reqwest::header::RETRY_AFTER)?.to_str().ok()?;
    v.trim().parse::<u64>().ok().map(Duration::from_secs)
}

fn truncate(s: &str, max: usize) -> String {
    if s.chars().count() <= max {
        s.to_string()
    } else {
        s.chars().take(max).collect::<String>() + "…"
    }
}

