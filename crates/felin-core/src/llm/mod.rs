//! LLM client (plan step 6): an OpenAI-compatible chat client with a plaintext
//! translation contract, retry/backoff, and tolerant JSON extraction for the
//! (non-fatal) name-extraction pass.
//!
//! Fully config-driven: endpoint / model are seeded from `felin.toml [llm]`
//! (see `crate::config::LlmDefaults`) and overridable per project in the GUI
//! settings page; the key comes from the GUI. This module stays transport-only
//! and Tauri-agnostic.

pub mod client;
pub mod json;
pub mod prompt;

pub use client::LlmClient;
pub use json::extract_json;
pub use prompt::{build_messages, TranslateRequest};
/// Re-exported so `AppState` can build one app-wide rate limiter and pass it to
/// every `LlmClient::with_limiter` (the unified-concurrency model).
pub use tokio::sync::Semaphore;

use serde::{Deserialize, Serialize};
use std::time::Duration;

/// Chat message role.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Role {
    System,
    User,
    Assistant,
}

/// One chat message.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ChatMessage {
    pub role: Role,
    pub content: String,
}

impl ChatMessage {
    pub fn system(c: impl Into<String>) -> Self {
        Self { role: Role::System, content: c.into() }
    }
    pub fn user(c: impl Into<String>) -> Self {
        Self { role: Role::User, content: c.into() }
    }
    pub fn assistant(c: impl Into<String>) -> Self {
        Self { role: Role::Assistant, content: c.into() }
    }
}

/// LLM client configuration. Secrets stay local (set from the GUI settings page).
#[derive(Debug, Clone)]
pub struct LlmConfig {
    /// OpenAI-compatible base URL (e.g. `https://api.stepfun.com/v1`).
    pub endpoint: String,
    pub model: String,
    pub api_key: String,
    /// Per-request timeout.
    pub timeout: Duration,
    /// Retries after the first attempt (so total attempts = max_retries + 1).
    pub max_retries: u32,
    pub base_delay: Duration,
    pub max_delay: Duration,
    pub temperature: Option<f64>,
    pub max_tokens: Option<u32>,
    /// Global cap on simultaneous LLM calls across ALL features (see
    /// `docs/data-contract.md` §6). Used only when the client owns its own
    /// semaphore; shared-limb clients share the app-wide one.
    pub concurrency: u64,
}

impl Default for LlmConfig {
    fn default() -> Self {
        // endpoint/model are empty by design: the runtime bakes in no provider.
        // Real values come from `felin.toml [llm]` (whose first-launch template
        // ships the factory endpoint/model) plus per-project GUI settings.
        Self {
            endpoint: String::new(),
            model: String::new(),
            api_key: String::new(),
            timeout: Duration::from_secs(120),
            max_retries: 3,
            base_delay: Duration::from_millis(500),
            max_delay: Duration::from_secs(30),
            temperature: None,
            max_tokens: None,
            concurrency: 2,
        }
    }
}
