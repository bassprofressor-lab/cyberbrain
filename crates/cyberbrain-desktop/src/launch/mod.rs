//! Starting `cyberbrain serve` and finding out where it landed.
//!
//! Everything here is deliberately platform-neutral, which is not tidiness for its own
//! sake: it means the interesting half of a Windows-only program can be tested on the
//! machine that builds it, against the real binary rather than a stand-in. The Windows
//! half — the tray, the job object, the dialogs — is in `win`.

use std::io::{BufRead, BufReader};
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::sync::mpsc;
use std::sync::{Arc, Mutex};
use std::time::Duration;

/// Name of the store directory, repeated here rather than depended on: this crate does not
/// link the rest of the workspace, and one string is a smaller price than that edge.
const STORE_DIR: &str = ".cyberbrain";

/// How long `serve` gets to print its address before we give up on it. Generous, because
/// the first start on a cold machine opens an index and a page bundle.
const START_TIMEOUT: Duration = Duration::from_secs(20);

/// Keep the tail of the child's output for the error dialog, not all of it.
const DIAG_LIMIT: usize = 8 * 1024;

#[derive(Debug)]
pub enum StartError {
    /// The chosen folder has no store. Recoverable, and the caller offers to create one.
    NoStore { dir: PathBuf },
    /// It started and said nothing we could use, or it died. Carries what it printed.
    Failed { message: String, output: String },
    /// The `cyberbrain` beside us was built without its web page. There is nothing for this
    /// program to open, so it says so instead of pointing a browser at a JSON error.
    NoPage,
}

impl std::fmt::Display for StartError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            StartError::NoStore { dir } => {
                write!(f, "no {STORE_DIR} store in {}", dir.display())
            }
            StartError::Failed { message, .. } => write!(f, "{message}"),
            StartError::NoPage => write!(
                f,
                concat!(
                    "this copy of cyberbrain was built without its web page ",
                    "(--no-default-features), and the page is the whole of what this ",
                    "launcher opens"
                )
            ),
        }
    }
}

/// A running `cyberbrain serve`, and the address it printed.
pub struct Server {
    child: Child,
    pub url: String,
    pub project_dir: PathBuf,
}

impl Server {
    /// Stop the server and wait for it. Called on quit and before a restart; on Windows the
    /// job object is the belt to this pair of braces, for the case where we are killed
    /// rather than asked.
    pub fn stop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
    }

    /// Whether the child is still running. `None` means still alive.
    pub fn exit_status(&mut self) -> Option<std::process::ExitStatus> {
        self.child.try_wait().ok().flatten()
    }

    /// The child's process handle, for the job object that outlives a kill.
    #[cfg(windows)]
    pub fn raw_handle(&self) -> windows_sys::Win32::Foundation::HANDLE {
        use std::os::windows::io::AsRawHandle;
        self.child.as_raw_handle() as windows_sys::Win32::Foundation::HANDLE
    }
}

impl Drop for Server {
    fn drop(&mut self) {
        self.stop();
    }
}

/// Where the `cyberbrain` binary is: next to us first, then whatever the path says.
///
/// Next to us first is the important half. The installer puts both binaries in one
/// directory, and a machine that also has an older `cyberbrain` on its PATH must still get
/// the one it just installed.
pub fn locate_server(exe_dir: Option<&Path>) -> Option<PathBuf> {
    let name = if cfg!(windows) {
        "cyberbrain.exe"
    } else {
        "cyberbrain"
    };
    if let Some(dir) = exe_dir {
        let beside = dir.join(name);
        if beside.is_file() {
            return Some(beside);
        }
    }
    let path = std::env::var_os("PATH")?;
    std::env::split_paths(&path)
        .map(|d| d.join(name))
        .find(|c| c.is_file())
}

/// Pull the address out of the line `serve` prints when it has bound.
///
/// Parsing another program's stdout is a contract, so it is a narrow one: the first
/// `http://` on the line, up to whitespace. The alternative — picking a free port here and
/// passing it in — has a race between the probe and the bind, and it loses to this.
pub fn parse_serve_url(line: &str) -> Option<String> {
    let start = line.find("http://")?;
    let rest = &line[start..];
    let end = rest.find(|c: char| c.is_whitespace()).unwrap_or(rest.len());
    let url = rest[..end].trim_end_matches(',').to_string();
    // A bare scheme is not an address, and neither is one without a port.
    if url.len() > "http://".len() && url.contains(':') {
        Some(url)
    } else {
        None
    }
}

/// The project directory for a folder the user picked.
///
/// People pick the store itself about as often as the project that holds it, and both are
/// reasonable readings of "where is your memory". Accept either.
pub fn normalise_project_dir(chosen: &Path) -> PathBuf {
    if chosen.file_name().and_then(|n| n.to_str()) == Some(STORE_DIR) {
        chosen.parent().unwrap_or(chosen).to_path_buf()
    } else {
        chosen.to_path_buf()
    }
}

/// Whether this directory, or one above it, holds a store — the same walk the CLI does, so
/// that a subdirectory of a project is as good an answer as its root.
pub fn has_store(dir: &Path) -> bool {
    dir.ancestors()
        .any(|d| d.join(STORE_DIR).join("notes").is_dir())
}

/// Run `cyberbrain init` in a directory that has no store yet.
pub fn init_store(server: &Path, project_dir: &Path) -> Result<(), String> {
    let out = command(server, project_dir)
        .arg("init")
        .output()
        .map_err(|e| format!("cannot run {}: {e}", server.display()))?;
    if out.status.success() {
        Ok(())
    } else {
        Err(String::from_utf8_lossy(&out.stderr).trim().to_string())
    }
}

/// Start `cyberbrain serve` in `project_dir` and wait until it says where it is.
///
/// `--port 0` lets the operating system pick, which is what makes several projects open at
/// once possible and removes the "port 7777 is taken" class of complaint entirely.
pub fn start(server: &Path, project_dir: &Path) -> Result<Server, StartError> {
    if !has_store(project_dir) {
        return Err(StartError::NoStore {
            dir: project_dir.to_path_buf(),
        });
    }

    let mut child = command(server, project_dir)
        .args(["serve", "--port", "0", "--no-open"])
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .map_err(|e| StartError::Failed {
            message: format!("cannot run {}: {e}", server.display()),
            output: String::new(),
        })?;

    // Both pipes get a reader for the life of the process. Not for the diagnostics alone:
    // a pipe nobody drains fills up, and the next line `serve` prints would block it
    // forever. The URL arrives on a channel; the readers keep going after that.
    let diag = Arc::new(Mutex::new(String::new()));
    let (tx, rx) = mpsc::channel();

    let stdout = child.stdout.take().expect("stdout was piped");
    let d = Arc::clone(&diag);
    std::thread::spawn(move || {
        let mut said = false;
        for line in BufReader::new(stdout).lines().map_while(Result::ok) {
            if !said {
                if line.trim() == NO_PAGE_MARKER {
                    said = true;
                    let _ = tx.send(Signal::NoPage);
                } else if let Some(url) = parse_serve_url(&line) {
                    said = true;
                    let _ = tx.send(Signal::Url(url));
                }
            }
            append(&d, &line);
        }
    });

    let stderr = child.stderr.take().expect("stderr was piped");
    let d = Arc::clone(&diag);
    std::thread::spawn(move || {
        for line in BufReader::new(stderr).lines().map_while(Result::ok) {
            append(&d, &line);
        }
    });

    match rx.recv_timeout(START_TIMEOUT) {
        Ok(Signal::Url(url)) => Ok(Server {
            child,
            url,
            project_dir: project_dir.to_path_buf(),
        }),
        // The marker comes before the address, so this is decided before a browser is
        // opened rather than after somebody has read a JSON error.
        Ok(Signal::NoPage) => {
            let _ = child.kill();
            let _ = child.wait();
            Err(StartError::NoPage)
        }
        Err(_) => {
            let _ = child.kill();
            let _ = child.wait();
            let output = diag.lock().map(|d| d.clone()).unwrap_or_default();
            // A store that disappeared between the check above and now, or one the CLI
            // rejects for a reason of its own: report it as the recoverable case, because
            // the message is the CLI's own and the fix is the same.
            if output.contains("store found")
                || (output.contains(STORE_DIR) && output.contains("init"))
            {
                return Err(StartError::NoStore {
                    dir: project_dir.to_path_buf(),
                });
            }
            Err(StartError::Failed {
                message: "cyberbrain serve did not report an address".to_string(),
                output,
            })
        }
    }
}

/// What the reader thread found on `serve`'s stdout, whichever came first.
enum Signal {
    Url(String),
    NoPage,
}

/// The line `cyberbrain serve` prints when its build has no page.
///
/// Duplicated from `cyberbrain::serve::NO_PAGE_MARKER` because these are two programs, not
/// two modules: the launcher runs whichever `cyberbrain.exe` is beside it, which may have
/// been built from a different tree. A shared constant would be a compile-time promise about
/// a runtime relationship. `the_marker_matches_what_serve_prints` checks the real binary.
pub const NO_PAGE_MARKER: &str = "cyberbrain serve: no web page in this build";

/// An address left behind by a running launcher, accepted only if it is loopback.
///
/// The file this comes from sits in the user's own profile, so anything able to write there
/// could point it somewhere else — and the caller hands the result to the browser. Whatever
/// wrote it, what comes back from here is `http://127.0.0.1:<port>/` or nothing.
///
/// Liveness deliberately is not checked here. The mutex already answers that question: a
/// launcher that crashed does not hold it, so a file it left behind is never read in the
/// first place.
pub fn loopback_url(raw: &str) -> Option<String> {
    let rest = raw.trim().strip_prefix("http://")?;
    let hostport = rest.strip_suffix('/').unwrap_or(rest);
    let (host, port) = hostport.rsplit_once(':')?;
    if host != "127.0.0.1" {
        return None;
    }
    // A port is a number, and 0 is not one you can connect to.
    let port: u16 = port.parse().ok()?;
    if port == 0 {
        return None;
    }
    Some(format!("http://{host}:{port}/"))
}

/// What came of one delivery attempt.
#[derive(Debug, Clone, PartialEq)]
pub enum Pushed {
    /// Rows went up, or there were none to send. Either way the hub heard from us.
    Delivered,
    /// This store belongs to nobody's hub. Nothing to do, now or later.
    NotEnrolled,
    /// A hub that is enrolled and did not answer. Normal on a train; never a dialog.
    Failed,
}

/// Deliver this store's audit rows to the hub it was enrolled with.
///
/// The CLI is meant for a timer, and on a workstation the launcher *is* the timer: it is
/// running exactly when the person is working, which is when there is anything to deliver.
/// Without this an enrolled machine would collect nothing centrally until somebody set up a
/// scheduled task — which is the command prompt coming back in through the window.
///
/// Nothing is reported to the user. A hub that stops hearing from a machine sees that in its
/// fleet view, which is the whole design; a dialog on every tunnel and hotel wifi would
/// teach people to dismiss dialogs.
pub fn push(server: &Path, project_dir: &Path) -> Pushed {
    let Ok(out) = command(server, project_dir).arg("hub").arg("push").output() else {
        return Pushed::Failed;
    };
    if out.status.success() {
        return Pushed::Delivered;
    }
    if String::from_utf8_lossy(&out.stderr).contains("not enrolled") {
        Pushed::NotEnrolled
    } else {
        Pushed::Failed
    }
}

/// Enrol this project's store with a company hub, from an invitation file.
///
/// The CLI already does this; what the launcher adds is that nobody has to find a prompt to
/// run it. On the machines this product is for, the invitation arrives as an email
/// attachment and the person who has to act on it does not know what a working directory is.
///
/// `Ok` carries what the CLI said, which names the hub and where the token was put.
pub fn enrol(
    server: &Path,
    project_dir: &Path,
    invitation: &Path,
) -> std::result::Result<String, String> {
    let out = command(server, project_dir)
        .arg("hub")
        .arg("enrol")
        .arg(invitation)
        .output()
        .map_err(|e| format!("cyberbrain could not be started: {e}"))?;
    let text = |b: &[u8]| String::from_utf8_lossy(b).trim().to_string();
    if out.status.success() {
        Ok(text(&out.stdout))
    } else {
        // Its own words, not ours: the CLI knows why an invitation was refused, and a
        // paraphrase here would be one more thing to keep in step.
        let msg = text(&out.stderr);
        Err(if msg.is_empty() {
            format!("enrolment failed ({})", out.status)
        } else {
            msg
        })
    }
}

fn command(server: &Path, project_dir: &Path) -> Command {
    let mut c = Command::new(server);
    // The working directory is how the store is chosen: the CLI walks up from here, so a
    // project root and any directory inside it both resolve to the same store. Passing
    // --store would have meant guessing which of the two the user picked.
    c.current_dir(project_dir);
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        // CREATE_NO_WINDOW. Without it every start flashes a console, which is the one
        // thing this whole program exists to remove.
        c.creation_flags(0x0800_0000);
    }
    c
}

fn append(buf: &Arc<Mutex<String>>, line: &str) {
    if let Ok(mut b) = buf.lock()
        && b.len() < DIAG_LIMIT
    {
        b.push_str(line);
        b.push('\n');
    }
}

/// How often an enrolled machine delivers, in ticks of the one-second message loop.
///
/// A quarter of an hour: often enough that the fleet view's two-day silence threshold means
/// something, rare enough to be invisible. The first one comes sooner, because a laptop that
/// was shut all weekend has the most to say and the least time to say it in.
const PUSH_EVERY: u32 = 15 * 60;
const FIRST_PUSH_AFTER: u32 = 30;

/// The delivery timer, and what it learned about this store.
///
/// A push can take over two minutes in the worst case the transport allows, and this loop is
/// the window message pump: doing it here would freeze the tray icon and the menu. So the
/// work goes to a thread and the answer is collected on a later tick.
#[derive(Default)]
pub struct Delivery {
    ticks: u32,
    next: Option<u32>,
    /// Set once the CLI says this store belongs to no hub. Then there is nothing to retry:
    /// most machines are not enrolled, and spawning a process every quarter hour to be told
    /// so again is work nobody asked for.
    enrolled: Option<bool>,
    /// The delivery in flight, if there is one. At most one: a slow hub must not end up
    /// with a queue of pushes started while it was not answering.
    running: Option<std::sync::mpsc::Receiver<Pushed>>,
}

impl Delivery {
    /// A timer that fires on the next tick.
    pub fn now() -> Self {
        Delivery {
            next: Some(0),
            ..Default::default()
        }
    }

    pub fn tick(&mut self, server_exe: &Path, project_dir: &Path) {
        self.tick_with(|| spawn_push(server_exe, project_dir));
    }

    /// The schedule itself, with what starts a delivery handed in so a test can watch the
    /// clock without running anything.
    fn tick_with(&mut self, start: impl FnOnce() -> std::sync::mpsc::Receiver<Pushed>) {
        self.collect();
        if self.enrolled == Some(false) || self.running.is_some() {
            return;
        }
        self.ticks = self.ticks.saturating_add(1);
        if self.ticks < self.next.unwrap_or(FIRST_PUSH_AFTER) {
            return;
        }
        self.next = Some(self.ticks.saturating_add(PUSH_EVERY));
        self.running = Some(start());
    }

    /// Take the answer if there is one. Never waits: this runs inside the message pump.
    fn collect(&mut self) {
        let Some(rx) = &self.running else { return };
        match rx.try_recv() {
            Ok(p) => {
                self.note(p);
                self.running = None;
            }
            // The thread went away without answering. Treat it as a failed attempt rather
            // than as a permanent verdict, and let the next tick start a new one.
            Err(std::sync::mpsc::TryRecvError::Disconnected) => self.running = None,
            Err(std::sync::mpsc::TryRecvError::Empty) => {}
        }
    }

    fn note(&mut self, p: Pushed) {
        match p {
            Pushed::Delivered => self.enrolled = Some(true),
            Pushed::NotEnrolled => self.enrolled = Some(false),
            // A hub that did not answer is still a hub. Try again next time, say nothing.
            Pushed::Failed => self.enrolled = Some(true),
        }
    }

    /// On the way out, and only for a store we know is enrolled — asking the question for
    /// the first time while the user waits for the program to close is the wrong moment.
    ///
    /// Bounded, because quitting must stay quick: what does not go now is still in the
    /// local audit log, and the next start sends it along with everything since.
    pub fn final_push(&mut self, server_exe: &Path, project_dir: &Path) {
        if self.enrolled != Some(true) {
            return;
        }
        let rx = spawn_push(server_exe, project_dir);
        let _ = rx.recv_timeout(std::time::Duration::from_secs(5));
    }
}

fn spawn_push(server_exe: &Path, project_dir: &Path) -> std::sync::mpsc::Receiver<Pushed> {
    let (tx, rx) = std::sync::mpsc::channel();
    let exe = server_exe.to_path_buf();
    let dir = project_dir.to_path_buf();
    std::thread::spawn(move || {
        let _ = tx.send(push(&exe, &dir));
    });
    rx
}

#[cfg(test)]
mod tests;
