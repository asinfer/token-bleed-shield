//! Anthropic Messages API normalization (system splitting, tool_use/tool_result blocks).

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
    if let Some(system) = v.get("system") {
        req.messages
            .push(message(Role::System, text_from_content(system)));
    }
    if let Some(messages) = v.get("messages").and_then(Value::as_array) {
        for m in messages {
            parse_message(m, &mut req);
        }
    }
    Ok(req)
}

fn parse_message(m: &Value, req: &mut NormalizedRequest) {
    let role = match m.get("role").and_then(Value::as_str).unwrap_or("user") {
        "assistant" => Role::Assistant,
        _ => Role::User,
    };
    let content = m.get("content");
    let Some(blocks) = content.and_then(Value::as_array) else {
        let text = content.map(text_from_content).unwrap_or_default();
        req.messages.push(message(role, text));
        return;
    };

    let mut main = message(role, String::new());
    for block in blocks {
        match block.get("type").and_then(Value::as_str).unwrap_or("") {
            "text" => {
                if let Some(t) = block.get("text").and_then(Value::as_str) {
                    if !main.text.is_empty() {
                        main.text.push('\n');
                    }
                    main.text.push_str(t);
                }
            }
            "tool_use" => {
                let name = block
                    .get("name")
                    .and_then(Value::as_str)
                    .unwrap_or("")
                    .to_string();
                let args = block.get("input").cloned().unwrap_or(Value::Null);
                let id = block.get("id").and_then(Value::as_str).map(str::to_string);
                main.tool_calls.push(ToolCall { id, name, args });
            }
            "tool_result" => {
                let mut tool_msg = message(
                    Role::Tool,
                    text_from_content(block.get("content").unwrap_or(&Value::Null)),
                );
                tool_msg.tool_call_id = block
                    .get("tool_use_id")
                    .and_then(Value::as_str)
                    .map(str::to_string);
                req.messages.push(tool_msg);
            }
            _ => {}
        }
    }
    if !main.text.is_empty() || !main.tool_calls.is_empty() {
        req.messages.push(main);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::adapter::{extract_delta_text, extract_usage, normalize};
    use crate::ir::Provider;
    use crate::scanner::SseFrame;

    const BODY: &str = r#"{
      "model": "claude-sonnet-4",
      "stream": true,
      "system": [{"type": "text", "text": "Be brief."}],
      "messages": [
        {"role": "user", "content": [{"type": "text", "text": "list files"}]},
        {"role": "assistant", "content": [
          {"type": "tool_use", "id": "tu_1", "name": "ls", "input": {"path": "."}}
        ]},
        {"role": "user", "content": [
          {"type": "tool_result", "tool_use_id": "tu_1", "content": "a.rs\nb.rs"}
        ]}
      ],
      "tools": [{"name": "ls", "input_schema": {"type": "object"}}]
    }"#;

    #[test]
    fn parses_system_and_blocks() {
        let req = normalize(Provider::Anthropic, BODY.as_bytes(), None).unwrap();
        assert_eq!(req.messages[0].role, Role::System);
        assert_eq!(req.messages[0].text, "Be brief.");
        assert_eq!(req.messages[1].text, "list files");
        assert_eq!(req.messages[2].tool_calls[0].name, "ls");
        assert_eq!(req.messages[3].role, Role::Tool);
        assert_eq!(req.messages[3].tool_call_id.as_deref(), Some("tu_1"));
    }

    #[test]
    fn extracts_delta_and_usage_from_frames() {
        let delta = SseFrame {
            event: Some("content_block_delta".into()),
            data: r#"{"delta":{"type":"text_delta","text":"hi"}}"#.into(),
        };
        assert_eq!(
            extract_delta_text(Provider::Anthropic, &delta).as_deref(),
            Some("hi")
        );
        let start = SseFrame {
            event: Some("message_start".into()),
            data: r#"{"message":{"usage":{"input_tokens":42}}}"#.into(),
        };
        assert_eq!(extract_usage(Provider::Anthropic, &start).input, Some(42));
        let stop = SseFrame {
            event: Some("message_delta".into()),
            data: r#"{"usage":{"output_tokens":7}}"#.into(),
        };
        assert_eq!(extract_usage(Provider::Anthropic, &stop).output, Some(7));
    }
}
