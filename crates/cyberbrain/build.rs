//! Refuses to build a binary that would embed a mock web UI.
//!
//! `rust-embed` bakes `ui/dist` into the release binary. A mock bundle renders fabricated
//! compliance figures — "nothing has left this machine", audit rows, egress counts — in the
//! one screen whose entire purpose is to be believed. The page carries a MOCK DATA badge,
//! which guards a developer looking at it and guards nothing at all about a binary handed
//! to somebody else.
//!
//! Set `CYBERBRAIN_ALLOW_MOCK_UI=1` to build one deliberately.

use std::path::Path;

fn main() {
    // Without the `ui` feature nothing is embedded, so there is nothing to guard. That is
    // the configuration a published crate installs under, where `ui/dist` does not exist
    // at all and this check would otherwise fail every `cargo install`.
    if std::env::var_os("CARGO_FEATURE_UI").is_none() {
        return;
    }

    let ui = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../ui/dist");
    println!("cargo:rerun-if-changed={}", ui.display());
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
                "ui/dist is missing or empty; a release binary would embed no web UI at all.\n\
                 Build it first:  cd ui && npm ci && npm run build"
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
