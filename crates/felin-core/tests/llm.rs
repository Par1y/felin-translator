//! LLM client + prompt-assembly integration tests.
//!
//! Moved here from the crate's inline `#[cfg(test)]` modules (per project
//! policy: no test code alongside application code). They drive the *public*
//! `felin_core::llm` API; `chat_url` / the private message renderer are
//! exercised indirectly through [`LlmClient::chat`] / [`build_messages`].

use felin_core::llm::{
    build_messages, ChatMessage, LlmConfig, LlmClient, Role, TranslateRequest,
};
use serde_json::json;
use std::time::Duration;
use wiremock::matchers::{method, path};
use wiremock::{Mock, MockServer, ResponseTemplate};

/// The factory prompt templates a fresh `felin.toml` ships with (single source
/// of truth: `TechConfig::default_template()`, not code constants).
fn tpl() -> (String, String) {
    let c: felin_core::config::TechConfig =
        felin_core::config::TechConfig::from_toml_str(&felin_core::config::TechConfig::default_template())
            .unwrap();
    (c.prompt.translation_system, c.prompt.translation_user)
}

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
        concurrency: 2,
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
    assert!(matches!(err, felin_core::Error::Llm { .. }));
    assert_eq!(server.received_requests().await.unwrap().len(), 1);
}

#[tokio::test]
async fn gives_up_after_max_retries() {
    let server = MockServer::start().await;
    Mock::given(method("POST")).respond_with(ResponseTemplate::new(503)).mount(&server).await;
    let mut c = cfg(server.uri());
    c.max_retries = 2;
    let client = LlmClient::new(c).unwrap();
    assert!(matches!(client.chat(&[ChatMessage::user("hi")]).await.unwrap_err(), felin_core::Error::Llm { .. }));
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
    let req = TranslateRequest { guidelines: "总则".into(), source: "原文".into(), ..Default::default() };
    assert_eq!(client.translate(&req).await.unwrap(), "译文");
}

/// URL handling (the 404 fix): a bare base gets `/chat/completions` appended
/// once; a full URL is kept verbatim. Exercised through the HTTP path so the
/// private `chat_url` helper is covered behaviorally.
#[tokio::test]
async fn endpoint_normalization_base_and_full_urls() {
    // Bare base → one append.
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/chat/completions"))
        .respond_with(ResponseTemplate::new(200).set_body_json(ok_body("好")))
        .mount(&server)
        .await;
    let client = LlmClient::new(cfg(server.uri())).unwrap();
    assert_eq!(client.chat(&[ChatMessage::user("hi")]).await.unwrap(), "好");

    // Full chat-completions URL → kept verbatim (a second append would 404).
    let server2 = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/step_plan/v1/chat/completions"))
        .respond_with(ResponseTemplate::new(200).set_body_json(ok_body("好")))
        .expect(1)
        .mount(&server2)
        .await;
    let endpoint = format!("{}/step_plan/v1/chat/completions", server2.uri());
    let client = LlmClient::new(cfg(endpoint)).unwrap();
    assert_eq!(client.chat(&[ChatMessage::user("hi")]).await.unwrap(), "好");
    assert_eq!(server2.received_requests().await.unwrap().len(), 1);
}

#[tokio::test]
async fn fatal_404_carries_endpoint_hint() {
    let server = MockServer::start().await;
    Mock::given(method("POST")).respond_with(ResponseTemplate::new(404)).expect(1).mount(&server).await;
    let client = LlmClient::new(cfg(server.uri())).unwrap();
    let err = client.chat(&[ChatMessage::user("hi")]).await.unwrap_err();
    let felin_core::Error::Llm { detail } = err else { panic!("expected Llm error") };
    assert!(detail.contains("endpoint URI错误"), "hint should mention endpoint format: {detail}");
    assert_eq!(server.received_requests().await.unwrap().len(), 1, "404 is fatal, no retry");
}

#[test]
fn assembles_default_system_and_user_from_placeholders() {
    let (sys_tpl, user_tpl) = tpl();
    let req = TranslateRequest {
        guidelines: "保持原排版。".into(),
        instruction: Some("句尾用“呢”。".into()),
        glossary: Some("田中 → 田中\n佐藤 → 佐藤".into()),
        context: Some("前文译文。".into()),
        source: "本文。".into(),
        system_template: sys_tpl,
        user_template: user_tpl,
    };
    let m = build_messages(&req);
    assert_eq!(m.len(), 2);
    assert_eq!(m[0].role, Role::System);
    assert!(m[0].content.contains("保持原排版。"));
    assert!(m[0].content.contains("句尾用“呢”。"));
    assert!(m[0].content.contains("专名参考（词表，必须使用）："));
    assert!(m[0].content.contains("田中 → 田中"));
    assert_eq!(m[1].role, Role::User);
    assert!(m[1].content.contains("前文译文。"));
    assert!(m[1].content.contains("本文。"));
}

#[test]
fn omits_empty_instruction_glossary_and_context_blocks() {
    let (sys_tpl, user_tpl) = tpl();
    let req = TranslateRequest {
        guidelines: "总则".into(),
        source: "唯一原文。".into(),
        system_template: sys_tpl,
        user_template: user_tpl,
        ..Default::default()
    };
    let m = build_messages(&req);
    assert_eq!(m.len(), 2);
    assert_eq!(m[0].role, Role::System);
    assert_eq!(m[0].content, "总则");
    assert_eq!(m[1].role, Role::User);
    assert_eq!(m[1].content, "【待翻译原文】\n唯一原文。");
    assert!(!m[1].content.contains("上文参考"));
}

#[test]
fn custom_templates_are_used_verbatim() {
    let req = TranslateRequest {
        guidelines: "总则".into(),
        source: "原文".into(),
        system_template: "自定义：{guidelines} 优先".into(),
        user_template: "{source}｜完".into(),
        ..Default::default()
    };
    let m = build_messages(&req);
    assert_eq!(m.len(), 2);
    assert_eq!(m[0].content, "自定义：总则 优先");
    assert_eq!(m[1].content, "原文｜完");
}

#[test]
fn empty_templates_mean_no_such_message() {
    // Config-file-driven semantics: an empty system template sends no system
    // message; an empty user template sends the raw source.
    let req = TranslateRequest {
        guidelines: "总则".into(),
        source: "原文".into(),
        system_template: String::new(),
        user_template: String::new(),
        ..Default::default()
    };
    let m = build_messages(&req);
    assert_eq!(m.len(), 1, "no system message when translation_system is empty");
    assert_eq!(m[0].role, Role::User);
    assert_eq!(m[0].content, "原文");
}

// ----- tolerant JSON extraction (the name-extraction pass) -------------------

#[test]
fn parses_direct_array() {
    let v = felin_core::llm::extract_json(r#"[{"japanese":"田中","guess_chinese":"田中"}]"#).unwrap();
    assert!(v.is_array());
    assert_eq!(v[0]["japanese"], "田中");
}

#[test]
fn extracts_from_code_fence() {
    let s = "```json\n[{\"a\":1}]\n```";
    assert_eq!(felin_core::llm::extract_json(s).unwrap(), serde_json::json!([{"a":1}]));
}

#[test]
fn extracts_from_surrounding_prose() {
    let s = "候选如下：\n[{\"japanese\":\"猫\"}]\n以上。";
    let v = felin_core::llm::extract_json(s).unwrap();
    assert_eq!(v[0]["japanese"], "猫");
}

#[test]
fn ignores_brackets_inside_strings() {
    let v = felin_core::llm::extract_json(r#"prefix ["a]b", "c"] suffix"#).unwrap();
    assert_eq!(v, serde_json::json!(["a]b", "c"]));
}

#[test]
fn returns_none_for_no_json() {
    assert!(felin_core::llm::extract_json("すみません、JSONを出力できません。").is_none());
}

#[test]
fn handles_object() {
    assert_eq!(
        felin_core::llm::extract_json("note {\"k\": true} end").unwrap(),
        serde_json::json!({"k": true})
    );
}

// ----- unified concurrency limiter -------------------------------------------

#[tokio::test]
async fn shared_limiter_caps_concurrent_llm_calls() {
    use std::sync::Arc;
    use tokio::sync::Semaphore;

    // A server that delays each response so concurrent calls would overlap if
    // the semaphore didn't cap them.
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/chat/completions"))
        .respond_with(
            ResponseTemplate::new(200)
                .set_delay(Duration::from_millis(120))
                .set_body_json(ok_body("ok")),
        )
        .mount(&server)
        .await;

    // Two clients sharing one limiter of capacity 1.
    let limiter = Arc::new(Semaphore::new(1));
    let a = LlmClient::with_limiter(cfg(server.uri()), Arc::clone(&limiter)).unwrap();
    let b = LlmClient::with_limiter(cfg(server.uri()), limiter).unwrap();

    // Fire both concurrently. With a shared cap of 1 they serialize, so the
    // total wall time ≈ 2 × delay; without sharing they'd overlap ≈ 1 × delay.
    let msg = [ChatMessage::user("hi")];
    let start = std::time::Instant::now();
    let (r1, r2) = tokio::join!(a.chat(&msg), b.chat(&msg));
    let elapsed = start.elapsed();
    assert_eq!(r1.unwrap(), "ok");
    assert_eq!(r2.unwrap(), "ok");
    assert!(
        elapsed >= Duration::from_millis(200),
        "shared cap 1 must serialize two calls (elapsed {elapsed:?})"
    );

    // And each client's own limit would also apply if not shared.
    let solo = LlmClient::new(LlmConfig {
        concurrency: 1,
        ..cfg(server.uri())
    })
    .unwrap();
    let start = std::time::Instant::now();
    let (r1, r2) = tokio::join!(solo.chat(&msg), solo.chat(&msg));
    assert!(start.elapsed() >= Duration::from_millis(200));
    assert_eq!(r1.unwrap(), "ok");
    assert_eq!(r2.unwrap(), "ok");
}
