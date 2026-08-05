//! OCR backend contract: the manifest, per-page JSON, and JSONL progress-event
//! shapes produced by the `ocr-cli extract` command (plan §2/§3). The
//! translation app reads the **manifest as the single entry point** and never
//! parses stderr or guesses page numbers from filenames.
//!
//! These types deserialize exactly the documented contract; unknown extra
//! fields are ignored, and optional fields default so a minimal-but-valid
//! producer still parses.

use serde::{Deserialize, Serialize};

/// JSON Schema version this build understands for manifests and per-page JSON.
pub const SUPPORTED_MANIFEST_SCHEMA: i64 = 1;

/// Per-page terminal status. (A run may be *cancelled* at the manifest level,
/// but individual completed pages are always `ok` or `failed`.)
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum PageStatus {
    Ok,
    Failed,
}

/// One page's structured OCR result, written to `<base>-%04d.json` for both
/// successful and failed pages.
#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct PageJson {
    pub page: i64,
    pub status: PageStatus,
    #[serde(default)]
    pub text: String,
    #[serde(default)]
    pub score: Option<f64>,
    /// When false, `score` is meaningless and the paragraph's `page_score` is NULL.
    #[serde(default)]
    pub score_present: bool,
    #[serde(default)]
    pub provider: Option<String>,
    #[serde(default)]
    pub fallback: bool,
    #[serde(default)]
    pub quality_warning: bool,
    /// A near-blank page short-circuited by the OCR backend (no OCR call).
    #[serde(default)]
    pub blank: bool,
    #[serde(default)]
    pub best_score: Option<f64>,
    #[serde(default)]
    pub error: Option<String>,
    #[serde(default)]
    pub source_file: Option<String>,
    #[serde(default)]
    pub image: Option<String>,
}

/// A page entry inside the manifest's `pages` array.
#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct ManifestPageEntry {
    pub page: i64,
    pub status: PageStatus,
    #[serde(default)]
    pub score: Option<f64>,
    /// Filename (relative to the output dir) of this page's JSON.
    pub file: String,
    #[serde(default)]
    pub error: Option<String>,
}

/// The per-input manifest — the translation app's single read entry point.
#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct Manifest {
    pub schema_version: i64,
    pub source: String,
    #[serde(rename = "type")]
    pub source_type: String,
    pub pages_total: i64,
    pub pages_attempted: i64,
    pub pages_ok: i64,
    pub pages_failed: i64,
    #[serde(default)]
    pub window_size: Option<i64>,
    #[serde(default)]
    pub page_workers: Option<i64>,
    #[serde(default)]
    pub evaluator_enabled: Option<bool>,
    #[serde(default)]
    pub provider: Option<String>,
    #[serde(default)]
    pub started_at: Option<String>,
    #[serde(default)]
    pub finished_at: Option<String>,
    #[serde(default)]
    pub elapsed_ms: Option<i64>,
    /// Present as `"cancelled"` when flushed on SIGTERM (plan §5).
    #[serde(default)]
    pub status: Option<String>,
    #[serde(default)]
    pub pages: Vec<ManifestPageEntry>,
}

/// A single line of the `--progress json` JSONL stream. Concurrent producers may
/// interleave `page` events out of order, so every line is self-contained
/// (`page`/`done`/`total`).
#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(tag = "event", rename_all = "lowercase")]
pub enum ProgressEvent {
    Start {
        source: String,
        pages_total: i64,
    },
    Page {
        page: i64,
        status: PageStatus,
        #[serde(default)]
        score: Option<f64>,
        #[serde(default)]
        error: Option<String>,
        done: i64,
        total: i64,
    },
    Done {
        pages_ok: i64,
        pages_failed: i64,
        #[serde(default)]
        manifest: Option<String>,
    },
}
