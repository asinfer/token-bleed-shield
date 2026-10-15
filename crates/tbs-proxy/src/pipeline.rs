//! Observe-mode request pipeline: collect → normalize → pre-dispatch detect →
//! forward → stream-scan → finalize → ledger. Nothing here mutates traffic
//! (docs/architecture/02); detection is a pure side channel.

use std::sync::Arc;
use std::time::Instant;

use bytes::Bytes;
use http_body_util::BodyExt;
use http_body_util::combinators::BoxBody as RawBoxBody;
use http_body_util::{LengthLimitError, Limited};
use hyper::body::{Body as HyperBody, Frame, Incoming};
use hyper::header::{CONTENT_TYPE, HeaderName, HeaderValue};
use hyper::{Method, Request, Response, StatusCode};
use serde_json::Value;
use tbs_core::adapter::{
    PartialUsage, detect_provider, extract_delta_text, extract_usage, is_done, normalize,
    sniff_usage_json,
};
use tbs_core::detectors::{RequestAnalysis, StreamDetectors, analyze_request, merge_verdicts};
use tbs_core::events::{AttrMethod, Attrib, UsageSource, validate_id_token, validate_label};
use tbs_core::ir::Provider;
use tbs_core::ledger::{RequestOutcome, build_events};
use tbs_core::scanner::{DEFAULT_FRAME_CAP_BYTES, SseScanner};
use tbs_core::sink::emit;
use tbs_core::token_est::estimate_tokens;
use tbs_core::window::{DEFAULT_WINDOW_BYTES, SlidingWindow};
use time::OffsetDateTime;
use time::format_description::well_known::Rfc3339;
use tracing::{error, info, warn};

use crate::state::AppState;
use crate::upstream;

pub type BoxBody = RawBoxBody<Bytes, hyper::Error>;

/// Streaming response body backed by a bounded mpsc of chunks.
pub struct ChannelBody {
    rx: tokio::sync::mpsc::Receiver<Bytes>,
}

impl HyperBody for ChannelBody {
    type Data = Bytes;
    type Error = hyper::Error;

    fn poll_frame(
        mut self: std::pin::Pin<&mut Self>,
        cx: &mut std::task::Context<'_>,
    ) -> std::task::Poll<Option<Result<Frame<Bytes>, hyper::Error>>> {
        match self.rx.poll_recv(cx) {
            std::task::Poll::Ready(Some(data)) => {
                std::task::Poll::Ready(Some(Ok(Frame::data(data))))
            }
            std::task::Poll::Ready(None) => std::task::Poll::Ready(None),
            std::task::Poll::Pending => std::task::Poll::Pending,
        }
    }
}

#[derive(Debug, thiserror::Error)]
pub enum ProxyError {
    #[error("request body exceeds the configured limit")]
    PayloadTooLarge,
    #[error("bad request: {0}")]
    BadRequest(&'static str),
    #[error("upstream failure: {0}")]
    Upstream(String),
}

impl ProxyError {
    fn status(&self) -> StatusCode {
        match self {
            ProxyError::PayloadTooLarge => StatusCode::PAYLOAD_TOO_LARGE,
            ProxyError::BadRequest(_) => StatusCode::BAD_REQUEST,
            ProxyError::Upstream(_) => StatusCode::BAD_GATEWAY,
        }
    }

    fn code(&self) -> &'static str {
        match self {
            ProxyError::PayloadTooLarge => "tbs_payload_too_large",
            ProxyError::BadRequest(code) => code,
            ProxyError::Upstream(_) => "tbs_upstream_error",
        }
    }
}

pub async fn handle(
    state: Arc<AppState>,
    req: Request<Incoming>,
) -> Result<Response<BoxBody>, std::convert::Infallible> {
    Ok(handle_inner(state, req).await)
}

async fn handle_inner(state: Arc<AppState>, req: Request<Incoming>) -> Response<BoxBody> {
    let t0 = Instant::now();
    let (parts, body) = req.into_parts();
    let request_id = state.next_request_id();

    if parts.method == Method::GET && parts.uri.path() == "/tbs/health" {
        let health = serde_json::json!({
            "status": "ok",
            "mode": "observe",
            "floor": state.policy.floor.as_str(),
            "guard_trips": tbs_core::events::GUARD_TRIPS.load(std::sync::atomic::Ordering::Relaxed),
            "requests": state.counter_snapshot(),
        });
        return json_response(StatusCode::OK, &health);
    }

    let body_bytes = match Limited::new(body, state.max_request_bytes).collect().await {
        Ok(c) => c.to_bytes(),
        Err(err) if err.downcast_ref::<LengthLimitError>().is_some() => {
            return error_response(&state, ProxyError::PayloadTooLarge, &request_id);
        }
        Err(_) => {
            return error_response(
                &state,
                ProxyError::BadRequest("tbs_body_read_failed"),
                &request_id,
            );
        }
    };

    let path_and_query = parts
        .uri
        .path_and_query()
        .map(|p| p.as_str().to_string())
        .unwrap_or_else(|| parts.uri.path().to_string());

    let provider = detect_provider(parts.uri.path());
    let feature_header = state
        .policy
        .attribution
        .feature_header
        .parse::<HeaderName>()
        .ok();
    let feature_label = feature_header
        .and_then(|name| parts.headers.get(&name))
        .and_then(|v| v.to_str().ok())
        .and_then(validate_label);

    let normalized = normalize(provider, &body_bytes, feature_label.clone()).ok();
    let analysis = normalized
        .as_ref()
        .map(|n| analyze_request(n, &state.policy));
    let model = normalized
        .as_ref()
        .map(|n| n.model.clone())
        .unwrap_or_default();
    let request_text = normalized
        .as_ref()
        .map(|n| n.request_text.clone())
        .unwrap_or_default();
    let stream_requested = normalized.as_ref().is_some_and(|n| n.stream);
    let trajectory_id = resolve_trajectory_id(&parts.headers, provider, &body_bytes, &request_id);

    let upstream_req = match upstream::build_upstream_request(
        &parts.method,
        &state.upstream,
        &path_and_query,
        &parts.headers,
        body_bytes.clone(),
    ) {
        Ok(r) => r,
        Err(err) => return error_response(&state, err, &request_id),
    };

    let resp = match tokio::time::timeout(state.connect_timeout, state.client.request(upstream_req))
        .await
    {
        Ok(Ok(r)) => r,
        Ok(Err(err)) => {
            return error_response(
                &state,
                ProxyError::Upstream(format!("request_failed: {err}")),
                &request_id,
            );
        }
        Err(_) => {
            return error_response(
                &state,
                ProxyError::Upstream("connect_timeout".into()),
                &request_id,
            );
        }
    };

    let (resp_parts, upstream_body) = resp.into_parts();
    let is_sse = resp_parts
        .headers
        .get(CONTENT_TYPE)
        .and_then(|v| v.to_str().ok())
        .is_some_and(|ct| ct.contains("text/event-stream"));

    let mut headers = upstream::filter_response_headers(&resp_parts.headers);
    if let Ok(id) = HeaderValue::from_str(&request_id) {
        headers.insert("x-tbs-request-id", id);
    }
    headers.insert("x-tbs-mode", HeaderValue::from_static("observe"));
    if let Some(label) = &feature_label
        && let Ok(v) = HeaderValue::from_str(label)
    {
        headers.insert("x-tbs-feature", v);
    }

    let (tx, stream_rx) = tokio::sync::mpsc::channel::<Bytes>(16);
    let mut response = Response::new(ChannelBody { rx: stream_rx }.boxed());
    *response.status_mut() = resp_parts.status;
    *response.headers_mut() = headers;

    let ctx = PumpCtx {
        request_id,
        trajectory_id,
        provider,
        model,
        is_sse,
        stream_requested,
        degraded: false,
        request_text,
        body_len: body_bytes.len(),
        analysis,
        t0,
    };
    tokio::spawn(async move {
        pump(state, upstream_body, tx, ctx).await;
    });

    response
}

fn resolve_trajectory_id(
    headers: &hyper::HeaderMap,
    provider: Provider,
    body: &[u8],
    fallback: &str,
) -> String {
    if let Some(v) = headers
        .get("x-tbs-trajectory-id")
        .and_then(|v| v.to_str().ok())
        .filter(|s| validate_id_token(s))
    {
        return v.to_string();
    }
    if let Ok(value) = serde_json::from_slice::<Value>(body) {
        let user = match provider {
            Provider::OpenAi => value.get("user").and_then(Value::as_str),
            Provider::Anthropic => value
                .get("metadata")
                .and_then(|m| m.get("user_id"))
                .and_then(Value::as_str),
        };
        if let Some(u) = user.filter(|s| validate_id_token(s)) {
            return u.to_string();
        }
    }
    fallback.to_string()
}

fn error_response(_state: &AppState, err: ProxyError, request_id: &str) -> Response<BoxBody> {
    let mut resp = Response::new(upstream::boxed_full(Bytes::from(format!(
        "tbs: {}\n",
        err.code()
    ))));
    *resp.status_mut() = err.status();
    resp.headers_mut()
        .insert(CONTENT_TYPE, HeaderValue::from_static("text/plain"));
    if let Ok(id) = HeaderValue::from_str(request_id) {
        resp.headers_mut().insert("x-tbs-request-id", id);
    }
    resp.headers_mut()
        .insert("x-tbs-mode", HeaderValue::from_static("observe"));
    resp
}

fn json_response(status: StatusCode, value: &serde_json::Value) -> Response<BoxBody> {
    let mut resp = Response::new(upstream::boxed_full(Bytes::from(
        serde_json::to_vec(value).unwrap_or_default(),
    )));
    *resp.status_mut() = status;
    resp.headers_mut()
        .insert(CONTENT_TYPE, HeaderValue::from_static("application/json"));
    resp
}

struct PumpCtx {
    request_id: String,
    trajectory_id: String,
    provider: Provider,
    model: String,
    is_sse: bool,
    #[allow(dead_code)]
    stream_requested: bool,
    degraded: bool,
    request_text: String,
    body_len: usize,
    analysis: Option<RequestAnalysis>,
    t0: Instant,
}

async fn pump(
    state: Arc<AppState>,
    upstream_body: Incoming,
    tx: tokio::sync::mpsc::Sender<Bytes>,
    ctx: PumpCtx,
) {
    let mut scanner = SseScanner::new(DEFAULT_FRAME_CAP_BYTES);
    let mut window = SlidingWindow::new(DEFAULT_WINDOW_BYTES);
    let mut tail = tbs_core::scanner::TailBuffer::new(16 * 1024);
    let mut detectors = StreamDetectors::new();
    let mut usage = PartialUsage::default();
    let mut degraded = ctx.degraded;
    let mut body = upstream_body;
    let mut done = false;

    while !done {
        match tokio::time::timeout(state.idle_timeout, body.frame()).await {
            Ok(Some(Ok(frame))) => {
                let Ok(data) = frame.into_data() else {
                    continue;
                };
                if tx.send(data.clone()).await.is_err() {
                    degraded = true;
                    break;
                }
                tail.push(&data);
                if ctx.is_sse {
                    for frame in scanner.feed(&data) {
                        if let Some(text) = extract_delta_text(ctx.provider, &frame) {
                            detectors.observe(&text);
                            window.push(&text);
                        }
                        usage.merge(extract_usage(ctx.provider, &frame));
                        if is_done(ctx.provider, &frame) {
                            done = true;
                        }
                    }
                    if scanner.take_overflow() {
                        degraded = true;
                    }
                }
            }
            Ok(Some(Err(_))) => {
                degraded = true;
                break;
            }
            Ok(None) => break,
            Err(_) => {
                degraded = true;
                break;
            }
        }
    }

    if !ctx.is_sse {
        usage.merge(sniff_usage_json(&tail.text()));
    }
    let window_text = window.text().to_string();
    // Explicit scopes: dropped before event build (memory-only, never logged).
    drop(window);
    drop(scanner);
    drop(tail);

    finalize(&state, &ctx, usage, &detectors, &window_text, degraded);
}

fn finalize(
    state: &AppState,
    ctx: &PumpCtx,
    mut usage: PartialUsage,
    detectors: &StreamDetectors,
    window_text: &str,
    mut degraded: bool,
) {
    if usage.input.is_none() {
        usage.input = Some(if ctx.request_text.is_empty() {
            (ctx.body_len / 4) as u32
        } else {
            estimate_tokens(&ctx.request_text)
        });
    }
    if usage.output.is_none() {
        usage.output = Some(detectors.output_tokens_est());
    }
    let input = usage.input.unwrap_or(0);
    let output = usage.output.unwrap_or(0);
    let cache_read = usage.cache_read.unwrap_or(0);
    let source = if usage.reported() {
        UsageSource::Reported
    } else {
        UsageSource::Estimated
    };
    let (cost, price_known) = state
        .prices
        .cost_with_cache(&ctx.model, input, output, cache_read);

    let analysis = ctx.analysis.as_ref();
    let verdicts = match analysis {
        Some(a) => merge_verdicts(
            a.verdicts.clone(),
            detectors.finalize(window_text, &ctx.request_text),
        ),
        None => detectors.finalize(window_text, &ctx.request_text),
    };
    let attrib = analysis.map(|a| a.attrib.clone()).unwrap_or(Attrib {
        method: AttrMethod::None,
        feature_label: None,
        intent_cluster_id: None,
        template_hash: None,
        label_source: None,
        confidence: 0.0,
    });
    let overfetch = analysis.map(|a| a.overfetch.clone()).unwrap_or_default();
    let schema_reducible = analysis.map(|a| a.schema.reducible_tokens).unwrap_or(0);
    if analysis.is_none() {
        degraded = true;
    }

    let latency_ms = ctx.t0.elapsed().as_millis().min(u64::MAX as u128) as u32;
    let outcome = RequestOutcome {
        ts: now_rfc3339(),
        tenant_id: state.tenant.clone(),
        project_id: state.project.clone(),
        trajectory_id: ctx.trajectory_id.clone(),
        request_id: ctx.request_id.clone(),
        model: ctx.model.clone(),
        input_tokens: input,
        output_tokens: output,
        cache_read_tokens: cache_read,
        usage_source: source,
        cost_usd: cost,
        price_known,
        price_table_v: state.prices.version,
        latency_ms,
        attrib,
        verdicts,
        overfetch,
        schema_reducible_tokens: schema_reducible,
        degraded,
    };

    let events = build_events(&outcome, &state.policy, &state.prices);
    for ev in &events {
        let out = emit(&state.sinks, ev);
        if out.guard_tripped {
            error!(request_id = %ctx.request_id, "egress guard tripped; event dropped");
        }
        for err in &out.sink_errors {
            warn!(request_id = %ctx.request_id, "sink error: {err}");
        }
    }

    info!(
        request_id = %ctx.request_id,
        trajectory_id = %ctx.trajectory_id,
        model = %ctx.model,
        input_tokens = input,
        output_tokens = output,
        usage_source = ?source,
        cost_usd = cost,
        price_known,
        events = events.len(),
        degraded,
        latency_ms,
        "tbs_request_complete"
    );
}

fn now_rfc3339() -> String {
    OffsetDateTime::now_utc()
        .format(&Rfc3339)
        .unwrap_or_else(|_| "1970-01-01T00:00:00Z".into())
}
