//! The hub as a Windows service.
//!
//! # Why the same binary does both
//!
//! A service is started by the service control manager, which expects the process to call
//! back into it within seconds and then answer stop requests. A console program does not do
//! that, and a program that only does that cannot be run by hand to see what it says.
//!
//! So `cyberbrain hub serve` tries the service handshake first. Started by the SCM it
//! succeeds and the process behaves as a service; started from a prompt it fails with one
//! specific error, and the program carries on as an ordinary console server. No flag to
//! remember, no second executable that drifts from the first.
//!
//! # Why installation is not a command somebody types
//!
//! It is, for administrators who prefer one — but the installer registers the service on its
//! own if that box is ticked. A small company should not have to learn `sc.exe` to collect
//! its own audit trail, and the moment a setup requires a prompt, the person who needed the
//! product most is the one who stops.

use cyberbrain_core::{Error, Result};
use std::path::{Path, PathBuf};

/// Name the service is registered under, and how it appears in services.msc.
pub const SERVICE_NAME: &str = "CyberbrainHub";
pub const DISPLAY_NAME: &str = "Cyberbrain Hub";
#[cfg_attr(not(windows), allow(dead_code))]
pub const DESCRIPTION: &str = concat!(
    "Collects the audit rows of Cyberbrain clients on this network. ",
    "Holds no notes: only records of what happened, in a chain that cannot be edited."
);

/// Where a service keeps its record when nobody said otherwise.
///
/// `%PROGRAMDATA%`, not the install directory: the record outlives the program, survives an
/// uninstall, and is the thing a backup has to include. A database under Program Files is
/// one Windows update away from being a surprise.
pub fn default_data_dir() -> PathBuf {
    std::env::var_os("PROGRAMDATA")
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from("C:\\ProgramData"))
        .join("Cyberbrain")
}

/// A licence dropped next to the record is picked up on start.
///
/// The alternative is telling somebody to run an install command with a path in it, which is
/// exactly the sort of step that turns into a support call. Copy the file in, restart the
/// service, done — and the service says in its log which licence it found.
pub fn licence_drop_path(data_dir: &Path) -> PathBuf {
    data_dir.join(LICENCE_NAMES[0])
}

/// The name to use, and the ones people actually end up with.
///
/// `licence.txt.txt` because Explorer hides known extensions by default, so saving an
/// attachment as "licence.txt" produces that and shows it as "licence.txt". `license` because
/// half the world spells it that way and our own documentation is the odd one out. Accepting
/// them costs nothing; refusing them costs a support call over a file the person cannot even
/// see the real name of. The log always says which one it took.
pub const LICENCE_NAMES: [&str; 4] = [
    "licence.txt",
    "licence.txt.txt",
    "license.txt",
    "license.txt.txt",
];

/// The licence file lying in the data directory, whichever spelling it arrived under.
pub fn find_licence_file(data_dir: &Path) -> Option<PathBuf> {
    LICENCE_NAMES
        .iter()
        .map(|n| data_dir.join(n))
        .find(|p| p.is_file())
}

/// What to write in the log when the hub has no licence and no file was found.
///
/// The first version of this said nothing at all in that case, on the grounds that a hub
/// licensed months ago has no file lying about. True, and useless the one time it matters:
/// the log said "no licence installed" and left the reader with no way to tell whether the
/// file had been looked for, looked for somewhere else, or found and rejected.
pub fn where_it_looked(data_dir: &Path) -> String {
    format!(
        "no licence file in {}. Put the one you were sent there as {} and restart this \
         service.",
        data_dir.display(),
        LICENCE_NAMES[0]
    )
}

/// Take a licence file sitting next to the record, if there is one and it is usable.
///
/// Returns what happened, for the log. Deliberately quiet about a missing file: not having
/// dropped one in is the normal state of a hub that was licensed months ago.
pub fn adopt_dropped_licence(hub: &super::HubStore, data_dir: &Path) -> Option<String> {
    let path = find_licence_file(data_dir)?;
    let text = std::fs::read_to_string(&path).ok()?;
    // Compared before it is parsed, not after. The file is meant to be left where it was
    // copied, so the ordinary case is one that is already installed: that should cost
    // nothing and say nothing, on every restart, for years.
    if hub.licence_text().ok().flatten().as_deref() == Some(text.as_str()) {
        return None;
    }
    match super::licence::parse(&text) {
        Ok(signed) => match hub.set_licence(&text) {
            Ok(()) => Some(format!(
                "licence picked up from {}: {}, {} seat(s), until {}",
                path.display(),
                signed.licence().customer,
                signed.licence().seats,
                signed.licence().valid_until
            )),
            Err(e) => Some(format!(
                "could not store the licence from {}: {e}",
                path.display()
            )),
        },
        // A bad file is worth saying out loud: somebody put it there on purpose.
        Err(e) => Some(format!(
            "the licence at {} is not usable: {e}",
            path.display()
        )),
    }
}

/// What `hub serve` does, given something that says when to stop.
///
/// A boxed closure rather than a plain function because the address and the record's path
/// are already parsed by the time we know whether we are a service, and re-parsing them in
/// a second place is how the two routes drift apart.
pub type Serve = Box<dyn Fn(std::sync::mpsc::Receiver<()>) -> Result<()> + Send + Sync>;

static LOG_PATH: std::sync::OnceLock<PathBuf> = std::sync::OnceLock::new();
static SERVE: std::sync::OnceLock<Serve> = std::sync::OnceLock::new();

/// Remember what serving means, before asking whether we are a service.
///
/// It has to be waiting rather than passed in: the service control manager calls a function
/// with a fixed signature, on a thread of its own.
pub fn set_serve(f: Serve) {
    let _ = SERVE.set(f);
}

/// Run it, whoever asked. Both routes end here, so there is one server, not two that drift.
pub fn run_serve(stop: std::sync::mpsc::Receiver<()>) -> Result<()> {
    match SERVE.get() {
        Some(f) => f(stop),
        None => Err(Error::Config("nothing to serve was set up".into())),
    }
}

/// Where the service writes what it would otherwise have printed.
///
/// A service has no console, so a message printed to stdout is a message nobody will ever
/// read — including the one explaining why the thing will not start. The file sits next to
/// the record, which is the directory an administrator already has to know about.
pub fn set_log_path(data_db: &Path) {
    let dir = data_db.parent().unwrap_or(Path::new("."));
    let _ = LOG_PATH.set(dir.join("hub-service.log"));
}

/// Say something, to the log file when there is one and to stderr otherwise.
pub fn log(msg: &str) {
    let line = format!("{} {msg}\n", jiff::Timestamp::now());
    match LOG_PATH.get() {
        Some(p) => {
            use std::io::Write;
            if let Ok(mut f) = std::fs::OpenOptions::new()
                .create(true)
                .append(true)
                .open(p)
            {
                let _ = f.write_all(line.as_bytes());
            }
        }
        None => eprint!("{line}"),
    }
}

/// An operating system error with its number, because the number is what a person can look
/// up and quote. Without it "Access is denied" and "The specified service already exists"
/// are two sentences with no thread back to anything.
/// Called from the Windows module and from the tests; on any other platform the binary
/// itself has no caller for it.
#[cfg_attr(not(windows), allow(dead_code))]
pub fn describe_os_error(io: &std::io::Error) -> String {
    match io.raw_os_error() {
        Some(code) => format!("{io} (Windows error {code})"),
        None => io.to_string(),
    }
}

#[cfg(not(windows))]
mod platform {
    use super::*;

    pub fn install(_exe: &Path, _data: &Path, _addr: &str) -> Result<()> {
        Err(unsupported())
    }
    pub fn uninstall() -> Result<()> {
        Err(unsupported())
    }
    pub fn set_state(_start: bool) -> Result<()> {
        Err(unsupported())
    }
    pub fn status() -> Result<String> {
        Err(unsupported())
    }

    /// Nothing here launches services, so `hub serve` is always the console server.
    pub fn try_dispatch() -> Result<bool> {
        Ok(false)
    }

    fn unsupported() -> Error {
        Error::Config(
            "Windows services exist only on Windows. On Linux use a systemd unit; \
             docs/HUB.md has one."
                .into(),
        )
    }
}

#[cfg(windows)]
mod platform {
    use super::*;
    use std::ffi::OsString;
    use windows_service::service::{
        ServiceAccess, ServiceErrorControl, ServiceInfo, ServiceStartType, ServiceState,
        ServiceType,
    };
    use windows_service::service_manager::{ServiceManager, ServiceManagerAccess};

    /// What actually went wrong, rather than the wrapper's opinion of it.
    ///
    /// `windows_service::Error::Winapi` displays as "IO error in winapi call" and keeps the
    /// operating system's message and code in `source()`. Printing only the outer layer is
    /// how a failed installation ends up telling somebody nothing at all. It did.
    fn why(e: windows_service::Error) -> String {
        match &e {
            windows_service::Error::Winapi(io) => describe_os_error(io),
            other => other.to_string(),
        }
    }

    /// Windows codes worth naming, because each has a different fix.
    const ERROR_ACCESS_DENIED: i32 = 5;
    const ERROR_SERVICE_MARKED_FOR_DELETE: i32 = 1072;

    fn os_code(e: &windows_service::Error) -> Option<i32> {
        match e {
            windows_service::Error::Winapi(io) => io.raw_os_error(),
            _ => None,
        }
    }

    fn manager(access: ServiceManagerAccess) -> Result<ServiceManager> {
        ServiceManager::local_computer(None::<&str>, access).map_err(|e| {
            let hint = if os_code(&e) == Some(ERROR_ACCESS_DENIED) {
                " This needs administrator rights: right-click, Run as administrator."
            } else {
                ""
            };
            Error::Config(format!(
                "cannot reach the service control manager: {}.{hint}",
                why(e)
            ))
        })
    }

    pub fn install(exe: &Path, data: &Path, addr: &str) -> Result<()> {
        let m = manager(ServiceManagerAccess::CREATE_SERVICE | ServiceManagerAccess::CONNECT)?;
        let info = ServiceInfo {
            name: OsString::from(SERVICE_NAME),
            display_name: OsString::from(DISPLAY_NAME),
            service_type: ServiceType::OWN_PROCESS,
            // Automatic: a hub that only runs when somebody remembers to start it makes
            // silence useless as a signal, which is the whole point of the fleet view.
            start_type: ServiceStartType::AutoStart,
            error_control: ServiceErrorControl::Normal,
            executable_path: exe.to_path_buf(),
            launch_arguments: vec![
                OsString::from("hub"),
                OsString::from("serve"),
                OsString::from("--data"),
                OsString::from(data),
                OsString::from("--addr"),
                OsString::from(addr),
            ],
            dependencies: vec![],
            account_name: None, // LocalSystem
            account_password: None,
        };
        // A service that is already there is the ordinary case, not a failure: reinstalling
        // over the top, repairing, upgrading. Failing here left the first installer saying
        // "could not be registered" about a machine that was already set up correctly.
        let existing = m.open_service(
            SERVICE_NAME,
            ServiceAccess::CHANGE_CONFIG | ServiceAccess::START | ServiceAccess::QUERY_STATUS,
        );
        let service = match existing {
            Ok(service) => {
                // Take the new paths and the new address: an upgrade that kept the old
                // command line would run yesterday's executable from a directory that may
                // no longer exist.
                service
                    .change_config(&info)
                    .map_err(|e| Error::Config(format!("cannot update the service: {}", why(e))))?;
                service
            }
            Err(e) if os_code(&e) == Some(ERROR_SERVICE_MARKED_FOR_DELETE) => {
                return Err(Error::Config(
                    concat!(
                        "the service is being removed and Windows will not finish until ",
                        "everything holding it lets go. Close services.msc and the Services ",
                        "tab in Task Manager, then try again; a restart always clears it."
                    )
                    .into(),
                ));
            }
            Err(_) => m
                .create_service(&info, ServiceAccess::CHANGE_CONFIG | ServiceAccess::START)
                .map_err(|e| {
                    let hint = if os_code(&e) == Some(ERROR_ACCESS_DENIED) {
                        " This needs administrator rights."
                    } else {
                        ""
                    };
                    Error::Config(format!("cannot register the service: {}.{hint}", why(e)))
                })?,
        };
        let _ = service.set_description(DESCRIPTION);
        // Already running after an update is success, not an error to report.
        match service.start::<OsString>(&[]) {
            Ok(()) => Ok(()),
            Err(_) if service_is_running(&service) => Ok(()),
            Err(e) => Err(Error::Config(format!(
                "it is registered, but did not start: {}. \
                 The log beside the record says why.",
                why(e)
            ))),
        }
    }

    fn service_is_running(service: &windows_service::service::Service) -> bool {
        service
            .query_status()
            .is_ok_and(|s| s.current_state == ServiceState::Running)
    }

    pub fn uninstall() -> Result<()> {
        let m = manager(ServiceManagerAccess::CONNECT)?;
        let service = m
            .open_service(SERVICE_NAME, ServiceAccess::STOP | ServiceAccess::DELETE)
            .map_err(|e| Error::Config(format!("cannot open the service: {}", why(e))))?;
        // Stop first, ignoring "already stopped": deleting a running service leaves it
        // marked for deletion until reboot, which looks like the command did nothing.
        let _ = service.stop();
        service
            .delete()
            .map_err(|e| Error::Config(format!("cannot remove the service: {}", why(e))))?;
        Ok(())
    }

    pub fn set_state(start: bool) -> Result<()> {
        let m = manager(ServiceManagerAccess::CONNECT)?;
        let access = if start {
            ServiceAccess::START
        } else {
            ServiceAccess::STOP
        };
        let service = m
            .open_service(SERVICE_NAME, access)
            .map_err(|e| Error::Config(format!("cannot open the service: {}", why(e))))?;
        if start {
            service
                .start::<OsString>(&[])
                .map_err(|e| Error::Config(format!("cannot start it: {}", why(e))))?;
        } else {
            service
                .stop()
                .map_err(|e| Error::Config(format!("cannot stop it: {}", why(e))))?;
        }
        Ok(())
    }

    pub fn status() -> Result<String> {
        let m = manager(ServiceManagerAccess::CONNECT)?;
        let service = m
            .open_service(SERVICE_NAME, ServiceAccess::QUERY_STATUS)
            .map_err(|e| {
                Error::Config(format!(
                    "the service is not registered ({}). Install it with \
                     `cyberbrain hub service install`, or tick the box in the installer.",
                    why(e)
                ))
            })?;
        let s = service
            .query_status()
            .map_err(|e| Error::Config(format!("cannot read the status: {}", why(e))))?;
        Ok(match s.current_state {
            ServiceState::Running => "running".into(),
            ServiceState::Stopped => "stopped".into(),
            ServiceState::StartPending => "starting".into(),
            ServiceState::StopPending => "stopping".into(),
            other => format!("{other:?}"),
        })
    }

    // -----------------------------------------------------------------------------------
    // Being a service.

    use windows_service::service::{
        ServiceControl, ServiceControlAccept, ServiceExitCode, ServiceStatus,
    };
    use windows_service::service_control_handler::{self, ServiceControlHandlerResult};
    use windows_service::service_dispatcher;

    /// Windows reports this when a process calls the dispatcher without having been started
    /// as a service. It is not a fault: it is how the program learns it was run by a person.
    const NOT_A_SERVICE: i32 = 1063; // ERROR_FAILED_SERVICE_CONTROLLER_CONNECT

    windows_service::define_windows_service!(ffi_service_main, service_main);

    fn service_main(_args: Vec<OsString>) {
        // Nothing above this catches an error, so anything that goes wrong has to be
        // written down here or it is lost with the process.
        if let Err(e) = run_service() {
            log(&format!("the service stopped: {e}"));
        }
    }

    fn run_service() -> Result<()> {
        let (tx, rx) = std::sync::mpsc::channel::<()>();
        let handle =
            service_control_handler::register(SERVICE_NAME, move |control| match control {
                // Shutdown as well as Stop: a machine being turned off should close the
                // listener the same way, not have the process killed mid-write.
                ServiceControl::Stop | ServiceControl::Shutdown => {
                    let _ = tx.send(());
                    ServiceControlHandlerResult::NoError
                }
                ServiceControl::Interrogate => ServiceControlHandlerResult::NoError,
                _ => ServiceControlHandlerResult::NotImplemented,
            })
            .map_err(|e| Error::Config(format!("cannot register the control handler: {e}")))?;

        let status = |state, exit, controls| ServiceStatus {
            service_type: ServiceType::OWN_PROCESS,
            current_state: state,
            controls_accepted: controls,
            exit_code: exit,
            checkpoint: 0,
            wait_hint: std::time::Duration::from_secs(10),
            process_id: None,
        };

        handle
            .set_service_status(status(
                ServiceState::Running,
                ServiceExitCode::Win32(0),
                ServiceControlAccept::STOP | ServiceControlAccept::SHUTDOWN,
            ))
            .map_err(|e| Error::Config(format!("cannot report Running: {e}")))?;

        let outcome = super::run_serve(rx);

        // Reported even when the server failed. A service that dies without saying Stopped
        // leaves the SCM waiting, and services.msc shows "stopping" until a reboot.
        let exit = match &outcome {
            Ok(()) => ServiceExitCode::Win32(0),
            Err(_) => ServiceExitCode::ServiceSpecific(1),
        };
        let _ = handle.set_service_status(status(
            ServiceState::Stopped,
            exit,
            ServiceControlAccept::empty(),
        ));
        outcome
    }

    /// Try the service handshake. `Ok(false)` means this is an ordinary run from a prompt.
    pub fn try_dispatch() -> Result<bool> {
        match service_dispatcher::start(SERVICE_NAME, ffi_service_main) {
            Ok(()) => Ok(true),
            Err(windows_service::Error::Winapi(e)) if e.raw_os_error() == Some(NOT_A_SERVICE) => {
                Ok(false)
            }
            Err(e) => Err(Error::Config(format!(
                "the service control manager refused the connection: {e}"
            ))),
        }
    }
}

pub use platform::{install, set_state, status, try_dispatch, uninstall};
