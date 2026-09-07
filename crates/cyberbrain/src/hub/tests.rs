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

/// A licence that allows collecting, for the tests that are about delivery rather than
/// about licensing.
fn collecting() -> LicenceState {
    LicenceState::Valid {
        customer: "Test GmbH".into(),
        seats: 5,
        valid_until: "2099-01-01T00:00:00Z".into(),
        warning: None,
    }
}

fn lapsed() -> LicenceState {
    LicenceState::Expired {
        customer: "Test GmbH".into(),
        valid_until: "2026-01-01T00:00:00Z".into(),
    }
}

#[test]
fn a_first_delivery_is_taken_and_moves_the_anchor() {
    let (mut hub, _, token) = hub_with_device();
    let client = Client::new();
    client.act("a");
    client.act("b");

    let a = ingest(
        &mut hub,
        &collecting(),
        Some(&token),
        &client.all(),
        Some("0.2.1"),
        NOW,
    )
    .unwrap();
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
    ingest(
        &mut hub,
        &collecting(),
        Some(&token),
        &client.all(),
        None,
        NOW,
    )
    .unwrap();

    client.act("b");
    client.act("c");
    let a = ingest(
        &mut hub,
        &collecting(),
        Some(&token),
        &client.bundle_from(1),
        None,
        NOW,
    )
    .unwrap();
    assert_eq!(a.accepted, 2);
    assert_eq!(a.total_rows, 3, "the device's chain is three rows long");
}

/// The failure this whole design exists to catch: a period that was never delivered.
#[test]
fn a_skipped_period_does_not_anchor() {
    let (mut hub, _, token) = hub_with_device();
    let client = Client::new();
    client.act("a");
    ingest(
        &mut hub,
        &collecting(),
        Some(&token),
        &client.all(),
        None,
        NOW,
    )
    .unwrap();

    client.act("b"); // never delivered
    client.act("c");
    let err = ingest(
        &mut hub,
        &collecting(),
        Some(&token),
        &client.bundle_from(2),
        None,
        NOW,
    )
    .unwrap_err();
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
        ingest(&mut hub, &collecting(), Some(&token), &b, None, NOW)
            .unwrap()
            .accepted,
        1
    );
    let again = ingest(&mut hub, &collecting(), Some(&token), &b, None, NOW).unwrap();
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
    ingest(
        &mut hub,
        &collecting(),
        Some(&token),
        &client.all(),
        None,
        NOW,
    )
    .unwrap();

    client.act("c");
    client.act("d");
    // Everything from the start, including the two rows the hub already has.
    let a = ingest(
        &mut hub,
        &collecting(),
        Some(&token),
        &client.all(),
        None,
        NOW,
    )
    .unwrap();
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

    let err = ingest(&mut hub, &collecting(), Some(&token), &tampered, None, NOW).unwrap_err();
    assert!(matches!(err, Refusal::BadBundle(_)), "{err:?}");
    assert!(err.to_string().contains("does not match its hash"), "{err}");
    assert_eq!(hub.total_entries().unwrap(), 0, "nothing was stored");
}

#[test]
fn a_delivery_without_a_token_is_not_parsed_at_all() {
    let (mut hub, _, _) = hub_with_device();
    let client = Client::new();
    client.act("a");
    let err = ingest(&mut hub, &collecting(), None, &client.all(), None, NOW).unwrap_err();
    assert!(matches!(err, Refusal::NotAuthorised(_)), "{err:?}");
}

#[test]
fn an_unknown_token_is_refused() {
    let (mut hub, _, _) = hub_with_device();
    let client = Client::new();
    client.act("a");
    let err = ingest(
        &mut hub,
        &collecting(),
        Some("cbh_nonsense"),
        &client.all(),
        None,
        NOW,
    )
    .unwrap_err();
    assert!(err.to_string().contains("unknown device token"), "{err}");
}

#[test]
fn a_revoked_device_may_not_send_but_keeps_what_it_sent() {
    let (mut hub, device, token) = hub_with_device();
    let client = Client::new();
    client.act("a");
    ingest(
        &mut hub,
        &collecting(),
        Some(&token),
        &client.all(),
        None,
        NOW,
    )
    .unwrap();

    assert!(hub.revoke(&device.id, NOW).unwrap());
    client.act("b");
    let err = ingest(
        &mut hub,
        &collecting(),
        Some(&token),
        &client.bundle_from(1),
        None,
        NOW,
    )
    .unwrap_err();
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
    let a = ingest(
        &mut hub,
        &collecting(),
        Some(&token),
        &client.all(),
        Some("0.2.1"),
        NOW,
    )
    .unwrap();
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

    ingest(
        &mut hub,
        &collecting(),
        Some(&token_a),
        &ca.all(),
        None,
        NOW,
    )
    .unwrap();
    ingest(
        &mut hub,
        &collecting(),
        Some(&token_b),
        &cb.all(),
        None,
        NOW,
    )
    .unwrap();

    let a = hub.device_by_token(&token_a).unwrap().unwrap();
    let b = hub.device_by_token(&token_b).unwrap().unwrap();
    assert_eq!(a.rows, 1);
    assert_eq!(b.rows, 2);
    assert_ne!(a.anchor, b.anchor, "one chain each, not one shared");

    // And a bundle from one device does not continue the other's chain.
    let ca2 = ca.bundle_from(1);
    assert!(matches!(
        ingest(&mut hub, &collecting(), Some(&token_b), &ca2, None, NOW),
        Err(Refusal::WrongAnchor { .. }) | Ok(_)
    ));
}

#[test]
fn a_failed_delivery_leaves_the_anchor_where_it_was() {
    let (mut hub, _, token) = hub_with_device();
    let client = Client::new();
    client.act("a");
    ingest(
        &mut hub,
        &collecting(),
        Some(&token),
        &client.all(),
        None,
        NOW,
    )
    .unwrap();
    let before = hub.device_by_token(&token).unwrap().unwrap();

    client.act("b");
    let tampered = client.bundle_from(1).replace("note.write", "note.forge");
    assert!(ingest(&mut hub, &collecting(), Some(&token), &tampered, None, NOW).is_err());

    let after = hub.device_by_token(&token).unwrap().unwrap();
    assert_eq!(
        before.anchor, after.anchor,
        "a refused delivery must not move the chain, or the next honest one cannot follow"
    );
    assert_eq!(before.rows, after.rows);
}

// ---- what the licence gates, and what it does not (slice 4) ----

/// The test for the promise in the design: expiry stops collection and touches nothing else.
#[test]
fn an_expired_licence_stops_collection_and_keeps_everything_else() {
    let (mut hub, _, token) = hub_with_device();
    let client = Client::new();
    client.act("a");
    ingest(
        &mut hub,
        &collecting(),
        Some(&token),
        &client.all(),
        None,
        NOW,
    )
    .unwrap();
    assert_eq!(hub.total_entries().unwrap(), 1);

    client.act("b");
    let err = ingest(&mut hub, &lapsed(), Some(&token), &client.all(), None, NOW).unwrap_err();
    assert!(matches!(err, Refusal::NotCollecting(_)), "{err:?}");
    assert!(err.to_string().contains("Keep buffering"), "{err}");
    assert!(err.to_string().contains("stays readable"), "{err}");

    // Nothing was lost, nothing was locked, and the record still reads.
    assert_eq!(hub.total_entries().unwrap(), 1);
    assert_eq!(hub.devices().unwrap().len(), 1);
    assert_eq!(hub.device_by_token(&token).unwrap().unwrap().rows, 1);
}

/// Renewing picks up exactly where it stopped: what the client held lands, chain unbroken.
#[test]
fn renewing_takes_what_the_client_held() {
    let (mut hub, _, token) = hub_with_device();
    let client = Client::new();
    client.act("a");
    client.act("b");
    assert!(ingest(&mut hub, &lapsed(), Some(&token), &client.all(), None, NOW).is_err());

    let a = ingest(
        &mut hub,
        &collecting(),
        Some(&token),
        &client.all(),
        None,
        NOW,
    )
    .unwrap();
    assert_eq!(a.accepted, 2, "the rows held during the lapse arrive");
}

#[test]
fn a_hub_without_a_licence_collects_nothing() {
    let (mut hub, _, token) = hub_with_device();
    let client = Client::new();
    client.act("a");
    let err = ingest(
        &mut hub,
        &LicenceState::Missing,
        Some(&token),
        &client.all(),
        None,
        NOW,
    )
    .unwrap_err();
    assert!(matches!(err, Refusal::NotCollecting(_)), "{err:?}");
    assert!(err.to_string().contains("no licence installed"), "{err}");
}

/// This one goes through the real reader, because the question is whether a properly signed
/// licence from the wrong key is accepted — and that is exactly what the reader decides.
#[test]
fn a_licence_signed_by_somebody_else_does_not_count() {
    let hub = HubStore::in_memory().unwrap();
    let (other_private, _) = licence::generate_key().unwrap();
    let l = licence::Licence {
        version: 1,
        id: "lic_forged".into(),
        customer: "Somebody Else".into(),
        seats: 9_999,
        valid_from: "2020-01-01T00:00:00Z".into(),
        valid_until: "2099-01-01T00:00:00Z".into(),
        issued_at: "2026-01-01T00:00:00Z".into(),
    };
    hub.set_licence(&licence::issue(&l, &other_private).unwrap().render())
        .unwrap();

    let state = LicenceState::read(&hub, jiff::Timestamp::now());
    assert!(matches!(state, LicenceState::Invalid(_)), "{state:?}");
    assert!(!state.may_collect(), "9999 seats signed by nobody we know");
}

/// An empty settings row is what an untouched installation looks like.
#[test]
fn a_hub_that_was_never_licensed_reads_as_missing() {
    let hub = HubStore::in_memory().unwrap();
    let state = LicenceState::read(&hub, jiff::Timestamp::now());
    assert_eq!(state, LicenceState::Missing);
    assert!(!state.may_collect());
    assert_eq!(state.seats(), None);
}

#[test]
fn seats_count_devices_that_can_still_send() {
    let hub = HubStore::in_memory().unwrap();
    let (a, _) = hub.add_device("one", "2026-09-07T00:00:00Z").unwrap();
    hub.add_device("two", "2026-09-07T00:00:00Z").unwrap();
    assert_eq!(hub.active_device_count().unwrap(), 2);

    // Revoking frees the seat — the rows stay, the person left.
    hub.revoke(&a.id, NOW).unwrap();
    assert_eq!(hub.active_device_count().unwrap(), 1);
    assert_eq!(hub.devices().unwrap().len(), 2, "both are still on record");
}

#[test]
fn a_warning_is_not_a_stop() {
    let warned = LicenceState::Valid {
        customer: "Test GmbH".into(),
        seats: 5,
        valid_until: "2026-09-17T00:00:00Z".into(),
        warning: Some(
            "ends in 10 day(s). After that the hub stops accepting rows; nothing is deleted."
                .into(),
        ),
    };
    assert!(warned.may_collect());
    let line = warned.line();
    assert!(line.contains("nothing is deleted"), "{line}");
}

// ---- what the hub can tell an administrator, and what it can hand an auditor (slice 6) ----

use super::report::{self, Concern};

const HUB_VERSION: &str = "0.2.1";

fn now_ts() -> jiff::Timestamp {
    NOW.parse().unwrap()
}

/// The case the refusal record exists for. A gap is refused, so it leaves no rows — without
/// remembering the refusal, the fleet view would show a device that merely went quiet, which
/// is a different problem with a different fix.
#[test]
fn a_refused_delivery_shows_up_as_a_concern() {
    let (mut hub, _, token) = hub_with_device();
    let client = Client::new();
    client.act("a");
    ingest(
        &mut hub,
        &collecting(),
        Some(&token),
        &client.all(),
        None,
        NOW,
    )
    .unwrap();

    client.act("b"); // never delivered
    client.act("c");
    assert!(
        ingest(
            &mut hub,
            &collecting(),
            Some(&token),
            &client.bundle_from(2),
            None,
            NOW
        )
        .is_err()
    );

    let rows = report::fleet(&hub, now_ts(), HUB_VERSION).unwrap();
    let concerns = &rows[0].concerns;
    assert!(
        concerns
            .iter()
            .any(|c| matches!(c, Concern::Refused { .. })),
        "{concerns:?}"
    );
    let line = concerns[0].line();
    assert!(line.contains("refused"), "{line}");
}

/// And a later good delivery clears it: a stale complaint is worse than none.
#[test]
fn a_successful_delivery_clears_the_concern() {
    let (mut hub, _, token) = hub_with_device();
    let client = Client::new();
    client.act("a");
    client.act("b");
    assert!(
        ingest(
            &mut hub,
            &collecting(),
            Some(&token),
            &client.bundle_from(1),
            None,
            NOW
        )
        .is_err()
    );
    ingest(
        &mut hub,
        &collecting(),
        Some(&token),
        &client.all(),
        None,
        NOW,
    )
    .unwrap();

    let rows = report::fleet(&hub, now_ts(), HUB_VERSION).unwrap();
    assert!(rows[0].concerns.is_empty(), "{:?}", rows[0].concerns);
}

#[test]
fn silence_becomes_a_concern_after_the_threshold() {
    let (mut hub, _, token) = hub_with_device();
    let client = Client::new();
    client.act("a");
    ingest(
        &mut hub,
        &collecting(),
        Some(&token),
        &client.all(),
        None,
        NOW,
    )
    .unwrap();

    let soon = now_ts() + std::time::Duration::from_secs(3600);
    assert!(
        report::fleet(&hub, soon, HUB_VERSION).unwrap()[0]
            .concerns
            .is_empty(),
        "an hour is not silence"
    );

    let later = now_ts() + std::time::Duration::from_secs(72 * 3600);
    let concerns = &report::fleet(&hub, later, HUB_VERSION).unwrap()[0].concerns;
    assert!(
        matches!(concerns.first(), Some(Concern::Quiet { hours }) if *hours >= 48),
        "{concerns:?}"
    );
}

#[test]
fn an_older_client_is_named_and_a_newer_or_odd_one_is_not() {
    let (mut hub, _, token) = hub_with_device();
    let client = Client::new();
    client.act("a");
    ingest(
        &mut hub,
        &collecting(),
        Some(&token),
        &client.all(),
        Some("0.1.0"),
        NOW,
    )
    .unwrap();
    let concerns = &report::fleet(&hub, now_ts(), HUB_VERSION).unwrap()[0].concerns;
    assert!(
        concerns.iter().any(|c| matches!(c, Concern::Behind { .. })),
        "{concerns:?}"
    );

    // A client ahead of the hub, and one with a version nobody can parse, are both left
    // alone: nagging about either would train people to ignore the column.
    for v in ["9.9.9", "my-build"] {
        let (mut hub, _, token) = hub_with_device();
        let client = Client::new();
        client.act("a");
        ingest(
            &mut hub,
            &collecting(),
            Some(&token),
            &client.all(),
            Some(v),
            NOW,
        )
        .unwrap();
        let concerns = &report::fleet(&hub, now_ts(), HUB_VERSION).unwrap()[0].concerns;
        assert!(
            !concerns.iter().any(|c| matches!(c, Concern::Behind { .. })),
            "{v} was called behind: {concerns:?}"
        );
    }
}

#[test]
fn a_revoked_device_is_not_a_problem_to_solve() {
    let (mut hub, device, token) = hub_with_device();
    let client = Client::new();
    client.act("a");
    ingest(
        &mut hub,
        &collecting(),
        Some(&token),
        &client.all(),
        None,
        NOW,
    )
    .unwrap();
    hub.revoke(&device.id, NOW).unwrap();

    let much_later = now_ts() + std::time::Duration::from_secs(500 * 3600);
    let rows = report::fleet(&hub, much_later, HUB_VERSION).unwrap();
    assert!(
        rows[0].concerns.is_empty(),
        "a decision somebody made is not an alert: {:?}",
        rows[0].concerns
    );
}

#[test]
fn devices_that_need_attention_come_first() {
    let hub = HubStore::in_memory().unwrap();
    let (_, quiet_token) = hub.add_device("zzz-quiet", "2026-09-07T00:00:00Z").unwrap();
    let (_, fine_token) = hub.add_device("aaa-fine", "2026-09-07T00:00:00Z").unwrap();
    let mut hub = hub;

    for token in [&quiet_token, &fine_token] {
        let c = Client::new();
        c.act("a");
        ingest(&mut hub, &collecting(), Some(token), &c.all(), None, NOW).unwrap();
    }
    // One of them then fails a delivery.
    let c = Client::new();
    c.act("x");
    c.act("y");
    let _ = ingest(
        &mut hub,
        &collecting(),
        Some(&quiet_token),
        &c.bundle_from(1),
        None,
        NOW,
    );

    let rows = report::fleet(&hub, now_ts(), HUB_VERSION).unwrap();
    assert_eq!(
        rows[0].device.name, "zzz-quiet",
        "trouble sorts above a name that would come first alphabetically"
    );
}

#[test]
fn verify_re_derives_the_chains_from_what_is_on_disk() {
    let (mut hub, _, token) = hub_with_device();
    let client = Client::new();
    client.act("a");
    client.act("b");
    ingest(
        &mut hub,
        &collecting(),
        Some(&token),
        &client.all(),
        None,
        NOW,
    )
    .unwrap();

    let r = report::verify(&hub).unwrap();
    assert!(r.ok);
    assert_eq!(r.rows, 2);
    assert_eq!(r.devices[0].chain.as_ref().unwrap(), &2);
}

#[test]
fn a_period_comes_out_as_a_bundle_that_verifies_on_its_own() {
    let (mut hub, device, token) = hub_with_device();
    let client = Client::new();
    client.act("a");
    client.act("b");
    ingest(
        &mut hub,
        &collecting(),
        Some(&token),
        &client.all(),
        None,
        NOW,
    )
    .unwrap();

    let (text, n) = report::device_bundle(&hub, &device.id, None, None, "test").unwrap();
    assert_eq!(n, 2);
    // The same check an outsider runs, over rows that made a round trip through the hub's
    // database. If storage lost or reordered anything, this is where it shows.
    let verdict = cyberbrain_policy::bundle::verify(&text).unwrap();
    assert_eq!(verdict.rows, 2);
}

#[test]
fn a_device_with_nothing_in_the_period_still_gets_a_file() {
    let (mut hub, _, token) = hub_with_device();
    let (silent, _) = hub.add_device("silent", "2026-09-07T00:00:00Z").unwrap();
    let client = Client::new();
    client.act("a");
    ingest(
        &mut hub,
        &collecting(),
        Some(&token),
        &client.all(),
        None,
        NOW,
    )
    .unwrap();

    let dir = tempfile::tempdir().unwrap();
    let out = report::write_report(&hub, dir.path(), None, None, "test").unwrap();
    let files = out["files"].as_array().unwrap();
    assert_eq!(files.len(), 2, "both devices, including the silent one");
    let quiet_file = dir.path().join(format!("{}.jsonl", silent.id));
    assert!(quiet_file.is_file());
    // "This machine did nothing that week" is a finding, and it verifies like any other.
    let text = std::fs::read_to_string(&quiet_file).unwrap();
    assert_eq!(cyberbrain_policy::bundle::verify(&text).unwrap().rows, 0);
    assert!(dir.path().join("summary.txt").is_file());
}

/// A record created by an older build must keep working when the program is upgraded.
#[test]
fn an_older_record_gains_the_new_columns() {
    let file = tempfile::NamedTempFile::new().unwrap();
    {
        // The devices table as an earlier version wrote it: no version, no refusal columns.
        let conn = rusqlite::Connection::open(file.path()).unwrap();
        conn.execute_batch(
            "CREATE TABLE devices (
                 id TEXT PRIMARY KEY, name TEXT NOT NULL, token_hash TEXT NOT NULL UNIQUE,
                 created_at TEXT NOT NULL, revoked_at TEXT, last_seen TEXT,
                 anchor TEXT NOT NULL, rows INTEGER NOT NULL DEFAULT 0
             );
             INSERT INTO devices (id, name, token_hash, created_at, anchor)
             VALUES ('dev_old', 'from-an-older-build', 'hash', '2026-01-01T00:00:00Z', 'genesis');",
        )
        .unwrap();
    }

    let hub = HubStore::open(file.path()).unwrap();
    let devices = hub.devices().unwrap();
    assert_eq!(
        devices.len(),
        1,
        "the row from the old build is still there"
    );
    assert_eq!(devices[0].name, "from-an-older-build");
    assert_eq!(devices[0].version, None);
    assert_eq!(devices[0].last_refusal, None);
}

// ---- the two-person rule (slice 7) ----

use super::access::{Denied, RequestState, Role};

fn people(hub: &HubStore) -> (String, String, String) {
    let (_, auditor) = hub.add_principal("M. Kraus", Role::Auditor, NOW).unwrap();
    let (_, council) = hub
        .add_principal("Works council", Role::Countersigner, NOW)
        .unwrap();
    let (_, admin) = hub.add_principal("A. Weber", Role::Admin, NOW).unwrap();
    (auditor, council, admin)
}

/// A hub with one device that has delivered, and the three roles.
fn hub_with_activity() -> (HubStore, String, String, String, String) {
    let (mut hub, device, token) = hub_with_device();
    let client = Client::new();
    client.act("a");
    client.act("b");
    ingest(
        &mut hub,
        &collecting(),
        Some(&token),
        &client.all(),
        None,
        NOW,
    )
    .unwrap();
    let (auditor, council, admin) = people(&hub);
    (hub, device.id, auditor, council, admin)
}

fn tmpdir() -> tempfile::TempDir {
    tempfile::tempdir().unwrap()
}

#[test]
fn activity_cannot_be_read_without_a_countersignature() {
    let (hub, device, auditor, _, _) = hub_with_activity();
    let who = hub.principal_for(Some(&auditor), Role::Auditor).unwrap();
    let req = hub
        .create_request(&who, Some(&device), None, None, "a reason", NOW)
        .unwrap();

    let dir = tmpdir();
    let err =
        report::disclose(&hub, Some(&auditor), &req.id, dir.path(), now_ts(), "test").unwrap_err();
    assert!(matches!(err, Denied::NotApproved(_)), "{err:?}");
    assert!(err.to_string().contains("somebody else approves"), "{err}");
    assert_eq!(std::fs::read_dir(dir.path()).unwrap().count(), 0);
}

#[test]
fn the_administrator_cannot_read_activity_at_all() {
    let (hub, _, _, _, admin) = hub_with_activity();
    // Not "no approved request" — the wrong role, which is a different sentence and a
    // different fix.
    let err = hub.principal_for(Some(&admin), Role::Auditor).unwrap_err();
    assert!(
        matches!(err, Denied::WrongRole { .. }),
        "an administrator asking for activity: {err:?}"
    );
}

#[test]
fn an_auditor_cannot_countersign() {
    let (hub, _, auditor, _, _) = hub_with_activity();
    let err = hub
        .principal_for(Some(&auditor), Role::Countersigner)
        .unwrap_err();
    assert!(matches!(err, Denied::WrongRole { .. }), "{err:?}");
}

/// Defence in depth: roles make this unreachable today, because one principal has one role
/// and only an auditor can create a request. If roles ever become plural, this is the check
/// that still holds — so it is tested at the level where it lives.
#[test]
fn a_request_cannot_be_approved_by_the_person_who_made_it() {
    let (hub, device, auditor, _, _) = hub_with_activity();
    let who = hub.principal_for(Some(&auditor), Role::Auditor).unwrap();
    let req = hub
        .create_request(&who, Some(&device), None, None, "a reason", NOW)
        .unwrap();

    let err = hub
        .approve_request(&req.id, &who, "2099-01-01T00:00:00Z", NOW)
        .unwrap_err();
    assert_eq!(err, Denied::SamePerson);
    assert!(err.to_string().contains("not an obstacle to work around"));
}

#[test]
fn an_approved_request_opens_a_window_that_closes_itself() {
    let (hub, device, auditor, council, _) = hub_with_activity();
    let who = hub.principal_for(Some(&auditor), Role::Auditor).unwrap();
    let signer = hub
        .principal_for(Some(&council), Role::Countersigner)
        .unwrap();
    let req = hub
        .create_request(&who, Some(&device), None, None, "a reason", NOW)
        .unwrap();
    assert_eq!(req.state(now_ts()), RequestState::Pending);

    let expires = (now_ts() + std::time::Duration::from_secs(3600)).to_string();
    let approved = hub
        .approve_request(&req.id, &signer, &expires, NOW)
        .unwrap();
    assert_eq!(approved.state(now_ts()), RequestState::Open);

    // Inside the window it works.
    let dir = tmpdir();
    let out =
        report::disclose(&hub, Some(&auditor), &req.id, dir.path(), now_ts(), "test").unwrap();
    assert_eq!(out["rows"].as_i64(), Some(2));

    // Two hours later it does not, and the message says to ask again rather than extend.
    let later = now_ts() + std::time::Duration::from_secs(2 * 3600);
    assert_eq!(approved.state(later), RequestState::Closed);
    let err =
        report::disclose(&hub, Some(&auditor), &req.id, dir.path(), later, "test").unwrap_err();
    assert!(matches!(err, Denied::WindowClosed(_)), "{err:?}");
    assert!(err.to_string().contains("Make a new request"), "{err}");
}

#[test]
fn another_auditor_cannot_collect_somebody_elses_approval() {
    let (hub, device, auditor, council, _) = hub_with_activity();
    let (_, second) = hub
        .add_principal("second auditor", Role::Auditor, NOW)
        .unwrap();

    let who = hub.principal_for(Some(&auditor), Role::Auditor).unwrap();
    let signer = hub
        .principal_for(Some(&council), Role::Countersigner)
        .unwrap();
    let req = hub
        .create_request(&who, Some(&device), None, None, "a reason", NOW)
        .unwrap();
    hub.approve_request(&req.id, &signer, "2099-01-01T00:00:00Z", NOW)
        .unwrap();

    let dir = tmpdir();
    let err =
        report::disclose(&hub, Some(&second), &req.id, dir.path(), now_ts(), "test").unwrap_err();
    assert!(err.to_string().contains("not by you"), "{err}");
}

/// The record the works council reads. Every step of the procedure is in it, in order, and
/// the chain says nothing was removed afterwards.
#[test]
fn every_step_is_in_the_hubs_own_chain() {
    let (hub, device, auditor, council, _) = hub_with_activity();
    let who = hub.principal_for(Some(&auditor), Role::Auditor).unwrap();
    let signer = hub
        .principal_for(Some(&council), Role::Countersigner)
        .unwrap();
    let req = hub
        .create_request(&who, Some(&device), None, None, "why we looked", NOW)
        .unwrap();
    hub.approve_request(&req.id, &signer, "2099-01-01T00:00:00Z", NOW)
        .unwrap();
    let dir = tmpdir();
    report::disclose(&hub, Some(&auditor), &req.id, dir.path(), now_ts(), "test").unwrap();

    let events = hub.hub_events(100).unwrap();
    let actions: Vec<&str> = events.iter().map(|e| e.action.as_str()).collect();
    assert_eq!(
        actions,
        [
            "role.granted",
            "role.granted",
            "role.granted",
            "access.requested",
            "access.approved",
            "access.disclosed",
        ]
    );
    // The reason is in the record, not only in somebody's memory of the conversation.
    let requested = &events[3];
    assert_eq!(requested.detail["reason"], "why we looked");
    assert_eq!(hub.verify_hub_chain().unwrap(), 6);
}

#[test]
fn the_hubs_own_chain_notices_an_edited_entry() {
    let (hub, _, _, _, _) = hub_with_activity();
    assert!(hub.verify_hub_chain().is_ok());

    // The triggers stop an UPDATE, so a tamperer would have to rebuild the table. This is
    // what the chain is for: the row count still adds up, and the arithmetic does not.
    hub.conn
        .execute_batch(
            "DROP TRIGGER hub_audit_no_update;
             UPDATE hub_audit SET action = 'role.revoked' WHERE seq = 1;",
        )
        .unwrap();
    let err = hub.verify_hub_chain().unwrap_err().to_string();
    assert!(err.contains("was edited"), "{err}");
}

#[test]
fn a_revoked_credential_stops_working_immediately() {
    let (hub, _, auditor, _, _) = hub_with_activity();
    let who = hub.principal_for(Some(&auditor), Role::Auditor).unwrap();
    assert!(hub.revoke_principal(&who.id, NOW).unwrap());
    let err = hub
        .principal_for(Some(&auditor), Role::Auditor)
        .unwrap_err();
    assert!(err.to_string().contains("revoked"), "{err}");
}

// ---- the licence a customer drops next to the record (0.3.0) ----
//
// The service route has to work without a command prompt, so licensing a hub is copying a
// file into the data directory. These are about what that does when the file is missing or
// wrong — the cases a support call is made of. The happy path needs the issuer's real
// signing key, which lives outside this repository, so it is covered by the licence tests
// against a generated key pair rather than here.

#[test]
fn the_licence_is_dropped_next_to_the_record() {
    let p = super::service::licence_drop_path(std::path::Path::new("/var/lib/cyberbrain"));
    assert_eq!(p, std::path::Path::new("/var/lib/cyberbrain/licence.txt"));
}

#[test]
fn the_names_people_actually_end_up_with_are_taken_too() {
    // Explorer hides known extensions, so saving the attachment as "licence.txt" produces
    // licence.txt.txt and shows it as licence.txt. There is no way for the person to see
    // what went wrong, so refusing it would be a support call about an invisible character.
    for name in [
        "licence.txt",
        "licence.txt.txt",
        "license.txt",
        "license.txt.txt",
    ] {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join(name), "x").unwrap();
        assert_eq!(
            super::service::find_licence_file(dir.path()),
            Some(dir.path().join(name)),
            "{name} was not found"
        );
    }
    // Calibration: it is not simply returning the first thing it sees.
    let dir = tempfile::tempdir().unwrap();
    std::fs::write(dir.path().join("HOW-TO-LICENCE.txt"), "x").unwrap();
    std::fs::write(dir.path().join("notes.txt"), "x").unwrap();
    assert_eq!(super::service::find_licence_file(dir.path()), None);
}

#[test]
fn the_documented_name_wins_over_the_typo() {
    let dir = tempfile::tempdir().unwrap();
    std::fs::write(dir.path().join("licence.txt.txt"), "the accident").unwrap();
    std::fs::write(dir.path().join("licence.txt"), "the one that was meant").unwrap();
    assert_eq!(
        super::service::find_licence_file(dir.path()),
        Some(dir.path().join("licence.txt"))
    );
}

#[test]
fn a_hub_with_no_licence_is_told_where_it_looked() {
    let said = super::service::where_it_looked(std::path::Path::new("C:\\ProgramData\\Cyberbrain"));
    // The log line that was missing said only "no licence installed", which leaves the
    // reader unable to tell whether the file was looked for, looked for somewhere else,
    // or found and rejected.
    assert!(said.contains("C:\\ProgramData\\Cyberbrain"), "{said}");
    assert!(said.contains("licence.txt"), "{said}");
    assert!(
        said.contains("restart"),
        "it has to say what to do next: {said}"
    );
}

#[test]
fn no_licence_file_is_not_worth_a_word() {
    let dir = tempfile::tempdir().unwrap();
    let hub = HubStore::in_memory().unwrap();
    // A hub licensed months ago has no file lying about, and a line every start would
    // teach whoever reads the log to skip it.
    assert_eq!(
        super::service::adopt_dropped_licence(&hub, dir.path()),
        None
    );
}

#[test]
fn an_unusable_licence_file_is_named_and_changes_nothing() {
    let dir = tempfile::tempdir().unwrap();
    let hub = HubStore::in_memory().unwrap();
    hub.set_licence("the one that is already installed")
        .unwrap();
    std::fs::write(dir.path().join("licence.txt"), "not a licence at all").unwrap();

    let note = super::service::adopt_dropped_licence(&hub, dir.path())
        .expect("somebody put that file there on purpose; silence would be the wrong answer");
    assert!(note.contains("not usable"), "{note}");
    assert!(
        note.contains("licence.txt"),
        "the message has to say which file: {note}"
    );
    // The point: a bad file must not take away the licence the hub is running on.
    assert_eq!(
        hub.licence_text().unwrap().as_deref(),
        Some("the one that is already installed")
    );
}

#[test]
fn the_same_licence_twice_is_not_news() {
    let dir = tempfile::tempdir().unwrap();
    let hub = HubStore::in_memory().unwrap();
    let text = "whatever is installed";
    hub.set_licence(text).unwrap();
    std::fs::write(dir.path().join("licence.txt"), text).unwrap();
    // Left in place after the first start, as people do.
    assert_eq!(
        super::service::adopt_dropped_licence(&hub, dir.path()),
        None
    );

    // Calibration: the same call does speak up when the file is not what is installed, so
    // the silence above comes from the comparison and not from the function being mute.
    std::fs::write(dir.path().join("licence.txt"), "something else entirely").unwrap();
    assert!(super::service::adopt_dropped_licence(&hub, dir.path()).is_some());
}

// ---- what a failed service registration says ----
//
// The first person to tick the installer's hub box got:
//
//     cannot register the service: IO error in winapi call
//
// which names nothing, suggests nothing, and cannot be looked up. The wrapper's Display
// says that; the operating system's message and number are one level down in `source()`.

#[test]
fn an_os_error_is_reported_with_its_number() {
    // 5 is ERROR_ACCESS_DENIED on Windows and EIO here; the number is the point, not which
    // number this machine happens to give it.
    let io = std::io::Error::from_raw_os_error(5);
    let said = super::service::describe_os_error(&io);
    assert!(
        said.contains("(Windows error 5)"),
        "the number a person can look up is missing: {said}"
    );
    assert!(
        said.len() > "(Windows error 5)".len(),
        "the number without the sentence is not much better: {said}"
    );
    assert!(
        !said.contains("winapi"),
        "this is the layer that told nobody anything: {said}"
    );
}

#[test]
fn an_error_without_a_number_still_says_something() {
    let io = std::io::Error::other("the pipe went away");
    let said = super::service::describe_os_error(&io);
    assert_eq!(said, "the pipe went away");
}
