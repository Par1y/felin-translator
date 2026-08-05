//! Multi-pattern proper-noun matching over a project's glossary Japanese forms
//! (canonical + aliases), using Aho-Corasick with leftmost-longest semantics so
//! a longer glossary entry wins over a shorter one it contains (e.g. 田中角栄
//! over 田中). Matching runs on NFKC-normalized text.

use crate::error::{Error, Result};
use crate::names::normalize::normalize;
use aho_corasick::{AhoCorasick, MatchKind};

/// A glossary hit in a piece of text.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Hit {
    pub name_id: i64,
    /// Byte offsets into the NFKC-normalized text.
    pub start: usize,
    pub end: usize,
    pub form: String,
}

/// A compiled matcher over `(japanese_form, name_id)` pairs. Forms may repeat a
/// `name_id` (aliases of the same entry).
pub struct Matcher {
    ac: Option<AhoCorasick>,
    name_ids: Vec<i64>,
}

impl Matcher {
    /// Compile a matcher. Empty forms are skipped; an empty set yields a matcher
    /// that finds nothing.
    pub fn build(forms: &[(String, i64)]) -> Result<Self> {
        let mut patterns: Vec<String> = Vec::new();
        let mut name_ids: Vec<i64> = Vec::new();
        for (form, id) in forms {
            let n = normalize(form);
            if n.is_empty() {
                continue;
            }
            patterns.push(n);
            name_ids.push(*id);
        }
        if patterns.is_empty() {
            return Ok(Self { ac: None, name_ids });
        }
        let ac = AhoCorasick::builder()
            .match_kind(MatchKind::LeftmostLongest)
            .build(&patterns)
            .map_err(|e| Error::InvalidInput { detail: format!("failed to build matcher: {e}") })?;
        Ok(Self { ac: Some(ac), name_ids })
    }

    /// Non-overlapping leftmost-longest glossary hits in `text`.
    pub fn find_hits(&self, text: &str) -> Vec<Hit> {
        let Some(ac) = &self.ac else {
            return Vec::new();
        };
        let norm = normalize(text);
        ac.find_iter(&norm)
            .map(|m| Hit {
                name_id: self.name_ids[m.pattern().as_usize()],
                start: m.start(),
                end: m.end(),
                form: norm[m.start()..m.end()].to_string(),
            })
            .collect()
    }

    /// Distinct name_ids appearing in `text` (for glossary-prompt injection and
    /// stale-TU detection).
    pub fn name_ids_in(&self, text: &str) -> Vec<i64> {
        let mut ids: Vec<i64> = self.find_hits(text).into_iter().map(|h| h.name_id).collect();
        ids.sort_unstable();
        ids.dedup();
        ids
    }
}

#[cfg(test)]
mod tests {
    use super::*;

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
}
