//! The hub's HTTP surface. Four routes, and three of them are read-only.
//!
//! Deliberately not the store's API: no recall, no retention, nothing that reaches into a
//! store and changes it. What a client may do here is hand over rows, hand over notes for
//! the bereiche it was granted, and say hello.
//!
//! Notes were added on 2026-09-10 and the sentence above was rewritten rather than left to
//! age into a falsehood. What has not changed: nothing here writes into anybody's store.
//! The hub holds what it was given, for the bereiche a device was granted, and rings 0 and
//! 1 are refused at three separate places — the sender, `ingest_notes`, and a CHECK on the
//! table itself.

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
    /// Who is signed in. In memory, so a restart signs everybody out.
    pub sessions: super::admin::Sessions,
    /// The port it is listening on, so the page can suggest an address for invitations.
    pub port: u16,
    /// Where the record lives, so the page can say so and find a licence beside it.
    pub record: std::path::PathBuf,
    /// Whether this surface is encrypted. Not cosmetic: it decides whether a password may
    /// be typed into it from anywhere but the machine itself, and whether the session
    /// cookie is marked `Secure`.
    pub encrypted: bool,
    /// The last thing the page did, shown once on the next render.
    pub flash: Mutex<Option<Result<String, String>>>,
}

pub fn router(state: Arc<HubState>) -> Router {
    Router::new()
        .route("/", get(page))
        .route("/claim", post(claim))
        .route("/login", post(login))
        .route("/logout", get(logout))
        .route("/password", post(change_password))
        .route("/licence", post(install_licence))
        .route("/devices", post(add_device))
        .route("/health", get(health))
        .route("/api/v1/ingest", post(post_ingest))
        .route("/api/v1/notes", post(post_notes))
        .route("/api/v1/erase", post(post_erase))
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

const ELSEWHERE: &str = "This hub has not been set up yet. Open it on the machine it runs \
     on to set the administrator password. Devices deliver to /api/v1/ingest as usual.";

/// Why a password was refused on an unencrypted hub, and what to do instead.
///
/// It names both ways out, because the operator who reads this may not be the one who can
/// take the second: signing in at the machine works today, a certificate is a change to how
/// the hub is started.
const PLAINTEXT: &str = "This hub is not encrypted, so a password typed here would travel \
     across the network in the clear. Sign in on the machine the hub runs on, or start it \
     with --tls-cert and --tls-key and come back over https (docs/HUB.md). Devices go on \
     delivering to /api/v1/ingest either way.";

/// Whether a password may be typed into this hub from where this request came from.
///
/// Encrypted: from anywhere, which is the entire reason the page has a password. Not
/// encrypted: only from the machine itself, where nothing goes over a wire at all.
fn password_may_travel(state: &HubState, from: &std::net::SocketAddr) -> bool {
    state.encrypted || at_the_machine(from)
}

/// What a caller is allowed to see.
enum Who {
    /// Signed in, or at the machine before anybody has claimed it.
    Admin,
    /// Nobody has set a password yet and this caller is at the machine.
    MayClaim,
    /// Not signed in. Show the door.
    Stranger,
    /// Nobody has set a password and this caller is not at the machine.
    TooEarly,
}

fn who(state: &HubState, headers: &HeaderMap, from: &std::net::SocketAddr) -> Who {
    let claimed = {
        match state.hub.lock() {
            Ok(hub) => super::admin::is_claimed(&hub),
            Err(_) => true, // fail towards asking for a password
        }
    };
    if !claimed {
        // Before there is a password, being at the machine is the credential. Whoever is at
        // the console can read the record with any SQLite tool, so this grants nothing that
        // was not already theirs.
        return if at_the_machine(from) {
            Who::MayClaim
        } else {
            Who::TooEarly
        };
    }
    let cookie =
        super::admin::cookie_from(headers.get(header::COOKIE).and_then(|v| v.to_str().ok()));
    match cookie {
        Some(t) if state.sessions.holds(&t, jiff::Timestamp::now()) => Who::Admin,
        _ => Who::Stranger,
    }
}

fn html(body: String) -> Response {
    Html(body).into_response()
}

/// A failed password costs this much time. Not a lockout: locking out the administrator is
/// a way to take a hub away from its own operator. Enough that guessing over a network is
/// hopeless, little enough that a typo is not a punishment.
async fn stumble() {
    tokio::time::sleep(std::time::Duration::from_millis(
        super::admin::FAILURE_DELAY_MS,
    ))
    .await;
}

async fn page(
    State(state): State<Arc<HubState>>,
    headers: HeaderMap,
    ConnectInfo(from): ConnectInfo<std::net::SocketAddr>,
) -> Response {
    match who(&state, &headers, &from) {
        Who::Admin => {}
        Who::MayClaim => return html(super::page::claim_page(None)),
        Who::TooEarly => return (StatusCode::FORBIDDEN, ELSEWHERE).into_response(),
        Who::Stranger => return html(super::page::login_page(None)),
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
        state.encrypted,
        jiff::Timestamp::now(),
        flash,
    );
    Html(super::page::render(&view)).into_response()
}

#[derive(serde::Deserialize)]
pub struct ClaimForm {
    password: String,
    again: String,
}

/// Set the first password, from the machine itself.
async fn claim(
    State(state): State<Arc<HubState>>,
    ConnectInfo(from): ConnectInfo<std::net::SocketAddr>,
    Form(form): Form<ClaimForm>,
) -> Response {
    if !at_the_machine(&from) {
        return (StatusCode::FORBIDDEN, ELSEWHERE).into_response();
    }
    let hub = match state.hub.lock() {
        Ok(h) => h,
        Err(e) => return (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()).into_response(),
    };
    // Checked again here and not only in the browser: a form is a suggestion, and the
    // second field exists so a typo does not lock somebody out of their own hub.
    if super::admin::is_claimed(&hub) {
        return html(super::page::login_page(Some(
            "This hub already has a password.",
        )));
    }
    if form.password != form.again {
        return html(super::page::claim_page(Some("The two did not match.")));
    }
    match super::admin::set_password(&hub, &form.password) {
        Ok(()) => {
            drop(hub);
            let token = state.sessions.open(jiff::Timestamp::now());
            // Straight in, rather than showing the sign-in form to somebody who has just
            // proved who they are twice.
            (
                [(
                    header::SET_COOKIE,
                    super::admin::set_cookie(&token, state.encrypted),
                )],
                Redirect::to("/"),
            )
                .into_response()
        }
        Err(e) => html(super::page::claim_page(Some(&e))),
    }
}

#[derive(serde::Deserialize)]
pub struct LoginForm {
    password: String,
}

async fn login(
    State(state): State<Arc<HubState>>,
    ConnectInfo(from): ConnectInfo<std::net::SocketAddr>,
    Form(form): Form<LoginForm>,
) -> Response {
    // Before the password is looked at, not after: a hub that checks first and refuses
    // afterwards has already been told the password by the time it objects.
    if !password_may_travel(&state, &from) {
        return (StatusCode::FORBIDDEN, PLAINTEXT).into_response();
    }
    let ok = match state.hub.lock() {
        Ok(hub) => super::admin::verify(&hub, &form.password),
        Err(_) => false,
    };
    if !ok {
        stumble().await;
        return html(super::page::login_page(Some("That is not the password.")));
    }
    let token = state.sessions.open(jiff::Timestamp::now());
    (
        [(
            header::SET_COOKIE,
            super::admin::set_cookie(&token, state.encrypted),
        )],
        Redirect::to("/"),
    )
        .into_response()
}

async fn logout(State(state): State<Arc<HubState>>, headers: HeaderMap) -> Response {
    if let Some(t) =
        super::admin::cookie_from(headers.get(header::COOKIE).and_then(|v| v.to_str().ok()))
    {
        state.sessions.close(&t);
    }
    (
        [(
            header::SET_COOKIE,
            super::admin::clear_cookie(state.encrypted),
        )],
        Redirect::to("/"),
    )
        .into_response()
}

#[derive(serde::Deserialize)]
pub struct PasswordForm {
    current: String,
    password: String,
    again: String,
}

async fn change_password(
    State(state): State<Arc<HubState>>,
    headers: HeaderMap,
    ConnectInfo(from): ConnectInfo<std::net::SocketAddr>,
    Form(form): Form<PasswordForm>,
) -> Response {
    if !matches!(who(&state, &headers, &from), Who::Admin) {
        return html(super::page::login_page(None));
    }
    // The form carries the current password and the new one, so the same rule applies here
    // as at sign-in. Reachable only with a session, which over plain text can only have been
    // opened at the machine — belt and braces, and it costs one comparison.
    if !password_may_travel(&state, &from) {
        return (StatusCode::FORBIDDEN, PLAINTEXT).into_response();
    }
    let outcome = {
        let hub = match state.hub.lock() {
            Ok(h) => h,
            Err(e) => return (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()).into_response(),
        };
        // The current one, even though the session already proves who this is: a cookie left
        // open on a shared machine should not be enough to change the password on it.
        if !super::admin::verify(&hub, &form.current) {
            Err("The current password is not right.".to_string())
        } else if form.password != form.again {
            Err("The two new ones did not match.".to_string())
        } else {
            super::admin::set_password(&hub, &form.password)
                .map(|()| "The password has been changed.".to_string())
        }
    };
    if outcome.is_err() {
        stumble().await;
    }
    if let Ok(mut f) = state.flash.lock() {
        *f = Some(outcome);
    }
    Redirect::to("/").into_response()
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
    headers: HeaderMap,
    ConnectInfo(from): ConnectInfo<std::net::SocketAddr>,
    Form(form): Form<LicenceForm>,
) -> Response {
    if !matches!(who(&state, &headers, &from), Who::Admin) {
        return html(super::page::login_page(None));
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
    headers: HeaderMap,
    ConnectInfo(from): ConnectInfo<std::net::SocketAddr>,
    Form(form): Form<DeviceForm>,
) -> Response {
    if !matches!(who(&state, &headers, &from), Who::Admin) {
        return html(super::page::login_page(None));
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

/// Erase one note. Not behind the same setting as delivery: withdrawing content must work
/// even where sharing has since been switched off.
async fn post_erase(
    State(state): State<Arc<HubState>>,
    headers: HeaderMap,
    body: String,
) -> Response {
    let token = bearer(&headers);
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
    match super::erase_note(&mut hub, token.as_deref(), &body, &now) {
        Ok(c) => (StatusCode::OK, Json(json!(c))).into_response(),
        Err(refusal) => {
            let code = match &refusal {
                Refusal::NotAuthorised(_) => StatusCode::UNAUTHORIZED,
                _ => StatusCode::BAD_REQUEST,
            };
            (code, Json(json!({ "error": refusal.to_string() }))).into_response()
        }
    }
}

/// Take a delivery of notes. Same authentication as `post_ingest`, different cargo, and a
/// per-note answer: a batch is not all-or-nothing, so the sender learns which note it
/// should not have offered instead of only that something was wrong.
async fn post_notes(
    State(state): State<Arc<HubState>>,
    headers: HeaderMap,
    body: String,
) -> Response {
    let token = bearer(&headers);
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
    let licence = LicenceState::read(&hub, jiff::Timestamp::now());
    match super::ingest_notes(&mut hub, &licence, token.as_deref(), &body, &now) {
        Ok(a) => (StatusCode::OK, Json(json!(a))).into_response(),
        Err(refusal) => {
            let code = match &refusal {
                Refusal::NotAuthorised(_) => StatusCode::UNAUTHORIZED,
                Refusal::NotCollecting(_) => StatusCode::SERVICE_UNAVAILABLE,
                _ => StatusCode::BAD_REQUEST,
            };
            (code, Json(json!({ "error": refusal.to_string() }))).into_response()
        }
    }
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
    headers: HeaderMap,
    ConnectInfo(from): ConnectInfo<std::net::SocketAddr>,
) -> Response {
    // Device names and when each was last heard from are not row content, but they are still
    // a picture of an organisation, and this had no authentication at all.
    if !matches!(who(&state, &headers, &from), Who::Admin) {
        return (
            StatusCode::UNAUTHORIZED,
            Json(json!({ "error": "sign in at / first" })),
        )
            .into_response();
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
