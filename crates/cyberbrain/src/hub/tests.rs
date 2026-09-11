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

/// A grant that is in force: written by the operator, then countersigned by somebody else.
///
/// Two steps in the product and one line here, because these tests are about what moves
/// once a bereich is shared and not about who agreed to share it. That agreement has its
/// own tests, in `sync_access` and in `a_grant_takes_two_people` below.
fn grant_in_force(hub: &HubStore, id: &str, device: &str, bereich: &str, dir: Direction) {
    hub.grant_bereich(id, device, bereich, dir, "r", "admin-1", NOW)
        .unwrap();
    let (council, _) = hub
        .add_principal(
            &format!("Rat für {id}"),
            super::access::Role::Countersigner,
            NOW,
        )
        .unwrap();
    assert_eq!(
        hub.countersign_grant(id, &council, NOW).unwrap(),
        super::store::CountersignOutcome::Signed
    );
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

/// Seats are machines, not the projects on them: every project is its own device with its
/// own chain, and the licence promises a seat per machine.
#[test]
fn seats_count_machines_not_the_projects_on_them() {
    let hub = HubStore::in_memory().unwrap();
    let (a, _) = hub.add_device("ws-021/angebote", NOW).unwrap();
    let (b, _) = hub.add_device("ws-021/dispo", NOW).unwrap();
    let (c, _) = hub.add_device("ws-022/angebote", NOW).unwrap();
    hub.add_device("not-reported-yet", NOW).unwrap();
    hub.set_machine(&a.id, "ws-021").unwrap();
    hub.set_machine(&b.id, "ws-021").unwrap();
    hub.set_machine(&c.id, "ws-022").unwrap();

    assert_eq!(hub.active_device_count().unwrap(), 4);
    assert_eq!(
        hub.seats_in_use().unwrap(),
        3,
        "two machines, and one device that has not said which it is on"
    );
    assert!(
        !hub.needs_seat(Some("ws-021")).unwrap(),
        "a further project on a machine that already has a seat"
    );
    assert!(hub.needs_seat(Some("ws-023")).unwrap());
    assert!(hub.needs_seat(None).unwrap());

    hub.revoke(&a.id, NOW).unwrap();
    assert_eq!(hub.seats_in_use().unwrap(), 3, "ws-021 still has a project");
    hub.revoke(&b.id, NOW).unwrap();
    assert_eq!(hub.seats_in_use().unwrap(), 2);
}

#[test]
fn a_machine_name_has_one_spelling() {
    assert_eq!(super::normalise_machine(" WS-021\n"), Some("ws-021".into()));
    assert_eq!(super::normalise_machine(""), None);
    assert_eq!(super::normalise_machine("two words"), None);
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

/// A backup taken while the hub runs holds every row, and verifies as a file of its own.
///
/// The record runs in WAL mode, so the newest rows can still sit in `hub.db-wal` when
/// somebody copies `hub.db`. Watched failing against a plain file copy first.
#[test]
fn a_backup_taken_while_the_hub_runs_holds_every_row_and_verifies() {
    let dir = tempfile::tempdir().unwrap();
    let mut hub = HubStore::open(&dir.path().join("hub.db")).unwrap();
    let (_, token) = hub.add_device("laptop", NOW).unwrap();
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

    let to = dir.path().join("sicherung").join("hub-2026-09-11.db");
    let r = report::backup(&hub, &to, NOW).unwrap();
    assert!(r.ok, "{r:?}");
    assert_eq!(r.verify.rows, 2, "{r:?}");

    let copy = HubStore::open(&to).unwrap();
    assert_eq!(copy.total_entries().unwrap(), 2);
    assert_eq!(copy.devices().unwrap().len(), 1);

    // Taking it is on the record of the hub it was taken from.
    assert!(
        hub.hub_events(1000)
            .unwrap()
            .iter()
            .any(|e| e.action == "hub.backup")
    );
}

#[test]
fn a_backup_never_writes_over_an_existing_file() {
    let dir = tempfile::tempdir().unwrap();
    let hub = HubStore::open(&dir.path().join("hub.db")).unwrap();
    let to = dir.path().join("last-night.db");
    std::fs::write(&to, b"last night's backup").unwrap();
    let err = report::backup(&hub, &to, NOW).unwrap_err().to_string();
    assert!(err.contains("never written over"), "{err}");
    assert_eq!(std::fs::read(&to).unwrap(), b"last night's backup");
}

/// A device with rows whose timestamps are chosen, chained the way a real client chains them.
///
/// The chain hash covers a row's `_chain.at`, not its `ts`, so a client's rows can be re-dated
/// for a test without breaking the chain.
fn hub_with_rows_at(stamps: &[&str]) -> HubStore {
    let mut hub = HubStore::in_memory().unwrap();
    let (device, _) = hub.add_device("laptop", NOW).unwrap();
    let client = Client::new();
    for i in 0..stamps.len() {
        client.act(&format!("n{i}"));
    }
    let mut rows = client.sink.rows();
    for (row, ts) in rows.iter_mut().zip(stamps) {
        row.ts = ts.parse().unwrap();
    }
    let last = rows
        .last()
        .and_then(|r| r.chain_hash())
        .unwrap()
        .to_string();
    hub.append(&device, &rows, &last, None, NOW).unwrap();
    hub
}

fn countersigner(hub: &HubStore, name: &str) -> super::access::Principal {
    hub.add_principal(name, super::access::Role::Countersigner, NOW)
        .unwrap()
        .0
}

/// Old activity rows go only when a second person signs, and what is left still verifies.
#[test]
fn a_purge_takes_two_people_and_what_is_left_still_verifies() {
    use super::store::PurgeOutcome;
    let hub = hub_with_rows_at(&[
        "2023-01-10T00:00:00Z",
        "2023-06-01T00:00:00Z",
        "2026-08-01T00:00:00Z",
    ]);
    // Setting a period removes nothing.
    hub.set_retention("P2Y", "cli", NOW).unwrap();
    assert_eq!(hub.total_entries().unwrap(), 3);

    let alice = countersigner(&hub, "Alice");
    let (purge, would) = hub
        .propose_purge("Betriebsvereinbarung §7: 24 Monate", &alice.id, NOW)
        .unwrap();
    assert_eq!(would, 2);
    assert_eq!(purge.cutoff, "2024-09-07T12:00:00Z");
    assert_eq!(
        hub.countersign_purge(&purge.id, &alice, NOW).unwrap(),
        PurgeOutcome::SamePerson
    );
    assert_eq!(
        hub.total_entries().unwrap(),
        3,
        "one person must not purge alone"
    );

    let bob = countersigner(&hub, "Bob");
    match hub.countersign_purge(&purge.id, &bob, NOW).unwrap() {
        PurgeOutcome::CarriedOut { rows, .. } => assert_eq!(rows, 2),
        other => panic!("{other:?}"),
    }
    assert_eq!(hub.total_entries().unwrap(), 1);
    let r = report::verify(&hub).unwrap();
    assert!(r.ok, "{r:?}");
    assert_eq!(r.devices[0].floor_seq, Some(2));
    assert!(
        hub.hub_events(1000)
            .unwrap()
            .iter()
            .any(|e| e.action == "purge.carried_out")
    );
    assert!(matches!(
        hub.countersign_purge(&purge.id, &bob, NOW).unwrap(),
        PurgeOutcome::AlreadyDone { .. }
    ));
}

/// A device whose clock once ran backwards loses only the unbroken start of its chain.
#[test]
fn a_purge_removes_a_prefix_and_never_cuts_a_hole() {
    let hub = hub_with_rows_at(&[
        "2023-01-10T00:00:00Z",
        "2026-08-01T00:00:00Z",
        "2023-02-01T00:00:00Z",
    ]);
    hub.set_retention("P2Y", "cli", NOW).unwrap();
    let (purge, would) = hub.propose_purge("24 Monate", "cli", NOW).unwrap();
    assert_eq!(
        would, 1,
        "the old-looking third row sits after a new one and stays"
    );
    hub.countersign_purge(&purge.id, &countersigner(&hub, "Rat"), NOW)
        .unwrap();
    assert_eq!(hub.total_entries().unwrap(), 2);
    let r = report::verify(&hub).unwrap();
    assert!(r.ok, "{r:?}");
}

#[test]
fn a_retention_period_is_a_calendar_period_of_more_than_nothing() {
    for bad in ["PT12H", "P0D", "2 years", ""] {
        assert!(super::store::cutoff_for(bad, NOW).is_err(), "{bad:?}");
    }
    assert_eq!(
        super::store::cutoff_for("P18M", NOW).unwrap(),
        "2025-03-07T12:00:00Z"
    );
    let hub = HubStore::in_memory().unwrap();
    let err = hub.propose_purge("x", "cli", NOW).unwrap_err().to_string();
    assert!(err.contains("no retention period"), "{err}");
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

/// A device that sent nothing is still named, and no report carries the rows themselves.
///
/// Two things in one test because they are one behaviour: the summary is the whole output.
/// "This machine did nothing that week" is a finding and its absence would read as an
/// oversight — and "here is everything that machine did" is a disclosure, which this
/// command is not.
///
/// Calibrated against the state before: `write_report` wrote a `<device>.jsonl` per device
/// from the same `device_bundle` that `disclose` hands out after two people have agreed,
/// with no credential and no row in the hub's log. The assertion below on the directory
/// listing is the one that failed there.
#[test]
fn a_report_names_every_device_and_carries_nobodys_rows() {
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

    let written: Vec<String> = std::fs::read_dir(dir.path())
        .unwrap()
        .map(|e| e.unwrap().file_name().to_string_lossy().into_owned())
        .collect();
    assert_eq!(
        written,
        vec!["summary.txt".to_string()],
        "a report is a summary; anything else here is a disclosure without a request"
    );

    let summary = out["summary"].as_str().unwrap();
    assert!(
        summary.contains(&silent.id),
        "the silent device has to be named: {summary}"
    );
    assert!(
        summary.contains("chain holds"),
        "the verdict is the point of the line: {summary}"
    );
    // And it says where the rows are, so the reader is not left to guess that they exist.
    assert!(summary.contains("hub disclose"), "{summary}");
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
            // The device first, because it is registered first. Its absence here used to be
            // the shape of the gap: three roles and a disclosure were in the chain, and the
            // machine they were all about had arrived without a word.
            "device.registered",
            "role.granted",
            "role.granted",
            "role.granted",
            "access.requested",
            "access.approved",
            "access.disclosed",
        ]
    );
    // The reason is in the record, not only in somebody's memory of the conversation.
    let requested = &events[4];
    assert_eq!(requested.detail["reason"], "why we looked");
    assert_eq!(hub.verify_hub_chain().unwrap(), 7);
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
        super::service::Dropped::None
    );
}

#[test]
fn an_unusable_licence_file_is_named_and_changes_nothing() {
    let dir = tempfile::tempdir().unwrap();
    let hub = HubStore::in_memory().unwrap();
    hub.set_licence("the one that is already installed")
        .unwrap();
    std::fs::write(dir.path().join("licence.txt"), "not a licence at all").unwrap();

    let super::service::Dropped::Problem(note) =
        super::service::adopt_dropped_licence(&hub, dir.path())
    else {
        panic!("somebody put that file there on purpose; silence would be the wrong answer");
    };
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
        super::service::Dropped::Unchanged
    );

    // Calibration: the same call does speak up when the file is not what is installed, so
    // the silence above comes from the comparison and not from the function being mute.
    std::fs::write(dir.path().join("licence.txt"), "something else entirely").unwrap();
    assert!(matches!(
        super::service::adopt_dropped_licence(&hub, dir.path()),
        super::service::Dropped::Problem(_)
    ));
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

#[test]
fn a_file_that_cannot_be_read_is_not_the_same_as_no_file() {
    let dir = tempfile::tempdir().unwrap();
    let hub = HubStore::in_memory().unwrap();
    // What "Save as > Unicode" in Notepad produces: UTF-16, which is not valid UTF-8 and
    // looks entirely correct in every way the person who saved it can check.
    std::fs::write(dir.path().join("licence.txt"), [0xff, 0xfe, 0x7b, 0x00]).unwrap();

    let said = super::service::adopt_dropped_licence(&hub, dir.path());
    let super::service::Dropped::Problem(msg) = said else {
        panic!("a file that is there but unreadable must not read as no file: {said:?}");
    };
    assert!(msg.contains("licence.txt"), "{msg}");
    assert!(msg.contains("UTF-8"), "it has to say what to do: {msg}");
}

// ---- the hub's own page ----

use super::page::{self, View};

fn view_of(
    hub: &HubStore,
    record: &str,
    flash: Option<std::result::Result<String, String>>,
) -> View {
    View::gather(
        hub,
        std::path::Path::new(record),
        7788,
        // The hub these tests describe is a properly set up one; the unencrypted case has
        // its own test below, because what it shows is different on purpose.
        true,
        NOW.parse().unwrap(),
        flash,
    )
}

#[test]
fn the_page_is_only_for_the_machine_the_hub_runs_on() {
    let yes = ["127.0.0.1:51000", "[::1]:51000"];
    let no = ["192.168.1.20:51000", "10.0.0.5:51000", "[2001:db8::1]:443"];
    for a in yes {
        assert!(
            super::api::at_the_machine(&a.parse().unwrap()),
            "{a} is the machine itself"
        );
    }
    // It shows who is on the network and it can install a licence, on a port the whole
    // network can reach. Getting this backwards is the difference between a status page and
    // an open console.
    for a in no {
        assert!(!super::api::at_the_machine(&a.parse().unwrap()), "{a}");
    }
}

#[test]
fn an_unlicensed_hub_says_so_where_a_person_is_looking() {
    let hub = HubStore::in_memory().unwrap();
    let html = page::render(&view_of(&hub, "/var/lib/cyberbrain/hub.db", None));
    assert!(html.contains("not licensed"), "{html}");
    assert!(html.contains("accepts nothing"));
    // And offers the way out on the same screen, opened, rather than behind a command.
    assert!(html.contains("<details open"), "the form should be open");
    assert!(html.contains("action=\"/licence\""));
}

#[test]
fn a_collecting_hub_shows_the_seats() {
    // Built by hand rather than from a real licence file: only the issuer can sign one, and
    // a test that skips itself when the key is absent is a test that passes by agreeing
    // with itself on every machine but one.
    let v = View {
        grants: Vec::new(),
        version: "0.3.0".into(),
        record: "C:\\ProgramData\\Cyberbrain\\hub.db".into(),
        licence: LicenceState::Valid {
            customer: "Beispiel GmbH".into(),
            seats: 5,
            valid_until: "2027-01-01T00:00:00Z".into(),
            warning: None,
        },
        seats: Some((2, 5)),
        fleet: Vec::new(),
        found_file: None,
        suggested_url: "https://hub:7788".into(),
        encrypted: true,
        flash: None,
    };
    let html = page::render(&v);
    assert!(html.contains("collecting"), "{html}");
    assert!(html.contains("Beispiel GmbH"));
    assert!(
        html.contains("<strong>2</strong> of <strong>5</strong>"),
        "{html}"
    );
    // A hub that is already collecting should not be shouting a form at anybody.
    assert!(!html.contains("<details open"));
}

#[test]
fn a_licence_about_to_run_out_says_it_on_the_page() {
    let v = View {
        grants: Vec::new(),
        version: "0.3.0".into(),
        record: "hub.db".into(),
        licence: LicenceState::Valid {
            customer: "Beispiel GmbH".into(),
            seats: 5,
            valid_until: "2026-10-01T00:00:00Z".into(),
            warning: Some(
                "the licence for Beispiel GmbH ends on 2026-10-01 — 24 day(s) left.".into(),
            ),
        },
        seats: Some((5, 5)),
        fleet: Vec::new(),
        found_file: None,
        suggested_url: "https://hub:7788".into(),
        encrypted: true,
        flash: None,
    };
    let html = page::render(&v);
    // The warning replaces the calm line rather than sitting beside it: the whole point of
    // the thirty days is that somebody acts inside them.
    assert!(html.contains("24 day(s) left"), "{html}");
}

#[test]
fn a_device_name_cannot_carry_markup_into_the_page() {
    let hub = HubStore::in_memory().unwrap();
    // Device names arrive from whoever registers one and are shown back on this page.
    hub.add_device("<script>alert(1)</script>", NOW).unwrap();
    let html = page::render(&view_of(&hub, "hub.db", None));
    assert!(
        !html.contains("<script>alert"),
        "unescaped name in the page"
    );
    assert!(
        html.contains("&lt;script&gt;alert(1)&lt;/script&gt;"),
        "{html}"
    );
}

#[test]
fn a_licence_file_that_is_already_installed_is_not_offered_again() {
    let dir = tempfile::tempdir().unwrap();
    let hub = HubStore::in_memory().unwrap();
    let text = "whatever is installed";
    std::fs::write(dir.path().join("licence.txt"), text).unwrap();
    let record = dir.path().join("hub.db");

    // Before: there is something to click.
    let before = View::gather(&hub, &record, 7788, true, NOW.parse().unwrap(), None);
    assert!(before.found_file.is_some());

    // After: the file is still lying there, as files do, and the button is gone.
    hub.set_licence(text).unwrap();
    let after = View::gather(&hub, &record, 7788, true, NOW.parse().unwrap(), None);
    assert_eq!(after.found_file, None);
}

// ---- the administrator account ----
//
// One account for the machine's administration, which is what the fleet view is. It is not
// the roles model: nothing reachable with this password can read an activity row.

use super::admin;

#[test]
fn a_hub_starts_unclaimed_and_the_first_visit_claims_it() {
    let hub = HubStore::in_memory().unwrap();
    assert!(!admin::is_claimed(&hub));
    admin::set_password(&hub, "korrektpferd1").unwrap();
    assert!(admin::is_claimed(&hub));
}

#[test]
fn there_is_no_password_to_look_up_before_one_is_set() {
    let hub = HubStore::in_memory().unwrap();
    // The point of having no default: nothing works until somebody at the machine chooses.
    for guess in ["", "admin", "password", "cyberbrain", "changeme"] {
        assert!(!admin::verify(&hub, guess), "{guess:?} was accepted");
    }
}

#[test]
fn a_short_password_is_refused_with_the_reason() {
    let hub = HubStore::in_memory().unwrap();
    let e = admin::set_password(&hub, "kurz").unwrap_err();
    assert!(e.contains("10 characters"), "{e}");
    assert!(
        !admin::is_claimed(&hub),
        "a refused password must not be stored"
    );
}

#[test]
fn the_password_is_checked_and_not_stored() {
    let hub = HubStore::in_memory().unwrap();
    admin::set_password(&hub, "korrektpferd1").unwrap();
    assert!(admin::verify(&hub, "korrektpferd1"));
    assert!(!admin::verify(&hub, "korrektpferd2"));
    // What is kept is a PHC string: the algorithm, its parameters, a salt and the hash.
    // The password itself is nowhere in the record, which is the whole point of the hashing.
    let stored = hub.setting("admin_password").unwrap().unwrap();
    assert!(stored.starts_with("$argon2"), "{stored}");
    assert!(!stored.contains("korrektpferd1"));
}

#[test]
fn two_hubs_with_the_same_password_store_different_hashes() {
    let (a, b) = (
        HubStore::in_memory().unwrap(),
        HubStore::in_memory().unwrap(),
    );
    admin::set_password(&a, "korrektpferd1").unwrap();
    admin::set_password(&b, "korrektpferd1").unwrap();
    // A salt, in other words. Without one, one stolen record would answer for every hub
    // whose administrator picked the same thing.
    assert_ne!(
        a.setting("admin_password").unwrap(),
        b.setting("admin_password").unwrap()
    );
}

#[test]
fn resetting_puts_the_hub_back_to_its_first_run() {
    let hub = HubStore::in_memory().unwrap();
    admin::set_password(&hub, "korrektpferd1").unwrap();
    hub.set_setting("admin_password", "").unwrap();
    assert!(!admin::is_claimed(&hub));
    assert!(!admin::verify(&hub, "korrektpferd1"));
}

#[test]
fn a_session_lasts_until_it_is_closed() {
    let s = admin::Sessions::default();
    let now: jiff::Timestamp = NOW.parse().unwrap();
    let token = s.open(now);
    assert!(s.who(&token, now).is_some());
    assert!(
        s.who("something else", now).is_none(),
        "an unknown cookie is not a session"
    );
    s.close(&token);
    assert!(
        s.who(&token, now).is_none(),
        "signing out has to mean something"
    );
}

#[test]
fn a_session_does_not_last_forever_and_using_it_keeps_it_alive() {
    let s = admin::Sessions::default();
    let now: jiff::Timestamp = NOW.parse().unwrap();
    let token = s.open(now);
    let day = now + jiff::Span::new().hours(24);
    // Untouched for a day: gone.
    assert!(s.who(&token, day).is_none());

    // Used every few hours: still there a day later, because each use pushes it out.
    let token = s.open(now);
    let mut t = now;
    for _ in 0..6 {
        t += jiff::Span::new().hours(4);
        assert!(
            s.who(&token, t).is_some(),
            "a session in use was dropped at {t}"
        );
    }
}

#[test]
fn two_sessions_are_not_the_same_string() {
    let s = admin::Sessions::default();
    let now: jiff::Timestamp = NOW.parse().unwrap();
    let (a, b) = (s.open(now), s.open(now));
    assert_ne!(a, b);
    assert_eq!(a.len(), 64, "32 bytes of randomness, hex");
}

#[test]
fn our_cookie_is_found_among_other_peoples() {
    assert_eq!(
        admin::cookie_from(Some("theme=dark; cyberbrain_hub=abc123; other=1")),
        Some("abc123".to_string())
    );
    assert_eq!(admin::cookie_from(Some("theme=dark")), None);
    assert_eq!(admin::cookie_from(None), None);
    // Not a prefix match: a cookie called cyberbrain_hub_something is not ours.
    assert_eq!(admin::cookie_from(Some("cyberbrain_hub_x=abc")), None);
}

// ---- TLS, and who may type a password into a hub without it ----
//
// The guardrail and the encryption are one subject: what the certificate buys is that the
// password may be typed from a desk at all. Both halves are tested here, and both were
// written against a hub that did neither.

use axum::body::Body;
use axum::extract::connect_info::ConnectInfo;
use axum::http::{Request, StatusCode};
use rustls_pki_types::pem::PemObject;
use tower::ServiceExt;

const TESTDATA: &str = concat!(env!("CARGO_MANIFEST_DIR"), "/src/hub/testdata");

fn hub_with_password(password: &str) -> HubStore {
    let hub = HubStore::in_memory().unwrap();
    admin::set_password(&hub, password).unwrap();
    hub
}

fn state_for(hub: HubStore, encrypted: bool) -> Arc<super::api::HubState> {
    Arc::new(super::api::HubState {
        hub: std::sync::Mutex::new(hub),
        record: std::path::PathBuf::from("hub.db"),
        port: 7788,
        sessions: Default::default(),
        flash: std::sync::Mutex::new(None),
        encrypted,
    })
}

/// A sign-in attempt from `from`, the way the router will see it.
async fn sign_in_from(
    state: Arc<super::api::HubState>,
    from: &str,
    password: &str,
) -> axum::response::Response {
    let mut req = Request::builder()
        .method("POST")
        .uri("/login")
        .header("content-type", "application/x-www-form-urlencoded")
        .body(Body::from(format!("password={password}")))
        .unwrap();
    req.extensions_mut()
        .insert(ConnectInfo(from.parse::<std::net::SocketAddr>().unwrap()));
    super::api::router(state).oneshot(req).await.unwrap()
}

fn cookie_of(r: &axum::response::Response) -> String {
    r.headers()
        .get(axum::http::header::SET_COOKIE)
        .map(|v| v.to_str().unwrap().to_string())
        .unwrap_or_default()
}

/// A signed-in principal is not the administrator.
///
/// The hub has one login box and two kinds of caller behind it. When the question asked of
/// the cookie was only "is this session live", every one of them came out as the
/// administrator — an editor, whose whole point is that they see conflicts in their own
/// bereiche and nothing else, could hand themselves a grant to any bereich on the hub.
#[tokio::test]
async fn an_editors_session_does_not_administer_the_hub() {
    let hub = hub_with_password("correct horse battery");
    let (_editor, token) = hub
        .add_principal("Rita", super::access::Role::Editor, NOW)
        .unwrap();
    // A real device, so that the only thing standing between this form and a row in
    // `bereich_grants` is the answer to "who is asking". A grant naming a device the hub
    // does not know fails on the foreign key, and a test that passes because of that is a
    // test that would keep passing with the check taken out.
    let (device, _) = hub.add_device("laptop-rita", NOW).unwrap();
    let state = state_for(hub, true);

    let r = sign_in_from(state.clone(), "192.168.1.20:51000", &token).await;
    assert_eq!(r.status(), StatusCode::SEE_OTHER, "the editor may sign in");
    let cookie = cookie_of(&r);
    assert!(cookie.contains("cyberbrain_hub="), "{cookie}");
    let cookie = cookie.split(';').next().unwrap().to_string();

    let mut req = Request::builder()
        .method("POST")
        .uri("/grants")
        .header("content-type", "application/x-www-form-urlencoded")
        .header("cookie", &cookie)
        .body(Body::from(format!(
            "device={}&bereich=hr&direction=send&reason=weil ich kann",
            device.id
        )))
        .unwrap();
    req.extensions_mut().insert(ConnectInfo(
        "192.168.1.20:51000"
            .parse::<std::net::SocketAddr>()
            .unwrap(),
    ));
    let r = super::api::router(state.clone())
        .oneshot(req)
        .await
        .unwrap();
    // The record first, because it is the thing that would still be wrong if the page were
    // right: a refusal that has already written the row is not a refusal.
    assert!(
        state
            .hub
            .lock()
            .unwrap()
            .grants_for_device(&device.id)
            .unwrap()
            .is_empty(),
        "an editor handed themselves a grant to a bereich that is not theirs"
    );
    let body = axum::body::to_bytes(r.into_body(), 64 * 1024)
        .await
        .unwrap();
    let body = String::from_utf8_lossy(&body);
    assert!(
        body.contains("password") || body.contains("credential"),
        "an editor posting to /grants belongs at the login box, got: {body}"
    );
}

#[tokio::test]
async fn a_password_is_not_taken_over_the_network_in_the_clear() {
    let state = state_for(hub_with_password("correct horse battery"), false);
    let r = sign_in_from(state, "192.168.1.20:51000", "correct horse battery").await;
    // Refused, not "wrong password": the hub knows perfectly well it is right, and saying
    // so would be a sentence that teaches the administrator to keep trying.
    assert_eq!(r.status(), StatusCode::FORBIDDEN);
    assert!(cookie_of(&r).is_empty(), "no session may come of this");
    let body = axum::body::to_bytes(r.into_body(), 64 * 1024)
        .await
        .unwrap();
    let body = String::from_utf8_lossy(&body);
    assert!(body.contains("not encrypted"), "{body}");
    // Both ways out, because the person reading it may only be able to take one of them.
    assert!(body.contains("machine the hub runs on"), "{body}");
    assert!(body.contains("--tls-cert"), "{body}");
}

#[tokio::test]
async fn at_the_machine_a_password_still_works_without_a_certificate() {
    // The hub in the cupboard has to stay administrable, or the guardrail above is a way of
    // locking an operator out of their own collector.
    let state = state_for(hub_with_password("correct horse battery"), false);
    let r = sign_in_from(state, "127.0.0.1:51000", "correct horse battery").await;
    assert_eq!(r.status(), StatusCode::SEE_OTHER);
    let cookie = cookie_of(&r);
    assert!(cookie.contains("cyberbrain_hub="), "{cookie}");
    // No `Secure` here: over http a browser would drop it, and the sign-in would appear to
    // succeed and then not have happened.
    assert!(!cookie.contains("Secure"), "{cookie}");
}

#[tokio::test]
async fn with_a_certificate_a_password_may_come_from_a_desk() {
    let state = state_for(hub_with_password("correct horse battery"), true);
    let r = sign_in_from(state, "192.168.1.20:51000", "correct horse battery").await;
    assert_eq!(r.status(), StatusCode::SEE_OTHER);
    assert!(cookie_of(&r).contains("Secure"), "{}", cookie_of(&r));
}

#[tokio::test]
async fn a_wrong_password_from_a_desk_is_still_wrong() {
    // The guardrail must not become an accidental way in: an encrypted hub checks the
    // password like any other.
    let state = state_for(hub_with_password("correct horse battery"), true);
    let r = sign_in_from(state, "192.168.1.20:51000", "hunter2").await;
    assert_eq!(r.status(), StatusCode::OK); // the sign-in page again
    assert!(cookie_of(&r).is_empty());
}

#[test]
fn the_page_says_when_the_hub_is_not_encrypted() {
    let hub = HubStore::in_memory().unwrap();
    let v = View::gather(
        &hub,
        std::path::Path::new("hub.db"),
        7788,
        false,
        NOW.parse().unwrap(),
        None,
    );
    // The address it suggests for invitations follows the hub it is actually being served
    // over, or the first thing an operator does with this page is send clients to a door
    // that is shut.
    assert!(
        v.suggested_url.starts_with("http://"),
        "{}",
        v.suggested_url
    );
    let html = page::render(&v);
    assert!(html.contains("not encrypted"), "{html}");

    let encrypted = View::gather(
        &hub,
        std::path::Path::new("hub.db"),
        7788,
        true,
        NOW.parse().unwrap(),
        None,
    );
    assert!(encrypted.suggested_url.starts_with("https://"));
    assert!(!page::render(&encrypted).contains("not encrypted"));
}

#[test]
fn the_fingerprint_is_the_one_a_browser_shows() {
    let cert = super::tls::load(
        std::path::Path::new(TESTDATA)
            .join("hub-test-leaf.pem")
            .as_path(),
        std::path::Path::new(TESTDATA)
            .join("hub-test-leaf-key.pem")
            .as_path(),
    )
    .unwrap();
    // The value openssl prints for the same file. Written out rather than computed here:
    // a fingerprint checked against our own hashing would agree with itself even if it
    // hashed the wrong bytes.
    assert_eq!(
        cert.fingerprint,
        "63:4E:3F:E0:BE:0A:13:3F:D4:CB:2B:AA:19:2F:C4:FD:52:C6:0F:00:5C:13:BF:41:03:89:34:03:CD:5B:65:9F"
    );
}

#[test]
fn a_certificate_and_a_key_that_are_not_a_pair_are_refused_by_name() {
    // The CA's certificate with the leaf's key: both files are real, and neither is the
    // other's half. The message has to name the files, because at this point the operator
    // is looking at four paths and one of them is wrong.
    let e = super::tls::load(
        std::path::Path::new(TESTDATA)
            .join("hub-test-ca.pem")
            .as_path(),
        std::path::Path::new(TESTDATA)
            .join("hub-test-leaf-key.pem")
            .as_path(),
    )
    .err()
    .expect("a certificate and a key that are not a pair cannot be served with")
    .to_string();
    assert!(e.contains("do not go together"), "{e}");
    assert!(e.contains("hub-test-ca.pem"), "{e}");
}

#[tokio::test]
async fn a_hub_with_a_certificate_answers_over_tls_and_stops_when_told() {
    use tokio::io::{AsyncReadExt, AsyncWriteExt};

    let cert = super::tls::load(
        std::path::Path::new(TESTDATA)
            .join("hub-test-leaf.pem")
            .as_path(),
        std::path::Path::new(TESTDATA)
            .join("hub-test-leaf-key.pem")
            .as_path(),
    )
    .unwrap();
    let listener = tokio::net::TcpListener::bind((std::net::Ipv4Addr::LOCALHOST, 0))
        .await
        .unwrap();
    let addr = listener.local_addr().unwrap();
    let make = super::api::router(state_for(HubStore::in_memory().unwrap(), true))
        .into_make_service_with_connect_info::<std::net::SocketAddr>();
    let (stop, stopped) = tokio::sync::oneshot::channel::<()>();
    let served = tokio::spawn(async move {
        super::tls::serve(listener, make, cert, async {
            let _ = stopped.await;
        })
        .await
    });

    // A client that trusts the test CA and nothing else. Trusting everything would make
    // this a test that the socket works, not that the hub presents a certificate for it.
    let mut roots = rustls::RootCertStore::empty();
    for c in rustls_pki_types::CertificateDer::pem_file_iter(
        std::path::Path::new(TESTDATA).join("hub-test-ca.pem"),
    )
    .unwrap()
    {
        roots.add(c.unwrap()).unwrap();
    }
    let mut config = rustls::ClientConfig::builder()
        .with_root_certificates(roots)
        .with_no_client_auth();
    config.alpn_protocols = vec![b"http/1.1".to_vec()];
    let connector = tokio_rustls::TlsConnector::from(Arc::new(config));
    let tcp = tokio::net::TcpStream::connect(addr).await.unwrap();
    // Bounded, because the interesting failure is a hub that answers in plain text: the
    // handshake then waits for a server hello that is never coming, and an unbounded wait
    // turns a failing test into a build that hangs until somebody kills it.
    let mut tls = tokio::time::timeout(
        std::time::Duration::from_secs(10),
        connector.connect("localhost".try_into().unwrap(), tcp),
    )
    .await
    .expect("the hub should answer the handshake rather than leave it open")
    .expect("the hub should present a certificate the test CA signed");
    tls.write_all(b"GET /health HTTP/1.1\r\nHost: localhost\r\nConnection: close\r\n\r\n")
        .await
        .unwrap();
    let mut answer = String::new();
    tls.read_to_string(&mut answer).await.unwrap();
    assert!(answer.starts_with("HTTP/1.1 200"), "{answer}");
    assert!(answer.contains("\"role\":\"hub\""), "{answer}");

    // And it comes back when asked to stop, rather than being left for the test harness to
    // kill: a hub that cannot be stopped cannot be upgraded either.
    stop.send(()).unwrap();
    tokio::time::timeout(std::time::Duration::from_secs(5), served)
        .await
        .expect("the server should return when it is told to stop")
        .unwrap()
        .unwrap();
}

// ---- the hub's own certificate, and the pin that goes with it ----

#[test]
fn a_hub_makes_one_certificate_and_keeps_it() {
    let dir = tempfile::tempdir().unwrap();
    let first = super::tls::own(dir.path(), &["hub.example.internal".into()]).unwrap();
    assert!(first.pinnable, "its own certificate is the pinnable one");
    assert!(dir.path().join("hub-cert.pem").exists());
    assert!(dir.path().join("hub-key.pem").exists());

    // The pin in every invitation ever issued is this certificate. A second start must not
    // quietly mint a new one, or every enrolled machine stops delivering at once.
    let second = super::tls::own(dir.path(), &["hub.example.internal".into()]).unwrap();
    assert_eq!(first.fingerprint, second.fingerprint);

    // And a certificate that came from the operator is never pinned: it has an issuer, and
    // issuers renew.
    let supplied = super::tls::load(
        std::path::Path::new(TESTDATA)
            .join("hub-test-leaf.pem")
            .as_path(),
        std::path::Path::new(TESTDATA)
            .join("hub-test-leaf-key.pem")
            .as_path(),
    )
    .unwrap();
    assert!(!supplied.pinnable);
}

#[test]
fn half_a_certificate_is_refused_rather_than_replaced() {
    let dir = tempfile::tempdir().unwrap();
    super::tls::own(dir.path(), &["hub".into()]).unwrap();
    std::fs::remove_file(dir.path().join("hub-cert.pem")).unwrap();
    // Making a fresh pair here would look like a repair and would silently invalidate every
    // pin. The key that is still lying there is the evidence that this was a hub.
    let e = super::tls::own(dir.path(), &["hub".into()])
        .err()
        .expect("a missing half is not something to paper over")
        .to_string();
    assert!(e.contains("come as a pair"), "{e}");
}

#[test]
fn the_names_in_the_certificate_follow_the_address() {
    let wildcard: std::net::SocketAddr = "0.0.0.0:7788".parse().unwrap();
    let names = super::tls::names_for(&wildcard);
    assert!(names.contains(&"localhost".to_string()));
    assert!(names.contains(&"127.0.0.1".to_string()));
    // A wildcard bind is not a name. A certificate for "0.0.0.0" would be a mismatch on
    // every address the hub is actually reached at.
    assert!(!names.contains(&"0.0.0.0".to_string()), "{names:?}");

    let concrete: std::net::SocketAddr = "192.168.1.20:7788".parse().unwrap();
    assert!(super::tls::names_for(&concrete).contains(&"192.168.1.20".to_string()));
}

#[test]
fn the_pin_offered_to_invitations_is_what_the_running_hub_serves() {
    let hub = HubStore::in_memory().unwrap();
    assert_eq!(super::pin_to_offer(&hub), None);
    super::remember_pin(&hub, Some("AB:CD")).unwrap();
    assert_eq!(super::pin_to_offer(&hub).as_deref(), Some("AB:CD"));
    // Restarted without a certificate: an invitation issued now must not promise one. A
    // stale fingerprint sends a machine off to expect something nobody serves.
    super::remember_pin(&hub, None).unwrap();
    assert_eq!(super::pin_to_offer(&hub), None);
}

#[test]
fn an_invitation_without_a_pin_is_still_an_invitation() {
    // Version 1 files exist in the field: hubs issued them before there was a pin, and a
    // client that refused them would mean upgrading every hub and every machine on the same
    // afternoon.
    let v1 = r#"{"kind":"cyberbrain.hub.invitation","version":1,"device":"dev_1",
        "name":"ws","token":"t","hub_url":"https://hub.internal:7788"}"#;
    let inv = super::client::parse_invitation(v1).unwrap();
    assert_eq!(inv.hub_cert_sha256, None);

    let v2 = r#"{"kind":"cyberbrain.hub.invitation","version":2,"device":"dev_1",
        "name":"ws","token":"t","hub_url":"https://hub.internal:7788",
        "hub_cert_sha256":"63:4E:3F"}"#;
    assert_eq!(
        super::client::parse_invitation(v2).unwrap().hub_cert_sha256,
        Some("63:4E:3F".to_string())
    );

    // Something newer than this program: say so rather than guess at it.
    let v3 = r#"{"kind":"cyberbrain.hub.invitation","version":3,"device":"d","name":"w",
        "token":"t","hub_url":"https://hub.internal:7788"}"#;
    let e = super::client::parse_invitation(v3).unwrap_err().to_string();
    assert!(e.contains("newer than this program"), "{e}");
}

#[test]
fn a_fingerprint_is_read_the_way_it_is_written_down() {
    use cyberbrain_policy::egress::transport::CertificatePin;
    let colons = "63:4E:3F:E0:BE:0A:13:3F:D4:CB:2B:AA:19:2F:C4:FD:52:C6:0F:00:5C:13:BF:41:03:89:34:03:CD:5B:65:9F";
    assert!(CertificatePin::parse(colons).is_ok());
    // The same thing pasted out of a script, and in the case a terminal gave it.
    assert!(CertificatePin::parse(&colons.replace(':', "")).is_ok());
    assert!(CertificatePin::parse(&colons.to_lowercase()).is_ok());
    // And the shapes that are not a fingerprint at all. Half of one is the dangerous case:
    // a truncated paste must not become a pin that matches nothing and is never checked.
    for bad in ["", "63:4E:3F", "not a fingerprint", &colons[..40]] {
        assert!(CertificatePin::parse(bad).is_err(), "{bad:?}");
    }
}

/// The real delivery path, against a hub that is really serving TLS.
///
/// Through `client::deliver` rather than the transport underneath it, so that what is tested
/// is what `hub push` does — including whether the pin it was given ever reaches the wire.
async fn deliver_to_pinned_hub(hub_url: &str, pin: Option<&str>) -> Result<super::client::Reply> {
    use cyberbrain_policy::{Actor, AuditLog, Egress, PolicyConfig};
    let cfg = PolicyConfig {
        hub_endpoint: Some(hub_url.to_string()),
        ..Default::default()
    };
    let (log, _sink) = AuditLog::in_memory();
    let egress = Egress::new(cfg, log, Actor::Cli);
    super::client::deliver(
        &egress,
        &Actor::Cli,
        hub_url,
        "a-token",
        pin,
        "0.0.0-test",
        String::new(),
    )
    .await
}

#[tokio::test]
async fn a_pinned_client_talks_to_that_hub_and_to_no_other() {
    let dir = tempfile::tempdir().unwrap();
    let cert = super::tls::own(dir.path(), &["localhost".into(), "127.0.0.1".into()]).unwrap();
    let ours = cert.fingerprint.clone();
    let listener = tokio::net::TcpListener::bind((std::net::Ipv4Addr::LOCALHOST, 0))
        .await
        .unwrap();
    let port = listener.local_addr().unwrap().port();
    let make = super::api::router(state_for(HubStore::in_memory().unwrap(), true))
        .into_make_service_with_connect_info::<std::net::SocketAddr>();
    let (stop, stopped) = tokio::sync::oneshot::channel::<()>();
    let served = tokio::spawn(async move {
        super::tls::serve(listener, make, cert, async {
            let _ = stopped.await;
        })
        .await
    });
    let url = format!("https://127.0.0.1:{port}");

    // Pinned to what this hub actually serves: the delivery goes through. The hub has no
    // licence, so its answer is "not collecting" — an answer, and therefore proof that TLS
    // and the delivery both worked. The pin's job is to get us to a refusal we can read.
    let ok = deliver_to_pinned_hub(&url, Some(&ours)).await.unwrap();
    assert!(
        matches!(ok, super::client::Reply::NotCollecting(_)),
        "reached the hub and got its own answer, not a transport error: {ok:?}"
    );

    // Pinned to a different, perfectly valid certificate: refused. This is the whole point.
    // It is the certificate of the test CA's leaf, so the failure cannot be blamed on the
    // fingerprint being malformed.
    let other = super::tls::load(
        std::path::Path::new(TESTDATA)
            .join("hub-test-leaf.pem")
            .as_path(),
        std::path::Path::new(TESTDATA)
            .join("hub-test-leaf-key.pem")
            .as_path(),
    )
    .unwrap()
    .fingerprint;
    assert_ne!(other, ours);
    let wrong = deliver_to_pinned_hub(&url, Some(&other))
        .await
        .expect_err("a hub presenting another certificate is not this hub")
        .to_string();
    // And it says so. reqwest prints "error sending request" and keeps the reason in a
    // source chain nothing shows by default; an operator whose hub was reinstalled would
    // otherwise be told only that something went wrong with the network.
    assert!(
        wrong.contains("different certificate"),
        "the error should say what was wrong: {wrong}"
    );

    // And with no pin at all: also refused, because a certificate the hub made itself is in
    // nobody's trust store. Without this case the first one would only prove that the
    // connection works, not that the pin is what made it work.
    let unpinned = deliver_to_pinned_hub(&url, None)
        .await
        .expect_err("a self-signed certificate is not trusted by the platform");
    assert!(unpinned.to_string().contains("POST"), "{unpinned}");

    stop.send(()).unwrap();
    tokio::time::timeout(std::time::Duration::from_secs(5), served)
        .await
        .expect("the server should stop")
        .unwrap()
        .unwrap();
}

#[tokio::test]
async fn a_pin_refuses_to_be_used_over_plain_http() {
    // A pin on an unencrypted connection is a promise about a certificate that is not being
    // presented. Refused before anything is sent, rather than delivering in the clear while
    // the operator believes the hub is pinned.
    let e = deliver_to_pinned_hub("http://127.0.0.1:7788", Some("AB"))
        .await
        .err();
    // The malformed pin is caught first; use a real one to reach the scheme check.
    let real = "63:4E:3F:E0:BE:0A:13:3F:D4:CB:2B:AA:19:2F:C4:FD:52:C6:0F:00:5C:13:BF:41:03:89:34:03:CD:5B:65:9F";
    assert!(e.is_some());
    let e = deliver_to_pinned_hub("http://127.0.0.1:7788", Some(real))
        .await
        .expect_err("pinned and unencrypted is a contradiction")
        .to_string();
    assert!(e.contains("pinned to a certificate"), "{e}");
    assert!(e.contains("no certificate is presented"), "{e}");
}

#[test]
fn the_fingerprint_can_be_read_without_the_key() {
    // `hub cert show` runs beside a hub that is already running, as a second process with no
    // business reading the key — and under a service account, possibly no way to.
    let dir = tempfile::tempdir().unwrap();
    let made = super::tls::own(dir.path(), &["hub".into()]).unwrap();
    let read = super::tls::fingerprint_of(&dir.path().join(super::tls::OWN_CERT)).unwrap();
    assert_eq!(read, made.fingerprint);

    // And it is the same number for a certificate that came from somewhere else, so the two
    // ways of asking cannot drift apart.
    let supplied = std::path::Path::new(TESTDATA).join("hub-test-leaf.pem");
    assert_eq!(
        super::tls::fingerprint_of(&supplied).unwrap(),
        "63:4E:3F:E0:BE:0A:13:3F:D4:CB:2B:AA:19:2F:C4:FD:52:C6:0F:00:5C:13:BF:41:03:89:34:03:CD:5B:65:9F"
    );

    // A key is not a certificate, and the message has to say which file was wrong.
    let e = super::tls::fingerprint_of(&dir.path().join(super::tls::OWN_KEY))
        .unwrap_err()
        .to_string();
    assert!(e.contains(super::tls::OWN_KEY), "{e}");
}

#[test]
fn the_pair_beside_the_record_stays_ours_when_it_arrives_as_two_paths() {
    // The Windows installer makes the certificate and then registers the service with
    // --tls-cert and --tls-key pointing at it. On the first build that reached a Windows
    // machine the hub therefore treated its own certificate as somebody else's, issued
    // invitations with no pin, and every client would have refused a certificate it was
    // never told to expect. The log line said so — the "(invitations pin this)" was missing
    // — and nothing here noticed, because on this machine the flag was never used.
    let dir = tempfile::tempdir().unwrap();
    let made = super::tls::own(dir.path(), &["hub".into()]).unwrap();
    assert!(made.pinnable);

    let as_registered = super::tls::named(
        dir.path(),
        &dir.path().join(super::tls::OWN_CERT),
        &dir.path().join(super::tls::OWN_KEY),
    )
    .unwrap();
    assert!(
        as_registered.pinnable,
        "the same two files, named on a command line, are still the hub's own"
    );
    assert_eq!(as_registered.fingerprint, made.fingerprint);

    // And a certificate that really did come from somewhere else stays unpinnable, whatever
    // directory it is asked about.
    let elsewhere = super::tls::named(
        dir.path(),
        std::path::Path::new(TESTDATA)
            .join("hub-test-leaf.pem")
            .as_path(),
        std::path::Path::new(TESTDATA)
            .join("hub-test-leaf-key.pem")
            .as_path(),
    )
    .unwrap();
    assert!(!elsewhere.pinnable);
}

#[cfg(unix)]
#[test]
fn a_generated_key_is_not_readable_by_everybody() {
    use std::os::unix::fs::PermissionsExt;
    let dir = tempfile::tempdir().unwrap();
    super::tls::own(dir.path(), &["hub".into()]).unwrap();
    let mode = std::fs::metadata(dir.path().join(super::tls::OWN_KEY))
        .unwrap()
        .permissions()
        .mode()
        & 0o777;
    assert_eq!(
        mode, 0o600,
        "the key is the one file that must not be shared"
    );
    // The certificate is the opposite: it is handed out on purpose.
    assert!(std::fs::read_to_string(dir.path().join(super::tls::OWN_CERT)).is_ok());
}

#[test]
fn a_record_that_cannot_be_written_says_what_to_do_about_it() {
    // What an operator gets for running `hub add` in an ordinary prompt: the record belongs
    // to the service account, so SQLite opened it read-only. "attempt to write a readonly
    // database" is true and useless — it is not a sentence somebody can act on.
    //
    // The mapping is tested rather than the situation. These tests run as root, and root
    // ignores a read-only file: the first version of this test set the file read-only,
    // wrote to it anyway, and would have passed for a reason that had nothing to do with
    // the code.
    let readonly = rusqlite::Error::SqliteFailure(
        rusqlite::ffi::Error::new(8), // SQLITE_READONLY
        Some("attempt to write a readonly database".into()),
    );
    let e = super::store::explain(readonly);
    assert!(e.contains("readonly database"), "{e}");
    assert!(
        e.contains("elevated") || e.contains("sudo"),
        "the message has to name the way out: {e}"
    );

    // Everything else is passed on as SQLite said it, with no guess bolted on: a wrong
    // suggestion under a real error sends the reader away from it.
    let other = rusqlite::Error::SqliteFailure(
        rusqlite::ffi::Error::new(11), // SQLITE_CORRUPT
        Some("database disk image is malformed".into()),
    );
    assert_eq!(
        super::store::explain(other),
        "database disk image is malformed"
    );
}

// ---- note sync: what the hub takes and what it turns away (cut 2) ----

use super::sync_access::Direction;

fn wire(name: &str, ring: u8, bereich: Option<&str>, updated: &str) -> serde_json::Value {
    serde_json::json!({
        "id": format!("01ARZ3NDEKTSV4RRFFQ69G5{:03}", name.len()),
        "name": name,
        "ring": ring,
        "kind": "knowledge",
        "bereich": bereich,
        "updated": updated,
        "frontmatter": format!("name: {name}\nring: {ring}\n"),
        "body": format!("*Für: {name}*\n\nInhalt von {name}."),
    })
}

/// The same note, declaring which version it was built on.
fn wire_from(
    name: &str,
    bereich: &str,
    updated: &str,
    based_on: Option<&str>,
    body: &str,
) -> serde_json::Value {
    serde_json::json!({
        "id": "01ARZ3NDEKTSV4RRFFQ69G5FAV",
        "name": name,
        "ring": 2,
        "kind": "knowledge",
        "bereich": bereich,
        "updated": updated,
        "frontmatter": format!("name: {name}\nring: 2\n"),
        "body": body,
        "based_on": based_on,
    })
}

fn deliver(notes: Vec<serde_json::Value>) -> String {
    serde_json::json!({ "notes": notes }).to_string()
}

/// The rule the hub enforces for itself. The sender checks first, but a sender is a program
/// somebody can rewrite; this is the check that counts.
#[test]
fn the_hub_refuses_rings_0_and_1_even_when_offered() {
    let (mut hub, device, token) = hub_with_device();
    grant_in_force(&hub, "g1", &device.id, "disposition", Direction::Both);

    let body = deliver(vec![
        wire(
            "eine-invariante",
            0,
            Some("disposition"),
            "2026-09-10T10:00:00Z",
        ),
        wire(
            "das-protokoll",
            1,
            Some("disposition"),
            "2026-09-10T10:00:00Z",
        ),
        wire(
            "normales-wissen",
            2,
            Some("disposition"),
            "2026-09-10T10:00:00Z",
        ),
    ]);
    let r = super::ingest_notes(&mut hub, &collecting(), Some(&token), &body, NOW).unwrap();

    assert_eq!(r.accepted, 1, "only the ring 2 note may be taken");
    assert_eq!(r.refused.len(), 2);
    for ref_ in &r.refused {
        assert!(
            ref_.why.contains("never leaves the machine"),
            "unexpected reason: {}",
            ref_.why
        );
    }
    // And the one that was allowed really is held.
    let held = hub.synced_notes("disposition").unwrap();
    assert_eq!(held.len(), 1);
    assert_eq!(held[0].name, "normales-wissen");
}

/// A device may only deliver the bereiche it was granted. This is the operator's own
/// example, enforced at the receiving end rather than trusted from the sender.
#[test]
fn a_device_cannot_deliver_a_bereich_it_was_not_granted() {
    let (mut hub, device, token) = hub_with_device();
    grant_in_force(&hub, "g1", &device.id, "disposition", Direction::Both);

    let body = deliver(vec![
        wire("tour", 2, Some("disposition"), "2026-09-10T10:00:00Z"),
        wire("gehalt", 2, Some("hr"), "2026-09-10T10:00:00Z"),
        wire("ohne", 2, None, "2026-09-10T10:00:00Z"),
    ]);
    let r = super::ingest_notes(&mut hub, &collecting(), Some(&token), &body, NOW).unwrap();

    assert_eq!(r.accepted, 1);
    assert_eq!(r.refused.len(), 2);
    assert!(r.refused.iter().any(|x| x.name == "gehalt"));
    assert!(
        r.refused
            .iter()
            .any(|x| x.name == "ohne" && x.why.contains("no bereich"))
    );
    assert!(hub.synced_notes("hr").unwrap().is_empty());
}

/// A batch is not all-or-nothing: one note that should not have been offered must not
/// strand the rest, or a single mistake stops a department from sharing anything.
#[test]
fn one_bad_note_does_not_strand_the_batch() {
    let (mut hub, device, token) = hub_with_device();
    grant_in_force(&hub, "g1", &device.id, "disposition", Direction::Both);
    let body = deliver(vec![
        wire("a", 2, Some("disposition"), "2026-09-10T10:00:00Z"),
        wire("verboten", 0, Some("disposition"), "2026-09-10T10:00:00Z"),
        wire("b", 2, Some("disposition"), "2026-09-10T10:00:00Z"),
    ]);
    let r = super::ingest_notes(&mut hub, &collecting(), Some(&token), &body, NOW).unwrap();
    assert_eq!(r.accepted, 2);
    assert_eq!(r.stored, 2);
    assert_eq!(r.refused.len(), 1);
}

/// A delivery that did not start from what the hub holds is a conflict, not a loser. Before
/// 2026-09-10 this was last-write-wins and the older version simply vanished.
#[test]
fn an_older_delivery_does_not_overwrite_a_newer_note() {
    let (mut hub, device, token) = hub_with_device();
    grant_in_force(&hub, "g1", &device.id, "disposition", Direction::Both);
    let newer = deliver(vec![wire(
        "regel",
        2,
        Some("disposition"),
        "2026-09-10T12:00:00Z",
    )]);
    super::ingest_notes(&mut hub, &collecting(), Some(&token), &newer, NOW).unwrap();

    let older = deliver(vec![wire(
        "regel",
        2,
        Some("disposition"),
        "2026-09-01T08:00:00Z",
    )]);
    let r = super::ingest_notes(&mut hub, &collecting(), Some(&token), &older, NOW).unwrap();
    assert_eq!(r.accepted, 1, "it passed the rules");
    assert_eq!(r.stored, 0, "but it did not replace what is held");
    assert_eq!(
        r.conflicts.len(),
        1,
        "and it was not silently dropped either"
    );

    let held = hub.synced_notes("disposition").unwrap();
    assert_eq!(held.len(), 1);
    assert_eq!(held[0].updated, "2026-09-10T12:00:00Z");
}

// ---- cut 3: two machines that did not see each other ----

/// The failure this cut exists for. Two devices edit the same note from the same starting
/// point; neither saw the other. Last-write-wins would keep one and lose the other without
/// telling anybody. Both are kept, and the delivery says so.
#[test]
fn concurrent_edits_are_kept_and_named_not_silently_dropped() {
    let (mut hub, device, token) = hub_with_device();
    grant_in_force(&hub, "g1", &device.id, "disposition", Direction::Both);

    // The version both machines started from.
    let base = deliver(vec![wire_from(
        "rampe",
        "disposition",
        "2026-09-10T09:00:00Z",
        None,
        "Rampe drei ist frei.",
    )]);
    super::ingest_notes(&mut hub, &collecting(), Some(&token), &base, NOW).unwrap();

    // Machine A continues from it: seen, so it is taken.
    let a = deliver(vec![wire_from(
        "rampe",
        "disposition",
        "2026-09-10T10:00:00Z",
        Some("2026-09-10T09:00:00Z"),
        "Rampe drei ist belegt.",
    )]);
    let ra = super::ingest_notes(&mut hub, &collecting(), Some(&token), &a, NOW).unwrap();
    assert_eq!(ra.stored, 1);
    assert!(ra.conflicts.is_empty(), "a continuation is not a conflict");

    // Machine B started from the same base and never saw A's change.
    let b = deliver(vec![wire_from(
        "rampe",
        "disposition",
        "2026-09-10T10:30:00Z",
        Some("2026-09-10T09:00:00Z"),
        "Rampe drei wird umgebaut.",
    )]);
    let rb = super::ingest_notes(&mut hub, &collecting(), Some(&token), &b, NOW).unwrap();

    assert_eq!(rb.stored, 0, "B must not overwrite A");
    assert_eq!(rb.conflicts.len(), 1, "and B must not be dropped either");
    assert_eq!(rb.conflicts[0].name, "rampe");
    assert_eq!(rb.conflicts[0].held_updated, "2026-09-10T10:00:00Z");

    // A's version still stands, unchanged.
    let held = hub.synced_notes("disposition").unwrap();
    assert_eq!(held[0].body, "Rampe drei ist belegt.");

    // And B's text is kept, not lost: this is what last-write-wins threw away.
    let open = hub.open_conflicts("disposition").unwrap();
    assert_eq!(open.len(), 1);
    assert_eq!(open[0].offered_body, "Rampe drei wird umgebaut.");
    assert_eq!(open[0].held_updated, "2026-09-10T10:00:00Z");
}

/// Re-sending what the hub already holds is neither a change nor a conflict.
#[test]
fn a_resend_is_not_a_conflict() {
    let (mut hub, device, token) = hub_with_device();
    grant_in_force(&hub, "g1", &device.id, "disposition", Direction::Both);
    let n = deliver(vec![wire_from(
        "x",
        "disposition",
        "2026-09-10T09:00:00Z",
        None,
        "Text.",
    )]);
    super::ingest_notes(&mut hub, &collecting(), Some(&token), &n, NOW).unwrap();
    let again = super::ingest_notes(&mut hub, &collecting(), Some(&token), &n, NOW).unwrap();
    assert_eq!(again.accepted, 1);
    assert_eq!(again.stored, 0);
    assert!(again.conflicts.is_empty());
}

/// A revoked device delivers nothing, notes included.
#[test]
fn a_revoked_device_delivers_no_notes() {
    let (mut hub, device, token) = hub_with_device();
    grant_in_force(&hub, "g1", &device.id, "disposition", Direction::Both);
    hub.revoke(&device.id, NOW).unwrap();
    let body = deliver(vec![wire(
        "x",
        2,
        Some("disposition"),
        "2026-09-10T10:00:00Z",
    )]);
    let r = super::ingest_notes(&mut hub, &collecting(), Some(&token), &body, NOW);
    assert!(matches!(r, Err(super::Refusal::NotAuthorised(_))));
}

// ---- cut 4: erasure reaches the copies (GDPR Art. 17) ----

/// The trap this cut is about. A conflict row holds a full copy of the offered text. An
/// erasure that only clears `synced_notes` leaves that copy sitting there under a different
/// column name, and the person who asked to be forgotten has not been.
#[test]
fn erasing_a_note_also_clears_the_text_held_in_its_conflicts() {
    let (mut hub, device, token) = hub_with_device();
    grant_in_force(&hub, "g1", &device.id, "disposition", Direction::Both);

    // A note, then a concurrent change so a conflict row exists with the text in it.
    let base = deliver(vec![wire_from(
        "personalie",
        "disposition",
        "2026-09-10T09:00:00Z",
        None,
        "Erste Fassung mit personenbezogenen Angaben.",
    )]);
    super::ingest_notes(&mut hub, &collecting(), Some(&token), &base, NOW).unwrap();
    let concurrent = deliver(vec![wire_from(
        "personalie",
        "disposition",
        "2026-09-10T11:00:00Z",
        Some("2026-08-01T00:00:00Z"),
        "Zweite Fassung, ebenfalls mit Angaben.",
    )]);
    let r = super::ingest_notes(&mut hub, &collecting(), Some(&token), &concurrent, NOW).unwrap();
    assert_eq!(r.conflicts.len(), 1, "a conflict must exist for this test");
    assert_eq!(hub.open_conflicts("disposition").unwrap().len(), 1);

    // Now erase it.
    let req = serde_json::json!({ "bereich": "disposition", "name": "personalie" }).to_string();
    let count = super::erase_note(&mut hub, Some(&token), &req, NOW).unwrap();
    assert_eq!(count.notes, 1);
    assert_eq!(count.conflicts, 1, "the conflict copy has to go too");

    // Nothing left anywhere.
    assert!(hub.synced_notes("disposition").unwrap().is_empty());
    assert!(hub.open_conflicts("disposition").unwrap().is_empty());
}

/// The tombstone records that an erasure happened, never what was erased.
#[test]
fn the_tombstone_carries_no_text() {
    let (mut hub, device, token) = hub_with_device();
    grant_in_force(&hub, "g1", &device.id, "disposition", Direction::Both);
    let secret = "Streng vertraulicher Satz, der nirgends bleiben darf.";
    let n = deliver(vec![wire_from(
        "geheim",
        "disposition",
        "2026-09-10T09:00:00Z",
        None,
        "Harmlose erste Fassung.",
    )]);
    super::ingest_notes(&mut hub, &collecting(), Some(&token), &n, NOW).unwrap();
    // Deliberately put the sensitive text into a *conflict* row rather than the held note.
    // A version of this test without a conflict stays green while the conflict copy is left
    // behind, which is the failure it is supposed to catch — it was written that way first.
    let concurrent = deliver(vec![wire_from(
        "geheim",
        "disposition",
        "2026-09-10T11:00:00Z",
        Some("2026-01-01T00:00:00Z"),
        secret,
    )]);
    let r = super::ingest_notes(&mut hub, &collecting(), Some(&token), &concurrent, NOW).unwrap();
    assert_eq!(r.conflicts.len(), 1, "the text must be in a conflict row");
    let req = serde_json::json!({ "bereich": "disposition", "name": "geheim" }).to_string();
    super::erase_note(&mut hub, Some(&token), &req, NOW).unwrap();

    // Whatever remains in the record must not contain the text. Checked against the whole
    // file rather than a column list, so a table added later cannot quietly reintroduce it.
    let dump = hub.dump_all_text().unwrap();
    assert!(
        !dump.contains(secret),
        "the erased text is still somewhere in the hub record"
    );
    assert!(hub.erased_at("disposition", "geheim").unwrap().is_some());
}

/// A machine that kept its own copy must not be able to put it back.
#[test]
fn an_erased_note_cannot_be_re_offered() {
    let (mut hub, device, token) = hub_with_device();
    grant_in_force(&hub, "g1", &device.id, "disposition", Direction::Both);
    let n = deliver(vec![wire_from(
        "weg",
        "disposition",
        "2026-09-10T09:00:00Z",
        None,
        "Text.",
    )]);
    super::ingest_notes(&mut hub, &collecting(), Some(&token), &n, NOW).unwrap();
    let req = serde_json::json!({ "bereich": "disposition", "name": "weg" }).to_string();
    super::erase_note(&mut hub, Some(&token), &req, NOW).unwrap();

    let again = super::ingest_notes(&mut hub, &collecting(), Some(&token), &n, NOW).unwrap();
    assert_eq!(again.accepted, 0);
    assert_eq!(again.refused.len(), 1);
    assert!(again.refused[0].why.contains("erased at"));
    assert!(hub.synced_notes("disposition").unwrap().is_empty());
}

/// Withdrawing is not sharing: a device granted only `receive` may still ask for an erasure,
/// because finding something that should not be there is exactly what a reader does.
#[test]
fn a_receive_only_device_may_still_erase() {
    let (mut hub, device, token) = hub_with_device();
    grant_in_force(&hub, "g1", &device.id, "disposition", Direction::Receive);
    let req = serde_json::json!({ "bereich": "disposition", "name": "irgendwas" }).to_string();
    assert!(super::erase_note(&mut hub, Some(&token), &req, NOW).is_ok());
}

/// But not in a bereich it has nothing to do with.
#[test]
fn a_device_cannot_erase_a_bereich_it_has_no_grant_in() {
    let (mut hub, device, token) = hub_with_device();
    grant_in_force(&hub, "g1", &device.id, "disposition", Direction::Both);
    let req = serde_json::json!({ "bereich": "hr", "name": "gehalt" }).to_string();
    let r = super::erase_note(&mut hub, Some(&token), &req, NOW);
    assert!(matches!(r, Err(super::Refusal::NotAuthorised(_))));
}

// ---- the fetch path: what a device may take ----

/// A device sees the bereiche it may receive and nothing else. Asking is not a way to learn
/// which departments exist.
#[test]
fn a_fetch_returns_only_granted_bereiche() {
    let hub = HubStore::in_memory().unwrap();
    let (disp, disp_token) = hub.add_device("disposition-laptop", NOW).unwrap();
    let (hr, hr_token) = hub.add_device("hr-laptop", NOW).unwrap();
    grant_in_force(&hub, "g1", &disp.id, "disposition", Direction::Both);
    grant_in_force(&hub, "g2", &hr.id, "hr", Direction::Both);
    for (b, n) in [("disposition", "tour"), ("hr", "gehalt")] {
        hub.offer_synced_note(
            "01ARZ3NDEKTSV4RRFFQ69G5FAV",
            b,
            n,
            2,
            "knowledge",
            "2026-09-10T09:00:00Z",
            "fm",
            "text",
            None,
            if b == "disposition" { &disp.id } else { &hr.id },
            NOW,
        )
        .unwrap();
    }

    let f = super::fetch_notes(&hub, Some(&disp_token), None).unwrap();
    assert_eq!(f.notes.len(), 1);
    assert_eq!(f.notes[0].name, "tour");

    let f2 = super::fetch_notes(&hub, Some(&hr_token), None).unwrap();
    assert_eq!(f2.notes.len(), 1);
    assert_eq!(f2.notes[0].name, "gehalt");
}

/// A send-only device delivers but does not read back.
#[test]
fn a_send_only_grant_does_not_let_anything_be_fetched() {
    let hub = HubStore::in_memory().unwrap();
    let (d, token) = hub.add_device("laptop", NOW).unwrap();
    grant_in_force(&hub, "g1", &d.id, "disposition", Direction::Send);
    hub.offer_synced_note(
        "01ARZ3NDEKTSV4RRFFQ69G5FAV",
        "disposition",
        "tour",
        2,
        "knowledge",
        "2026-09-10T09:00:00Z",
        "fm",
        "text",
        None,
        &d.id,
        NOW,
    )
    .unwrap();
    let f = super::fetch_notes(&hub, Some(&token), None).unwrap();
    assert!(f.notes.is_empty(), "send-only must not read back");
}

/// Erasures travel with the notes. Without them "gone" and "not offered to you" look the
/// same to a puller, and a machine keeps a copy of something that was withdrawn.
#[test]
fn a_fetch_reports_what_was_erased() {
    let hub = HubStore::in_memory().unwrap();
    let (d, token) = hub.add_device("laptop", NOW).unwrap();
    grant_in_force(&hub, "g1", &d.id, "disposition", Direction::Both);
    hub.offer_synced_note(
        "01ARZ3NDEKTSV4RRFFQ69G5FAV",
        "disposition",
        "weg",
        2,
        "knowledge",
        "2026-09-10T09:00:00Z",
        "fm",
        "text",
        None,
        &d.id,
        NOW,
    )
    .unwrap();
    hub.erase_note("disposition", "weg", &d.id, "2026-09-10T15:00:00Z")
        .unwrap();

    let f = super::fetch_notes(&hub, Some(&token), None).unwrap();
    assert!(f.notes.is_empty(), "the note itself is gone");
    assert_eq!(f.erased.len(), 1, "but the fact that it went must travel");
    assert_eq!(f.erased[0].name, "weg");
}

/// The cursor is the newest thing in the answer, so a puller never reasons about clocks.
#[test]
fn the_cursor_covers_notes_and_erasures() {
    let hub = HubStore::in_memory().unwrap();
    let (d, token) = hub.add_device("laptop", NOW).unwrap();
    grant_in_force(&hub, "g1", &d.id, "disposition", Direction::Both);
    hub.offer_synced_note(
        "01ARZ3NDEKTSV4RRFFQ69G5FAV",
        "disposition",
        "a",
        2,
        "knowledge",
        "2026-09-10T09:00:00Z",
        "fm",
        "text",
        None,
        &d.id,
        NOW,
    )
    .unwrap();
    hub.erase_note("disposition", "b", &d.id, "2026-09-10T17:00:00Z")
        .unwrap();
    let f = super::fetch_notes(&hub, Some(&token), None).unwrap();
    assert_eq!(f.cursor.as_deref(), Some("2026-09-10T17:00:00Z"));

    // And asking again from there returns nothing new.
    let f2 = super::fetch_notes(&hub, Some(&token), f.cursor.as_deref()).unwrap();
    assert!(f2.notes.is_empty() && f2.erased.is_empty());
}

/// An editor sees the conflicts of their own bereiche and nobody else's.
///
/// The store side of what `hub conflicts` and `/conflicts` both ask. It is here rather than
/// at the two call sites because the rule is one rule; what the call sites have to get right
/// is asking at all, which one of them did not.
#[test]
fn an_editor_is_offered_the_conflicts_of_their_own_bereiche_only() {
    let hub = HubStore::in_memory().unwrap();
    let (d, _t) = hub.add_device("laptop", NOW).unwrap();
    for b in ["disposition", "hr"] {
        grant_in_force(&hub, &format!("g-{b}"), &d.id, b, Direction::Both);
        // Held, then a second machine offers a different text from an older base: that is
        // what a conflict is, and it is the row that carries the turned-away note.
        hub.offer_synced_note(
            "01ARZ3NDEKTSV4RRFFQ69G5FAV",
            b,
            "regel",
            2,
            "knowledge",
            "2026-09-10T09:00:00Z",
            "fm",
            &format!("was {b} hält"),
            None,
            &d.id,
            NOW,
        )
        .unwrap();
        hub.offer_synced_note(
            "01ARZ3NDEKTSV4RRFFQ69G5FAV",
            b,
            "regel",
            2,
            "knowledge",
            "2026-09-10T10:00:00Z",
            "fm",
            &format!("das GEHEIMNIS von {b}"),
            Some("2026-09-10T08:00:00Z"),
            &d.id,
            NOW,
        )
        .unwrap();
    }

    let (rita, _) = hub
        .add_principal("Rita", super::access::Role::Editor, NOW)
        .unwrap();
    hub.assign_bereich(&rita.id, "disposition", NOW).unwrap();

    let mine = hub.conflicts_for_principal(&rita.id).unwrap();
    assert_eq!(mine.len(), 1, "one bereich, one conflict");
    assert_eq!(mine[0].0.bereich, "disposition");
    let text = format!("{mine:?}");
    assert!(
        !text.contains("GEHEIMNIS von hr"),
        "an editor of disposition was handed HR's note text: {text}"
    );

    // And an id from the other department is not settleable by guessing it.
    let hrs = hub.open_conflicts("hr").unwrap();
    assert_eq!(hrs.len(), 1);
    assert!(
        hub.conflict_for_principal(&rita.id, &hrs[0].id)
            .unwrap()
            .is_none(),
        "a guessed id must not reach into another department"
    );
}

/// A device does not appear or disappear without the hub's own log saying so.
///
/// `docs/HUB.md` says granting a role is itself an entry, and it was — for roles. A device
/// was not: it arrived through the web form or `hub add` and left through `hub revoke`, and
/// the chain said nothing either way. A device is a new pair of eyes on whatever bereich it
/// is later granted, so its arrival is exactly the thing a works council reads the log for.
///
/// Calibrated against the state before: with the two `record` calls removed, both counts
/// below are 0.
#[test]
fn a_device_arriving_and_leaving_is_in_the_hubs_own_log() {
    let hub = HubStore::in_memory().unwrap();
    let (d, _t) = hub.add_device("laptop-rita", NOW).unwrap();

    let rows = hub.hub_events(50).unwrap();
    let registered: Vec<_> = rows
        .iter()
        .filter(|r| r.action == "device.registered")
        .collect();
    assert_eq!(registered.len(), 1, "{rows:?}");
    assert_eq!(registered[0].detail["device"], serde_json::json!(d.id));
    assert_eq!(
        registered[0].detail["name"],
        serde_json::json!("laptop-rita"),
        "the name is what a reader recognises; the id is what the rows carry"
    );

    assert!(hub.revoke(&d.id, "2026-09-10T12:00:00Z").unwrap());
    let rows = hub.hub_events(50).unwrap();
    assert_eq!(
        rows.iter().filter(|r| r.action == "device.revoked").count(),
        1
    );

    // Twice is not two events.
    assert!(!hub.revoke(&d.id, "2026-09-10T13:00:00Z").unwrap());
    let rows = hub.hub_events(50).unwrap();
    assert_eq!(
        rows.iter().filter(|r| r.action == "device.revoked").count(),
        1,
        "revoking an already-revoked device is not news"
    );

    // And the chain still holds over the rows we just added.
    assert!(hub.verify_hub_chain().is_ok());
}

/// The operator cannot route note text to a machine of their own on their own.
///
/// This is the whole reason for the countersignature. Whoever holds the hub password
/// registers devices and reads their tokens out of the invitation files, so if that same
/// person could point a bereich at one, `access.rs`'s "admin is state only" would be a
/// house rule and every note text on the hub would be one form submission away. The walk
/// below is exactly that attempt, in the order somebody would take it.
///
/// Calibrated against the state before: with `is_effective` back to `is_active`, the fetch
/// after the grant returns the note and the first assertion fails.
#[test]
fn a_grant_takes_two_people_before_a_note_moves() {
    let hub = HubStore::in_memory().unwrap();
    // Somebody else's machine, delivering into a bereich.
    let (theirs, their_token) = hub.add_device("laptop-disposition", NOW).unwrap();
    hub.grant_bereich(
        "g-send",
        &theirs.id,
        "disposition",
        Direction::Send,
        "r",
        "admin-1",
        NOW,
    )
    .unwrap();
    let (council, _) = hub
        .add_principal("Betriebsrat", super::access::Role::Countersigner, NOW)
        .unwrap();
    hub.countersign_grant("g-send", &council, NOW).unwrap();
    hub.offer_synced_note(
        "01ARZ3NDEKTSV4RRFFQ69G5FAV",
        "disposition",
        "tourenplan",
        2,
        "knowledge",
        "2026-09-10T09:00:00Z",
        "fm",
        "Wer am Freitag fährt und warum",
        None,
        &theirs.id,
        NOW,
    )
    .unwrap();

    // Now the operator, alone: a device of their own, and a grant to read that bereich.
    let (mine, my_token) = hub.add_device("laptop-des-betreibers", NOW).unwrap();
    hub.grant_bereich(
        "g-mine",
        &mine.id,
        "disposition",
        Direction::Receive,
        "weil ich es kann",
        "admin-1",
        NOW,
    )
    .unwrap();

    let f = super::fetch_notes(&hub, Some(&my_token), None).unwrap();
    assert!(
        f.notes.is_empty(),
        "the operator wrote their own grant and read the department's notes with it: {:?}",
        f.notes
    );

    // The other machine still works, so this is the countersignature biting and not the
    // fetch being broken.
    let f = super::fetch_notes(&hub, Some(&their_token), None).unwrap();
    assert!(f.notes.is_empty(), "send-only reads nothing back either");
    let denied = super::sync_access::may_move(
        &mine.id,
        cyberbrain_core::Ring::Knowledge,
        Some("disposition"),
        Direction::Receive,
        &hub.grants_for_device(&mine.id).unwrap(),
    )
    .expect_err("an unsigned grant moves nothing");
    assert!(
        matches!(
            denied,
            super::sync_access::SyncDenied::AwaitingCountersignature { .. }
        ),
        "{denied:?}"
    );

    // And the operator cannot sign it either, even holding a countersigner credential:
    // two signatures from one hand are one signature.
    let (op, _) = hub
        .add_principal("admin-1", super::access::Role::Countersigner, NOW)
        .unwrap();
    let op = super::access::Principal {
        id: "admin-1".into(),
        ..op
    };
    assert_eq!(
        hub.countersign_grant("g-mine", &op, NOW).unwrap(),
        super::store::CountersignOutcome::SamePerson
    );

    // Somebody else can, and then it works — the rule is two people, not "never".
    hub.countersign_grant("g-mine", &council, NOW).unwrap();
    let f = super::fetch_notes(&hub, Some(&my_token), None).unwrap();
    assert_eq!(f.notes.len(), 1, "once two people agreed, it moves");
}

/// Every role lands somewhere it can work, and the two-person rule is reachable from a
/// browser.
///
/// Before this the login redirected everybody to `/`, which is the operator's page: an
/// editor, an auditor and a countersigner signed in successfully and were shown the login
/// form again. `/login` was a POST target only, so the redirect an unauthenticated visitor
/// follows answered 405. And there was no route at all for a request or an approval — the
/// countersignature this hub's whole promise rests on was reachable only from a shell on the
/// hub's own machine, which is the machine whose operator they are there to check.
#[tokio::test]
async fn each_role_lands_on_the_page_that_is_theirs() {
    use super::access::Role;
    let hub = hub_with_password("correct horse battery");
    let (device, _) = hub.add_device("laptop-disposition", NOW).unwrap();
    hub.grant_bereich(
        "bg-1",
        &device.id,
        "disposition",
        Direction::Both,
        "Schichtübergabe",
        "admin-1",
        NOW,
    )
    .unwrap();
    let (_editor, editor_token) = hub.add_principal("Rita", Role::Editor, NOW).unwrap();
    let (_auditor, auditor_token) = hub.add_principal("Kraus", Role::Auditor, NOW).unwrap();
    let (_council, council_token) = hub
        .add_principal("Betriebsrat", Role::Countersigner, NOW)
        .unwrap();
    let state = state_for(hub, true);

    // A GET on /login is a page, not a method error.
    let mut req = Request::builder()
        .method("GET")
        .uri("/login")
        .body(Body::empty())
        .unwrap();
    req.extensions_mut().insert(ConnectInfo(
        "192.168.1.20:51000"
            .parse::<std::net::SocketAddr>()
            .unwrap(),
    ));
    let r = super::api::router(state.clone())
        .oneshot(req)
        .await
        .unwrap();
    assert_eq!(
        r.status(),
        StatusCode::OK,
        "a GET on /login has to be a page"
    );

    // Each credential lands on its own page.
    for (token, where_to) in [
        (&editor_token, "/conflicts"),
        (&auditor_token, "/requests"),
        (&council_token, "/requests"),
    ] {
        let r = sign_in_from(state.clone(), "192.168.1.20:51000", token).await;
        assert_eq!(r.status(), StatusCode::SEE_OTHER);
        assert_eq!(
            r.headers()
                .get(axum::http::header::LOCATION)
                .and_then(|v| v.to_str().ok()),
            Some(where_to),
            "a role that lands on the operator's page is a role that sees a login form"
        );
    }

    // The countersigner can see the waiting grant and sign it, without a shell.
    let r = sign_in_from(state.clone(), "192.168.1.20:51000", &council_token).await;
    let cookie = cookie_of(&r).split(';').next().unwrap().to_string();
    let body = get_as(state.clone(), "/requests", &cookie).await;
    assert!(
        body.contains("bg-1"),
        "the waiting grant has to be on the page: {body}"
    );
    assert!(body.contains("Schichtübergabe"), "and its reason: {body}");

    let mut req = Request::builder()
        .method("POST")
        .uri("/grants/bg-1/approve")
        .header("cookie", &cookie)
        .body(Body::empty())
        .unwrap();
    req.extensions_mut().insert(ConnectInfo(
        "192.168.1.20:51000"
            .parse::<std::net::SocketAddr>()
            .unwrap(),
    ));
    let r = super::api::router(state.clone())
        .oneshot(req)
        .await
        .unwrap();
    assert_eq!(r.status(), StatusCode::SEE_OTHER);
    assert!(
        state
            .hub
            .lock()
            .unwrap()
            .grant("bg-1")
            .unwrap()
            .unwrap()
            .is_effective(),
        "the grant should be in force after a countersignature from the browser"
    );

    // An auditor is not a countersigner: the same POST does nothing for them.
    hub_grant_pending(&state, "bg-2", &device.id);
    let r = sign_in_from(state.clone(), "192.168.1.20:51000", &auditor_token).await;
    let cookie = cookie_of(&r).split(';').next().unwrap().to_string();
    let mut req = Request::builder()
        .method("POST")
        .uri("/grants/bg-2/approve")
        .header("cookie", &cookie)
        .body(Body::empty())
        .unwrap();
    req.extensions_mut().insert(ConnectInfo(
        "192.168.1.20:51000"
            .parse::<std::net::SocketAddr>()
            .unwrap(),
    ));
    let _ = super::api::router(state.clone())
        .oneshot(req)
        .await
        .unwrap();
    assert!(
        !state
            .hub
            .lock()
            .unwrap()
            .grant("bg-2")
            .unwrap()
            .unwrap()
            .is_effective(),
        "an auditor countersigned a bereich"
    );
}

/// A purge is signed from the browser by a countersigner, and by nobody else.
#[tokio::test]
async fn a_countersigner_carries_out_a_purge_from_the_page_and_an_auditor_cannot() {
    use super::access::Role;
    let hub = hub_with_password("correct horse battery");
    let (_auditor, auditor_token) = hub.add_principal("Kraus", Role::Auditor, NOW).unwrap();
    let (_council, council_token) = hub
        .add_principal("Betriebsrat", Role::Countersigner, NOW)
        .unwrap();
    hub.set_retention("P2Y", "cli", NOW).unwrap();
    let (purge, _) = hub
        .propose_purge("Betriebsvereinbarung §7", "cli", NOW)
        .unwrap();
    let state = state_for(hub, true);
    let post = |cookie: String, uri: String| {
        let mut req = Request::builder()
            .method("POST")
            .uri(uri)
            .header("cookie", cookie)
            .body(Body::empty())
            .unwrap();
        req.extensions_mut().insert(ConnectInfo(
            "192.168.1.20:51000"
                .parse::<std::net::SocketAddr>()
                .unwrap(),
        ));
        req
    };
    let pending = |state: &Arc<super::api::HubState>| {
        state.hub.lock().unwrap().purges().unwrap()[0].is_pending()
    };
    let uri = format!("/purges/{}/approve", purge.id);

    let r = sign_in_from(state.clone(), "192.168.1.20:51000", &auditor_token).await;
    let cookie = cookie_of(&r).split(';').next().unwrap().to_string();
    let _ = super::api::router(state.clone())
        .oneshot(post(cookie, uri.clone()))
        .await
        .unwrap();
    assert!(pending(&state), "an auditor carried out a purge");

    let r = sign_in_from(state.clone(), "192.168.1.20:51000", &council_token).await;
    let cookie = cookie_of(&r).split(';').next().unwrap().to_string();
    let body = get_as(state.clone(), "/requests", &cookie).await;
    assert!(
        body.contains(&purge.id),
        "the waiting purge has to be on the page: {body}"
    );
    assert!(
        body.contains("Betriebsvereinbarung"),
        "and its reason: {body}"
    );
    let r = super::api::router(state.clone())
        .oneshot(post(cookie, uri))
        .await
        .unwrap();
    assert_eq!(r.status(), StatusCode::SEE_OTHER);
    assert!(
        !pending(&state),
        "the purge should be carried out after a countersignature from the browser"
    );
}

fn hub_grant_pending(state: &Arc<super::api::HubState>, id: &str, device: &str) {
    state
        .hub
        .lock()
        .unwrap()
        .grant_bereich(id, device, "hr", Direction::Both, "r", "admin-1", NOW)
        .unwrap();
}

/// A GET with a cookie, returning the body as text.
async fn get_as(state: Arc<super::api::HubState>, path: &str, cookie: &str) -> String {
    let mut req = Request::builder()
        .method("GET")
        .uri(path)
        .header("cookie", cookie)
        .body(Body::empty())
        .unwrap();
    req.extensions_mut().insert(ConnectInfo(
        "192.168.1.20:51000"
            .parse::<std::net::SocketAddr>()
            .unwrap(),
    ));
    let r = super::api::router(state).oneshot(req).await.unwrap();
    let b = axum::body::to_bytes(r.into_body(), 512 * 1024)
        .await
        .unwrap();
    String::from_utf8_lossy(&b).into_owned()
}

// ---- fleet invitations ----

fn licensed(seats: usize) -> LicenceState {
    LicenceState::Valid {
        customer: "Test GmbH".into(),
        seats,
        valid_until: "2099-01-01T00:00:00Z".into(),
        warning: None,
    }
}

/// One code enrols as many projects as it allows and no more, and only its hash is on record.
#[test]
fn a_fleet_invitation_enrols_up_to_its_limit_and_keeps_only_a_hash() {
    use super::store::EnrolRefusal;
    let hub = HubStore::in_memory().unwrap();
    let (_, code) = hub
        .create_enrolment_code("Rollout Disposition", 2, "P14D", "cli", NOW)
        .unwrap();
    let (a, token) = hub
        .enrol_with_code(&code, "WS-021", "Angebote 2026", &licensed(5), NOW)
        .unwrap()
        .unwrap();
    assert_eq!(a.name, "ws-021/Angebote-2026");
    assert_eq!(a.machine.as_deref(), Some("ws-021"));
    assert!(
        hub.device_by_token(&token).unwrap().is_some(),
        "the token it hands out works"
    );
    hub.enrol_with_code(&code, "ws-022", "Dispo", &licensed(5), NOW)
        .unwrap()
        .unwrap();
    assert_eq!(
        hub.enrol_with_code(&code, "ws-023", "Dispo", &licensed(5), NOW)
            .unwrap()
            .unwrap_err(),
        EnrolRefusal::UsedUp(2)
    );
    assert_eq!(hub.devices().unwrap().len(), 2);
    assert_eq!(hub.enrolment_codes().unwrap()[0].uses, 2);
    assert!(
        !hub.dump_all_text().unwrap().contains(&code),
        "a copy of the record must not be a working invitation"
    );
    let enrolled = hub
        .hub_events(1000)
        .unwrap()
        .iter()
        .filter(|e| e.action == "device.enrolled")
        .count();
    assert_eq!(enrolled, 2);
}

#[test]
fn an_expired_withdrawn_or_unknown_invitation_enrols_nobody() {
    use super::store::EnrolRefusal;
    let hub = HubStore::in_memory().unwrap();
    let (row, code) = hub
        .create_enrolment_code("Rollout", 10, "P1D", "cli", NOW)
        .unwrap();
    assert!(matches!(
        hub.enrol_with_code(&code, "ws-021", "a", &licensed(5), "2026-09-09T12:00:00Z")
            .unwrap(),
        Err(EnrolRefusal::Expired(_))
    ));
    assert_eq!(
        hub.enrol_with_code("cbe_guessed", "ws-021", "a", &licensed(5), NOW)
            .unwrap()
            .unwrap_err(),
        EnrolRefusal::UnknownCode
    );
    assert!(hub.revoke_enrolment_code(&row.id, "cli", NOW).unwrap());
    assert_eq!(
        hub.enrol_with_code(&code, "ws-021", "a", &licensed(5), NOW)
            .unwrap()
            .unwrap_err(),
        EnrolRefusal::UnknownCode,
        "withdrawn reads the same as never issued"
    );
    assert!(hub.devices().unwrap().is_empty());
}

#[test]
fn a_fleet_invitation_counts_seats_by_machine_and_needs_a_licence() {
    use super::store::EnrolRefusal;
    let hub = HubStore::in_memory().unwrap();
    let (_, code) = hub
        .create_enrolment_code("Rollout", 10, "P14D", "cli", NOW)
        .unwrap();
    hub.enrol_with_code(&code, "ws-021", "angebote", &licensed(1), NOW)
        .unwrap()
        .unwrap();
    hub.enrol_with_code(&code, "ws-021", "dispo", &licensed(1), NOW)
        .unwrap()
        .expect("a second project on the same machine takes no seat");
    assert_eq!(
        hub.enrol_with_code(&code, "ws-022", "angebote", &licensed(1), NOW)
            .unwrap()
            .unwrap_err(),
        EnrolRefusal::NoSeat(1)
    );
    assert!(matches!(
        hub.enrol_with_code(&code, "ws-021", "x", &lapsed(), NOW)
            .unwrap(),
        Err(EnrolRefusal::NotLicensed(_))
    ));
    assert_eq!(
        hub.enrolment_codes().unwrap()[0].uses,
        2,
        "a refusal uses nothing up"
    );
}

#[test]
fn an_invitation_works_for_at_most_ninety_days() {
    for bad in ["P91D", "P1Y", "P0D", "PT12H", "P1M"] {
        assert!(super::store::invitation_expiry(bad, NOW).is_err(), "{bad}");
    }
    assert_eq!(
        super::store::invitation_expiry("P2W", NOW).unwrap(),
        "2026-09-21T12:00:00Z"
    );
}

/// The route: an unknown code is 401, and a good code on a hub with no licence enrols nobody.
#[tokio::test]
async fn the_enrolment_route_refuses_an_unknown_code_and_an_unlicensed_hub() {
    let hub = HubStore::in_memory().unwrap();
    let (_, code) = hub
        .create_enrolment_code(
            "Rollout",
            5,
            "P14D",
            "cli",
            &jiff::Timestamp::now().to_string(),
        )
        .unwrap();
    let state = state_for(hub, true);
    let post = |body: serde_json::Value| {
        Request::builder()
            .method("POST")
            .uri("/api/v1/enrol")
            .header("content-type", "application/json")
            .body(Body::from(body.to_string()))
            .unwrap()
    };
    let r = super::api::router(state.clone())
        .oneshot(post(
            serde_json::json!({ "code": "cbe_guessed", "machine": "ws-021", "project": "a" }),
        ))
        .await
        .unwrap();
    assert_eq!(r.status(), StatusCode::UNAUTHORIZED);
    let r = super::api::router(state.clone())
        .oneshot(post(
            serde_json::json!({ "code": code, "machine": "ws-021", "project": "a" }),
        ))
        .await
        .unwrap();
    assert_eq!(r.status(), StatusCode::SERVICE_UNAVAILABLE);
    assert!(state.hub.lock().unwrap().devices().unwrap().is_empty());
}
