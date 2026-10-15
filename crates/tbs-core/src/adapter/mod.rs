//! Protocol adapters: normalize provider wire formats into `NormalizedRequest`
//! but never mutate forwarded bytes in observe mode (docs/architecture/02 §1).

pub mod anthropic;
pub mod openai;

use std::sync::LazyLock;

use regex::Regex;
use serde_json::Value;
use thiserror::Error;

use crate::ir::{MAX_REQUEST_TEXT_BYTES, NormMessage, NormalizedRequest, Provider, Role};
use crate::scanner::SseFrame;

#[derive(Debug, Error)]
pub enum AdapterError {
    #[error("request body is not valid JSON")]
    InvalidJson,
    #[error("request body is not a JSON object")]
    NotAnObject,
}

#[derive(Debug, Clone, Copy, Default, PartialEq)]
pub struct PartialUsage {
    pub input: Option<u32>,
    pub output: Option<u32>,
    pub cache_read: Option<u32>,
}

impl PartialUsage {
    pub fn merge(&mut self, other: PartialUsage) {
        self.input = other.input.or(self.input);
        self.output = other.output.or(self.output);
        self.cache_read = other.cache_read.or(self.cache_read);
    }

    pub fn reported(&self) -> bool {
        self.input.is_some() || self.output.is_some()
    }
}

pub fn detect_provider(path: &str) -> Provider {
    if path.starts_with("/v1/messages") || path.starts_with("/v1/complete") {
        Provider::Anthropic
    } else {
        Provider::OpenAi
    }
}

pub fn normalize(
    provider: Provider,
    body: &[u8],
    feature_label: Option<String>,
) -> Result<NormalizedRequest, AdapterError> {
    let value: Value = serde_json::from_slice(body).map_err(|_| AdapterError::InvalidJson)?;
    if !value.is_object() {
        return Err(AdapterError::NotAnObject);
    }
    let mut req = match provider {
        Provider::OpenAi => openai::parse(&value)?,
        Provider::Anthropic => anthropic::parse(&value)?,
    };
    req.provider = Some(provider);
    req.feature_label = feature_label;
    resolve_tool_names(&mut req.messages);
    req.request_text = build_request_text(&req);
    Ok(req)
}

pub fn build_request_text(req: &NormalizedRequest) -> String {
    let mut out = String::new();
    for msg in &req.messages {
        if !matches!(msg.role, Role::System) {
            continue;
        }
        append_capped(&mut out, &msg.text);
    }
    for msg in &req.messages {
        append_capped(&mut out, &msg.text);
        for call in &msg.tool_calls {
            append_capped(&mut out, &call.name);
            append_capped(&mut out, &call.args.to_string());
        }
    }
    for tool in &req.tools {
        append_capped(&mut out, &tool.to_string());
    }
    out
}

fn append_capped(out: &mut String, text: &str) {
    if out.len() >= MAX_REQUEST_TEXT_BYTES {
        return;
    }
    out.push_str(text);
    out.push('\n');
    if out.len() > MAX_REQUEST_TEXT_BYTES {
        let mut cut = MAX_REQUEST_TEXT_BYTES;
        while !out.is_char_boundary(cut) {
            cut -= 1;
        }
        out.truncate(cut);
    }
}

pub fn message(role: Role, text: String) -> NormMessage {
    NormMessage {
        role,
        text,
        tool_calls: Vec::new(),
        tool_call_id: None,
        name: None,
    }
}

/// Resolve `role=tool` message names from assistant tool_call ids, in both providers.
pub fn resolve_tool_names(messages: &mut [NormMessage]) {
    let mut id_to_name: std::collections::HashMap<String, String> =
        std::collections::HashMap::new();
    for msg in messages.iter() {
        for call in &msg.tool_calls {
            if let Some(id) = &call.id {
                id_to_name.insert(id.clone(), call.name.clone());
            }
        }
    }
    for msg in messages.iter_mut() {
        if msg.role == crate::ir::Role::Tool
            && msg.name.is_none()
            && let Some(id) = &msg.tool_call_id
            && let Some(name) = id_to_name.get(id)
        {
            msg.name = Some(name.clone());
        }
    }
}

/// Extract streamed assistant text from a frame, provider-specific.
pub fn extract_delta_text(provider: Provider, frame: &SseFrame) -> Option<String> {
    match provider {
        Provider::OpenAi => {
            let v: Value = serde_json::from_str(&frame.data).ok()?;
            let delta = v.get("choices")?.get(0)?.get("delta")?;
            let content = delta.get("content").and_then(Value::as_str)?;
            Some(content.to_string())
        }
        Provider::Anthropic => match frame.event.as_deref() {
            Some("content_block_delta") => {
                let v: Value = serde_json::from_str(&frame.data).ok()?;
                v.get("delta")?.get("text")?.as_str().map(str::to_string)
            }
            _ => None,
        },
    }
}

pub fn extract_usage(provider: Provider, frame: &SseFrame) -> PartialUsage {
    match provider {
        Provider::OpenAi => {
            let Ok(v) = serde_json::from_str::<Value>(&frame.data) else {
                return PartialUsage::default();
            };
            let Some(usage) = v.get("usage").filter(|u| !u.is_null()) else {
                return PartialUsage::default();
            };
            PartialUsage {
                input: usage
                    .get("prompt_tokens")
                    .and_then(Value::as_u64)
                    .map(|n| n as u32),
                output: usage
                    .get("completion_tokens")
                    .and_then(Value::as_u64)
                    .map(|n| n as u32),
                cache_read: usage
                    .get("prompt_tokens_details")
                    .and_then(|d| d.get("cached_tokens"))
                    .and_then(Value::as_u64)
                    .map(|n| n as u32),
            }
        }
        Provider::Anthropic => {
            let Ok(v) = serde_json::from_str::<Value>(&frame.data) else {
                return PartialUsage::default();
            };
            match frame.event.as_deref() {
                Some("message_start") => {
                    let usage = v.get("message").and_then(|m| m.get("usage"));
                    PartialUsage {
                        input: usage
                            .and_then(|u| u.get("input_tokens"))
                            .and_then(Value::as_u64)
                            .map(|n| n as u32),
                        output: None,
                        cache_read: usage
                            .and_then(|u| u.get("cache_read_input_tokens"))
                            .and_then(Value::as_u64)
                            .map(|n| n as u32),
                    }
                }
                Some("message_delta") => PartialUsage {
                    input: None,
                    output: v
                        .get("usage")
                        .and_then(|u| u.get("output_tokens"))
                        .and_then(Value::as_u64)
                        .map(|n| n as u32),
                    cache_read: None,
                },
                _ => PartialUsage::default(),
            }
        }
    }
}

pub fn is_done(provider: Provider, frame: &SseFrame) -> bool {
    match provider {
        Provider::OpenAi => frame.data.trim() == "[DONE]",
        Provider::Anthropic => frame.event.as_deref() == Some("message_stop"),
    }
}

static PROMPT_TOKENS_RE: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r#""prompt_tokens"\s*:\s*(\d+)"#).expect("regex"));
static COMPLETION_TOKENS_RE: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r#""completion_tokens"\s*:\s*(\d+)"#).expect("regex"));
static INPUT_TOKENS_RE: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r#""input_tokens"\s*:\s*(\d+)"#).expect("regex"));
static OUTPUT_TOKENS_RE: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r#""output_tokens"\s*:\s*(\d+)"#).expect("regex"));

/// Sniff usage integers from a bounded JSON tail (non-SSE responses only).
pub fn sniff_usage_json(tail: &str) -> PartialUsage {
    fn cap(re: &Regex, text: &str) -> Option<u32> {
        re.captures(text)
            .and_then(|c| c.get(1))
            .and_then(|m| m.as_str().parse().ok())
    }
    PartialUsage {
        input: cap(&PROMPT_TOKENS_RE, tail).or_else(|| cap(&INPUT_TOKENS_RE, tail)),
        output: cap(&COMPLETION_TOKENS_RE, tail).or_else(|| cap(&OUTPUT_TOKENS_RE, tail)),
        cache_read: None,
    }
}
