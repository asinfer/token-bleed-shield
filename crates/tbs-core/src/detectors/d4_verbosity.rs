//! D4 — verbosity / filler classification (docs/architecture/03 §4).
//!
//! Static: JSON-schema lint (redundant titles/descriptions/examples) whose
//! savings are countable per dispatch → ledger `measured` class.
//! Dynamic: filler-phrase ratio over the stream window (score only in v1;
//! output compression is a future opt-in actuator, 04 §1.5).

use std::sync::LazyLock;

use regex::Regex;
use serde_json::Value;

use crate::detectors::{SchemaFindings, Verdict};
use crate::events::DetectorKind;
use crate::token_est::estimate_tokens_json;

static FILLER_RE: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(concat!(
        r"(?i)\b(certainly|of course|sure thing|great question|as an ai\b|i hope this helps|",
        r"it'?s important to note|in conclusion|to summarize|in summary|let me walk you through|",
        r"let me start by|here'?s a (?:step|detailed|comprehensive)|i'?ll go ahead and|",
        r"feel free to|firstly,? let me|without further ado|in this (?:guide|section))"
    ))
    .expect("regex")
});

const MIN_REDUCIBLE_TOKENS: u32 = 8;
const MAX_SCHEMA_WALK: usize = 500;

pub fn filler_chars(text: &str) -> usize {
    FILLER_RE.find_iter(text).map(|m| m.end() - m.start()).sum()
}

pub fn filler_ratio(text: &str) -> f64 {
    if text.is_empty() {
        return 0.0;
    }
    filler_chars(text) as f64 / text.len() as f64
}

pub fn analyze_schema(tools: &[Value]) -> (Verdict, SchemaFindings) {
    let mut findings = SchemaFindings::default();
    let mut redundant_chars = 0usize;
    let mut redundant_hits = 0u32;
    let mut visited = 0usize;

    for tool in tools {
        let params = tool
            .get("function")
            .and_then(|f| f.get("parameters"))
            .or_else(|| tool.get("input_schema"))
            .or_else(|| tool.get("parameters"));
        let Some(params) = params else { continue };
        findings.schema_tokens += estimate_tokens_json(params);
        walk(
            params,
            &mut redundant_chars,
            &mut redundant_hits,
            &mut visited,
            0,
        );
    }
    findings.redundant_fields = redundant_hits;
    let reducible = redundant_chars.div_ceil(4) as u32;
    findings.reducible_tokens = reducible.min(findings.schema_tokens);

    let score = if findings.schema_tokens == 0 {
        0.0
    } else {
        findings.reducible_tokens as f64 / findings.schema_tokens as f64
    };
    let verdict = Verdict::new(
        DetectorKind::Verbosity,
        score,
        findings.reducible_tokens >= MIN_REDUCIBLE_TOKENS,
        [
            ("schema_tokens", findings.schema_tokens as f64),
            ("schema_reducible_tokens", findings.reducible_tokens as f64),
            ("redundant_fields", findings.redundant_fields as f64),
        ],
    );
    (verdict, findings)
}

fn walk(
    node: &Value,
    redundant_chars: &mut usize,
    redundant_hits: &mut u32,
    visited: &mut usize,
    depth: usize,
) {
    if depth > 6 || *visited > MAX_SCHEMA_WALK {
        return;
    }
    match node {
        Value::Object(map) => {
            *visited += 1;
            if let Some(props) = map.get("properties").and_then(Value::as_object) {
                for (name, prop) in props {
                    score_property(name, prop, redundant_chars, redundant_hits);
                    walk(prop, redundant_chars, redundant_hits, visited, depth + 1);
                }
            }
            if let Some(items) = map.get("items") {
                walk(items, redundant_chars, redundant_hits, visited, depth + 1);
            }
            if let Some(defs) = map.get("$defs").or_else(|| map.get("definitions")) {
                walk(defs, redundant_chars, redundant_hits, visited, depth + 1);
            }
        }
        Value::Array(items) => {
            for item in items {
                walk(item, redundant_chars, redundant_hits, visited, depth + 1);
            }
        }
        _ => {}
    }
}

fn score_property(name: &str, prop: &Value, redundant_chars: &mut usize, redundant_hits: &mut u32) {
    if let Some(title) = prop.get("title").and_then(Value::as_str)
        && title.eq_ignore_ascii_case(name)
    {
        *redundant_chars += title.len() + 8;
        *redundant_hits += 1;
    }
    if let Some(desc) = prop.get("description").and_then(Value::as_str) {
        let typ = prop.get("type").and_then(Value::as_str).unwrap_or("");
        if desc.eq_ignore_ascii_case(name) || desc.eq_ignore_ascii_case(typ) {
            *redundant_chars += desc.len() + 16;
            *redundant_hits += 1;
        }
    }
    if (prop.get("example").is_some() || prop.get("examples").is_some())
        && !matches!(
            prop.get("type").and_then(Value::as_str),
            Some("object") | Some("array")
        )
    {
        *redundant_chars += 24;
        *redundant_hits += 1;
    }
}

pub fn finalize_stream(filler_chars: u64, output_chars: u64, window_text: &str) -> Verdict {
    let ratio = if output_chars == 0 {
        0.0
    } else {
        filler_chars as f64 / output_chars as f64
    };
    Verdict::new(
        DetectorKind::Verbosity,
        ratio,
        false,
        [
            ("filler_ratio", ratio),
            ("filler_chars", filler_chars as f64),
            ("output_chars", output_chars as f64),
            ("output_tokens_est", (output_chars / 4) as f64),
            ("window_bytes", window_text.len() as f64),
        ],
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn filler_phrases_are_counted() {
        let text = "Certainly! Great question. Let me walk you through the answer step by step.";
        assert!(filler_chars(text) > 10);
        assert!(filler_ratio(text) > 0.1);
    }

    #[test]
    fn technical_prose_has_low_filler() {
        let text = "The retry budget applies per trajectory; identical canonical tool arguments \
                    increment a counter that trips the breaker after three consecutive hits.";
        assert!(filler_ratio(text) < 0.02, "{}", filler_ratio(text));
    }

    #[test]
    fn static_schema_minify_is_countable() {
        let tools = vec![serde_json::json!({
            "type": "function",
            "function": {
                "name": "lookup",
                "description": "lookup",
                "parameters": {
                    "type": "object",
                    "properties": {
                        "query": {"type": "string", "title": "query", "description": "query",
                                   "examples": ["abc", "def"]},
                        "limit": {"type": "integer", "description": "integer"}
                    }
                }
            }
        })];
        let (verdict, findings) = analyze_schema(&tools);
        assert!(findings.schema_tokens > 0);
        assert!(
            findings.reducible_tokens >= MIN_REDUCIBLE_TOKENS,
            "{findings:?}"
        );
        assert!(verdict.would_act);
    }

    #[test]
    fn empty_tools_score_zero() {
        let (verdict, findings) = analyze_schema(&[]);
        assert_eq!(verdict.score, 0.0);
        assert!(!verdict.would_act);
        assert_eq!(findings.schema_tokens, 0);
    }
}
