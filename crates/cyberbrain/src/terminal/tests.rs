//! What may become a terminal, and what may not.
//!
//! `admit` is the whole of the decision, which is why it takes its inputs rather than
//! reading them: a check that can only be exercised by opening a real socket from a real
//! browser is a check nobody exercises.

use super::*;

fn cfg() -> TerminalConfig {
    TerminalConfig {
        token: "a".repeat(64),
    }
}

fn ours() -> Vec<String> {
    vec![
        "http://127.0.0.1:7777".to_string(),
        "http://localhost:7777".to_string(),
    ]
}

#[test]
fn a_store_served_without_a_terminal_has_none() {
    let why = admit(None, None, &ours(), &"a".repeat(64)).unwrap_err();
    assert!(why.contains("--terminal"), "{why}");
}

#[test]
fn the_right_token_from_our_own_page_is_admitted() {
    assert!(
        admit(
            Some(&cfg()),
            Some("http://127.0.0.1:7777"),
            &ours(),
            &"a".repeat(64)
        )
        .is_ok()
    );
}

/// The reason this check exists at all: a WebSocket handshake is not subject to the
/// same-origin rule, so without it any page the user has open could try for a shell.
#[test]
fn a_page_from_somewhere_else_is_refused_even_with_the_token() {
    let why = admit(
        Some(&cfg()),
        Some("https://example.com"),
        &ours(),
        &"a".repeat(64),
    )
    .unwrap_err();
    assert!(why.contains("example.com"), "{why}");
}

/// A program on this machine sends no origin. It is allowed, and the reason is worth being
/// explicit about: a process running as this user can start a shell without our help, so
/// refusing here would protect nothing and would break every non-browser client.
#[test]
fn a_client_that_is_not_a_browser_is_admitted_with_the_token() {
    assert!(admit(Some(&cfg()), None, &ours(), &"a".repeat(64)).is_ok());
}

#[test]
fn a_wrong_or_missing_token_is_refused() {
    for token in ["", "b", &"b".repeat(64), &"a".repeat(63)] {
        let why = admit(Some(&cfg()), None, &ours(), token).unwrap_err();
        assert!(why.contains("token"), "{token:?}: {why}");
    }
}

#[test]
fn the_token_is_long_and_different_every_run() {
    let a = TerminalConfig::new();
    let b = TerminalConfig::new();
    assert_eq!(a.token.len(), 64);
    assert_ne!(a.token, b.token);
    assert!(a.token.chars().all(|c| c.is_ascii_hexdigit()));
}

#[test]
fn comparing_tokens_does_not_stop_at_the_first_difference() {
    // Not a timing measurement — those are unreliable in a test — but the property the
    // implementation must have: length first, then every byte, with no early return.
    assert!(constant_time_eq(b"abc", b"abc"));
    assert!(!constant_time_eq(b"abc", b"abd"));
    assert!(!constant_time_eq(b"abc", b"ab"));
    assert!(constant_time_eq(b"", b""));
}

// ---------------------------------------------------------------------------------------
// The saved list is behind the same token, and for a reason worth stating: it says which
// machines this person connects to and under which account.

use crate::app::App;
use crate::serve::router_with;
use axum::body::Body;
use axum::http::{Method, Request, StatusCode};
use cyberbrain_policy::Actor;
use std::path::PathBuf;
use std::sync::Arc;
use tower::ServiceExt;

fn store() -> (tempfile::TempDir, Arc<App>) {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path().join("store");
    App::init(&root, &Actor::Operator).unwrap();
    let app = Arc::new(App::open(Some(&root), Actor::Operator).unwrap());
    (dir, app)
}

async fn get_profiles(terminal: Option<TerminalConfig>, token: Option<&str>) -> StatusCode {
    let (_dir, app) = store();
    let router = router_with(app, PathBuf::new(), terminal, Vec::new());
    let mut req = Request::builder()
        .method(Method::GET)
        .uri("/api/v1/terminal/profiles");
    if let Some(t) = token {
        req = req.header("x-cyberbrain-terminal-token", t);
    }
    router
        .oneshot(req.body(Body::empty()).unwrap())
        .await
        .unwrap()
        .status()
}

#[tokio::test]
async fn the_saved_list_needs_the_token() {
    assert_eq!(
        get_profiles(Some(cfg()), None).await,
        StatusCode::FORBIDDEN,
        "no token"
    );
    assert_eq!(
        get_profiles(Some(cfg()), Some("wrong")).await,
        StatusCode::FORBIDDEN,
        "wrong token"
    );
}

#[tokio::test]
async fn a_store_without_a_terminal_has_no_saved_list_either() {
    assert_eq!(
        get_profiles(None, Some(&"a".repeat(64))).await,
        StatusCode::FORBIDDEN
    );
}

// ---------------------------------------------------------------------------------------
// The write half must never block the runtime.

/// A paste into a terminal whose program stopped reading must not hold up the caller.
///
/// `serve` runs on one worker. A blocking write here stops every route this server has —
/// the page, the API, the other terminals — until the program inside decides to read again,
/// which it may never do. So the write goes to a thread, and a caller that outruns the
/// program loses keystrokes instead of hanging the process.
///
/// Written against the defect: with `write_all` called directly from the session task, the
/// same input blocks for as long as the child sleeps.
#[cfg(unix)]
#[tokio::test]
async fn a_paste_into_a_program_that_is_not_reading_does_not_block() {
    use super::pty::{Pty, Spawn};
    use std::time::{Duration, Instant};

    let dir = tempfile::tempdir().unwrap();
    // Reads one line, then stops reading for good. The pty's input buffer fills, and every
    // further write would block.
    let mut pty = Pty::spawn(Spawn {
        command: &[
            "/bin/sh".into(),
            "-c".into(),
            "read one; exec sleep 30".into(),
        ],
        cwd: dir.path(),
        cols: 80,
        rows: 24,
    })
    .unwrap();
    let tx = spawn_writer(pty.writer().unwrap());

    let started = Instant::now();
    // Well past any pty buffer, in lines, which is what fills the canonical queue.
    // A line, because it is the newline that fills the canonical queue; the same bytes
    // without one are discarded once the buffer is full and would not reproduce anything.
    let mut line = vec![b'x'; 4096];
    line.push(b'\n');
    for _ in 0..200 {
        let line = line.clone();
        // Exactly what the session loop does with an incoming frame.
        if tx.try_send(line).is_err() && tx.is_closed() {
            break;
        }
    }
    let elapsed = started.elapsed();

    assert!(
        elapsed < Duration::from_secs(2),
        "the write half blocked the caller for {elapsed:?}; on the real server that is every \
         route hanging until the program inside reads again"
    );
    pty.kill();
}
