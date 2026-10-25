//! Who is on the other end of a connection: this account, or another one on the machine.
//!
//! Loopback keeps the network out, and [`super::origin`] keeps other web pages out. Neither
//! keeps out another **account** on the same machine: on a shared server, every user could
//! read, change and erase this store through `/api/v1`, because a TCP connection to
//! `127.0.0.1` carries no name. SPEC §8 draws the line "around other origins and other
//! users", and only the terminal held it, with its token.
//!
//! A token for the whole API would hold it too, but the page would then have to be handed
//! the token, and whatever hands it over is a request any account can make. The kernel
//! already knows the answer, and it cannot be forged: on Linux every TCP socket is listed in
//! `/proc/net/tcp` with the uid that owns it. A loopback connection appears there twice, and
//! the client's half — local address = the peer we see, remote address = our listening
//! socket — names the account that opened it. So the check needs nothing from the page, the
//! desktop launcher or `curl`, and refuses only what it exists to refuse: a connection from
//! an account that is neither this one nor root. Root is admitted because it can read the
//! store's files directly, and refusing it would protect nothing.
//!
//! A connection whose owner cannot be found is refused too. That is fail-closed, and it is
//! not expected to happen: the client's socket exists for as long as its connection does.
//!
//! Elsewhere (macOS, Windows) there is no such table to read here, and connections are
//! admitted as before; the startup line says which of the two this run is.

use axum::serve::Listener;
use std::io;
use std::net::SocketAddr;
use tokio::net::{TcpListener, TcpStream};

/// A listener that only hands over connections opened by this account (or root).
pub struct OwnUserOnly {
    inner: TcpListener,
    own: Option<u32>,
}

/// Wrap a bound listener. Where the platform cannot name a connection's account, the
/// wrapper admits everything, as the bare listener did.
pub fn own_user_only(inner: TcpListener) -> OwnUserOnly {
    OwnUserOnly {
        inner,
        own: own_uid(),
    }
}

/// Whether this platform can refuse other accounts. Printed on the startup line.
pub const ENFORCED: bool = cfg!(target_os = "linux");

impl Listener for OwnUserOnly {
    type Io = TcpStream;
    type Addr = SocketAddr;

    async fn accept(&mut self) -> (Self::Io, Self::Addr) {
        loop {
            let (io, peer) = Listener::accept(&mut self.inner).await;
            let Some(own) = self.own else {
                return (io, peer);
            };
            let local = match io.local_addr() {
                Ok(a) => a,
                Err(e) => {
                    eprintln!("cyberbrain serve: refused a connection from {peer}: {e}");
                    continue;
                }
            };
            match admit(own, peer, local) {
                Ok(()) => return (io, peer),
                // Dropped without an answer: an HTTP refusal would already be a conversation
                // with somebody who has no business here, and it would tell them the API is
                // up. The operator sees the line.
                Err(why) => eprintln!("cyberbrain serve: refused a connection from {peer}: {why}"),
            }
        }
    }

    fn local_addr(&self) -> io::Result<Self::Addr> {
        self.inner.local_addr()
    }
}

#[cfg(target_os = "linux")]
fn own_uid() -> Option<u32> {
    // Safety: geteuid cannot fail and touches no memory.
    Some(unsafe { libc::geteuid() })
}

#[cfg(not(target_os = "linux"))]
fn own_uid() -> Option<u32> {
    None
}

/// Admit a connection from `peer` to our `local` socket when its owner is `own` or root.
#[cfg(target_os = "linux")]
fn admit(own: u32, peer: SocketAddr, local: SocketAddr) -> Result<(), String> {
    let table = std::fs::read_to_string("/proc/net/tcp")
        .map_err(|e| format!("cannot read /proc/net/tcp to tell whose it is ({e})"))?;
    decide(own, uid_in(&table, peer, local))
}

#[cfg(not(target_os = "linux"))]
fn admit(_own: u32, _peer: SocketAddr, _local: SocketAddr) -> Result<(), String> {
    Ok(())
}

#[cfg(target_os = "linux")]
fn decide(own: u32, owner: Option<u32>) -> Result<(), String> {
    match owner {
        Some(uid) if uid == own || uid == 0 => Ok(()),
        Some(uid) => Err(format!(
            "it was opened by uid {uid}, and this store is served to uid {own} only"
        )),
        None => Err("its owner could not be found in /proc/net/tcp".into()),
    }
}

/// The uid owning the client side of a connection: the row whose local address is the peer
/// and whose remote address is our socket. IPv4 only, because the bind is.
#[cfg(target_os = "linux")]
fn uid_in(table: &str, peer: SocketAddr, local: SocketAddr) -> Option<u32> {
    let (SocketAddr::V4(peer), SocketAddr::V4(local)) = (peer, local) else {
        return None;
    };
    // The kernel prints the address as the in-memory u32 of the network-order bytes, so on
    // a little-endian machine 127.0.0.1 is `0100007F`; the port is plain big-endian hex.
    let hex = |a: std::net::SocketAddrV4| {
        format!(
            "{:08X}:{:04X}",
            u32::from_ne_bytes(a.ip().octets()),
            a.port()
        )
    };
    let (want_local, want_remote) = (hex(peer), hex(local));
    table.lines().skip(1).find_map(|line| {
        let cols: Vec<&str> = line.split_whitespace().collect();
        // sl local rem st tx:rx tr:when retrnsmt uid ...
        (cols.len() > 7
            && cols[1].eq_ignore_ascii_case(&want_local)
            && cols[2].eq_ignore_ascii_case(&want_remote))
        .then(|| cols[7].parse().ok())
        .flatten()
    })
}

// In a file of its own: the test connects, and the egress scan skips only test files.
// Linux only, because only there is there anything to check.
#[cfg(all(test, target_os = "linux"))]
mod tests;
