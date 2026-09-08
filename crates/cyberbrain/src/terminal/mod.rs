//! A terminal in the window: `GET /api/v1/terminal` over a WebSocket, a pseudo-console
//! behind it, and a program of your choosing inside that.
//!
//! **This is the surface that changed what `serve` is**, and it is guarded accordingly.
//! Every other route here reads or writes this store's notes; this one starts a process.
//! SPEC §8.1 used to say there was no authentication because there was nothing remote to
//! authenticate, and that sentence was true while the worst a local caller could do was
//! write a note. It is not true of a shell, so this route has three conditions and refuses
//! without all of them:
//!
//! 1. **It is off unless asked for.** `cyberbrain serve` has no terminal; `serve --terminal`
//!    does. A store served for reading cannot be talked into starting a process.
//! 2. **A token the caller cannot fetch.** It is minted per run and handed to the page in
//!    the URL *fragment*, which the browser keeps to itself and never sends to a server. A
//!    process that can only speak HTTP to this port cannot read it, and it is not in any
//!    request line, so it does not reach a log.
//! 3. **An `Origin` that is ours, or none at all.** A WebSocket handshake is not subject to
//!    the same-origin rule, so any page in the user's browser could otherwise open one. A
//!    request carrying somebody else's origin is refused. No origin at all is allowed,
//!    because that is a program rather than a page — and a program is what the token is for.
//!
//! **The token is what keeps other users out; the origin check only keeps other pages out.**
//! Saying "no origin means a program running as *this* user" would be wrong: loopback is not
//! per-account, so it means a program running as *any* account on this machine. Admitting it
//! is still right — requiring an origin would refuse every client that is not a browser —
//! but the weight then sits entirely on the token, and everywhere that token can be read is
//! a hole. Two were: the address was handed to a browser as a command-line argument, where
//! `/proc` makes it public to every account, and the saved-connections file was written
//! world-readable. Both closed; the reasoning is written down here because the next such
//! hole will be found by re-reading this paragraph.
//!
//! What none of this claims to stop is the user's own account. A process running as you can
//! start a shell without asking us.

pub mod profiles;
pub mod pty;
#[cfg(test)]
mod tests;

use crate::serve::ServeState;
use crate::serve::error::{ApiError, ApiResult};
use axum::extract::State;
use axum::extract::ws::{Message, WebSocket, WebSocketUpgrade};
use axum::http::{HeaderMap, StatusCode};
use axum::response::{IntoResponse, Response};
use pty::{Pty, Spawn};
use serde::Deserialize;
use std::io::{Read, Write};
use std::path::PathBuf;
use std::sync::Arc;

/// What a run of `serve --terminal` holds: the one token that opens a session.
#[derive(Debug, Clone)]
pub struct TerminalConfig {
    pub token: String,
}

impl TerminalConfig {
    /// A fresh token per run. Never written to the store, never logged, never in a URL the
    /// server sees: it reaches the page in the fragment and comes back in the socket's
    /// first frame.
    pub fn new() -> TerminalConfig {
        let mut bytes = [0u8; 32];
        getrandom::fill(&mut bytes).expect("the system random source");
        TerminalConfig {
            token: bytes.iter().map(|b| format!("{b:02x}")).collect(),
        }
    }
}

/// The first frame the client sends. Nothing happens until it arrives and checks out.
#[derive(Debug, Deserialize)]
pub struct Open {
    pub token: String,
    #[serde(default)]
    pub cols: u16,
    #[serde(default)]
    pub rows: u16,
    /// The command line as typed. Empty or absent means the platform's usual shell.
    ///
    /// A line rather than argv, and split here rather than in the page: the splitting is a
    /// contract (quotes group, a backslash escapes, and nothing else is a shell) and two
    /// implementations of a contract disagree eventually. The page had one for a day; this
    /// is the one that stays.
    #[serde(default)]
    pub command: String,
}

/// Anything the client sends after that.
#[derive(Debug, Deserialize)]
#[serde(tag = "type", rename_all = "kebab-case")]
pub enum Control {
    Resize { cols: u16, rows: u16 },
}

/// Whether this handshake may become a terminal, and why not when it may not.
///
/// Separated from the socket so the decision can be tested without one — it is the part
/// that matters and the part a mistake in is invisible.
pub fn admit(
    configured: Option<&TerminalConfig>,
    origin: Option<&str>,
    expected_origins: &[String],
    token: &str,
) -> Result<(), String> {
    let Some(cfg) = configured else {
        return Err(
            "this store is served without a terminal. Start it with `cyberbrain serve \
             --terminal` if you want one."
                .to_string(),
        );
    };
    // Present and not ours: a page somewhere else in the browser. A WebSocket handshake is
    // not subject to the same-origin rule, so this is the check that stands in for it.
    if let Some(origin) = origin
        && !expected_origins.iter().any(|o| o == origin)
    {
        return Err(format!("a terminal cannot be opened from {origin}"));
    }
    // Constant time, because a token compared byte by byte tells a caller how much of it
    // was right.
    if !constant_time_eq(cfg.token.as_bytes(), token.as_bytes()) {
        return Err("that is not this session's terminal token".to_string());
    }
    Ok(())
}

fn constant_time_eq(a: &[u8], b: &[u8]) -> bool {
    if a.len() != b.len() {
        return false;
    }
    a.iter().zip(b).fold(0u8, |acc, (x, y)| acc | (x ^ y)) == 0
}

/// The token, from the header a browser will not put there for another origin.
///
/// A header rather than a query parameter: a token in a request line reaches every log that
/// records one, and the point of this token is that it does not.
const TOKEN_HEADER: &str = "x-cyberbrain-terminal-token";

fn token_of(headers: &HeaderMap) -> String {
    headers
        .get(TOKEN_HEADER)
        .and_then(|v| v.to_str().ok())
        .unwrap_or_default()
        .to_string()
}

/// Saved command lines. Behind the same token as the terminal itself, because the list says
/// which machines this person connects to and under which account, and that is not something
/// to hand to anything that can reach the port.
pub async fn list_profiles(
    State(st): State<Arc<ServeState>>,
    headers: HeaderMap,
) -> ApiResult<axum::Json<serde_json::Value>> {
    guard(&st, &headers)?;
    let saved = profiles::load().map_err(|e| ApiError::internal(e.to_string()))?;
    Ok(axum::Json(serde_json::json!({
        "path": profiles::path().map(|p| cyberbrain_core::Slash(&p).to_string()),
        "profiles": saved.profiles,
    })))
}

pub async fn put_profiles(
    State(st): State<Arc<ServeState>>,
    headers: HeaderMap,
    axum::Json(saved): axum::Json<profiles::Profiles>,
) -> ApiResult<axum::Json<serde_json::Value>> {
    guard(&st, &headers)?;
    let path = profiles::save(&saved).map_err(|e| ApiError::bad_request(e.to_string()))?;
    Ok(axum::Json(serde_json::json!({
        "path": cyberbrain_core::Slash(&path).to_string(),
        "profiles": saved.profiles,
    })))
}

/// The same two conditions the socket applies, for the two routes beside it. The origin
/// check is absent here on purpose and the token carries the weight: a same-origin `fetch`
/// sends no `Origin` at all, so requiring one would refuse our own page.
fn guard(st: &ServeState, headers: &HeaderMap) -> Result<(), ApiError> {
    match st.terminal.as_ref() {
        None => Err(refusal(
            "this store is served without a terminal; `cyberbrain serve --terminal` enables it",
        )),
        Some(cfg) if constant_time_eq(cfg.token.as_bytes(), token_of(headers).as_bytes()) => Ok(()),
        Some(_) => Err(refusal("that is not this session's terminal token")),
    }
}

/// A refusal in the shape every other route here uses, so the page has one error body to
/// understand. `403` is the taxonomy's policy refusal (SPEC §8.1), which is what this is.
fn refusal(message: &str) -> ApiError {
    ApiError::new(StatusCode::FORBIDDEN, "policy-refusal", message.to_string())
}

pub async fn open(
    State(st): State<Arc<ServeState>>,
    headers: HeaderMap,
    ws: WebSocketUpgrade,
) -> Response {
    if st.terminal.is_none() {
        // Answered before the upgrade so a caller gets a readable reason rather than a
        // socket that closes for no stated cause.
        return (
            StatusCode::NOT_FOUND,
            "this store is served without a terminal; `cyberbrain serve --terminal` enables it",
        )
            .into_response();
    }
    let origin = headers
        .get(axum::http::header::ORIGIN)
        .and_then(|v| v.to_str().ok())
        .map(str::to_string);
    ws.on_upgrade(move |socket| session(st, origin, socket))
}

async fn session(st: Arc<ServeState>, origin: Option<String>, mut socket: WebSocket) {
    let Some(Ok(first)) = socket.recv().await else {
        return;
    };
    let text = match first {
        Message::Text(t) => t.to_string(),
        _ => {
            let _ = say(&mut socket, "the first frame has to be the open message").await;
            return;
        }
    };
    let Ok(req) = serde_json::from_str::<Open>(&text) else {
        let _ = say(&mut socket, "the open message did not parse").await;
        return;
    };
    if let Err(why) = admit(
        st.terminal.as_ref(),
        origin.as_deref(),
        &st.origins,
        &req.token,
    ) {
        let _ = say(&mut socket, &why).await;
        return;
    }

    let cwd: PathBuf = st
        .app
        .root()
        .parent()
        .unwrap_or_else(|| st.app.root())
        .to_path_buf();
    let argv = match crate::serve::command::tokenise(&req.command) {
        Ok(argv) => argv.unwrap_or_default(),
        Err(why) => {
            let _ = say(&mut socket, &why).await;
            return;
        }
    };
    let mut pty = match Pty::spawn(Spawn {
        command: &argv,
        cwd: &cwd,
        cols: req.cols.max(1),
        rows: req.rows.max(1),
    }) {
        Ok(p) => p,
        Err(e) => {
            let _ = say(&mut socket, &format!("the terminal could not start: {e}")).await;
            return;
        }
    };
    let (Ok(mut reader), Ok(writer)) = (pty.reader(), pty.writer()) else {
        let _ = say(&mut socket, "the terminal could not be read or written").await;
        pty.kill();
        return;
    };

    // Neither half of a pty has an async form on either platform, so **both** live on
    // threads and talk to this task over channels.
    //
    // The write half is not symmetry for its own sake. `serve` runs on a
    // `new_current_thread` runtime — one worker — and a write to a pty blocks as soon as the
    // program inside stops reading its input. A person pastes a block into a terminal where
    // something is sleeping, and every route this server has stops answering: the page, the
    // API, the other terminals. It is one keystroke from a working product to a hung one,
    // and the only reason the read half was already on a thread is that its blocking is
    // obvious while the write half's is not.
    let (out_tx, mut out_rx) = tokio::sync::mpsc::channel::<Vec<u8>>(64);
    std::thread::spawn(move || {
        let mut buf = [0u8; 8192];
        loop {
            match reader.read(&mut buf) {
                Ok(0) | Err(_) => break,
                Ok(n) => {
                    if out_tx.blocking_send(buf[..n].to_vec()).is_err() {
                        break;
                    }
                }
            }
        }
    });
    let in_tx = spawn_writer(writer);

    loop {
        tokio::select! {
            out = out_rx.recv() => match out {
                Some(bytes) => {
                    if socket.send(Message::Binary(bytes.into())).await.is_err() {
                        break;
                    }
                }
                // End of the terminal's output: the program is gone.
                None => break,
            },
            incoming = socket.recv() => match incoming {
                Some(Ok(Message::Binary(bytes))) => {
                    // Never blocks: a full channel means the program is not reading, and the
                    // answer to that is to drop keystrokes, not to stop the server.
                    if in_tx.try_send(bytes.to_vec()).is_err() && in_tx.is_closed() {
                        break;
                    }
                }
                Some(Ok(Message::Text(t))) => {
                    if let Ok(Control::Resize { cols, rows }) = serde_json::from_str(&t) {
                        let _ = pty.resize(cols.max(1), rows.max(1));
                    }
                }
                Some(Ok(_)) => {}
                Some(Err(_)) | None => break,
            },
        }
    }

    // What became of the program, before it is killed: a shell that exits with a code is
    // saying something, and a pane that just stops leaves the person guessing whether it
    // crashed or they closed it.
    if let Some(code) = pty.exited() {
        let _ = socket
            .send(Message::Text(
                serde_json::json!({ "type": "exit", "code": code })
                    .to_string()
                    .into(),
            ))
            .await;
    }

    // Whoever ended it, the program goes with the window. A shell left running with nothing
    // attached to it is a process nobody can see and nobody will stop.
    //
    // On a thread, because this waits: `Child::wait` on Unix and `WaitForSingleObject` on
    // Windows, and this task is the runtime's only worker.
    tokio::task::spawn_blocking(move || pty.kill());
    // Dropping the socket closes it. `axum` has no `close()` on this type, and sending a
    // close frame by hand adds nothing the drop does not do.
    drop(socket);
}

/// A thread that writes what arrives on the channel into the terminal.
///
/// The channel is what makes the write non-blocking for the caller. Bounded, and
/// deliberately not large: if the program inside is not reading, the right answer is to lose
/// keystrokes rather than to queue megabytes of them for a program that may never want them.
fn spawn_writer(mut writer: Box<dyn Write + Send>) -> tokio::sync::mpsc::Sender<Vec<u8>> {
    let (tx, mut rx) = tokio::sync::mpsc::channel::<Vec<u8>>(64);
    std::thread::spawn(move || {
        while let Some(bytes) = rx.blocking_recv() {
            if writer.write_all(&bytes).is_err() {
                break;
            }
            let _ = writer.flush();
        }
    });
    tx
}

async fn say(socket: &mut WebSocket, message: &str) -> Result<(), axum::Error> {
    socket
        .send(Message::Text(
            serde_json::json!({ "type": "error", "message": message })
                .to_string()
                .into(),
        ))
        .await
}
