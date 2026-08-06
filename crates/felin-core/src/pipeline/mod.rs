//! Translation pipeline (plan step 8).
//!
//! Controlled multi-threading (tokio): a scheduler owns the chapter-activation
//! window `W` and feeds eligible TU ids into a bounded channel; a worker pool
//! `N` (default 2, doubling as the LLM rate limit) pulls, CAS-claims from the
//! database (the single source of truth for TU status), translates, and writes
//! back. The TU-level state gate enforces the core invariant — **at most one
//! writer per TU at any instant**: workers claim only `pending/queued`, and a
//! write-back is discarded (atomically) if the TU left `translating` meanwhile
//! (e.g. the user started reviewing it). `reviewing`/`approved`/`exported` TUs
//! are never touched by the pipeline.
//!
//! Translation-memory dedup by normalized source hash skips the LLM for
//! repeated sources. Stop is graceful (in-flight complete) unless
//! `stop_aborts_inflight`; crash recovery marks stale `translating` →
//! `interrupted` and re-queues them on the next run. Retry is explicit
//! (user-triggered): failed TUs wait for the retry button.

pub mod prompt;
mod runner;

pub use prompt::{default_guidelines, glossary_block, truncate_chars};

use crate::error::Result;
use crate::llm::{LlmClient, TranslateRequest};
use crate::storage::ProjectDb;
use serde::{Deserialize, Serialize};
use std::future::Future;
use std::sync::Arc;
use tokio::sync::{mpsc, watch, Notify};

/// A translator backend: one prompt in, draft text out. Implemented by
/// [`LlmTranslator`] (HTTP) and by mocks in tests.
///
/// The return type is an explicit `impl Future + Send` (not `async fn`):
/// `async fn` in a trait neither makes it dyn-compatible (opaque return type,
/// per the Rust reference) nor guarantees the future is `Send` when called
/// through a generic bound — both are needed here (worker pool spawns tasks).
/// Implementations may still write `async fn`, which the compiler checks
/// against the `+ Send` bound.
pub trait Translator: Send + Sync {
    fn translate(&self, req: &TranslateRequest) -> impl Future<Output = Result<String>> + Send;
}

/// Production translator backed by the OpenAI-compatible [`LlmClient`].
pub struct LlmTranslator {
    pub client: LlmClient,
}

impl Translator for LlmTranslator {
    async fn translate(&self, req: &TranslateRequest) -> Result<String> {
        self.client.translate(req).await
    }
}

/// Runtime parameters for one pipeline run. `workers`/`window`/`memory_dedup`/
/// `stop_aborts_inflight` come from project settings (GUI); the *technical*
/// limits come from `felin.toml [pipeline]`.
#[derive(Debug, Clone)]
pub struct RunConfig {
    /// Worker pool size N (1–8), doubling as the LLM rate limit.
    pub workers: usize,
    /// Chapter activation window W (1–5).
    pub window: usize,
    /// Translation-memory dedup by normalized source hash.
    pub memory_dedup: bool,
    /// true → stop aborts in-flight TUs (→ `interrupted`); false → they complete.
    pub stop_aborts_inflight: bool,
    /// Bound on the scheduler's in-memory eligible-TU buffer (≥ workers).
    pub queue_capacity: usize,
    /// Injected previous-approved context is truncated to this many chars.
    pub context_max_chars: usize,
    /// Cap on the injected 总则 length (chars).
    pub guidelines_max_chars: usize,
}

/// Progress events emitted to the UI.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "event", rename_all = "snake_case")]
pub enum PipelineEvent {
    Started { total_tus: usize },
    TuStart { tu_id: i64 },
    TuDone { tu_id: i64, memory_hit: bool },
    TuFailed { tu_id: i64, error: String },
    Stopped,
    Finished { total_tus: usize },
}

/// Run one translation pass to completion (or stop).
///
/// * `db` — the project DB (single source of truth for TU status *and* the
///   enabled small-glossary entries used to compile the prompt-injection name
///   matcher).
/// * `translator` — the LLM backend. Generic (not `dyn`): `async fn` in a trait
///   makes it dyn-incompatible per the Rust reference, so `T` is monomorphized.
/// * `stop` — setting this `true` stops claiming; in-flight behavior follows
///   `cfg.stop_aborts_inflight`.
/// * `wake` — notify the scheduler that new TUs became claimable (e.g. after a
///   retry command) while the pipeline is waiting.
/// * `events` — progress events.
pub async fn run_pipeline<T: Translator + 'static>(
    db: Arc<ProjectDb>,
    translator: Arc<T>,
    cfg: RunConfig,
    stop: watch::Receiver<bool>,
    wake: Arc<Notify>,
    events: mpsc::UnboundedSender<PipelineEvent>,
) -> Result<()> {
    let mut stop = stop;
    runner::run(db, translator, cfg, &mut stop, wake, events).await
}

/// Normalize a source text for translation-memory hashing: NFKC + collapse all
/// whitespace (so paragraph boundaries and OCR spacing don't defeat dedup).
pub fn normalize_source(s: &str) -> String {
    crate::names::normalize(s)
        .split_whitespace()
        .collect::<Vec<_>>()
        .join(" ")
}

/// SHA-256 (hex) of [`normalize_source`] — the translation-memory key.
pub fn source_hash(s: &str) -> String {
    use sha2::{Digest, Sha256};
    let mut h = Sha256::new();
    h.update(normalize_source(s).as_bytes());
    format!("{:x}", h.finalize())
}
