//! The model-download transport (SPEC §6: "every fetch goes through our own registered
//! egress path"; feature `http`).
//!
//! The inference layer builds its own client in `cyberbrain-llm` and asks the gate through
//! `EgressGate::permit`. This module is the *other* registered path: fetching a model
//! artefact. Every function here takes an [`EgressTicket`], which only [`Egress::open`] can
//! issue, and refuses a URL whose scheme, host or port differ from the ticket's destination.
//! The client is built per ticket and, per SPEC §12.1.1:
//!
//! - follows **no redirects**: a redirect is a new destination, so [`download`] opens a new
//!   ticket per hop and each hop is checked and audited;
//! - uses **no proxy**, ignoring `HTTP(S)_PROXY`: a proxy is a host the register does not
//!   name. Operators behind a mandatory proxy cannot download through Cyberbrain today;
//!   that is a deliberate gap, and it is reported;
//! - **pins the connection** to the addresses the gate resolved and checked
//!   (`resolve_to_addrs`), so a DNS answer cannot change between the check and the connect;
//! - verifies TLS against the **platform** trust store (`rustls-platform-verifier`) with the
//!   `ring` provider. No root store is compiled into the binary; no OpenSSL.
//!
//! Bodies are read fully into memory. The largest thing that goes through here is a ~30 MB
//! model artefact.

use super::{Egress, EgressTicket, Outcome};
use crate::audit::Actor;
use cyberbrain_core::{EgressPurpose, Error, Result};
use std::net::SocketAddr;
use std::sync::Once;
use std::time::Duration;
use url::Url;

const USER_AGENT: &str = concat!("cyberbrain/", env!("CARGO_PKG_VERSION"));
const CONNECT_TIMEOUT: Duration = Duration::from_secs(15);
const READ_TIMEOUT: Duration = Duration::from_secs(120);

static TLS_PROVIDER: Once = Once::new();

fn install_tls_provider() {
    TLS_PROVIDER.call_once(|| {
        // Another crate may have installed one already; that is fine.
        let _ = rustls::crypto::ring::default_provider().install_default();
    });
}

/// What came back. `redirect` is set for 3xx responses with a `Location` header; the caller
/// decides whether to open a new ticket for it.
#[derive(Debug, Clone)]
pub struct Response {
    pub status: u16,
    pub body: Vec<u8>,
    pub content_type: Option<String>,
    pub redirect: Option<String>,
}

fn err_for(purpose: EgressPurpose, msg: String) -> Error {
    match purpose {
        EgressPurpose::ModelDownload => Error::Embed(msg),
        EgressPurpose::LocalInference => Error::Llm(msg),
    }
}

fn check(ticket: &EgressTicket, url: &str) -> Result<Url> {
    let u = Url::parse(url).map_err(|e| Error::Config(format!("{url:?}: {e}")))?;
    if !ticket.permits(&u) {
        return Err(ticket.refuse(&format!(
            "request to {}://{}:{} is not covered by ticket {} for {}",
            u.scheme(),
            u.host_str().unwrap_or(""),
            u.port_or_known_default().unwrap_or(0),
            ticket.id(),
            ticket.destination().origin(),
        )));
    }
    Ok(u)
}

/// The only HTTP client construction in this crate. Takes the ticket, which took the gate.
fn client(ticket: &EgressTicket) -> Result<reqwest::Client> {
    install_tls_provider();
    let dest = ticket.destination();
    let pinned: Vec<SocketAddr> = dest
        .addrs
        .iter()
        .map(|ip| SocketAddr::new(*ip, dest.port))
        .collect();
    reqwest::Client::builder()
        .user_agent(USER_AGENT)
        .redirect(reqwest::redirect::Policy::none())
        .no_proxy()
        .connect_timeout(CONNECT_TIMEOUT)
        .read_timeout(READ_TIMEOUT)
        .https_only(ticket.purpose() == EgressPurpose::ModelDownload)
        .resolve_to_addrs(&dest.host, &pinned)
        .build()
        .map_err(|e| err_for(ticket.purpose(), format!("building HTTP client: {e}")))
}

async fn finish(ticket: &EgressTicket, resp: reqwest::Response) -> Result<Response> {
    let status = resp.status().as_u16();
    let content_type = resp
        .headers()
        .get(reqwest::header::CONTENT_TYPE)
        .and_then(|v| v.to_str().ok())
        .map(str::to_owned);
    let redirect = if (300..400).contains(&status) {
        resp.headers()
            .get(reqwest::header::LOCATION)
            .and_then(|v| v.to_str().ok())
            .map(|loc| {
                let base = format!(
                    "{}{}",
                    ticket.destination().origin(),
                    ticket.destination().path
                );
                match Url::parse(loc) {
                    Ok(u) => u.to_string(),
                    Err(_) => Url::parse(&base)
                        .and_then(|b| b.join(loc))
                        .map(|u| u.to_string())
                        .unwrap_or_else(|_| loc.to_string()),
                }
            })
    } else {
        None
    };
    let body = resp
        .bytes()
        .await
        .map_err(|e| err_for(ticket.purpose(), format!("reading response body: {e}")))?
        .to_vec();
    Ok(Response {
        status,
        body,
        content_type,
        redirect,
    })
}

/// HTTP GET. The URL must be covered by the ticket.
pub async fn get(ticket: &EgressTicket, url: &str) -> Result<Response> {
    let u = check(ticket, url)?;
    let c = client(ticket)?;
    let resp = c
        .get(u)
        .send()
        .await
        .map_err(|e| err_for(ticket.purpose(), format!("GET {}: {e}", redact(url))))?;
    finish(ticket, resp).await
}

/// A completed download: the bytes and the hop chain that produced them.
#[derive(Debug)]
pub struct Downloaded {
    pub bytes: Vec<u8>,
    pub final_url: String,
    /// Every URL contacted, in order. Each one has an audit row.
    pub hops: Vec<String>,
}

/// Fetch a model artefact, following HTTPS redirects up to `max_hops` times with one
/// audited ticket per hop. Hash verification is the caller's job (SPEC §6) and happens
/// after this returns.
pub async fn download(
    egress: &Egress,
    actor: &Actor,
    url: &str,
    max_hops: usize,
) -> Result<Downloaded> {
    let mut hops = Vec::new();
    let mut current = url.to_string();
    let mut previous: Option<EgressTicket> = None;
    loop {
        let ticket = match &previous {
            None => egress.open(actor, EgressPurpose::ModelDownload, &current)?,
            Some(prev) => egress.open_redirect(prev, &current)?,
        };
        if let Some(prev) = previous.take() {
            prev.close(Outcome::ok(Some(302)))?;
        }
        hops.push(redact(&current));
        let resp = match get(&ticket, &current).await {
            Ok(r) => r,
            Err(e) => {
                ticket.close(Outcome::failed(&e.to_string()))?;
                return Err(e);
            }
        };
        match resp.redirect {
            Some(next) => {
                if hops.len() > max_hops {
                    ticket.close(Outcome::failed("too many redirects"))?;
                    return Err(Error::Embed(format!(
                        "{}: more than {max_hops} redirects",
                        redact(url)
                    )));
                }
                previous = Some(ticket);
                current = next;
            }
            None if (200..300).contains(&resp.status) => {
                ticket.close(Outcome::ok(Some(resp.status)).bytes(0, resp.body.len() as u64))?;
                return Ok(Downloaded {
                    bytes: resp.body,
                    final_url: redact(&current),
                    hops,
                });
            }
            None => {
                ticket.close(Outcome::failed(&format!("HTTP {}", resp.status)))?;
                return Err(Error::Embed(format!(
                    "{}: HTTP {}",
                    redact(&current),
                    resp.status
                )));
            }
        }
    }
}

/// URL without query or fragment, for error messages and audit rows.
fn redact(url: &str) -> String {
    match Url::parse(url) {
        Ok(u) => {
            let mut s = format!("{}://{}", u.scheme(), u.host_str().unwrap_or(""));
            if let Some(p) = u.port() {
                s.push_str(&format!(":{p}"));
            }
            s.push_str(u.path());
            s
        }
        Err(_) => "<unparseable url>".to_string(),
    }
}

#[cfg(test)]
mod tests {
    //! The production route for this transport is HTTPS to a public model host, which an
    //! offline test cannot take. What can be taken is the same code path over plain HTTP to
    //! loopback with a `LocalInference` ticket (the ticket's purpose only flips
    //! `https_only`). The TLS and public-host leg is exercised by the gate tests above and
    //! by the first real download.

    use super::*;
    use crate::audit::AuditLog;
    use crate::config::PolicyConfig;
    use std::io::{Read, Write};
    use std::net::TcpListener;
    use std::sync::Arc;
    use std::sync::atomic::{AtomicUsize, Ordering};

    /// A one-shot HTTP/1.1 server on loopback. Returns its port, a counter of accepted
    /// connections, and the last request it saw.
    fn serve(reply: &'static str) -> (u16, Arc<AtomicUsize>, Arc<std::sync::Mutex<String>>) {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let port = listener.local_addr().unwrap().port();
        let hits = Arc::new(AtomicUsize::new(0));
        let seen = Arc::new(std::sync::Mutex::new(String::new()));
        let (h, s) = (hits.clone(), seen.clone());
        std::thread::spawn(move || {
            for stream in listener.incoming().flatten() {
                h.fetch_add(1, Ordering::SeqCst);
                let mut stream = stream;
                let mut buf = vec![0u8; 65536];
                let mut req = Vec::new();
                loop {
                    let n = stream.read(&mut buf).unwrap_or(0);
                    if n == 0 {
                        break;
                    }
                    req.extend_from_slice(&buf[..n]);
                    if req.windows(4).any(|w| w == b"\r\n\r\n") {
                        break;
                    }
                }
                *s.lock().unwrap() = String::from_utf8_lossy(&req).to_string();
                let _ = stream.write_all(reply.as_bytes());
                let _ = stream.flush();
            }
        });
        (port, hits, seen)
    }

    fn gate() -> (Egress, Arc<crate::audit::MemoryAuditSink>) {
        let (log, sink) = AuditLog::in_memory();
        (Egress::new(PolicyConfig::default(), log, Actor::Cli), sink)
    }

    #[tokio::test]
    async fn a_ticketed_get_reaches_the_server_and_is_audited() {
        let (port, hits, seen) = serve(
            "HTTP/1.1 200 OK\r\nContent-Type: application/octet-stream\r\nContent-Length: 5\r\nConnection: close\r\n\r\nhello",
        );
        let (g, sink) = gate();
        let url = format!("http://127.0.0.1:{port}/models/m.bin");
        let t = g
            .open(&Actor::Cli, EgressPurpose::LocalInference, &url)
            .unwrap();
        let r = get(&t, &url).await.unwrap();
        assert_eq!(r.status, 200);
        assert_eq!(r.body, b"hello");
        assert_eq!(r.content_type.as_deref(), Some("application/octet-stream"));
        t.close(Outcome::ok(Some(r.status)).bytes(0, r.body.len() as u64))
            .unwrap();
        assert_eq!(hits.load(Ordering::SeqCst), 1);
        let req = seen.lock().unwrap().clone();
        assert!(req.starts_with("GET /models/m.bin HTTP/1.1"), "{req}");
        assert!(
            req.to_ascii_lowercase()
                .contains(&format!("user-agent: {USER_AGENT}").to_ascii_lowercase()),
            "{req}"
        );
        assert_eq!(sink.actions(), ["egress.permitted", "egress.completed"]);
        assert_eq!(sink.rows()[1].detail["bytes_in"], 5);
    }

    /// Would fail if the transport contacted a host the ticket does not cover.
    #[tokio::test]
    async fn a_ticket_for_one_host_cannot_be_spent_on_another() {
        let (port_a, hits_a, _) =
            serve("HTTP/1.1 200 OK\r\nContent-Length: 0\r\nConnection: close\r\n\r\n");
        let (port_b, hits_b, _) =
            serve("HTTP/1.1 200 OK\r\nContent-Length: 0\r\nConnection: close\r\n\r\n");
        let (g, sink) = gate();
        let t = g
            .open(
                &Actor::Cli,
                EgressPurpose::LocalInference,
                &format!("http://127.0.0.1:{port_a}/x"),
            )
            .unwrap();
        let err = get(&t, &format!("http://127.0.0.1:{port_b}/x"))
            .await
            .unwrap_err();
        assert!(matches!(err, Error::PolicyRefusal { .. }), "{err}");
        assert_eq!(
            hits_b.load(Ordering::SeqCst),
            0,
            "the other server must never be contacted"
        );
        assert_eq!(hits_a.load(Ordering::SeqCst), 0);
        assert_eq!(sink.actions(), ["egress.permitted", "policy.refusal"]);
        t.close(Outcome::failed("refused")).unwrap();
    }

    #[tokio::test]
    async fn redirects_are_reported_not_followed() {
        let (port, hits, _) = serve(
            "HTTP/1.1 302 Found\r\nLocation: /elsewhere\r\nContent-Length: 0\r\nConnection: close\r\n\r\n",
        );
        let (g, _) = gate();
        let url = format!("http://127.0.0.1:{port}/x");
        let t = g
            .open(&Actor::Cli, EgressPurpose::LocalInference, &url)
            .unwrap();
        let r = get(&t, &url).await.unwrap();
        assert_eq!(r.status, 302);
        assert_eq!(
            r.redirect.as_deref(),
            Some(format!("http://127.0.0.1:{port}/elsewhere").as_str()),
            "relative Location is resolved against the ticket"
        );
        assert_eq!(hits.load(Ordering::SeqCst), 1, "exactly one request");
        t.close(Outcome::ok(Some(302))).unwrap();
    }

    #[tokio::test]
    async fn a_download_cannot_go_over_plaintext() {
        let (port, hits, _) =
            serve("HTTP/1.1 200 OK\r\nContent-Length: 0\r\nConnection: close\r\n\r\n");
        let (log, sink) = AuditLog::in_memory();
        let cfg = PolicyConfig {
            model_source: Some("https://models.example.org/".into()),
            model_download_consent: true,
            ..Default::default()
        };
        let g = Egress::new(cfg, log, Actor::Operator);
        let err = download(
            &g,
            &Actor::Operator,
            &format!("http://127.0.0.1:{port}/m.bin"),
            3,
        )
        .await
        .unwrap_err();
        assert!(matches!(err, Error::PolicyRefusal { .. }), "{err}");
        assert_eq!(hits.load(Ordering::SeqCst), 0);
        assert_eq!(sink.actions(), ["policy.refusal"]);
    }
}
