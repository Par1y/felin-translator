//! Prompt assembly for the plaintext translation contract.

use crate::llm::ChatMessage;

/// Default translation system-message template. Placeholders: `{guidelines}`
/// (project 总则), `{instruction}` (per-TU 附加要求, higher priority than 总则),
/// `{glossary}` (matched 词表). Blank-line-separated blocks whose placeholders
/// are all empty are dropped, so optional sections vanish cleanly. Used when
/// `[prompt].translation_system` is empty.
pub const DEFAULT_TRANSLATION_SYSTEM: &str = concat!(
    "{guidelines}\n\n",
    "附加要求（优先级高于总则）：\n",
    "{instruction}\n\n",
    "专名参考（词表，必须使用）：\n",
    "{glossary}",
);

/// Default translation user-message template. Placeholders: `{context}`
/// (previous approved translation, for style/naming continuity), `{source}`
/// (the TU source). The `{context}` block is dropped when empty.
pub const DEFAULT_TRANSLATION_USER: &str = concat!(
    "【上文参考（已校对，仅供风格与称谓参考，勿重复翻译）】\n",
    "{context}\n\n",
    "【待翻译原文】\n",
    "{source}",
);

/// Inputs for one translation prompt. Assembly (per plan): 总则 + instruction
/// (per-item, higher priority than 总则) + glossary references + optional
/// context (previous approved translation) + source. The message *framing* is
/// driven by [`TranslateRequest::system_template`] /
/// [`TranslateRequest::user_template`] — editable via `felin.toml [prompt]`;
/// empty templates fall back to the [`DEFAULT_TRANSLATION_SYSTEM`] /
/// [`DEFAULT_TRANSLATION_USER`] defaults.
#[derive(Debug, Clone, Default)]
pub struct TranslateRequest {
    /// 总则 (project guidelines) — fills the `{guidelines}` placeholder.
    pub guidelines: String,
    pub instruction: Option<String>,
    pub glossary: Option<String>,
    pub context: Option<String>,
    pub source: String,
    /// System-message template (`{guidelines}` / `{instruction}` / `{glossary}`).
    pub system_template: String,
    /// User-message template (`{context}` / `{source}`).
    pub user_template: String,
}

/// Build the chat messages for a translation request.
pub fn build_messages(req: &TranslateRequest) -> Vec<ChatMessage> {
    let system_template = effective(&req.system_template, DEFAULT_TRANSLATION_SYSTEM);
    let sys = render_template(
        system_template,
        &[
            ("guidelines", Some(&req.guidelines)),
            ("instruction", req.instruction.as_deref()),
            ("glossary", req.glossary.as_deref()),
        ],
    );
    let mut msgs = Vec::new();
    if !sys.trim().is_empty() {
        msgs.push(ChatMessage::system(sys));
    }
    let user_template = effective(&req.user_template, DEFAULT_TRANSLATION_USER);
    let user = render_template(
        user_template,
        &[("context", req.context.as_deref()), ("source", Some(&req.source))],
    );
    msgs.push(ChatMessage::user(if user.trim().is_empty() { req.source.clone() } else { user }));
    msgs
}

/// `s` when non-empty, else the built-in `default` (空 = 内置默认).
fn effective<'a>(s: &'a str, default: &'a str) -> &'a str {
    if s.trim().is_empty() {
        default
    } else {
        s
    }
}

/// Fill `{name}` placeholders in `template`. Blank-line-separated paragraph
/// blocks are dropped when *every* placeholder they reference is empty (e.g. a
/// TU with no instruction/glossary/context), so optional sections vanish
/// cleanly. Blocks with no placeholders are kept verbatim.
fn render_template(template: &str, vars: &[(&str, Option<&str>)]) -> String {
    let rendered: Vec<String> = template
        .split("\n\n")
        .filter_map(|block| render_block(block, vars))
        .collect();
    rendered.join("\n\n")
}

/// Render one paragraph block, or `None` to drop it (no non-empty placeholder
/// filled in it).
fn render_block(block: &str, vars: &[(&str, Option<&str>)]) -> Option<String> {
    let block = block.trim();
    if block.is_empty() {
        return None;
    }
    let mut out = String::new();
    let mut rest = block;
    let mut referenced = false;
    let mut any_filled = false;
    while let Some(open) = rest.find('{') {
        let after = &rest[open + 1..];
        let Some(close) = after.find('}') else { break };
        let name = &after[..close];
        out.push_str(&rest[..open]);
        referenced = true;
        let filled = vars
            .iter()
            .find(|(k, _)| *k == name)
            .and_then(|(_, v)| *v)
            .map(str::trim)
            .filter(|s| !s.is_empty());
        if let Some(v) = filled {
            out.push_str(v);
            any_filled = true;
        }
        rest = &after[close + 1..];
    }
    if !referenced {
        return Some(block.to_string());
    }
    if any_filled {
        out.push_str(rest);
        Some(out)
    } else {
        None
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::llm::Role;

    #[test]
    fn assembles_default_system_and_user_from_placeholders() {
        let req = TranslateRequest {
            guidelines: "保持原排版。".into(),
            instruction: Some("句尾用“呢”。".into()),
            glossary: Some("田中 → 田中\n佐藤 → 佐藤".into()),
            context: Some("前文译文。".into()),
            source: "本文。".into(),
            system_template: DEFAULT_TRANSLATION_SYSTEM.to_string(),
            user_template: DEFAULT_TRANSLATION_USER.to_string(),
        };
        let m = build_messages(&req);
        assert_eq!(m.len(), 2);
        assert_eq!(m[0].role, Role::System);
        assert!(m[0].content.contains("保持原排版。"));
        assert!(m[0].content.contains("句尾用“呢”。"));
        assert!(m[0].content.contains("专名参考（词表，必须使用）："));
        assert!(m[0].content.contains("田中 → 田中"));
        assert_eq!(m[1].role, Role::User);
        assert!(m[1].content.contains("前文译文。"));
        assert!(m[1].content.contains("本文。"));
    }

    #[test]
    fn omits_empty_instruction_glossary_and_context_blocks() {
        let req = TranslateRequest {
            guidelines: "总则".into(),
            source: "唯一原文。".into(),
            system_template: DEFAULT_TRANSLATION_SYSTEM.to_string(),
            user_template: DEFAULT_TRANSLATION_USER.to_string(),
        };
        let m = build_messages(&req);
        assert_eq!(m.len(), 2);
        assert_eq!(m[0].role, Role::System);
        assert_eq!(m[0].content, "总则");
        assert_eq!(m[1].role, Role::User);
        assert_eq!(m[1].content, "【待翻译原文】\n唯一原文。");
        assert!(!m[1].content.contains("上文参考"));
    }

    #[test]
    fn custom_templates_are_used_verbatim() {
        let req = TranslateRequest {
            guidelines: "总则".into(),
            source: "原文".into(),
            system_template: "自定义：{guidelines} 优先".into(),
            user_template: "{source}｜完".into(),
            ..Default::default()
        };
        let m = build_messages(&req);
        assert_eq!(m.len(), 2);
        assert_eq!(m[0].content, "自定义：总则 优先");
        assert_eq!(m[1].content, "原文｜完");
    }
}
