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
//!    because that is a program on this machine running as this user — which already has
//!    every shell it wants and needs no help from us.
//!
//! What none of this claims to stop is the user's own account. A process running as you can
//! start a shell without asking us. The line this draws is around *other* origins and
//! *other* users, and that is the line worth drawing.

pub mod pty;
#[cfg(test)]
mod tests;

use crate::serve::ServeState;
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
    /// argv. Empty means the platform's usual shell.
    #[serde(default)]
    pub command: Vec<String>,
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
    let mut pty = match Pty::spawn(Spawn {
        command: &req.command,
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
    let (Ok(mut reader), Ok(mut writer)) = (pty.reader(), pty.writer()) else {
        let _ = say(&mut socket, "the terminal could not be read or written").await;
        pty.kill();
        return;
    };

    // The pty's read is blocking and has no async form on either platform, so it lives on a
    // thread and hands bytes over a channel. Chunks rather than lines: a terminal stream is
    // not lines, and waiting for one would hold back a prompt that has no newline.
    let (tx, mut rx) = tokio::sync::mpsc::channel::<Vec<u8>>(64);
    std::thread::spawn(move || {
        let mut buf = [0u8; 8192];
        loop {
            match reader.read(&mut buf) {
                Ok(0) | Err(_) => break,
                Ok(n) => {
                    if tx.blocking_send(buf[..n].to_vec()).is_err() {
                        break;
                    }
                }
            }
        }
    });

    loop {
        tokio::select! {
            out = rx.recv() => match out {
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
                    if writer.write_all(&bytes).is_err() {
                        break;
                    }
                    let _ = writer.flush();
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
    pty.kill();
    // Dropping the socket closes it. `axum` has no `close()` on this type, and sending a
    // close frame by hand adds nothing the drop does not do.
    drop(socket);
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
