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

#[derive(Debug, Clone)]
pub struct HeldWrite {
    pub hold: PiiHold,
    /// The request exactly as it was first attempted; the resolution adds `choice`.
    pub request: WriteRequest,
    /// The findings as the scan reported them, with matched text; never serialised.
    pub findings: Vec<Finding>,
    /// `true` for `POST /notes` (a new note), `false` for `PUT` (an edit).
    pub created: bool,
}

#[derive(Default)]
pub struct Holds {
    inner: Mutex<HashMap<String, HeldWrite>>,
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
        map.insert(
            hold.hold_id.clone(),
            HeldWrite {
                hold: hold.clone(),
                request,
                findings,
                created,
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
