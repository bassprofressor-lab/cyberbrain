//! The hub's own TLS, and the reason it is not somebody else's.
//!
//! # Why the hub terminates rather than a proxy in front of it
//!
//! `docs/HUB.md` said to put a reverse proxy inside the network, and for a hosted hub that is
//! still the better answer. For the hub this product actually ships to — a Windows service on
//! a machine in a company of eleven people — it is not an answer at all: it is a second piece
//! of software to install, configure and renew, owned by the one person who was never going
//! to open a prompt. A collector that is only encrypted when somebody else did the work is a
//! collector that mostly is not.
//!
//! # What this slice does and does not do
//!
//! It terminates TLS with a certificate the operator supplies, and it prints the
//! certificate's fingerprint so it can be compared with what a browser shows. It does **not**
//! generate a certificate and it does **not** pin one at enrolment — that is the next slice,
//! and until it lands a hub in a network with no certificate authority of its own still has
//! the choice between a browser warning and plain text.
//!
//! The client side needs nothing: `cyberbrain-policy`'s transport verifies against the
//! platform trust store, so a certificate from a company CA or a public one is trusted the
//! day it is installed, by the same rules as everything else on that machine.
//!
//! # Why the accept loop is written out by hand
//!
//! `axum::serve` takes a listener, not an acceptor, and the handshake has to happen on the
//! connection's own task. Doing it in the accept loop would let one client that opens a
//! socket and then says nothing hold up every delivery behind it, which is a cheap way to
//! stop a hub collecting without ever authenticating.

use cyberbrain_core::{Error, Result};
use rustls::ServerConfig;
use rustls_pki_types::pem::PemObject;
use rustls_pki_types::{CertificateDer, PrivateKeyDer};
use std::path::Path;
use std::sync::{Arc, Once};

/// How long a stop waits for connections that are still being served before it goes anyway.
/// Long enough for a delivery to finish, short enough that a service stop does not look hung.
const DRAIN: std::time::Duration = std::time::Duration::from_secs(20);

static TLS_PROVIDER: Once = Once::new();

fn install_tls_provider() {
    TLS_PROVIDER.call_once(|| {
        // The egress transport installs the same one; whoever gets there first wins and the
        // other call is a no-op.
        let _ = rustls::crypto::ring::default_provider().install_default();
    });
}

/// A loaded certificate and the key that goes with it.
///
/// Clone is cheap and it has to be: what serving means is set up once as a closure that the
/// service control manager may call, so everything it captures is shared rather than moved.
#[derive(Clone)]
pub struct Certificate {
    config: Arc<ServerConfig>,
    /// SHA-256 over the leaf certificate, in the grouping browsers show it in.
    pub fingerprint: String,
}

/// Read a PEM certificate chain and its key.
///
/// Errors name the file and what was wrong with it. This runs at startup, under a service
/// control manager, where the alternative to a sentence somebody can act on is a service
/// that does not start and a log that says so in hex.
pub fn load(cert: &Path, key: &Path) -> Result<Certificate> {
    install_tls_provider();
    let chain: Vec<CertificateDer<'static>> = CertificateDer::pem_file_iter(cert)
        .map_err(|e| {
            Error::Config(format!(
                "cannot read the certificate {}: {e}",
                cert.display()
            ))
        })?
        .collect::<std::result::Result<_, _>>()
        .map_err(|e| {
            Error::Config(format!(
                "cannot read the certificate {}: {e}",
                cert.display()
            ))
        })?;
    let leaf = chain.first().cloned().ok_or_else(|| {
        Error::Config(format!(
            "{} holds no certificate; a PEM file with a private key in it is not the \
             certificate, it is the --tls-key",
            cert.display()
        ))
    })?;
    let key_der = PrivateKeyDer::from_pem_file(key)
        .map_err(|e| Error::Config(format!("cannot read the key {}: {e}", key.display())))?;
    let mut config = ServerConfig::builder()
        .with_no_client_auth()
        .with_single_cert(chain, key_der)
        .map_err(|e| {
            Error::Config(format!(
                "the certificate and the key do not go together ({e}). {} and {} have to be \
                 the pair that was issued together.",
                cert.display(),
                key.display()
            ))
        })?;
    // HTTP/2 first, because a delivery is one long POST and the client asks for it. Both are
    // offered: a browser on a network that mangles h2 still gets the page.
    config.alpn_protocols = vec![b"h2".to_vec(), b"http/1.1".to_vec()];
    Ok(Certificate {
        config: Arc::new(config),
        fingerprint: fingerprint(&leaf),
    })
}

/// `AB:CD:…`, uppercase and in pairs, because that is how a browser shows it and the whole
/// point of printing it is that somebody compares the two by eye.
pub fn fingerprint(cert: &CertificateDer<'_>) -> String {
    use sha2::Digest;
    let digest = sha2::Sha256::digest(cert.as_ref());
    digest
        .iter()
        .map(|b| format!("{b:02X}"))
        .collect::<Vec<_>>()
        .join(":")
}

/// Serve until `stop` resolves, then let what is in flight finish.
///
/// `make` is the router with connect info, exactly as `axum::serve` would have taken it: the
/// page's rule about being at the machine is a check on the peer address, so a connection
/// that arrives without one would quietly widen it.
pub async fn serve(
    listener: tokio::net::TcpListener,
    mut make: axum::extract::connect_info::IntoMakeServiceWithConnectInfo<
        axum::Router,
        std::net::SocketAddr,
    >,
    cert: Certificate,
    stop: impl std::future::Future<Output = ()> + Send + 'static,
) -> Result<()> {
    use hyper_util::rt::{TokioExecutor, TokioIo};
    use hyper_util::service::TowerToHyperService;
    use tower::Service;

    let acceptor = tokio_rustls::TlsAcceptor::from(cert.config);
    // Every connection task holds a clone. When the loop ends we drop ours and wait for the
    // channel to close, which happens when the last task is gone: a connection counter that
    // cannot drift, because nothing has to remember to decrement it.
    let (alive, mut all_gone) = tokio::sync::mpsc::channel::<()>(1);
    let mut stop = std::pin::pin!(stop);

    loop {
        let (stream, peer) = tokio::select! {
            accepted = listener.accept() => match accepted {
                Ok(pair) => pair,
                // Out of file descriptors, or a connection that died between the queue and
                // us. Neither is a reason to stop collecting from everybody else.
                Err(e) => {
                    super::service::log(&format!("accept failed: {e}"));
                    continue;
                }
            },
            () = &mut stop => break,
        };
        // Built here, where the peer is known; the handshake below is what moves to the task.
        let svc = match make.call(peer).await {
            Ok(svc) => svc,
            Err(never) => match never {},
        };
        let acceptor = acceptor.clone();
        let alive = alive.clone();
        tokio::spawn(async move {
            let _alive = alive;
            let tls = match acceptor.accept(stream).await {
                Ok(tls) => tls,
                // A scanner, a browser that was shown a warning and left, a client with the
                // wrong certificate. Logged at all because "my client cannot connect" is
                // otherwise unanswerable from the hub's side.
                Err(e) => {
                    super::service::log(&format!("TLS handshake with {peer} failed: {e}"));
                    return;
                }
            };
            let served = hyper_util::server::conn::auto::Builder::new(TokioExecutor::new())
                .serve_connection_with_upgrades(TokioIo::new(tls), TowerToHyperService::new(svc))
                .await;
            if let Err(e) = served {
                super::service::log(&format!("connection from {peer} ended: {e}"));
            }
        });
    }

    super::service::log("stop requested; closing the listener");
    drop(listener);
    drop(alive);
    // The drain is bounded on purpose. A connection that will not end must not be the reason
    // a machine cannot be shut down or a service cannot be upgraded.
    if tokio::time::timeout(DRAIN, all_gone.recv()).await.is_err() {
        super::service::log(&format!(
            "still serving after {}s; going anyway",
            DRAIN.as_secs()
        ));
    }
    Ok(())
}
