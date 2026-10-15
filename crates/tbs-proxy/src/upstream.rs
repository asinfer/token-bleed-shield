//! Upstream client construction + header hygiene for observe-mode forwarding.

use std::time::Duration;

use bytes::Bytes;
use http_body_util::BodyExt;
use http_body_util::Full;
use http_body_util::combinators::BoxBody;
use hyper::Request;
use hyper::header::{ACCEPT_ENCODING, CONTENT_LENGTH, HeaderMap, HeaderName, HeaderValue};
use hyper_rustls::HttpsConnectorBuilder;
use hyper_util::client::legacy::Client;
use hyper_util::rt::TokioExecutor;

use crate::state::UpstreamClient;
use crate::{HOP_BY_HOP, ProxyError};

pub fn build_client() -> UpstreamClient {
    let https = HttpsConnectorBuilder::new()
        .with_webpki_roots()
        .https_or_http()
        .enable_http1()
        .build();
    Client::builder(TokioExecutor::new())
        .pool_idle_timeout(Duration::from_secs(90))
        .build(https)
}

/// Join the configured upstream base with the inbound path + query.
pub fn join_uri(base: &hyper::Uri, path_and_query: &str) -> Result<hyper::Uri, ProxyError> {
    let scheme = base.scheme_str().unwrap_or("https");
    let authority = base
        .authority()
        .ok_or_else(|| ProxyError::Upstream("upstream missing authority".into()))?
        .as_str();
    let prefix = base.path().trim_end_matches('/');
    let target = format!("{scheme}://{authority}{prefix}{path_and_query}");
    target
        .parse()
        .map_err(|_| ProxyError::Upstream(format!("bad target uri: {target}")))
}

fn header_allowed(name: &HeaderName) -> bool {
    let n = name.as_str();
    HOP_BY_HOP.iter().any(|h| n.eq_ignore_ascii_case(h))
        || n.eq_ignore_ascii_case("content-length")
        || n.eq_ignore_ascii_case("accept-encoding")
        || n.starts_with("x-tbs-")
}

pub fn build_upstream_request(
    method: &hyper::Method,
    base: &hyper::Uri,
    path_and_query: &str,
    headers: &HeaderMap,
    body: Bytes,
) -> Result<Request<Full<Bytes>>, ProxyError> {
    let uri = join_uri(base, path_and_query)?;
    let mut builder = Request::builder().method(method.clone()).uri(uri);
    {
        let out = builder
            .headers_mut()
            .ok_or(ProxyError::BadRequest("tbs_header_build_failed"))?;
        for (name, value) in headers.iter() {
            if header_allowed(name) {
                out.append(name.clone(), value.clone());
            }
        }
        out.insert(CONTENT_LENGTH, HeaderValue::from(body.len()));
        out.insert(ACCEPT_ENCODING, HeaderValue::from_static("identity"));
    }
    builder
        .body(Full::new(body))
        .map_err(|_| ProxyError::BadRequest("tbs_request_build_failed"))
}

/// Strip hop-by-hop headers; content-length is re-framed by the streaming writer.
pub fn filter_response_headers(headers: &HeaderMap) -> HeaderMap {
    let mut out = HeaderMap::new();
    for (name, value) in headers.iter() {
        let n = name.as_str();
        if HOP_BY_HOP.iter().any(|h| n.eq_ignore_ascii_case(h))
            || n.eq_ignore_ascii_case("content-length")
        {
            continue;
        }
        out.append(name.clone(), value.clone());
    }
    out
}

pub fn boxed_full(bytes: Bytes) -> BoxBody<Bytes, hyper::Error> {
    Full::new(bytes).map_err(|never| match never {}).boxed()
}
