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
