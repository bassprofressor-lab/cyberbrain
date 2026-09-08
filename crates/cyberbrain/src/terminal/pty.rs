//! A pseudo-terminal, on Windows and on Unix.
//!
//! Two implementations of four operations: start a program attached to a terminal, read what
//! it prints, write what the person types, and tell it the window changed size. Everything
//! above this module is platform-neutral.
//!
//! Both platforms are here on purpose rather than Windows alone. The desktop launcher is a
//! Windows program, but the machine this is written and tested on is not one, and a terminal
//! whose only implementation runs where nobody can try it is a terminal nobody has tried.
//! The Unix half is also useful in its own right: `cyberbrain serve` runs on a server.

use std::path::Path;

/// What a terminal session is asked to run.
pub struct Spawn<'a> {
    /// argv. Empty means the platform's default shell.
    pub command: &'a [String],
    pub cwd: &'a Path,
    pub cols: u16,
    pub rows: u16,
}

#[cfg(unix)]
pub use unix::Pty;
#[cfg(windows)]
pub use windows::Pty;

// ---------------------------------------------------------------------------------------

#[cfg(unix)]
mod unix {
    use super::{Spawn, default_shell};
    use std::io::{self, Read, Write};
    use std::os::fd::{AsRawFd, FromRawFd, OwnedFd, RawFd};
    use std::os::unix::process::CommandExt;
    use std::process::{Child, Command};

    pub struct Pty {
        master: OwnedFd,
        child: Child,
    }

    impl Pty {
        pub fn spawn(req: Spawn<'_>) -> io::Result<Pty> {
            // Safety: openpty fills two descriptors or returns non-zero; nothing else here
            // touches the values until it has said it succeeded.
            let (master, slave) = unsafe {
                let mut m: RawFd = -1;
                let mut s: RawFd = -1;
                let size = libc::winsize {
                    ws_row: req.rows,
                    ws_col: req.cols,
                    ws_xpixel: 0,
                    ws_ypixel: 0,
                };
                if libc::openpty(
                    &mut m,
                    &mut s,
                    std::ptr::null_mut(),
                    std::ptr::null(),
                    &size,
                ) != 0
                {
                    return Err(io::Error::last_os_error());
                }
                (OwnedFd::from_raw_fd(m), OwnedFd::from_raw_fd(s))
            };

            let argv = default_shell(req.command);
            let mut cmd = Command::new(&argv[0]);
            cmd.args(&argv[1..])
                .current_dir(req.cwd)
                // What a program looks at to decide whether it may use colour and cursor
                // movement. Without it many tools fall back to their dumbest output.
                .env("TERM", "xterm-256color");

            let slave_fd = slave.as_raw_fd();
            unsafe {
                cmd.pre_exec(move || {
                    // A session of its own, with the pty as its controlling terminal, or
                    // Ctrl-C reaches this process instead of the child's job.
                    if libc::setsid() < 0 {
                        return Err(io::Error::last_os_error());
                    }
                    if libc::ioctl(slave_fd, libc::TIOCSCTTY as _, 0) < 0 {
                        return Err(io::Error::last_os_error());
                    }
                    for target in [libc::STDIN_FILENO, libc::STDOUT_FILENO, libc::STDERR_FILENO] {
                        if libc::dup2(slave_fd, target) < 0 {
                            return Err(io::Error::last_os_error());
                        }
                    }
                    if slave_fd > libc::STDERR_FILENO {
                        libc::close(slave_fd);
                    }
                    Ok(())
                });
            }
            let child = cmd.spawn()?;
            // The parent has no use for the slave end, and holding it open would mean the
            // read below never sees end-of-file when the child exits.
            drop(slave);
            Ok(Pty { master, child })
        }

        pub fn reader(&self) -> io::Result<Box<dyn Read + Send>> {
            Ok(Box::new(std::fs::File::from(self.master.try_clone()?)))
        }

        pub fn writer(&self) -> io::Result<Box<dyn Write + Send>> {
            Ok(Box::new(std::fs::File::from(self.master.try_clone()?)))
        }

        pub fn resize(&self, cols: u16, rows: u16) -> io::Result<()> {
            let size = libc::winsize {
                ws_row: rows,
                ws_col: cols,
                ws_xpixel: 0,
                ws_ypixel: 0,
            };
            // Safety: a valid descriptor and a filled struct; the kernel copies it out.
            if unsafe { libc::ioctl(self.master.as_raw_fd(), libc::TIOCSWINSZ, &size) } < 0 {
                return Err(io::Error::last_os_error());
            }
            Ok(())
        }

        pub fn kill(&mut self) {
            let _ = self.child.kill();
            let _ = self.child.wait();
        }

        pub fn exited(&mut self) -> Option<i32> {
            self.child
                .try_wait()
                .ok()
                .flatten()
                .map(|s| s.code().unwrap_or(-1))
        }
    }
}

// ---------------------------------------------------------------------------------------

#[cfg(windows)]
mod windows {
    use super::{Spawn, default_shell};
    use std::io::{self, Read, Write};
    use std::os::windows::io::{FromRawHandle, OwnedHandle};
    use std::ptr;
    use windows_sys::Win32::Foundation::{CloseHandle, HANDLE, INVALID_HANDLE_VALUE};
    use windows_sys::Win32::System::Console::{
        COORD, ClosePseudoConsole, CreatePseudoConsole, HPCON, ResizePseudoConsole,
    };
    use windows_sys::Win32::System::Pipes::CreatePipe;
    use windows_sys::Win32::System::Threading::{
        CreateProcessW, DeleteProcThreadAttributeList, EXTENDED_STARTUPINFO_PRESENT,
        GetExitCodeProcess, InitializeProcThreadAttributeList, LPPROC_THREAD_ATTRIBUTE_LIST,
        PROCESS_INFORMATION, STARTUPINFOEXW, TerminateProcess, UpdateProcThreadAttribute,
        WaitForSingleObject,
    };

    /// Documented in the ConPTY samples and not exported by windows-sys.
    const PROC_THREAD_ATTRIBUTE_PSEUDOCONSOLE: usize = 0x0002_0016;

    pub struct Pty {
        pc: HPCON,
        /// What we write into: the terminal's input.
        input: OwnedHandle,
        /// What we read from: the terminal's output.
        output: OwnedHandle,
        process: OwnedHandle,
        thread: OwnedHandle,
    }

    // The handles are owned by this struct and only touched through it.
    unsafe impl Send for Pty {}

    impl Pty {
        pub fn spawn(req: Spawn<'_>) -> io::Result<Pty> {
            unsafe {
                // Two pipes, crossed: the console reads what we write, and writes what we
                // read. Both ends of each are created, and the two the console keeps are
                // closed here once it has them.
                let (mut in_read, mut in_write) = (INVALID_HANDLE_VALUE, INVALID_HANDLE_VALUE);
                let (mut out_read, mut out_write) = (INVALID_HANDLE_VALUE, INVALID_HANDLE_VALUE);
                if CreatePipe(&mut in_read, &mut in_write, ptr::null(), 0) == 0
                    || CreatePipe(&mut out_read, &mut out_write, ptr::null(), 0) == 0
                {
                    return Err(io::Error::last_os_error());
                }

                // `HPCON` is an integer handle in windows-sys, not a pointer.
                let mut pc: HPCON = 0;
                let size = COORD {
                    X: req.cols.max(1) as i16,
                    Y: req.rows.max(1) as i16,
                };
                let hr = CreatePseudoConsole(size, in_read, out_write, 0, &mut pc);
                // The console holds its own references now.
                CloseHandle(in_read);
                CloseHandle(out_write);
                if hr != 0 {
                    CloseHandle(in_write);
                    CloseHandle(out_read);
                    return Err(io::Error::from_raw_os_error(hr));
                }

                // The attribute list is what ties the new process to the console. Sized by
                // asking, because the size is not ours to assume.
                let mut bytes: usize = 0;
                InitializeProcThreadAttributeList(ptr::null_mut(), 1, 0, &mut bytes);
                let mut attrs = vec![0u8; bytes];
                let list = attrs.as_mut_ptr() as LPPROC_THREAD_ATTRIBUTE_LIST;
                if InitializeProcThreadAttributeList(list, 1, 0, &mut bytes) == 0
                    || UpdateProcThreadAttribute(
                        list,
                        0,
                        PROC_THREAD_ATTRIBUTE_PSEUDOCONSOLE,
                        pc as *const std::ffi::c_void,
                        size_of::<HPCON>(),
                        ptr::null_mut(),
                        ptr::null(),
                    ) == 0
                {
                    let e = io::Error::last_os_error();
                    ClosePseudoConsole(pc);
                    return Err(e);
                }

                let mut si: STARTUPINFOEXW = std::mem::zeroed();
                si.StartupInfo.cb = size_of::<STARTUPINFOEXW>() as u32;
                si.lpAttributeList = list;
                let mut pi: PROCESS_INFORMATION = std::mem::zeroed();

                let mut line = command_line(&default_shell(req.command));
                let cwd = wide(&req.cwd.display().to_string());
                let ok = CreateProcessW(
                    ptr::null(),
                    line.as_mut_ptr(),
                    ptr::null(),
                    ptr::null(),
                    0,
                    EXTENDED_STARTUPINFO_PRESENT,
                    ptr::null(),
                    cwd.as_ptr(),
                    &si.StartupInfo,
                    &mut pi,
                );
                DeleteProcThreadAttributeList(list);
                if ok == 0 {
                    let e = io::Error::last_os_error();
                    ClosePseudoConsole(pc);
                    return Err(e);
                }

                Ok(Pty {
                    pc,
                    input: OwnedHandle::from_raw_handle(in_write as _),
                    output: OwnedHandle::from_raw_handle(out_read as _),
                    process: OwnedHandle::from_raw_handle(pi.hProcess as _),
                    thread: OwnedHandle::from_raw_handle(pi.hThread as _),
                })
            }
        }

        pub fn reader(&self) -> io::Result<Box<dyn Read + Send>> {
            Ok(Box::new(std::fs::File::from(self.output.try_clone()?)))
        }

        pub fn writer(&self) -> io::Result<Box<dyn Write + Send>> {
            Ok(Box::new(std::fs::File::from(self.input.try_clone()?)))
        }

        pub fn resize(&self, cols: u16, rows: u16) -> io::Result<()> {
            let size = COORD {
                X: cols.max(1) as i16,
                Y: rows.max(1) as i16,
            };
            let hr = unsafe { ResizePseudoConsole(self.pc, size) };
            if hr != 0 {
                return Err(io::Error::from_raw_os_error(hr));
            }
            Ok(())
        }

        pub fn kill(&mut self) {
            unsafe {
                TerminateProcess(handle(&self.process), 1);
                WaitForSingleObject(handle(&self.process), 2000);
            }
        }

        pub fn exited(&mut self) -> Option<i32> {
            const STILL_ACTIVE: u32 = 259;
            let mut code: u32 = 0;
            if unsafe { GetExitCodeProcess(handle(&self.process), &mut code) } == 0 {
                return Some(-1);
            }
            (code != STILL_ACTIVE).then_some(code as i32)
        }
    }

    impl Drop for Pty {
        fn drop(&mut self) {
            // The console before the handles: closing it is what tells the child its
            // terminal is gone, and a child holding a pipe nobody reads never exits.
            unsafe { ClosePseudoConsole(self.pc) };
            let _ = &self.thread;
        }
    }

    fn handle(h: &OwnedHandle) -> HANDLE {
        use std::os::windows::io::AsRawHandle;
        h.as_raw_handle() as HANDLE
    }

    fn wide(s: &str) -> Vec<u16> {
        use std::ffi::OsStr;
        use std::os::windows::ffi::OsStrExt;
        OsStr::new(s).encode_wide().chain(Some(0)).collect()
    }

    /// argv joined the way `CreateProcessW` parses it back apart.
    ///
    /// Quoted when a piece contains a space, and inner quotes escaped, because the person
    /// typing `ssh root@host "cd /srv && ls"` means one argument and not three.
    fn command_line(argv: &[String]) -> Vec<u16> {
        let mut line = String::new();
        for (i, part) in argv.iter().enumerate() {
            if i > 0 {
                line.push(' ');
            }
            if part.contains(' ') || part.contains('"') || part.is_empty() {
                line.push('"');
                line.push_str(&part.replace('\\', "\\\\").replace('"', "\\\""));
                line.push('"');
            } else {
                line.push_str(part);
            }
        }
        wide(&line)
    }
}

// ---------------------------------------------------------------------------------------

/// The command to run, or the platform's usual shell when none was named.
fn default_shell(command: &[String]) -> Vec<String> {
    if !command.is_empty() {
        return command.to_vec();
    }
    if cfg!(windows) {
        vec![std::env::var("COMSPEC").unwrap_or_else(|_| "cmd.exe".to_string())]
    } else {
        vec![std::env::var("SHELL").unwrap_or_else(|_| "/bin/sh".to_string())]
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A terminal is not a pipe, and the difference is what these check: the child gets a
    /// device it believes is a terminal, and what it prints comes back with the line endings
    /// a terminal produces.
    #[cfg(unix)]
    #[test]
    fn a_program_runs_in_it_and_its_output_comes_back() {
        let dir = tempfile::tempdir().unwrap();
        let mut pty = Pty::spawn(Spawn {
            command: &[
                "/bin/sh".into(),
                "-c".into(),
                "printf 'hello-from-pty'".into(),
            ],
            cwd: dir.path(),
            cols: 80,
            rows: 24,
        })
        .unwrap();
        let mut out = pty.reader().unwrap();
        let mut buf = Vec::new();
        // Read to end: the master reports EOF once the child is gone and the slave is
        // closed, which is the contract the session loop above depends on.
        let _ = out.read_to_end(&mut buf);
        let text = String::from_utf8_lossy(&buf);
        assert!(text.contains("hello-from-pty"), "{text:?}");
        pty.kill();
    }

    #[cfg(unix)]
    #[test]
    fn the_child_is_told_it_is_a_terminal() {
        let dir = tempfile::tempdir().unwrap();
        // `test -t 0` is the question itself: a pipe answers no, a terminal answers yes.
        let mut pty = Pty::spawn(Spawn {
            command: &[
                "/bin/sh".into(),
                "-c".into(),
                "test -t 0 && printf yes || printf no".into(),
            ],
            cwd: dir.path(),
            cols: 80,
            rows: 24,
        })
        .unwrap();
        let mut buf = Vec::new();
        let _ = pty.reader().unwrap().read_to_end(&mut buf);
        assert_eq!(String::from_utf8_lossy(&buf).trim(), "yes");
        pty.kill();
    }

    /// The size is not decoration: a full-screen program draws to it, and one that thinks
    /// the window is 80x24 when it is not paints over itself.
    #[cfg(unix)]
    #[test]
    fn the_size_is_the_one_asked_for_and_a_resize_reaches_the_child() {
        let dir = tempfile::tempdir().unwrap();
        let mut pty = Pty::spawn(Spawn {
            command: &[
                "/bin/sh".into(),
                "-c".into(),
                // Report on every window change, so the second line is the resize.
                "trap 'stty size' WINCH; stty size; sleep 2".into(),
            ],
            cwd: dir.path(),
            cols: 120,
            rows: 40,
        })
        .unwrap();
        let mut out = pty.reader().unwrap();
        let mut buf = [0u8; 256];
        let n = out.read(&mut buf).unwrap();
        assert!(
            String::from_utf8_lossy(&buf[..n]).contains("40 120"),
            "{:?}",
            String::from_utf8_lossy(&buf[..n])
        );

        pty.resize(100, 30).unwrap();
        let n = out.read(&mut buf).unwrap();
        assert!(
            String::from_utf8_lossy(&buf[..n]).contains("30 100"),
            "the resize did not reach the child: {:?}",
            String::from_utf8_lossy(&buf[..n])
        );
        pty.kill();
    }

    #[cfg(unix)]
    #[test]
    fn what_is_typed_reaches_the_program() {
        let dir = tempfile::tempdir().unwrap();
        let mut pty = Pty::spawn(Spawn {
            command: &[
                "/bin/sh".into(),
                "-c".into(),
                "read line; printf \"got:%s\" \"$line\"".into(),
            ],
            cwd: dir.path(),
            cols: 80,
            rows: 24,
        })
        .unwrap();
        pty.writer().unwrap().write_all(b"typed-this\n").unwrap();
        let mut buf = Vec::new();
        let _ = pty.reader().unwrap().read_to_end(&mut buf);
        assert!(
            String::from_utf8_lossy(&buf).contains("got:typed-this"),
            "{:?}",
            String::from_utf8_lossy(&buf)
        );
        pty.kill();
    }

    #[test]
    fn a_named_command_wins_over_the_default_shell() {
        let named = vec!["ssh".to_string(), "root@example".to_string()];
        assert_eq!(default_shell(&named), named);
        assert_eq!(default_shell(&[]).len(), 1);
    }
}
