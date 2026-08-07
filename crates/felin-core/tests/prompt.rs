//! Pipeline prompt-assembly integration tests: truncation, glossary block
//! rendering, and TU → `TranslateRequest` assembly.
//!
//! Moved here from the crate's inline `#[cfg(test)]` modules (project policy:
//! no test code alongside application code).

use felin_core::names::Hit;
use felin_core::pipeline::prompt::{build_tu_request, default_guidelines, glossary_block, truncate_chars};
use std::collections::HashMap;

#[test]
fn truncate_respects_char_boundary() {
    assert_eq!(truncate_chars("あいうえお", 5), "あいうえお");
    assert_eq!(truncate_chars("あいうえお", 3), "あいう…");
    assert_eq!(truncate_chars("abc", 3), "abc");
}

#[test]
fn glossary_block_dedupes_by_id_in_hit_order() {
    let mut lookup: HashMap<i64, (String, Option<String>)> = HashMap::new();
    lookup.insert(1, ("田中".into(), Some("田中".into())));
    lookup.insert(2, ("佐藤".into(), None));
    let hits = vec![
        Hit { name_id: 1, start: 0, end: 2, form: "田中".into() },
        Hit { name_id: 2, start: 2, end: 4, form: "佐藤".into() },
        Hit { name_id: 1, start: 4, end: 6, form: "田中".into() },
    ];
    let block = glossary_block(&hits, &lookup).unwrap();
    assert_eq!(block, "田中 → 田中\n佐藤");
}

#[test]
fn build_request_truncates_context_not_source() {
    let long_ctx = "あ".repeat(10_000);
    let long_src = "い".repeat(50_000);
    let req = build_tu_request(
        "总则".into(),
        100,
        None,
        None,
        Some(long_ctx),
        100,
        long_src.clone(),
        String::new(),
        String::new(),
    );
    assert_eq!(req.context.as_ref().unwrap().chars().count(), 101); // 100 + '…'
    assert_eq!(req.source.chars().count(), 50_000);
}

#[test]
fn factory_prompt_templates_are_nonempty_and_well_formed() {
    // The prompt text is config-driven (no runtime defaults); the shipped
    // templates live only in the first-launch felin.toml. This guards that
    // template against accidentally shipping an empty/non-functional prompt.
    let tpl = felin_core::config::TechConfig::default_template();
    let parsed = felin_core::config::TechConfig::from_toml_str(&tpl).unwrap();
    let p = parsed.prompt;

    let g = default_guidelines();
    assert!(g.contains("日译中"));

    assert!(!p.translation_system.is_empty());
    assert!(p.translation_system.contains("{guidelines}"));
    assert!(p.translation_system.contains("{instruction}"));
    assert!(p.translation_system.contains("{glossary}"));

    assert!(!p.translation_user.is_empty());
    assert!(p.translation_user.contains("{context}"));
    assert!(p.translation_user.contains("{source}"));

    assert!(!p.extract_system.is_empty());
    assert!(p.extract_system.contains("JSON"));
}
