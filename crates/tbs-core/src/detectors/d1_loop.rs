//! D1 — loop / repetition detection (docs/architecture/03 §1).
//!
//! L0: canonical tool-arg hashing + distinct-state ratio over trajectory
//! metadata, n-gram repetition over the streaming window. Enforce wiring (circuit
//! breaker) is Phase 1; observe mode scores and trips `would_act` only.

use std::collections::HashMap;

use crate::detectors::Verdict;
use crate::events::DetectorKind;
use crate::hash::{canonical_tool_args, fnv1a64, sha256_16};
use crate::ir::{NormalizedRequest, Role};
use crate::policy::CompiledPolicy;

pub const IDENTICAL_CALL_TRIP: u32 = 2;
pub const DISTINCT_RATIO_TRIP: f64 = 0.3;
pub const ERROR_RUN_TRIP: u32 = 3;
pub const REPETITION_TRIP: f64 = 0.5;
pub const MIN_SHINGLES: u64 = 24;
const MAX_TOOL_CALL_WINDOW: usize = 12;

const ERROR_MARKERS: &[&str] = &[
    "traceback",
    "exception",
    "permission denied",
    "segmentation fault",
    "error:",
    "failed",
    "exit code 1",
    "command not found",
];

pub fn analyze_request(req: &NormalizedRequest, policy: &CompiledPolicy) -> Verdict {
    let mut call_hashes: Vec<u64> = Vec::new();
    for msg in &req.messages {
        if msg.role != Role::Assistant {
            continue;
        }
        for call in &msg.tool_calls {
            let canon = format!("{}\u{1}{}", call.name, canonical_tool_args(&call.args));
            call_hashes.push(fnv1a64(canon.as_bytes()));
        }
    }
    let start = call_hashes.len().saturating_sub(MAX_TOOL_CALL_WINDOW);
    let call_hashes = call_hashes[start..].to_vec();

    let total_calls = call_hashes.len() as u64;
    let mut freq: HashMap<u64, u64> = HashMap::new();
    for h in &call_hashes {
        *freq.entry(*h).or_insert(0) += 1;
    }
    let identical_call_max = freq.values().copied().max().unwrap_or(0) as u32;
    let distinct_ratio = if total_calls == 0 {
        1.0
    } else {
        freq.len() as f64 / total_calls as f64
    };

    let error_run = trailing_error_run(req);

    let min_window = policy
        .rule(DetectorKind::LoopRepetition)
        .and_then(|r| r.param_u32("min_window"))
        .unwrap_or(3)
        .min(2);

    let s_ident = f64::from(identical_call_max) / 4.0;
    let s_distinct = if total_calls >= 8 {
        1.0 - distinct_ratio
    } else {
        0.0
    };
    let s_err = f64::from(error_run) / f64::from(ERROR_RUN_TRIP);
    let score = s_ident.max(s_distinct).max(s_err);

    let would_act = (total_calls >= u64::from(min_window)
        && identical_call_max >= IDENTICAL_CALL_TRIP)
        || (total_calls >= 8 && distinct_ratio < DISTINCT_RATIO_TRIP)
        || error_run >= ERROR_RUN_TRIP;

    Verdict::new(
        DetectorKind::LoopRepetition,
        score,
        would_act,
        [
            ("identical_call_max", f64::from(identical_call_max)),
            ("distinct_state_ratio", distinct_ratio),
            ("error_signature_run", f64::from(error_run)),
            ("tool_calls_in_window", total_calls as f64),
        ],
    )
}

fn trailing_error_run(req: &NormalizedRequest) -> u32 {
    let mut run = 0u32;
    for msg in req.messages.iter().rev() {
        if msg.role != Role::Tool {
            continue;
        }
        let lower = msg.text.to_lowercase();
        if ERROR_MARKERS.iter().any(|m| lower.contains(m)) {
            run += 1;
        } else {
            break;
        }
    }
    run
}

pub fn finalize_stream(shingle_total: u64, shingle_repeats: u64, window_text: &str) -> Verdict {
    let rate = if shingle_total >= MIN_SHINGLES {
        shingle_repeats as f64 / shingle_total as f64
    } else {
        0.0
    };
    Verdict::new(
        DetectorKind::LoopRepetition,
        rate,
        shingle_total >= MIN_SHINGLES && rate >= REPETITION_TRIP,
        [
            ("repetition_rate", rate),
            ("shingle_total", shingle_total as f64),
            ("shingle_repeats", shingle_repeats as f64),
            ("window_bytes", window_text.len() as f64),
        ],
    )
}

/// Content-addressed trajectory key fallback when no header/user id exists.
pub fn trajectory_fallback_key(req: &NormalizedRequest) -> String {
    let mut seed = String::new();
    for msg in &req.messages {
        if matches!(msg.role, Role::System | Role::User) {
            seed.push_str(&crate::hash::template_skeleton(&msg.text));
            break;
        }
    }
    sha256_16(seed)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::adapter::normalize;
    use crate::ir::Provider;

    fn openai_body(messages: &str) -> String {
        format!(r#"{{"model":"gpt-4o","messages":{messages}}}"#)
    }

    #[test]
    fn identical_tool_calls_trip() {
        let body = openai_body(
            r#"[
              {"role":"user","content":"check"},
              {"role":"assistant","tool_calls":[{"id":"a","type":"function","function":{"name":"read_file","arguments":"{\"path\":\"src/lib.rs\"}"}}]},
              {"role":"tool","tool_call_id":"a","content":"ok"},
              {"role":"assistant","tool_calls":[{"id":"b","type":"function","function":{"name":"read_file","arguments":"{\"path\":\"src/lib.rs\"}"}}]},
              {"role":"tool","tool_call_id":"b","content":"ok"}
            ]"#,
        );
        let req = normalize(Provider::OpenAi, body.as_bytes(), None).unwrap();
        let v = analyze_request(&req, &CompiledPolicy::default());
        assert!(v.would_act, "signals: {:?}", v.signals);
        assert!(v.signals["identical_call_max"] >= 2.0);
    }

    #[test]
    fn distinct_tool_calls_do_not_trip() {
        let body = openai_body(
            r#"[
              {"role":"user","content":"scan"},
              {"role":"assistant","tool_calls":[{"id":"a","type":"function","function":{"name":"read_file","arguments":"{\"path\":\"a.rs\"}"}}]},
              {"role":"tool","tool_call_id":"a","content":"ok"},
              {"role":"assistant","tool_calls":[{"id":"b","type":"function","function":{"name":"read_file","arguments":"{\"path\":\"b.rs\"}"}}]},
              {"role":"tool","tool_call_id":"b","content":"ok"},
              {"role":"assistant","tool_calls":[{"id":"c","type":"function","function":{"name":"read_file","arguments":"{\"path\":\"c.rs\"}"}}]},
              {"role":"tool","tool_call_id":"c","content":"ok"},
              {"role":"assistant","tool_calls":[{"id":"d","type":"function","function":{"name":"read_file","arguments":"{\"path\":\"d.rs\"}"}}]},
              {"role":"tool","tool_call_id":"d","content":"ok"}
            ]"#,
        );
        let req = normalize(Provider::OpenAi, body.as_bytes(), None).unwrap();
        let v = analyze_request(&req, &CompiledPolicy::default());
        assert!(!v.would_act, "signals: {:?}", v.signals);
        assert_eq!(v.signals["distinct_state_ratio"], 1.0);
    }

    #[test]
    fn volatile_args_do_not_mask_identity() {
        let mut a = serde_json::json!({"path": "x.rs", "timestamp": 111});
        let mut b = serde_json::json!({"path": "x.rs", "timestamp": 999});
        crate::hash::strip_volatile(&mut a);
        crate::hash::strip_volatile(&mut b);
        assert_eq!(
            crate::hash::canonical_json(&a),
            crate::hash::canonical_json(&b)
        );
    }

    #[test]
    fn consecutive_error_signatures_trip() {
        let body = openai_body(
            r#"[
              {"role":"tool","tool_call_id":"a","content":"Error: boom"},
              {"role":"tool","tool_call_id":"b","content":"Error: boom again"},
              {"role":"tool","tool_call_id":"c","content":"exit code 1"}
            ]"#,
        );
        let req = normalize(Provider::OpenAi, body.as_bytes(), None).unwrap();
        let v = analyze_request(&req, &CompiledPolicy::default());
        assert!(v.would_act, "signals: {:?}", v.signals);
    }

    #[test]
    fn stream_repetition_trips_and_distinct_passes() {
        let phrase = "same tool call again ";
        let loop_text = phrase.repeat(12);
        let shingles = crate::hash::shingle_hashes(&loop_text, 8);
        let mut seen = crate::window::SeenShingles::new(1024);
        let (total, repeats) = seen.observe(&shingles);
        assert!(total >= MIN_SHINGLES, "shingles {total}");
        let v = finalize_stream(total, repeats, &loop_text);
        assert!(v.would_act, "score {}", v.score);

        let fresh = "alpha bravo charlie delta echo foxtrot golf hotel india juliet \
                     kilo lima mike november oscar papa quebec romeo sierra tango";
        let shingles = crate::hash::shingle_hashes(fresh, 8);
        let mut seen = crate::window::SeenShingles::new(1024);
        let (total, repeats) = seen.observe(&shingles);
        let v = finalize_stream(total, repeats, fresh);
        assert!(!v.would_act);
    }
}
