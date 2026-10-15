//! End-to-end: tbs-proxy binary against an in-process mock upstream.
//!
//! Covers JSON passthrough, SSE passthrough, usage capture, ledger events,
//! receipt rendering, the health endpoint, and the request-size limit.

use std::net::SocketAddr;
use std::process::{Command, Stdio};
use std::time::Duration;

use bytes::Bytes;
use http_body_util::{BodyExt, Full};
use hyper::body::Incoming;
use hyper::server::conn::http1;
use hyper::service::service_fn;
use hyper::{Request, Response, StatusCode, header::CONTENT_TYPE};
use hyper_util::client::legacy::Client;
use hyper_util::rt::{TokioExecutor, TokioIo};
use tokio::net::{TcpListener, TcpStream};

const JSON_BODY: &str = r#"{"id":"chatcmpl-1","object":"chat.completion","choices":[{"index":0,"message":{"role":"assistant","content":"Hi there"},"finish_reason":"stop"}],"usage":{"prompt_tokens":42,"completion_tokens":7,"total_tokens":49}}"#;

const SSE_BODY: &str = "data: {\"choices\":[{\"delta\":{\"content\":\"Hello \"}}]}\n\n\
data: {\"choices\":[{\"delta\":{\"content\":\"streaming world\"}}]}\n\n\
data: {\"usage\":{\"prompt_tokens\":11,\"completion_tokens\":4}}\n\n\
data: [DONE]\n\n";

async fn spawn_mock_upstream() -> SocketAddr {
    let listener = TcpListener::bind("127.0.0.1:0").await.expect("bind mock");
    let addr = listener.local_addr().expect("mock addr");
    tokio::spawn(async move {
        loop {
            let Ok((stream, _)) = listener.accept().await else {
                break;
            };
            tokio::spawn(async move {
                let io = TokioIo::new(stream);
                let svc = service_fn(|req: Request<Incoming>| async move {
                    let body = req.collect().await.expect("mock read").to_bytes();
                    let text = String::from_utf8_lossy(&body);
                    let sse = text.contains("\"stream\":true");
                    let payload = if sse { SSE_BODY } else { JSON_BODY };
                    let mut builder = Response::builder().status(StatusCode::OK);
                    if sse {
                        builder = builder.header(CONTENT_TYPE, "text/event-stream");
                    } else {
                        builder = builder.header(CONTENT_TYPE, "application/json");
                    }
                    Ok::<_, std::convert::Infallible>(
                        builder
                            .body(Full::new(Bytes::from(payload)))
                            .expect("mock response"),
                    )
                });
                let _ = http1::Builder::new().serve_connection(io, svc).await;
            });
        }
    });
    addr
}

fn free_port() -> u16 {
    std::net::TcpListener::bind("127.0.0.1:0")
        .expect("free port")
        .local_addr()
        .expect("addr")
        .port()
}

async fn wait_for_port(port: u16) {
    for _ in 0..200 {
        if TcpStream::connect(("127.0.0.1", port)).await.is_ok() {
            return;
        }
        tokio::time::sleep(Duration::from_millis(25)).await;
    }
    panic!("proxy did not come up on port {port}");
}

fn spawn_proxy(port: u16, mock: SocketAddr, events: &std::path::Path, extra: &[&str]) {
    let mut cmd = Command::new(env!("CARGO_BIN_EXE_tbs-proxy"));
    cmd.args([
        "serve",
        "--listen",
        &format!("127.0.0.1:{port}"),
        "--upstream",
        &format!("http://{mock}"),
        "--events",
        events.to_str().expect("events path"),
        "--tenant",
        "org_e2e",
        "--project",
        "proj_e2e",
    ])
    .args(extra)
    .stdout(Stdio::null())
    .stderr(Stdio::null());
    let child = cmd.spawn().expect("spawn proxy");
    std::mem::forget(child);
}

fn chat_request(stream: bool) -> Bytes {
    Bytes::from(format!(
        r#"{{"model":"gpt-4o-mini","messages":[{{"role":"user","content":"Say hi"}}],"stream":{stream}}}"#
    ))
}

async fn post_chat(port: u16) -> Response<Incoming> {
    let client: Client<_, Full<Bytes>> = Client::builder(TokioExecutor::new()).build_http();
    let req = Request::post(format!("http://127.0.0.1:{port}/v1/chat/completions"))
        .header(CONTENT_TYPE, "application/json")
        .body(Full::new(chat_request(false)))
        .expect("request");
    client.request(req).await.expect("proxy response")
}

async fn wait_for_events(path: &std::path::Path, min_lines: usize) -> Vec<String> {
    for _ in 0..200 {
        if let Ok(text) = std::fs::read_to_string(path) {
            let lines: Vec<String> = text
                .lines()
                .filter(|l| !l.trim().is_empty())
                .map(str::to_string)
                .collect();
            if lines.len() >= min_lines {
                return lines;
            }
        }
        std::thread::sleep(Duration::from_millis(25));
    }
    panic!("events file never reached {min_lines} lines");
}

#[tokio::test]
async fn json_round_trip_writes_reported_usage_event() {
    let mock = spawn_mock_upstream().await;
    let dir = tempfile::tempdir().expect("tempdir");
    let events = dir.path().join("events.jsonl");
    let port = free_port();
    spawn_proxy(port, mock, &events, &[]);
    wait_for_port(port).await;

    let resp = post_chat(port).await;
    assert_eq!(resp.status(), StatusCode::OK);
    assert_eq!(
        resp.headers().get("x-tbs-mode").expect("mode header"),
        "observe"
    );
    assert!(resp.headers().get("x-tbs-request-id").is_some());
    let body = resp.into_body().collect().await.expect("body").to_bytes();
    assert_eq!(body, Bytes::from(JSON_BODY), "bytes must pass through");

    let lines = wait_for_events(&events, 1).await;
    let ev: serde_json::Value = serde_json::from_str(&lines[0]).expect("event json");
    assert_eq!(ev["kind"], "actual");
    assert_eq!(ev["actual"]["input_tokens"], 42);
    assert_eq!(ev["actual"]["output_tokens"], 7);
    assert_eq!(ev["actual"]["usage_source"], "reported");
    assert_eq!(ev["tenant_id"], "org_e2e");
    assert!(ev["event_id"].as_str().expect("id").starts_with("sha256:"));

    let receipt = Command::new(env!("CARGO_BIN_EXE_tbs-proxy"))
        .args([
            "receipt",
            "--events",
            events.to_str().expect("path"),
            "--tenant",
            "org_e2e",
            "--project",
            "proj_e2e",
        ])
        .output()
        .expect("receipt");
    assert!(receipt.status.success());
    let text = String::from_utf8_lossy(&receipt.stdout);
    assert!(text.contains("actual"), "receipt: {text}");
}

#[tokio::test]
async fn sse_bytes_pass_through_and_usage_is_captured() {
    let mock = spawn_mock_upstream().await;
    let dir = tempfile::tempdir().expect("tempdir");
    let events = dir.path().join("events.jsonl");
    let port = free_port();
    spawn_proxy(port, mock, &events, &[]);
    wait_for_port(port).await;

    let client: Client<_, Full<Bytes>> = Client::builder(TokioExecutor::new()).build_http();
    let req = Request::post(format!("http://127.0.0.1:{port}/v1/chat/completions"))
        .header(CONTENT_TYPE, "application/json")
        .body(Full::new(chat_request(true)))
        .expect("request");
    let resp = client.request(req).await.expect("proxy response");
    assert_eq!(resp.status(), StatusCode::OK);
    assert_eq!(
        resp.headers().get(CONTENT_TYPE).expect("content-type"),
        "text/event-stream"
    );
    let body = resp.into_body().collect().await.expect("body").to_bytes();
    assert_eq!(body, Bytes::from(SSE_BODY), "sse bytes must pass through");

    let lines = wait_for_events(&events, 1).await;
    let ev: serde_json::Value = serde_json::from_str(&lines[0]).expect("event json");
    assert_eq!(ev["actual"]["input_tokens"], 11);
    assert_eq!(ev["actual"]["output_tokens"], 4);
    assert_eq!(ev["actual"]["usage_source"], "reported");
    assert!(ev["verdict_scores"].is_object());
}

#[tokio::test]
async fn oversized_request_is_rejected_with_413() {
    let mock = spawn_mock_upstream().await;
    let dir = tempfile::tempdir().expect("tempdir");
    let events = dir.path().join("events.jsonl");
    let port = free_port();
    spawn_proxy(port, mock, &events, &["--max-request-mib", "1"]);
    wait_for_port(port).await;

    let big = Bytes::from(vec![b'a'; 1_200_000]);
    let client: Client<_, Full<Bytes>> = Client::builder(TokioExecutor::new()).build_http();
    let req = Request::post(format!("http://127.0.0.1:{port}/v1/chat/completions"))
        .header(CONTENT_TYPE, "application/json")
        .body(Full::new(big))
        .expect("request");
    let resp = client.request(req).await.expect("proxy response");
    assert_eq!(resp.status(), StatusCode::PAYLOAD_TOO_LARGE);
}

#[tokio::test]
async fn health_endpoint_reports_observe_mode() {
    let mock = spawn_mock_upstream().await;
    let dir = tempfile::tempdir().expect("tempdir");
    let events = dir.path().join("events.jsonl");
    let port = free_port();
    spawn_proxy(port, mock, &events, &[]);
    wait_for_port(port).await;

    let client: Client<_, Full<Bytes>> = Client::builder(TokioExecutor::new()).build_http();
    let req = Request::get(format!("http://127.0.0.1:{port}/tbs/health"))
        .body(Full::new(Bytes::new()))
        .expect("request");
    let resp = client.request(req).await.expect("proxy response");
    assert_eq!(resp.status(), StatusCode::OK);
    let body = resp.into_body().collect().await.expect("body").to_bytes();
    let v: serde_json::Value = serde_json::from_slice(&body).expect("json");
    assert_eq!(v["status"], "ok");
    assert_eq!(v["mode"], "observe");
    assert_eq!(v["guard_trips"], 0);
}
