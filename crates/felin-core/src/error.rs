//! Error type shared across `felin-core`.
//!
//! Library code returns typed [`Error`]s (via [`Result`]); the Tauri command
//! layer converts these into strings for the frontend. Variants that carry an
//! actionable distinction for the UI (a *hard* OCR failure vs. a *partial*
//! success, a locked project, a too-new schema) are modeled explicitly rather
//! than collapsed into a generic message.

use std::path::PathBuf;

/// Convenience alias for `Result<T, felin_core::Error>`.
pub type Result<T> = std::result::Result<T, Error>;

/// All error conditions produced by the core.
#[derive(Debug, thiserror::Error)]
#[non_exhaustive]
pub enum Error {
    #[error("I/O error: {0}")]
    Io(#[from] std::io::Error),

    #[error("SQLite error: {0}")]
    Sqlite(#[from] rusqlite::Error),

    #[error("connection pool error: {0}")]
    Pool(#[from] r2d2::Error),

    #[error("JSON error: {0}")]
    Json(#[from] serde_json::Error),

    #[error("CSV error: {0}")]
    Csv(#[from] csv::Error),

    /// A migration step failed; the transaction was rolled back.
    #[error("migration failed at version {version}: {message}")]
    Migration { version: i64, message: String },

    /// The on-disk database is newer than this build understands. We refuse to
    /// open it rather than risk corrupting a forward-versioned DB.
    #[error("database schema v{found} is newer than supported (max v{supported_max}); upgrade the app")]
    SchemaTooNew { found: i64, supported_max: i64 },

    /// Another process already holds the project's single-open lock.
    #[error("project is already open in another window: {path}")]
    ProjectLocked { path: PathBuf },

    /// Text encoding could not be detected/decoded.
    #[error("text encoding error: {detail}")]
    Encoding { detail: String },

    /// The OCR sidecar could not be spawned or its I/O failed.
    #[error("OCR sidecar error: {detail}")]
    Sidecar { detail: String },

    /// The ocr-router config.yaml could not be parsed or written back.
    #[error("OCR config error: {detail}")]
    OcrConfig { detail: String },

    /// The OCR sidecar exited with a hard/fatal status (exit code 20): no
    /// usable output was produced. Distinct from a *partial* success (exit 10),
    /// which is not an error — failed pages are recorded for later rescue.
    #[error("OCR failed (exit {exit_code}): {message}")]
    OcrFatal { exit_code: i32, message: String },

    /// The operation was cancelled (e.g. the sidecar received SIGTERM → exit 130).
    #[error("operation cancelled")]
    Cancelled,

    /// A manifest or per-page JSON violated the documented contract.
    #[error("OCR contract violation: {detail}")]
    Contract { detail: String },

    /// A project-archive (export/import) operation failed.
    #[error("project archive error: {detail}")]
    Archive { detail: String },

    /// An LLM request failed (after retries, or a fatal client error).
    #[error("LLM error: {detail}")]
    Llm { detail: String },

    /// A requested entity did not exist.
    #[error("not found: {what}")]
    NotFound { what: String },

    /// Caller passed invalid input.
    #[error("invalid input: {detail}")]
    InvalidInput { detail: String },
}

impl Error {
    pub(crate) fn migration(version: i64, message: impl Into<String>) -> Self {
        Error::Migration { version, message: message.into() }
    }

    pub(crate) fn sidecar(detail: impl Into<String>) -> Self {
        Error::Sidecar { detail: detail.into() }
    }

    pub(crate) fn ocr_config(detail: impl Into<String>) -> Self {
        Error::OcrConfig { detail: detail.into() }
    }

    pub(crate) fn contract(detail: impl Into<String>) -> Self {
        Error::Contract { detail: detail.into() }
    }

    pub(crate) fn archive(detail: impl Into<String>) -> Self {
        Error::Archive { detail: detail.into() }
    }

    pub(crate) fn llm(detail: impl Into<String>) -> Self {
        Error::Llm { detail: detail.into() }
    }
}
