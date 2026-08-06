//! TU → `TranslateRequest` assembly: 总则 + instruction + glossary + context +
//! source, with the truncation rules (truncate context and 总则, never source).

use crate::llm::TranslateRequest;
use crate::names::{normalize, Hit};
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

#[cfg(test)]
mod tests {
    use super::*;

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
        );
        assert_eq!(req.context.as_ref().unwrap().chars().count(), 101); // 100 + '…'
        assert_eq!(req.source.chars().count(), 50_000);
    }
}
