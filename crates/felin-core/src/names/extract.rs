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

const EXTRACT_SYSTEM: &str = "你是日文专有名词抽取助手。从给定日文文本中抽取专有名词（人名、地名、\
组织、作品名、独特术语等），忽略普通词汇。只输出 JSON 数组，每项形如 \
{\"japanese\":\"原文形式\",\"guess_chinese\":\"推测中文\",\"context\":\"简短出处\"}，\
不要输出任何其他文字。";

/// Run extraction over `chapters` (each entry is a chapter's full text).
/// Returns candidates deduplicated by normalized japanese form.
pub async fn extract_names(client: &LlmClient, chapters: &[String]) -> Vec<Candidate> {
    let mut merged: BTreeMap<String, Candidate> = BTreeMap::new();
    for (i, text) in chapters.iter().enumerate() {
        if text.trim().is_empty() {
            continue;
        }
        let messages = [ChatMessage::system(EXTRACT_SYSTEM), ChatMessage::user(text.clone())];
        let resp = match client.chat(&messages).await {
            Ok(r) => r,
            Err(e) => {
                tracing::warn!(chapter = i, error = %e, "name-extraction call failed; skipping");
                continue;
            }
        };
        match parse_candidates(&resp) {
            Some(list) => {
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_candidates_from_fenced_json() {
        let resp = "```json\n[{\"japanese\":\"田中\",\"guess_chinese\":\"田中\",\"context\":\"主人公\"},\
                    {\"japanese\":\"東京\",\"chinese\":\"东京\"}]\n```";
        let list = parse_candidates(resp).unwrap();
        assert_eq!(list.len(), 2);
        assert_eq!(list[0].japanese, "田中");
        assert_eq!(list[0].guess_chinese, "田中");
        // `chinese` is accepted as an alias for `guess_chinese`.
        assert_eq!(list[1].guess_chinese, "东京");
        assert_eq!(list[1].context, "");
    }

    #[test]
    fn returns_none_for_unparseable() {
        assert!(parse_candidates("抱歉，无法输出。").is_none());
    }
}
