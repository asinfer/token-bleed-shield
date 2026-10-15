# 10 · Product Unit Economics

**Role of this doc**: our infrastructure + intelligence spend vs. projected customer savings at three customer sizes, with the **≤3% intelligence-budget math shown explicitly**.

> **Framing note**: every number below is an **illustrative model driven by the labeled assumptions** (A1–A8), not a market statistic. Inputs are deliberately conservative and customer-verifiable; the point is the *shape* of the math and the 3% mechanism, not a revenue forecast.

---

## 1. Assumptions register (inputs)

| ID | Assumption | Value used | Sensitivity |
|---|---|---|---|
| A1 | Waste share of LLM spend (un-prevented, before TBS) | 20–35% of gross LLM bill | low — bounded by waste classes in [COMPETETIVE_ANALYSIS](../COMPETETIVE_ANALYSIS.md) |
| A2 | Fraction of waste *measurably* prevented (Phase 1 actuators live) | 40–60% of detected waste | **high** — roadmap Phase 0/1 gates target this |
| A3 | Contract rate on measured savings | 12% (startup), 10% (enterprise, volume) | medium — pricing decision |
| A4 | L2 frontier sampling rate | 1% of requests (budget-gated) | low — auto-degrades ([03 §9](03-detection-engine.md)) |
| A5 | L2 cost per sampled call | ~$0.001–0.003 (small prompts, short completions) | low |
| A6 | Marginal infra cost per 1M requests (proxy compute + egress, SaaS) | ~$8–15 (Rust binary, small pods, high RPS density) | medium — cloud-dependent |
| A7 | Control-plane + event store amortized per tenant/mo | ~$3–20 (PG+CH+Redis share, scales sub-linearly) | low |
| A8 | L1 CPU-ms amortization (ONNX + embeddings) | included in A6 pod cost (same binary) | low |

**ROI framing for the customer** (what a founder/finance lead checks): `ROI = (measured_savings − TBS_fee − residual_infr) / (TBS_fee + residual_infr)`, where `residual_infr` = any customer-side cost (usually ~0 in SaaS mode beyond the SDK/env change). Because TBS_fee = A3 × measured_savings < measured_savings, **ROI > 0 whenever measured_savings > 0 and TBS_fee is the only charge** — that's the "invoice already covered by savings" pitch ([COMPETETIVE_ANALYSIS](../COMPETETIVE_ANALYSIS.md) §Why Customers Pay).

---

## 2. The 3% intelligence-budget mechanism (explicit)

**Rule** (enforced per-tenant, 30-day rolling): 

```
intelligence_spend  ≤  0.03 × measured_savings
```

`intelligence_spend` = L2 token cost (A5×sampling) + amortized L1 CPU. L0 is free. The ladder in [03 §9](03-detection-engine.md) trims L2→L1→L0 as the ratio rises; the ratio is itself a **product feature** (shown on the receipt as `intelligence_budget_utilization`, §5 below).

**Unit proof** — when does L2 alone stay under 3%?

Let `p` = price per 1M *output* tokens of the frontier model in play (~$10/M for a GPT-4o-class model, customer's own table). A sampled L2 call burns ~1,000 output tokens → cost ≈ $0.001 per sample. For the 1% sampling to stay ≤3%:

```
0.01 × $0.001 ≤ 0.03 × measured_savings_per_request
$0.00001      ≤ 0.03 × MS
MS            ≥ $0.00033 per request  (i.e. ≥ ~33 "cents-per-100-requests" order)
```

A single prevented frontier loop turn or a cached frontier response clears this by an order of magnitude. For **cheap-model traffic** (all-mini workloads), L2 sampling auto-degrades faster — by design ([03 §9](03-detection-engine.md)): the ladder exists precisely so the ratio never goes toxic when savings are thin.

---

## 3. Three customer sizes — monthly model

### 3.1 Small startup — 10 engineers, ~1M req/mo

| Line | Value | Derivation |
|---|---|---|
| Gross LLM spend | **$4,000** | assumption (median small-agent workload) |
| Waste (A1 @ 30%) | $1,200 | |
| Detected waste | $1,200 | L0+L1 coverage of loop/overfetch/verbosity |
| Measured savings (A2 @ 50%) | **$600** | only `measured` class billed ([05 §3.1](05-savings-ledger.md)) |
| Projected (disclosed, not billed) | ~$300 | circuit-breaker horizon class |
| **TBS fee (A3 @ 12%)** | **$72** | |
| Our infra (A6+A7) | ~$14 + $3 = **$17** | 1M req × ~$14/M + small tenant CP share |
| Our L2 spend | ≤ $18 | 1% × 1M = 10k samples × ~$0.0015 = $15, budget-capped at 3%×$600 = **$18** → ratio 3.0% ceiling, ladder keeps ≤3% |
| **Our gross margin on this tenant** | **$72 − $17 − ≤$18 = ≥$37/mo (≥51%)** | L2 is the margin risk; ladder is the defense |
| Customer ROI | ($600 − $72)/$72 ≈ **733%** | |

*Reading*: small tenants are thin absolute-margin but the free tier is top-of-funnel (usage cap, observe-only); the value is conversion, not this line.

### 3.2 Growth startup — 50 engineers, ~20M req/mo

| Line | Value | Derivation |
|---|---|---|
| Gross LLM spend | **$90,000** | heavier agent usage, some frontier routing |
| Waste (30%) | $27,000 | |
| Measured savings (50%) | **$13,500** | cache + routing (measured-heavy) + clamp |
| Projected (disclosed) | ~$6,750 | breaker horizon |
| **TBS fee (12%)** | **$1,620** | |
| Our infra | 20M × $12/M = $240 + $20 CP = **$260** | |
| Our L2 spend | budget cap = 3% × $13,500 = **$405** (actual likely $250–350; ladder trims if hot) | 20M × 1% = 200k samples would be $300 @ $0.0015 — within cap |
| **Gross margin** | **$1,620 − $260 − ≤$405 = ≥$955/mo (~59%)** | |
| Customer ROI | ($13,500 − $1,620)/$1,620 ≈ **733%** | scales linearly at fixed rate |

*Reading*: the flywheel — more traffic → more cache hits (measured) → bigger denominator → L2 ratio falls → margin expands. Routing rewrites on frontier traffic are the highest-value measured savings.

### 3.3 Enterprise — 500 engineers, ~500M req/mo (BYOC/self-host license)

| Line | Value | Derivation |
|---|---|---|
| Gross LLM spend | **$2,000,000** | multi-team agents + production workloads |
| Waste (25%, more mature teams) | $500,000 | |
| Measured savings (50%) | **$250,000** | |
| Projected (disclosed) | ~$100,000 | |
| **TBS fee** | annual license **$180–240k/yr (~$15–20k/mo)** OR 10% of measured ($25k/mo) — contract choice | savings-anchored *or* license; both defensible |
| Our infra (BYOC) | ~**$0 marginal** (they run the binary; our cost = support + CP mirror) | [08](08-deployment-topologies.md) |
| Our L2 spend | **$0 if customer-key mode** (enterprise default); else budget-capped at 3%×$250k = $7.5k | BYOC answer to the L2-cost question |
| **Gross margin** | license path: **~$15–20k/mo revenue at ~$0 infra = very high**; savings-path: $25k − support amortization | enterprise is highest-margin tier ([COMPETETIVE_ANALYSIS](../COMPETETIVE_ANALYSIS.md)) |
| Customer ROI | ($250k − $20k)/$20k ≈ **1,150%** at license fee | |

*Reading*: enterprise economics invert the L2 problem (customer keys → our intelligence spend ≈ 0) and the infra problem (they host). Our cost is support + compliance, which is why the compliance posture ([09](09-security-threat-model.md)) is itself a margin feature.

---

## 4. Consolidated view

| | Small (1M) | Growth (20M) | Enterprise (500M) |
|---|---|---|---|
| Customer measured savings | $600 | $13,500 | $250,000 |
| Our revenue | $72 | $1,620 | ~$15–25k |
| Our infra | $17 | $260 | ~$0 (BYOC) |
| Our intelligence (L2) | ≤$18 (capped) | ≤$405 (capped) | $0 (customer key) |
| **Our gross margin** | **≥51%** | **≥59%** | **very high (license)** |
| Intelligence as % of savings | ≤3.0% | ≤3.0% | 0% (BYOC) or ≤3% (SaaS) |
| Customer ROI | ~733% | ~733% | ~1,150% |

**Takeaways**

1. **The 3% cap is the margin governor**: L2 is the only per-request variable cost that scales with usage; the ladder makes it sub-linear under pressure, and cache/routing savings grow the denominator. The unit is designed so intelligence spend *cannot* outrun measured savings — the mechanism is automatic, not aspirational.
2. **Free tier = CAC, not margin**: observe-only, capped, no L2 — bounded loss, maximum funnel.
3. **Enterprise BYOC collapses both cost curves** (no infra, no L2) — which is exactly why compliance + self-host is the moat ([COMPETETIVE_ANALYSIS](../COMPETETIVE_ANALYSIS.md) §Distribution).
4. **Risk concentration**: A2 (measurable-prevention rate) drives *everything*. Phase 0/1 roadmap ([11](11-roadmap.md)) is sequenced to falsify A2 before we scale sales, not after.

---

*Next: [11 — Phased Roadmap](11-roadmap.md).*
