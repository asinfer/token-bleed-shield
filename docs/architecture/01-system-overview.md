# 01 · System Overview

**Role of this doc**: C4 context + container diagrams, component inventory, and the explicit ANTI-GOALS that bound the design.

---

## 1. C4 — Context (system in its environment)

```mermaid
flowchart TB
    subgraph Customer["Customer environment"]
        APP["Customer application / agent framework<br/>(OpenAI / Anthropic / Google / Bedrock / Azure SDK calls)"]
        DEV["Engineers &amp; finance team"]
        VPC["Customer VPC<br/>(BYOC / self-hosted topologies)"]
    end

    subgraph TBS["Token Bleed Shield"]
        DP["Data Plane proxy<br/>(inline, streaming)"]
        DET["Detection Engine<br/>(L0/L1/L2 tiered)"]
        ACT["Actuation Layer<br/>(cache, clamp, break, route)"]
        LEDG["Savings Ledger<br/>(counterfactual receipts)"]
        CP["Control Plane<br/>(tenants, policies, RBAC)"]
        OBS["Event Store + OTel export"]
    end

    subgraph Providers["LLM providers"]
        OAI["OpenAI"]
        ANT["Anthropic"]
        GEM["Google"]
        AWS["Bedrock"]
        AZ["Azure OpenAI"]
    end

    subgraph Third["Ecosystem"]
        OSS["Langfuse / Datadog / Grafana<br/>(customer-owned OTel consumers)"]
        BILL["OpenMeter-class metering + Stripe"]
        ALERT["Slack / PagerDuty / webhooks"]
        KMS["KMS (ours or customer's)"]
    end

    APP -->|"API calls (SSE/chunked)"| DP
    DP --> OAI & ANT & GEM & AWS & AZ
    DP --> DET --> ACT
    DET & ACT --> LEDG
    LEDG --> CP
    CP --> DET & ACT
    DET & ACT --> OBS
    OBS --> OSS
    LEDG --> BILL
    CP --> ALERT
    DP <--> KMS
    DEV -->|"console: policies, receipts, rollout"| CP
    VPC -.->|"BYOC: same binary, customer-run"| DP
```

**Relationship summary**

| Neighbor | Direction | Protocol |
|---|---|---|
| Customer app | inbound | OpenAI-compatible / Anthropic / Google / Bedrock / Azure APIs, HTTP/1.1 + SSE |
| LLM providers | outbound | same, TLS 1.3 |
| OTel consumers (Langfuse, Datadog) | outbound | OTLP/gRPC or OTLP/HTTP |
| Metering/billing | outbound | REST webhook (OpenMeter-compatible events → Stripe) |
| Alerting | outbound | Slack webhook / PagerDuty Events API v2 / generic webhook |
| KMS | bidirectional | AWS KMS / GCP KMS / Vault transit (envelope crypto) |

---

## 2. C4 — Containers

```mermaid
flowchart TB
    subgraph Edge["Edge"]
        LB["Load balancer / Ingress<br/>(mTLS, per-tenant routing)"]
    end

    subgraph DataPlane["Data plane — one core binary: tbs-proxy (Rust)"]
        PA["Protocol adapters<br/>normalize provider wire formats"]
        SP["Streaming pipeline<br/>byte-passthrough + frame scanner"]
        L0["L0 detectors (in-process, zero $ marginal cost)"]
        L1["L1 detectors (embedded ONNX / simhash / local embedder)"]
        ACF["Actuators<br/>cache · clamp · breaker · router"]
        NB["No-payload guard<br/>(egress filters: metadata only)"]
    end

    subgraph Sidecar["Detection sidecar (optional, same binary flag)"]
        L2["L2 sampler<br/>(frontier checks, customer key)"]
    end

    subgraph Control["Control plane — tbs-control (Go)"]
        API["REST/GraphQL API + console BFF"]
        POL["Policy service<br/>(YAML→RBAC→rollout%, kill switch)"]
        TEN["Tenancy &amp; entitlements"]
        AUD["Audit log writer (append-only)"]
    end

    subgraph Data["Data services"]
        PG["PostgreSQL<br/>(tenants, policies, audit metadata)"]
        CH["ClickHouse<br/>(events, ledger, attribution)"]
        RD["Redis<br/>(semantic cache index, session/loop state)"]
        OBJ["Object store (opt-in sampled traces only,<br/>redacted before write)"]
    end

    subgraph Ott["Interops"]
        OTLP["OTLP exporter"]
        WH["Billing &amp; alerting webhooks"]
    end

    Client["Customer app"] --> LB --> PA --> SP
    SP <--> L0 & L1
    SP <--> ACF
    L0 & L1 -->|"decisions + metrics"| NB
    L2 -.->|"sampled, ambiguous only"| NB
    NB -->|"metadata events"| CH
    NB -->|"cache lookup/put"| RD
    L0 & L1 & ACF -->|"policy fetch (cached)"| POL
    API --> POL & TEN & AUD
    AUD --> PG
    POL --> PG
    CH --> OTLP & WH
    PA -->|"forwarded request"| Prov["Providers"]
```

### Component inventory

| Container | Responsibility (one paragraph) |
|---|---|
| **`tbs-proxy` — protocol adapters** | Terminates the customer's provider-shaped HTTP request, normalizes messages/tool schemas/usage into a single internal `NormalizedRequest` IR, and re-serializes to the target provider. Owns base-URL-swap, SDK-wrapper, and sidecar attachment modes, key custody (inject/censor `Authorization`), and provider quirks (Bedrock SigV4, Azure deployment paths, Anthropic `system` splitting). Must be pure passthrough when all policies are observe-mode. |
| **`tbs-proxy` — streaming pipeline** | Forwards response bytes chunk-by-chunk with bounded buffers: an incremental SSE/frame scanner feeds a sliding window to detectors while unmodified bytes flow straight to the client. Implements backpressure (paused upstream reads), TTFB budget, and mid-stream intervention primitives (inject SSE event, truncate with terminator, or abort). Never accumulates a full response. |
| **`tbs-proxy` — L0 detectors** | In-process heuristics at zero marginal cost: n-gram repetition/entropy, exact & near-dup hashes, regex structural bloat (stacktraces, base64, log floods), token estimates, loop counters over trajectory metadata. |
| **`tbs-proxy` — L1 detectors** | Embedded small models: quantized ONNX classifiers (verbosity, filler), local embedding model for similarity over sliding windows, MinHash/simhash for near-dup, routing-complexity scorer. Runs in-process or in the optional detection sidecar to bound p99. |
| **`tbs-proxy` — actuators** | Enforces decisions: semantic-cache hit injection, tool-output clamping/summarization, loop circuit-breaker (SSE interrupt), routing policy rewrite (model swap before dispatch), prompt-hygiene rewrite (off by default). Every actuation emits an audit event with before/after token counts. |
| **`tbs-proxy` — no-payload guard** | Architectural enforcement of zero-retention: a single choke point where event builders can only receive an allowlisted field set (ids, counts, hashes, latencies, verdicts). Typed so payload references cannot flow to any sink; sampled-trace export is a separate, explicitly opted-in path that passes the PII redactor first. |
| **L2 sampler** | Frontier-model checks (grounding verification, ambiguous loop disambiguation, intent clustering refinement) executed only on sampled/ambiguous cases, priced against the 3% intelligence budget, using **customer-supplied keys** in BYOC so payloads never hit our model accounts. Degrades off first when margin thins. |
| **`tbs-control` — API + console BFF** | Serves tenant onboarding, policy CRUD, rollout controls, receipts, and audit query. Stateless; all state in Postgres/ClickHouse. |
| **Policy service** | Compiles declarative YAML policies (see [04](04-actuation-layer.md)) into a signed, versioned snapshot distributed to proxies; enforces per-policy rollout percentage, modes, entitlements, and the global kill switch. |
| **Tenancy & entitlements** | Orgs → projects → api-keys model, RBAC roles, per-tier feature entitlements (which detectors/actuators a plan may enable), rate limits for L2 quota. |
| **Audit log writer** | Append-only record of every intervention decision (attempted, applied, suppressed, overridden) — the substrate for dispute resolution and the kill-switch forensics. |
| **PostgreSQL** | Source of truth for tenants, RBAC, policy snapshots, entitlements, audit metadata; Postgres-native encryption at rest + row-level security per tenant. |
| **ClickHouse** | High-volume event + ledger storage (detected waste, counterfactuals, attribution) — see [07](07-data-architecture.md) for justification. |
| **Redis** | Semantic-cache vector index (via HNSW set ops or embedded index), loop/trajectory session state, policy snapshot cache, rate-limit counters. Ephemeral by design — no payload at rest. |
| **Object store (opt-in)** | Only PII-redacted, explicitly sampled traces; off by default; separate KMS key; short retention (7/30 days by tier). |
| **OTLP exporter** | Streams metrics/log/trace events to customer-owned Langfuse/Datadog/Grafana — interop is a product feature, not a fallback. |
| **Billing & alerting webhooks** | Emits metering events (OpenMeter-class schema) and alert routes (Slack/PagerDuty/webhook). |

---

## 3. Request lifecycle (happy path, shadow mode)

```mermaid
sequenceDiagram
    autonumber
    actor C as Customer app
    participant P as tbs-proxy (adapter)
    participant S as Streaming pipeline
    participant D as Detectors (L0/L1)
    participant U as Upstream provider

    C->>P: POST /v1/chat/completions (SSE)
    P->>P: normalize → NormalizedRequest (memory only)
    P->>D: pre-dispatch L0/L1 on request body
    D-->>P: verdicts (advisory, mode=observe)
    P->>U: forward request (unmodified in observe mode)
    U-->>S: chunk 1..n (bytes)
    S->>S: scan frames, update sliding window
    S->>D: windowed features (metadata + window)
    D-->>S: verdicts (loop / verbosity / grounding risk)
    S-->>C: bytes forwarded immediately (no buffering)
    S->>S: on completion: emit events via no-payload guard
    Note over P,C: enforce mode only: inject SSE event,<br/>truncate, cache-hit, or route rewrite
```

---

## 4. ANTI-GOALS (what we deliberately do not build)

| Anti-goal | Why |
|---|---|
| **Dashboards-first observability** | Langfuse/Arize own per-call dashboards. We sell *prevention* priced on savings; a dashboard is a support tool, not the product. Our console shows receipts, policies, and rollout state — not trace explorers. |
| **Payload persistence** | Prompt/completion bytes never hit disk in the hot path (constraint #1). No dataset moat built on customer data. Opt-in sampled traces are redacted first. |
| **Training on customer data** | Non-starter for DPA/trust; also unnecessary — our intelligence layer uses small self-hosted models + customer-keyed L2 calls, not fine-tuned-on-tenant-data models. |
| **Hosting frontier models** | No inference platform, no GPU fleet. L2 is a thin, budgeted caller of the customer's own provider keys (or ours, strictly opt-in in SaaS mode). |
| **Replacing the customer's gateway/SDK** | Interop, not lock-in: OTel out to Langfuse/Datadog, standard wire protocols in, no proprietary SDK required for base-URL-swap mode. |
| **Generic prompt compression as the product** | LLMLingua-style compression is a *possible actuator*, not the differentiator. Our edge is trajectory-level governance + intent fingerprinting (see [COMPETETIVE_ANALYSIS](../COMPETETIVE_ANALYSIS.md)). |
| **Budget alerts as the primary control** | Reactive, post-hoc — the "2 AM email" the [manifesto](../PREVENTIVE_MANIFESTO.md) rejects. Budgets exist as a backstop actuator, not the pitch. |
| **Self-hosted model weights for L1** | Keep install footprint one binary + optional ONNX files; no PyTorch runtime, no model-serving stack in customer VPC beyond quantized files we ship. |

---

*Next: [02 — Data Plane](02-data-plane.md).*
