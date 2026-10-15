//! OpenAI-compatible chat completions + responses normalization.

use serde_json::Value;

use crate::adapter::{AdapterError, message};
use crate::ir::{NormalizedRequest, Role, ToolCall};
use crate::token_est::text_from_content;

pub fn parse(v: &Value) -> Result<NormalizedRequest, AdapterError> {
    let mut req = NormalizedRequest {
        model: v
            .get("model")
            .and_then(Value::as_str)
            .unwrap_or("unknown")
            .to_string(),
        stream: v.get("stream").and_then(Value::as_bool).unwrap_or(false),
        temperature: v.get("temperature").and_then(Value::as_f64),
        tools: v
            .get("tools")
            .and_then(Value::as_array)
            .cloned()
            .unwrap_or_default(),
        ..Default::default()
    };
    if let Some(messages) = v.get("messages").and_then(Value::as_array) {
        for m in messages {
            req.messages.push(parse_message(m));
        }
    }
    Ok(req)
}

fn parse_message(m: &Value) -> crate::ir::NormMessage {
    let role = match m.get("role").and_then(Value::as_str).unwrap_or("user") {
        "system" => Role::System,
        "assistant" => Role::Assistant,
        "tool" => Role::Tool,
        _ => Role::User,
    };
    let text = m.get("content").map(text_from_content).unwrap_or_default();
    let mut msg = message(role, text);
    msg.tool_call_id = m
        .get("tool_call_id")
        .and_then(Value::as_str)
        .map(str::to_string);
    if let Some(calls) = m.get("tool_calls").and_then(Value::as_array) {
        for call in calls {
            if let Some(func) = call.get("function") {
                let name = func
                    .get("name")
                    .and_then(Value::as_str)
                    .unwrap_or("")
                    .to_string();
                let args = func
                    .get("arguments")
                    .and_then(Value::as_str)
                    .and_then(|s| serde_json::from_str(s).ok())
                    .unwrap_or(Value::Null);
                let id = call.get("id").and_then(Value::as_str).map(str::to_string);
                msg.tool_calls.push(ToolCall { id, name, args });
            }
        }
    }
    msg
}

#[cfg(test)]
mod tests {

    use crate::adapter::normalize;
    use crate::ir::{Provider, Role};

    const BODY: &str = r#"{
      "model": "gpt-4o-mini",
      "stream": true,
      "temperature": 0.2,
      "messages": [
        {"role": "system", "content": "You are terse."},
        {"role": "user", "content": [{"type": "text", "text": "read main.rs"}]},
        {"role": "assistant", "tool_calls": [
          {"id": "c1", "type": "function",
           "function": {"name": "read_file", "arguments": "{\"path\":\"src/main.rs\",\"ts\":123}"}}
        ]},
        {"role": "tool", "tool_call_id": "c1", "content": "fn main() {}"}
      ],
      "tools": [{"type": "function", "function": {"name": "read_file", "parameters": {"type": "object"}}}]
    }"#;

    #[test]
    fn parses_messages_and_tool_calls() {
        let req = normalize(Provider::OpenAi, BODY.as_bytes(), None).unwrap();
        assert_eq!(req.model, "gpt-4o-mini");
        assert!(req.stream);
        assert_eq!(req.messages.len(), 4);
        assert_eq!(req.messages[0].role, Role::System);
        assert_eq!(req.messages[1].text, "read main.rs");
        assert_eq!(req.messages[2].tool_calls[0].name, "read_file");
        assert_eq!(
            req.messages[2].tool_calls[0].args.get("path").unwrap(),
            "src/main.rs"
        );
        assert_eq!(req.messages[3].role, Role::Tool);
        assert_eq!(req.tools.len(), 1);
        assert!(req.request_text.contains("You are terse."));
    }

    #[test]
    fn rejects_non_json() {
        assert!(normalize(Provider::OpenAi, b"not json", None).is_err());
    }
}
