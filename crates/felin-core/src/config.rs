//! Layered configuration.
//!
//! Non-technical, user-facing options live in the GUI (persisted per project or
//! globally). *Technical* parameters live in [`TechConfig`], loaded from an
//! editable `felin.toml` next to the software so developers / advanced users can
//! tune them without recompiling. Missing fields fall back to the defaults here.

use serde::{Deserialize, Serialize};

/// Default chapter-heading recognizer pattern (users may add/replace patterns).
/// A marker must be followed by a separator or end-of-line, so a sentence that
/// merely starts like a heading is not misread as one.
pub const DEFAULT_CHAPTER_PATTERN: &str = concat!(
    r"^(?:",
    r"第[0-9０-９〇一二三四五六七八九十百千两]+\s*[章話话回節节巻卷部篇編编幕]",
    r"|序章|終章|终章|序幕|終幕|终幕|プロローグ|エピローグ",
    r"|序言|前言|後記|后记|あとがき|まえがき",
    r"|[Cc]hapter\s+\d+",
    r")(?:[\s　:：・.。、!！?？」』)）\-—–]|$)",
);

/// Default sentence-terminating characters for cross-page paragraph merging.
pub const DEFAULT_SENTENCE_ENDERS: &str = "。！？」』.!?…”）】〕》";

/// Technical configuration (editable `felin.toml`).
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
#[serde(default)]
pub struct TechConfig {
    pub seg: SegConfig,
    pub ocr: OcrConfig,
    pub names: NamesConfig,
    pub llm: LlmDefaults,
    pub sidecar: SidecarConfig,
    pub db: DbConfig,
    pub import: ImportConfig,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct SegConfig {
    /// Default TU block size (characters); a soft target. Also settable per project in the GUI.
    pub default_block_size: usize,
    pub fallback_chapter_title: String,
    /// A candidate heading line longer than this (chars) is never treated as a heading.
    pub heading_max_chars: usize,
    /// Regex patterns; a line matching any is a chapter heading.
    pub chapter_heading_patterns: Vec<String>,
    /// Characters that end a sentence (a page-tail paragraph not ending in one is
    /// merged with the next page's first paragraph).
    pub sentence_enders: String,
}

impl Default for SegConfig {
    fn default() -> Self {
        Self {
            default_block_size: 3000,
            fallback_chapter_title: "正文".to_string(),
            heading_max_chars: 40,
            chapter_heading_patterns: vec![DEFAULT_CHAPTER_PATTERN.to_string()],
            sentence_enders: DEFAULT_SENTENCE_ENDERS.to_string(),
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct OcrConfig {
    /// Pages scoring below this (when the evaluator is on) are flagged low-score.
    pub low_score_threshold: f64,
    /// Cap on a single per-page JSON file read (bytes).
    pub max_page_json_bytes: u64,
    /// Cap on a manifest file read (bytes).
    pub max_manifest_bytes: u64,
}

impl Default for OcrConfig {
    fn default() -> Self {
        Self {
            low_score_threshold: 0.6,
            max_page_json_bytes: 16 * 1024 * 1024,
            max_manifest_bytes: 64 * 1024 * 1024,
        }
    }
}

/// Proper-noun matching tunables.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct NamesConfig {
    /// OCR-typo tolerance: edit distance ≤ this is flagged "suspect" (never auto-applied).
    pub fuzzy_max_distance: usize,
}

impl Default for NamesConfig {
    fn default() -> Self {
        Self { fuzzy_max_distance: 1 }
    }
}

/// SQLite connection tuning.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct DbConfig {
    pub read_pool_size: u32,
    pub busy_timeout_ms: u32,
}

impl Default for DbConfig {
    fn default() -> Self {
        Self { read_pool_size: 8, busy_timeout_ms: 5000 }
    }
}

/// Import limits.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct ImportConfig {
    /// Cap on a single txt/CSV file read (bytes).
    pub max_file_bytes: u64,
}

impl Default for ImportConfig {
    fn default() -> Self {
        Self { max_file_bytes: 128 * 1024 * 1024 }
    }
}

/// Default LLM transport tunables (endpoint/model/key are user-facing, set in the GUI).
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct LlmDefaults {
    pub timeout_secs: u64,
    pub max_retries: u32,
    pub base_delay_ms: u64,
    pub max_delay_secs: u64,
    pub temperature: Option<f64>,
    pub max_tokens: Option<u32>,
}

impl Default for LlmDefaults {
    fn default() -> Self {
        Self {
            timeout_secs: 120,
            max_retries: 3,
            base_delay_ms: 500,
            max_delay_secs: 30,
            temperature: None,
            max_tokens: None,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct SidecarConfig {
    pub cancel_grace_secs: u64,
    pub poll_ms: u64,
}

impl Default for SidecarConfig {
    fn default() -> Self {
        Self { cancel_grace_secs: 8, poll_ms: 150 }
    }
}

impl TechConfig {
    /// Parse from a TOML string (missing fields use defaults).
    pub fn from_toml_str(s: &str) -> Result<Self, String> {
        toml::from_str(s).map_err(|e| e.to_string())
    }

    /// Serialize to a TOML string (for writing a default `felin.toml`).
    pub fn to_toml_string(&self) -> String {
        toml::to_string_pretty(self).unwrap_or_default()
    }

    /// Sentence-ender characters as a `Vec<char>`.
    pub fn sentence_enders(&self) -> Vec<char> {
        self.seg.sentence_enders.chars().collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn default_roundtrips_through_toml() {
        let s = TechConfig::default().to_toml_string();
        let back = TechConfig::from_toml_str(&s).unwrap();
        assert_eq!(back.seg.default_block_size, 3000);
        assert_eq!(back.ocr.low_score_threshold, 0.6);
        assert_eq!(back.seg.sentence_enders, DEFAULT_SENTENCE_ENDERS);
        assert_eq!(back.seg.chapter_heading_patterns, vec![DEFAULT_CHAPTER_PATTERN.to_string()]);
        assert_eq!(back.llm.max_retries, 3);
    }

    #[test]
    fn partial_toml_fills_missing_with_defaults() {
        let c = TechConfig::from_toml_str("[ocr]\nlow_score_threshold = 0.8\n").unwrap();
        assert_eq!(c.ocr.low_score_threshold, 0.8);
        assert_eq!(c.seg.default_block_size, 3000);
        assert_eq!(c.sidecar.cancel_grace_secs, 8);
    }
}

