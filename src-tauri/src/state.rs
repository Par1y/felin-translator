//! Application state shared across Tauri commands.

use felin_core::storage::{GlobalDb, ProjectDb, ProjectLock};
use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::{Arc, Mutex, MutexGuard};
use tokio::sync::watch;

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

/// Managed application state (`.manage`d on the Tauri app).
pub struct AppState {
    /// Portable data root: `<software>/felin-data` (or `FELIN_DATA_DIR`).
    pub data_dir: PathBuf,
    /// Technical parameters loaded from `<data_dir>/felin.toml`.
    pub config: felin_core::config::TechConfig,
    /// The shared glossary DB, opened at startup.
    pub global: GlobalDb,
    /// Resolved path to the OCR sidecar binary.
    pub sidecar: PathBuf,
    /// The single open project, if any.
    pub project: Mutex<Option<OpenProject>>,
    /// Cancellation handles for in-flight OCR imports, keyed by task id.
    pub tasks: Mutex<HashMap<String, watch::Sender<bool>>>,
}

impl AppState {
    pub fn projects_dir(&self) -> PathBuf {
        self.data_dir.join("projects")
    }

    /// Lock `project`, recovering from poisoning so one panicked command can't
    /// permanently brick the app (mirrors the DB layer's policy).
    pub fn project_guard(&self) -> MutexGuard<'_, Option<OpenProject>> {
        self.project.lock().unwrap_or_else(|p| p.into_inner())
    }

    /// Lock the task registry, recovering from poisoning.
    pub fn tasks_guard(&self) -> MutexGuard<'_, HashMap<String, watch::Sender<bool>>> {
        self.tasks.lock().unwrap_or_else(|p| p.into_inner())
    }
}
