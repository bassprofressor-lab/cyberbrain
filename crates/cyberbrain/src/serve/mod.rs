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
//! - **This account only** ([`peer`]): on Linux a connection opened by another account on
//!   the machine is dropped before it is read, because loopback is shared by every user of
//!   the machine and the API would otherwise be theirs too (2026-10-25).
//! - **Only under its own name** ([`origin::host_guard`]): a `Host` other than
//!   `127.0.0.1:<port>`, `localhost:<port>` or `[::1]:<port>` is refused with 421 on every
//!   route. Loopback alone does not stop a browser that was pointed here by DNS rebinding.
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
pub(crate) mod origin;
mod peer;
mod policy;
mod proposals;
mod transcribe;
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
    /// Environment changes for the processes `POST /command` starts (`None` removes the
    /// variable). Empty in a real run: the child inherits this server's environment. Tests set
    /// it so a child is the operator or an agent on purpose, not by whoever ran the suite
    /// (F5, audit 2026-11-04: run from Claude Code, an inherited `CLAUDECODE` hid H1).
    pub child_env: Vec<(String, Option<String>)>,
    /// Commands running from this window now (`command::MAX_RUNNING`).
    pub running: Arc<std::sync::atomic::AtomicUsize>,
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
    router_with_child_env(app, self_exe, terminal, origins, Vec::new())
}

/// [`router_with`], with the environment changes its command line's children get.
pub fn router_with_child_env(
    app: Arc<App>,
    self_exe: PathBuf,
    terminal: Option<crate::terminal::TerminalConfig>,
    origins: Vec<String>,
    child_env: Vec<(String, Option<String>)>,
) -> Router {
    let state = Arc::new(ServeState {
        app,
        holds: Holds::new(),
        scans: Mutex::new(ScanTimes::default()),
        self_exe,
        terminal,
        origins: origins.clone(),
        child_env,
        running: Arc::default(),
    });
    // The recording is the one body here measured in megabytes rather than kilobytes.
    let upload_limit = usize::try_from(
        state
            .app
            .config()
            .transcription
            .max_upload_mb
            .saturating_mul(1024 * 1024),
    )
    .unwrap_or(usize::MAX);
    let hosts = Arc::new(origin::hosts_of(&origins));
    let ours = Arc::new(origins);
    let api = Router::new()
        .route("/status", get(ops::status))
        .route("/hub", get(ops::hub_status))
        .route("/canaries", get(ops::canaries))
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
        .route("/proposals", get(proposals::list))
        .route("/proposals/history", get(proposals::history))
        .route("/proposals/{name}/decision", post(proposals::decide))
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
        .route("/transcription", get(transcribe::status))
        .route(
            "/transcribe",
            post(transcribe::dictate).layer(axum::extract::DefaultBodyLimit::max(upload_limit)),
        )
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
        // Outside the origin rule, and around the page as well as the API (2026-09-22): a
        // request under somebody else's name is DNS rebinding, and there the origin rule sees
        // a same-origin request and a read sees its own answer. See `origin.rs`.
        .layer(axum::middleware::from_fn(
            move |req: axum::extract::Request, next: axum::middleware::Next| {
                let hosts = hosts.clone();
                async move { origin::host_guard(hosts, req, next).await }
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
    // The URL stays first on this line and is followed by whitespace: the desktop launcher
    // takes the first `http://` up to the next space.
    println!(
        "cyberbrain serve: http://{bound}/  (loopback only, {}; API at /api/v1)",
        if peer::ENFORCED {
            "this account only"
        } else {
            "no authentication"
        }
    );
    if terminal.is_some() {
        // Not on stdout (S20): stdout is what a service manager's journal or a redirected log
        // keeps, readable by whoever reads the log, long after this run. The file is this
        // account's alone from the moment it exists.
        let file = write_terminal_address(&url, bound.port())?;
        println!(
            "cyberbrain serve: terminal enabled. The address that opens it carries this run's \
             token, so it is not printed here; it is in a file only this account can read. \
             Open that address yourself, and nothing else — it is new every run and does \
             not belong in a ticket or a log."
        );
        println!("{TERMINAL_ADDRESS_MARKER}{}", file.display());
        println!(
            "cyberbrain serve: not opening a browser for you, because the address would go \
             into that browser's command line, where every account on this machine can read \
             it."
        );
    }
    if should_open(open, terminal.is_some()) {
        open_browser(&url);
    }
    axum::serve(
        peer::own_user_only(listener),
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

/// Printed on stdout, followed by the path of the file holding the address with the terminal
/// token. Matched by the desktop launcher, which reads the file, so the wording is part of
/// the interface between the two programs rather than a message.
pub const TERMINAL_ADDRESS_MARKER: &str = "cyberbrain serve: terminal address in ";

/// Where the terminal address files go: this account's runtime directory where there is one
/// (`$XDG_RUNTIME_DIR`, which the system makes per user and clears at logout), otherwise
/// beside the other per-user state, `%LOCALAPPDATA%` on Windows.
fn terminal_address_dir() -> Result<PathBuf> {
    let base = if cfg!(windows) {
        std::env::var_os("LOCALAPPDATA").map(|d| PathBuf::from(d).join("cyberbrain"))
    } else {
        std::env::var_os("XDG_RUNTIME_DIR")
            .filter(|d| !d.is_empty())
            .map(|d| PathBuf::from(d).join("cyberbrain"))
            .or_else(|| {
                std::env::var_os("XDG_STATE_HOME")
                    .filter(|d| !d.is_empty())
                    .map(PathBuf::from)
                    .or_else(|| {
                        std::env::var_os("HOME")
                            .map(|h| PathBuf::from(h).join(".local").join("state"))
                    })
                    .map(|d| d.join("cyberbrain"))
            })
    };
    base.map(|b| b.join("serve")).ok_or_else(|| {
        Error::Config(
            "serve --terminal: there is no per-user directory to put the terminal address in \
             (neither XDG_RUNTIME_DIR nor HOME, or LOCALAPPDATA on Windows, is set)"
                .into(),
        )
    })
}

/// Write the address with the token to a file of its own, readable by this account only, and
/// return where. The modes are set as the directory and the file are created, so there is no
/// moment at which another account could open either.
fn write_terminal_address(url: &str, port: u16) -> Result<PathBuf> {
    let dir = terminal_address_dir()?;
    let io = |path: &std::path::Path| {
        let path = path.to_path_buf();
        move |source| Error::Io { path, source }
    };
    let mut builder = std::fs::DirBuilder::new();
    builder.recursive(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::DirBuilderExt;
        builder.mode(0o700);
    }
    builder.create(&dir).map_err(io(&dir))?;
    // A stale file from an earlier run on this port, and files nobody collected for a day: a
    // run started by the desktop launcher has its file removed once it is read, one started
    // at a prompt leaves it behind, holding a token that died with that run.
    if let Ok(entries) = std::fs::read_dir(&dir) {
        let day = std::time::Duration::from_secs(24 * 60 * 60);
        for e in entries.flatten() {
            let old = e
                .metadata()
                .and_then(|m| m.modified())
                .ok()
                .and_then(|t| t.elapsed().ok())
                .is_some_and(|age| age > day);
            let name = e.file_name();
            let name = name.to_string_lossy();
            if name.starts_with("terminal-") && name.ends_with(".url") && old {
                let _ = std::fs::remove_file(e.path());
            }
        }
    }
    let file = dir.join(format!("terminal-{port}.url"));
    let _ = std::fs::remove_file(&file);
    let mut opts = std::fs::OpenOptions::new();
    opts.write(true).create_new(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        opts.mode(0o600);
    }
    let mut f = opts.open(&file).map_err(io(&file))?;
    std::io::Write::write_all(&mut f, format!("{url}\n").as_bytes()).map_err(io(&file))?;
    f.sync_all().map_err(io(&file))?;
    Ok(file)
}

/// Whether to hand the address to a browser, or leave it to the person.
///
/// Not when there is a terminal. Opening a browser means passing the address as an argument
/// to `xdg-open` or `open`, and from there it becomes part of the browser's own command
/// line — which on Linux any account on the machine can read out of `/proc`, for as long as
/// the browser runs. The address carries the terminal token, and the token is the whole of
/// what stands between another user of this machine and a shell as you.
///
/// The desktop launcher is unaffected and keeps opening things: it navigates its own window
/// in-process and passes the address to nobody.
fn should_open(asked: bool, terminal: bool) -> bool {
    asked && !terminal
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
