# 11 · Phased Roadmap

**Role of this doc**: sequencing with acceptance criteria, measurable NFRs, and — most importantly — the **riskiest assumption each phase exists to falsify**. Phases gate on evidence; a phase does not start until the prior phase's acceptance criteria pass on real traffic (design partners, then dogfooding, then beta).

---

## Phase 0 — Shadow-mode detection + savings ledger MVP

**Scope**: the data plane in observe mode + L0 detectors + L1 scoring + the counterfactual ledger. **No actuators mutate traffic.** Detectors: **loop/repetition (D1), verbose output (D4), over-fetch (D3)**; context-rot (D2) and hallucination (D5) ship *scores only*. Intent attribution: template-hash only (no clustering yet). One topology: SaaS, base-URL-swap mode only, OpenAI-compatible + Anthropic.

| Acceptance criterion | Gate |
|---|---|
| Passthrough overhead | **< 15ms p99** added latency, observe mode, all detectors on ([02 §3](02-data-plane.md)) |
| Streaming TTFB delta | **< 10ms p99** on SSE |
| Bounded memory | ≤ 512 KiB per connection under load-test (slow-reader + 100k-token streams) |
| Zero payload-at-rest | `--audit-egress` report shows **0 payload bytes** to any sink; type-guard trip counter = 0 ([09 §5](09-security-threat-model.md)) |
| Detection quality | D1 precision ≥ 0.90 on labeled replay set of 1k trajectories; D3 flags ≥ p95-size fetches with referenced_frac < 0.15 in ≥ 70% of known-overfetch cases |
| Ledger integrity | Replay of event stream reproduces identical totals (idempotency); measured vs projected classes never mixed in headline ([05 §3](05-savings-ledger.md)) |
| Receipt | first monthly receipt generated for ≥ 5 design partners, drill-down to `event_id` works |
| Scale smoke | 1 tenant @ 50M req/mo replay without event loss (spool handles 10× burst) |

**Riskiest assumption**: **A2a — "detected waste is measurable and attributable without acting."** If shadow-mode savings numbers don't survive design-partner CFO scrutiny (counterfactual methodology challenged, projected savings dominate, attribution too fuzzy), the whole savings-anchored model is in doubt — better to learn this while we've built no actuators. Kill/reshape criterion: if < 3/5 partners accept the methodology, revisit estimator classes before Phase 1.

---

## Phase 1 — Actuators: cache, clamping, breaker

**Scope**: enable enforce path for **semantic cache (1.1), tool-output clamping (1.2, mechanical only), loop circuit breaker (1.3)**; graduated rollout machinery + kill switches + full actuation audit. Prompt-hygiene still off. Routing advisor still observe-only.

| Acceptance criterion | Gate |
|---|---|
| Rollout safety | observe→advise→enforce ladder enforced in code; kill switch (rule/project/global) takes effect ≤ 5s p99 |
| Actuator FP control | breaker enforce: FP rate ≤ 0.5% of enforced trips over 10k enforced requests (auto-rollback wired); clamp: 0 data-loss incidents (defined as customer-reported missing-critical-line) at cap ≥ 1200 tok |
| Cache quality | shadow-sampled hit-vs-fresh agreement ≥ 99% (±1% output-token tolerance, [05 §3.1](05-savings-ledger.md)) |
| Savings shift | measured share of prevented USD rises to ≥ 70% of total (was breaker-heavy projected); billable savings per tenant up ≥ 3× vs Phase 0 projection |
| Failure-open | detector/CP/cache-outage chaos tests: traffic passthrough maintained, `detection_degraded` events fire ([02 §5](02-data-plane.md)) |
| Latency | enforce-mode stream actuation inject ≤ 1ms added on trip; no TTFB regression |

**Riskiest assumption**: **A2b — "prevention (actuation) converts detection into CFO-verifiable measured savings without unacceptable quality regressions."** If breaker FPs or cache-hit quality issues churn design partners, actuators must ship narrower (e.g. breaker enforce only for ≥2-signal trips, cache max-temperature lowered). This phase is where "prevention not observation" is proven or disproven.

---

## Phase 2 — Intent attribution + routing advisor

**Scope**: L1 intent clustering (D7) + console feature-ledger + **routing advisor enforce (D6)** with allowlisted downgrade pairs + holdout accounting; dynamic verbosity/hygiene *opt-in* rewrites (default still off, per [04 §1.5](04-actuation-layer.md)); hallucination-score actuation remains advise-only.

| Acceptance criterion | Gate |
|---|---|
| Attribution quality | ≥ 60% of spend maps to stable clusters (silhouette ≥ 0.5) or explicit labels; receipt shows `attr_method` + confidence on every line ([05 §4](05-savings-ledger.md)) |
| Feature-ledger adoption | ≥ 3 design partners use per-feature budget alerts as primary view (survey) |
| Routing safety | holdout (10%) shows no quality regression (retry/failure rate delta ≤ 0.5pp vs frontier holdout); allowlist-only rewrites, 100% of enforces logged |
| Routing savings | measured routing savings ≥ 20% of total measured savings for tenants with frontier traffic |
| Cluster economics | L2 cluster-naming batch cost ≤ 0.5% of tenant intelligence budget (it must not erode the 3% headroom for detection) |

**Riskiest assumption**: **"intent-level attribution + routing is the differentiator customers will pay a premium for"** (vs. Langfuse/LiteLLM doing pieces). Falsified if feature-ledger is unused after 60d or routing holdouts show no quality-safe savings. Mitigation: routing enforce ships behind a separate entitlement so attribution value is testable independently.

---

## Phase 3 — BYOC + compliance

**Scope**: Helm chart `tbs-byoc`, Docker Compose self-hosted, customer-KMS custody, customer-key L2 mode, SSO/SAML + SCIM, auditor role + audit export, EU-region pinning, pen-test remediation, signed images + SBOM, break-glass support access, EULA/DPA/sub-processor doc set finalized.

| Acceptance criterion | Gate |
|---|---|
| One-binary invariant | BYOC/self-host run the **same image digest** as SaaS; policy YAML compiles unchanged ([08 §2](08-deployment-topologies.md)) |
| Zero egress by default | `phone_home.enabled=false` default; compose/helm install makes **zero** outbound calls except provider + optional customer-configured sink ([08 §3](08-deployment-topologies.md)) |
| Customer-key L2 | payload never traverses our KMS/model accounts in `l2.key_source: customer_supplied`; verified by egress audit |
| Compliance artifacts | SOC2-aligned controls list mapped to [09](09-security-threat-model.md) mechanisms; DPA + zero-retention whitepaper + EULA shipped; external pen-test report with 0 critical/high open |
| Erasure SLA | hard-delete drill across CH/PG/Redis/obj-store + fingerprint-key destruction ≤ 7d demonstrated end-to-end |
| License enforcement | offline-verifiable signed license; no telemetry dependency |

**Riskiest assumption**: **"enterprise compliance posture is the moat and is winnable"** — i.e., that shipping provable zero-retention + BYOC converts the enterprise segment on the strength of security review alone. Falsified if enterprise deals stall on something we didn't build (e.g. they demand on-prem *control plane* air-gap, or regional sub-processors we can't stand up). Mitigation: enterprise design partners drive the Phase 3 backlog; `control.mode: embedded` already anticipates air-gap.

---

## Cross-phase NFRs (measurable, CI-enforced from Phase 0)

| NFR | Target | Enforcement |
|---|---|---|
| Passthrough p99 overhead | < 15ms | perf gate in CI (replay harness, HDR histogram) |
| Stream TTFB delta p99 | < 10ms | same |
| Per-connection memory ceiling | ≤ 512 KiB | load test + arena accounting assertion |
| Payload-at-rest | 0 bytes, always | `--audit-egress` in CI + type-guard trip counter |
| Intelligence budget | ≤ 3% measured savings, 30d rolling | runtime ladder + receipt + CI unit test of ladder math |
| Kill-switch propagation | ≤ 5s p99 | integration test |
| Ledger replay determinism | byte-identical totals | golden-file test |
| Event loss under 10× burst | 0 (spool absorbs) | soak test |
| One-binary / one-schema across topologies | identical digest + schema version | release pipeline check |
| Detector precision (D1 enforce) | ≥ 0.90 (replay), FP ≤ 0.5% (live) | labeled set + live auto-rollback |

---

## Sequencing rationale (one line each)

- **Phase 0 before actuators**: must *see* waste credibly before touching traffic; builds the ledger trust the pricing depends on.
- **Actuators before attribution**: prevention is the product; attribution without prevention is dashboards (an ANTI-GOAL, [01 §4](01-system-overview.md)).
- **Attribution before BYOC**: enterprise self-host is high-margin but high-cost to support; enter it with the full feature story, not a partial one.
- **Compliance last but not late**: it's the moat, but building it before the prevention thesis is proven inverts the risk order — and [09](09-security-threat-model.md) mechanisms (no-payload guard, egress audit) are in the binary from Phase 0, so nothing is retrofitted.

---

*Next: [12 — Self-Audit](12-self-audit.md).*
