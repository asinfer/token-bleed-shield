//! L0 detectors (docs/architecture/03): zero marginal cost heuristics over
//! request IR + streaming windows. Detection runs in every mode; nothing here
//! mutates traffic.

pub mod d1_loop;
pub mod d2_rot;
pub mod d3_overfetch;
pub mod d4_verbosity;
pub mod d5_grounding;

use std::collections::BTreeMap;

use crate::events::{AttrMethod, Attrib, DetectorKind};
use crate::ir::NormalizedRequest;
use crate::policy::CompiledPolicy;

#[derive(Debug, Clone)]
pub struct Verdict {
    pub detector: DetectorKind,
    pub score: f64,
    pub would_act: bool,
    pub signals: BTreeMap<String, f64>,
}

impl Verdict {
    pub fn new(
        detector: DetectorKind,
        score: f64,
        would_act: bool,
        signals: impl IntoIterator<Item = (&'static str, f64)>,
    ) -> Self {
        Self {
            detector,
            score: score.clamp(0.0, 1.0),
            would_act,
            signals: signals
                .into_iter()
                .map(|(k, v)| (k.to_string(), v))
                .collect(),
        }
    }

    pub fn merge_signals(&mut self, other: &Verdict) {
        for (k, v) in &other.signals {
            self.signals.entry(k.clone()).or_insert(*v);
        }
    }
}

#[derive(Debug, Clone)]
pub struct OverFetchItem {
    pub tool: String,
    pub size_tokens: u32,
    pub cap_tokens: u32,
    pub referenced_frac: f64,
    pub excess_tokens: u32,
    /// Later turns in this request's trajectory that re-send the block (+1).
    pub resend_mult: u32,
}

#[derive(Debug, Clone, Default)]
pub struct SchemaFindings {
    pub schema_tokens: u32,
    pub reducible_tokens: u32,
    pub redundant_fields: u32,
}

#[derive(Debug, Clone)]
pub struct RequestAnalysis {
    pub verdicts: Vec<Verdict>,
    pub template_hash: String,
    pub attrib: Attrib,
    pub overfetch: Vec<OverFetchItem>,
    pub schema: SchemaFindings,
}

pub fn analyze_request(req: &NormalizedRequest, policy: &CompiledPolicy) -> RequestAnalysis {
    let template_hash = template_hash(req);
    let attrib = match req.feature_label.as_ref() {
        Some(label) => Attrib {
            method: AttrMethod::ExplicitHeader,
            feature_label: Some(label.clone()),
            intent_cluster_id: None,
            template_hash: Some(template_hash.clone()),
            label_source: Some("header".into()),
            confidence: 1.0,
        },
        None => Attrib {
            method: AttrMethod::TemplateHash,
            feature_label: None,
            intent_cluster_id: None,
            template_hash: Some(template_hash.clone()),
            label_source: None,
            confidence: 0.8,
        },
    };

    let mut verdicts = vec![d1_loop::analyze_request(req, policy), d2_rot::analyze(req)];
    let (overfetch_verdict, overfetch) = d3_overfetch::analyze(req, policy);
    verdicts.push(overfetch_verdict);
    let (schema_verdict, schema) = d4_verbosity::analyze_schema(&req.tools);
    verdicts.push(schema_verdict);

    RequestAnalysis {
        verdicts,
        template_hash,
        attrib,
        overfetch,
        schema,
    }
}

pub fn template_hash(req: &NormalizedRequest) -> String {
    let mut skeleton = String::new();
    for msg in &req.messages {
        if matches!(msg.role, crate::ir::Role::System) {
            skeleton.push_str(&crate::hash::template_skeleton(&msg.text));
            skeleton.push('\n');
            break;
        }
    }
    for msg in &req.messages {
        if matches!(msg.role, crate::ir::Role::User) {
            skeleton.push_str(&crate::hash::template_skeleton(&msg.text));
            break;
        }
    }
    crate::hash::sha256_16(skeleton)
}

/// Response-side detector accumulators; all counters, no retained payload.
pub struct StreamDetectors {
    seen: crate::window::SeenShingles,
    shingle_total: u64,
    shingle_repeats: u64,
    output_chars: u64,
    filler_chars: u64,
}

impl Default for StreamDetectors {
    fn default() -> Self {
        Self::new()
    }
}

impl StreamDetectors {
    pub fn new() -> Self {
        Self {
            seen: crate::window::SeenShingles::new(crate::window::DEFAULT_SHINGLE_CAP),
            shingle_total: 0,
            shingle_repeats: 0,
            output_chars: 0,
            filler_chars: 0,
        }
    }

    pub fn observe(&mut self, text: &str) {
        self.output_chars += text.chars().count() as u64;
        self.filler_chars += d4_verbosity::filler_chars(text) as u64;
        let shingles = crate::hash::shingle_hashes(text, 8);
        let (total, repeats) = self.seen.observe(&shingles);
        self.shingle_total += total;
        self.shingle_repeats += repeats;
    }

    pub fn output_tokens_est(&self) -> u32 {
        self.output_chars.div_ceil(4) as u32
    }

    pub fn finalize(&self, window_text: &str, request_text: &str) -> Vec<Verdict> {
        vec![
            d1_loop::finalize_stream(self.shingle_total, self.shingle_repeats, window_text),
            d4_verbosity::finalize_stream(self.filler_chars, self.output_chars, window_text),
            d5_grounding::analyze(window_text, request_text),
        ]
    }
}

/// Merge request-side and stream-side verdicts per detector (score = max, act = OR).
pub fn merge_verdicts(mut base: Vec<Verdict>, extra: Vec<Verdict>) -> Vec<Verdict> {
    for incoming in extra {
        match base.iter_mut().find(|v| v.detector == incoming.detector) {
            Some(existing) => {
                if incoming.score > existing.score {
                    existing.score = incoming.score;
                }
                existing.would_act |= incoming.would_act;
                existing.merge_signals(&incoming);
            }
            None => base.push(incoming),
        }
    }
    base
}

pub fn scores_map(verdicts: &[Verdict]) -> BTreeMap<String, f64> {
    verdicts
        .iter()
        .map(|v| (v.detector.as_str().to_string(), v.score))
        .collect()
}

/// Feature-level signal flatmap for the ledger event (numbers only).
pub fn feature_map(verdicts: &[Verdict]) -> BTreeMap<String, f64> {
    let mut out = BTreeMap::new();
    for v in verdicts {
        for (k, val) in &v.signals {
            out.insert(format!("{}.{}", v.detector.as_str(), k), *val);
        }
        out.insert(
            format!("{}.would_act", v.detector.as_str()),
            if v.would_act { 1.0 } else { 0.0 },
        );
    }
    out
}

pub fn detector_score(verdicts: &[Verdict], detector: DetectorKind) -> Option<&Verdict> {
    verdicts.iter().find(|v| v.detector == detector)
}

/// Keep the ledger event's verdict map on allowlisted numeric keys.
pub fn event_scores(verdicts: &[Verdict]) -> BTreeMap<String, f64> {
    let mut out = BTreeMap::new();
    for v in verdicts {
        out.insert(format!("d.{}", v.detector.as_str()), v.score);
    }
    out
}
