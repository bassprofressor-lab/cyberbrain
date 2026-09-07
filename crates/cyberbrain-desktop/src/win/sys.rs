//! The four things this program needs from Win32, and nothing else.
//!
//! Every `unsafe` block in the crate is in this file. They are all the same shape: build a
//! null-terminated UTF-16 string, call one function, look at what came back.

use crate::launch::Server;
use std::ffi::OsStr;
use std::os::windows::ffi::OsStrExt;
use std::path::Path;
use windows_sys::Win32::Foundation::{CloseHandle, ERROR_ALREADY_EXISTS, GetLastError, HANDLE};
use windows_sys::Win32::System::JobObjects::{
    AssignProcessToJobObject, CreateJobObjectW, JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE,
    JOBOBJECT_EXTENDED_LIMIT_INFORMATION, JobObjectExtendedLimitInformation,
    SetInformationJobObject,
};
use windows_sys::Win32::System::Threading::{CreateMutexW, ReleaseMutex};
use windows_sys::Win32::UI::Shell::ShellExecuteW;
use windows_sys::Win32::UI::WindowsAndMessaging::{
    DispatchMessageW, GetMessageW, IDYES, KillTimer, MB_ICONERROR, MB_ICONINFORMATION,
    MB_ICONQUESTION, MB_SETFOREGROUND, MB_TOPMOST, MB_YESNO, MSG, MessageBoxW, PostQuitMessage,
    SW_SHOWNORMAL, SetTimer, TranslateMessage,
};

/// What the caller wants after handling whatever just arrived.
pub enum Pump {
    Continue,
    Stop,
}

/// How often the loop wakes up on its own, to notice a server that died quietly.
const TICK_MS: u32 = 1_000;
const TICK_ID: usize = 1;

fn wide(s: &str) -> Vec<u16> {
    OsStr::new(s).encode_wide().chain(Some(0)).collect()
}

/// A modal error. On top, because the window that would otherwise own it does not exist:
/// this program has a tray icon and no main window, and a dialog behind everything else is
/// a program that has hung as far as anyone can tell.
pub fn error_box(title: &str, text: &str) {
    let (t, c) = (wide(text), wide(title));
    unsafe {
        MessageBoxW(
            std::ptr::null_mut(),
            t.as_ptr(),
            c.as_ptr(),
            MB_ICONERROR | MB_SETFOREGROUND | MB_TOPMOST,
        );
    }
}

/// Something worked, and the person who clicked deserves to be told so in the same place
/// they would have been told it had not.
pub fn info_box(title: &str, text: &str) {
    let (t, c) = (wide(text), wide(title));
    unsafe {
        MessageBoxW(
            std::ptr::null_mut(),
            t.as_ptr(),
            c.as_ptr(),
            MB_ICONINFORMATION | MB_SETFOREGROUND | MB_TOPMOST,
        );
    }
}

pub fn ask_yes_no(title: &str, text: &str) -> bool {
    let (t, c) = (wide(text), wide(title));
    let answer = unsafe {
        MessageBoxW(
            std::ptr::null_mut(),
            t.as_ptr(),
            c.as_ptr(),
            MB_YESNO | MB_ICONQUESTION | MB_SETFOREGROUND | MB_TOPMOST,
        )
    };
    answer == IDYES
}

/// Hand the address to whatever the user has set as their browser. Deliberately not a
/// browser we choose: the page is theirs to open where they read things.
pub fn open_in_browser(url: &str) {
    shell_open(url);
}

pub fn open_in_explorer(dir: &Path) {
    shell_open(&dir.display().to_string());
}

fn shell_open(target: &str) {
    let (verb, file) = (wide("open"), wide(target));
    unsafe {
        ShellExecuteW(
            std::ptr::null_mut(),
            verb.as_ptr(),
            file.as_ptr(),
            std::ptr::null(),
            std::ptr::null(),
            SW_SHOWNORMAL,
        );
    }
}

/// Run the Win32 message loop, calling `on_tick` after every message and once a second.
///
/// The tray icon needs the loop to exist at all; the timer is what turns the loop into
/// something that also notices a server that stopped without being asked.
pub fn pump_messages(mut on_tick: impl FnMut() -> Pump) {
    unsafe {
        SetTimer(std::ptr::null_mut(), TICK_ID, TICK_MS, None);
    }
    let mut msg: MSG = unsafe { std::mem::zeroed() };
    loop {
        let got = unsafe { GetMessageW(&mut msg, std::ptr::null_mut(), 0, 0) };
        if got <= 0 {
            break; // 0 is WM_QUIT, -1 is an error we cannot do anything useful about.
        }
        unsafe {
            TranslateMessage(&msg);
            DispatchMessageW(&msg);
        }
        if let Pump::Stop = on_tick() {
            unsafe { PostQuitMessage(0) };
        }
    }
    unsafe {
        KillTimer(std::ptr::null_mut(), TICK_ID);
    }
}

/// A job object that holds every server this launcher starts.
///
/// The point is the limit flag: when the last handle to the job closes — including the one
/// that closes because this process was killed rather than asked to quit — Windows ends
/// the members. Without it, quitting through the task manager would leave a `cyberbrain
/// serve` holding a port with no way left to reach it.
pub struct JobObject {
    handle: HANDLE,
}

impl JobObject {
    pub fn new() -> Self {
        let handle = unsafe { CreateJobObjectW(std::ptr::null(), std::ptr::null()) };
        if !handle.is_null() {
            let mut info: JOBOBJECT_EXTENDED_LIMIT_INFORMATION = unsafe { std::mem::zeroed() };
            info.BasicLimitInformation.LimitFlags = JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE;
            unsafe {
                SetInformationJobObject(
                    handle,
                    JobObjectExtendedLimitInformation,
                    (&raw const info).cast(),
                    size_of::<JOBOBJECT_EXTENDED_LIMIT_INFORMATION>() as u32,
                );
            }
        }
        Self { handle }
    }

    /// Put a started server in the job. A failure here is not worth a dialog: the server
    /// runs, and `Server::stop` still ends it on a normal quit. What is lost is only the
    /// guarantee for the abnormal one.
    pub fn adopt(&self, server: &Server) {
        if self.handle.is_null() {
            return;
        }
        unsafe {
            AssignProcessToJobObject(self.handle, server.raw_handle());
        }
    }
}

impl Drop for JobObject {
    fn drop(&mut self) {
        if !self.handle.is_null() {
            unsafe { CloseHandle(self.handle) };
        }
    }
}

/// A named mutex that only one launcher can hold.
///
/// `Local\\` rather than `Global\\` on purpose: the scope is the logon session. Two people
/// signed in to the same machine each get their own launcher, which is right — they have
/// their own projects, their own settings file and their own idea of what "already running"
/// means.
pub struct SingleInstance {
    handle: HANDLE,
}

impl SingleInstance {
    /// `None` when another launcher already holds it.
    ///
    /// A mutex that cannot be created at all counts as free. Refusing to start because the
    /// check itself failed would trade a duplicate tray icon for a program that does not
    /// run, which is the worse of the two.
    pub fn acquire() -> Option<Self> {
        let name = wide("Local\\cyberbrain-desktop-single-instance");
        let handle = unsafe { CreateMutexW(std::ptr::null(), 1, name.as_ptr()) };
        if handle.is_null() {
            return Some(Self {
                handle: std::ptr::null_mut(),
            });
        }
        // GetLastError right after the call, before anything else can overwrite it: the
        // handle comes back valid either way, and this is the only thing that says whether
        // we made the mutex or merely opened someone else's.
        if unsafe { GetLastError() } == ERROR_ALREADY_EXISTS {
            unsafe { CloseHandle(handle) };
            return None;
        }
        Some(Self { handle })
    }
}

impl Drop for SingleInstance {
    fn drop(&mut self) {
        if !self.handle.is_null() {
            unsafe {
                ReleaseMutex(self.handle);
                CloseHandle(self.handle);
            }
        }
    }
}
