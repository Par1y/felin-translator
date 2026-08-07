//! Prompt assembly for the plaintext translation contract.

use crate::llm::ChatMessage;

/// Inputs for one translation prompt. Assembly (per plan): 总则 + instruction
/// (per-item, higher priority than 总则) + glossary references + optional
/// context (previous approved translation) + source.
///
/// The message *framing* is fully config-driven — there is no built-in prompt
/// text in the code. The user-supplied templates come from `felin.toml
/// [prompt]` and are baked into [`TranslateRequest::system_template`] /
/// [`TranslateRequest::user_template`]; an empty template means the message
/// section is simply not sent (the config file is the single source of truth).
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
    let sys = render_template(
        &req.system_template,
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
    let user = render_template(
        &req.user_template,
        &[("context", req.context.as_deref()), ("source", Some(&req.source))],
    );
    msgs.push(ChatMessage::user(if user.trim().is_empty() { req.source.clone() } else { user }));
    msgs
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

