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
    pub pipeline: PipelineTuning,
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

/// Sidecar (OCR backend) resolution. The *binary and its config location are
/// user-managed*: set them here (`felin.toml [sidecar]`), via `FELIN_SIDECAR` /
/// `FELIN_SIDECAR_CONFIG` for quick dev runs, or rely on the production bundle
/// (next to the executable). The app never guesses a location itself beyond
/// those fallbacks.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct SidecarConfig {
    pub cancel_grace_secs: u64,
    pub poll_ms: u64,
    /// Optional path to the OCR sidecar binary (`ocr-cli`). Empty → env
    /// override, then `<exe dir>/ocr-cli` (bundled sidecar).
    pub bin: Option<String>,
    /// Optional path to the sidecar config file (`config.yaml`, holds provider
    /// keys). Empty → env override, then `<exe dir>/config.yaml`.
    pub config: Option<String>,
}

impl Default for SidecarConfig {
    fn default() -> Self {
        Self { cancel_grace_secs: 8, poll_ms: 150, bin: None, config: None }
    }
}

/// Translation-pipeline tuning (technical; user-facing knobs like workers N and
/// window W live in project settings / the GUI).
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct PipelineTuning {
    /// Bound on the scheduler's in-memory eligible-TU buffer (must be ≥ workers).
    pub queue_capacity: usize,
    /// Injected previous-approved-TU context is truncated to this many chars
    /// (truncate context, never the source).
    pub context_max_chars: usize,
    /// Cap on the injected 总则 (system prompt) length, in chars.
    pub guidelines_max_chars: usize,
}

impl Default for PipelineTuning {
    fn default() -> Self {
        Self { queue_capacity: 64, context_max_chars: 4000, guidelines_max_chars: 8000 }
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

    /// Self-documenting default `felin.toml`, written on first launch so users can
    /// discover and tune technical parameters — including the user-managed OCR
    /// sidecar path + config — without relying on env vars. Values mirror
    /// [`TechConfig::default`]; `default_template_matches_defaults` keeps them in
    /// sync. Only written when the file is missing (never overwrites user edits).
    pub fn default_template() -> String {
        format!(
            r#"# Felin Translator 技术参数配置（felin.toml）
#
# 本文件由应用首次启动时自动生成（数据目录，见 PROGRESS §5）。技术参数在此编辑，
# 非技术选项（模型/密钥/并发等）在 GUI 里设置。删除后重启会按本模板重新生成。
# 所有字段都可省略 —— 省略即用内置默认值。

[seg]
default_block_size = 3000
fallback_chapter_title = "正文"
heading_max_chars = 40
chapter_heading_patterns = ["{chapter_pattern}"]
sentence_enders = "{sentence_enders}"

[ocr]
low_score_threshold = 0.6
max_page_json_bytes = 16777216
max_manifest_bytes = 67108864

[names]
fuzzy_max_distance = 1

[llm]
timeout_secs = 120
max_retries = 3
base_delay_ms = 500
max_delay_secs = 30
# temperature = 0.3     # 可选，覆盖模型默认
# max_tokens = 2048     # 可选

[sidecar]
cancel_grace_secs = 8
poll_ms = 150
# OCR 后端（ocr-cli）二进制与 provider 配置文件由用户管理，填绝对路径即生效：
#   bin    = "/path/to/ocr-router/bin/ocr-cli"
#   config = "/path/to/ocr-router/config.yaml"   # 含 provider 密钥
# 两者留空（默认）时依次退回环境变量 FELIN_SIDECAR / FELIN_SIDECAR_CONFIG，
# 再缺省时打包版找可执行文件旁的 ocr-cli / config.yaml；找不到会在导入时报错。

[pipeline]
queue_capacity = 64
context_max_chars = 4000
guidelines_max_chars = 8000

[db]
read_pool_size = 8
busy_timeout_ms = 5000

[import]
max_file_bytes = 134217728
"#,
            // Values are injected TOML-escaped (backslash is the only character
            // that needs it here) so a regex like `\s` survives the round-trip.
            chapter_pattern = toml_escape(DEFAULT_CHAPTER_PATTERN),
            sentence_enders = toml_escape(DEFAULT_SENTENCE_ENDERS),
        )
    }

    /// Sentence-ender characters as a `Vec<char>`.
    pub fn sentence_enders(&self) -> Vec<char> {
        self.seg.sentence_enders.chars().collect()
    }
}

/// Escape a value for injection into the TOML default template. The template
/// strings here are raw (`r#"…"#`), so backslashes in consts like the chapter
/// regex must be doubled to survive a TOML parse→re-serialize round-trip.
fn toml_escape(s: &str) -> String {
    s.replace('\\', "\\\\")
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
        assert_eq!(back.pipeline.queue_capacity, 64);
        assert_eq!(back.pipeline.context_max_chars, 4000);
    }

    #[test]
    fn partial_toml_fills_missing_with_defaults() {
        let c = TechConfig::from_toml_str("[ocr]\nlow_score_threshold = 0.8\n").unwrap();
        assert_eq!(c.ocr.low_score_threshold, 0.8);
        assert_eq!(c.seg.default_block_size, 3000);
        assert_eq!(c.sidecar.cancel_grace_secs, 8);
    }

    #[test]
    fn default_template_matches_defaults() {
        // The first-launch felin.toml template must stay value-identical to the
        // in-code defaults (only comments are added), so parsing it back and
        // re-serializing yields exactly the default serialization.
        let from_tpl = TechConfig::from_toml_str(&TechConfig::default_template()).unwrap();
        assert_eq!(from_tpl.to_toml_string(), TechConfig::default().to_toml_string());
        // And it documents the user-managed sidecar keys.
        let tpl = TechConfig::default_template();
        assert!(tpl.contains("#   bin    = \"/path/to/ocr-router/bin/ocr-cli\""));
        assert!(tpl.contains("FELIN_SIDECAR_CONFIG"));
    }
}

