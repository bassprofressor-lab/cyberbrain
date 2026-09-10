//! The shared vocabulary of the tree. Every other crate speaks these types and nothing
//! defines its own copy. See SPEC §3.

use crate::error::{Error, Result};
use serde::{Deserialize, Serialize};
use std::fmt;
use std::path::PathBuf;

/// Trust tier. Lower wins when two blocks contradict (SPEC §3.2).
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(try_from = "u8", into = "u8")]
#[repr(u8)]
pub enum Ring {
    /// Operator invariants. Hard rules. Always injected, override everything.
    Invariant = 0,
    /// Operating protocol and active handoff state. Always injected.
    Protocol = 1,
    /// Curated project knowledge. Retrieved on demand.
    Knowledge = 2,
    /// Session records and observations. Retrieved with lower weight.
    Session = 3,
    /// Imported or unverified material. Retrieved last, marked unverified.
    External = 4,
}

impl Ring {
    pub const ALL: [Ring; 5] = [
        Ring::Invariant,
        Ring::Protocol,
        Ring::Knowledge,
        Ring::Session,
        Ring::External,
    ];

    pub fn as_u8(self) -> u8 {
        self as u8
    }

    /// Rings 0 and 1 ride along in every session, so they carry a hard cap (SPEC §3.2).
    pub fn is_resident(self) -> bool {
        matches!(self, Ring::Invariant | Ring::Protocol)
    }

    /// Retrieval weight applied after rank fusion (SPEC §7 step 4).
    ///
    /// These are deliberately close to 1. Fused RRF scores live in a narrow band around
    /// `1/61`, so a factor of 2 would not nudge the ranking, it would sort by ring and use
    /// relevance only to break ties — a highly relevant ring-2 block would lose to a barely
    /// related ring-0 one. Trust decides who wins a *contradiction* (§3.2); it does not
    /// decide what the query was about.
    ///
    /// The resident rings get the smallest boost of all, because they are injected into
    /// every session regardless (§3.2). Weighting them up here would count them twice.
    pub fn weight(self) -> f32 {
        match self {
            Ring::Invariant => 1.15,
            Ring::Protocol => 1.10,
            Ring::Knowledge => 1.00,
            Ring::Session => 0.92,
            Ring::External => 0.80,
        }
    }

    /// Directory name inside `notes/`.
    pub fn dir(self) -> &'static str {
        match self {
            Ring::Invariant => "r0",
            Ring::Protocol => "r1",
            Ring::Knowledge => "r2",
            Ring::Session => "r3",
            Ring::External => "r4",
        }
    }
}

impl TryFrom<u8> for Ring {
    type Error = Error;
    fn try_from(v: u8) -> Result<Self> {
        Ok(match v {
            0 => Ring::Invariant,
            1 => Ring::Protocol,
            2 => Ring::Knowledge,
            3 => Ring::Session,
            4 => Ring::External,
            other => return Err(Error::BadRing(other)),
        })
    }
}

impl From<Ring> for u8 {
    fn from(r: Ring) -> u8 {
        r as u8
    }
}

impl fmt::Display for Ring {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "r{}", self.as_u8())
    }
}

/// What kind of thing a note records. Drives nothing in retrieval; it exists so a human
/// and the UI can filter, and so `write` can suggest a template.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum NoteKind {
    Knowledge,
    Bug,
    Lesson,
    Decision,
    Reference,
    Session,
}

/// Result of the write-time PII scan (SPEC §12.4).
///
/// The default is [`PiiState::Unscanned`], not [`PiiState::None`], and the difference is the
/// whole point of the type. An absent `pii:` key means nobody looked; letting that read as
/// "looked, found nothing" would hand the compliance layer a clean bill of health that no
/// scan ever issued. A hand-written note is unscanned until a scan says otherwise.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum PiiState {
    /// No scan has run over this note. The default for anything not written by us.
    #[default]
    Unscanned,
    /// Scanned, nothing found.
    None,
    /// Findings were shown to the operator and accepted.
    Reviewed,
    /// Findings stand unaddressed.
    Flagged,
}

impl PiiState {
    /// True only when a scan actually ran. Callers that gate on "is this note clean" must
    /// use this rather than `!= Flagged`, which would wave through everything unscanned.
    pub fn was_scanned(self) -> bool {
        !matches!(self, PiiState::Unscanned)
    }
}

pub type NoteId = ulid::Ulid;

/// The YAML block at the head of every note (SPEC §3.1).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Frontmatter {
    /// Immutable. Assigned once at creation and never rewritten, so citations survive renames.
    pub id: NoteId,
    /// kebab-case slug, unique within the store. May change.
    pub name: String,
    pub ring: Ring,
    pub kind: NoteKind,
    pub created: jiff::Timestamp,
    pub updated: jiff::Timestamp,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub tags: Vec<String>,
    /// Derived from `[[...]]` in the body and written back by `scan`. Never hand-maintained.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub links: Vec<String>,
    /// Which part of the organisation this note belongs to: a department, a team, a
    /// domain. Optional, because a single-project store does not need one and every
    /// existing note predates the field. Filters recall; never changes ranking.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub bereich: Option<String>,
    /// ISO-8601 duration. Absent means keep indefinitely (SPEC §12.5).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub retention: Option<String>,
    #[serde(default)]
    pub pii: PiiState,
}

/// A note as it exists on disk. The file is authoritative; the index is a cache.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Note {
    pub front: Frontmatter,
    pub body: String,
    pub path: PathBuf,
}

/// A retrievable slice of a note, at most 512 tokens (SPEC §3.3).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Block {
    pub citation: crate::citation::Citation,
    pub note_id: NoteId,
    pub idx: u32,
    pub text: String,
    pub token_count: u32,
}

/// One result from `recall`. Always carries its citation: an answer without one is a bug.
#[derive(Debug, Clone, Serialize)]
pub struct Hit {
    pub citation: String,
    pub note_id: NoteId,
    pub note_name: String,
    pub ring: Ring,
    pub score: f32,
    pub text: String,
}

/// Two hits that disagree. The lower ring wins and both are named (SPEC §7).
#[derive(Debug, Clone, Serialize)]
pub struct Conflict {
    pub winner: String,
    pub loser: String,
    pub reason: String,
}

/// The full answer to a recall, including what could not be checked.
#[derive(Debug, Clone, Serialize)]
pub struct RecallResult {
    pub hits: Vec<Hit>,
    pub conflicts: Vec<Conflict>,
    /// Checks that were skipped and why. Never empty out of politeness: a skipped
    /// contradiction check that says nothing is indistinguishable from one that passed.
    pub caveats: Vec<String>,
}

/// The only reasons bytes may leave this machine (SPEC §12.1). Adding a variant is the
/// deliberate act that adding an outbound path is meant to be.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum EgressPurpose {
    /// Fetching a model artefact, once, on explicit consent.
    ModelDownload,
    /// A request to an inference endpoint on loopback or a private range.
    LocalInference,
    /// Audit rows to a collecting hub, so a team can hold its own compliance evidence in
    /// one place. Rows only: the request carries what happened, never what a note said.
    AuditSync,
    /// A program the user started in a terminal this program opened for them (SPEC §8.3).
    ///
    /// Listed rather than gated, and that distinction is the point. This is the one entry
    /// the gate does not mediate: `permit` is never called for it, because what `ssh` does
    /// is not ours to allow or refuse. It is in the register so that a reader of the
    /// register is not misled about what can leave the machine — a register that quietly
    /// omitted it would be false, and its whole value is that it can be believed.
    Terminal,
}

impl EgressPurpose {
    pub fn describe(self) -> &'static str {
        match self {
            EgressPurpose::Terminal => {
                "whatever you run in a terminal; this program neither mediates nor records it"
            }
            EgressPurpose::ModelDownload => {
                "downloads a model artefact from the configured source, once, after you agree"
            }
            EgressPurpose::AuditSync => concat!(
                "sends this store's audit rows to the hub you enrolled with; rows describe ",
                "what happened, never what a note said"
            ),
            EgressPurpose::LocalInference => {
                "sends note text to the configured inference endpoint on your own network"
            }
        }
    }
}

/// The gate every outbound request passes through (SPEC §12.1).
///
/// The rule is that all network I/O goes through one checked wrapper. That wrapper cannot
/// live in `cyberbrain-policy`, because the crates that actually make requests must not
/// depend on it — so the *seam* lives here and the policy crate implements it, the same
/// inversion used for [`Embedder`]. A crate that wants to make a request must hold one of
/// these and call [`EgressGate::permit`] first; there is no other supported way to build an
/// HTTP client, and CI enforces that by whitelisting only call sites that take a gate.
///
/// `permit` both decides and records. Separating the two would allow a call that was
/// permitted and never audited, which is the shape of the failure the register exists to
/// prevent.
pub trait EgressGate: Send + Sync {
    /// Returns `Ok(())` only if the active profile permits `purpose` towards `destination`.
    /// The attempt is recorded either way — a refusal is the most interesting audit row
    /// there is. `destination` is the full URL or host:port about to be contacted, already
    /// resolved where resolution applies, so the record names what was really reached and
    /// not what the config string claimed.
    fn permit(&self, purpose: EgressPurpose, destination: &str) -> Result<()>;
}

/// A gate that permits nothing. The correct default for any code path that has not been
/// given a real one: failing closed means a forgotten wiring shows up as a refusal in the
/// log rather than as a silent, unaudited request.
#[derive(Debug, Clone, Copy, Default)]
pub struct DenyAllEgress;

impl EgressGate for DenyAllEgress {
    fn permit(&self, purpose: EgressPurpose, destination: &str) -> Result<()> {
        Err(crate::Error::PolicyRefusal {
            profile: "no-gate".into(),
            reason: format!(
                concat!(
                    "{:?} towards {} was refused because no egress gate is wired in; ",
                    "this is a wiring bug, not a configuration choice"
                ),
                purpose, destination
            ),
        })
    }
}

/// Produces vectors. Implemented in `cyberbrain-embed`; consumed by `cyberbrain-index`.
/// Lives here so the index never depends on the embedder's implementation.
pub trait Embedder: Send + Sync {
    /// Vector dimension. Constant for the life of a profile.
    fn dim(&self) -> usize;

    /// Identifies model, dimension and pooling together. Stored alongside the vectors so a
    /// changed model is detected instead of silently degrading search (SPEC §5).
    fn profile_id(&self) -> &str;

    /// L2-normalised vectors, one per input, in input order.
    ///
    /// Degenerate input is the one exception to normalisation (SPEC §6.3): an empty string,
    /// or one whose every token is unknown, yields the **all-zero vector**. Not an error,
    /// because a single empty block must not abort a batch of thousands, and not a NaN,
    /// because a NaN reaching cosine similarity silently poisons every ranking it touches.
    /// A caller scoring vectors must treat an all-zero vector as "no semantic signal" and
    /// skip it rather than divide by its norm.
    fn embed(&self, texts: &[&str]) -> Result<Vec<Vec<f32>>>;
}
