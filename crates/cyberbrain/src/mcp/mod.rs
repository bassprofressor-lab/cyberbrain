//! `cyberbrain mcp` (SPEC §9.2): the five operations — `recall`, `recall_id`, `find`,
//! `write`, `status` — over MCP on stdio, as a thin adapter over [`App`].
//!
//! The protocol is JSON-RPC 2.0 with a small method set, so it is hand-rolled here rather
//! than taken from a crate: an MCP library that does its own I/O would be a transport
//! nobody audited, and SPEC §12.1 promises that every path bytes can take off the machine
//! is registered. This module opens no socket and makes no request; it reads stdin and
//! writes stdout.
//!
//! **stdout carries protocol only.** Every diagnostic goes through [`diag!`] to stderr. A
//! stray `println!` here corrupts the stream, and the failure shows up as a parse error on
//! the client's side of the pipe, where it looks like the client's bug.
//!
//! Entry point for the CLI: [`serve_stdio`]. The loop itself, [`serve`], is generic over
//! the two streams so tests can drive it over an in-memory pipe.

mod describe;
mod jsonrpc;
mod stdio;
#[cfg(test)]
mod tests;
mod tools;

use crate::app::App;
use cyberbrain_core::{Error, Result, Slash};
use jsonrpc::{Frame, Framing, Message, RpcError};
use serde_json::{Value, json};
use std::io;
use std::sync::Arc;
use tokio::io::{AsyncBufRead, AsyncWrite, BufReader};

/// Everything a diagnostic needs: stderr, and a prefix so the harness log says who spoke.
macro_rules! diag {
    ($($arg:tt)*) => {
        eprintln!("cyberbrain mcp: {}", format_args!($($arg)*))
    };
}

/// The protocol revision this server implements. Tool listing and calling, with
/// `structuredContent` and `isError` on results, is the whole surface used.
pub const LATEST_PROTOCOL: &str = "2025-06-18";

/// Revisions the server will answer in. The tools surface is the same across them; what
/// differs (batching, structured content) is handled leniently here.
pub const SUPPORTED_PROTOCOLS: &[&str] = &["2025-06-18", "2025-03-26", "2024-11-05"];

/// What the agent is told at `initialize`. MCP-only text: it describes the envelope, not
/// the operations, which the tool descriptions cover from the CLI help.
const INSTRUCTIONS: &str = "Cyberbrain is a cited, trust-tiered memory of this project. \
Every recall hit carries a citation (`r2-...`); expand one with recall_id. Every recall \
result carries `caveats`: read them before trusting the hits. A caveat saying the \
contradiction check was skipped means the hits were not checked against each other, so an \
unchecked result must not be treated as a checked one. Rings: 0 operator invariants, 1 \
protocol, 2 curated knowledge, 3 session records, 4 unverified external material; lower \
wins a contradiction. A write with `isError` and `outcome: held` is a decision, not a \
failure: the body contained something that looks like personal data, nothing was written, \
and you must choose (see the write tool).";

/// Serve on the process's stdin and stdout until the client closes stdin or asks to shut
/// down. Returns when the session is over; an I/O failure on the streams is the only error.
pub async fn serve_stdio(app: Arc<App>) -> Result<()> {
    diag!(
        "serving on stdio; store {}; protocol {} (also {})",
        Slash(app.root()),
        LATEST_PROTOCOL,
        SUPPORTED_PROTOCOLS[1..].join(", ")
    );
    let reader = BufReader::new(stdio::stdin_reader());
    let writer = stdio::stdout_writer();
    serve(app, reader, writer).await
}

/// The session loop over arbitrary streams. Requests are answered one at a time, in
/// order; a notification gets no answer; a frame that cannot be parsed gets an error
/// frame and the loop carries on.
pub async fn serve<R, W>(app: Arc<App>, mut reader: R, mut writer: W) -> Result<()>
where
    R: AsyncBufRead + Unpin,
    W: AsyncWrite + Unpin,
{
    let mut session = Session {
        app,
        protocol: None,
        initialized: false,
        exit: false,
        warned_before_init: false,
    };
    loop {
        let frame = match jsonrpc::read_frame(&mut reader).await {
            Ok(Some(f)) => f,
            Ok(None) => {
                diag!("stdin closed; session over");
                return Ok(());
            }
            Err(e) => return Err(stream_error(e)),
        };
        let (bytes, framing) = match frame {
            Frame::Body(b, f) => (b, f),
            Frame::Refused(err, f) => {
                diag!("refused a frame: {}", err.message);
                if !send(&mut writer, f, &jsonrpc::error_response(&Value::Null, &err)).await? {
                    return Ok(());
                }
                continue;
            }
        };
        let parsed: std::result::Result<Value, _> = serde_json::from_slice(&bytes);
        let value = match parsed {
            Ok(v) => v,
            Err(e) => {
                // A malformed frame is answered, never fatal: the next frame may be fine.
                diag!("unparseable frame ({} bytes): {e}", bytes.len());
                let err = RpcError::new(jsonrpc::PARSE_ERROR, format!("parse error: {e}"));
                if !send(
                    &mut writer,
                    framing,
                    &jsonrpc::error_response(&Value::Null, &err),
                )
                .await?
                {
                    return Ok(());
                }
                continue;
            }
        };
        let responses = match value {
            Value::Array(items) if items.is_empty() => {
                let err = RpcError::new(jsonrpc::INVALID_REQUEST, "empty batch");
                vec![jsonrpc::error_response(&Value::Null, &err)]
            }
            Value::Array(items) => {
                // Batches were dropped from the protocol in 2025-06-18; older clients may
                // still send them, and answering is cheaper than arguing.
                let mut out = Vec::new();
                for item in items {
                    if let Some(r) = session.handle(jsonrpc::classify(item)).await {
                        out.push(r);
                    }
                }
                if out.is_empty() {
                    vec![]
                } else {
                    vec![Value::Array(out)]
                }
            }
            single => session
                .handle(jsonrpc::classify(single))
                .await
                .into_iter()
                .collect(),
        };
        for r in &responses {
            if !send(&mut writer, framing, r).await? {
                return Ok(());
            }
        }
        if session.exit {
            diag!("shutdown requested; session over");
            return Ok(());
        }
    }
}

/// Write one frame. `Ok(false)` means the client went away (broken pipe), which ends the
/// session without being an error: the other side hung up, and there is no one to tell.
async fn send<W: AsyncWrite + Unpin>(w: &mut W, framing: Framing, msg: &Value) -> Result<bool> {
    match jsonrpc::write_frame(w, framing, msg).await {
        Ok(()) => Ok(true),
        Err(e) if e.kind() == io::ErrorKind::BrokenPipe => {
            diag!("stdout closed by the client; session over");
            Ok(false)
        }
        Err(e) => Err(stream_error(e)),
    }
}

fn stream_error(e: io::Error) -> Error {
    Error::Io {
        path: "<stdio>".into(),
        source: e,
    }
}

struct Session {
    app: Arc<App>,
    /// Negotiated at `initialize`; `None` until then.
    protocol: Option<String>,
    initialized: bool,
    exit: bool,
    warned_before_init: bool,
}

impl Session {
    async fn handle(&mut self, msg: Message) -> Option<Value> {
        match msg {
            Message::Invalid { id, error } => {
                diag!("invalid request: {}", error.message);
                Some(jsonrpc::error_response(&id, &error))
            }
            Message::Notification { method, params } => {
                self.notification(&method, params);
                None
            }
            Message::Request { id, method, params } => {
                let outcome = self.request(&method, params).await;
                Some(match outcome {
                    Ok(result) => jsonrpc::response(&id, result),
                    Err(e) => jsonrpc::error_response(&id, &e),
                })
            }
        }
    }

    fn notification(&mut self, method: &str, _params: Value) {
        match method {
            "notifications/initialized" | "initialized" => {
                self.initialized = true;
            }
            "exit" => {
                self.exit = true;
            }
            // Every branch that declines to act says why (SPEC §14.5).
            "notifications/cancelled" => {
                diag!(
                    "notification {method} ignored: requests are answered in order, nothing is in flight to cancel"
                );
            }
            "notifications/progress" | "notifications/roots/list_changed" => {
                diag!(
                    "notification {method} ignored: this server tracks neither progress nor roots"
                );
            }
            other => {
                diag!("notification {other} ignored: unknown");
            }
        }
    }

    async fn request(
        &mut self,
        method: &str,
        params: Value,
    ) -> std::result::Result<Value, RpcError> {
        if !self.initialized && !matches!(method, "initialize" | "ping") && !self.warned_before_init
        {
            self.warned_before_init = true;
            diag!("request {method} arrived before initialize completed; answering anyway");
        }
        match method {
            "initialize" => Ok(self.initialize(&params)),
            "ping" => Ok(json!({})),
            "tools/list" => Ok(tools::list_value()),
            "tools/call" => {
                let name = params
                    .get("name")
                    .and_then(Value::as_str)
                    .ok_or_else(|| RpcError::invalid_params("tools/call needs a string `name`"))?;
                let args = params.get("arguments").cloned().unwrap_or(Value::Null);
                tools::call(&self.app, name, &args).await
            }
            "shutdown" => {
                // Not an MCP method (the stdio transport ends when stdin closes) but harmless
                // to honour: answer, then leave after the answer is on the wire.
                self.exit = true;
                Ok(Value::Null)
            }
            other => Err(RpcError::method_not_found(other)),
        }
    }

    fn initialize(&mut self, params: &Value) -> Value {
        let asked = params
            .get("protocolVersion")
            .and_then(Value::as_str)
            .unwrap_or("");
        let client = params
            .get("clientInfo")
            .and_then(|c| c.get("name"))
            .and_then(Value::as_str)
            .unwrap_or("unnamed client");
        let (version, note) = if SUPPORTED_PROTOCOLS.contains(&asked) {
            (asked.to_string(), None)
        } else {
            // Degrade clearly: answer with what we do speak, say so on stderr and in the
            // instructions the client shows its model, and let the client decide.
            let note = format!(
                "protocol note: {client} asked for MCP protocol {:?}, which this server does \
                 not implement; answering in {LATEST_PROTOCOL} (also available: {}). \
                 Disconnect if that is unacceptable.",
                asked,
                SUPPORTED_PROTOCOLS.join(", ")
            );
            diag!("{note}");
            (LATEST_PROTOCOL.to_string(), Some(note))
        };
        diag!("initialize from {client}: protocol {version}");
        self.protocol = Some(version.clone());
        let instructions = match note {
            Some(n) => format!("{INSTRUCTIONS}\n\n{n}"),
            None => INSTRUCTIONS.to_string(),
        };
        json!({
            "protocolVersion": version,
            "capabilities": { "tools": { "listChanged": false } },
            "serverInfo": {
                "name": "cyberbrain",
                "title": "Cyberbrain",
                "version": env!("CARGO_PKG_VERSION"),
            },
            "instructions": instructions,
        })
    }
}
