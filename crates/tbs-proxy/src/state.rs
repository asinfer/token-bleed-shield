//! Shared proxy state: compiled policy, price table, sinks, upstream client.

use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::Duration;

use anyhow::{Context, Result};
use http_body_util::Full;
use hyper_rustls::HttpsConnector;
use hyper_util::client::legacy::Client;
use hyper_util::client::legacy::connect::HttpConnector;
use tbs_core::policy::CompiledPolicy;
use tbs_core::pricing::PriceTable;
use tbs_core::sink::EventSink;

pub type UpstreamClient = Client<HttpsConnector<HttpConnector>, Full<bytes::Bytes>>;

pub struct AppState {
    pub policy: Arc<CompiledPolicy>,
    pub prices: Arc<PriceTable>,
    pub sinks: Vec<Arc<dyn EventSink>>,
    pub client: UpstreamClient,
    pub upstream: hyper::Uri,
    pub tenant: String,
    pub project: String,
    pub idle_timeout: Duration,
    pub connect_timeout: Duration,
    pub max_request_bytes: usize,
    counter: AtomicU64,
    start_unix_ms: u128,
}

impl AppState {
    #[allow(clippy::too_many_arguments)]
    pub fn new(
        policy: Arc<CompiledPolicy>,
        prices: Arc<PriceTable>,
        sinks: Vec<Arc<dyn EventSink>>,
        upstream: &str,
        tenant: String,
        project: String,
        idle_timeout: Duration,
        connect_timeout: Duration,
        max_request_bytes: usize,
    ) -> Result<Self> {
        let upstream: hyper::Uri = upstream.parse().context("parsing upstream URL")?;
        if !matches!(upstream.scheme_str(), Some("http" | "https"))
            || upstream.authority().is_none()
        {
            anyhow::bail!("upstream must be http(s) with a host, got: {upstream}");
        }
        let start_unix_ms = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_millis())
            .unwrap_or(0);
        Ok(Self {
            policy,
            prices,
            sinks,
            client: crate::upstream::build_client(),
            upstream,
            tenant,
            project,
            idle_timeout,
            connect_timeout,
            max_request_bytes,
            counter: AtomicU64::new(0),
            start_unix_ms,
        })
    }

    pub fn next_request_id(&self) -> String {
        let n = self.counter.fetch_add(1, Ordering::Relaxed);
        format!(
            "req_{:012x}_{:08x}",
            self.start_unix_ms % 0xffff_ffff_ffff,
            n
        )
    }

    pub fn counter_snapshot(&self) -> u64 {
        self.counter.load(Ordering::Relaxed)
    }
}
