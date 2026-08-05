//! Tolerant JSON extraction for LLM output. The name-extraction pass asks the
//! model for a JSON array; models often wrap it in prose or a ```json fence, so
//! we parse directly, then fall back to stripping fences and pulling out the
//! first balanced `[...]` / `{...}`.

use serde_json::Value;

/// Try to extract a JSON value from `text`.
pub fn extract_json(text: &str) -> Option<Value> {
    let trimmed = text.trim();
    if let Ok(v) = serde_json::from_str::<Value>(trimmed) {
        return Some(v);
    }
    let unfenced = strip_code_fence(trimmed);
    if let Ok(v) = serde_json::from_str::<Value>(unfenced.trim()) {
        return Some(v);
    }
    let candidate = first_balanced(unfenced)?;
    serde_json::from_str::<Value>(&candidate).ok()
}

/// Strip a leading ```json / ``` fence (and its trailing ```), if present.
fn strip_code_fence(s: &str) -> &str {
    let t = s.trim();
    if let Some(rest) = t.strip_prefix("```") {
        // Drop the optional language tag on the fence's first line.
        let after = rest.splitn(2, '\n').nth(1).unwrap_or("");
        return after.trim().trim_end_matches("```").trim();
    }
    t
}

/// Return the first balanced `[...]` or `{...}` substring, honoring quoted
/// strings and escapes. Structural characters are ASCII, so byte scanning is
/// safe over UTF-8 content.
fn first_balanced(s: &str) -> Option<String> {
    let bytes = s.as_bytes();
    let start = bytes.iter().position(|&b| b == b'[' || b == b'{')?;
    let open = bytes[start];
    let close = if open == b'[' { b']' } else { b'}' };
    let mut depth = 0i32;
    let mut in_str = false;
    let mut escaped = false;
    for (i, &b) in bytes.iter().enumerate().skip(start) {
        if in_str {
            if escaped {
                escaped = false;
            } else if b == b'\\' {
                escaped = true;
            } else if b == b'"' {
                in_str = false;
            }
        } else if b == b'"' {
            in_str = true;
        } else if b == open {
            depth += 1;
        } else if b == close {
            depth -= 1;
            if depth == 0 {
                return Some(s[start..=i].to_string());
            }
        }
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_direct_array() {
        let v = extract_json(r#"[{"japanese":"田中","guess_chinese":"田中"}]"#).unwrap();
        assert!(v.is_array());
        assert_eq!(v[0]["japanese"], "田中");
    }

    #[test]
    fn extracts_from_code_fence() {
        let s = "```json\n[{\"a\":1}]\n```";
        assert_eq!(extract_json(s).unwrap(), serde_json::json!([{"a":1}]));
    }

    #[test]
    fn extracts_from_surrounding_prose() {
        let s = "候选如下：\n[{\"japanese\":\"猫\"}]\n以上。";
        let v = extract_json(s).unwrap();
        assert_eq!(v[0]["japanese"], "猫");
    }

    #[test]
    fn ignores_brackets_inside_strings() {
        let v = extract_json(r#"prefix ["a]b", "c"] suffix"#).unwrap();
        assert_eq!(v, serde_json::json!(["a]b", "c"]));
    }

    #[test]
    fn returns_none_for_no_json() {
        assert!(extract_json("すみません、JSONを出力できません。").is_none());
    }

    #[test]
    fn handles_object() {
        assert_eq!(extract_json("note {\"k\": true} end").unwrap(), serde_json::json!({"k": true}));
    }
}
