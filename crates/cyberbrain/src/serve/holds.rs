//! Held writes (SPEC §12.4, §8.1). `App::write` returns `Held { findings }` as a value and
//! writes nothing; the UI then has to offer the operator their four choices and come back
//! with one. Between those two requests the write lives here, keyed by a hold id, and the
//! resolution re-runs the *same* `App::write` with the operator's choice filled in — so the
//! scan, the conflict check and the audit rows are the real path's, not a copy.
//!
//! A hold expires. After that the client must resubmit, because the note may have moved
//! on and an old body silently overwriting a newer one is the failure the `expected_updated`
//! check exists to prevent.

use super::wire::{PiiFinding, PiiHold};
use crate::app::WriteRequest;
use cyberbrain_policy::Finding;
use std::collections::HashMap;
use std::sync::Mutex;

/// How long a hold waits for its decision.
pub const HOLD_TTL: jiff::SignedDuration = jiff::SignedDuration::from_secs(15 * 60);

/// Holds waiting at once (S20). Each keeps a whole note body in memory for fifteen minutes;
/// past this the oldest one goes, so a flood of held writes costs memory up to here and no
/// further.
pub const MAX_HOLDS: usize = 256;

#[derive(Debug, Clone)]
pub struct HeldWrite {
    pub hold: PiiHold,
    /// The request exactly as it was first attempted; the resolution adds `choice`.
    pub request: WriteRequest,
    /// The findings as the scan reported them, with matched text; never serialised.
    pub findings: Vec<Finding>,
    /// `true` for `POST /notes` (a new note), `false` for `PUT` (an edit).
    pub created: bool,
    /// Order of arrival, for dropping the oldest past [`MAX_HOLDS`].
    seq: u64,
}

#[derive(Default)]
pub struct Holds {
    inner: Mutex<HashMap<String, HeldWrite>>,
    next: std::sync::atomic::AtomicU64,
}

impl Holds {
    pub fn new() -> Self {
        Self::default()
    }

    fn prune(map: &mut HashMap<String, HeldWrite>) {
        let now = jiff::Timestamp::now();
        map.retain(|_, h| h.hold.expires_at > now);
    }

    /// Register a held write and return the hold as the client sees it.
    pub fn insert(
        &self,
        request: WriteRequest,
        findings: Vec<Finding>,
        created: bool,
        rendered: Vec<PiiFinding>,
    ) -> PiiHold {
        let hold = PiiHold {
            hold_id: ulid::Ulid::generate().to_string(),
            note: request.name.clone(),
            findings: rendered,
            expires_at: jiff::Timestamp::now() + HOLD_TTL,
            dry_run: request.dry_run,
        };
        let mut map = self.inner.lock().unwrap_or_else(|p| p.into_inner());
        Self::prune(&mut map);
        while map.len() >= MAX_HOLDS {
            let Some(oldest) = map
                .iter()
                .min_by_key(|(_, h)| h.seq)
                .map(|(k, _)| k.clone())
            else {
                break;
            };
            map.remove(&oldest);
        }
        map.insert(
            hold.hold_id.clone(),
            HeldWrite {
                hold: hold.clone(),
                request,
                findings,
                created,
                seq: self.next.fetch_add(1, std::sync::atomic::Ordering::Relaxed),
            },
        );
        hold
    }

    /// Remove and return a hold. `None` for an unknown or expired id.
    pub fn take(&self, id: &str) -> Option<HeldWrite> {
        let mut map = self.inner.lock().unwrap_or_else(|p| p.into_inner());
        Self::prune(&mut map);
        map.remove(id)
    }

    /// Every pending hold, oldest first.
    pub fn list(&self) -> Vec<PiiHold> {
        let mut map = self.inner.lock().unwrap_or_else(|p| p.into_inner());
        Self::prune(&mut map);
        let mut out: Vec<PiiHold> = map.values().map(|h| h.hold.clone()).collect();
        out.sort_by_key(|h| h.expires_at);
        out
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn req(name: &str) -> WriteRequest {
        WriteRequest {
            ring: cyberbrain_core::Ring::Knowledge,
            kind: cyberbrain_core::NoteKind::Knowledge,
            name: name.into(),
            body: "x".into(),
            tags: Vec::new(),
            bereich: None,
            retention: None,
            force: false,
            choice: None,
            expected_updated: None,
            supersedes: None,
            valid_from: None,
            invalid_at: None,
            arriving: None,
            dry_run: false,
            anchors: crate::app::Anchoring::default(),
        }
    }

    #[test]
    fn holds_are_bounded_and_the_oldest_goes_first() {
        let h = Holds::new();
        let first = h.insert(req("n-0"), Vec::new(), true, Vec::new());
        for i in 1..MAX_HOLDS + 10 {
            h.insert(req(&format!("n-{i}")), Vec::new(), true, Vec::new());
        }
        assert_eq!(h.list().len(), MAX_HOLDS);
        assert!(h.take(&first.hold_id).is_none(), "the oldest went");
    }
}
