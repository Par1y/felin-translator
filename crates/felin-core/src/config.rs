//! Layered configuration.
//!
//! Non-technical, user-facing options live in the GUI (persisted per project or
//! globally). *Technical* parameters live in [`TechConfig`], loaded from an
//! editable `felin.toml` next to the software so developers / advanced users can
//! tune them without recompiling. Missing fields fall back to the defaults here.

use serde::{Deserialize, Serialize};
use std::path::Path;

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
    pub prompt: PromptConfig,
    pub debug: DebugConfig,
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

/// Editable LLM prompt templates (`felin.toml [prompt]`).
///
/// The prompt text sent to the LLM is **never hardcoded in the runtime** — it
/// is read entirely from `felin.toml`. The single source of truth is the
/// `[prompt]` section of the first-launch template ([`TechConfig::default_template`],
/// whose values are fixed by [`factory_prompt_config`]); `Default` here is
/// *empty* by design, so a config file that omits a field yields an empty
/// string (that message section is then simply not sent), not a hidden default.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct PromptConfig {
    /// Name-extraction system message (专名抽取). Empty → no system message.
    pub extract_system: String,
    /// Name-classification system message (专名自动打标签). Empty → the
    /// auto-tag command refuses with a config hint (找不到即报错).
    pub extract_tags_system: String,
    /// Translation system-message template with `{guidelines}` / `{instruction}`
    /// / `{glossary}` placeholders. Empty → no system message.
    pub translation_system: String,
    /// Translation user-message template with `{context}` / `{source}`
    /// placeholders. Empty → only the raw source is sent.
    pub translation_user: String,
}

impl Default for PromptConfig {
    fn default() -> Self {
        // Deliberately empty: prompt text must come from the config file.
        Self {
            extract_system: String::new(),
            extract_tags_system: String::new(),
            translation_system: String::new(),
            translation_user: String::new(),
        }
    }
}

/// The *factory* prompt templates baked into the first-launch `felin.toml`
/// (single source of truth for the shipped defaults). Referenced **only** by
/// [`TechConfig::default_template`] — the runtime never reads these; it uses
/// whatever the config file says. Editing the values here changes what a fresh
/// install gets, not what already-configured installs run.
pub(crate) fn factory_prompt_config() -> PromptConfig {
    PromptConfig {
        extract_system: "你是日文专有名词抽取助手。从给定日文文本中抽取专有名词（人名、地名、\
组织、作品名、独特术语等），忽略普通词汇。只输出 JSON 数组，每项形如 \
{\"japanese\":\"原文形式\",\"guess_chinese\":\"推测中文\",\"category\":\"类别\",\
\"context\":\"简短出处\"}，其中 category 只能是：人名、地名、组织、作品名、物品、系统、\
术语、其他。不要输出任何其他文字。"
            .to_string(),
        extract_tags_system: "你是专有名词分类助手。对用户给出的每个专有名词判断其类别，\
只输出 JSON 数组，每项形如 {\"japanese\":\"原文形式\",\"category\":\"类别\"}，\
其中 category 只能是：人名、地名、组织、作品名、物品、系统、术语、其他。\
不要输出任何其他文字。"
            .to_string(),
        translation_system: "{guidelines}\n\n附加要求（优先级高于总则）：\n{instruction}\n\n专名参考（词表，必须使用）：\n{glossary}".to_string(),
        translation_user: "【上文参考（已校对，仅供风格与称谓参考，勿重复翻译）】\n{context}\n\n【待翻译原文】\n{source}".to_string(),
    }
}

/// Default LLM transport tunables plus the endpoint/model *seeds*
/// (`felin.toml [llm]`).
///
/// Like [`PromptConfig`], `endpoint`/`model` are **empty in-code by design**:
/// the runtime never falls back to a baked-in provider. The first-launch
/// template ([`TechConfig::default_template`]) ships the factory endpoint/model
/// so a fresh install connects out of the box, and the GUI settings page can
/// override both per project (project DB `llm_endpoint` / `llm_model`). An
/// empty endpoint means "not configured" — LLM features then fail with a
/// connection error until one is set, never silently calling a hidden default.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct LlmDefaults {
    /// OpenAI-compatible base URL used when the current project has not saved
    /// its own in the GUI (a bare base or a full chat-completions URL).
    pub endpoint: String,
    /// Model name seed, likewise per-project overridable.
    pub model: String,
    pub timeout_secs: u64,
    pub max_retries: u32,
    pub base_delay_ms: u64,
    pub max_delay_secs: u64,
    pub temperature: Option<f64>,
    pub max_tokens: Option<u32>,
    /// **Global** cap on simultaneous LLM calls across ALL features (translation
    /// workers, name extraction, auto-tag, connection test). 1–16. The unified
    /// concurrency model (see `docs/data-contract.md` §6) funnels every LLM call
    /// through one shared semaphore sized by this, so an extraction pass can't
    /// pile on top of a running translation.
    pub concurrency: u64,
}

impl Default for LlmDefaults {
    fn default() -> Self {
        Self {
            endpoint: String::new(),
            model: String::new(),
            timeout_secs: 120,
            max_retries: 3,
            base_delay_ms: 500,
            max_delay_secs: 30,
            temperature: None,
            max_tokens: None,
            concurrency: 2,
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

/// Diagnostic toggles. Off by default so normal runs stay quiet.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct DebugConfig {
    /// Emit key-operation logs (import / segmentation / translation / name
    /// extraction / export) for diagnosis. Off by default.
    pub enabled: bool,
}

impl Default for DebugConfig {
    fn default() -> Self {
        Self { enabled: false }
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
        let base = format!(
            r#"# Felin Translator 技术参数配置（felin.toml）
# 如果你不知道该文件内容的具体含义，不要修改该文件。

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
# 默认接口与模型
endpoint = "https://api.openai.com/v1"
model = "gpt-4o"
timeout_secs = 120
max_retries = 3
base_delay_ms = 500
max_delay_secs = 30
# temperature = 0.3     # 可选，覆盖模型默认
# max_tokens = 2048     # 可选
# concurrency = 2       # 全局 LLM 并发上限（1–16）：翻译 worker、专名抽取、
#                       # 自动打标签、连接测试共用此限流，避免同时打爆上游。

[sidecar]
cancel_grace_secs = 8
poll_ms = 150
# OCR 后端（ocr-cli）二进制与 provider 配置文件：
#   bin    = "/path/to/ocr-router/bin/ocr-cli"
#   config = "/path/to/ocr-router/config.yaml"   # 含 provider 密钥
# 两者留空（默认）时依次退回环境变量 FELIN_SIDECAR / FELIN_SIDECAR_CONFIG，
# 或打包版本可执行文件旁的 ocr-cli / config.yaml。

[pipeline]
queue_capacity = 64
context_max_chars = 4000
guidelines_max_chars = 8000

[db]
read_pool_size = 8
busy_timeout_ms = 5000

[import]
max_file_bytes = 134217728

[debug]
# 开启后输出关键操作日志（导入/分段/翻译/专名抽取/导出），便于诊断；默认关。
enabled = false
"#,
            // Values are injected TOML-escaped (backslash is the only character
            // that needs it here) so a regex like `\s` survives the round-trip.
            chapter_pattern = toml_escape(DEFAULT_CHAPTER_PATTERN),
            sentence_enders = toml_escape(DEFAULT_SENTENCE_ENDERS),
        );
        // The factory prompt templates (single source of truth), appended last
        // with the comment header. Values come from `factory_prompt_config()`,
        // NOT `PromptConfig::default()` (which is empty by design).
        format!(
            "{base}\n# 提示词模板 —— 发送给 LLM 的 prompt。\n\
             # 翻译 System 占位符 {{guidelines}} {{instruction}} {{glossary}}；\n\
             # 翻译 User 占位符 {{context}} {{source}}。\n\
             # 某字段留空（\"\"）即不发送该消息段。可在设置页「提示词」编辑\n\
             # （GUI 保存后立即生效，无需重启）；也可直接改本文件（重启生效）。\n\
             {}",
            prompt_block()
        )
    }

    /// Sentence-ender characters as a `Vec<char>`.
    pub fn sentence_enders(&self) -> Vec<char> {
        self.seg.sentence_enders.chars().collect()
    }

    /// Load from `felin.toml` on disk, applying the self-documenting template
    /// when the file is missing — and *healing* an older file that predates the
    /// `[prompt]` section by appending the factory prompt block (so prompt text
    /// is always present in the config file, never silently empty). After any
    /// write the returned config is re-parsed from disk, so it always matches
    /// what the app actually reads next time.
    ///
    /// Returns `(config, written)` where `written` = a template/`[prompt]`
    /// block was written (fresh install or healed legacy file). The load still
    /// succeeds if a write fails (config parses as-is rather than a broken file).
    pub fn load_from_disk(path: &Path) -> (TechConfig, bool) {
        match std::fs::read_to_string(path) {
            Ok(s) => match TechConfig::from_toml_str(&s) {
                Ok(mut c) => {
                    let written = if !has_prompt_section(&s) {
                        match append_prompt_block(path, &s) {
                            Ok(()) => true,
                            Err(e) => {
                                tracing::warn!(error = %e, "could not append [prompt] to felin.toml");
                                false
                            }
                        }
                    } else if !prompt_section_has_key(&s, "extract_tags_system") {
                        // `[prompt]` exists but predates the `extract_tags_system`
                        // field entirely (a legacy install whose section was
                        // written before auto-tag existed): patch in just that
                        // field with the factory default so auto-tag works out of
                        // the box. Only *absent* fields are healed — an explicitly
                        // present-but-empty value (`extract_tags_system = ""`) is
                        // a deliberate "auto-tag off" and is never touched.
                        let mut patched = c.prompt.clone();
                        patched.extract_tags_system = factory_prompt_config().extract_tags_system;
                        match set_prompt_section(path, &patched) {
                            Ok(()) => true,
                            Err(e) => {
                                tracing::warn!(error = %e, "could not heal extract_tags_system in felin.toml");
                                false
                            }
                        }
                    } else {
                        false
                    };
                    if written {
                        // Re-read so the returned config matches the healed file.
                        if let Ok(text) = std::fs::read_to_string(path) {
                            if let Ok(p) = TechConfig::from_toml_str(&text) {
                                c = p;
                            }
                        }
                    }
                    (c, written)
                }
                Err(e) => {
                    tracing::warn!(error = %e, "invalid felin.toml; using defaults");
                    (TechConfig::default(), false)
                }
            },
            Err(_) => {
                // Write the self-documenting template (commented guidance, incl.
                // the user-managed `[sidecar] bin`/`config` keys and the factory
                // prompt templates) so advanced users can discover and edit it;
                // only ever written when the file is missing.
                let written = std::fs::write(path, TechConfig::default_template()).is_ok();
                let c = std::fs::read_to_string(path)
                    .ok()
                    .and_then(|t| TechConfig::from_toml_str(&t).ok())
                    .unwrap_or_default();
                (c, written)
            }
        }
    }
}

/// Does the TOML text already contain a top-level `[prompt]` section? Line-based
/// scan (the whole text is produced by `toml` serde / the template, so a `[`
/// column-0 table header is unambiguous here).
fn has_prompt_section(text: &str) -> bool {
    text.lines().any(|l| l.trim_start() == "[prompt]")
}

/// Does the `[prompt]` section contain a `key = …` line? Scoped to the section
/// (from its header to the next `[` table header) so a mention of the key in a
/// comment or another section can't suppress healing. Matches `key` followed by
/// optional spaces then `=` (toml's `key = value`), mirroring what `toml` serde
/// writes.
fn prompt_section_has_key(text: &str, key: &str) -> bool {
    let mut in_prompt = false;
    for line in text.lines() {
        let t = line.trim_start();
        if t.starts_with('[') {
            in_prompt = t == "[prompt]";
            continue;
        }
        if in_prompt && t.starts_with(key) {
            let after = &t[key.len()..];
            if after.trim_start().starts_with('=') {
                return true;
            }
        }
    }
    false
}

/// Append a `[prompt]` section carrying the factory prompt templates to `path`
/// when the file lacks one (legacy pre-`[prompt]` installs). Plain string
/// append — every other section, value and comment is left untouched.
fn append_prompt_block(path: &Path, current: &str) -> std::io::Result<()> {
    let ending = if current.is_empty() || current.ends_with('\n') { "" } else { "\n" };
    std::fs::write(path, format!("{current}{ending}\n{}", prompt_block()))
}

/// The self-documenting `[prompt]` section with the factory prompt templates
/// (its single source of truth). The struct serializes as bare fields, so the
/// table header is added explicitly.
fn prompt_block() -> String {
    prompt_block_for(&factory_prompt_config())
}

/// A `[prompt]` section (header + the three fields) carrying `prompt`'s text —
/// the string form used whenever a `[prompt]` block is appended to or written
/// into `felin.toml`.
fn prompt_block_for(prompt: &PromptConfig) -> String {
    format!("[prompt]\n{}", toml::to_string_pretty(prompt).unwrap_or_default())
}

/// Replace or append the top-level `[prompt]` section of `felin.toml` at `path`
/// with `prompt`, preserving every other section, value and comment.
///
/// - No `[prompt]` yet → the new section is appended at the end (mirroring the
///   legacy healing in [`TechConfig::load_from_disk`]).
/// - `[prompt]` present → the existing section's header + fields are replaced
///   in place; everything before the header and after the section's content
///   (including any blank-line separator before the next section) is preserved
///   byte-for-byte.
///
/// Only `felin.toml` is touched. Returns a clear error if the file cannot be
/// read or written.
pub fn set_prompt_section(path: &Path, prompt: &PromptConfig) -> Result<(), String> {
    let text = std::fs::read_to_string(path)
        .map_err(|e| format!("cannot read {}: {e}", path.display()))?;
    let block = prompt_block_for(prompt);
    let new_text = if has_prompt_section(&text) {
        replace_prompt_section(&text, &block)
    } else {
        let ending = if text.is_empty() || text.ends_with('\n') { "" } else { "\n" };
        format!("{text}{ending}\n{block}")
    };
    std::fs::write(path, new_text).map_err(|e| format!("cannot write {}: {e}", path.display()))
}

/// Replace the existing top-level `[prompt]` section (its header line + fields)
/// in `text` with `block`. The section's region runs from the `[prompt]` header
/// to just before the next top-level table header (a line whose trimmed form
/// starts with `[`); the region is cut out and `block` spliced in, so everything
/// outside it — comments, other sections, and any blank lines separating the
/// prompt section from the next one — survives byte-for-byte.
fn replace_prompt_section(text: &str, block: &str) -> String {
    // Byte offset of the `[prompt]` header (its own line; guaranteed present).
    let header_start = text.find("[prompt]").expect("caller checked has_prompt_section");
    let rest = &text[header_start..];
    // Offset (relative to `rest`) just after the header line's terminator.
    let body_start = rest.find('\n').map(|i| i + 1).unwrap_or(rest.len());
    // End (relative to `rest`) of the last non-blank body line, including its
    // line terminator — blank lines between the section and the next one are
    // preserved as part of the untouched suffix.
    let mut last_content_end = body_start;
    let mut pos = body_start;
    while pos < rest.len() {
        let line_end = rest[pos..].find('\n').map(|i| pos + i + 1).unwrap_or(rest.len());
        let line = &rest[pos..line_end];
        if line.trim_start().starts_with('[') {
            break; // next top-level table header starts the preserved suffix
        }
        if !line.trim().is_empty() {
            last_content_end = line_end;
        }
        pos = line_end;
    }
    let mut out = String::with_capacity(text.len() + block.len());
    out.push_str(&text[..header_start]);
    out.push_str(block);
    out.push_str(&rest[last_content_end..]);
    out
}

/// Escape a value for injection into the TOML default template. The template
/// strings here are raw (`r#"…"#`), so backslashes in consts like the chapter
/// regex must be doubled to survive a TOML parse→re-serialize round-trip.
fn toml_escape(s: &str) -> String {
    s.replace('\\', "\\\\")
}

