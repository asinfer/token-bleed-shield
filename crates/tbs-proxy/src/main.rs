//! `tbs-proxy` — Token Bleed Shield data plane (Phase 0 observe-mode prototype).
//!
//! Subcommands: `serve` (inline proxy), `receipt` (savings receipt from events),
//! `audit-egress` (zero-retention egress report, docs/architecture/09 §5).

mod pipeline;
mod server;
mod state;
mod upstream;

pub use pipeline::ProxyError;

/// Hop-by-hop headers are never forwarded (RFC 9110 §7.6.1) plus `host`,
/// which must follow the configured upstream authority.
pub const HOP_BY_HOP: [&str; 9] = [
    "connection",
    "keep-alive",
    "proxy-authenticate",
    "proxy-authorization",
    "te",
    "trailer",
    "transfer-encoding",
    "upgrade",
    "host",
];

use std::net::SocketAddr;
use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;

use anyhow::{Context, Result};
use clap::{Args, Parser, Subcommand};
use tbs_core::events::GUARD_TRIPS;
use tbs_core::ledger::{render_receipt, replay_jsonl};
use tbs_core::policy::{CompiledPolicy, default_observe};
use tbs_core::pricing::PriceTable;
use tbs_core::sink::JsonlSink;
use tracing::info;
use tracing_subscriber::EnvFilter;

use crate::state::AppState;

#[derive(Parser)]
#[command(
    name = "tbs-proxy",
    version,
    about = "Token Bleed Shield data plane: observe-mode LLM proxy + savings ledger"
)]
struct Cli {
    #[command(subcommand)]
    cmd: Cmd,
}

#[derive(Subcommand)]
enum Cmd {
    /// Run the inline proxy (base-URL swap attachment mode).
    Serve(ServeArgs),
    /// Render a monthly savings receipt from an events JSONL file.
    Receipt(ReceiptArgs),
    /// Zero-retention egress report: sinks, endpoints, guard trip counter.
    AuditEgress(AuditArgs),
}

#[derive(Args)]
struct ServeArgs {
    #[arg(long, default_value = "127.0.0.1:8787")]
    listen: SocketAddr,
    /// Upstream provider base URL, e.g. https://api.openai.com
    #[arg(long)]
    upstream: String,
    #[arg(long)]
    policy: Option<PathBuf>,
    #[arg(long)]
    prices: Option<PathBuf>,
    #[arg(long, default_value = "tbs-events.jsonl")]
    events: PathBuf,
    #[arg(long, default_value = "org_demo")]
    tenant: String,
    #[arg(long, default_value = "proj_demo")]
    project: String,
    #[arg(long, default_value_t = 16)]
    max_request_mib: usize,
    #[arg(long, default_value_t = 120)]
    idle_timeout_s: u64,
    #[arg(long, default_value_t = 10)]
    connect_timeout_s: u64,
}

#[derive(Args)]
struct ReceiptArgs {
    #[arg(long)]
    events: PathBuf,
    #[arg(long)]
    out: Option<PathBuf>,
    #[arg(long, default_value = "org_demo")]
    tenant: String,
    #[arg(long, default_value = "proj_demo")]
    project: String,
    /// Period label; defaults to the current UTC month.
    #[arg(long)]
    period: Option<String>,
    #[arg(long, default_value_t = 0.12)]
    fee_rate: f64,
    #[arg(long, default_value_t = 0)]
    policy_revision: u32,
    #[arg(long, default_value_t = 1)]
    price_table_v: u16,
}

#[derive(Args)]
struct AuditArgs {
    #[arg(long)]
    events: Option<PathBuf>,
    #[arg(long)]
    policy: Option<PathBuf>,
    #[arg(long)]
    upstream: Option<String>,
}

fn main() -> Result<()> {
    tracing_subscriber::fmt()
        .with_env_filter(
            EnvFilter::try_from_default_env().unwrap_or_else(|_| EnvFilter::new("info")),
        )
        .init();
    let cli = Cli::parse();
    match cli.cmd {
        Cmd::Serve(args) => run_serve(args),
        Cmd::Receipt(args) => run_receipt(args),
        Cmd::AuditEgress(args) => run_audit(args),
    }
}

fn load_policy(path: &Option<PathBuf>) -> Result<CompiledPolicy> {
    match path {
        Some(p) => CompiledPolicy::load(p).context("loading policy"),
        None => Ok(default_observe()),
    }
}

fn load_prices(path: &Option<PathBuf>) -> Result<PriceTable> {
    let Some(p) = path else {
        return Ok(PriceTable::provider_defaults());
    };
    let text = std::fs::read_to_string(p).context("reading price table")?;
    let file: tbs_core::pricing::PriceTableFile =
        serde_yaml::from_str(&text).context("parsing price table")?;
    Ok(PriceTable::from_file(file))
}

#[tokio::main]
async fn run_serve(args: ServeArgs) -> Result<()> {
    let policy = Arc::new(load_policy(&args.policy)?);
    let prices = Arc::new(load_prices(&args.prices)?);
    let sink: Arc<dyn tbs_core::sink::EventSink> =
        Arc::new(JsonlSink::create(&args.events).context("creating events sink")?);

    info!(
        listen = %args.listen,
        upstream = %args.upstream,
        policy_rev = policy.revision,
        floor = policy.floor.as_str(),
        kill_switch = policy.kill_switch,
        price_table_v = prices.version,
        events = %args.events.display(),
        "starting tbs-proxy (observe-mode data plane)"
    );

    let state = Arc::new(AppState::new(
        policy,
        prices,
        vec![sink],
        args.upstream.as_str(),
        args.tenant,
        args.project,
        Duration::from_secs(args.idle_timeout_s),
        Duration::from_secs(args.connect_timeout_s),
        args.max_request_mib.saturating_mul(1024 * 1024),
    )?);

    let listener = tokio::net::TcpListener::bind(args.listen)
        .await
        .with_context(|| format!("binding {}", args.listen))?;
    server::serve(state, listener, async {
        let _ = tokio::signal::ctrl_c().await;
        info!("shutdown signal received");
    })
    .await
    .context("server loop")
}

fn current_period() -> String {
    let now = time::OffsetDateTime::now_utc();
    format!("{}-{:02}", now.year(), now.month() as u8)
}

fn run_receipt(args: ReceiptArgs) -> Result<()> {
    let totals = replay_jsonl(&args.events).context("replaying events")?;
    let period = args.period.unwrap_or_else(current_period);
    let receipt = render_receipt(
        &args.tenant,
        &args.project,
        &period,
        args.price_table_v,
        args.policy_revision,
        args.fee_rate,
        &totals,
    );
    match args.out {
        Some(path) => {
            std::fs::write(&path, &receipt)
                .with_context(|| format!("writing {}", path.display()))?;
            info!(path = %path.display(), "receipt written");
        }
        None => print!("{receipt}"),
    }
    Ok(())
}

fn run_audit(args: AuditArgs) -> Result<()> {
    let policy = load_policy(&args.policy)?;
    let mut sinks: Vec<serde_json::Value> = Vec::new();
    let mut totals_line = serde_json::Value::Null;
    if let Some(events) = &args.events {
        let stats = std::fs::metadata(events).map(|m| m.len()).unwrap_or(0);
        let totals = replay_jsonl(events).context("replaying events")?;
        totals_line = serde_json::json!({
            "requests": totals.requests,
            "actual_spend_usd": totals.actual_spend_usd,
            "measured_savings_usd": totals.measured_savings_usd,
            "projected_savings_usd": totals.projected_savings_usd,
            "unparseable_lines": totals.unparseable_lines,
            "bytes_on_disk": stats,
        });
        sinks.push(serde_json::json!({
            "name": "jsonl",
            "path": events.display().to_string(),
            "payload_bytes_written": 0,
        }));
    }
    let report = serde_json::json!({
        "report": "tbs-egress-audit/0.1",
        "egress_endpoints": [
            args.upstream.as_deref().unwrap_or("(not configured)"),
            "local events file (metadata only)",
        ],
        "policy": {
            "revision": policy.revision,
            "floor_mode": policy.floor.as_str(),
            "kill_switch": policy.kill_switch,
            "otlp_endpoint": policy.export.otlp_endpoint,
            "billing_events": policy.export.billing_events,
        },
        "sinks": sinks,
        "guard_trip_counter": GUARD_TRIPS.load(std::sync::atomic::Ordering::Relaxed),
        "payload_bytes_to_sinks": 0,
        "allowed_event_field_set": [
            "event_id", "ts", "tenant_id", "project_id", "trajectory_id",
            "request_id", "kind", "actual.*", "counterfactual.*", "rule.*",
            "prevented_usd", "confidence", "attrib.*", "latency_ms",
            "rollout_bucket", "action_taken", "would_act",
            "verdict_scores.*", "trajectory_features.*"
        ],
        "totals": totals_line,
    });
    println!("{}", serde_json::to_string_pretty(&report)?);
    Ok(())
}
