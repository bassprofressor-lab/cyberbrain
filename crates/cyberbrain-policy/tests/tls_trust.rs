//! Where TLS trust comes from, checked rather than asserted in prose.
//!
//! SPEC §12.1 said for months that no root store was available and that `https://` failed
//! closed unless the operator supplied a CA. The client had been using the platform's
//! certificate store the whole time — a public HTTPS host answered on the first try. Nobody
//! noticed, because nothing compared the sentence to the tree.
//!
//! This is that comparison. It fails when the trust arrangement changes, which is the moment
//! the documentation and the register have to change with it.

use std::fs;
use std::path::{Path, PathBuf};

fn workspace_root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../..")
        .canonicalize()
        .unwrap()
}

fn lockfile() -> String {
    fs::read_to_string(workspace_root().join("Cargo.lock")).expect("the workspace has a lockfile")
}

fn has_crate(lock: &str, name: &str) -> bool {
    // Anchored to the line, because "webpki-roots" is a substring of nothing else here but
    // "rustls-webpki" is a substring of plenty, and a check that matches the wrong crate is
    // worse than none.
    lock.lines()
        .any(|l| l.trim() == format!("name = \"{name}\""))
}

/// The check has to fail on the arrangement it exists to catch. Fed synthetic lockfiles
/// rather than the real one, because cargo rewrites that file before a test can read a
/// doctored copy — the first attempt at this calibration passed while measuring nothing.
#[test]
fn the_check_notices_a_compiled_in_bundle() {
    let platform = "[[package]]\nname = \"rustls-platform-verifier\"\nversion = \"0.7.0\"\n";
    let bundle = "[[package]]\nname = \"webpki-roots\"\nversion = \"0.26.0\"\n";
    let webpki_lib = "[[package]]\nname = \"rustls-webpki\"\nversion = \"0.103.15\"\n";

    assert!(has_crate(platform, "rustls-platform-verifier"));
    assert!(
        !has_crate(platform, "webpki-roots"),
        "not present, not found"
    );
    assert!(has_crate(bundle, "webpki-roots"), "present, found");
    assert!(
        !has_crate(webpki_lib, "webpki-roots"),
        "rustls-webpki is the verification library, not a bundle of roots; matching it here \
         would make this test cry wolf on every build"
    );
}

/// The arrangement as it actually is: the platform verifier, and no bundle of our own.
#[test]
fn trust_comes_from_the_platform_and_not_from_a_bundle_we_ship() {
    let lock = lockfile();

    assert!(
        has_crate(&lock, "rustls-platform-verifier"),
        "TLS trust no longer comes from the platform. That is a change to what SPEC §12.1 \
         promises and to what `policy egress` prints — change both, then this test."
    );

    // The alternative arrangement, and the one the old wording described: a list of root
    // certificates compiled into the binary. If this ever appears, the trust anchors stop
    // being the operator's to manage.
    assert!(
        !has_crate(&lock, "webpki-roots"),
        "a certificate bundle is compiled into the binary now. An organisation removing a CA \
         from its own store would no longer stop this program trusting it, which is the \
         property SPEC §12.1 exists to keep."
    );
}

/// The three places that describe this must agree. A sentence in one of them and a different
/// arrangement in the tree is what this whole test file is here to prevent.
#[test]
fn the_spec_and_the_register_say_the_same_thing() {
    let spec = fs::read_to_string(workspace_root().join("docs/SPEC.md")).expect("SPEC.md");

    let claim = "TLS trust comes from the operating system";
    assert!(
        spec.contains(claim),
        "SPEC §12.1 no longer states where TLS trust comes from"
    );
    // The wording that was wrong, kept here by its exact shape so it cannot come back.
    assert!(
        !spec.contains("No root certificate store is compiled in, so"),
        "the old claim is back in the SPEC: it says https fails closed without an operator \
         CA, and the platform verifier means it does not"
    );

    let register = cyberbrain_policy::egress::TLS_TRUST;
    assert!(
        register.contains("operating system's certificate store"),
        "the register no longer names where trust comes from: {register}"
    );
    // And the register makes the point that matters: trust does not widen the destinations.
    assert!(
        register.contains("never which destinations are allowed"),
        "the register stopped saying that the certificate store does not widen the list of \
         permitted destinations, which is the actual control: {register}"
    );
}
