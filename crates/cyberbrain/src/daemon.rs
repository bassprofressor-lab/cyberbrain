//! A resident process that keeps the model loaded and answers `recall` over a local socket.
//!
//! Why it exists (measured 2026-09-25): after mapping the weights and caching their digest
//! (SPEC §6.5), a CLI `recall` still took 1.4 s, and 1.08 s of that was the tokenizers
//! crate building its Unigram model from 500,353 pieces — work it redoes on every load, from
//! any format. The search itself takes 2–9 ms. The only way to stop paying for the build is
//! not to repeat it.
//!
//! How it behaves:
//! - `recall` connects to `<store>/daemon.sock`. When nobody answers it starts
//!   `cyberbrain daemon` in its own process group and answers this one query itself.
//! - One JSON line each way. The daemon returns exactly what the CLI would have printed.
//!   Anything that is not a clean answer — a refused connection, a timeout, an error, a stale
//!   daemon — makes the client do the work itself, so errors and exit codes stay those of the
//!   local path.
//! - One `App` per actor, all sharing one loaded model: the audit log still names who asked.
//! - It leaves after `--idle-secs` without a request, and at once when the binary,
//!   `cyberbrain.toml` or the model manifest changes (answering the request that noticed
//!   with "stale", so the client runs it locally).
//! - The socket is `0600`; the client's actor is taken as sent, the same trust the CLI's
//!   own environment gets. `CYBERBRAIN_NO_DAEMON=1` turns all of this off.
//!
//! Unix only. Elsewhere the client always answers locally and `daemon` refuses to start.

use crate::app::{App, RecallRequest};
use crate::render;
use cyberbrain_core::{Error, Result};
use cyberbrain_policy::Actor;
use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};

pub const SOCKET: &str = "daemon.sock";
const PROTOCOL: u32 = 1;
/// A path longer than this does not fit `sockaddr_un` on every platform.
const MAX_SOCKET_PATH: usize = 100;

#[derive(Debug, Serialize, Deserialize)]
struct Request {
    v: u32,
    actor: String,
    query: String,
    n: Option<usize>,
    ring: Option<u8>,
    bereich: Option<String>,
    json: bool,
}

#[derive(Debug, Default, Serialize, Deserialize)]
#[cfg_attr(not(unix), allow(dead_code))] // only a Unix daemon answers
struct Response {
    /// What the CLI would print. `None`: do it yourself.
    output: Option<String>,
    /// Why there is no output, for the daemon's log and for tests.
    reason: Option<String>,
}

fn socket_path(root: &Path) -> Option<PathBuf> {
    let p = root.join(SOCKET);
    (p.as_os_str().len() <= MAX_SOCKET_PATH).then_some(p)
}

fn disabled() -> bool {
    std::env::var_os("CYBERBRAIN_NO_DAEMON").is_some_and(|v| !v.is_empty() && v != "0")
}

/// The actor as the daemon's audit log should name it. Only the two the CLI can be.
#[cfg_attr(not(unix), allow(dead_code))]
fn actor_from(s: &str) -> Actor {
    match s {
        "operator" => Actor::Operator,
        other => Actor::Agent(other.strip_prefix("agent:").unwrap_or(other).to_string()),
    }
}

/// What the CLI prints for a recall result, byte for byte (`Out::emit`).
#[cfg_attr(not(unix), allow(dead_code))]
fn rendered(r: &cyberbrain_core::RecallResult, json: bool) -> Result<String> {
    if json {
        serde_json::to_string_pretty(r)
            .map_err(|e| Error::Index(format!("report does not serialise: {e}")))
    } else {
        Ok(render::recall(r))
    }
}

// ---------------------------------------------------------------------------------------
// Client

/// Asks the store's daemon. `Some(text)` is the finished output; `None` means answer
/// locally (and, if no daemon was there, one has been started for next time).
pub fn recall(
    root: &Path,
    actor: &Actor,
    query: &str,
    req: &RecallRequest,
    json: bool,
) -> Option<String> {
    if disabled() {
        return None;
    }
    let path = socket_path(root)?;
    client::ask(
        root,
        &path,
        &Request {
            v: PROTOCOL,
            actor: actor.to_string(),
            query: query.to_string(),
            n: req.n,
            ring: req.ring.map(|r| r.as_u8()),
            bereich: req.bereich.clone(),
            json,
        },
    )
}

#[cfg(unix)]
mod client {
    use super::*;
    use std::io::{BufRead, BufReader, Write};
    use std::os::unix::net::UnixStream;
    use std::os::unix::process::CommandExt;
    use std::time::Duration;

    pub(super) fn ask(root: &Path, path: &Path, req: &Request) -> Option<String> {
        let Ok(mut stream) = UnixStream::connect(path) else {
            start(root);
            return None;
        };
        // Longer than a recall with its contradiction budget, shorter than patience.
        let _ = stream.set_read_timeout(Some(Duration::from_secs(60)));
        let _ = stream.set_write_timeout(Some(Duration::from_secs(5)));
        let mut line = serde_json::to_string(req).ok()?;
        line.push('\n');
        stream.write_all(line.as_bytes()).ok()?;
        let mut answer = String::new();
        BufReader::new(stream).read_line(&mut answer).ok()?;
        serde_json::from_str::<Response>(&answer).ok()?.output
    }

    /// Starts a daemon for this store and does not wait for it. Its own process group, so
    /// an interrupt meant for this command does not reach it; no inherited pipes, so a
    /// caller reading our output is not kept waiting for it.
    fn start(root: &Path) {
        let Ok(exe) = std::env::current_exe() else {
            return;
        };
        let _ = std::process::Command::new(exe)
            .arg("--store")
            .arg(root)
            .arg("daemon")
            .stdin(std::process::Stdio::null())
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null())
            .process_group(0)
            .spawn();
    }
}

#[cfg(not(unix))]
mod client {
    use super::*;
    pub(super) fn ask(_: &Path, _: &Path, _: &Request) -> Option<String> {
        None
    }
}

// ---------------------------------------------------------------------------------------
// Server

#[cfg(not(unix))]
pub fn serve(_base: App, _idle_secs: u64) -> Result<i32> {
    Err(Error::Config(
        "the daemon needs Unix sockets; this platform answers every recall locally".into(),
    ))
}

#[cfg(unix)]
pub fn serve(base: App, idle_secs: u64) -> Result<i32> {
    server::run(base, idle_secs)
}

#[cfg(unix)]
mod server {
    use super::*;
    use std::collections::HashMap;
    use std::io::{BufRead, BufReader, Read, Write};
    use std::os::unix::fs::PermissionsExt;
    use std::os::unix::net::{UnixListener, UnixStream};
    use std::sync::atomic::{AtomicU64, Ordering};
    use std::sync::{Arc, Mutex};
    use std::time::{Duration, SystemTime, UNIX_EPOCH};

    fn now_secs() -> u64 {
        SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map(|d| d.as_secs())
            .unwrap_or(0)
    }

    /// Length, mtime and inode of each watched file; a missing file is a stamp too.
    fn stamps(files: &[PathBuf]) -> Vec<Option<(u64, SystemTime, u64)>> {
        files
            .iter()
            .map(|f| {
                let m = std::fs::metadata(f).ok()?;
                Some((
                    m.len(),
                    m.modified().ok()?,
                    std::os::unix::fs::MetadataExt::ino(&m),
                ))
            })
            .collect()
    }

    struct Shared {
        base: Arc<App>,
        apps: Mutex<HashMap<String, Arc<App>>>,
        watched: Vec<PathBuf>,
        at_start: Vec<Option<(u64, SystemTime, u64)>>,
        socket: PathBuf,
        last: AtomicU64,
    }

    impl Shared {
        fn stale(&self) -> bool {
            stamps(&self.watched) != self.at_start
        }

        fn app_for(&self, actor: &str) -> Result<Arc<App>> {
            let mut apps = self
                .apps
                .lock()
                .map_err(|_| Error::Index("daemon lock".into()))?;
            if let Some(a) = apps.get(actor) {
                return Ok(a.clone());
            }
            let app = App::open(Some(self.base.root()), actor_from(actor))?;
            app.share_model_with(&self.base);
            let app = Arc::new(app);
            apps.insert(actor.to_string(), app.clone());
            Ok(app)
        }

        /// Removes the socket and exits. A successor cannot be holding it yet: one that
        /// started while this daemon was alive found the socket answering and left.
        fn leave(&self) -> ! {
            let _ = std::fs::remove_file(&self.socket);
            std::process::exit(0)
        }
    }

    pub(super) fn run(base: App, idle_secs: u64) -> Result<i32> {
        let Some(socket) = socket_path(base.root()) else {
            return Err(Error::Config(format!(
                "the store path is too long for a socket ({} > {MAX_SOCKET_PATH} bytes); \
                 recall stays in its own process",
                base.root().join(SOCKET).as_os_str().len()
            )));
        };
        let listener = match UnixListener::bind(&socket) {
            Ok(l) => l,
            Err(e) if e.kind() == std::io::ErrorKind::AddrInUse => {
                if UnixStream::connect(&socket).is_ok() {
                    return Ok(0); // another daemon got there first
                }
                // Left behind by a daemon that did not get to clean up.
                let _ = std::fs::remove_file(&socket);
                UnixListener::bind(&socket).map_err(|source| Error::Io {
                    path: socket.clone(),
                    source,
                })?
            }
            Err(source) => {
                return Err(Error::Io {
                    path: socket.clone(),
                    source,
                });
            }
        };
        let _ = std::fs::set_permissions(&socket, std::fs::Permissions::from_mode(0o600));

        let mut watched = base.watched_files();
        if let Ok(exe) = std::env::current_exe() {
            watched.push(exe);
        }
        let at_start = stamps(&watched);
        let shared = Arc::new(Shared {
            base: Arc::new(base),
            apps: Mutex::new(HashMap::new()),
            watched,
            at_start,
            socket,
            last: AtomicU64::new(now_secs()),
        });

        // Bound first, loaded second: a client that arrives meanwhile waits on the same
        // load instead of starting a second daemon.
        {
            let s = shared.clone();
            std::thread::spawn(move || s.base.preload_model());
        }
        {
            let s = shared.clone();
            std::thread::spawn(move || {
                loop {
                    std::thread::sleep(Duration::from_secs(idle_secs.clamp(1, 30)));
                    let idle = now_secs().saturating_sub(s.last.load(Ordering::Relaxed));
                    if idle >= idle_secs || s.stale() {
                        s.leave();
                    }
                }
            });
        }

        for stream in listener.incoming() {
            let Ok(stream) = stream else { continue };
            let s = shared.clone();
            std::thread::spawn(move || handle(&s, stream));
        }
        Ok(0)
    }

    fn handle(s: &Shared, stream: UnixStream) {
        s.last.store(now_secs(), Ordering::Relaxed);
        let _ = stream.set_read_timeout(Some(Duration::from_secs(10)));
        let mut line = String::new();
        let Ok(reader) = stream.try_clone() else {
            return;
        };
        if BufReader::new(reader.take(1 << 20))
            .read_line(&mut line)
            .is_err()
        {
            return;
        }
        let stale = s.stale();
        let response = if stale {
            Response {
                output: None,
                reason: Some("stale".into()),
            }
        } else {
            match answer(s, &line) {
                Ok(text) => Response {
                    output: Some(text),
                    reason: None,
                },
                Err(e) => Response {
                    output: None,
                    reason: Some(e.to_string()),
                },
            }
        };
        let mut out = serde_json::to_string(&response).unwrap_or_else(|_| "{}".into());
        out.push('\n');
        let mut w = stream;
        let _ = w.write_all(out.as_bytes());
        let _ = w.flush();
        s.last.store(now_secs(), Ordering::Relaxed);
        if stale {
            s.leave();
        }
    }

    fn answer(s: &Shared, line: &str) -> Result<String> {
        let req: Request = serde_json::from_str(line)
            .map_err(|e| Error::Config(format!("not a daemon request: {e}")))?;
        if req.v != PROTOCOL {
            return Err(Error::Config(format!(
                "protocol {} is not {PROTOCOL}",
                req.v
            )));
        }
        let app = s.app_for(&req.actor)?;
        let ring = req.ring.map(cyberbrain_core::Ring::try_from).transpose()?;
        let rr = RecallRequest {
            n: req.n,
            ring,
            bereich: req.bereich,
        };
        let rt = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .map_err(|e| Error::Index(format!("cannot start the async runtime: {e}")))?;
        let result = rt.block_on(app.recall(&req.query, &rr))?;
        rendered(&result, req.json)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_actor_round_trips_through_its_name() {
        for a in [Actor::Operator, Actor::Agent("claude-code:8c734a31".into())] {
            assert_eq!(actor_from(&a.to_string()).to_string(), a.to_string());
        }
    }

    #[test]
    fn a_path_too_long_for_a_socket_gets_no_daemon() {
        assert!(socket_path(Path::new("/s")).is_some());
        assert!(socket_path(&PathBuf::from("/".to_string() + &"x".repeat(120))).is_none());
    }
}
