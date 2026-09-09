//! SPEC §12.1: "CI whitelists HTTP-client construction only at call sites that take an
//! `EgressGate`."
//!
//! This is that check, run as a test so it cannot be forgotten. It walks every crate in the
//! workspace and fails if any Rust source constructs an HTTP client or opens a raw outbound
//! socket, unless:
//!
//! - the file is `cyberbrain-policy/src/egress/transport.rs`, whose functions take an
//!   `EgressTicket` that only the gate can issue; or
//! - the enclosing function's signature names `EgressGate`, meaning the call site holds a
//!   gate and can be checked for calling `permit`.
//!
//! It also fails if a crate other than `cyberbrain-policy` or `cyberbrain-llm` declares an
//! HTTP client or a self-fetching dependency: a dependency that is present will eventually
//! be used. Inbound listeners (`TcpListener`, axum's `serve`) are allowed; the web UI binds
//! loopback.
//!
//! # The one exception, and why it is about capability rather than about a name
//!
//! `hyper` and `hyper-util` are both halves in one crate each: the client and the server.
//! The hub terminates its own TLS and therefore drives connections itself, which needs the
//! server half — and forbidding the name would have meant either giving up on encrypting
//! the hub or asking every customer to install a reverse proxy, which is how a rule stops
//! protecting anything and starts being worked around.
//!
//! So the rule reads the declaration instead: `default-features = false` and no feature
//! whose name contains `client`. That is checkable, it is what the compiler acts on, and a
//! crate declared this way has no client to reach for. The source-level check above is
//! unchanged and still catches `hyper::Client`, `hyper_util::client` and
//! `TcpStream::connect` wherever no gate is held.

use std::fs;
use std::path::{Path, PathBuf};

const TRANSPORT: &str = "cyberbrain-policy/src/egress/transport.rs";

/// Source patterns that mean "this code can send bytes off the machine".
const CLIENT_CONSTRUCTION: &[&str] = &[
    "reqwest::Client::builder",
    "reqwest::Client::new",
    "reqwest::ClientBuilder",
    "reqwest::blocking",
    "reqwest::get(",
    "ureq::",
    "hyper::Client",
    "hyper_util::client",
    "isahc::",
    "attohttpc::",
    "minreq::",
    "curl::",
    "hf_hub::",
    "TcpStream::connect",
    "UdpSocket::bind",
    "UdpSocket::connect",
    "TcpSocket::new",
];

/// Crates that perform network I/O of their own (SPEC §15: disqualified outside the gate).
const NETWORK_CRATES: &[&str] = &[
    "reqwest",
    "ureq",
    "hyper",
    "hyper-util",
    "isahc",
    "attohttpc",
    "minreq",
    "curl",
    "hf-hub",
    "hf_hub",
    "surf",
    "awc",
];

/// Crates allowed to depend on an HTTP client at all.
const CRATES_WITH_CLIENTS: &[&str] = &["cyberbrain-policy", "cyberbrain-llm"];

/// Crates that carry a client and a server, and may be declared for the server half alone.
const SPLIT_CRATES: &[&str] = &["hyper", "hyper-util"];

/// Whether a dependency line takes only the server half: defaults off, and no feature that
/// names the client. Written against the line as cargo reads it, so there is no second
/// opinion about what was enabled.
fn server_half_only(line: &str) -> bool {
    let Some((_, rest)) = line.split_once('{') else {
        // A bare `hyper = "1"` takes whatever the defaults are, which is not a decision
        // anybody made here.
        return false;
    };
    let compact = rest.replace(' ', "");
    if !compact.contains("default-features=false") {
        return false;
    }
    // Split on the whole `features=[`, not on the word: `default-features` ends in the same
    // eight letters, and matching those alone read the tail of *that* key as the feature
    // list. A declaration with no list at all then looked like a list with no client in it,
    // which is the one answer this function must never give.
    let Some(list) = compact.split("features=[").nth(1) else {
        return false;
    };
    let list = list.split(']').next().unwrap_or("");
    !list.is_empty() && !list.contains("client")
}

fn workspace_root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../..")
        .canonicalize()
        .unwrap()
}

fn walk(dir: &Path, ext: &str, out: &mut Vec<PathBuf>) {
    let Ok(rd) = fs::read_dir(dir) else { return };
    for entry in rd.flatten() {
        let p = entry.path();
        if p.is_dir() {
            let name = p.file_name().unwrap().to_string_lossy();
            if name == "target" || name == "node_modules" || name.starts_with('.') {
                continue;
            }
            walk(&p, ext, out);
        } else if p.extension().is_some_and(|e| e == ext) {
            out.push(p);
        }
    }
}

fn slash(p: &Path) -> String {
    p.to_string_lossy().replace('\\', "/")
}

/// The signature of the function enclosing `line_idx`: from its `fn` line to its opening
/// brace. `None` if no `fn` precedes it (module-level code).
fn enclosing_fn_signature(lines: &[&str], line_idx: usize) -> Option<String> {
    let is_fn_line = |l: &str| {
        let t = l.trim_start();
        // Strip a visibility modifier: `pub`, or `pub(crate)` / `pub(super)` / `pub(in ...)`.
        // The previous version stripped every alphanumeric, paren, colon and space after
        // `pub `, which also ate `fn build_http_client(` — so no `pub fn` was ever
        // recognised as a function head, and a violation inside one got attributed to
        // whatever unrelated private function happened to sit above it. That is how this
        // check reported the right file and the wrong reason.
        let t = match t.strip_prefix("pub") {
            Some(rest) => {
                let rest = rest.trim_start();
                match rest.strip_prefix('(') {
                    Some(inner) => inner
                        .split_once(')')
                        .map(|(_, after)| after.trim_start())
                        .unwrap_or(rest),
                    None => rest,
                }
            }
            None => t,
        };
        let t = t.strip_prefix("const ").unwrap_or(t);
        let t = t.strip_prefix("async ").unwrap_or(t);
        let t = t.strip_prefix("unsafe ").unwrap_or(t);
        t.starts_with("fn ")
    };
    let start = (0..=line_idx).rev().find(|i| is_fn_line(lines[*i]))?;
    let mut sig = String::new();
    for l in &lines[start..=line_idx] {
        sig.push_str(l);
        sig.push(' ');
        if l.contains('{') {
            break;
        }
    }
    Some(sig)
}

#[test]
fn http_clients_are_built_only_where_an_egress_gate_is_held() {
    let root = workspace_root();
    let mut files = Vec::new();
    walk(&root.join("crates"), "rs", &mut files);
    assert!(
        files.len() > 5,
        "found too few .rs files under {}; is the walk broken?",
        root.display()
    );

    // Test-only code is out of scope, and the reason is narrow: this rule protects the
    // paths that exist in the shipped binary, and nothing under `#[cfg(test)]` is in it. A
    // test connecting to its own loopback server is not an egress path.
    //
    // The exclusion is counted and printed rather than applied quietly. A check that
    // silently stops looking at part of the tree still reports "ok", which is exactly the
    // shape of failure this whole file exists to prevent.
    let is_test_only = |f: &std::path::PathBuf| {
        let p = slash(f);
        p.ends_with("/tests.rs") || p.contains("/tests/") || p.ends_with("/benches.rs")
    };
    let skipped: Vec<String> = files
        .iter()
        .filter(|f| is_test_only(f))
        .map(|f| slash(f))
        .collect();
    println!(
        "scanned {} file(s); {} skipped as test-only: {}",
        files.len() - skipped.len(),
        skipped.len(),
        skipped.join(", ")
    );

    let mut offences = Vec::new();
    for f in files
        .iter()
        .filter(|f| !slash(f).ends_with(TRANSPORT) && !is_test_only(f))
    {
        let src = fs::read_to_string(f).unwrap();
        let lines: Vec<&str> = src.lines().collect();
        for (i, line) in lines.iter().enumerate() {
            if line.trim_start().starts_with("//") {
                continue;
            }
            for pat in CLIENT_CONSTRUCTION {
                if !line.contains(pat) {
                    continue;
                }
                let sig = enclosing_fn_signature(&lines, i).unwrap_or_default();
                if sig.contains("EgressGate") {
                    continue;
                }
                offences.push(format!(
                    "{}:{}: `{pat}` in a function that does not take an EgressGate\n      {}",
                    slash(f.strip_prefix(&root).unwrap()),
                    i + 1,
                    line.trim()
                ));
            }
        }
    }
    assert!(
        offences.is_empty(),
        "SPEC §12.1: an HTTP client may be built only where an EgressGate is held and \
         `permit` is called first, or inside cyberbrain_policy::egress::transport.\n  {}",
        offences.join("\n  ")
    );
}

#[test]
fn network_dependencies_are_declared_only_by_policy_and_llm() {
    let root = workspace_root();
    let mut manifests = Vec::new();
    walk(&root.join("crates"), "toml", &mut manifests);
    let mut offences = Vec::new();
    for m in manifests
        .iter()
        .filter(|m| m.file_name().is_some_and(|n| n == "Cargo.toml"))
    {
        let crate_dir = m
            .parent()
            .unwrap()
            .file_name()
            .unwrap()
            .to_string_lossy()
            .to_string();
        if CRATES_WITH_CLIENTS.contains(&crate_dir.as_str()) {
            continue;
        }
        let text = fs::read_to_string(m).unwrap();
        let mut in_deps = false;
        for (lineno, line) in text.lines().enumerate() {
            let t = line.trim();
            if t.starts_with('[') {
                in_deps = t.contains("dependencies");
                continue;
            }
            if !in_deps || t.starts_with('#') || t.is_empty() {
                continue;
            }
            let key = t
                .split(['=', ' ', '.'])
                .next()
                .unwrap_or("")
                .trim_matches('"');
            if SPLIT_CRATES.contains(&key) && server_half_only(t) {
                continue;
            }
            if NETWORK_CRATES.contains(&key) {
                offences.push(format!(
                    "{}:{}: {t}",
                    slash(m.strip_prefix(&root).unwrap()),
                    lineno + 1
                ));
            }
        }
    }
    assert!(
        offences.is_empty(),
        "SPEC §12.1/§15: only cyberbrain-policy and cyberbrain-llm may depend on an HTTP \
         client or a self-fetching crate. Remove:\n  {}",
        offences.join("\n  ")
    );
}

#[test]
fn the_server_half_exception_does_not_let_a_client_through() {
    // The predicate is the whole exception, so it is tested against the shapes it has to
    // refuse rather than only the one it has to allow.
    assert!(server_half_only(
        "hyper-util = { version = \"0.1.20\", default-features = false, features = [\"server\", \"tokio\"] }"
    ));
    // Defaults left on: nobody decided what came with them.
    assert!(!server_half_only(
        "hyper-util = { version = \"0.1.20\", features = [\"server\"] }"
    ));
    // The client half, asked for by name.
    assert!(!server_half_only(
        "hyper-util = { version = \"0.1.20\", default-features = false, features = [\"client\", \"server\"] }"
    ));
    assert!(!server_half_only(
        "hyper-util = { version = \"0.1.20\", default-features = false, features = [\"client-legacy\"] }"
    ));
    // No features at all is not a server-only declaration either.
    assert!(!server_half_only(
        "hyper-util = { version = \"0.1.20\", default-features = false }"
    ));
    assert!(!server_half_only("hyper-util = \"0.1.20\""));
}
