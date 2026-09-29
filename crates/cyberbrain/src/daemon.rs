//! A resident process that keeps the model loaded and answers `recall`, `write` and a
//! `scan` with work to do over a local socket.
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
//! - The socket is `0600`. The client's actor is taken as sent, except that the daemon reads
//!   the connecting process's own environment (`SO_PEERCRED`, then `/proc/<pid>/environ`) and
//!   a process started as an agent is an agent whatever it claims (2026-09-29; before, a
//!   Python script at the socket that sent `"actor":"operator"` wrote ring 0). That is the
//!   same trust the CLI's environment gets, now checked rather than asserted.
//!   `CYBERBRAIN_NO_DAEMON=1` turns all of this off.
//!
//! Unix only. Elsewhere the client always answers locally and `daemon` refuses to start.

use crate::app::{App, RecallRequest, WriteRequest};
use cyberbrain_core::{Error, NoteKind, Result};
use cyberbrain_policy::Actor;
use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};

pub const SOCKET: &str = "daemon.sock";
/// 2: `op` and `write` (2026-09-25). 3: `scan` (2026-09-25). 4: validity — `at` on a
/// recall, `supersedes`, `valid_from` and `invalid_at` on a write (2026-09-27).
///
/// A daemon refuses a version it does not know before doing anything, and the client then
/// does the work itself. So a client sends the version it speaks, and a 3-daemon refuses a
/// 4-request rather than carry out a write without the bounds it asked for. A daemon
/// accepts every version from [`OLDEST_PROTOCOL`] on: a request of an older version is
/// one without the newer fields, and means exactly what it meant there. That lets a CLI or
/// hook still on 0.6.x be answered by a 0.7 daemon while the binaries are swapped.
const PROTOCOL: u32 = 4;
/// The oldest request version a daemon still carries out.
#[cfg_attr(not(unix), allow(dead_code))]
const OLDEST_PROTOCOL: u32 = 3;
/// A path longer than this does not fit `sockaddr_un` on every platform.
const MAX_SOCKET_PATH: usize = 100;
/// The server reads at most this much of a request line; a longer body is written locally.
const MAX_REQUEST: usize = 1 << 20;

#[derive(Debug, Serialize, Deserialize)]
struct Request {
    v: u32,
    op: Op,
    actor: String,
    /// `--json`: the answer is rendered as the CLI renders it under that flag.
    json: bool,
}

#[derive(Debug, Serialize, Deserialize)]
#[serde(tag = "op", rename_all = "kebab-case")]
enum Op {
    Recall {
        query: String,
        n: Option<usize>,
        ring: Option<u8>,
        bereich: Option<String>,
        /// `--stand`: judge validity as of this moment. Protocol 4; absent is now.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        at: Option<jiff::Timestamp>,
    },
    Write(WriteArgs),
    Scan {
        full: bool,
        dry_run: bool,
    },
}

/// A `write` as the CLI builds it: no operator choice, no expected timestamp, nothing
/// arriving from a hub. Those belong to the UI and the hub, which do not go through here.
#[derive(Debug, Serialize, Deserialize)]
struct WriteArgs {
    ring: u8,
    kind: NoteKind,
    name: String,
    body: String,
    tags: Vec<String>,
    bereich: Option<String>,
    retention: Option<String>,
    force: bool,
    dry_run: bool,
    /// Protocol 4. Absent keeps what the note has, as in [`WriteRequest`].
    #[serde(default, skip_serializing_if = "Option::is_none")]
    supersedes: Option<Vec<String>>,
    /// Protocol 4. Three states, as in [`WriteRequest`]: absent keeps what the note has,
    /// `null` removes it, a timestamp sets it.
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        deserialize_with = "present"
    )]
    valid_from: Option<Option<jiff::Timestamp>>,
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        deserialize_with = "present"
    )]
    invalid_at: Option<Option<jiff::Timestamp>>,
}

/// A key that is there, even as `null`, is `Some`: `null` is "remove it", which is not the
/// same request as leaving the key out. (serde alone reads `null` as the outer `None`.)
fn present<'de, D, T>(d: D) -> std::result::Result<Option<Option<T>>, D::Error>
where
    D: serde::Deserializer<'de>,
    T: Deserialize<'de>,
{
    Option::<T>::deserialize(d).map(Some)
}

#[derive(Debug, Default, Serialize, Deserialize)]
#[cfg_attr(not(unix), allow(dead_code))] // only a Unix daemon answers
struct Response {
    /// Set when the daemon carried the request out: the process's exit code. Then `stdout`
    /// and `stderr` are exactly what the CLI would have printed. Unset: it did nothing, and
    /// `reason` says why.
    code: Option<i32>,
    stdout: Option<String>,
    stderr: Option<String>,
    reason: Option<String>,
}

/// What became of a request sent to the daemon.
pub enum Answer {
    /// Carried out; print this and exit with `code`.
    Done {
        stdout: Option<String>,
        stderr: Option<String>,
        code: i32,
    },
    /// Not carried out (no daemon, a stale one, a version mismatch): do it locally.
    Local,
}

fn socket_path(root: &Path) -> Option<PathBuf> {
    let p = root.join(SOCKET);
    if p.as_os_str().len() <= MAX_SOCKET_PATH {
        return Some(p);
    }
    // A store deeper than a socket path can reach used to mean no daemon at all, silently:
    // every recall paid 1.5 s instead of 8 ms and neither `status` nor `doctor` said so
    // (2026-09-29). The socket goes to a directory of this user's own instead, named by a hash
    // of the store's path. Never /tmp: another account could create the name first and read
    // what the client sends it.
    let dir = std::env::var_os("XDG_RUNTIME_DIR")
        .map(PathBuf::from)
        .filter(|d| d.is_absolute())
        .map(|d| d.join("cyberbrain"))
        .or_else(|| {
            std::env::var_os("HOME")
                .map(PathBuf::from)
                .map(|h| h.join(".cache").join("cyberbrain"))
        })?;
    let root = std::fs::canonicalize(root).unwrap_or_else(|_| root.to_path_buf());
    let tag = blake3::hash(root.as_os_str().as_encoded_bytes()).to_hex();
    let p = dir.join(format!("{}.sock", &tag[..16]));
    (p.as_os_str().len() <= MAX_SOCKET_PATH).then_some(p)
}

/// For `status`: whether a daemon answers for this store, in one line.
pub fn state(root: &Path) -> String {
    if disabled() {
        return "off (CYBERBRAIN_NO_DAEMON is set); every command loads the model itself".into();
    }
    let Some(path) = socket_path(root) else {
        return "unavailable: no socket path short enough; every command loads the model itself"
            .into();
    };
    #[cfg(unix)]
    {
        if std::os::unix::net::UnixStream::connect(&path).is_ok() {
            return format!("running at {}", cyberbrain_core::Slash(&path));
        }
        format!(
            "not running (socket {}); the next command that needs the model starts one",
            cyberbrain_core::Slash(&path)
        )
    }
    #[cfg(not(unix))]
    {
        let _ = path;
        "not available on this platform; every command loads the model itself".into()
    }
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

/// What the CLI prints for a value, byte for byte (`Out::emit`).
#[cfg_attr(not(unix), allow(dead_code))]
fn rendered<T: Serialize>(
    value: &T,
    json: bool,
    human: impl FnOnce(&T) -> String,
) -> Result<String> {
    if json {
        serde_json::to_string_pretty(value)
            .map_err(|e| Error::Index(format!("report does not serialise: {e}")))
    } else {
        Ok(human(value))
    }
}

// ---------------------------------------------------------------------------------------
// Client

/// Asks the store's daemon for a recall. `Some(text)` is the finished output; `None` means
/// answer locally (and, if no daemon was there, one has been started for next time). A
/// recall only reads, so anything short of a clean answer is simply done again here.
pub fn recall(
    root: &Path,
    actor: &Actor,
    query: &str,
    req: &RecallRequest,
    json: bool,
) -> Option<String> {
    // `at` needs protocol 4; a daemon that does not speak it refuses the request as a
    // whole, and this recall is then answered here, as of the day asked.
    let op = Op::Recall {
        query: query.to_string(),
        n: req.n,
        ring: req.ring.map(|r| r.as_u8()),
        bereich: req.bereich.clone(),
        at: req.at,
    };
    match send(root, actor, op, json)? {
        Sent::Answered(Response {
            code: Some(0),
            stdout,
            ..
        }) => stdout,
        _ => None,
    }
}

/// Asks the store's daemon to carry out a write.
///
/// A write is not repeated locally once the daemon may have done it: that would write the
/// note twice and put a second row in the audit log (a second refusal, for a ring 0
/// write). So the only way back to the local path is a daemon that said it did nothing, or
/// none at all. A request that went out and got no readable answer is an error.
pub fn write(root: &Path, actor: &Actor, req: &WriteRequest, json: bool) -> Result<Answer> {
    let what = format!("the write of {}", req.name);
    // `supersedes` and the validity bounds travel since protocol 4. What the CLI never
    // sets (an operator's choice, an expected timestamp, a note arriving from a hub) is not
    // part of the protocol, so such a write stays local rather than arriving without it.
    if req.body.len() > MAX_REQUEST / 2
        || req.choice.is_some()
        || req.expected_updated.is_some()
        || req.arriving.is_some()
    {
        return Ok(Answer::Local);
    }
    let op = Op::Write(WriteArgs {
        ring: req.ring.as_u8(),
        kind: req.kind,
        name: req.name.clone(),
        body: req.body.clone(),
        tags: req.tags.clone(),
        bereich: req.bereich.clone().flatten(),
        retention: req.retention.clone().flatten(),
        force: req.force,
        dry_run: req.dry_run,
        supersedes: req.supersedes.clone(),
        valid_from: req.valid_from,
        invalid_at: req.invalid_at,
    });
    once(
        send(root, actor, op, json),
        &what,
        "Check with `cyberbrain recall` before writing it again",
    )
}

/// Asks the store's daemon to carry out a scan. The same rule as [`write`]: a scan that
/// may have run there is not run again here, so it is never done twice and never
/// reported twice in the audit log.
pub fn scan(root: &Path, actor: &Actor, full: bool, dry_run: bool, json: bool) -> Result<Answer> {
    once(
        send(root, actor, Op::Scan { full, dry_run }, json),
        "the scan",
        "`cyberbrain doctor` says whether the index still differs from the files",
    )
}

/// The answer to a request that must not be carried out twice.
fn once(sent: Option<Sent>, what: &str, check: &str) -> Result<Answer> {
    match sent {
        None => Ok(Answer::Local),
        Some(Sent::Answered(Response {
            code: Some(code),
            stdout,
            stderr,
            ..
        })) => Ok(Answer::Done {
            stdout,
            stderr,
            code,
        }),
        Some(Sent::Answered(_)) => Ok(Answer::Local),
        Some(Sent::Lost) => Err(Error::Index(format!(
            "{what} went to the background daemon, which did not answer; it may or may not \
             have happened. {check}"
        ))),
    }
}

#[cfg_attr(not(unix), allow(dead_code))] // only a Unix daemon answers
enum Sent {
    Answered(Response),
    /// Sent, and no readable answer came back.
    Lost,
}

/// `None`: nothing was sent (disabled, no socket path, no daemon — one is started).
fn send(root: &Path, actor: &Actor, op: Op, json: bool) -> Option<Sent> {
    if disabled() {
        return None;
    }
    let path = socket_path(root)?;
    client::ask(
        root,
        &path,
        &Request {
            v: PROTOCOL,
            op,
            actor: actor.to_string(),
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

    pub(super) fn ask(root: &Path, path: &Path, req: &Request) -> Option<Sent> {
        let Ok(mut stream) = UnixStream::connect(path) else {
            start(root);
            return None;
        };
        // Longer than a recall with its contradiction budget, shorter than patience. A scan
        // embeds every changed note, a `--full` one the whole store (5.5 s for 1,495 notes,
        // measured); it gets ten minutes before "it may or may not have happened".
        let wait = match req.op {
            Op::Scan { .. } => 600,
            _ => 60,
        };
        let _ = stream.set_read_timeout(Some(Duration::from_secs(wait)));
        let _ = stream.set_write_timeout(Some(Duration::from_secs(5)));
        let mut line = serde_json::to_string(req).ok()?;
        line.push('\n');
        // A request that could not be sent in full was not carried out: the daemon acts
        // only on a complete line.
        stream.write_all(line.as_bytes()).ok()?;
        let mut answer = String::new();
        if BufReader::new(stream).read_line(&mut answer).is_err() {
            return Some(Sent::Lost);
        }
        Some(
            serde_json::from_str::<Response>(&answer)
                .map(Sent::Answered)
                .unwrap_or(Sent::Lost),
        )
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
    pub(super) fn ask(_: &Path, _: &Path, _: &Request) -> Option<Sent> {
        None
    }
}

/// Whether a daemon carries out a request of version `v`.
#[cfg_attr(not(unix), allow(dead_code))]
fn accepts(v: u32) -> bool {
    (OLDEST_PROTOCOL..=PROTOCOL).contains(&v)
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
        if let Some(dir) = socket_path(base.root())
            .as_deref()
            .and_then(Path::parent)
            .filter(|d| !d.starts_with(base.root()))
        {
            use std::os::unix::fs::DirBuilderExt;
            let _ = std::fs::DirBuilder::new()
                .recursive(true)
                .mode(0o700)
                .create(dir);
        }
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

    /// The agent behind this connection, from the connecting process's environment as it was
    /// when that process started. `/proc/<pid>/environ` is fixed at exec: a script that
    /// removes `CLAUDECODE` from its own environment afterwards still shows it here.
    ///
    /// Linux only; elsewhere, and if the process cannot be read (it has already gone), the
    /// actor the client sent stands, which is where every client stood before this.
    #[cfg(target_os = "linux")]
    pub(super) fn peer_agent(stream: &UnixStream) -> Option<Actor> {
        use std::os::fd::AsRawFd;
        let mut cred = libc::ucred {
            pid: 0,
            uid: 0,
            gid: 0,
        };
        let mut len = std::mem::size_of::<libc::ucred>() as libc::socklen_t;
        // SAFETY: `cred` and `len` are valid for writes of the sizes given, and the fd is a
        // live socket owned by `stream` for the duration of the call.
        let rc = unsafe {
            libc::getsockopt(
                stream.as_raw_fd(),
                libc::SOL_SOCKET,
                libc::SO_PEERCRED,
                (&raw mut cred).cast(),
                &raw mut len,
            )
        };
        if rc != 0 || cred.pid <= 0 {
            return None;
        }
        let raw = std::fs::read(format!("/proc/{}/environ", cred.pid)).ok()?;
        let vars: std::collections::HashMap<String, String> = raw
            .split(|b| *b == 0)
            .filter_map(|kv| {
                let kv = String::from_utf8_lossy(kv);
                kv.split_once('=')
                    .map(|(k, v)| (k.to_string(), v.to_string()))
            })
            .collect();
        crate::agent_from_env(|k| vars.get(k).cloned())
    }

    #[cfg(not(target_os = "linux"))]
    pub(super) fn peer_agent(_stream: &UnixStream) -> Option<Actor> {
        None
    }

    fn handle(s: &Shared, stream: UnixStream) {
        s.last.store(now_secs(), Ordering::Relaxed);
        let peer = peer_agent(&stream);
        let _ = stream.set_read_timeout(Some(Duration::from_secs(10)));
        let mut line = String::new();
        let Ok(reader) = stream.try_clone() else {
            return;
        };
        if BufReader::new(reader.take(MAX_REQUEST as u64))
            .read_line(&mut line)
            .is_err()
        {
            return;
        }
        let stale = s.stale();
        let response = if stale {
            not_done("stale: the binary, the configuration or the model changed")
        } else if !line.ends_with('\n') {
            // Cut off at MAX_REQUEST or by a client that went away: never act on half a
            // request, least of all half a write.
            not_done("incomplete request")
        } else {
            answer(s, &line, peer)
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

    fn not_done(reason: &str) -> Response {
        Response {
            reason: Some(reason.to_string()),
            ..Response::default()
        }
    }

    /// Carries a request out and answers the way the CLI would have: stdout, the stderr
    /// line of an error, the exit code. Only a request that cannot be read is "not done".
    fn answer(s: &Shared, line: &str, peer: Option<Actor>) -> Response {
        let mut req: Request = match serde_json::from_str(line) {
            Ok(r) => r,
            Err(e) => return not_done(&format!("not a daemon request: {e}")),
        };
        if !accepts(req.v) {
            return not_done(&format!(
                "protocol {} is not one of {OLDEST_PROTOCOL}..={PROTOCOL}",
                req.v
            ));
        }
        if let Some(agent) = peer {
            req.actor = agent.to_string();
        }
        let json = req.json;
        match carry_out(s, req) {
            Ok((stdout, code)) => Response {
                code: Some(code),
                stdout: Some(stdout),
                ..Response::default()
            },
            Err(Failed { stdout, error }) => Response {
                code: Some(error.exit_code()),
                stdout,
                stderr: Some(crate::error_text(&error, json)),
                ..Response::default()
            },
        }
    }

    /// An error, with whatever the CLI had already printed before it (a write conflict).
    struct Failed {
        stdout: Option<String>,
        error: Error,
    }

    impl From<Error> for Failed {
        fn from(error: Error) -> Self {
            Failed {
                stdout: None,
                error,
            }
        }
    }

    fn carry_out(s: &Shared, req: Request) -> std::result::Result<(String, i32), Failed> {
        let app = s.app_for(&req.actor)?;
        match req.op {
            Op::Recall {
                query,
                n,
                ring,
                bereich,
                at,
            } => {
                let ring = ring.map(cyberbrain_core::Ring::try_from).transpose()?;
                let rr = RecallRequest {
                    n,
                    ring,
                    bereich,
                    at,
                };
                let rt = tokio::runtime::Builder::new_current_thread()
                    .enable_all()
                    .build()
                    .map_err(|e| Error::Index(format!("cannot start the async runtime: {e}")))?;
                let result = rt.block_on(app.recall(&query, &rr))?;
                Ok((rendered(&result, req.json, crate::render::recall)?, 0))
            }
            Op::Write(w) => {
                let wr = WriteRequest {
                    ring: cyberbrain_core::Ring::try_from(w.ring)?,
                    kind: w.kind,
                    name: w.name,
                    body: w.body,
                    tags: w.tags,
                    bereich: w.bereich.map(Some),
                    retention: w.retention.map(Some),
                    force: w.force,
                    choice: None,
                    expected_updated: None,
                    supersedes: w.supersedes,
                    valid_from: w.valid_from,
                    invalid_at: w.invalid_at,
                    arriving: None,
                    dry_run: w.dry_run,
                };
                let outcome = app.write(wr)?;
                let stdout = rendered(&outcome, req.json, crate::render_write)?;
                match crate::write_exit(&outcome) {
                    Ok(code) => Ok((stdout, code)),
                    Err(error) => Err(Failed {
                        stdout: Some(stdout),
                        error,
                    }),
                }
            }
            Op::Scan { full, dry_run } => {
                let report = app.scan(crate::app::ScanOptions { full, dry_run })?;
                Ok((rendered(&report, req.json, crate::render::scan)?, 0))
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The daemon asks the connecting process, not the request. Over a socket pair inside this
    /// test the peer is this very process, so the answer has to be whatever this process's
    /// own environment says: an agent when run from Claude Code, the operator otherwise.
    #[cfg(target_os = "linux")]
    #[test]
    fn the_peer_is_judged_by_its_own_environment() {
        let (a, _b) = std::os::unix::net::UnixStream::pair().unwrap();
        let expected = crate::agent_from_env(|k| std::env::var(k).ok());
        assert_eq!(
            super::server::peer_agent(&a).map(|x| x.to_string()),
            expected.map(|x| x.to_string())
        );
    }

    #[test]
    fn the_actor_round_trips_through_its_name() {
        for a in [Actor::Operator, Actor::Agent("claude-code:8c734a31".into())] {
            assert_eq!(actor_from(&a.to_string()).to_string(), a.to_string());
        }
    }

    /// What a 0.6.x client sends: protocol 3, none of the newer keys. It is carried out
    /// and means what it meant, so the binaries need not be swapped in one instant.
    #[test]
    fn a_protocol_3_request_is_still_understood() {
        let line = r#"{"v":3,"op":{"op":"write","ring":2,"kind":"knowledge","name":"x","body":"y","tags":[],"bereich":null,"retention":null,"force":false,"dry_run":false},"actor":"operator","json":true}"#;
        let req: Request = serde_json::from_str(line).unwrap();
        assert!(accepts(req.v));
        let Op::Write(w) = req.op else {
            panic!("a write")
        };
        assert_eq!(
            (w.supersedes, w.valid_from, w.invalid_at),
            (None, None, None),
            "absent keeps what the note has"
        );
        let line = r#"{"v":3,"op":{"op":"recall","query":"q","n":null,"ring":null,"bereich":null},"actor":"operator","json":false}"#;
        let req: Request = serde_json::from_str(line).unwrap();
        assert!(matches!(req.op, Op::Recall { at: None, .. }));
        assert!(!accepts(2) && !accepts(PROTOCOL + 1));
    }

    /// The three states of a bound survive the socket: absent, `null` (remove), a value.
    #[test]
    fn validity_keeps_its_three_states_on_the_wire() {
        let t: jiff::Timestamp = "2026-09-01T00:00:00Z".parse().unwrap();
        for (from, to) in [
            (None, None),
            (Some(None), Some(Some(t))),
            (Some(Some(t)), Some(None)),
        ] {
            let args = WriteArgs {
                ring: 2,
                kind: NoteKind::Knowledge,
                name: "x".into(),
                body: "y".into(),
                tags: Vec::new(),
                bereich: None,
                retention: None,
                force: false,
                dry_run: false,
                supersedes: Some(vec!["alt".into()]),
                valid_from: from,
                invalid_at: to,
            };
            let line = serde_json::to_string(&Op::Write(args)).unwrap();
            let Op::Write(back) = serde_json::from_str(&line).unwrap() else {
                panic!("a write")
            };
            assert_eq!((back.valid_from, back.invalid_at), (from, to), "{line}");
            assert_eq!(back.supersedes, Some(vec!["alt".to_string()]));
        }
    }

    /// A deep store used to get no daemon at all. Its socket now lives in a directory of this
    /// user's own, short enough to bind, and never in the store or in /tmp.
    #[test]
    fn a_store_too_deep_for_a_socket_gets_one_elsewhere() {
        assert_eq!(
            socket_path(Path::new("/s")),
            Some(PathBuf::from("/s").join(SOCKET))
        );
        let deep = PathBuf::from("/".to_string() + &"x".repeat(120));
        match socket_path(&deep) {
            Some(p) => {
                assert!(p.as_os_str().len() <= MAX_SOCKET_PATH, "{p:?}");
                assert!(!p.starts_with(&deep) && !p.starts_with("/tmp"), "{p:?}");
                assert_eq!(
                    socket_path(&deep),
                    Some(p),
                    "the same store, the same socket"
                );
            }
            // Neither XDG_RUNTIME_DIR nor HOME: nowhere of this user's own to put it.
            None => assert!(
                std::env::var_os("XDG_RUNTIME_DIR").is_none() && std::env::var_os("HOME").is_none()
            ),
        }
    }
}
