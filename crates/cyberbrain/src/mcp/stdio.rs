//! The process's stdin and stdout as async streams, without tokio's `io-std` feature.
//!
//! `tokio::io::stdin()` is not available to this crate (the feature is not enabled and
//! `Cargo.toml` is not this module's to edit), so stdin is bridged by one blocking thread
//! feeding a channel, and stdout is written synchronously. Both are fine for a server that
//! answers one request at a time: a blocked write means the client stopped reading, and
//! there is nothing useful to do concurrently anyway.
//!
//! Stdout is the protocol stream. Nothing in this module or its siblings writes to it
//! except [`write_frame`](super::jsonrpc::write_frame) through [`ProtocolStdout`].

use std::io::{self, Read, Write};
use std::pin::Pin;
use std::task::{Context, Poll};
use tokio::io::{AsyncRead, AsyncWrite, ReadBuf};
use tokio::sync::mpsc;

/// Chunks read from stdin on a dedicated thread, delivered as an `AsyncRead`.
pub struct StdinReader {
    rx: mpsc::Receiver<Vec<u8>>,
    pending: Vec<u8>,
    pos: usize,
}

/// Start the reader thread. It ends when stdin reaches EOF or when the receiver is gone.
pub fn stdin_reader() -> StdinReader {
    let (tx, rx) = mpsc::channel::<Vec<u8>>(8);
    let spawned = std::thread::Builder::new()
        .name("cyberbrain-mcp-stdin".into())
        .spawn(move || {
            let mut stdin = io::stdin().lock();
            let mut buf = vec![0u8; 64 * 1024];
            loop {
                match stdin.read(&mut buf) {
                    Ok(0) => break,
                    Ok(n) => {
                        if tx.blocking_send(buf[..n].to_vec()).is_err() {
                            break; // the server is gone; nobody to deliver to
                        }
                    }
                    Err(e) if e.kind() == io::ErrorKind::Interrupted => continue,
                    Err(e) => {
                        eprintln!("cyberbrain mcp: stdin read failed: {e}");
                        break;
                    }
                }
            }
        });
    if let Err(e) = spawned {
        // The channel's sender is dropped with the failed closure, so the reader sees EOF
        // immediately; the server then exits cleanly rather than hanging. Say why.
        eprintln!("cyberbrain mcp: cannot start the stdin thread: {e}");
    }
    StdinReader {
        rx,
        pending: Vec::new(),
        pos: 0,
    }
}

impl AsyncRead for StdinReader {
    fn poll_read(
        mut self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        buf: &mut ReadBuf<'_>,
    ) -> Poll<io::Result<()>> {
        loop {
            if self.pos < self.pending.len() {
                let n = (self.pending.len() - self.pos).min(buf.remaining());
                let start = self.pos;
                buf.put_slice(&self.pending[start..start + n]);
                self.pos += n;
                return Poll::Ready(Ok(()));
            }
            match self.rx.poll_recv(cx) {
                Poll::Ready(Some(chunk)) => {
                    self.pending = chunk;
                    self.pos = 0;
                }
                Poll::Ready(None) => return Poll::Ready(Ok(())), // EOF
                Poll::Pending => return Poll::Pending,
            }
        }
    }
}

/// The protocol stream. Synchronous writes to the process's stdout behind the `AsyncWrite`
/// interface the server loop is written against.
pub struct ProtocolStdout {
    out: io::Stdout,
}

pub fn stdout_writer() -> ProtocolStdout {
    ProtocolStdout { out: io::stdout() }
}

impl AsyncWrite for ProtocolStdout {
    fn poll_write(
        self: Pin<&mut Self>,
        _cx: &mut Context<'_>,
        buf: &[u8],
    ) -> Poll<io::Result<usize>> {
        let mut lock = self.out.lock();
        Poll::Ready(lock.write_all(buf).map(|()| buf.len()))
    }

    fn poll_flush(self: Pin<&mut Self>, _cx: &mut Context<'_>) -> Poll<io::Result<()>> {
        Poll::Ready(self.out.lock().flush())
    }

    fn poll_shutdown(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<io::Result<()>> {
        self.poll_flush(cx)
    }
}
