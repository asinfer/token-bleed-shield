//! D2 — context-rot scoring (docs/architecture/03 §2).
//!
//! Phase 0 ships the score only: cross-message duplication + structural bloat
//! noise. Staleness and grounding weighting join the score when trajectory age
//! and L1 features land; hygiene actuation stays off (04 §1.5).

use std::collections::HashSet;

use regex::Regex;
use std::sync::LazyLock;

use crate::detectors::Verdict;
use crate::events::DetectorKind;
use crate::hash::fnv1a64;
use crate::ir::{NormalizedRequest, Role};

static STACKTRACE_RE: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(
        r#"(?m)^(\s+at [\w.$]+\(|Traceback \(most recent call last\)|\s*File "[^"]+", line \d+)"#,
    )
    .expect("regex")
});
static BASE64_RE: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"[A-Za-z0-9+/]{200,}={0,2}").expect("regex"));
static LOG_LINE_RE: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"(?m)^(?:\S+\s+){0,2}(INFO|WARN(ING)?|ERROR|DEBUG|TRACE)\b").expect("regex")
});

const MAX_LINES_PER_MESSAGE: usize = 400;
const MAX_NOISE_SCAN_BYTES: usize = 512 * 1024;

pub fn analyze(req: &NormalizedRequest) -> Verdict {
    let dup_frac = duplication_frac(req);
    let (noise_frac, scanned) = noise_frac(req);
    let score = 0.5 * dup_frac + 0.5 * noise_frac;
    Verdict::new(
        DetectorKind::ContextRot,
        score,
        false,
        [
            ("dup_frac", dup_frac),
            ("noise_frac", noise_frac),
            ("stale_frac", 0.0),
            ("noise_scan_bytes", scanned as f64),
            ("message_count", req.messages.len() as f64),
        ],
    )
}

fn duplication_frac(req: &NormalizedRequest) -> f64 {
    let mut seen: HashSet<u64> = HashSet::new();
    let mut dup = 0u64;
    let mut total = 0u64;
    for msg in &req.messages {
        if matches!(msg.role, Role::Tool) {
            continue;
        }
        for line in msg.text.lines().take(MAX_LINES_PER_MESSAGE) {
            let line = line.trim();
            if line.chars().count() < 24 {
                continue;
            }
            let h = fnv1a64(line.to_lowercase().as_bytes());
            total += 1;
            if !seen.insert(h) {
                dup += 1;
            }
        }
    }
    if total == 0 {
        return 0.0;
    }
    dup as f64 / total as f64
}

fn noise_frac(req: &NormalizedRequest) -> (f64, usize) {
    let mut text = String::new();
    for msg in &req.messages {
        text.push_str(&msg.text);
        text.push('\n');
        if text.len() >= MAX_NOISE_SCAN_BYTES {
            break;
        }
    }
    text.truncate(MAX_NOISE_SCAN_BYTES);
    if text.is_empty() {
        return (0.0, 0);
    }
    let mut noise = 0usize;
    for m in STACKTRACE_RE.find_iter(&text) {
        noise += m.end() - m.start();
    }
    for m in BASE64_RE.find_iter(&text) {
        noise += m.end() - m.start();
    }
    for m in LOG_LINE_RE.find_iter(&text) {
        noise += m.end() - m.start();
    }
    let scanned = text.len();
    (noise as f64 / scanned as f64, scanned)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::adapter::normalize;
    use crate::ir::Provider;

    #[test]
    fn stacktrace_and_base64_raise_the_score() {
        let noisy = r#"{
          "model": "gpt-4o",
          "messages": [
            {"role":"user","content":"why did this fail?"},
            {"role":"assistant","content":"Traceback (most recent call last):\n  File \"app.py\", line 12, in run\n    raise ValueError(\"x\")\nAAAA"},
            {"role":"user","content":"again"},
            {"role":"assistant","content":"Traceback (most recent call last):\n  File \"app.py\", line 12, in run\n    raise ValueError(\"x\")\nAAAA"}
          ]
        }"#;
        let req = normalize(Provider::OpenAi, noisy.as_bytes(), None).unwrap();
        let v = analyze(&req);
        assert!(v.score > 0.1, "score {}", v.score);
        assert!(!v.would_act);
    }

    #[test]
    fn clean_conversation_scores_low() {
        let clean = r#"{
          "model": "gpt-4o",
          "messages": [
            {"role":"user","content":"summarize the quarterly report for leadership"},
            {"role":"assistant","content":"Revenue grew twelve percent driven by enterprise renewals while churn stayed flat."},
            {"role":"user","content":"now draft the investor note"}
          ]
        }"#;
        let req = normalize(Provider::OpenAi, clean.as_bytes(), None).unwrap();
        let v = analyze(&req);
        assert!(v.score < 0.1, "score {}", v.score);
    }
}
