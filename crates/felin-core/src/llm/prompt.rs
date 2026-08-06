//! Prompt assembly for the plaintext translation contract.

use crate::llm::ChatMessage;

/// Inputs for one translation prompt. Assembly order (per plan): system (总则)
/// + instruction (per-item, higher priority than 总则) + glossary references +
/// optional context (previous approved translation, for style/naming continuity)
/// + source.
#[derive(Debug, Clone, Default)]
pub struct TranslateRequest {
    pub system: String,
    pub instruction: Option<String>,
    pub glossary: Option<String>,
    pub context: Option<String>,
    pub source: String,
}

/// Build the chat messages for a translation request.
pub fn build_messages(req: &TranslateRequest) -> Vec<ChatMessage> {
    let mut msgs = Vec::new();
    if !req.system.trim().is_empty() {
        msgs.push(ChatMessage::system(req.system.clone()));
    }
    if let Some(instr) = req.instruction.as_ref().filter(|s| !s.trim().is_empty()) {
        msgs.push(ChatMessage::system(format!("附加要求（优先级高于总则）：{instr}")));
    }
    if let Some(gloss) = req.glossary.as_ref().filter(|s| !s.trim().is_empty()) {
        msgs.push(ChatMessage::system(format!("专名参考（词表，必须使用）：\n{gloss}")));
    }
    let mut user = String::new();
    if let Some(ctx) = req.context.as_ref().filter(|s| !s.trim().is_empty()) {
        user.push_str("【上文参考（已校对，仅供风格与称谓参考，勿重复翻译）】\n");
        user.push_str(ctx);
        user.push_str("\n\n");
    }
    user.push_str("【待翻译原文】\n");
    user.push_str(&req.source);
    msgs.push(ChatMessage::user(user));
    msgs
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::llm::Role;

    #[test]
    fn assembles_system_instruction_glossary_context_source_in_order() {
        let req = TranslateRequest {
            system: "保持原排版。".into(),
            instruction: Some("句尾用“呢”。".into()),
            glossary: Some("田中 → 田中\n佐藤 → 佐藤".into()),
            context: Some("前文译文。".into()),
            source: "本文。".into(),
        };
        let m = build_messages(&req);
        assert_eq!(m.len(), 4);
        assert_eq!(m[0].role, Role::System);
        assert_eq!(m[0].content, "保持原排版。");
        assert_eq!(m[1].role, Role::System);
        assert!(m[1].content.contains("句尾用“呢”。"));
        assert_eq!(m[2].role, Role::System);
        assert!(m[2].content.contains("专名参考"));
        assert!(m[2].content.contains("田中 → 田中"));
        assert_eq!(m[3].role, Role::User);
        assert!(m[3].content.contains("前文译文。"));
        assert!(m[3].content.contains("本文。"));
    }

    #[test]
    fn omits_empty_system_instruction_glossary_and_context() {
        let req = TranslateRequest { source: "唯一原文。".into(), ..Default::default() };
        let m = build_messages(&req);
        assert_eq!(m.len(), 1);
        assert_eq!(m[0].role, Role::User);
        assert!(m[0].content.contains("唯一原文。"));
        assert!(!m[0].content.contains("上文参考"));
        assert!(!m[0].content.contains("专名参考"));
    }
}
