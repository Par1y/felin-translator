//! Shared domain types: status enums (stored as TEXT in SQLite, serialized as
//! strings to the frontend) and lightweight row structs returned by the storage
//! layer. The OCR *contract* types (manifest, per-page JSON, progress events)
//! live in [`crate::ocr::contract`] next to their parser.

use serde::{Deserialize, Serialize};
use uuid::Uuid;

/// Generates a C-like enum that round-trips through both `serde` (string) and
/// `rusqlite` (TEXT), using the exact wire string given per variant. Keeping the
/// string explicit (rather than a `rename_all` rule) guarantees the on-disk and
/// on-wire representations match the implementation plan verbatim.
macro_rules! sql_enum {
    (
        $(#[$meta:meta])*
        $vis:vis enum $name:ident { $( $variant:ident => $repr:literal ),+ $(,)? }
    ) => {
        $(#[$meta])*
        #[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
        $vis enum $name {
            $( #[serde(rename = $repr)] $variant ),+
        }

        impl $name {
            /// The canonical string form (used for both storage and the wire).
            pub const fn as_str(&self) -> &'static str {
                match self { $( $name::$variant => $repr ),+ }
            }
            /// Every variant, in declaration order.
            pub const ALL: &'static [$name] = &[ $( $name::$variant ),+ ];
        }

        impl ::std::fmt::Display for $name {
            fn fmt(&self, f: &mut ::std::fmt::Formatter<'_>) -> ::std::fmt::Result {
                f.write_str(self.as_str())
            }
        }

        impl ::std::str::FromStr for $name {
            type Err = $crate::error::Error;
            fn from_str(s: &str) -> ::std::result::Result<Self, Self::Err> {
                match s {
                    $( $repr => Ok($name::$variant), )+
                    other => Err($crate::error::Error::InvalidInput {
                        detail: format!(concat!("invalid ", stringify!($name), " value: {:?}"), other),
                    }),
                }
            }
        }

        impl ::rusqlite::types::ToSql for $name {
            fn to_sql(&self) -> ::rusqlite::Result<::rusqlite::types::ToSqlOutput<'_>> {
                Ok(::rusqlite::types::ToSqlOutput::from(self.as_str()))
            }
        }

        impl ::rusqlite::types::FromSql for $name {
            fn column_result(v: ::rusqlite::types::ValueRef<'_>) -> ::rusqlite::types::FromSqlResult<Self> {
                let s = v.as_str()?;
                <$name as ::std::str::FromStr>::from_str(s).map_err(|_| {
                    ::rusqlite::types::FromSqlError::Other(
                        format!(concat!("invalid ", stringify!($name), " value: {:?}"), s).into(),
                    )
                })
            }
        }
    };
}

sql_enum! {
    /// Lifecycle of a glossary entry in the global DB.
    pub enum NameStatus { Imported => "imported", Draft => "draft", Confirmed => "confirmed" }
}

sql_enum! {
    /// Chapter-level coarse status.
    pub enum ChapterStatus {
        Pending => "pending", Translating => "translating", Reviewing => "reviewing", Done => "done"
    }
}

sql_enum! {
    /// Translation-Unit state machine (drives the pipeline; see [`crate::pipeline`]).
    pub enum TuStatus {
        Pending => "pending",
        Queued => "queued",
        Translating => "translating",
        Translated => "translated",
        Reviewing => "reviewing",
        Approved => "approved",
        Exported => "exported",
        Interrupted => "interrupted",
        FailedRetryable => "failed_retryable",
        FailedPermanent => "failed_permanent",
    }
}

sql_enum! {
    /// Status of a single translation attempt row.
    pub enum TranslationStatus {
        Draft => "draft", MemoryHit => "memory_hit", Failed => "failed",
    }
}

sql_enum! {
    /// Review status of an extracted name candidate (per-project).
    pub enum ExtractedNameStatus {
        New => "new", Matched => "matched", Confirmed => "confirmed", Rejected => "rejected",
    }
}

sql_enum! {
    /// Per-paragraph OCR quality status recorded at ingest.
    pub enum OcrParagraphStatus {
        Ok => "ok",
        LowScore => "low_score",
        Suspect => "suspect",
        PageFailedRecovered => "page_failed_recovered",
    }
}

/// A chapter row (`chapters` table). `ord` is the sort key (the plan calls this
/// "order"; we avoid the SQL reserved word).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Chapter {
    pub id: i64,
    pub title: String,
    pub ord: i64,
    pub status: ChapterStatus,
}

/// A persisted paragraph row (`paragraphs` table) — the stable minimal unit,
/// keyed by UUID so re-segmentation can reuse it.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Paragraph {
    pub id: String,
    pub chapter_id: i64,
    pub ord: i64,
    pub text: String,
    /// Starting page for cross-page-merged paragraphs; `None` for txt imports.
    pub page_num: Option<i64>,
    /// `None` when the evaluator was disabled / the page carried no score.
    pub page_score: Option<f64>,
    pub ocr_status: OcrParagraphStatus,
    /// JSON blob: `quality_warning` / `best_score` / `fallback` / source pointer.
    pub ocr_meta: Option<serde_json::Value>,
    pub source_file: Option<String>,
}

/// A paragraph produced by OCR/txt ingest, before it is assigned a chapter and
/// ordinal. Carries a stable UUID from the moment of creation.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct IngestedParagraph {
    pub id: Uuid,
    pub text: String,
    /// Starting page (first page contributing text to this paragraph).
    pub page_num: Option<i64>,
    pub source_file: String,
    pub page_score: Option<f64>,
    pub ocr_status: OcrParagraphStatus,
    pub ocr_meta: serde_json::Value,
}

impl IngestedParagraph {
    /// Create a new ingested paragraph with a fresh UUID.
    pub fn new(
        text: String,
        page_num: Option<i64>,
        source_file: String,
        page_score: Option<f64>,
        ocr_status: OcrParagraphStatus,
        ocr_meta: serde_json::Value,
    ) -> Self {
        Self { id: Uuid::new_v4(), text, page_num, source_file, page_score, ocr_status, ocr_meta }
    }
}

/// A Translation Unit row (`tus` table): a group of adjacent paragraphs (by
/// UUID) that are translated together.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Tu {
    pub id: i64,
    pub chapter_id: i64,
    pub paragraph_ids: Vec<String>,
    pub ord: i64,
    /// Combined character length at aggregation time (informational).
    pub budget: Option<i64>,
    pub status: TuStatus,
}

/// A translation row (`translations` table) — one row per TU. `llm_text` is the
/// model's raw output, kept forever; `final_text` is the editable, human-approved
/// draft. Older drafts accumulate in the `attempts` JSON array.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Translation {
    pub id: i64,
    pub tu_id: i64,
    pub status: TranslationStatus,
    /// Normalized-source hash for translation-memory dedup.
    pub source_hash: Option<String>,
    pub llm_text: Option<String>,
    pub final_text: Option<String>,
    /// Per-item manual guidance, injected on re-translate.
    pub instruction: Option<String>,
    pub attempts: Vec<String>,
    pub error: Option<String>,
    pub created_at: String,
    pub updated_at: String,
}

/// User-facing translation settings (per project; N/W/memory toggle live in the
/// GUI). Technical tuning lives in `felin.toml [pipeline]`.
#[derive(Debug, Clone, Copy, Serialize, Deserialize)]
pub struct TranslationSettings {
    /// Translation concurrency — worker pool size N (1–8). Doubles as the LLM rate limit.
    pub workers: i64,
    /// Chapter activation window W (1–5): how many chapters' TUs may be in flight.
    pub window: i64,
    /// Translation-memory dedup by normalized source hash.
    pub memory_dedup: bool,
    /// Stop behavior: false → in-flight TUs complete; true → they are interrupted.
    pub stop_aborts_inflight: bool,
}

impl Default for TranslationSettings {
    fn default() -> Self {
        Self { workers: 2, window: 1, memory_dedup: true, stop_aborts_inflight: false }
    }
}

/// One proper noun a TU's source matched against the project's *enabled* small
/// glossary — what translation prompt injection applied. Computed at query time
/// (no persistence) so existing data shows immediately. `chinese` is `None`
/// when the entry carries no non-blank Chinese rendering.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct MatchedName {
    /// The entry's canonical japanese form (as it matched, NFKC-normalized).
    pub japanese: String,
    /// The entry's Chinese rendering (None when unset).
    pub chinese: Option<String>,
}

/// A TU joined with its translation row and source text — the editable
/// 原文/译文 card the review screen drives from.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TuWithTranslation {
    pub id: i64,
    pub ord: i64,
    pub budget: Option<i64>,
    pub status: TuStatus,
    pub translation_status: Option<TranslationStatus>,
    /// Effective source text: the user's `source_override` if set, else the
    /// concatenated paragraph text.
    pub source: String,
    /// The enabled small-glossary entries `source` hits, de-duplicated by entry
    /// id in first-occurrence order (empty when none matched).
    pub matched_names: Vec<MatchedName>,
    pub final_text: Option<String>,
    pub llm_text: Option<String>,
    pub instruction: Option<String>,
    pub error: Option<String>,
    pub source_hash: Option<String>,
}

/// A glossary entry from the global DB (`names` table).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct GlossaryName {
    pub id: i64,
    pub japanese: String,
    pub chinese: Option<String>,
    pub english: Option<String>,
    pub category: Option<String>,
    pub notes: Option<String>,
    pub source: Option<String>,
    pub status: NameStatus,
    /// JSON-decoded tag array (user / LLM / source generated: 人名, 地名…).
    pub tags: Vec<String>,
    /// Per-entry enable toggle (translation only injects enabled entries).
    pub enabled: bool,
}

/// An entry in the project's small glossary (`glossary_entries`).
///
/// Self-contained snapshot (japanese/chinese/english/category/tags copied at
/// add-time) so a project archive carries its own glossary; `name_global_id`
/// records provenance in the global big glossary. Translation prompt injection
/// reads only `enabled = true` entries from here.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct GlossaryEntry {
    pub id: i64,
    pub name_global_id: Option<i64>,
    pub japanese: String,
    pub chinese: Option<String>,
    pub english: Option<String>,
    pub category: Option<String>,
    pub tags: Vec<String>,
    pub enabled: bool,
    pub notes: Option<String>,
    pub created_at: String,
    pub updated_at: String,
}

/// GUI-managed OCR import options (per project). Deep technical tuning (score
/// thresholds, byte caps, sidecar path…) stays in `felin.toml`.
#[derive(Debug, Clone, Copy, Serialize, Deserialize)]
pub struct OcrSettings {
    /// Concurrent image workers passed to `batch --workers` (images only;
    /// PDF pages run serially in the sidecar).
    pub batch_workers: i64,
    /// Whether to recurse into subdirectories when scanning an image folder.
    pub batch_recursive: bool,
}

impl Default for OcrSettings {
    fn default() -> Self {
        Self { batch_workers: 4, batch_recursive: false }
    }
}

/// Preview of scanning an image directory against an
/// [`crate::ocr::select::ImageMatchRule`] — what the user confirms before the
/// `batch` import runs.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct FileSelection {
    /// Total image files in the directory (before rule filtering).
    pub total: usize,
    /// Files that matched the rule (in natural reading order).
    pub matched: usize,
    /// Matched file basenames (ordered).
    pub names: Vec<String>,
    /// Sum of matched file sizes (bytes) where stat succeeds.
    pub bytes: u64,
}

/// Result of a deterministic 译文导出 (汉化 .txt + CSV).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TranslationExport {
    pub txt_path: String,
    pub csv_path: String,
    /// Number of TUs with a non-empty translation that were written.
    pub tus: usize,
}

/// An extracted proper-noun candidate awaiting review (`extracted_names` table).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ExtractedName {
    pub id: i64,
    pub japanese: String,
    pub matched_name_id: Option<i64>,
    pub candidate_chinese: Option<String>,
    pub status: ExtractedNameStatus,
    /// Category tags proposed by the LLM / edited by the user (人名、地名…).
    /// Persisted in `extracted_names.tags` (JSON array).
    pub tags: Vec<String>,
    pub notes: Option<String>,
}
