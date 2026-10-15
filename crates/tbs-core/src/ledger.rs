//! Counterfactual savings ledger (docs/architecture/05): estimator classes kept
//! strictly separate, content-addressed events, replay-deterministic totals,
//! and the monthly receipt.

use std::collections::{BTreeMap, HashSet};
use std::io::{BufRead, BufReader};
use std::path::Path;

use crate::detectors::{OverFetchItem, Verdict, detector_score, event_scores, feature_map};
use crate::events::{
    ActionTaken, ActualSide, Attrib, Counterfactual, DetectorKind, Estimator, EventKind, Evidence,
    Method, RuleRef, UsageEvent, UsageSource,
};
use crate::policy::CompiledPolicy;
use crate::pricing::PriceTable;

/// Phase 0 wires no actuators; every event reports observe-mode reality.
pub const CAN_ACTUATE: bool = false;
pub const LOOP_HORIZON_MULT: f64 = 3.0;
pub const MIN_SCHEMA_SAVINGS_TOKENS: u32 = 8;

#[derive(Debug, Clone)]
pub struct RequestOutcome {
    pub ts: String,
    pub tenant_id: String,
    pub project_id: String,
    pub trajectory_id: String,
    pub request_id: String,
    pub model: String,
    pub input_tokens: u32,
    pub output_tokens: u32,
    pub cache_read_tokens: u32,
    pub usage_source: UsageSource,
    pub cost_usd: f64,
    pub price_known: bool,
    pub price_table_v: u16,
    pub latency_ms: u32,
    pub attrib: Attrib,
    pub verdicts: Vec<Verdict>,
    pub overfetch: Vec<OverFetchItem>,
    pub schema_reducible_tokens: u32,
    pub degraded: bool,
}

impl Default for RequestOutcome {
    fn default() -> Self {
        Self {
            ts: String::new(),
            tenant_id: String::new(),
            project_id: String::new(),
            trajectory_id: String::new(),
            request_id: String::new(),
            model: String::new(),
            input_tokens: 0,
            output_tokens: 0,
            cache_read_tokens: 0,
            usage_source: UsageSource::Estimated,
            cost_usd: 0.0,
            price_known: true,
            price_table_v: 1,
            latency_ms: 0,
            attrib: Attrib {
                method: crate::events::AttrMethod::None,
                feature_label: None,
                intent_cluster_id: None,
                template_hash: None,
                label_source: None,
                confidence: 0.0,
            },
            verdicts: Vec::new(),
            overfetch: Vec::new(),
            schema_reducible_tokens: 0,
            degraded: false,
        }
    }
}

pub fn build_events(
    outcome: &RequestOutcome,
    policy: &CompiledPolicy,
    prices: &PriceTable,
) -> Vec<UsageEvent> {
    let mut features = feature_map(&outcome.verdicts);
    if outcome.degraded {
        features.insert("detection_degraded".into(), 1.0);
    }
    if !outcome.price_known {
        features.insert("price_unknown".into(), 1.0);
    }

    let mut events = vec![UsageEvent {
        event_id: String::new(),
        ts: outcome.ts.clone(),
        tenant_id: outcome.tenant_id.clone(),
        project_id: outcome.project_id.clone(),
        trajectory_id: outcome.trajectory_id.clone(),
        request_id: outcome.request_id.clone(),
        kind: EventKind::Actual,
        actual: Some(ActualSide {
            model: outcome.model.clone(),
            input_tokens: outcome.input_tokens,
            output_tokens: outcome.output_tokens,
            cache_read_tokens: outcome.cache_read_tokens,
            cost_usd: outcome.cost_usd,
            price_table_v: outcome.price_table_v,
            usage_source: outcome.usage_source,
        }),
        counterfactual: None,
        rule: None,
        prevented_usd: 0.0,
        confidence: 1.0,
        attrib: outcome.attrib.clone(),
        latency_ms: outcome.latency_ms,
        rollout_bucket: policy.rollout_bucket(&outcome.trajectory_id, "actual"),
        action_taken: ActionTaken::None,
        would_act: false,
        verdict_scores: event_scores(&outcome.verdicts),
        trajectory_features: features,
    }];

    let input_price = prices.input_price_per_token(&outcome.model);
    let cf_model = outcome.model.clone();

    // D4 static schema minify — countable byte diff → measured (05 §3.1).
    // Observe mode: actual = provider-reported (bloated), cf = minified dispatch;
    // prevented_usd is the countable delta, never a modeled horizon.
    if outcome.schema_reducible_tokens >= MIN_SCHEMA_SAVINGS_TOKENS && input_price > 0.0 {
        let saved = f64::from(outcome.schema_reducible_tokens) * input_price;
        events.push(prevented_event(
            outcome,
            policy,
            DetectorKind::Verbosity,
            Some(ActualSide {
                model: outcome.model.clone(),
                input_tokens: outcome.input_tokens,
                output_tokens: outcome.output_tokens,
                cache_read_tokens: outcome.cache_read_tokens,
                cost_usd: outcome.cost_usd,
                price_table_v: outcome.price_table_v,
                usage_source: outcome.usage_source,
            }),
            Counterfactual {
                model: cf_model.clone(),
                input_tokens: outcome
                    .input_tokens
                    .saturating_sub(outcome.schema_reducible_tokens),
                output_tokens: outcome.output_tokens,
                cost_usd: (outcome.cost_usd - saved).max(0.0),
                estimator: Estimator::Measured,
                method: Method::VerbositySchemaMinify,
                confidence: 0.95,
                evidence: Evidence {
                    basis: "schema_byte_diff".into(),
                    parent_event_id: None,
                },
            },
            saved,
        ));
    }

    // D1 loop trip — future burn beyond the trip → projected (05 §3.1).
    if let Some(v) = detector_score(&outcome.verdicts, DetectorKind::LoopRepetition)
        && v.would_act
        && outcome.cost_usd > 0.0
    {
        let projected = LOOP_HORIZON_MULT * outcome.cost_usd;
        events.push(prevented_event(
            outcome,
            policy,
            DetectorKind::LoopRepetition,
            None,
            Counterfactual {
                model: cf_model.clone(),
                input_tokens: outcome.input_tokens,
                output_tokens: outcome.output_tokens,
                cost_usd: projected,
                estimator: Estimator::Projected,
                method: Method::CircuitBreaker,
                confidence: 0.5,
                evidence: Evidence {
                    basis: "horizon_3x_current_turn_burn".into(),
                    parent_event_id: None,
                },
            },
            projected,
        ));
    }

    // D3 over-fetch flag — hypothetical clamp under cap → projected (05 §3.1).
    for item in &outcome.overfetch {
        if input_price <= 0.0 {
            break;
        }
        let avoided_tokens = f64::from(item.excess_tokens) * f64::from(item.resend_mult);
        let projected = avoided_tokens * input_price;
        events.push(prevented_event(
            outcome,
            policy,
            DetectorKind::OverFetch,
            None,
            Counterfactual {
                model: cf_model.clone(),
                input_tokens: outcome
                    .input_tokens
                    .saturating_sub((avoided_tokens as u32).min(outcome.input_tokens)),
                output_tokens: outcome.output_tokens,
                cost_usd: projected,
                estimator: Estimator::Projected,
                method: Method::OverFetchFlag,
                confidence: 0.6,
                evidence: Evidence {
                    basis: "hypothetical_clamp_at_cap".into(),
                    parent_event_id: None,
                },
            },
            projected,
        ));
    }

    for ev in &mut events {
        ev.assign_id();
    }
    events
}

fn rule_ref(policy: &CompiledPolicy, detector: DetectorKind) -> RuleRef {
    let actuator = policy
        .rule(detector)
        .map(|r| r.actuator)
        .unwrap_or(match detector {
            DetectorKind::LoopRepetition => crate::events::ActuatorKind::CircuitBreaker,
            DetectorKind::OverFetch => crate::events::ActuatorKind::ToolOutputClamp,
            _ => crate::events::ActuatorKind::None,
        });
    RuleRef {
        id: policy.rule_id(detector),
        rev: policy.revision,
        mode: policy.effective_mode(detector),
        actuator,
    }
}

fn prevented_event(
    outcome: &RequestOutcome,
    policy: &CompiledPolicy,
    detector: DetectorKind,
    actual: Option<ActualSide>,
    cf: Counterfactual,
    prevented_usd: f64,
) -> UsageEvent {
    let rule = rule_ref(policy, detector);
    let confidence = cf.confidence;
    let score = detector_score(&outcome.verdicts, detector)
        .map(|v| v.score)
        .unwrap_or(1.0);
    let bucket = policy.rollout_bucket(&outcome.trajectory_id, &rule.id);
    let in_rollout = policy
        .rule(detector)
        .map(|r| policy.in_rollout(&outcome.trajectory_id, r))
        .unwrap_or(true);
    UsageEvent {
        event_id: String::new(),
        ts: outcome.ts.clone(),
        tenant_id: outcome.tenant_id.clone(),
        project_id: outcome.project_id.clone(),
        trajectory_id: outcome.trajectory_id.clone(),
        request_id: outcome.request_id.clone(),
        kind: EventKind::Prevented,
        actual,
        counterfactual: Some(cf),
        rule: Some(rule),
        prevented_usd,
        confidence,
        attrib: outcome.attrib.clone(),
        latency_ms: outcome.latency_ms,
        rollout_bucket: bucket,
        action_taken: if CAN_ACTUATE {
            ActionTaken::Enforced
        } else {
            ActionTaken::ObserveOnly
        },
        would_act: in_rollout,
        verdict_scores: BTreeMap::from([(format!("d.{}", detector.as_str()), score)]),
        trajectory_features: feature_map(&outcome.verdicts),
    }
}

#[derive(Debug, Default, Clone, PartialEq)]
pub struct FeatureTotals {
    pub measured: f64,
    pub projected: f64,
    pub actual_spend: f64,
    pub requests: u64,
    pub dominant_method: String,
}

#[derive(Debug, Default, Clone, PartialEq)]
pub struct LedgerTotals {
    pub requests: u64,
    pub actual_spend_usd: f64,
    pub measured_savings_usd: f64,
    pub projected_savings_usd: f64,
    pub intelligence_spend_usd: f64,
    pub by_feature: BTreeMap<String, FeatureTotals>,
    pub duplicate_events: u64,
    pub unparseable_lines: u64,
}

impl LedgerTotals {
    /// `potential = measured + projected`; never mixed in the headline.
    pub fn potential_savings_usd(&self) -> f64 {
        self.measured_savings_usd + self.projected_savings_usd
    }

    pub fn intelligence_ratio(&self) -> f64 {
        if self.measured_savings_usd <= 0.0 {
            0.0
        } else {
            self.intelligence_spend_usd / self.measured_savings_usd
        }
    }
}

fn feature_key(ev: &UsageEvent) -> String {
    ev.attrib
        .feature_label
        .clone()
        .or_else(|| ev.attrib.template_hash.clone())
        .unwrap_or_else(|| "unattributed".into())
}

/// Replay an event stream into totals: idempotent by `event_id`, deterministic,
/// estimator classes kept in separate buckets.
pub fn replay<'a>(events: impl IntoIterator<Item = &'a UsageEvent>) -> LedgerTotals {
    let mut totals = LedgerTotals::default();
    let mut seen: HashSet<String> = HashSet::new();
    let mut method_spend: BTreeMap<String, BTreeMap<String, f64>> = BTreeMap::new();

    for ev in events {
        if !seen.insert(ev.event_id.clone()) {
            totals.duplicate_events += 1;
            continue;
        }
        let key = feature_key(ev);
        let feature = totals.by_feature.entry(key.clone()).or_default();
        match ev.kind {
            EventKind::Actual => {
                let cost = ev.actual.as_ref().map(|a| a.cost_usd).unwrap_or(0.0);
                totals.actual_spend_usd += cost;
                feature.actual_spend += cost;
                feature.requests += 1;
                totals.requests += 1;
            }
            EventKind::Prevented => {
                let cf = ev.counterfactual.as_ref();
                let method = cf.map(|c| c.method.as_str()).unwrap_or("unknown");
                match cf.map(|c| c.estimator) {
                    Some(Estimator::Measured) => {
                        totals.measured_savings_usd += ev.prevented_usd;
                        feature.measured += ev.prevented_usd;
                    }
                    Some(Estimator::Projected) | None => {
                        totals.projected_savings_usd += ev.prevented_usd;
                        feature.projected += ev.prevented_usd;
                    }
                }
                *method_spend
                    .entry(key)
                    .or_default()
                    .entry(method.to_string())
                    .or_default() += ev.prevented_usd;
            }
            EventKind::IntelligenceSpend => {
                let cost = ev.actual.as_ref().map(|a| a.cost_usd).unwrap_or(0.0);
                totals.intelligence_spend_usd += cost;
            }
            EventKind::Adjustment => {}
        }
    }

    for (key, methods) in method_spend {
        if let Some((method, _)) = methods.into_iter().max_by(|a, b| a.1.total_cmp(&b.1))
            && let Some(feature) = totals.by_feature.get_mut(&key)
        {
            feature.dominant_method = method;
        }
    }
    totals
}

pub fn replay_jsonl(path: &Path) -> std::io::Result<LedgerTotals> {
    let file = std::fs::File::open(path)?;
    let reader = BufReader::new(file);
    let mut events = Vec::new();
    let mut unparseable = 0u64;
    for line in reader.lines() {
        let line = line?;
        if line.trim().is_empty() {
            continue;
        }
        match serde_json::from_str::<UsageEvent>(&line) {
            Ok(ev) => events.push(ev),
            Err(_) => unparseable += 1,
        }
    }
    let mut totals = replay(events.iter());
    totals.unparseable_lines = unparseable;
    Ok(totals)
}

pub fn render_receipt(
    tenant: &str,
    project: &str,
    period: &str,
    price_table_v: u16,
    policy_rev: u32,
    fee_rate: f64,
    totals: &LedgerTotals,
) -> String {
    let spend_without = totals.actual_spend_usd + totals.potential_savings_usd();
    let fee = fee_rate * totals.measured_savings_usd;
    let net = totals.measured_savings_usd - fee;
    let ratio_pct = totals.intelligence_ratio() * 100.0;

    let mut out = String::new();
    out.push_str("# Token Bleed Shield — Savings Receipt\n\n");
    out.push_str(&format!(
        "Tenant: {tenant} · Project: {project} · Period: {period} · Price table v{price_table_v} · Policy rev {policy_rev}\n\n"
    ));
    out.push_str("## Headline\n");
    out.push_str(&format!(
        "  Measured savings (billable) .......... ${:.2}\n",
        totals.measured_savings_usd
    ));
    out.push_str(&format!(
        "  Projected savings (disclosed) ........ ${:.2}   ← not invoiced\n",
        totals.projected_savings_usd
    ));
    out.push_str(&format!(
        "  Your spend without TBS ............... ${:.2}\n",
        spend_without
    ));
    out.push_str(&format!(
        "  Your actual spend .................... ${:.2}\n",
        totals.actual_spend_usd
    ));
    out.push_str(&format!(
        "  Intelligence spend (our cost) ........ ${:.2}  ({ratio_pct:.2}% of measured savings, cap 3%)\n",
        totals.intelligence_spend_usd
    ));
    out.push_str(&format!(
        "  TBS fee (contract rate {:.0}%) ....... ${:.2}\n",
        fee_rate * 100.0,
        fee
    ));
    out.push_str(&format!(
        "  Net position ........................ +${:.2}\n",
        net
    ));
    out.push_str(&format!(
        "  Requests analyzed .................... {}\n",
        totals.requests
    ));

    out.push_str("\n## Top prevented waste by feature\n");
    out.push_str("| Feature | Measured | Projected | Dominant rule |\n");
    out.push_str("|---|---:|---:|---|\n");
    let mut rows: Vec<(&String, &FeatureTotals)> = totals.by_feature.iter().collect();
    rows.sort_by(|a, b| (b.1.measured + b.1.projected).total_cmp(&(a.1.measured + a.1.projected)));
    for (feature, f) in rows.iter().take(10) {
        out.push_str(&format!(
            "| {} | ${:.2} | ${:.2} | {} |\n",
            feature, f.measured, f.projected, f.dominant_method
        ));
    }
    if rows.is_empty() {
        out.push_str("| — | $0.00 | $0.00 | — |\n");
    }

    out.push_str("\n## Methodology & confidence\n");
    out.push_str("- Counterfactual branches: verbosity_schema_minify (measured, byte diff),\n");
    out.push_str("  circuit_breaker (projected, horizon = 3× current-turn burn),\n");
    out.push_str("  over_fetch_flag (projected, hypothetical clamp at cap × resend multiplier).\n");
    out.push_str("- Measured and projected savings are never mixed in headline numbers;\n");
    out.push_str("  only measured savings are billable by default.\n");
    out.push_str("- Dispute: every line item links to a content-addressed `event_id`;\n");
    out.push_str("  replaying the event stream reproduces identical totals.\n");
    out.push_str("- Full methodology: docs/architecture/05-savings-ledger.md (pinned rev).\n");
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::detectors::Verdict;
    use crate::events::AttrMethod;

    fn outcome(cost: f64) -> RequestOutcome {
        RequestOutcome {
            ts: "2026-10-15T12:00:00Z".into(),
            tenant_id: "org_123".into(),
            project_id: "proj_prod_agents".into(),
            trajectory_id: "traj_abc".into(),
            request_id: "req_xyz".into(),
            model: "gpt-4o-mini".into(),
            input_tokens: 1800,
            output_tokens: 340,
            usage_source: UsageSource::Reported,
            cost_usd: cost,
            price_table_v: 7,
            latency_ms: 12,
            attrib: Attrib {
                method: AttrMethod::TemplateHash,
                feature_label: Some("document-qa".into()),
                intent_cluster_id: None,
                template_hash: Some("0123456789abcdef".into()),
                label_source: None,
                confidence: 0.8,
            },
            verdicts: vec![Verdict::new(
                DetectorKind::LoopRepetition,
                0.7,
                true,
                [("repetition_rate", 0.7)],
            )],
            overfetch: vec![OverFetchItem {
                tool: "search".into(),
                size_tokens: 5000,
                cap_tokens: 800,
                referenced_frac: 0.04,
                excess_tokens: 4200,
                resend_mult: 4,
            }],
            schema_reducible_tokens: 64,
            ..Default::default()
        }
    }

    #[test]
    fn builds_measured_and_projected_classes() {
        let prices = PriceTable::provider_defaults();
        let policy = CompiledPolicy::default();
        let events = build_events(&outcome(0.00148), &policy, &prices);
        assert_eq!(events.len(), 4, "actual + schema + loop + overfetch");
        let kinds: Vec<EventKind> = events.iter().map(|e| e.kind).collect();
        assert_eq!(kinds[0], EventKind::Actual);
        let measured: Vec<&UsageEvent> = events
            .iter()
            .filter(|e| {
                e.counterfactual
                    .as_ref()
                    .is_some_and(|c| c.estimator == Estimator::Measured)
            })
            .collect();
        let projected: Vec<&UsageEvent> = events
            .iter()
            .filter(|e| {
                e.counterfactual
                    .as_ref()
                    .is_some_and(|c| c.estimator == Estimator::Projected)
            })
            .collect();
        assert_eq!(measured.len(), 1);
        assert_eq!(
            measured[0].counterfactual.as_ref().unwrap().method,
            Method::VerbositySchemaMinify
        );
        assert_eq!(projected.len(), 2);
        for ev in &events {
            assert!(crate::events::validate(ev).is_ok());
        }
    }

    #[test]
    fn replay_is_deterministic_and_idempotent() {
        let prices = PriceTable::provider_defaults();
        let policy = CompiledPolicy::default();
        let mut events = build_events(&outcome(0.00148), &policy, &prices);
        let mut dup = events[1].clone();
        dup.assign_id();
        events.push(dup);

        let a = replay(events.iter());
        let b = replay(events.iter());
        assert_eq!(a, b);
        assert_eq!(a.requests, 1);
        assert_eq!(a.duplicate_events, 1);
        assert!(a.measured_savings_usd > 0.0);
        assert!(a.projected_savings_usd > 0.0);
        // 64 reducible schema tokens at $0.15/M input.
        let expected = 64.0 * 0.15 / 1_000_000.0;
        assert!(
            (a.measured_savings_usd - expected).abs() < 1e-12,
            "measured = {}",
            a.measured_savings_usd
        );
    }

    #[test]
    fn event_ids_stable_across_builds() {
        let prices = PriceTable::provider_defaults();
        let policy = CompiledPolicy::default();
        let a = build_events(&outcome(0.00148), &policy, &prices);
        let b = build_events(&outcome(0.00148), &policy, &prices);
        let ids_a: Vec<_> = a.iter().map(|e| e.event_id.clone()).collect();
        let ids_b: Vec<_> = b.iter().map(|e| e.event_id.clone()).collect();
        assert_eq!(ids_a, ids_b);
    }

    #[test]
    fn receipt_keeps_classes_separate() {
        let prices = PriceTable::provider_defaults();
        let policy = CompiledPolicy::default();
        let events = build_events(&outcome(0.00148), &policy, &prices);
        let totals = replay(events.iter());
        let receipt = render_receipt("Acme", "prod", "2026-10", 7, 41, 0.12, &totals);
        assert!(receipt.contains("Measured savings (billable)"));
        assert!(receipt.contains("Projected savings (disclosed)"));
        assert!(receipt.contains("not invoiced"));
        assert!(receipt.contains("document-qa"));
    }
}
