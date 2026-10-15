//! Token estimation table (chars/4 heuristic), flagged `estimated` in the ledger
//! when provider usage is absent (docs/architecture/02 §2.6).

use serde_json::Value;

pub fn estimate_tokens(text: &str) -> u32 {
    let chars = text.chars().count();
    if chars == 0 {
        return 0;
    }
    chars.div_ceil(4) as u32
}

pub fn estimate_tokens_json(v: &Value) -> u32 {
    estimate_tokens(&serde_json::to_string(v).unwrap_or_default())
}

/// Flatten provider content (string | array of parts) into plain text.
pub fn text_from_content(v: &Value) -> String {
    match v {
        Value::String(s) => s.clone(),
        Value::Array(parts) => {
            let mut out = String::new();
            for part in parts {
                if let Some(t) = part.get("text").and_then(Value::as_str) {
                    if !out.is_empty() {
                        out.push('\n');
                    }
                    out.push_str(t);
                }
            }
            out
        }
        Value::Null => String::new(),
        other => other.to_string(),
    }
}
