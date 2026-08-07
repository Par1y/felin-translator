//! Application state shared across Tauri commands.

use felin_core::config::PromptConfig;
use felin_core::llm::Semaphore;
use felin_core::storage::{GlobalDb, ProjectDb, ProjectLock};
use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::{Arc, Mutex, MutexGuard};
use tokio::sync::{watch, Notify};

/// The currently-open project. Holds the single-open lock for its lifetime.
pub struct OpenProject {
    pub slug: String,
    pub name: String,
    pub root: PathBuf,
    /// Shared so an in-flight import can keep writing to *this* project even if
    /// the user switches/closes the open project meanwhile.
    pub db: Arc<ProjectDb>,
    // Held purely for its RAII effect: dropping it releases the single-open
    // lock. Never read, hence the allow.
    #[allow(dead_code)]
    pub lock: ProjectLock,
}

/// The active translation run, if any. Lives until the run's thread finishes
/// and deregisters itself (RAII), mirroring [`AppState::tasks`].
pub struct TranslationRun {
    pub task_id: String,
    /// Setting `true` requests a stop (graceful, or aborting per
    /// `stop_aborts_inflight`); the run thread ends and clears itself.
    pub stop: watch::Sender<bool>,
    /// Wake the scheduler so it re-scans the window immediately after a retry /
    /// re-translate command re-queued TUs.
    pub wake: Arc<Notify>,
}

/// Managed application state (`.manage`d on the Tauri app).
pub struct AppState {
    /// Portable data root: `<software>/felin-data` (or `FELIN_DATA_DIR`).
    pub data_dir: PathBuf,
    /// Technical parameters loaded from `<data_dir>/felin.toml`.
    pub config: felin_core::config::TechConfig,
    /// The runtime-effective `[prompt]` templates (from `felin.toml`). Seeded
    /// from `config.prompt` at startup; `set_prompt_config` updates it in place
    /// so edits take effect without a restart (translation / name extraction
    /// read this, not `config.prompt`).
    pub prompt: Mutex<PromptConfig>,
    /// The shared glossary DB, opened at startup.
    pub global: GlobalDb,
    /// OCR sidecar binary (`ocr-cli`) resolved from *user-managed* sources:
    /// `[sidecar] bin` in `felin.toml`, else `FELIN_SIDECAR`. `None` when the
    /// user hasn't configured one — commands then report a clear "not
    /// configured" error instead of guessing a location.
    pub sidecar: Option<PathBuf>,
    /// Sidecar's config file (`config.yaml`, holds OCR providers' keys),
    /// resolved from `[sidecar] config` in `felin.toml`, else
    /// `FELIN_SIDECAR_CONFIG`. `None` → don't pass `-c`; `ocr-cli` falls back
    /// to its own default `config.yaml`.
    pub sidecar_config: Option<PathBuf>,
    /// The single open project, if any.
    pub project: Mutex<Option<OpenProject>>,
    /// Cancellation handles for in-flight OCR imports, keyed by task id.
    pub tasks: Mutex<HashMap<String, watch::Sender<bool>>>,
    /// The active translation run, if any.
    pub translation: Mutex<Option<TranslationRun>>,
    /// App-wide LLM rate limiter: every `LlmClient::with_limiter` shares this
    /// `Arc`, so translation workers, name extraction, auto-tag and connection
    /// tests all queue on one global concurrency cap (see
    /// `docs/data-contract.md` §6).
    pub llm_limiter: Arc<Semaphore>,
}

impl AppState {
    pub fn projects_dir(&self) -> PathBuf {
        self.data_dir.join("projects")
    }

    /// Build the app-wide LLM rate limiter sized by `felin.toml [llm] concurrency`.
    pub fn llm_limiter(concurrency: u64) -> Arc<Semaphore> {
        Arc::new(Semaphore::new((concurrency.clamp(1, 16)) as usize))
    }

    /// Lock `project`, recovering from poisoning so one panicked command can't
    /// permanently brick the app (mirrors the DB layer's policy).
    pub fn project_guard(&self) -> MutexGuard<'_, Option<OpenProject>> {
        self.project.lock().unwrap_or_else(|p| p.into_inner())
    }

    /// Lock the runtime-effective prompt templates, recovering from poisoning.
    pub fn prompt_config(&self) -> MutexGuard<'_, PromptConfig> {
        self.prompt.lock().unwrap_or_else(|p| p.into_inner())
    }

    /// Lock the task registry, recovering from poisoning.
    pub fn tasks_guard(&self) -> MutexGuard<'_, HashMap<String, watch::Sender<bool>>> {
        self.tasks.lock().unwrap_or_else(|p| p.into_inner())
    }

    /// Lock the translation-run slot, recovering from poisoning.
    pub fn translation_guard(&self) -> MutexGuard<'_, Option<TranslationRun>> {
        self.translation.lock().unwrap_or_else(|p| p.into_inner())
    }
}
