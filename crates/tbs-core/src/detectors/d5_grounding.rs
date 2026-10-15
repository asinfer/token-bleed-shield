//! D5 — hallucination-risk grounding score (docs/architecture/03 §5).
//!
//! Phase 0 ships the score only: verifiable atoms in the streamed output that
//! never appear in the trajectory context. Enforce (quarantine) is Phase 2+.

use std::collections::HashSet;
use std::sync::LazyLock;

use regex::Regex;

use crate::detectors::Verdict;
use crate::events::DetectorKind;

static FILE_RE: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"[\w./\-]+\.(?:rs|py|ts|tsx|js|jsx|go|java|c|cpp|h|hpp|json|ya?ml|toml|md)\b")
        .expect("regex")
});
static FUNC_RE: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"\b[a-z_][a-z0-9_]{2,24}\s*\(").expect("regex"));
static UNIT_NUM_RE: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"\b\d+(?:\.\d+)?\s?(?:ms|kb|mb|gb|tb|%|x|tok)\b").expect("regex"));
static REF_RE: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"\b(?:usr|arn|s3|acct):[\w./\-]+").expect("regex"));

const MIN_ATOMS: usize = 3;
const MAX_ATOMS: usize = 64;
const MAX_CONTEXT_SCAN: usize = 1024 * 1024;

pub fn analyze(window_text: &str, request_text: &str) -> Verdict {
    let context = &request_text[..request_text.len().min(MAX_CONTEXT_SCAN)];
    let mut atoms: HashSet<String> = HashSet::new();
    for re in [&*FILE_RE, &*FUNC_RE, &*UNIT_NUM_RE, &*REF_RE] {
        for m in re.find_iter(window_text) {
            let atom = m.as_str().trim_end_matches('(').to_string();
            if atom.len() < 4 {
                continue;
            }
            atoms.insert(atom);
            if atoms.len() >= MAX_ATOMS {
                break;
            }
        }
    }
    let total = atoms.len();
    let unsupported = atoms
        .iter()
        .filter(|a| !context.contains(a.as_str()))
        .count();
    let density = if total >= MIN_ATOMS {
        unsupported as f64 / total as f64
    } else {
        0.0
    };
    Verdict::new(
        DetectorKind::HallucinationRisk,
        density,
        false,
        [
            ("atoms_total", total as f64),
            ("atoms_unsupported", unsupported as f64),
            ("unsupported_density", density),
        ],
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ungrounded_paths_raise_density() {
        let context = "The project only contains main.rs and config parsing.";
        let output = "Files src/secret_module.rs, src/hidden_bridge.rs and helpers in \
                      fetch_tenant_cache() will need a rebuild (120kb) \
                      plus usr:admin ops map, arn:aws:iam::123:role/x, and rewrite_config() later.";
        let v = analyze(output, context);
        assert!(v.score >= 0.5, "score {}", v.score);
        assert!(!v.would_act);
        let signals = &v.signals;
        assert!(signals["atoms_unsupported"] >= 3.0);
    }

    #[test]
    fn grounded_output_scores_low() {
        let context =
            "Edit retry_budget() in policy.rs; the breaker trips after 3 ms delays of 40kb each.";
        let output =
            "You can adjust retry_budget() in policy.rs; each pass waits 3 ms and buffers 40kb.";
        let v = analyze(output, context);
        assert!(v.score < 0.25, "score {}", v.score);
    }

    #[test]
    fn too_few_atoms_scores_zero() {
        let v = analyze("hello world", "hello world");
        assert_eq!(v.score, 0.0);
    }
}
