//! `cyberbrain serve`: the HTTP API at `/api/v1` and the embedded web UI (SPEC §8.1, §13).
//!
//! A thin adapter over [`App`], like every other front end (SPEC §8.2). Nothing here
//! reaches around it: every route is one or two `App` calls plus the reshaping that
//! `ui/src/api/types.ts` asks for.
//!
//! The rules this module holds, and where:
//!
//! - **Loopback only, not configurable** ([`serve`]). There is no authentication because
//!   there is no remote access to authenticate; the two facts are tied together by the
//!   absence of any bind-address parameter.
//! - **One failure taxonomy** ([`error`]): `exit_code` on the wire is
//!   `cyberbrain_core::Error::exit_code()`.
//! - **The two typed outcomes are not errors** ([`notes`]): a held write and a stale
//!   `expected_updated` are 409s carrying the findings and the hold id, so the UI can offer
//!   the operator their choices.
//! - **`?dry_run=true` swaps the writers, not the path**: every mutating route passes it
//!   straight into `App`, which is where the no-op writers live.
//! - **CSP as a header** ([`assets`]), including `frame-ancestors 'none'`, on every
//!   response — API and asset alike.

mod assets;
pub(crate) mod command;
pub(crate) mod error;
mod extract;
mod holds;
mod notes;
mod ops;
mod origin;
mod policy;
mod wire;

#[cfg(test)]
mod tests;

use crate::app::App;
use axum::Router;
use axum::http::{HeaderValue, header};
use axum::routing::{get, post};
use cyberbrain_core::{Error, Result};
use error::{ApiError, ApiResult};
use holds::Holds;
use std::net::{Ipv4Addr, SocketAddr};
use std::path::PathBuf;
use std::sync::{Arc, Mutex};
use tower_http::set_header::SetResponseHeaderLayer;

/// When the last scans ran through this process. `App` keeps no such timestamp; this is
/// the honest scope of what `status.index.last_scan` can say.
#[derive(Debug, Default, Clone, Copy)]
pub struct ScanTimes {
    pub last_scan: Option<jiff::Timestamp>,
    pub last_full_scan: Option<jiff::Timestamp>,
}

pub struct ServeState {
    pub app: Arc<App>,
    pub holds: Holds,
    pub scans: Mutex<ScanTimes>,
    /// The binary a typed command is run as. State rather than `current_exe()` at the point
    /// of use, because in a test `current_exe()` is the test harness, and an endpoint whose
    /// only untested path is the one that starts a process is an endpoint nobody has tried.
    pub self_exe: PathBuf,
    /// `Some` only when this run was started with `--terminal`. Its absence is the first of
    /// the three conditions in `crate::terminal`.
    pub terminal: Option<crate::terminal::TerminalConfig>,
    /// The origins a terminal handshake may carry. The page's own, and nothing else.
    pub origins: Vec<String>,
}

/// Run a synchronous `App` call off the async runtime's threads.
pub(crate) async fn blocking<T, F>(f: F) -> ApiResult<T>
where
    T: Send + 'static,
    F: FnOnce() -> ApiResult<T> + Send + 'static,
{
    tokio::task::spawn_blocking(f)
        .await
        .map_err(|e| ApiError::internal(format!("request worker failed: {e}")))?
}

async fn no_store(
    req: axum::extract::Request,
    next: axum::middleware::Next,
) -> axum::response::Response {
    let mut resp = next.run(req).await;
    resp.headers_mut()
        .entry(header::CACHE_CONTROL)
        .or_insert(HeaderValue::from_static("no-store"));
    resp
}

/// The router, for [`serve`] and for in-process tests: the binary that a typed command
/// re-runs, and the terminal settings when this run has one.
pub fn router_with(
    app: Arc<App>,
    self_exe: PathBuf,
    terminal: Option<crate::terminal::TerminalConfig>,
    origins: Vec<String>,
) -> Router {
    let state = Arc::new(ServeState {
        app,
        holds: Holds::new(),
        scans: Mutex::new(ScanTimes::default()),
        self_exe,
        terminal,
        origins: origins.clone(),
    });
    let ours = Arc::new(origins);
    let api = Router::new()
        .route("/status", get(ops::status))
        .route("/hub", get(ops::hub_status))
        .route("/recall", get(ops::recall))
        .route("/recall/{citation}", get(ops::expand))
        .route("/notes", get(notes::list_notes).post(notes::post_note))
        .route(
            "/notes/{target}",
            get(notes::get_note)
                .put(notes::put_note)
                .delete(notes::delete_note),
        )
        .route("/holds/{id}", post(notes::resolve_hold))
        .route("/graph", get(notes::graph))
        .route("/policy/egress", get(policy::egress))
        .route("/policy/obligations", get(policy::obligations))
        .route("/policy/audit", get(policy::audit))
        .route("/policy/pii", get(policy::pii))
        .route("/policy/retention", get(policy::retention))
        .route("/policy/retention/apply", post(policy::retention_apply))
        .route("/policy/model-card", get(policy::model_cards))
        .route("/policy/subject", get(policy::subject))
        .route("/usage", get(ops::usage))
        .route("/doctor", get(ops::doctor))
        .route("/scan", post(ops::scan))
        .route("/command", post(command::run))
        .route("/terminal", get(crate::terminal::open))
        .route(
            "/terminal/profiles",
            get(crate::terminal::list_profiles).put(crate::terminal::put_profiles),
        )
        .layer(axum::middleware::from_fn(no_store))
        .with_state(state);
    Router::new()
        .nest("/api/v1", api)
        .fallback(assets::fallback)
        // Outermost, so a request that may not change this store is refused before any
        // handler reads its body — and so the rule is one rule rather than one per route.
        .layer(axum::middleware::from_fn(
            move |req: axum::extract::Request, next: axum::middleware::Next| {
                let ours = ours.clone();
                async move { origin::guard(ours, req, next).await }
            },
        ))
        .layer(SetResponseHeaderLayer::overriding(
            header::CONTENT_SECURITY_POLICY,
            assets::csp_header(),
        ))
        .layer(SetResponseHeaderLayer::overriding(
            header::X_CONTENT_TYPE_OPTIONS,
            HeaderValue::from_static("nosniff"),
        ))
        .layer(SetResponseHeaderLayer::overriding(
            header::REFERRER_POLICY,
            HeaderValue::from_static("no-referrer"),
        ))
}

/// Bind `127.0.0.1:port` and serve until the process ends. The address is not a
/// parameter on purpose (SPEC §8.2): opening the bind without adding authentication is
/// the accident this signature prevents.
///
/// `open` belongs here rather than in the caller for one reason: with `--port 0` the
/// address does not exist until the bind returns, and the caller has nothing to open.
/// Printed on stdout when this build carries no web page, before the address line.
///
/// Matched by the desktop launcher, so the wording is part of the interface between the two
/// programs rather than a message. `no_page_marker_is_a_promise` in the tests says so.
pub const NO_PAGE_MARKER: &str = "cyberbrain serve: no web page in this build";

pub async fn serve(app: Arc<App>, port: u16, open: bool, terminal: bool) -> Result<()> {
    let addr = SocketAddr::from((Ipv4Addr::LOCALHOST, port));
    let listener = tokio::net::TcpListener::bind(addr)
        .await
        .map_err(|e| Error::Config(format!("cannot bind {addr}: {e}")))?;
    let bound = listener
        .local_addr()
        .map_err(|e| Error::Config(format!("cannot read the bound address: {e}")))?;
    if !assets::bundle_present() {
        // On stdout, because the desktop launcher reads this stream and matches on it. Its
        // whole job is to open the page; without one, the browser gets a JSON error where a
        // program should be. The address line below is already a contract between the two
        // binaries — this is the same contract saying there is nothing here to open.
        println!("{NO_PAGE_MARKER}");
        eprintln!(
            "cyberbrain serve: the web UI bundle is not embedded (ui/dist was missing at build time); the API works, the page will 404"
        );
    }
    if assets::meta_csp().is_none() {
        eprintln!(
            "cyberbrain serve: the built page carries no CSP <meta>; sending a strict fallback header, the inline theme bootstrap will be blocked"
        );
    }
    let terminal = terminal.then(crate::terminal::TerminalConfig::new);
    // Both spellings of the same place: a browser sends whichever the address bar holds.
    let origins = vec![
        format!("http://127.0.0.1:{}", bound.port()),
        format!("http://localhost:{}", bound.port()),
    ];

    // The token rides in the fragment, which a browser keeps to itself: it is in no request
    // line, so it reaches no log of ours and no proxy's. That is the whole reason it is
    // there rather than in a query parameter.
    let url = match &terminal {
        // `#/?t=…` and not `#/terminals?t=…`: the page takes the token, remembers it and
        // strips it from the address, then shows what it always shows. Landing somebody in
        // a terminal because they asked for one to be available is not the same thing.
        Some(t) => format!("http://{bound}/#/?t={}", t.token),
        None => format!("http://{bound}/"),
    };
    println!(
        "cyberbrain serve: http://{bound}/  (loopback only, no authentication; API at /api/v1)"
    );
    if terminal.is_some() {
        println!(
            "cyberbrain serve: terminal enabled. Open this address and nothing else — the \
             token in it is what opens a terminal, and it is new every run:\n{url}"
        );
    }
    if open {
        open_browser(&url);
    }
    axum::serve(
        listener,
        router_with(
            app,
            std::env::current_exe().unwrap_or_default(),
            terminal,
            origins,
        ),
    )
    .await
    .map_err(|e| Error::Config(format!("serve: {e}")))
}

/// Hand the address to whatever the machine uses to open things.
///
/// Best effort by design: on a headless server there is nothing to open, and that is not a
/// reason to fail a command whose job is to serve. It still says so, because a branch that
/// fails silently is how `--no-open` came to mean nothing in the first place.
fn open_browser(url: &str) {
    #[cfg(target_os = "windows")]
    let result = std::process::Command::new("cmd")
        .args(["/c", "start", "", url])
        .spawn();
    #[cfg(target_os = "macos")]
    let result = std::process::Command::new("open").arg(url).spawn();
    #[cfg(not(any(target_os = "windows", target_os = "macos")))]
    let result = std::process::Command::new("xdg-open").arg(url).spawn();

    if let Err(e) = result {
        eprintln!(
            "cyberbrain serve: could not open a browser ({e}); open the address above yourself"
        );
    }
}
