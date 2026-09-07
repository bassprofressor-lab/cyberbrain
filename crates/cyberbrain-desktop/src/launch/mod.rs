//! Starting `cyberbrain serve` and finding out where it landed.
//!
//! Everything here is deliberately platform-neutral, which is not tidiness for its own
//! sake: it means the interesting half of a Windows-only program can be tested on the
//! machine that builds it, against the real binary rather than a stand-in. The Windows
//! half — the tray, the job object, the dialogs — is in `win`.

use std::io::{BufRead, BufReader};
use std::net::ToSocketAddrs;
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

/// How long the check for an already-running launcher waits. Loopback: either it answers
/// immediately or there is nothing there.
const PROBE_TIMEOUT: Duration = Duration::from_millis(500);

#[derive(Debug)]
pub enum StartError {
    /// The chosen folder has no store. Recoverable, and the caller offers to create one.
    NoStore { dir: PathBuf },
    /// It started and said nothing we could use, or it died. Carries what it printed.
    Failed { message: String, output: String },
}

impl std::fmt::Display for StartError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            StartError::NoStore { dir } => {
                write!(f, "no {STORE_DIR} store in {}", dir.display())
            }
            StartError::Failed { message, .. } => write!(f, "{message}"),
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
        let mut found = false;
        for line in BufReader::new(stdout).lines().map_while(Result::ok) {
            if !found && let Some(url) = parse_serve_url(&line) {
                found = true;
                let _ = tx.send(url);
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
        Ok(url) => Ok(Server {
            child,
            url,
            project_dir: project_dir.to_path_buf(),
        }),
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

/// Whether something is still listening at an address a previous launcher left behind.
///
/// The file outlives the process that wrote it — a crash, a reboot, a kill — so the address
/// in it is a claim to check, not a fact. A connection that opens is enough: this asks
/// whether the port is alive, not what is on the other end, and on loopback in the second
/// after a launcher started, nothing else plausibly is.
pub fn responds(url: &str) -> bool {
    let hostport = url.trim_start_matches("http://").trim_end_matches('/');
    let Ok(mut addrs) = hostport.to_socket_addrs() else {
        return false;
    };
    addrs.any(|a| std::net::TcpStream::connect_timeout(&a, PROBE_TIMEOUT).is_ok())
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

#[cfg(test)]
mod tests;
