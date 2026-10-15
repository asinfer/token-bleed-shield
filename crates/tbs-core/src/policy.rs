//! Declarative policy compiler (docs/architecture/04 §2, 06 §3).
//!
//! YAML → validated, versioned in-memory snapshot. Invariants are rejected at
//! compile time; `kill_switch` forces every effective mode to observe.

use std::collections::BTreeMap;
use std::path::Path;

use serde::Deserialize;
use thiserror::Error;

use crate::events::{ActuatorKind, DetectorKind, Mode};
use crate::hash::sha256_hex;

#[derive(Debug, Error)]
pub enum PolicyError {
    #[error("policy file unreadable: {0}")]
    Io(#[from] std::io::Error),
    #[error("policy YAML invalid: {0}")]
    Yaml(#[from] serde_yaml::Error),
    #[error("apiVersion must be tbs.dev/v1")]
    BadApiVersion,
    #[error("kind must be Policy")]
    BadKind,
    #[error("rule `{0}`: enforce mode requires explicit rollout_percent")]
    EnforceNeedsRollout(String),
    #[error("rule `{0}`: rollout_percent must be 0-100")]
    RolloutRange(String),
    #[error("rule `{0}`: prompt_hygiene requires rollout_percent 0 in this phase")]
    HygieneMustBeOff(String),
    #[error("rule `{0}`: model_rewrite requires enumerated params.pairs")]
    RewriteNeedsPairs(String),
    #[error("duplicate rule id `{0}`")]
    DuplicateRule(String),
}

#[derive(Debug, Deserialize)]
pub struct PolicyFile {
    #[serde(rename = "apiVersion")]
    pub api_version: String,
    pub kind: String,
    pub metadata: PolicyMeta,
    pub spec: PolicySpec,
}

#[derive(Debug, Deserialize)]
pub struct PolicyMeta {
    pub project: String,
    pub revision: u32,
}

#[derive(Debug, Deserialize, Default)]
#[serde(default)]
pub struct PolicySpec {
    pub mode: Mode,
    pub kill_switch: bool,
    pub rules: Vec<RuleFile>,
    pub attribution: AttributionCfg,
    pub export: ExportCfg,
    pub intelligence_budget: BudgetCfg,
    pub pricing: Option<PricingCfg>,
}

#[derive(Debug, Deserialize, Clone)]
pub struct AttributionCfg {
    #[serde(default = "default_feature_header")]
    pub feature_header: String,
    #[serde(default = "default_true")]
    pub report_uncertain: bool,
}

impl Default for AttributionCfg {
    fn default() -> Self {
        Self {
            feature_header: default_feature_header(),
            report_uncertain: true,
        }
    }
}

fn default_feature_header() -> String {
    "x-tbs-feature".to_string()
}

fn default_true() -> bool {
    true
}

#[derive(Debug, Deserialize, Default, Clone)]
#[serde(default)]
pub struct ExportCfg {
    pub otlp_endpoint: Option<String>,
    pub billing_events: bool,
}

#[derive(Debug, Deserialize, Clone)]
pub struct BudgetCfg {
    #[serde(default = "default_max_ratio")]
    pub max_ratio: f64,
    #[serde(default = "default_l2_sampling")]
    pub l2_sampling: f64,
    #[serde(default = "default_monthly_cap")]
    pub absolute_monthly_cap_usd: f64,
}

fn default_max_ratio() -> f64 {
    0.03
}
fn default_l2_sampling() -> f64 {
    0.01
}
fn default_monthly_cap() -> f64 {
    25.0
}

impl Default for BudgetCfg {
    fn default() -> Self {
        Self {
            max_ratio: default_max_ratio(),
            l2_sampling: default_l2_sampling(),
            absolute_monthly_cap_usd: default_monthly_cap(),
        }
    }
}

#[derive(Debug, Deserialize, Clone)]
pub struct PricingCfg {
    pub version: Option<u16>,
    pub models: BTreeMap<String, crate::pricing::ModelPrice>,
}

#[derive(Debug, Deserialize, Clone)]
pub struct RuleFile {
    pub id: String,
    pub detector: DetectorKind,
    pub actuator: ActuatorKind,
    pub mode: Mode,
    pub rollout_percent: Option<u8>,
    #[serde(default)]
    pub params: serde_yaml::Value,
    #[serde(default)]
    pub thresholds: serde_yaml::Value,
}

#[derive(Debug, Clone)]
pub struct CompiledRule {
    pub id: String,
    pub detector: DetectorKind,
    pub actuator: ActuatorKind,
    pub declared_mode: Mode,
    pub effective_mode: Mode,
    pub rollout_percent: u8,
    pub params: serde_yaml::Value,
    pub thresholds: serde_yaml::Value,
}

#[derive(Debug, Clone)]
pub struct CompiledPolicy {
    pub project: String,
    pub revision: u32,
    pub floor: Mode,
    pub kill_switch: bool,
    pub rules: Vec<CompiledRule>,
    pub attribution: AttributionCfg,
    pub budget: BudgetCfg,
    pub export: ExportCfg,
}

impl Default for CompiledPolicy {
    fn default() -> Self {
        Self {
            project: "default".into(),
            revision: 0,
            floor: Mode::Observe,
            kill_switch: false,
            rules: Vec::new(),
            attribution: AttributionCfg::default(),
            budget: BudgetCfg::default(),
            export: ExportCfg::default(),
        }
    }
}

impl CompiledPolicy {
    pub fn from_yaml_str(yaml: &str) -> Result<Self, PolicyError> {
        let file: PolicyFile = serde_yaml::from_str(yaml)?;
        Self::compile(file)
    }

    pub fn load(path: &Path) -> Result<Self, PolicyError> {
        let text = std::fs::read_to_string(path)?;
        Self::from_yaml_str(&text)
    }

    fn compile(file: PolicyFile) -> Result<Self, PolicyError> {
        if file.api_version != "tbs.dev/v1" {
            return Err(PolicyError::BadApiVersion);
        }
        if file.kind != "Policy" {
            return Err(PolicyError::BadKind);
        }
        let mut seen = std::collections::HashSet::new();
        let mut rules = Vec::with_capacity(file.spec.rules.len());
        for rule in &file.spec.rules {
            if !seen.insert(rule.id.clone()) {
                return Err(PolicyError::DuplicateRule(rule.id.clone()));
            }
            if let Some(pct) = rule.rollout_percent
                && pct > 100
            {
                return Err(PolicyError::RolloutRange(rule.id.clone()));
            }
            let rollout = match rule.mode {
                Mode::Enforce => rule
                    .rollout_percent
                    .ok_or_else(|| PolicyError::EnforceNeedsRollout(rule.id.clone()))?,
                _ => rule.rollout_percent.unwrap_or(100),
            };
            if rule.actuator == ActuatorKind::PromptHygiene && rollout > 0 {
                return Err(PolicyError::HygieneMustBeOff(rule.id.clone()));
            }
            if rule.actuator == ActuatorKind::ModelRewrite {
                let has_pairs = rule
                    .params
                    .get("pairs")
                    .and_then(|p| p.as_sequence())
                    .is_some_and(|s| !s.is_empty());
                if !has_pairs {
                    return Err(PolicyError::RewriteNeedsPairs(rule.id.clone()));
                }
            }
            let effective = if file.spec.kill_switch {
                Mode::Observe
            } else {
                std::cmp::min(rule.mode, file.spec.mode)
            };
            rules.push(CompiledRule {
                id: rule.id.clone(),
                detector: rule.detector,
                actuator: rule.actuator,
                declared_mode: rule.mode,
                effective_mode: effective,
                rollout_percent: rollout,
                params: rule.params.clone(),
                thresholds: rule.thresholds.clone(),
            });
        }
        Ok(Self {
            project: file.metadata.project,
            revision: file.metadata.revision,
            floor: file.spec.mode,
            kill_switch: file.spec.kill_switch,
            rules,
            attribution: file.spec.attribution,
            budget: file.spec.intelligence_budget,
            export: file.spec.export,
        })
    }

    pub fn rule(&self, detector: DetectorKind) -> Option<&CompiledRule> {
        self.rules.iter().find(|r| r.detector == detector)
    }

    /// Default fallback rule id for a detector with no explicit rule.
    pub fn rule_id(&self, detector: DetectorKind) -> String {
        self.rule(detector)
            .map(|r| r.id.clone())
            .unwrap_or_else(|| format!("default-{}", detector.as_str()))
    }

    pub fn effective_mode(&self, detector: DetectorKind) -> Mode {
        self.rule(detector)
            .map(|r| r.effective_mode)
            .unwrap_or(Mode::Observe)
    }

    pub fn rollout_bucket(&self, trajectory_id: &str, rule_id: &str) -> u16 {
        let digest = sha256_hex(format!("{trajectory_id}:{rule_id}"));
        u16::from_str_radix(&digest[..4], 16).unwrap_or(0)
    }

    pub fn in_rollout(&self, trajectory_id: &str, rule: &CompiledRule) -> bool {
        if rule.rollout_percent >= 100 {
            return true;
        }
        if rule.rollout_percent == 0 {
            return false;
        }
        (self.rollout_bucket(trajectory_id, &rule.id) % 100) < u16::from(rule.rollout_percent)
    }
}

impl CompiledRule {
    pub fn param_u32(&self, key: &str) -> Option<u32> {
        self.params.get(key)?.as_u64().map(|n| n as u32)
    }

    pub fn threshold_u32(&self, key: &str, default: u32) -> u32 {
        self.thresholds
            .get(key)
            .and_then(serde_yaml::Value::as_u64)
            .map(|n| n as u32)
            .unwrap_or(default)
    }

    pub fn per_tool_cap(&self, tool: &str) -> Option<u32> {
        self.params
            .get("per_tool")?
            .get(tool)?
            .get("cap")?
            .as_u64()
            .map(|n| n as u32)
    }
}

pub fn default_observe() -> CompiledPolicy {
    CompiledPolicy::default()
}

#[cfg(test)]
mod tests {
    use super::*;

    const VALID: &str = r#"
apiVersion: tbs.dev/v1
kind: Policy
metadata: { project: prod-agents, revision: 41 }
spec:
  mode: observe
  kill_switch: false
  rules:
    - id: loop-breaker
      detector: loop_repetition
      actuator: circuit_breaker
      mode: observe
      rollout_percent: 100
      params: { max_identical_args: 1, min_window: 3 }
      thresholds: { gray_zone: [0.4, 0.7] }
    - id: clamp-tool-outputs
      detector: over_fetch
      actuator: tool_output_clamp
      mode: advise
      rollout_percent: 100
      params:
        default_cap_tokens: 2000
        per_tool:
          search: { cap: 800 }
  attribution: { feature_header: x-tbs-feature }
"#;

    #[test]
    fn compiles_valid_policy() {
        let p = CompiledPolicy::from_yaml_str(VALID).unwrap();
        assert_eq!(p.revision, 41);
        assert_eq!(
            p.rule(DetectorKind::LoopRepetition).unwrap().effective_mode,
            Mode::Observe
        );
        // Project floor = observe (Phase 0): declared advise clamps to observe.
        let clamp = p.rule(DetectorKind::OverFetch).unwrap();
        assert_eq!(clamp.declared_mode, Mode::Advise);
        assert_eq!(clamp.effective_mode, Mode::Observe);
        assert_eq!(clamp.per_tool_cap("search"), Some(800));
        assert_eq!(clamp.param_u32("default_cap_tokens"), Some(2000));
    }

    #[test]
    fn kill_switch_forces_observe() {
        let yaml = VALID
            .replace("kill_switch: false", "kill_switch: true")
            .replace(
                "mode: observe\n      rollout",
                "mode: enforce\n      rollout",
            );
        let p = CompiledPolicy::from_yaml_str(&yaml).unwrap();
        assert!(p.kill_switch);
        assert!(p.rules.iter().all(|r| r.effective_mode == Mode::Observe));
    }

    #[test]
    fn enforce_without_rollout_rejected() {
        let yaml = VALID
            .replace("mode: advise", "mode: enforce")
            .replace("rollout_percent: 100", "rollout_percent: 25");
        assert!(CompiledPolicy::from_yaml_str(&yaml).is_ok());
        let yaml2 = r#"
apiVersion: tbs.dev/v1
kind: Policy
metadata: { project: p, revision: 1 }
spec:
  mode: enforce
  rules:
    - id: r1
      detector: loop_repetition
      actuator: circuit_breaker
      mode: enforce
      params: {}
"#;
        assert!(matches!(
            CompiledPolicy::from_yaml_str(yaml2),
            Err(PolicyError::EnforceNeedsRollout(_))
        ));
    }

    #[test]
    fn hygiene_must_be_off_in_phase0() {
        let yaml = r#"
apiVersion: tbs.dev/v1
kind: Policy
metadata: { project: p, revision: 1 }
spec:
  mode: enforce
  rules:
    - id: hygiene
      detector: context_rot
      actuator: prompt_hygiene
      mode: enforce
      rollout_percent: 10
      params: { rewrite: true }
"#;
        assert!(matches!(
            CompiledPolicy::from_yaml_str(yaml),
            Err(PolicyError::HygieneMustBeOff(_))
        ));
    }

    #[test]
    fn rewrite_requires_pairs() {
        let yaml = r#"
apiVersion: tbs.dev/v1
kind: Policy
metadata: { project: p, revision: 1 }
spec:
  mode: observe
  rules:
    - id: route
      detector: routing_advisor
      actuator: model_rewrite
      mode: observe
      params: {}
"#;
        assert!(matches!(
            CompiledPolicy::from_yaml_str(yaml),
            Err(PolicyError::RewriteNeedsPairs(_))
        ));
    }

    #[test]
    fn rollout_lottery_is_deterministic_and_sticky() {
        let p = CompiledPolicy::from_yaml_str(VALID).unwrap();
        let rule = p.rule(DetectorKind::LoopRepetition).unwrap();
        let bucket_a = p.rollout_bucket("traj_1", &rule.id);
        let bucket_b = p.rollout_bucket("traj_1", &rule.id);
        assert_eq!(bucket_a, bucket_b);
        let mut pct_rule = rule.clone();
        pct_rule.rollout_percent = 50;
        let hits = (0..100u32)
            .filter(|i| p.in_rollout(&format!("traj_{i}"), &pct_rule))
            .count();
        assert!((30..70).contains(&hits), "hits = {hits}");
    }
}
