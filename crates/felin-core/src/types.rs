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
}

/// An extracted proper-noun candidate awaiting review (`extracted_names` table).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ExtractedName {
    pub id: i64,
    pub japanese: String,
    pub matched_name_id: Option<i64>,
    pub candidate_chinese: Option<String>,
    pub status: ExtractedNameStatus,
    pub notes: Option<String>,
}
