# 12 · Self-Audit

**Role of this doc**: the assumptions register (every assumption flagged), open questions, a confidence rating per deliverable, and the checklist an external auditor should run. This document's job is to make the architecture **attackable on purpose** — the same discipline the product sells (measurable, auditable) applied to ourselves.

---

## 1. Assumptions register

| ID | Assumption | Flag | Impact if wrong | How/when falsified |
|---|---|---|---|---|
| A1 | Waste is 20–35% of production LLM spend | 🔴 high-leverage | Whole savings-anchored pitch weakens at low waste | Phase 0 shadow data across design partners ([10 §1](10-unit-economics.md)) |
| A2 | 40–60% of *detected* waste is *measurably* preventable | 🔴 high-leverage (the thesis) | Billable savings collapse to projected-only | Phase 1 actuator quality gates ([11](11-roadmap.md)) |
| A3 | Customers accept 10–12%-of-savings pricing | 🟡 pricing risk | Unit economics need rework (license-heavy) | Sales conversations, Phase 0/1 |
| A4 | CFO-auditable counterfactual methodology is *buyable* (not just intellectually honest) | 🔴 trust risk | Ledger becomes a demo, not a product | ≥3/5 design partners accept methodology (Phase 0 gate) |
| A5 | L0+L1 (local, free) is sufficient for D1/D3/D4 at useful precision | 🟡 tech risk | L2 costs escape the 3% cap, margin erodes | Phase 0 labeled-set precision gates ([11](11-roadmap.md)) |
| A6 | Semantic cache hits on agent traffic ≥ ~2–5% (temp ≤0.7 requests) | 🟡 feature value | Cache actuator underdelivers; savings lean harder on routing/clamp/breaker | Phase 1 shadow-cache hit-rate measurement |
| A7 | Rust data plane is viable despite hiring/iteration cost; Go remains fallback behind `NormalizedRequest` IR | 🟢 medium (isolated) | Schedule slip, not architecture failure (swappable) | Phase 0 build; staffing review |
| A8 | Base-URL-swap alone (no SDK change) captures most adoption; sidecar/SDK-wrapper are complements not requirements | 🟡 GTM | Enterprise needs sidecar earlier → Phase 3 scope grows | Adoption-mode telemetry in Phase 0 |
| A9 | Feature-level attribution (cluster quality ≥ 0.5 silhouette) is achievable without customer labels | 🟡 tech risk | Attribution needs explicit-label adoption push; receipts show more `uncertain` | Phase 2 cluster eval ([11](11-roadmap.md)) |
| A10 | Routing rewrites can hit 0.97 calibration with a 10% holdout guaranteeing no quality regression | 🔴 quality risk | Routing enforce stays observe-only; lose highest-value measured savings class | Phase 2 holdout gates |
| A11 | Enterprises will run our binary (BYOC) and accept a hybrid control plane (signed snapshot pull, no live CP) | 🟢 medium | Self-hosted-embedded path grows; sales cycle lengthens | Phase 3 design partners |
| A12 | ONNX Runtime / `tract` in-process keeps L1 under the 6ms pre-dispatch budget on commodity pods | 🟡 perf risk | L1 moves to sidecar (architecturally allowed, more moving parts) | Phase 0 perf gate ([02 §3](02-data-plane.md)) |
| A13 | Customers provide accurate price tables (or accept provider-list defaults) | 🟢 low | Savings math drifts; mitigated by versioned table + defaults | Phase 0 receipt review |
| A14 | "No payload persistence" is provable enough (type guard + egress audit) to satisfy enterprise security review | 🔴 trust risk | Compliance moat (Phase 3) collapses; DPA claims become soft | Pen-test + auditor dry-run (Phase 3, [09 §5](09-security-threat-model.md)) |
| A15 | Free-tier observe-only is enough top-of-funnel (no enforce trial needed) | 🟡 GTM | Funnel conversion drops; may need time-boxed enforce sandbox | Free-tier cohort analysis |
| A16 | One binary + config-driven topology is maintainable (no "enterprise fork" entropy) | 🟢 medium (disciplined) | Build matrix explodes; integrity invariant breaks | Release-pipeline digest check ([08 §2](08-deployment-topologies.md)) |

🔴 = load-bearing for the business model · 🟡 = load-bearing for a feature · 🟢 = operational.

---

## 2. Open questions (unresolved by design, tracked)

1. **Projected-savings ethics**: at what projected/measured ratio does a receipt become misleading despite clean labeling? (Proposed guard: if projected > measured, receipt headline leads with measured-only and adds an explicit "projection-dominant period" banner.)
2. **Cache persistence vs zero-retention**: ephemeral cache is the default ([04 §1.1](04-actuation-layer.md)); a persisted completion cache is a payload-class store. Do we ever ship persisted cache, or is that permanently out (ANTI-GOALS adjacent)? *Current stance: ephemeral only; revisit only with per-tenant encrypted, opt-in, disclosed design.*
3. **L2 prompt-injection surface in SaaS mode**: customer payload → our model account. *Current stance: BYOC customer-key is the enterprise answer; SaaS L2 stays redacted+sampled+budgeted with account isolation. Open: is that enough for a hostile-payload tenant?*
4. **Trajectory ID reliability**: loop/context-rot quality depends on apps propagating a session/conversation id. *Open: how much do we invest in client-side trajectory stitching (header conventions, SDK helper) vs. accept degraded detection for header-less traffic?*
5. **Regional data residency beyond EU** (UK, AU, JP): required for which Phase 3 deals? Cost of multi-region CH/KMS pinning.
6. **Who owns the `adjustment` dispute SLA** when methodology revision ≠ billing period pin? (Need a published dispute-window policy.)
7. **Cross-tenant model improvement**: can L1 retrain on *aggregate, non-tenant-crossing* telemetry without tripping the "no training on customer data" ANTI-GOAL? *Current stance: self-supervised calibration only, per-tenant or fully anonymized; no content-derived training sets.*

---

## 3. Confidence rating per deliverable

| Deliverable (doc) | Confidence | Why |
|---|---|---|
| 01 System overview | **high** | Component boundaries and anti-goals are stable decisions, not tech bets |
| 02 Data plane | **medium-high** | Rust/`NormalizedRequest` IR derisks the big bet; streaming mechanism is well-understood proxy engineering; L1-in-process 6ms budget is the open perf item (A12) |
| 03 Detection engine | **medium** | D1/D3/D4 L0 approaches are proven patterns; D5 (hallucination) and D9 (context-rot) scoring precision is genuinely uncertain pre-data |
| 04 Actuation layer | **medium-high** | Rollout/kill-switch semantics are standard; cache-hit quality on agent traffic is the empirical unknown (A6) |
| 05 Savings ledger | **medium-high** | Formula is auditable by construction; *buyability* of the methodology (A4) is the real risk, not the math |
| 06 Control plane | **high** | Boring, well-trodden multi-tenant SaaS design |
| 07 Data architecture | **high** | ClickHouse-for-events is settled practice at this scale; attribution columns are additive |
| 08 Deployment topologies | **high** | One-binary/config-driven is a discipline, not a research problem (A16) |
| 09 Security & threat model | **medium-high** | Mechanisms (type guard, egress audit) are concrete; enterprise-acceptance (A14) unproven until pen-test + first security review |
| 10 Unit economics | **medium** | Shape of the math is solid; every input is an assumption with wide error bars (A1–A3) — treat as model, not forecast |
| 11 Roadmap | **high** | Sequencing is deliberately falsification-ordered; gates are measurable |

---

## 4. External auditor checklist

An auditor (security, financial-methodology, or architecture review) should be able to run this in a day and get yes/no answers.

### 4.1 Zero-retention / data-plane provability ([09 §5](09-security-threat-model.md))

- [ ] Read the `EventSink`/`TraceSink` type definitions; attempt to add a payload-bearing field; confirm the build/lint **fails**.
- [ ] Run `tbs-proxy --audit-egress` on a representative workload (incl. SSE streams, cache hit, clamp, breaker trip); confirm egress is only provider + sink + KMS with payload-free sink sizes.
- [ ] Confirm the only payload egress path is the opt-in trace path and that it redacts **before** persistence (enable in a test tenant, inspect stored trace).
- [ ] Verify Redis payload-class keys have TTL and **no AOF/RDB persistence** configured.
- [ ] Inspect the no-payload-guard trip counter (expect 0) and the `secrets_redaction_failure` alert wiring.

### 4.2 Secrets handling ([09 §2](09-security-threat-model.md))

- [ ] Confirm keys are envelope-encrypted (KMS KEK), zeroized on drop, absent from logs/argv/core dumps.
- [ ] In BYOC: confirm L2 `key_source: customer_supplied` never routes through vendor KMS (egress audit).
- [ ] Rotate a tenant key; confirm re-wrap doesn't touch data and old fingerprint HMAC key destruction renders historical fingerprints unlinkable.

### 4.3 Savings methodology ([05](05-savings-ledger.md))

- [ ] Pick 10 `prevented` events across `method` values; recompute `prevented_usd` by hand from `actual` + `counterfactual` + price table version.
- [ ] Confirm `measured` vs `projected` never mix in the headline; confirm projected is excluded from the billable figure by default.
- [ ] Trace one receipt line item down to its constituent `event_id`s; confirm replay reproduces totals (idempotency).
- [ ] Confirm anti-inflation rules ([05 §3.3](05-savings-ledger.md)): cf baseline is the *would-actually-have-used* model, no double-count (clamp vs cache), horizon cap on breaker projections.
- [ ] Run a dispute: file an `adjustment`; confirm history is append-only and the receipt re-renders with both entries.

### 4.4 Governance / kill switches ([04](04-actuation-layer.md), [06](06-control-plane.md))

- [ ] Engage rule/project/global kill switch; measure propagation to proxy ≤ 5s; confirm forced observe.
- [ ] Confirm enforce-mode changes require admin + approval and append audit entries with actor/prior/new value.
- [ ] Verify rollout lottery is sticky per trajectory and honors `rollout_percent` within statistical tolerance.
- [ ] Confirm entitlements reject out-of-tier rules at compile **and** snapshot-load.

### 4.5 Multi-tenancy isolation ([06 §1](06-control-plane.md))

- [ ] Attempt cross-tenant query without tenant context (RLS should deny).
- [ ] Feed org-A policy snapshot to org-B proxy; confirm signature/tenant check rejects.
- [ ] Confirm no shared in-memory state (trajectory windows, caches) across tenants in the data plane.

### 4.6 Topology integrity ([08](08-deployment-topologies.md))

- [ ] Compare BYOC/self-host image digest to SaaS digest (must match).
- [ ] On a fresh BYOC install with defaults: confirm **zero** outbound egress except provider (+ configured sink); `phone_home.enabled` defaults false.
- [ ] Compile a SaaS-authored policy YAML under self-hosted; must pass unchanged.

### 4.7 Interop ([07](07-data-architecture.md))

- [ ] Point OTLP export at a customer-controlled collector; confirm metrics/logs arrive with metadata only (no payload, fingerprints off by default).

---

## 5. Document control

| Rev | Date | Change |
|---|---|---|
| v0.1 | 2026-10-15 | Initial 12-deliverable architecture; language = Rust data plane / Go control plane; savings = measured-billable + projected-disclosed; 3% intelligence cap with degradation ladder |

*Back to [index](README.md). Cross-reference: [../ARCHITECTURE.md](../ARCHITECTURE.md) (CLI tool internals — the Phase-0 seed), [../COMPETETIVE_ANALYSIS.md](../COMPETETIVE_ANALYSIS.md), [../PREVENTIVE_MANIFESTO.md](../PREVENTIVE_MANIFESTO.md).*
