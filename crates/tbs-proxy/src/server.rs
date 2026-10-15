//! Accept loop: one Tokio task per connection, HTTP/1.1 only (Phase 0).

use std::sync::Arc;

use hyper::service::service_fn;
use hyper_util::rt::TokioIo;
use tokio::net::TcpListener;
use tracing::{error, info};

use crate::pipeline;
use crate::state::AppState;

pub async fn serve(
    state: Arc<AppState>,
    listener: TcpListener,
    shutdown: impl std::future::Future<Output = ()>,
) -> std::io::Result<()> {
    tokio::pin!(shutdown);
    loop {
        tokio::select! {
            _ = &mut shutdown => {
                info!("listener closed");
                return Ok(());
            }
            accepted = listener.accept() => {
                let (stream, peer) = accepted?;
                let state = Arc::clone(&state);
                tokio::spawn(async move {
                    let io = TokioIo::new(stream);
                    let svc = service_fn(move |req| {
                        let state = Arc::clone(&state);
                        async move { pipeline::handle(state, req).await }
                    });
                    if let Err(err) = hyper::server::conn::http1::Builder::new()
                        .serve_connection(io, svc)
                        .await
                        && !err.is_incomplete_message() {
                            error!(%peer, "connection error: {err}");
                        }
                });
            }
        }
    }
}
