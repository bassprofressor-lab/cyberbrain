//! The hub's HTTP surface. Four routes, and three of them are read-only.
//!
//! Deliberately not the store's API: no recall, no notes, no retention, nothing that can
//! change a note anywhere. What a client may do here is hand over rows and say hello.

use super::{HubStore, Refusal, ingest};
use axum::extract::State;
use axum::http::{HeaderMap, StatusCode, header};
use axum::response::{IntoResponse, Response};
use axum::routing::{get, post};
use axum::{Json, Router};
use serde_json::json;
use std::sync::{Arc, Mutex};

pub struct HubState {
    pub hub: Mutex<HubStore>,
}

pub fn router(state: Arc<HubState>) -> Router {
    Router::new()
        .route("/health", get(health))
        .route("/api/v1/ingest", post(post_ingest))
        .route("/api/v1/fleet", get(get_fleet))
        .with_state(state)
}

async fn health() -> impl IntoResponse {
    Json(json!({ "role": "hub", "version": env!("CARGO_PKG_VERSION") }))
}

/// Bearer token, or nothing. Deliberately strict about the scheme rather than accepting a
/// bare token as well: two accepted spellings is two things to get wrong later.
fn bearer(headers: &HeaderMap) -> Option<String> {
    let v = headers.get(header::AUTHORIZATION)?.to_str().ok()?;
    v.strip_prefix("Bearer ").map(|t| t.trim().to_string())
}

/// The client's version, for the fleet view. A header rather than part of the bundle: the
/// bundle is evidence and its shape is fixed, while this is operational chatter.
fn client_version(headers: &HeaderMap) -> Option<String> {
    headers
        .get("x-cyberbrain-version")?
        .to_str()
        .ok()
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty() && s.len() <= 64)
}

async fn post_ingest(
    State(state): State<Arc<HubState>>,
    headers: HeaderMap,
    body: String,
) -> Response {
    let token = bearer(&headers);
    let version = client_version(&headers);
    let now = jiff::Timestamp::now().to_string();

    let mut hub = match state.hub.lock() {
        Ok(h) => h,
        Err(e) => {
            return (
                StatusCode::INTERNAL_SERVER_ERROR,
                Json(json!({ "error": format!("hub record unavailable: {e}") })),
            )
                .into_response();
        }
    };

    match ingest(&mut hub, token.as_deref(), &body, version.as_deref(), &now) {
        Ok(a) => (StatusCode::OK, Json(json!(a))).into_response(),
        Err(refusal) => {
            // The status code carries the difference the sender has to act on: fix your
            // credentials, fix your file, or send what is missing first.
            let code = match &refusal {
                Refusal::NotAuthorised(_) => StatusCode::UNAUTHORIZED,
                Refusal::BadBundle(_) => StatusCode::BAD_REQUEST,
                Refusal::WrongAnchor { .. } => StatusCode::CONFLICT,
            };
            let mut body = json!({ "error": refusal.to_string() });
            if let Refusal::WrongAnchor { expected, got } = &refusal {
                body["expected_anchor"] = json!(expected);
                body["got_anchor"] = json!(got);
            }
            (code, Json(body)).into_response()
        }
    }
}

/// Who is out there, when they were last heard from, and how far their chain has come.
///
/// No row content: this answers "is the fleet reporting", not "what did people do". The
/// second question has its own path, and in the design it needs two people to walk it.
async fn get_fleet(State(state): State<Arc<HubState>>) -> Response {
    let hub = match state.hub.lock() {
        Ok(h) => h,
        Err(e) => {
            return (
                StatusCode::INTERNAL_SERVER_ERROR,
                Json(json!({ "error": format!("hub record unavailable: {e}") })),
            )
                .into_response();
        }
    };
    match hub.devices() {
        Ok(devices) => (StatusCode::OK, Json(json!({ "devices": devices }))).into_response(),
        Err(e) => (
            StatusCode::INTERNAL_SERVER_ERROR,
            Json(json!({ "error": e.to_string() })),
        )
            .into_response(),
    }
}
