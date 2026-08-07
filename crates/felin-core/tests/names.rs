//! Proper-noun integration tests: normalization, fuzzy distance, Aho-Corasick
//! matching, CSV import, and LLM candidate extraction.
//!
//! Moved here from the crate's inline `#[cfg(test)]` modules (project policy:
//! no test code alongside application code).

use felin_core::names::{levenshtein, normalize, within_distance, ColumnMapping, Matcher};
use felin_core::llm::extract_json;

// ----- names/normalize ------------------------------------------------------

#[test]
fn folds_fullwidth_ascii() {
    assert_eq!(normalize("Ｔａｎａｋａ"), "Tanaka");
    assert_eq!(normalize("１２３"), "123");
}

#[test]
fn folds_halfwidth_katakana() {
    // Halfwidth katakana → fullwidth.
    assert_eq!(normalize("ﾀﾅｶ"), "タナカ");
}

#[test]
fn leaves_normal_text_unchanged() {
    assert_eq!(normalize("田中"), "田中");
    assert_eq!(normalize("猫"), "猫");
}

// ----- names/fuzzy ----------------------------------------------------------

#[test]
fn basic_distances() {
    assert_eq!(levenshtein("田中", "田中"), 0);
    assert_eq!(levenshtein("田中", "田申"), 1); // substitution
    assert_eq!(levenshtein("田中", "田中角"), 1); // insertion
    assert_eq!(levenshtein("田中角", "田中"), 1); // deletion
    assert_eq!(levenshtein("田中", "本田"), 2);
    assert_eq!(levenshtein("", "猫"), 1);
}

#[test]
fn within_distance_pre_checks_length() {
    assert!(within_distance("サクラ", "サクヲ", 1)); // one substitution
    assert!(within_distance("サクラ", "サク", 1)); // one deletion
    assert!(!within_distance("あいうえお", "か", 1)); // far apart
}

// ----- names/matcher --------------------------------------------------------

#[test]
fn longest_match_wins() {
    let m = Matcher::build(&[("田中".into(), 1), ("田中角栄".into(), 2)]).unwrap();
    let hits = m.find_hits("昨日、田中角栄が来た。");
    assert_eq!(hits.len(), 1);
    assert_eq!(hits[0].name_id, 2);
    assert_eq!(hits[0].form, "田中角栄");
}

#[test]
fn shorter_entry_still_matches_when_alone() {
    let m = Matcher::build(&[("田中".into(), 1), ("田中角栄".into(), 2)]).unwrap();
    assert_eq!(m.name_ids_in("田中さんこんにちは"), vec![1]);
}

#[test]
fn aliases_share_a_name_id() {
    let m = Matcher::build(&[("猫".into(), 5), ("ネコ".into(), 5)]).unwrap();
    let hits = m.find_hits("猫とネコ");
    assert_eq!(hits.len(), 2);
    assert!(hits.iter().all(|h| h.name_id == 5));
    assert_eq!(m.name_ids_in("猫とネコ"), vec![5]);
}

#[test]
fn matching_is_normalization_aware() {
    // pattern in fullwidth katakana; text in halfwidth katakana.
    let m = Matcher::build(&[("タナカ".into(), 7)]).unwrap();
    assert_eq!(m.name_ids_in("ﾀﾅｶが来た"), vec![7]);
}

#[test]
fn empty_glossary_finds_nothing() {
    let m = Matcher::build(&[]).unwrap();
    assert!(m.find_hits("何か").is_empty());
}

// ----- names/csv -------------------------------------------------------------

fn mapping() -> ColumnMapping {
    ColumnMapping {
        japanese: 0,
        chinese: 1,
        english: Some(2),
        category: None,
        notes: None,
        has_header: true,
    }
}

#[test]
fn parses_rows_and_skips_incomplete() {
    // An extra "aliases" column (legacy CSV format) is tolerated and ignored:
    // only mapped columns are read.
    let data = "jp,zh,en,aliases\n田中,田中,Tanaka,たなか|タナカ\n,空,,\n猫,猫,cat,ネコ\n";
    let rows = felin_core::names::csv::parse(data.as_bytes(), &mapping()).unwrap();
    assert_eq!(rows.len(), 2); // the empty-japanese row is skipped
    assert_eq!(rows[0].japanese, "田中");
    assert_eq!(rows[0].english.as_deref(), Some("Tanaka"));
    assert_eq!(rows[1].japanese, "猫");
    assert_eq!(rows[1].english.as_deref(), Some("cat"));
}

#[test]
fn reads_headers() {
    let data = "jp,zh,en,aliases\n田中,田中,,\n";
    assert_eq!(felin_core::names::csv::headers(data.as_bytes()).unwrap(), vec!["jp", "zh", "en", "aliases"]);
}

#[test]
fn discards_unmapped_columns() {
    // The file has 日文/中文/英文/分类/备注 plus an extra "来源" column. Only
    // japanese/chinese/english are mapped, so the 分类/备注/来源 cell values must
    // never reach the parsed rows — unmapped columns are dropped by design
    // (this is what "丢弃不选用的列" relies on).
    let data = "jp,zh,en,cat,note,source\n田中,田中,Tanaka,人物,关西腔,第1卷\n猫,猫,cat,动物,,第2卷\n";
    let mapping = ColumnMapping {
        japanese: 0,
        chinese: 1,
        english: Some(2),
        category: None, // 丢弃
        notes: None,    // 丢弃
        has_header: true,
    };
    let rows = felin_core::names::csv::parse(data.as_bytes(), &mapping).unwrap();
    assert_eq!(rows.len(), 2);
    assert_eq!(rows[0].japanese, "田中");
    assert_eq!(rows[0].chinese, "田中");
    assert_eq!(rows[0].english.as_deref(), Some("Tanaka"));
    assert_eq!(rows[0].category, None);
    assert_eq!(rows[0].notes, None);
    assert_eq!(rows[1].category, None);
    assert_eq!(rows[1].notes, None);
    // The discarded cells (关西腔 / 动物 / 第1卷 / 第2卷) are absent entirely.
    assert!(rows.iter().all(|r| r.category.is_none() && r.notes.is_none()));
}

// ----- names/extract ----------------------------------------------------------

#[test]
fn parses_candidates_from_fenced_json() {
    let resp = "```json\n[{\"japanese\":\"田中\",\"guess_chinese\":\"田中\",\"context\":\"主人公\"},\
                {\"japanese\":\"東京\",\"chinese\":\"东京\"}]\n```";
    let list = felin_core::names::extract::parse_candidates(resp).unwrap();
    assert_eq!(list.len(), 2);
    assert_eq!(list[0].japanese, "田中");
    assert_eq!(list[0].guess_chinese, "田中");
    // `chinese` is accepted as an alias for `guess_chinese`.
    assert_eq!(list[1].guess_chinese, "东京");
    assert_eq!(list[1].context, "");
}

#[test]
fn returns_none_for_unparseable() {
    assert!(felin_core::names::extract::parse_candidates("抱歉，无法输出。").is_none());
}

#[test]
fn candidate_is_deserializable_with_defaults() {
    // The extraction pass's parse path: extract_json → serde deserialize.
    let v = extract_json(r#"{"japanese":"猫","guess_chinese":"猫"}"#).unwrap();
    let c: felin_core::names::extract::Candidate = serde_json::from_value(v).unwrap();
    assert_eq!(c.japanese, "猫");
    assert_eq!(c.guess_chinese, "猫");
    assert_eq!(c.context, "");
}
