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
//! # One fingerprint, and what may be pinned
//!
//! The fingerprint is SHA-256 over the certificate: the number a browser shows, the number
//! `openssl x509 -fingerprint -sha256` prints, the number in an invitation. There was a
//! version of this that pinned the public key instead, so that renewing a certificate for the
//! same key would leave every enrolled machine working. Pinning the key means a client has to
//! read the public key out of the certificate it was shown, which means an X.509 parser in
//! `cyberbrain-policy` — the one crate whose job is to have less in it, not more. The cost of
//! the simpler choice is bounded because of what may be pinned:
//!
//! - **A certificate the hub made itself** is pinned. It is made once and never replaced
//!   (see [`ensure_self_signed`]), so there is no renewal to break. Replacing it is deleting
//!   two files, which nobody does by accident, and the invitations then have to be reissued.
//! - **A certificate the operator supplied** is never pinned. It comes from a certificate
//!   authority, the machine's own trust store already knows it, and it may be renewed as
//!   often as its issuer likes without anything here caring.
//!
//! What falls between is a self-signed certificate the operator made by hand: trusted by
//! nobody and pinned by nobody, so clients will refuse it. Let the hub make its own.
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
    /// Whether this is the hub's own certificate, and may therefore be pinned in invitations.
    /// A certificate that came from the operator is not: it has an issuer, and issuers renew.
    pub pinnable: bool,
}

/// Read the certificate the operator supplied.
///
/// Not pinnable: it belongs to a certificate authority that will renew it, and a pin would
/// turn that renewal into every client on the network stopping at once.
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
        pinnable: false,
    })
}

/// The hub's own certificate: made if it is not there, then loaded like any other.
pub fn own(dir: &Path, names: &[String]) -> Result<Certificate> {
    let (cert, key) = ensure_self_signed(dir, names)?;
    Ok(Certificate {
        pinnable: true,
        ..load(&cert, &key)?
    })
}

/// A certificate named by two paths, which is how a service registration carries one.
///
/// The paths are the only thing there is to go on, so this is where the question of whose
/// certificate it is gets asked: the same call answers it for a company CA's certificate and
/// for the pair the installer made two minutes ago.
pub fn named(dir: &Path, cert: &Path, key: &Path) -> Result<Certificate> {
    Ok(Certificate {
        pinnable: is_own_pair(dir, cert, key),
        ..load(cert, key)?
    })
}

/// Whether these two files are the pair this hub made for itself.
///
/// It has to be asked, because a certificate arrives as two paths and nothing else. The
/// Windows installer makes the pair and then registers the service with `--tls-cert` and
/// `--tls-key` pointing at it, so the hub that starts sees exactly what it would see for a
/// certificate from a company CA. Without this it treats its own certificate as somebody
/// else's, issues invitations with no pin, and every client then refuses a certificate
/// nobody told them to expect. The answer is the convention: this pair, these names, beside
/// the record.
pub fn is_own_pair(dir: &Path, cert: &Path, key: &Path) -> bool {
    let same = |a: &Path, b: &Path| match (a.canonicalize(), b.canonicalize()) {
        (Ok(a), Ok(b)) => a == b,
        // On a path that cannot be resolved, compare what we were given. Being wrong here
        // means no pin, which is the safe direction to be wrong in.
        _ => a == b,
    };
    same(cert, &dir.join(OWN_CERT)) && same(key, &dir.join(OWN_KEY))
}

/// The hub's own certificate, made once and kept beside the record.
///
/// # Why the hub makes one at all
///
/// The customer this is for has no certificate authority, and telling them to obtain a
/// certificate is telling them to stay in plain text. A certificate nobody trusts still
/// encrypts the wire, and the pin in the invitation is what turns "nobody trusts it" into
/// "the machines that were invited trust exactly this one".
///
/// # Why it is not regenerated
///
/// The pin in every invitation ever issued is the public key in this file. Making a new one
/// because a hostname changed would silently stop every enrolled machine from delivering, so
/// an existing pair is used as it is; replacing it is deleting the two files, which is a
/// thing somebody does on purpose.
pub fn ensure_self_signed(
    dir: &Path,
    names: &[String],
) -> Result<(std::path::PathBuf, std::path::PathBuf)> {
    let cert_path = dir.join(OWN_CERT);
    let key_path = dir.join(OWN_KEY);
    if cert_path.exists() && key_path.exists() {
        return Ok((cert_path, key_path));
    }
    if cert_path.exists() != key_path.exists() {
        return Err(Error::Config(format!(
            "one half of the hub's certificate is missing: {} and {} come as a pair. \
             Delete the one that is left to have a new pair made, or put the other one back.",
            cert_path.display(),
            key_path.display()
        )));
    }
    // rcgen's own validity is 1975 to 4096 and it is left alone. A certificate that expires
    // would stop a hub years later for a reason nobody is looking for, and the thing that
    // says which hub this is here is the pin, not a date somebody has to renew.
    let mut params = rcgen::CertificateParams::new(names.to_vec())
        .map_err(|e| Error::Config(format!("cannot build a certificate for {names:?}: {e}")))?;
    params.distinguished_name = rcgen::DistinguishedName::new();
    params
        .distinguished_name
        .push(rcgen::DnType::CommonName, "Cyberbrain Hub");
    let key = rcgen::KeyPair::generate()
        .map_err(|e| Error::Config(format!("cannot generate a key: {e}")))?;
    let cert = params
        .self_signed(&key)
        .map_err(|e| Error::Config(format!("cannot sign the certificate: {e}")))?;
    std::fs::write(&cert_path, cert.pem()).map_err(|e| Error::Io {
        path: cert_path.clone(),
        source: e,
    })?;
    write_private(&key_path, &key.serialize_pem()).map_err(|e| Error::Io {
        path: key_path.clone(),
        source: e,
    })?;
    Ok((cert_path, key_path))
}

/// The names a hub's own certificate should carry.
///
/// Its own name first, because that is what the page suggests for invitations and what a
/// browser will be pointed at. `localhost` and the loopback address are there for the
/// administrator sitting at the machine, and the bound address when it is a real one — a
/// wildcard bind is not a name and would only produce a certificate for `0.0.0.0`.
pub fn names_for(addr: &std::net::SocketAddr) -> Vec<String> {
    let mut names = vec![
        super::page::hostname(),
        "localhost".to_string(),
        "127.0.0.1".to_string(),
    ];
    if !addr.ip().is_unspecified() && !addr.ip().is_loopback() {
        names.push(addr.ip().to_string());
    }
    names.dedup();
    names
}

/// Write a private key so that only this machine's administrators can read it.
///
/// The permissions are part of creating the file, not something done to it afterwards:
/// between a create and a chmod the key is readable by everybody, and that window is on the
/// one file that must never be.
///
/// Measured rather than assumed. On Windows the hub's key inherited the ACL of
/// `C:\ProgramData`, which grants `BUILTIN\Users` read; `icacls` on a real install said
/// `(I)(RX)`. Anybody who can log in to the hub could read its key and then be the hub, to
/// every machine that was invited to expect exactly that certificate.
fn write_private(path: &Path, contents: &str) -> std::io::Result<()> {
    use std::io::Write;
    let mut opts = std::fs::OpenOptions::new();
    // `create_new` rather than `create`: this is only ever called for a key that does not
    // exist yet, and it closes the race with anything that put a file there first.
    opts.write(true).create_new(true);

    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        opts.mode(0o600);
    }

    #[cfg(windows)]
    {
        use windows_sys::Win32::Security::Authorization::ConvertStringSecurityDescriptorToSecurityDescriptorW;
        use windows_sys::Win32::Security::SECURITY_ATTRIBUTES;

        // `D:P` is a DACL that inherits nothing, which is the whole point: what it would
        // inherit is the read for Users. Then full access for the local system account and
        // the administrators group, by SID and not by name — the names are localised and
        // this has to work on a German Windows.
        let sddl: Vec<u16> = "D:P(A;;FA;;;SY)(A;;FA;;;BA)\0".encode_utf16().collect();
        let mut descriptor = std::ptr::null_mut();
        let built = unsafe {
            ConvertStringSecurityDescriptorToSecurityDescriptorW(
                sddl.as_ptr(),
                1, // SDDL_REVISION_1
                &mut descriptor,
                std::ptr::null_mut(),
            )
        };
        // The descriptor is never freed: windows-sys 0.61 no longer binds LocalFree, and
        // this is a few hundred bytes once, in a process that generates one key and then
        // runs for months. Said out loud rather than left to be discovered.
        if built != 0 {
            use std::os::windows::ffi::OsStrExt;
            use std::os::windows::io::FromRawHandle;
            use windows_sys::Win32::Foundation::INVALID_HANDLE_VALUE;
            use windows_sys::Win32::Storage::FileSystem::{
                CREATE_NEW, CreateFileW, FILE_ATTRIBUTE_NORMAL, FILE_GENERIC_WRITE,
            };

            // `std`'s own OpenOptions has no way to pass security attributes — the extension
            // trait offers flags and access modes and nothing for a descriptor — so the file
            // is created through the API that takes one. This is the only reason the write
            // is not four lines.
            let attributes = SECURITY_ATTRIBUTES {
                nLength: std::mem::size_of::<SECURITY_ATTRIBUTES>() as u32,
                lpSecurityDescriptor: descriptor,
                bInheritHandle: 0,
            };
            let wide: Vec<u16> = path.as_os_str().encode_wide().chain(Some(0)).collect();
            let handle = unsafe {
                CreateFileW(
                    wide.as_ptr(),
                    FILE_GENERIC_WRITE,
                    0, // shared with nobody while it is being written
                    &attributes,
                    CREATE_NEW,
                    FILE_ATTRIBUTE_NORMAL,
                    std::ptr::null_mut(),
                )
            };
            if handle == INVALID_HANDLE_VALUE {
                return Err(std::io::Error::last_os_error());
            }
            let mut f = unsafe { std::fs::File::from_raw_handle(handle as _) };
            return f.write_all(contents.as_bytes());
        }
        // If the descriptor could not be built the file is still written below, with the
        // inherited ACL. A hub that refuses to start over a file mode helps nobody, and
        // `hub cert show` is where that gap belongs.
    }

    let mut f = opts.open(path)?;
    f.write_all(contents.as_bytes())
}

/// The name the hub's own certificate is kept under, beside the record.
pub const OWN_CERT: &str = "hub-cert.pem";
/// And its key. Named here so that nothing else spells either of them out.
pub const OWN_KEY: &str = "hub-key.pem";

/// The fingerprint of a certificate file, without needing its key.
///
/// `hub cert show` runs while the hub is running, as a second process that has no business
/// reading the key — and on a hub started by the service control manager, may not be able to.
pub fn fingerprint_of(path: &Path) -> Result<String> {
    let leaf = CertificateDer::pem_file_iter(path)
        .map_err(|e| Error::Config(format!("cannot read {}: {e}", path.display())))?
        .next()
        .transpose()
        .map_err(|e| Error::Config(format!("cannot read {}: {e}", path.display())))?
        .ok_or_else(|| Error::Config(format!("{} holds no certificate", path.display())))?;
    Ok(fingerprint(&leaf))
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
