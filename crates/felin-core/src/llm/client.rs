//! OpenAI-compatible chat client with retry/backoff.

use crate::error::{Error, Result};
use crate::llm::{ChatMessage, LlmConfig, TranslateRequest};
use serde::Deserialize;
use std::time::Duration;

/// A configured LLM client.
pub struct LlmClient {
    config: LlmConfig,
    http: reqwest::Client,
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
    /// Build a client from `config`.
    pub fn new(config: LlmConfig) -> Result<Self> {
        let http = reqwest::Client::builder()
            .timeout(config.timeout)
            .build()
            .map_err(|e| Error::llm(format!("failed to build HTTP client: {e}")))?;
        Ok(Self { config, http })
    }

    pub fn config(&self) -> &LlmConfig {
        &self.config
    }

    /// Translate under the plaintext contract; returns the model's raw text
    /// (kept verbatim by the caller — the model is assumed fallible).
    pub async fn translate(&self, req: &TranslateRequest) -> Result<String> {
        self.chat(&crate::llm::build_messages(req)).await
    }

    /// One chat completion, with retry/backoff on transient failures.
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
            match self.try_once(&url, &body).await {
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
                "（endpoint 可能多写了一层 /chat/completions，应填 https://host/v1 或完整 URL；\
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

#[cfg(test)]
mod tests {
    use super::{chat_url, LlmClient};
    use crate::error::Error;
    use crate::llm::{ChatMessage, LlmConfig, TranslateRequest};
    use serde_json::json;
    use std::time::Duration;
    use wiremock::matchers::{method, path};
    use wiremock::{Mock, MockServer, ResponseTemplate};

    fn cfg(uri: String) -> LlmConfig {
        LlmConfig {
            endpoint: uri,
            model: "test-model".into(),
            api_key: "sk-test".into(),
            timeout: Duration::from_secs(5),
            max_retries: 3,
            base_delay: Duration::from_millis(1),
            max_delay: Duration::from_millis(5),
            temperature: None,
            max_tokens: None,
        }
    }

    fn ok_body(content: &str) -> serde_json::Value {
        json!({ "choices": [ { "message": { "role": "assistant", "content": content } } ] })
    }

    #[tokio::test]
    async fn returns_content_on_success() {
        let server = MockServer::start().await;
        Mock::given(method("POST"))
            .and(path("/chat/completions"))
            .respond_with(ResponseTemplate::new(200).set_body_json(ok_body("你好")))
            .mount(&server)
            .await;
        let client = LlmClient::new(cfg(server.uri())).unwrap();
        assert_eq!(client.chat(&[ChatMessage::user("hi")]).await.unwrap(), "你好");
    }

    #[tokio::test]
    async fn retries_on_5xx_then_succeeds() {
        let server = MockServer::start().await;
        Mock::given(method("POST"))
            .respond_with(ResponseTemplate::new(500))
            .up_to_n_times(1)
            .mount(&server)
            .await;
        Mock::given(method("POST"))
            .respond_with(ResponseTemplate::new(200).set_body_json(ok_body("ok")))
            .mount(&server)
            .await;
        let client = LlmClient::new(cfg(server.uri())).unwrap();
        assert_eq!(client.chat(&[ChatMessage::user("hi")]).await.unwrap(), "ok");
        assert_eq!(server.received_requests().await.unwrap().len(), 2);
    }

    #[tokio::test]
    async fn fatal_on_401_without_retry() {
        let server = MockServer::start().await;
        Mock::given(method("POST"))
            .respond_with(ResponseTemplate::new(401).set_body_string("unauthorized"))
            .expect(1)
            .mount(&server)
            .await;
        let client = LlmClient::new(cfg(server.uri())).unwrap();
        let err = client.chat(&[ChatMessage::user("hi")]).await.unwrap_err();
        assert!(matches!(err, Error::Llm { .. }));
        assert_eq!(server.received_requests().await.unwrap().len(), 1);
    }

    #[tokio::test]
    async fn gives_up_after_max_retries() {
        let server = MockServer::start().await;
        Mock::given(method("POST")).respond_with(ResponseTemplate::new(503)).mount(&server).await;
        let mut c = cfg(server.uri());
        c.max_retries = 2;
        let client = LlmClient::new(c).unwrap();
        assert!(matches!(client.chat(&[ChatMessage::user("hi")]).await.unwrap_err(), Error::Llm { .. }));
        // 1 initial attempt + 2 retries.
        assert_eq!(server.received_requests().await.unwrap().len(), 3);
    }

    #[tokio::test]
    async fn translate_returns_model_text() {
        let server = MockServer::start().await;
        Mock::given(method("POST"))
            .and(path("/chat/completions"))
            .respond_with(ResponseTemplate::new(200).set_body_json(ok_body("译文")))
            .mount(&server)
            .await;
        let client = LlmClient::new(cfg(server.uri())).unwrap();
        let req = TranslateRequest { system: "总则".into(), source: "原文".into(), ..Default::default() };
        assert_eq!(client.translate(&req).await.unwrap(), "译文");
    }

    #[test]
    fn chat_url_handles_base_and_full_urls() {
        // Bare base → append once.
        assert_eq!(chat_url("https://api.stepfun.com/v1"), "https://api.stepfun.com/v1/chat/completions");
        assert_eq!(chat_url("https://api.stepfun.com/v1/"), "https://api.stepfun.com/v1/chat/completions");
        // A trailing full URL is kept verbatim (no double suffix → 404).
        assert_eq!(
            chat_url("https://api.stepfun.com/step_plan/v1/chat/completions"),
            "https://api.stepfun.com/step_plan/v1/chat/completions"
        );
        // Whitespace around the value is trimmed.
        assert_eq!(chat_url("  https://host/v1  "), "https://host/v1/chat/completions");
    }

    #[tokio::test]
    async fn full_url_endpoint_hits_single_chat_completions_path() {
        let server = MockServer::start().await;
        // The configured endpoint already ends in /chat/completions — the client
        // must NOT append the suffix again, or this route (and thus the test)
        // would 404.
        Mock::given(method("POST"))
            .and(path("/step_plan/v1/chat/completions"))
            .respond_with(ResponseTemplate::new(200).set_body_json(ok_body("好")))
            .expect(1)
            .mount(&server)
            .await;
        let endpoint = format!("{}/step_plan/v1/chat/completions", server.uri());
        let client = LlmClient::new(cfg(endpoint)).unwrap();
        assert_eq!(client.chat(&[ChatMessage::user("hi")]).await.unwrap(), "好");
        assert_eq!(server.received_requests().await.unwrap().len(), 1);
    }

    #[tokio::test]
    async fn fatal_404_carries_endpoint_hint() {
        let server = MockServer::start().await;
        Mock::given(method("POST")).respond_with(ResponseTemplate::new(404)).expect(1).mount(&server).await;
        let client = LlmClient::new(cfg(server.uri())).unwrap();
        let err = client.chat(&[ChatMessage::user("hi")]).await.unwrap_err();
        let Error::Llm { detail } = err else { panic!("expected Llm error") };
        assert!(detail.contains("/chat/completions"), "hint should mention endpoint format: {detail}");
        assert_eq!(server.received_requests().await.unwrap().len(), 1, "404 is fatal, no retry");
    }
}
