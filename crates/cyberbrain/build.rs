//! Refuses to build a binary that would embed a mock web UI.
//!
//! `rust-embed` bakes `ui/dist` into the release binary. A mock bundle renders fabricated
//! compliance figures — "nothing has left this machine", audit rows, egress counts — in the
//! one screen whose entire purpose is to be believed. The page carries a MOCK DATA badge,
//! which guards a developer looking at it and guards nothing at all about a binary handed
//! to somebody else.
//!
//! Set `CYBERBRAIN_ALLOW_MOCK_UI=1` to build one deliberately.

use std::path::{Path, PathBuf};

/// Where the built page is, in the two places it can be.
///
/// In a source checkout it is `ui/dist` at the repository root, which is where the node
/// build puts it and where it stays out of the crate. That path cannot survive publishing:
/// `cargo package` takes nothing from outside the package directory, silently, so a crate
/// published from this tree would carry no page and `cargo install cyberbrain --features
/// ui` would fail in this script. The release step therefore copies the built page to
/// `crates/cyberbrain/ui-dist`, which is inside the package and listed in `include`.
///
/// The in-crate copy wins when it exists, so a published crate embeds the page it shipped
/// with, and a working tree keeps embedding the page you just built.
fn ui_dir(manifest: &Path) -> PathBuf {
    let packaged = manifest.join("ui-dist");
    if packaged.join("index.html").is_file() {
        packaged
    } else {
        manifest.join("../../ui/dist")
    }
}

fn main() {
    // Without the `ui` feature nothing is embedded, so there is nothing to guard.
    if std::env::var_os("CARGO_FEATURE_UI").is_none() {
        return;
    }

    let manifest = Path::new(env!("CARGO_MANIFEST_DIR"));
    let ui = ui_dir(manifest);
    // rust-embed reads this: the folder is `$CYBERBRAIN_UI_DIR`, expanded at compile time.
    println!("cargo:rustc-env=CYBERBRAIN_UI_DIR={}", ui.display());
    println!("cargo:rerun-if-changed={}", ui.display());
    println!(
        "cargo:rerun-if-changed={}",
        manifest.join("ui-dist").display()
    );
    println!("cargo:rerun-if-env-changed=CYBERBRAIN_ALLOW_MOCK_UI");

    if std::env::var("CYBERBRAIN_ALLOW_MOCK_UI").is_ok_and(|v| v == "1") {
        println!("cargo:warning=embedding a MOCK web UI on purpose; do not ship this binary");
        return;
    }

    // Debug builds serve dist/ from disk, so an absent or half-built UI is a nuisance
    // rather than a hazard. A release build bakes whatever is there in, permanently.
    let release = std::env::var("PROFILE").as_deref() == Ok("release");
    if !ui.join("index.html").is_file() {
        if release {
            panic!(
                "{} is missing or empty; a release binary would embed no web UI at all.\n\
                 In a checkout:        cd ui && npm ci && npm run build\n\
                 In a published crate: the page was not copied to crates/cyberbrain/ui-dist \
                 before cargo publish; install with --no-default-features until it is",
                ui.display()
            );
        }
        println!("cargo:warning=ui/dist is missing; `cyberbrain serve` will have no UI");
        return;
    }

    match std::fs::read_to_string(ui.join("transport.txt")) {
        Ok(t) if t.trim() == "http" => {}
        Ok(t) if release => panic!(
            "ui/dist was built with transport {:?}: it renders fabricated data.\n\
             Rebuild it:  cd ui && npm run build",
            t.trim()
        ),
        Ok(t) => println!(
            "cargo:warning=ui/dist transport is {:?}, not http",
            t.trim()
        ),
        Err(_) if release => panic!(
            "ui/dist has no transport.txt, so there is no way to tell whether it renders \
             real data or mock data.\nRebuild it:  cd ui && npm run build"
        ),
        Err(_) => println!("cargo:warning=ui/dist has no transport.txt; rebuild it"),
    }
}
