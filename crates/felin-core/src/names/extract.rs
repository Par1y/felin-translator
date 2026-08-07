//! The project-level name-extraction pass: ask the LLM, per chapter, for
//! candidate proper nouns as JSON; parse tolerantly; dedup by normalized form.
//! Non-fatal — a chapter whose response can't be parsed is skipped.

use crate::llm::{extract_json, ChatMessage, LlmClient};
use crate::names::normalize::normalize;
use serde::Deserialize;
use std::collections::BTreeMap;

/// A raw proper-noun candidate returned by the model.
#[derive(Debug, Clone, Deserialize, PartialEq, Eq)]
pub struct Candidate {
    pub japanese: String,
    #[serde(default, alias = "chinese", alias = "zh")]
    pub guess_chinese: String,
    /// Category the model proposes (人名/地名/…). Falls back to `guess_tags[0]`
    /// when the model answered with a `tags` array instead.
    #[serde(default)]
    pub category: String,
    #[serde(default)]
    pub guess_tags: Vec<String>,
    #[serde(default)]
    pub context: String,
}

impl Candidate {
    /// The first usable category the model proposed (explicit `category` first,
    /// else `guess_tags[0]`), trimmed. Empty when neither was given.
    pub fn proposed_category(&self) -> String {
        let c = self.category.trim();
        if !c.is_empty() {
            return c.to_string();
        }
        self.guess_tags.first().map(|t| t.trim().to_string()).unwrap_or_default()
    }
}

/// Run extraction over `chapters` (each entry is a chapter's full text), using
/// the user-configurable system message `extract_system` from `felin.toml
/// [prompt]` (the config file is the single source of truth — an empty string
/// sends no system message, only the chapter text). Returns candidates
/// deduplicated by normalized japanese form.
pub async fn extract_names(
    client: &LlmClient,
    chapters: &[String],
    extract_system: &str,
) -> Vec<Candidate> {
    let mut merged: BTreeMap<String, Candidate> = BTreeMap::new();
    for (i, text) in chapters.iter().enumerate() {
        if text.trim().is_empty() {
            tracing::debug!(chapter = i, "chapter skipped (empty text)");
            continue;
        }
        let messages = if extract_system.trim().is_empty() {
            vec![ChatMessage::user(text.clone())]
        } else {
            vec![ChatMessage::system(extract_system), ChatMessage::user(text.clone())]
        };
        let resp = match client.chat(&messages).await {
            Ok(r) => r,
            Err(e) => {
                tracing::warn!(chapter = i, error = %e, "name-extraction call failed; skipping");
                continue;
            }
        };
        match parse_candidates(&resp) {
            Some(list) => {
                tracing::debug!(chapter = i, candidates = list.len(), "chapter name extraction done");
                for c in list {
                    if c.japanese.trim().is_empty() {
                        continue;
                    }
                    merged.entry(normalize(c.japanese.trim())).or_insert(c);
                }
            }
            None => tracing::warn!(chapter = i, "unparseable extraction JSON; skipping chapter"),
        }
    }
    merged.into_values().collect()
}

/// Parse an LLM response into candidates (tolerant of prose / code fences).
pub fn parse_candidates(resp: &str) -> Option<Vec<Candidate>> {
    serde_json::from_value::<Vec<Candidate>>(extract_json(resp)?).ok()
}

/// One japanese form's proposed category, as returned by the classification
/// pass ([`classify_names`]).
#[derive(Debug, Clone, Deserialize)]
pub struct TagSuggestion {
    #[serde(default)]
    pub japanese: String,
    #[serde(default)]
    pub category: String,
}

/// Ask the LLM to classify proper nouns (人名/地名/…) and return the
/// per-form category. `classify_system` comes from `felin.toml [prompt]
/// extract_tags_system` (empty → the caller must refuse; this returns an empty
/// vec). `forms` are deduplicated and normalized before the call; unmatched or
/// unparseable responses are skipped (non-fatal), and the empty category is
/// dropped from the result.
pub async fn classify_names(
    client: &LlmClient,
    forms: &[String],
    classify_system: &str,
) -> Vec<TagSuggestion> {
    if classify_system.trim().is_empty() {
        tracing::warn!("extract_tags_system is empty; auto-tag refused");
        return Vec::new();
    }
    let mut distinct: Vec<String> = Vec::new();
    for f in forms {
        let n = normalize(f.trim());
        if !n.is_empty() && !distinct.contains(&n) {
            distinct.push(n);
        }
    }
    if distinct.is_empty() {
        return Vec::new();
    }
    let user = distinct.join("\n");
    let messages = vec![ChatMessage::system(classify_system), ChatMessage::user(user)];
    let Ok(resp) = client.chat(&messages).await else {
        tracing::warn!("auto-tag classification call failed; no tags applied");
        return Vec::new();
    };
    let Ok(list) = serde_json::from_value::<Vec<TagSuggestion>>(
        extract_json(&resp).unwrap_or(serde_json::Value::Null),
    ) else {
        tracing::warn!("unparseable auto-tag JSON; no tags applied");
        return Vec::new();
    };
    let mut seen = std::collections::HashSet::new();
    list.into_iter()
        .filter_map(|s| {
            let n = normalize(s.japanese.trim());
            if n.is_empty() || s.category.trim().is_empty() || !seen.insert(n.clone()) {
                return None;
            }
            Some(TagSuggestion { japanese: n, category: s.category.trim().to_string() })
        })
        .collect()
}

