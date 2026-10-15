//! `NormalizedRequest` — the internal IR every adapter produces (docs/architecture/02 §1).

use serde_json::Value;

pub const MAX_REQUEST_TEXT_BYTES: usize = 2 * 1024 * 1024;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Provider {
    OpenAi,
    Anthropic,
}

impl Provider {
    pub fn as_str(self) -> &'static str {
        match self {
            Provider::OpenAi => "openai",
            Provider::Anthropic => "anthropic",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Role {
    System,
    User,
    Assistant,
    Tool,
}

#[derive(Debug, Clone)]
pub struct ToolCall {
    pub id: Option<String>,
    pub name: String,
    pub args: Value,
}

#[derive(Debug, Clone)]
pub struct NormMessage {
    pub role: Role,
    pub text: String,
    pub tool_calls: Vec<ToolCall>,
    pub tool_call_id: Option<String>,
    /// Tool name for `role=tool` messages when resolvable from the id map.
    pub name: Option<String>,
}

#[derive(Debug, Clone, Default)]
pub struct NormalizedRequest {
    pub provider: Option<Provider>,
    pub model: String,
    pub stream: bool,
    pub temperature: Option<f64>,
    pub messages: Vec<NormMessage>,
    pub tools: Vec<Value>,
    pub feature_label: Option<String>,
    pub request_text: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum UsageSource {
    Reported,
    Estimated,
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Usage {
    pub input_tokens: u32,
    pub output_tokens: u32,
    pub cache_read_tokens: u32,
    pub source: UsageSource,
}

impl Default for Usage {
    fn default() -> Self {
        Self {
            input_tokens: 0,
            output_tokens: 0,
            cache_read_tokens: 0,
            source: UsageSource::Estimated,
        }
    }
}
