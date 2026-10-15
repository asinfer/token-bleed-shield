# Customer Identification and Problem Analysis

Every token an LLM processes costs money. The real question is: how much of it is actually wasted?

So, in today's session, I want to evaluate and discuss an idea I've been circling around: creating a preventive solution for a problem that quietly drains budgets across the AI industry. It's the problem of AI drift, where models waste valuable tokens through hallucinations, context rot, working on the wrong context, filler data and repeated data, repeated context reloads, poor retrieval, over-fetched tools and verbose shell outputs, always routing the frontier model for everything, verbose system prompts, and finally, the deeper challenge of understanding the intent behind that verbose token waste to stop infinite loops before they spiral.

Each of these failure modes looks small in isolation. A verbose shell output here, an unnecessary context reload there. But multiply them across hundreds of daily API calls, and the waste compounds fast.

The goal is to find a viable solution. 

## What the Market Currently Offers

For these exact problems, the current solutions in the market fall into two camps:

1. **Custom firm-based, self-built LLM observability portals using FOSS tools.** Firms with engineering bandwidth roll their own internal dashboards just to keep an eye on token usage.
2. **Individual developers who never cared.** They could pass with just a Pro subscription alone, costing 20 to 30 bucks a month. When your entire bill is one flat subscription, there's little incentive to measure waste at all.

This gap between "firms who build their own tooling" and "solo devs who don't need it" is exactly where the opportunity sits.

#### The Open Source Tools

The backbone of most current observability setups comes from a handful of open source projects:

- **Langfuse:** Tracks token usage per call, not per feature. That is the drawback. You can see how much you spent, but not *why* you spent it or which feature is burning the budget.
- **Arize Phoenix:** Another LLM observability portal for executions, using graph-based visualization. Useful for tracing what happened, but it observes rather than prevents.
- **PromptFoo:** A system prompt benchmark. Helpful for testing prompts, but it doesn't touch live traffic.
- **LiteLLM:** Automatic fallback to cheaper LLMs. It reacts at the routing layer, switching models where possible.

#### The Competitor Analysis

To understand where the existing tools fall short, here's a breakdown of the three biggest names:

| Name | Why They Are a Threat | How They Prevent Waste |
| :--- | :--- | :--- |
| LiteLLM Proxy | Virtually every startup uses it as their central gateway | Real-time token budgets for teams/users, handles fallback to cheaper models, and drops inline malicious and looping connections |
| Portkey AI | The leading enterprise solution built specifically for high-volume control and cost governance | Employs a semantic caching layer (preventing similar requests) and automatic request conditioning/routing |
| LLMLingua | Gold standard for payload reduction, as it physically alters the requests to save money | Uses a tiny specialized model to compress massive contexts and RAG documents by 20x before sending them to the LLM |

These are serious tools with real adoption. So where is the edge? The problem, or our edge over these existing tools, is that I justify it by three specific gaps:

1. **LiteLLM** can't handle complex requests; it's just reactive. It sets budgets and reroutes traffic, but it doesn't understand what a request is actually trying to do. Before work even starts, the money is already gone.
2. **Portkey AI:** Even though it tries to stop similar requests through caching, if the intent is unknown, the model still goes into infinite output mode or thinking mode. Caching only helps when requests repeat. Drift doesn't repeat; it mutates.
3. **LLMLingua:** Even though it can compress input prompts, the output is decided by the context it has. If the context is rotten, there's no use for a better prompt. Compression saves money on garbage input, but the model still produces garbage output.

The pattern across all three is the same: they optimize pieces of the pipeline, but none of them understand the *intent* behind the tokens.

#### Who Are Our Customers?

Mostly startup founders and firms, not solo devs.

Here is why: current solo devs don't care about their project deadlines the same way. They could just wait until their limit resets, and for them, a Pro or a simple subscription is enough. There's no payroll, no burn rate, no investors asking why the API bill tripled this month.

Startups and firms are a different story. They have teams hitting rate limits, production workloads running around the clock, and finance teams actually reading the invoices. Token waste for them isn't a minor inconvenience; it's a line item that grows every month.

That said, we could still have some solo enthusiasts use it, and they matter as early adopters and community voices.

#### Legal Compliance

Since the product sits directly between clients and their LLM providers, intercepting payloads and API keys, the legal foundation has to be solid before a single enterprise customer signs on:

- **Limitation of Liability (ToS):** Protects you from lawsuits if your code accidentally kills a critical enterprise production loop.
- **Data Processing Addendum (DPA):** Legally binds you as a "Data Processor" who handles text but does not own or store it.
- **Zero-Retention Privacy Policy:** Bypasses deep GDPR/CCPA audits by explicitly stating you never save prompt payloads to a database.
- **Encryption In-Transit (TLS 1.3):** Mandates that all intercepted API keys and payloads are encrypted while moving through your proxy.
- **Self-Hosted License (EULA):** Passes all security compliance risks entirely to the client by letting them run your Docker container inside their own cloud.

#### Our Business Model

**Open-core, savings-anchored SaaS. We sell prevention, not observation.**

Every existing competitor monetizes visibility: dashboards, traces, logs. Our model monetizes the outcome instead, which changes the entire sales conversation from "look at your data" to "watch your bill shrink."

**Revenue Streams:**

1. **Free Tier (Solo Enthusiasts & Evaluation):** Limited monthly request analysis with community support. This tier acts as our top-of-funnel. It lets solo devs experience the waste-detection value firsthand, building the community credibility that firms look for before adopting any tool.
2. **Startup Tier (Subscription):** Per-seat monthly pricing with metered analysis volume. Includes full intent detection, loop-killing, context-health scoring, and semantic caching. This tier targets funded startups that already feel token burn and have the budget to fix it.
3. **Enterprise Tier (Self-Hosted License):** Annual license to run our Docker container inside the client's own cloud. Includes DPA, EULA, SLA-backed support, and full compliance posture. This is our highest-margin tier, since security risk transfers entirely to the client and they pay for the privilege of keeping everything in-house.

**Why Customers Pay:** The product is priced as a fraction of the measurable savings it creates. If we detect and prevent $10k of monthly token waste, we charge a small percentage of that proven saving. The tool pays for itself, which makes the ROI story trivial to sell to founders. Nobody debates an invoice that shows up already covered by savings.

**Distribution:** FOSS-first (like LiteLLM) to earn developer trust and community adoption, then convert firms through the self-hosted enterprise license, where compliance becomes the moat.

## Closing Thoughts

The problem is real, the customers are identifiable, and the existing tools leave a clear gap: nobody is attacking token waste at the level of intent. Before jumping to a solution, the next step is to sit with this problem a little longer and understand exactly where, how, and why the tokens leak.
