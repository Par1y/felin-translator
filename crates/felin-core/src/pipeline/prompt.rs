//! TU → `TranslateRequest` assembly: 总则 + instruction + glossary + context +
//! source, with the truncation rules (truncate context and 总则, never source).

use crate::error::Result;
use crate::llm::TranslateRequest;
use crate::names::{normalize, Hit, Matcher};
use crate::types::{GlossaryEntry, MatchedName};
use std::collections::HashMap;

/// The project's default 总则 template (editable; persisted per project).
pub fn default_guidelines() -> String {
    concat!(
        "你是日译中翻译校对助手。请把日文原文翻译成简体中文。\n",
        "规则：\n",
        "- 保持原文排版与空行结构，段落对应关系不变。\n",
        "- 对话与引用格式保持一致。\n",
        "- 称呼与敬称按中文习惯处理；专名必须使用词表译名。\n",
        "- 只输出译文本身，不要任何解释、注释或额外内容。"
    )
    .to_string()
}

/// Truncate `s` to at most `max` chars at a character boundary (append `…`
/// when cut). Used for 总则 and context — never for the source itself.
pub fn truncate_chars(s: &str, max: usize) -> String {
    let count = s.chars().count();
    if count <= max {
        return s.to_string();
    }
    let mut out: String = s.chars().take(max).collect();
    out.push('…');
    out
}

/// De-duplicate matcher hits by name id (keeping first-occurrence order) and
/// format a `「日文 → 中文」` reference block using `lookup`. Returns `None`
/// when no distinct names matched.
pub fn glossary_block(
    hits: &[Hit],
    lookup: &HashMap<i64, (String, Option<String>)>,
) -> Option<String> {
    let mut seen = std::collections::HashSet::new();
    let mut lines = Vec::new();
    for h in hits {
        if !seen.insert(h.name_id) {
            continue;
        }
        let Some((jp, zh)) = lookup.get(&h.name_id) else { continue };
        let jp = normalize(jp);
        if jp.is_empty() {
            continue;
        }
        match zh {
            Some(z) if !z.trim().is_empty() => lines.push(format!("{jp} → {z}")),
            _ => lines.push(jp),
        }
    }
    if lines.is_empty() {
        None
    } else {
        Some(lines.join("\n"))
    }
}

/// Compiled project-glossary matching data. The canonical japanese forms feed
/// the Aho-Corasick matcher; `lookup` maps each entry id to its (canonical
/// japanese, chinese) rendering. Built **once** over the *enabled* entries
/// ([`crate::storage::ProjectDb::matcher_entries`]) and reused across many TUs —
/// by the pipeline (one compile per run) and by the review query (one compile
/// per chapter listing).
pub struct GlossaryMatcher {
    /// Leftmost-longest matcher over the canonical japanese forms.
    pub matcher: Matcher,
    /// Entry id → (canonical japanese, chinese rendering) for surfaced names.
    pub lookup: HashMap<i64, (String, Option<String>)>,
}

impl GlossaryMatcher {
    /// Compile over `entries`. The caller decides the entry set — prompt
    /// injection and the review query both pass the *enabled* entries so the
    /// surfaced names are exactly what translation applied. An empty set yields
    /// a matcher that finds nothing (`None`).
    pub fn build(entries: &[GlossaryEntry]) -> Result<Option<Self>> {
        if entries.is_empty() {
            return Ok(None);
        }
        let mut forms: Vec<(String, i64)> = Vec::new();
        let mut lookup: HashMap<i64, (String, Option<String>)> = HashMap::new();
        for e in entries {
            forms.push((e.japanese.clone(), e.id));
            lookup.insert(e.id, (e.japanese.clone(), e.chinese.clone()));
        }
        let matcher = Matcher::build(&forms)?;
        Ok(Some(Self { matcher, lookup }))
    }

    /// The distinct entries `source` matches, de-duplicated by entry id in
    /// first-occurrence order — what prompt injection applied to this TU.
    pub fn matched_names(&self, source: &str) -> Vec<MatchedName> {
        let hits = self.matcher.find_hits(source);
        matched_names(&hits, &self.lookup)
    }
}

/// One-shot [`GlossaryMatcher`]: compile `entries` and return the distinct names
/// `source` matches. For a single source (tests / ad-hoc queries). Callers
/// processing many sources should reuse [`GlossaryMatcher`] so the matcher
/// compiles once.
pub fn tu_matched_names(source: &str, entries: &[GlossaryEntry]) -> Result<Vec<MatchedName>> {
    Ok(GlossaryMatcher::build(entries)?.map_or_else(Vec::new, |g| g.matched_names(source)))
}

/// Map matcher hits to distinct `{japanese, chinese}` pairs, de-duplicated by
/// entry id in first-occurrence order — the structured twin of
/// [`glossary_block`] for the review UI. Canonical japanese forms are
/// normalized; an entry without a non-blank Chinese keeps `chinese: None`.
pub fn matched_names(
    hits: &[Hit],
    lookup: &HashMap<i64, (String, Option<String>)>,
) -> Vec<MatchedName> {
    let mut seen = std::collections::HashSet::new();
    let mut out = Vec::new();
    for h in hits {
        if !seen.insert(h.name_id) {
            continue;
        }
        let Some((jp, zh)) = lookup.get(&h.name_id) else { continue };
        let jp = normalize(jp);
        if jp.is_empty() {
            continue;
        }
        out.push(MatchedName { japanese: jp, chinese: zh.clone() });
    }
    out
}

/// Assemble one TU's translation request. `glossary`/`context`/`instruction`
/// are the optional blocks; 总则 and context are truncated per limits, the
/// source passes through untouched. `system_template`/`user_template` come from
/// `felin.toml [prompt]` (empty → built-in defaults, see [`crate::llm`]).
#[allow(clippy::too_many_arguments)]
pub fn build_tu_request(
    system: String,
    guidelines_max_chars: usize,
    instruction: Option<String>,
    glossary: Option<String>,
    context: Option<String>,
    context_max_chars: usize,
    source: String,
    system_template: String,
    user_template: String,
) -> TranslateRequest {
    TranslateRequest {
        guidelines: truncate_chars(&system, guidelines_max_chars),
        instruction: instruction.map(|s| truncate_chars(&s, guidelines_max_chars)),
        glossary: glossary.map(|s| truncate_chars(&s, guidelines_max_chars)),
        context: context.map(|s| truncate_chars(&s, context_max_chars)),
        source,
        system_template,
        user_template,
    }
}

