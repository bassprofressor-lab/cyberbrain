//! JSON-RPC 2.0 over a byte stream, hand-rolled.
//!
//! Two framings are understood on the way in and answered in kind: newline-delimited JSON,
//! which is what the MCP stdio transport specifies, and LSP-style `Content-Length:` headers,
//! which some older clients still send. A frame is answered in the framing it arrived in, so
//! a client never has to guess.
//!
//! Nothing in here touches stdout or stderr directly; the caller owns the streams.

use serde_json::{Value, json};
use std::io;
use tokio::io::{AsyncBufRead, AsyncBufReadExt, AsyncReadExt, AsyncWrite, AsyncWriteExt};

pub const PARSE_ERROR: i64 = -32700;
pub const INVALID_REQUEST: i64 = -32600;
pub const METHOD_NOT_FOUND: i64 = -32601;
pub const INVALID_PARAMS: i64 = -32602;
pub const INTERNAL_ERROR: i64 = -32603;

/// A frame larger than this is refused with an error frame rather than buffered. A note
/// body is at most a few hundred kilobytes; anything past this is not a request.
pub const MAX_FRAME_BYTES: usize = 64 << 20;

/// A JSON-RPC error object. Used for *protocol* failures only: an unknown method, a
/// malformed request, arguments that do not fit the tool's schema. An operation that ran
/// and declined (a policy refusal, a held write) is a tool result, never one of these.
#[derive(Debug, Clone, PartialEq)]
pub struct RpcError {
    pub code: i64,
    pub message: String,
    pub data: Option<Value>,
}

impl RpcError {
    pub fn new(code: i64, message: impl Into<String>) -> Self {
        RpcError {
            code,
            message: message.into(),
            data: None,
        }
    }

    pub fn invalid_params(message: impl Into<String>) -> Self {
        Self::new(INVALID_PARAMS, message)
    }

    pub fn method_not_found(method: &str) -> Self {
        Self::new(METHOD_NOT_FOUND, format!("method not found: {method}"))
    }

    pub fn internal(message: impl Into<String>) -> Self {
        Self::new(INTERNAL_ERROR, message)
    }

    pub fn to_value(&self) -> Value {
        let mut v = json!({ "code": self.code, "message": self.message });
        if let Some(d) = &self.data {
            v["data"] = d.clone();
        }
        v
    }
}

/// How a frame was delimited on the wire.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Framing {
    /// One JSON object per line. The MCP stdio transport.
    Lines,
    /// `Content-Length: N\r\n\r\n<N bytes>`. LSP heritage; still seen.
    ContentLength,
}

/// One decoded message. `Invalid` carries the id we could recover, so the error can be
/// correlated, and the error to send.
#[derive(Debug)]
pub enum Message {
    Request {
        id: Value,
        method: String,
        params: Value,
    },
    Notification {
        method: String,
        params: Value,
    },
    Invalid {
        id: Value,
        error: RpcError,
    },
}

/// Sort one JSON value into request, notification or invalid. The distinction between
/// request and notification is the presence of an `id` key, per JSON-RPC 2.0; an explicit
/// `null` id is a request whose answer carries `null`.
pub fn classify(v: Value) -> Message {
    let Value::Object(mut obj) = v else {
        return Message::Invalid {
            id: Value::Null,
            error: RpcError::new(INVALID_REQUEST, "a request is a JSON object"),
        };
    };
    let id = obj.get("id").cloned();
    let id_ok = matches!(
        id,
        None | Some(Value::Null) | Some(Value::String(_)) | Some(Value::Number(_))
    );
    if !id_ok {
        return Message::Invalid {
            id: Value::Null,
            error: RpcError::new(INVALID_REQUEST, "id must be a string, a number or null"),
        };
    }
    if obj.get("jsonrpc").and_then(Value::as_str) != Some("2.0") {
        return Message::Invalid {
            id: id.unwrap_or(Value::Null),
            error: RpcError::new(INVALID_REQUEST, "jsonrpc must be \"2.0\""),
        };
    }
    let method = match obj.remove("method") {
        Some(Value::String(m)) => m,
        _ => {
            return Message::Invalid {
                id: id.unwrap_or(Value::Null),
                error: RpcError::new(INVALID_REQUEST, "method must be a string"),
            };
        }
    };
    let params = obj.remove("params").unwrap_or(Value::Null);
    match id {
        Some(id) => Message::Request { id, method, params },
        None => Message::Notification { method, params },
    }
}

pub fn response(id: &Value, result: Value) -> Value {
    json!({ "jsonrpc": "2.0", "id": id, "result": result })
}

pub fn error_response(id: &Value, error: &RpcError) -> Value {
    json!({ "jsonrpc": "2.0", "id": id, "error": error.to_value() })
}

fn trim_ascii(b: &[u8]) -> &[u8] {
    let start = b.iter().position(|c| !c.is_ascii_whitespace());
    let end = b.iter().rposition(|c| !c.is_ascii_whitespace());
    match (start, end) {
        (Some(s), Some(e)) => &b[s..=e],
        _ => &[],
    }
}

fn content_length(line: &[u8]) -> Option<usize> {
    let line = std::str::from_utf8(line).ok()?;
    let (name, value) = line.split_once(':')?;
    if !name.trim().eq_ignore_ascii_case("content-length") {
        return None;
    }
    value.trim().parse().ok()
}

/// What `read_frame` found.
#[derive(Debug)]
pub enum Frame {
    /// Bytes that should be one JSON-RPC message (or batch), and how they were delimited.
    Body(Vec<u8>, Framing),
    /// A frame that had to be discarded before it could be parsed; the bytes are gone and
    /// the caller should answer with this error and carry on.
    Refused(RpcError, Framing),
}

/// Read the next frame. `Ok(None)` is end of stream. Blank lines between frames are
/// skipped; they are not messages.
pub async fn read_frame<R: AsyncBufRead + Unpin>(r: &mut R) -> io::Result<Option<Frame>> {
    loop {
        let mut line = Vec::new();
        let n = r.read_until(b'\n', &mut line).await?;
        if n == 0 {
            return Ok(None);
        }
        if line.len() > MAX_FRAME_BYTES {
            return Ok(Some(Frame::Refused(
                RpcError::new(
                    INVALID_REQUEST,
                    format!(
                        "frame of {} bytes exceeds the {MAX_FRAME_BYTES} byte limit",
                        line.len()
                    ),
                ),
                Framing::Lines,
            )));
        }
        let trimmed = trim_ascii(&line);
        if trimmed.is_empty() {
            continue;
        }
        if let Some(len) = content_length(trimmed) {
            // Remaining headers up to the blank line; none of them matter to us.
            loop {
                let mut h = Vec::new();
                let n = r.read_until(b'\n', &mut h).await?;
                if n == 0 {
                    return Err(io::Error::new(
                        io::ErrorKind::UnexpectedEof,
                        "stream ended inside a Content-Length header block",
                    ));
                }
                if trim_ascii(&h).is_empty() {
                    break;
                }
            }
            if len > MAX_FRAME_BYTES {
                return Ok(Some(Frame::Refused(
                    RpcError::new(
                        INVALID_REQUEST,
                        format!("Content-Length {len} exceeds the {MAX_FRAME_BYTES} byte limit"),
                    ),
                    Framing::ContentLength,
                )));
            }
            let mut body = vec![0u8; len];
            r.read_exact(&mut body).await?;
            return Ok(Some(Frame::Body(body, Framing::ContentLength)));
        }
        return Ok(Some(Frame::Body(trimmed.to_vec(), Framing::Lines)));
    }
}

/// Write one message in the given framing and flush. Compact JSON never contains a raw
/// newline (strings escape it), which is what makes the line framing sound.
pub async fn write_frame<W: AsyncWrite + Unpin>(
    w: &mut W,
    framing: Framing,
    msg: &Value,
) -> io::Result<()> {
    let body = serde_json::to_vec(msg).map_err(io::Error::other)?;
    debug_assert!(
        !body.contains(&b'\n'),
        "compact JSON must not contain a raw newline"
    );
    match framing {
        Framing::Lines => {
            w.write_all(&body).await?;
            w.write_all(b"\n").await?;
        }
        Framing::ContentLength => {
            w.write_all(format!("Content-Length: {}\r\n\r\n", body.len()).as_bytes())
                .await?;
            w.write_all(&body).await?;
        }
    }
    w.flush().await
}
