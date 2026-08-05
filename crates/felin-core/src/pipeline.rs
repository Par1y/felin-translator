//! Translation pipeline (later milestone — step 8).
//!
//! Controlled multi-threading (tokio): a global priority queue ordered by
//! `(chapter.ord, tu.ord)`, a configurable worker pool `N` (default 2, doubling
//! as the LLM rate limit), and a chapter activation window `W` (default 1).
//! Per-TU state gate enforcing the core invariant — **at most one writer per TU
//! at any instant**: workers CAS `pending/queued → translating`; a TU in
//! `reviewing` is never touched by a worker (incl. auto-retry). Translation
//! memory dedup by normalized source hash. Stop/resume/retry with crash
//! recovery (`translating` → `interrupted` on startup).
//!
//! Not implemented in the foundation milestone.
