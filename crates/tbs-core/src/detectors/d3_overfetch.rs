//! D3 — over-fetch detection for tool outputs (docs/architecture/03 §3).
//!
//! Flag when a tool result exceeds its size cap AND downstream reference rate
//! stays under 0.15. Observe/advise only in Phase 0; clamping is Phase 1.

use std::collections::HashSet;

use crate::detectors::{OverFetchItem, Verdict};
use crate::events::DetectorKind;
use crate::ir::{NormalizedRequest, Role};
use crate::policy::CompiledPolicy;
use crate::token_est::estimate_tokens;

pub const DEFAULT_CAP_TOKENS: u32 = 2000;
pub const REFERENCE_TRIP: f64 = 0.15;
const MAX_RESEND_MULT: u32 = 12;
const MAX_SCAN_BYTES: usize = 512 * 1024;

pub fn analyze(req: &NormalizedRequest, policy: &CompiledPolicy) -> (Verdict, Vec<OverFetchItem>) {
    let rule = policy.rule(DetectorKind::OverFetch);
    let default_cap = rule
        .and_then(|r| r.param_u32("default_cap_tokens"))
        .unwrap_or(DEFAULT_CAP_TOKENS);

    let mut items = Vec::new();
    for (i, msg) in req.messages.iter().enumerate() {
        if msg.role != Role::Tool {
            continue;
        }
        let size = estimate_tokens(&msg.text);
        let cap = msg
            .name
            .as_deref()
            .and_then(|t| rule.and_then(|r| r.per_tool_cap(t)))
            .unwrap_or(default_cap);
        if size <= cap {
            continue;
        }
        let referenced = referenced_frac(req, i, &msg.text);
        if referenced >= REFERENCE_TRIP {
            continue;
        }
        let later = req.messages.len().saturating_sub(i + 1) as u32;
        items.push(OverFetchItem {
            tool: msg.name.clone().unwrap_or_else(|| "tool".into()),
            size_tokens: size,
            cap_tokens: cap,
            referenced_frac: referenced,
            excess_tokens: size - cap,
            resend_mult: (1 + later).min(MAX_RESEND_MULT),
        });
    }

    let total_excess: u64 = items.iter().map(|i| u64::from(i.excess_tokens)).sum();
    let score = (total_excess as f64 / 4000.0).min(1.0);
    let verdict = Verdict::new(
        DetectorKind::OverFetch,
        score,
        !items.is_empty(),
        [
            ("flagged_count", items.len() as f64),
            ("total_excess_tokens", total_excess as f64),
            (
                "max_size_tokens",
                items
                    .iter()
                    .map(|i| u64::from(i.size_tokens))
                    .max()
                    .unwrap_or(0) as f64,
            ),
        ],
    );
    (verdict, items)
}

/// Word 5-gram containment of the result in all later messages of the trajectory.
fn referenced_frac(req: &NormalizedRequest, index: usize, result_text: &str) -> f64 {
    use crate::hash::shingle_hashes;
    let result_shingles = shingle_hashes(result_text, 5);
    if result_shingles.is_empty() {
        return 0.0;
    }
    let mut later = String::new();
    for msg in &req.messages[index + 1..] {
        later.push_str(&msg.text);
        later.push('\n');
        if later.len() >= MAX_SCAN_BYTES {
            break;
        }
    }
    let later_shingles: HashSet<u64> = shingle_hashes(&later, 5).into_iter().collect();
    let hits = result_shingles
        .iter()
        .filter(|s| later_shingles.contains(s))
        .count();
    hits as f64 / result_shingles.len() as f64
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::adapter::normalize;
    use crate::ir::Provider;

    fn body(messages: &str) -> String {
        format!(r#"{{"model":"gpt-4o-mini","messages":{messages}}}"#)
    }

    #[test]
    fn big_unreferenced_tool_result_flags() {
        let filler: Vec<String> = (0..300)
            .map(|i| {
                format!(
                    "- file entry {:04}: generated artifact line with details",
                    i
                )
            })
            .collect();
        let content = filler.join("\n");
        let messages = format!(
            r#"[{{"role":"user","content":"list"}},
               {{"role":"assistant","tool_calls":[{{"id":"a","type":"function","function":{{"name":"ls","arguments":"{{}}"}}}}]}},
               {{"role":"tool","tool_call_id":"a","content":{}}},
               {{"role":"user","content":"and now?"}}]"#,
            serde_json::to_string(&content).unwrap()
        );
        let req = normalize(Provider::OpenAi, body(&messages).as_bytes(), None).unwrap();
        let (v, items) = analyze(&req, &CompiledPolicy::default());
        assert_eq!(items.len(), 1, "items: {:?}", items);
        assert!(v.would_act);
        assert!(items[0].referenced_frac < REFERENCE_TRIP);
        assert!(items[0].resend_mult >= 2);
    }

    #[test]
    fn referenced_tool_result_passes() {
        let line = "CRITICAL: rotation key is qa7-live-9931 and must be rotated";
        let quoted = line.repeat(150);
        let messages = format!(
            r#"[{{"role":"user","content":"list"}},
               {{"role":"assistant","tool_calls":[{{"id":"a","type":"function","function":{{"name":"ls","arguments":"{{}}"}}}}]}},
               {{"role":"tool","tool_call_id":"a","content":{}}},
               {{"role":"user","content":"use the details: {line} please"}}]"#,
            serde_json::to_string(&quoted).unwrap()
        );
        let req = normalize(Provider::OpenAi, body(&messages).as_bytes(), None).unwrap();
        let (_, items) = analyze(&req, &CompiledPolicy::default());
        assert!(items.is_empty(), "items: {:?}", items);
    }

    #[test]
    fn per_tool_cap_applies() {
        let yaml = r#"
apiVersion: tbs.dev/v1
kind: Policy
metadata: { project: p, revision: 1 }
spec:
  mode: observe
  rules:
    - id: clamp-tool-outputs
      detector: over_fetch
      actuator: tool_output_clamp
      mode: observe
      rollout_percent: 100
      params:
        default_cap_tokens: 5
        per_tool: { search: { cap: 4000 } }
"#;
        let policy = CompiledPolicy::from_yaml_str(yaml).unwrap();
        let big = "search result line with plenty of content here".repeat(50);
        let messages = format!(
            r#"[{{"role":"assistant","tool_calls":[{{"id":"a","type":"function","function":{{"name":"search","arguments":"{{}}"}}}}]}},
               {{"role":"tool","tool_call_id":"a","content":{}}}]"#,
            serde_json::to_string(&big).unwrap()
        );
        let req = normalize(Provider::OpenAi, body(&messages).as_bytes(), None).unwrap();
        let (_, items) = analyze(&req, &policy);
        assert!(items.is_empty(), "per-tool cap should skip: {:?}", items);
    }

    #[test]
    fn schema_lint_counts_redundant_fields() {
        let tools = vec![serde_json::json!({
            "type": "function",
            "function": {
                "name": "lookup",
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
        let (v, findings) = crate::detectors::d4_verbosity::analyze_schema(&tools);
        assert!(findings.redundant_fields >= 3, "{findings:?}");
        assert!(findings.reducible_tokens > 0);
        assert!(v.score > 0.0);
        assert!(findings.reducible_tokens <= findings.schema_tokens);
    }
}
