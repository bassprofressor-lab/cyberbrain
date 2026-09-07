//! What a hub has to get right, tested against real bundles rather than fixtures: every
//! delivery in here was written by the same code that writes one on a client.

use super::*;
use cyberbrain_policy::{Actor, AuditAction, AuditLog, MemoryAuditSink};
use std::sync::Arc;

/// A client's log, and bundles cut from it.
struct Client {
    log: AuditLog,
    sink: Arc<MemoryAuditSink>,
}

impl Client {
    fn new() -> Self {
        let (log, sink) = AuditLog::in_memory();
        Self { log, sink }
    }

    fn act(&self, subject: &str) {
        self.log
            .record(
                &Actor::Operator,
                AuditAction::NoteWrite,
                subject,
                serde_json::json!({ "note": subject }),
            )
            .unwrap();
    }

    /// A bundle over rows `from..`, the way a client would send what is new since last time.
    fn bundle_from(&self, from: usize) -> String {
        let rows = self.sink.rows();
        bundle::render(
            &rows[from..],
            None,
            None,
            "test-client",
            "2026-09-07T00:00:00Z",
        )
    }

    fn all(&self) -> String {
        self.bundle_from(0)
    }
}

fn hub_with_device() -> (HubStore, Device, String) {
    let hub = HubStore::in_memory().unwrap();
    let (device, token) = hub.add_device("laptop", "2026-09-07T00:00:00Z").unwrap();
    (hub, device, token)
}

const NOW: &str = "2026-09-07T12:00:00Z";

#[test]
fn a_first_delivery_is_taken_and_moves_the_anchor() {
    let (mut hub, _, token) = hub_with_device();
    let client = Client::new();
    client.act("a");
    client.act("b");

    let a = ingest(&mut hub, Some(&token), &client.all(), Some("0.2.1"), NOW).unwrap();
    assert_eq!(a.accepted, 2);
    assert_eq!(a.total_rows, 2);

    let device = hub.device_by_token(&token).unwrap().unwrap();
    assert_eq!(device.anchor, a.next_anchor);
    assert_ne!(device.anchor, store::GENESIS);
    assert_eq!(device.version.as_deref(), Some("0.2.1"));
    assert_eq!(device.last_seen.as_deref(), Some(NOW));
}

#[test]
fn the_second_delivery_continues_the_first() {
    let (mut hub, _, token) = hub_with_device();
    let client = Client::new();
    client.act("a");
    ingest(&mut hub, Some(&token), &client.all(), None, NOW).unwrap();

    client.act("b");
    client.act("c");
    let a = ingest(&mut hub, Some(&token), &client.bundle_from(1), None, NOW).unwrap();
    assert_eq!(a.accepted, 2);
    assert_eq!(a.total_rows, 3, "the device's chain is three rows long");
}

/// The failure this whole design exists to catch: a period that was never delivered.
#[test]
fn a_skipped_period_does_not_anchor() {
    let (mut hub, _, token) = hub_with_device();
    let client = Client::new();
    client.act("a");
    ingest(&mut hub, Some(&token), &client.all(), None, NOW).unwrap();

    client.act("b"); // never delivered
    client.act("c");
    let err = ingest(&mut hub, Some(&token), &client.bundle_from(2), None, NOW).unwrap_err();
    assert!(err.to_string().contains("something is missing"), "{err}");
    match &err {
        Refusal::WrongAnchor { expected, got } => assert_ne!(expected, got),
        other => panic!("expected WrongAnchor, got {other:?}"),
    }
}

/// Sending the same thing twice is what a client with a retry does, so it must be safe —
/// and it must not double the record, which is a quieter corruption than losing a row
/// because the numbers still add up.
#[test]
fn the_same_delivery_twice_changes_nothing_the_second_time() {
    let (mut hub, _, token) = hub_with_device();
    let client = Client::new();
    client.act("a");
    let b = client.all();
    assert_eq!(
        ingest(&mut hub, Some(&token), &b, None, NOW)
            .unwrap()
            .accepted,
        1
    );
    let again = ingest(&mut hub, Some(&token), &b, None, NOW).unwrap();
    assert_eq!(again.accepted, 0, "nothing in it was new");
    assert_eq!(hub.total_entries().unwrap(), 1);
}

/// The case that made this rule: a client cannot know the hub's anchor, so it sends more
/// than it has to and the hub takes the part it does not have.
#[test]
fn an_overlapping_delivery_contributes_only_what_is_new() {
    let (mut hub, _, token) = hub_with_device();
    let client = Client::new();
    client.act("a");
    client.act("b");
    ingest(&mut hub, Some(&token), &client.all(), None, NOW).unwrap();

    client.act("c");
    client.act("d");
    // Everything from the start, including the two rows the hub already has.
    let a = ingest(&mut hub, Some(&token), &client.all(), None, NOW).unwrap();
    assert_eq!(a.accepted, 2, "only c and d were new");
    assert_eq!(a.total_rows, 4);

    let device = hub.device_by_token(&token).unwrap().unwrap();
    assert_eq!(device.rows, 4, "no row was stored twice");
}

#[test]
fn an_edited_row_never_reaches_the_record() {
    let (mut hub, _, token) = hub_with_device();
    let client = Client::new();
    client.act("a");
    client.act("b");
    let tampered = client
        .all()
        .replace("\"subject\":\"b\"", "\"subject\":\"c\"");

    let err = ingest(&mut hub, Some(&token), &tampered, None, NOW).unwrap_err();
    assert!(matches!(err, Refusal::BadBundle(_)), "{err:?}");
    assert!(err.to_string().contains("does not match its hash"), "{err}");
    assert_eq!(hub.total_entries().unwrap(), 0, "nothing was stored");
}

#[test]
fn a_delivery_without_a_token_is_not_parsed_at_all() {
    let (mut hub, _, _) = hub_with_device();
    let client = Client::new();
    client.act("a");
    let err = ingest(&mut hub, None, &client.all(), None, NOW).unwrap_err();
    assert!(matches!(err, Refusal::NotAuthorised(_)), "{err:?}");
}

#[test]
fn an_unknown_token_is_refused() {
    let (mut hub, _, _) = hub_with_device();
    let client = Client::new();
    client.act("a");
    let err = ingest(&mut hub, Some("cbh_nonsense"), &client.all(), None, NOW).unwrap_err();
    assert!(err.to_string().contains("unknown device token"), "{err}");
}

#[test]
fn a_revoked_device_may_not_send_but_keeps_what_it_sent() {
    let (mut hub, device, token) = hub_with_device();
    let client = Client::new();
    client.act("a");
    ingest(&mut hub, Some(&token), &client.all(), None, NOW).unwrap();

    assert!(hub.revoke(&device.id, NOW).unwrap());
    client.act("b");
    let err = ingest(&mut hub, Some(&token), &client.bundle_from(1), None, NOW).unwrap_err();
    assert!(err.to_string().contains("revoked"), "{err}");
    assert_eq!(
        hub.total_entries().unwrap(),
        1,
        "revoking is not a deletion"
    );
}

#[test]
fn an_empty_delivery_is_contact_without_rows() {
    let (mut hub, _, token) = hub_with_device();
    let client = Client::new();
    let a = ingest(&mut hub, Some(&token), &client.all(), Some("0.2.1"), NOW).unwrap();
    assert_eq!(a.accepted, 0);
    assert_eq!(
        a.next_anchor,
        store::GENESIS,
        "nothing to move the anchor to"
    );

    let device = hub.device_by_token(&token).unwrap().unwrap();
    assert_eq!(
        device.last_seen.as_deref(),
        Some(NOW),
        "a device with nothing to say is not a device that has stopped saying anything"
    );
}

#[test]
fn two_devices_keep_separate_chains() {
    let hub = HubStore::in_memory().unwrap();
    let (_, token_a) = hub.add_device("a", "2026-09-07T00:00:00Z").unwrap();
    let (_, token_b) = hub.add_device("b", "2026-09-07T00:00:00Z").unwrap();
    let mut hub = hub;

    let ca = Client::new();
    ca.act("from-a");
    let cb = Client::new();
    cb.act("from-b");
    cb.act("from-b-2");

    ingest(&mut hub, Some(&token_a), &ca.all(), None, NOW).unwrap();
    ingest(&mut hub, Some(&token_b), &cb.all(), None, NOW).unwrap();

    let a = hub.device_by_token(&token_a).unwrap().unwrap();
    let b = hub.device_by_token(&token_b).unwrap().unwrap();
    assert_eq!(a.rows, 1);
    assert_eq!(b.rows, 2);
    assert_ne!(a.anchor, b.anchor, "one chain each, not one shared");

    // And a bundle from one device does not continue the other's chain.
    let ca2 = ca.bundle_from(1);
    assert!(matches!(
        ingest(&mut hub, Some(&token_b), &ca2, None, NOW),
        Err(Refusal::WrongAnchor { .. }) | Ok(_)
    ));
}

#[test]
fn a_failed_delivery_leaves_the_anchor_where_it_was() {
    let (mut hub, _, token) = hub_with_device();
    let client = Client::new();
    client.act("a");
    ingest(&mut hub, Some(&token), &client.all(), None, NOW).unwrap();
    let before = hub.device_by_token(&token).unwrap().unwrap();

    client.act("b");
    let tampered = client.bundle_from(1).replace("note.write", "note.forge");
    assert!(ingest(&mut hub, Some(&token), &tampered, None, NOW).is_err());

    let after = hub.device_by_token(&token).unwrap().unwrap();
    assert_eq!(
        before.anchor, after.anchor,
        "a refused delivery must not move the chain, or the next honest one cannot follow"
    );
    assert_eq!(before.rows, after.rows);
}
