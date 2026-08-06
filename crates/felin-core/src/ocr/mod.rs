//! OCR ingestion.
//!
//! Flow: spawn the `ocr-cli extract` sidecar → parse its `--progress json`
//! JSONL events → on completion read the **manifest** (single entry point) plus
//! each per-page JSON → assemble paragraphs (with cross-page merge). Also handles
//! txt import (with encoding detection).
//!
//! Layout:
//! - [`contract`] — the manifest / per-page JSON / progress-event wire types.
//! - [`ingest`] — pure paragraph assembly + cross-page merge.
//! - [`txt`] — plain-text import.
//! - [`sidecar`] — async process spawn, progress streaming, cancel/kill.
//! - [`select`] — image-directory selection (match presets, natural order, range).
//! - [`batch`] — `batch` sidecar orchestration + per-txt ingest.
//! - [`config`] — in-place read/write of the ocr-router `config.yaml`.

pub mod batch;
pub mod config;
pub mod contract;
pub mod ingest;
pub mod select;
pub mod sidecar;
pub mod txt;

use crate::error::{Error, Result};
use crate::types::IngestedParagraph;
use contract::{Manifest, PageJson, PageStatus, SUPPORTED_MANIFEST_SCHEMA};
use ingest::{build_paragraphs, PageForIngest};
use std::path::Path;

pub use contract::{Manifest as OcrManifest, ProgressEvent};
pub use sidecar::{ExtractArgs, ExtractOutcome};
pub use config::{OcrConfig, OcrEvaluatorConfig, OcrProviderConfig};

/// Default score threshold below which a page's paragraphs are flagged
/// low-score (used when the evaluator is on). Overridable per project.
pub const DEFAULT_LOW_SCORE_THRESHOLD: f64 = 0.6;

/// Outcome of reconciling a manifest and ingesting its pages.
#[derive(Debug)]
pub struct IngestResult {
    /// Assembled paragraphs, ready to persist.
    pub paragraphs: Vec<IngestedParagraph>,
    /// Page numbers whose OCR failed — candidates for `--pages` rescue re-OCR.
    pub failed_pages: Vec<i64>,
    /// Count of OK pages that contributed (incl. blank).
    pub pages_ok: usize,
    /// True if at least one page carried a real score (evaluator enabled).
    pub any_score_present: bool,
}

/// Read a file, refusing anything larger than `max` bytes (bounds memory against
/// a corrupt or hostile producer). Caps come from config.
fn read_capped(path: &Path, max: u64) -> Result<Vec<u8>> {
    let meta = std::fs::metadata(path)
        .map_err(|e| Error::contract(format!("cannot stat {}: {e}", path.display())))?;
    if meta.len() > max {
        return Err(Error::contract(format!(
            "{} is {} bytes, exceeding the {} byte cap",
            path.display(),
            meta.len(),
            max
        )));
    }
    std::fs::read(path).map_err(|e| Error::contract(format!("cannot read {}: {e}", path.display())))
}

/// Reject a manifest-supplied page filename that is absolute or escapes the
/// output directory (path-traversal / arbitrary-read guard). Only a plain
/// relative name made of normal components is allowed.
fn is_safe_page_file(file: &str) -> bool {
    use std::path::{Component, Path};
    let p = Path::new(file);
    !file.is_empty()
        && !p.is_absolute()
        && p.components().all(|c| matches!(c, Component::Normal(_) | Component::CurDir))
}

/// Read and validate a manifest file (capped at `max_manifest_bytes`).
pub fn read_manifest(path: &Path, max_manifest_bytes: u64) -> Result<Manifest> {
    let data = read_capped(path, max_manifest_bytes)?;
    let manifest: Manifest = serde_json::from_slice(&data)
        .map_err(|e| Error::contract(format!("bad manifest {}: {e}", path.display())))?;
    if manifest.schema_version != SUPPORTED_MANIFEST_SCHEMA {
        return Err(Error::contract(format!(
            "unsupported manifest schema_version {} (expected {})",
            manifest.schema_version, SUPPORTED_MANIFEST_SCHEMA
        )));
    }
    Ok(manifest)
}

/// Reconcile a manifest against its per-page JSON files in `out_dir` and
/// assemble paragraphs. Failed pages are collected (not turned into paragraphs);
/// blank pages contribute nothing. `recovered = true` marks these pages as the
/// result of a rescue re-OCR.
pub fn ingest_from_manifest(
    out_dir: &Path,
    manifest: &Manifest,
    low_score_threshold: f64,
    recovered: bool,
    sentence_enders: &[char],
    max_page_json_bytes: u64,
) -> Result<IngestResult> {
    let mut pages = Vec::new();
    let mut failed_pages = Vec::new();
    let mut any_score_present = false;
    let mut seen = std::collections::BTreeSet::new();

    for entry in &manifest.pages {
        // A page number listed twice is a contract violation (e.g. a rescue
        // re-OCR appended instead of replacing) and would silently duplicate text.
        if !seen.insert(entry.page) {
            return Err(Error::contract(format!("duplicate page {} in manifest", entry.page)));
        }
        match entry.status {
            PageStatus::Failed => failed_pages.push(entry.page),
            PageStatus::Ok => {
                if !is_safe_page_file(&entry.file) {
                    return Err(Error::contract(format!(
                        "manifest page file {:?} is not a safe relative path",
                        entry.file
                    )));
                }
                let page_path = out_dir.join(&entry.file);
                let data = read_capped(&page_path, max_page_json_bytes)?;
                let pj: PageJson = serde_json::from_slice(&data).map_err(|e| {
                    Error::contract(format!("bad page json {}: {e}", page_path.display()))
                })?;
                if pj.page != entry.page {
                    return Err(Error::contract(format!(
                        "page number mismatch: manifest entry {} vs file {} which says page {}",
                        entry.page, entry.file, pj.page
                    )));
                }
                if pj.score_present {
                    any_score_present = true;
                }
                pages.push(PageForIngest {
                    page: pj.page,
                    text: pj.text,
                    // page_score is NULL unless the page reports a present score.
                    score: if pj.score_present { pj.score } else { None },
                    quality_warning: pj.quality_warning,
                    blank: pj.blank,
                    best_score: pj.best_score,
                    fallback: pj.fallback,
                    source_file: pj.source_file.unwrap_or_else(|| manifest.source.clone()),
                    recovered,
                });
            }
        }
    }

    let pages_ok = pages.len();
    let paragraphs = build_paragraphs(&pages, low_score_threshold, sentence_enders);
    Ok(IngestResult { paragraphs, failed_pages, pages_ok, any_score_present })
}
