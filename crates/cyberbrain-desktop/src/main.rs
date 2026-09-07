//! Cyberbrain for Windows without a terminal: a Start menu entry that opens the memory.
//!
//! The product is a command-line tool that serves its own page, so the desktop question was
//! never "port it" — the binary already runs on Windows. It was "what is the way in". This
//! is that way in and nothing more: it starts `cyberbrain serve` on a port the operating
//! system picks, opens the browser at it, and sits in the notification area until told to
//! stop. No second implementation of anything, no copy of the UI, nothing to keep in sync.
//!
//! Windows only, and honest about it: on any other platform it prints where the same thing
//! lives on the command line and exits.

// No console window. The whole point is that clicking it shows the page, not a terminal.
#![cfg_attr(windows, windows_subsystem = "windows")]

#[cfg_attr(not(windows), allow(dead_code))]
mod launch;
#[cfg_attr(not(windows), allow(dead_code))]
mod settings;

#[cfg(windows)]
mod win;

#[cfg(windows)]
fn main() {
    win::run();
}

#[cfg(not(windows))]
fn main() {
    eprintln!(
        "cyberbrain-desktop is the Windows launcher. On this platform the same thing is\n\
         one command: `cyberbrain serve`, then open the address it prints."
    );
    std::process::exit(2);
}
