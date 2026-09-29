//! Localhost HTTP API. Claude Code hooks POST their stdin JSON here.

use crate::state::{Core, HookEvent};
use axum::body::Bytes;
use axum::extract::State;
use axum::http::{header, HeaderMap, StatusCode};
use axum::routing::post;
use axum::Router;
use chrono::Local;
use std::sync::Arc;
use tokio::sync::oneshot;

pub const HOOK_PATH: &str = "/claude-hook";

pub struct Server {
    pub port: u16,
    shutdown: Option<oneshot::Sender<()>>,
}

impl Drop for Server {
    fn drop(&mut self) {
        if let Some(tx) = self.shutdown.take() {
            let _ = tx.send(());
        }
    }
}

#[derive(Clone)]
struct Ctx {
    core: Arc<Core>,
    hosts: [String; 2],
}

/// Must run inside a tokio runtime.
pub async fn start(core: Arc<Core>, port: u16) -> Result<Server, String> {
    let listener = tokio::net::TcpListener::bind(("127.0.0.1", port))
        .await
        .map_err(|e| format!("Port {port} is not available for the Claude Code hooks: {e}"))?;
    let ctx = Ctx { core, hosts: [format!("127.0.0.1:{port}"), format!("localhost:{port}")] };
    let router = Router::new().route(HOOK_PATH, post(hook)).with_state(ctx);
    let (tx, rx) = oneshot::channel::<()>();
    tokio::spawn(async move {
        let result = axum::serve(listener, router)
            .with_graceful_shutdown(async {
                let _ = rx.await;
            })
            .await;
        if let Err(e) = result {
            eprintln!("[server] {e}");
        }
    });
    eprintln!("[server] listening on 127.0.0.1:{port}");
    Ok(Server { port, shutdown: Some(tx) })
}

async fn hook(State(ctx): State<Ctx>, headers: HeaderMap, body: Bytes) -> StatusCode {
    // The Host check stops DNS rebinding. Browsers cannot send application/json cross-origin
    // without a CORS preflight, which this server never answers.
    let host = headers.get(header::HOST).and_then(|h| h.to_str().ok()).unwrap_or("");
    if !ctx.hosts.iter().any(|h| h == host) {
        return StatusCode::FORBIDDEN;
    }
    let content_type = headers.get(header::CONTENT_TYPE).and_then(|h| h.to_str().ok()).unwrap_or("");
    if !content_type.starts_with("application/json") {
        return StatusCode::UNSUPPORTED_MEDIA_TYPE;
    }
    let Ok(event) = serde_json::from_slice::<HookEvent>(&body) else {
        return StatusCode::BAD_REQUEST;
    };
    match ctx.core.hook(&event, Local::now()) {
        Ok(()) => StatusCode::NO_CONTENT,
        Err(e) => {
            eprintln!("[server] {e}");
            StatusCode::INTERNAL_SERVER_ERROR
        }
    }
}
