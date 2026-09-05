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
    pub fn weight(self) -> f32 {
        match self {
            Ring::Invariant => 2.0,
            Ring::Protocol => 1.6,
            Ring::Knowledge => 1.0,
            Ring::Session => 0.8,
            Ring::External => 0.5,
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
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum PiiState {
    /// Scanned, nothing found.
    #[default]
    None,
    /// Findings were shown to the operator and accepted.
    Reviewed,
    /// Findings stand unaddressed.
    Flagged,
}

pub type NoteId = ulid::Ulid;

/// The YAML block at the head of every note (SPEC §3.1).
#[derive(Debug, Clone, Serialize, Deserialize)]
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
    /// ISO-8601 duration. Absent means keep indefinitely (SPEC §12.5).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub retention: Option<String>,
    #[serde(default)]
    pub pii: PiiState,
}

/// A note as it exists on disk. The file is authoritative; the index is a cache.
#[derive(Debug, Clone)]
pub struct Note {
    pub front: Frontmatter,
    pub body: String,
    pub path: PathBuf,
}

/// A retrievable slice of a note, at most 512 tokens (SPEC §3.3).
#[derive(Debug, Clone)]
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
}

impl EgressPurpose {
    pub fn describe(self) -> &'static str {
        match self {
            EgressPurpose::ModelDownload => {
                "downloads a model artefact from the configured source, once, after you agree"
            }
            EgressPurpose::LocalInference => {
                "sends note text to the configured inference endpoint on your own network"
            }
        }
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
