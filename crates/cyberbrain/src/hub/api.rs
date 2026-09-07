//! The hub's HTTP surface. Four routes, and three of them are read-only.
//!
//! Deliberately not the store's API: no recall, no notes, no retention, nothing that can
//! change a note anywhere. What a client may do here is hand over rows and say hello.

use super::{HubStore, LicenceState, Refusal, ingest};
use axum::extract::{ConnectInfo, Form, State};
use axum::http::{HeaderMap, StatusCode, header};
use axum::response::{Html, Redirect};
use axum::response::{IntoResponse, Response};
use axum::routing::{get, post};
use axum::{Json, Router};
use serde_json::json;
use std::sync::{Arc, Mutex};

pub struct HubState {
    pub hub: Mutex<HubStore>,
    /// The port it is listening on, so the page can suggest an address for invitations.
    pub port: u16,
    /// Where the record lives, so the page can say so and find a licence beside it.
    pub record: std::path::PathBuf,
    /// The last thing the page did, shown once on the next render.
    pub flash: Mutex<Option<Result<String, String>>>,
}

pub fn router(state: Arc<HubState>) -> Router {
    Router::new()
        .route("/", get(page))
        .route("/licence", post(install_licence))
        .route("/devices", post(add_device))
        .route("/health", get(health))
        .route("/api/v1/ingest", post(post_ingest))
        .route("/api/v1/fleet", get(get_fleet))
        .with_state(state)
}

/// Whether this request came from the machine the hub runs on.
///
/// The page shows who is on the network and can install a licence, and the hub binds an
/// address the whole network can reach. Rather than invent a sign-in for this slice, the
/// rule is that you have to be at the machine: a rule with an obvious shape, which cannot be
/// misconfigured. A networked view can come later behind the admin role that already exists.
pub(crate) fn at_the_machine(who: &std::net::SocketAddr) -> bool {
    who.ip().is_loopback()
}

const ELSEWHERE: &str = "This page is shown only on the machine the hub runs on. \
     Open http://localhost:7788/ there. Devices deliver to /api/v1/ingest as usual.";

async fn page(
    State(state): State<Arc<HubState>>,
    ConnectInfo(who): ConnectInfo<std::net::SocketAddr>,
) -> Response {
    if !at_the_machine(&who) {
        return (StatusCode::FORBIDDEN, ELSEWHERE).into_response();
    }
    let hub = match state.hub.lock() {
        Ok(h) => h,
        Err(e) => {
            return (
                StatusCode::INTERNAL_SERVER_ERROR,
                format!("hub record unavailable: {e}"),
            )
                .into_response();
        }
    };
    // Taken, not read: a message about what just happened should not survive a refresh.
    let flash = state.flash.lock().ok().and_then(|mut f| f.take());
    let view = super::page::View::gather(
        &hub,
        &state.record,
        state.port,
        jiff::Timestamp::now(),
        flash,
    );
    Html(super::page::render(&view)).into_response()
}

#[derive(serde::Deserialize)]
pub struct LicenceForm {
    #[serde(default)]
    text: String,
    #[serde(default)]
    use_found: String,
}

/// Install a licence from the page: the file lying next to the record, or pasted text.
async fn install_licence(
    State(state): State<Arc<HubState>>,
    ConnectInfo(who): ConnectInfo<std::net::SocketAddr>,
    Form(form): Form<LicenceForm>,
) -> Response {
    if !at_the_machine(&who) {
        return (StatusCode::FORBIDDEN, ELSEWHERE).into_response();
    }
    let outcome = {
        let hub = match state.hub.lock() {
            Ok(h) => h,
            Err(e) => return (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()).into_response(),
        };
        if form.use_found.is_empty() {
            install_text(&hub, &form.text)
        } else {
            let dir = state
                .record
                .parent()
                .unwrap_or(std::path::Path::new("."))
                .to_path_buf();
            match super::service::adopt_dropped_licence(&hub, &dir) {
                super::service::Dropped::Installed(m) => Ok(m),
                super::service::Dropped::Unchanged => {
                    Ok("That licence is already installed.".into())
                }
                super::service::Dropped::Problem(m) => Err(m),
                super::service::Dropped::None => Err("The file is no longer there.".into()),
            }
        }
    };
    if let Ok(mut f) = state.flash.lock() {
        *f = Some(outcome);
    }
    // Redirect rather than render, so a refresh does not install anything a second time.
    Redirect::to("/").into_response()
}

#[derive(serde::Deserialize)]
pub struct DeviceForm {
    name: String,
    #[serde(default)]
    hub_url: String,
}

/// Register a machine and write its invitation next to the record.
///
/// Written to a file rather than shown on the page: the token is in it, the file is what the
/// other machine needs, and a token read off a screen gets retyped wrongly. The page says
/// where it went; copying a file is something anybody can do.
async fn add_device(
    State(state): State<Arc<HubState>>,
    ConnectInfo(who): ConnectInfo<std::net::SocketAddr>,
    Form(form): Form<DeviceForm>,
) -> Response {
    if !at_the_machine(&who) {
        return (StatusCode::FORBIDDEN, ELSEWHERE).into_response();
    }
    let outcome = {
        let hub = match state.hub.lock() {
            Ok(h) => h,
            Err(e) => return (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()).into_response(),
        };
        register(&hub, &state.record, form.name.trim(), form.hub_url.trim())
    };
    if let Ok(mut f) = state.flash.lock() {
        *f = Some(outcome);
    }
    Redirect::to("/").into_response()
}

fn register(
    hub: &HubStore,
    record: &std::path::Path,
    name: &str,
    hub_url: &str,
) -> Result<String, String> {
    if name.is_empty() {
        return Err("A machine needs a name.".into());
    }
    // The same seat check the command does, and for the same reason: a device that is
    // allowed to register and then refused every night looks registered and collects
    // nothing, which is the worst of both.
    let state = LicenceState::read(hub, jiff::Timestamp::now());
    match state.seats() {
        None => return Err(format!("{} No device can be registered.", state.line())),
        Some(seats) => {
            let active = hub.active_device_count().map_err(|e| e.to_string())?;
            if active >= seats {
                return Err(format!(
                    "The licence covers {seats} seat(s) and {active} are in use. Revoke a \
                     machine that is gone, or extend the licence — its rows are kept either \
                     way."
                ));
            }
        }
    }

    let (device, token) = hub
        .add_device(name, &jiff::Timestamp::now().to_string())
        .map_err(|e| e.to_string())?;
    let invitation = json!({
        "kind": "cyberbrain.hub.invitation",
        "version": 1,
        "device": device.id,
        "name": device.name,
        "token": token,
        "hub_url": if hub_url.is_empty() { serde_json::Value::Null } else { json!(hub_url) },
        "inference_url": serde_json::Value::Null,
    });
    let text = serde_json::to_string_pretty(&invitation).map_err(|e| e.to_string())?;

    let dir = record
        .parent()
        .unwrap_or(std::path::Path::new("."))
        .join("invitations");
    std::fs::create_dir_all(&dir).map_err(|e| format!("cannot make {}: {e}", dir.display()))?;
    // Named after the device, not after the person's choice of name, so two machines called
    // "laptop" do not overwrite each other's token.
    let path = dir.join(format!("{}.json", device.id));
    std::fs::write(&path, format!("{text}\n"))
        .map_err(|e| format!("cannot write {}: {e}", path.display()))?;

    Ok(format!(
        "{name} registered. Its invitation is at {} — it carries the token, so hand it over \
         the way you would a password and delete it once that machine is set up.{}",
        path.display(),
        if hub_url.is_empty() {
            " No address was given, so the machine will still have to be told where to \
             deliver."
        } else {
            ""
        }
    ))
}

fn install_text(hub: &HubStore, text: &str) -> Result<String, String> {
    let text = text.trim();
    if text.is_empty() {
        return Err("Nothing was pasted.".into());
    }
    let signed = super::licence::parse(text).map_err(|e| e.to_string())?;
    hub.set_licence(text).map_err(|e| e.to_string())?;
    let l = signed.licence();
    Ok(format!(
        "Installed: {}, {} seat(s), until {}.",
        l.customer, l.seats, l.valid_until
    ))
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

    // Read per request, not at startup: a licence that lapses while the service runs has to
    // take effect without somebody remembering to restart it.
    let licence = LicenceState::read(&hub, jiff::Timestamp::now());

    match ingest(
        &mut hub,
        &licence,
        token.as_deref(),
        &body,
        version.as_deref(),
        &now,
    ) {
        Ok(a) => (StatusCode::OK, Json(json!(a))).into_response(),
        Err(refusal) => {
            // The status code carries the difference the sender has to act on: fix your
            // credentials, fix your file, or send what is missing first.
            let code = match &refusal {
                Refusal::NotAuthorised(_) => StatusCode::UNAUTHORIZED,
                Refusal::BadBundle(_) => StatusCode::BAD_REQUEST,
                Refusal::WrongAnchor { .. } => StatusCode::CONFLICT,
                // 503, not 402: the sender did nothing wrong and should try again later,
                // which is exactly what this code tells every retrying client on earth.
                Refusal::NotCollecting(_) => StatusCode::SERVICE_UNAVAILABLE,
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
async fn get_fleet(
    State(state): State<Arc<HubState>>,
    ConnectInfo(who): ConnectInfo<std::net::SocketAddr>,
) -> Response {
    // Device names and when each was last heard from are not row content, but they are still
    // a picture of an organisation, and this had no authentication at all. Same rule as the
    // page until there is a sign-in to put in front of it.
    if !at_the_machine(&who) {
        return (StatusCode::FORBIDDEN, Json(json!({ "error": ELSEWHERE }))).into_response();
    }
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
