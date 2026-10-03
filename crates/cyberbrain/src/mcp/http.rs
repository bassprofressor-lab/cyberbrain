//! `cyberbrain mcp --http <addr>`: the same MCP server over HTTP, for clients that cannot start
//! a process on this machine (C3, 2026-10-03). The case it is for: n8n in a container, whose
//! MCP client speaks the Streamable HTTP transport.
//!
//! What differs from stdio, and why:
//!
//! - **A token per client** ([`super::clients`]). stdio needs none: whoever can start the
//!   process is on the machine. A socket is reachable by everything on its network, so every
//!   request carries `Authorization: Bearer cbm_…` and is served as `agent:mcp:<client>`.
//!   That name is what `[provenance] untrusted_clients` lists, and what the audit log shows.
//! - **Loopback and private addresses only**, judged like an inference endpoint
//!   (`host_is_local`): no TLS here, so nothing that routes beyond the local network. `0.0.0.0`
//!   is refused; name the bridge address (`172.17.0.1`, say).
//! - **No browsers.** A request with an `Origin` header is refused: nothing here is meant for
//!   a page, and refusing it closes DNS rebinding as `serve` does with its host guard.
//! - **Stateless.** Every POST is answered on its own, as the transport allows: no session id,
//!   no server-sent events. The stdio session holds nothing a request needs from the one before.

use super::{Session, jsonrpc};
use crate::app::App;
use axum::Router;
use axum::body::Bytes;
use axum::extract::State;
use axum::http::{HeaderMap, StatusCode, header};
use axum::response::{IntoResponse, Response};
use axum::routing::post;
use cyberbrain_core::{Error, Result};
use cyberbrain_policy::Actor;
use serde_json::Value;
use std::collections::HashMap;
use std::net::SocketAddr;
use std::path::PathBuf;
use std::sync::{Arc, Mutex};

struct HttpState {
    store: Option<PathBuf>,
    /// One `App` per client, opened on its first request: the actor is fixed per `App`.
    apps: Mutex<HashMap<String, Arc<App>>>,
    /// Where the client list is read from; `None` reads the user's file on every request, so
    /// `mcp-client add/remove` take effect without a restart.
    clients: Option<Vec<(String, String)>>,
}

impl HttpState {
    fn app_for(&self, client: &str) -> Result<Arc<App>> {
        if let Some(a) = self.apps.lock().expect("apps lock").get(client) {
            return Ok(a.clone());
        }
        let app = Arc::new(App::open(
            self.store.as_deref(),
            Actor::Agent(format!("mcp:{client}")),
        )?);
        self.apps
            .lock()
            .expect("apps lock")
            .insert(client.to_string(), app.clone());
        Ok(app)
    }
}

/// Refuse an address that is not loopback or private before anything binds.
pub fn check_bind(addr: &SocketAddr) -> Result<()> {
    if cyberbrain_core::config::host_is_local(&addr.ip().to_string()) {
        Ok(())
    } else {
        Err(Error::Config(format!(
            "{addr} is not a loopback or private address. `mcp --http` has no TLS and serves \
             only the local network; name the address a container reaches this host on \
             (e.g. the docker bridge 172.17.0.1)"
        )))
    }
}

pub fn router(store: Option<PathBuf>, clients: Option<Vec<(String, String)>>) -> Router {
    let st = Arc::new(HttpState {
        store,
        apps: Mutex::new(HashMap::new()),
        clients,
    });
    Router::new()
        .route("/mcp", post(post_mcp).get(not_here).delete(not_here))
        .with_state(st)
}

pub async fn serve_http(store: Option<PathBuf>, addr: SocketAddr) -> Result<()> {
    check_bind(&addr)?;
    let listener = tokio::net::TcpListener::bind(addr)
        .await
        .map_err(|e| Error::Config(format!("cannot bind {addr}: {e}")))?;
    let n = super::clients::load().len();
    diag!(
        "serving MCP over HTTP at http://{addr}/mcp; {n} client(s) in {}",
        super::clients::path()
            .map(|p| cyberbrain_core::Slash(&p).to_string())
            .unwrap_or_default()
    );
    axum::serve(listener, router(store, None))
        .await
        .map_err(|e| Error::Config(format!("serving {addr}: {e}")))
}

async fn not_here() -> Response {
    (
        StatusCode::METHOD_NOT_ALLOWED,
        [(header::ALLOW, "POST")],
        "this server answers POST /mcp only; it opens no event stream\n",
    )
        .into_response()
}

async fn post_mcp(State(st): State<Arc<HttpState>>, headers: HeaderMap, body: Bytes) -> Response {
    if headers.contains_key(header::ORIGIN) {
        return (StatusCode::FORBIDDEN, "browsers are not served here\n").into_response();
    }
    let token = headers
        .get(header::AUTHORIZATION)
        .and_then(|v| v.to_str().ok())
        .and_then(|v| v.strip_prefix("Bearer "))
        .map(str::trim)
        .unwrap_or_default();
    let clients = st.clients.clone().unwrap_or_else(super::clients::load);
    let Some(client) = (!token.is_empty())
        .then(|| super::clients::lookup(&clients, token))
        .flatten()
    else {
        return (
            StatusCode::UNAUTHORIZED,
            [(header::WWW_AUTHENTICATE, "Bearer")],
            "a client token is required: `cyberbrain mcp-client add <name>`\n",
        )
            .into_response();
    };
    let opened = {
        let st = st.clone();
        let client = client.clone();
        tokio::task::spawn_blocking(move || st.app_for(&client)).await
    };
    let app = match opened {
        Ok(Ok(a)) => a,
        Ok(Err(e)) => {
            diag!("cannot open the store for client {client}: {e}");
            return (StatusCode::INTERNAL_SERVER_ERROR, "store not available\n").into_response();
        }
        Err(e) => {
            diag!("store open for client {client} panicked: {e}");
            return (StatusCode::INTERNAL_SERVER_ERROR, "store not available\n").into_response();
        }
    };
    let value: Value = match serde_json::from_slice(&body) {
        Ok(v) => v,
        Err(e) => {
            let err = jsonrpc::RpcError::new(jsonrpc::PARSE_ERROR, format!("parse error: {e}"));
            return (
                StatusCode::BAD_REQUEST,
                axum::Json(jsonrpc::error_response(&Value::Null, &err)),
            )
                .into_response();
        }
    };
    // Every request stands alone; the stdio session's flags only shape its stderr notes.
    let mut session = Session {
        app,
        protocol: None,
        initialized: true,
        exit: false,
        warned_before_init: true,
    };
    let answer = match value {
        Value::Array(items) => {
            let mut out = Vec::new();
            for item in items {
                if let Some(r) = session.handle(jsonrpc::classify(item)).await {
                    out.push(r);
                }
            }
            (!out.is_empty()).then_some(Value::Array(out))
        }
        single => session.handle(jsonrpc::classify(single)).await,
    };
    match answer {
        Some(v) => (StatusCode::OK, axum::Json(v)).into_response(),
        // Notifications only: accepted, nothing to say.
        None => StatusCode::ACCEPTED.into_response(),
    }
}
