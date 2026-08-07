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
    #[serde(default)]
    pub context: String,
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

