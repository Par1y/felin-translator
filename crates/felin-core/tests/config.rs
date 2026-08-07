//! TechConfig integration tests: TOML round-trip, partial-file defaults, the
//! self-documenting first-launch template, and the `[prompt]` section.
//!
//! Moved here from the crate's inline `#[cfg(test)]` module (project policy: no
//! test code alongside application code).

use felin_core::config::{set_prompt_section, PromptConfig, TechConfig};
use std::fs;
use tempfile::TempDir;

#[test]
fn default_roundtrips_through_toml() {
    let s = TechConfig::default().to_toml_string();
    let back = TechConfig::from_toml_str(&s).unwrap();
    assert_eq!(back.seg.default_block_size, 3000);
    assert_eq!(back.ocr.low_score_threshold, 0.6);
    assert_eq!(back.seg.sentence_enders, felin_core::config::DEFAULT_SENTENCE_ENDERS);
    assert_eq!(back.seg.chapter_heading_patterns, vec![felin_core::config::DEFAULT_CHAPTER_PATTERN.to_string()]);
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
    // The first-launch felin.toml must be value-identical to the in-code
    // defaults for every *technical* section (only comments are added), so a
    // parsing round-trip yields exactly the default serialization.
    //
    // `[prompt]` is the one deliberate exception: the template ships the
    // factory prompt text (its single source of truth, `factory_prompt_config()`)
    // so a fresh install translates out of the box, while `TechConfig::default()`
    // carries no prompt at all — the runtime is config-driven only and must not
    // bake prompt text into the code. `fresh_startup_felin_toml_is_spec_compliant`
    // separately locks the prompt section's shape.
    let from_tpl = TechConfig::from_toml_str(&TechConfig::default_template()).unwrap();
    let mut defaults = TechConfig::default();
    defaults.prompt = from_tpl.prompt.clone();
    assert_eq!(from_tpl.to_toml_string(), defaults.to_toml_string());
    // And it documents the user-managed sidecar keys.
    let tpl = TechConfig::default_template();
    assert!(tpl.contains("#   bin    = \"/path/to/ocr-router/bin/ocr-cli\""));
    assert!(tpl.contains("FELIN_SIDECAR_CONFIG"));
}

/// A fresh install's `felin.toml` must be fully spec-compliant: the prompt
/// text it ships is the *only* prompt source (runtime has none baked in), every
/// section parses, and the placeholders the renderer depends on are present.
#[test]
fn fresh_startup_felin_toml_is_spec_compliant() {
    let tpl = TechConfig::default_template();
    let parsed = TechConfig::from_toml_str(&tpl).unwrap();

    // The template carries a non-empty, placeholder-bearing `[prompt]` section —
    // this is what a fresh install actually runs on, so it must be functional.
    assert!(!parsed.prompt.translation_system.is_empty());
    assert!(parsed.prompt.translation_system.contains("{guidelines}"));
    assert!(parsed.prompt.translation_system.contains("{instruction}"));
    assert!(parsed.prompt.translation_system.contains("{glossary}"));
    assert!(!parsed.prompt.translation_user.is_empty());
    assert!(parsed.prompt.translation_user.contains("{context}"));
    assert!(parsed.prompt.translation_user.contains("{source}"));
    assert!(!parsed.prompt.extract_system.is_empty());
    assert!(parsed.prompt.extract_system.contains("JSON"));
    // The auto-tag classifier ships with the factory prompt too (a fresh
    // install can auto-tag out of the box).
    assert!(!parsed.prompt.extract_tags_system.is_empty());
    assert!(parsed.prompt.extract_tags_system.contains("JSON"));

    // Every technical section the config layer drives parses with the shipped values.
    assert_eq!(parsed.seg.default_block_size, 3000);
    assert_eq!(parsed.ocr.low_score_threshold, 0.6);
    assert_eq!(parsed.names.fuzzy_max_distance, 1);
    assert_eq!(parsed.llm.max_retries, 3);
    assert_eq!(parsed.sidecar.cancel_grace_secs, 8);
    assert_eq!(parsed.pipeline.queue_capacity, 64);
    assert_eq!(parsed.db.read_pool_size, 8);
    assert_eq!(parsed.import.max_file_bytes, 128 * 1024 * 1024);
}

#[test]
fn prompt_defaults_are_empty_not_hidden() {
    // The runtime carries NO prompt defaults: `PromptConfig::default()` is
    // empty, so whatever the config file says is exactly what runs. Empty = the
    // message section is not sent — never a silent built-in.
    let p = PromptConfig::default();
    assert!(p.extract_system.is_empty());
    assert!(p.translation_system.is_empty());
    assert!(p.translation_user.is_empty());
}

#[test]
fn prompt_values_come_from_the_config_file() {
    // A config file with an explicit `[prompt]` is honored verbatim.
    let c = TechConfig::from_toml_str("[prompt]\nextract_system = \"自定义抽取\"\n").unwrap();
    assert_eq!(c.prompt.extract_system, "自定义抽取");
    // Fields omitted from `[prompt]` are empty (not a hidden default).
    assert!(c.prompt.translation_system.is_empty());
    assert!(c.prompt.translation_user.is_empty());
}

#[test]
fn template_documents_prompt_placeholders() {
    let tpl = TechConfig::default_template();
    assert!(tpl.contains("[prompt]"));
    assert!(tpl.contains("{guidelines}"));
    assert!(tpl.contains("{source}"));
}

#[test]
fn debug_defaults_to_off() {
    // The debug switch is off by default: a config file without `[debug]`
    // (e.g. a legacy file) runs silently, and the first-launch template ships
    // it off.
    assert!(!TechConfig::default().debug.enabled);
    let c = TechConfig::from_toml_str("[ocr]\nlow_score_threshold = 0.8\n").unwrap();
    assert!(!c.debug.enabled, "missing [debug] section must default to false");
    let tpl = TechConfig::default_template();
    assert!(tpl.contains("[debug]"));
    assert!(
        !TechConfig::from_toml_str(&tpl).unwrap().debug.enabled,
        "first-launch template must ship debug off"
    );
}

#[test]
fn debug_enabled_parses() {
    let c = TechConfig::from_toml_str("[debug]\nenabled = true\n").unwrap();
    assert!(c.debug.enabled);
    let back = TechConfig::from_toml_str(&TechConfig::default().to_toml_string()).unwrap();
    assert!(!back.debug.enabled);
}

// ----- startup load + legacy-file healing -----------------------------------

/// A fresh install (no felin.toml): the documented template is written and the
/// returned config carries the factory prompt text — the app runs with prompts
/// out of the box, and they came from the written file, not the runtime.
#[test]
fn load_from_disk_writes_template_on_fresh_install() {
    let dir = TempDir::new().unwrap();
    let path = dir.path().join("felin.toml");
    let (cfg, written) = TechConfig::load_from_disk(&path);
    assert!(written, "a missing file must be written");
    let text = fs::read_to_string(&path).unwrap();
    assert!(text.contains("[prompt]"), "fresh template must carry [prompt]");
    assert!(!cfg.prompt.translation_system.is_empty(), "fresh install runs with factory prompts");
    assert!(cfg.prompt.translation_system.contains("{guidelines}"));
}

/// A legacy install whose felin.toml predates `[prompt]`: the section is
/// appended in place — user values/comments preserved, and the loaded config
/// now carries the factory prompt text (no silent empty prompts).
#[test]
fn load_from_disk_heals_legacy_file_missing_prompt_section() {
    let dir = TempDir::new().unwrap();
    let path = dir.path().join("felin.toml");
    let legacy = "# 手写注释要保留\n[llm]\ntimeout_secs = 7\n[sidecar]\nbin = \"/opt/ocr-cli\"\n";
    fs::write(&path, legacy).unwrap();

    let (cfg, written) = TechConfig::load_from_disk(&path);
    assert!(written, "a missing [prompt] section must be appended");
    // User values survive.
    assert_eq!(cfg.llm.timeout_secs, 7);
    assert_eq!(cfg.sidecar.bin.as_deref(), Some("/opt/ocr-cli"));
    // Factory prompts now come from the file.
    assert!(!cfg.prompt.translation_system.is_empty());
    assert!(!cfg.prompt.extract_system.is_empty());
    // Nothing else was disturbed.
    let text = fs::read_to_string(&path).unwrap();
    assert!(text.contains("# 手写注释要保留"));
    assert!(text.contains("[prompt]"));
    // And a second load sees the section already present → no rewrite.
    let (_, written2) = TechConfig::load_from_disk(&path);
    assert!(!written2);
}

/// A file that already has `[prompt]` is never touched — a user's custom
/// templates are honored verbatim and no append happens.
#[test]
fn load_from_disk_leaves_existing_prompt_untouched() {
    let dir = TempDir::new().unwrap();
    let path = dir.path().join("felin.toml");
    let text = "[prompt]\nextract_system = \"自定义抽取\"\n[llm]\ntimeout_secs = 9\n";
    fs::write(&path, text).unwrap();

    let (cfg, written) = TechConfig::load_from_disk(&path);
    assert!(!written);
    assert_eq!(cfg.prompt.extract_system, "自定义抽取");
    assert_eq!(cfg.llm.timeout_secs, 9);
    assert_eq!(fs::read_to_string(&path).unwrap(), text, "file byte-identical after load");
}

/// An invalid felin.toml falls back to defaults (empty prompts) without
/// touching the broken file.
#[test]
fn load_from_disk_invalid_file_falls_back_to_defaults() {
    let dir = TempDir::new().unwrap();
    let path = dir.path().join("felin.toml");
    fs::write(&path, "[seg\nno closing bracket").unwrap();
    let (cfg, written) = TechConfig::load_from_disk(&path);
    assert!(!written);
    assert!(cfg.prompt.translation_system.is_empty());
    assert_eq!(cfg.seg.default_block_size, 3000);
}

// ----- set_prompt_section (settings-page prompt editing) ---------------------

/// A file with no `[prompt]`: the section is appended at the end, user values
/// and comments preserved, and the file now carries the requested templates.
#[test]
fn set_prompt_section_appends_when_missing() {
    let dir = TempDir::new().unwrap();
    let path = dir.path().join("felin.toml");
    let legacy = "# 手写注释要保留\n[llm]\ntimeout_secs = 7\n[sidecar]\nbin = \"/opt/ocr-cli\"\n";
    fs::write(&path, legacy).unwrap();

    let new_prompt = PromptConfig {
        extract_system: "自定义抽取".into(),
        extract_tags_system: "自定义打标签".into(),
        translation_system: "自定义 system".into(),
        translation_user: "自定义 user".into(),
    };
    set_prompt_section(&path, &new_prompt).unwrap();

    let text = fs::read_to_string(&path).unwrap();
    // Prompt section appended; other content/comment untouched.
    assert!(text.contains("# 手写注释要保留"));
    assert!(text.contains("[llm]\ntimeout_secs = 7"));
    assert!(text.contains("[sidecar]\nbin = \"/opt/ocr-cli\""));
    assert!(text.contains("[prompt]"));
    // The file now round-trips to the requested prompt values.
    let cfg = TechConfig::from_toml_str(&text).unwrap();
    assert_eq!(cfg.prompt.extract_system, "自定义抽取");
    assert_eq!(cfg.prompt.translation_system, "自定义 system");
    assert_eq!(cfg.prompt.translation_user, "自定义 user");
    assert_eq!(cfg.llm.timeout_secs, 7);
    assert_eq!(cfg.sidecar.bin.as_deref(), Some("/opt/ocr-cli"));
}

/// A file that already has `[prompt]`: the section's header + fields are
/// replaced in place, every other section/comment/blank-line separator is
/// preserved byte-for-byte, and the file still parses with the new values.
#[test]
fn set_prompt_section_replaces_existing_and_preserves_rest() {
    let dir = TempDir::new().unwrap();
    let path = dir.path().join("felin.toml");
    let original = concat!(
        "# 顶部注释\n",
        "[prompt]\n",
        "extract_system = \"旧抽取\"\n",
        "translation_system = \"旧 system\"\n",
        "translation_user = \"旧 user\"\n",
        "\n",
        "[llm]\n",
        "timeout_secs = 9\n",
        "\n",
        "# 尾部注释\n",
    );
    fs::write(&path, original).unwrap();

    let new_prompt = PromptConfig {
        extract_system: "新抽取".into(),
        extract_tags_system: "新打标签".into(),
        translation_system: "新 system".into(),
        translation_user: "新 user".into(),
    };
    set_prompt_section(&path, &new_prompt).unwrap();

    let out = fs::read_to_string(&path).unwrap();
    // Only the prompt fields changed; surrounding comments, sections and the
    // blank-line separator before `[llm]` survive.
    assert!(out.starts_with("# 顶部注释\n[prompt]\n"));
    assert!(!out.contains("旧抽取"));
    assert!(out.contains("translation_user = \"新 user\"\n\n[llm]\ntimeout_secs = 9\n\n# 尾部注释\n"));
    // And it parses to the new values, with everything else intact.
    let cfg = TechConfig::from_toml_str(&out).unwrap();
    assert_eq!(cfg.prompt.extract_system, "新抽取");
    assert_eq!(cfg.prompt.translation_system, "新 system");
    assert_eq!(cfg.prompt.translation_user, "新 user");
    assert_eq!(cfg.llm.timeout_secs, 9);
}
