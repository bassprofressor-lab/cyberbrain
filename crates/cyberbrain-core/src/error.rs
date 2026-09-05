//! One error type for the whole tree. Exit codes follow SPEC §8.

use std::path::PathBuf;

#[derive(Debug, thiserror::Error)]
pub enum Error {
    #[error("io error at {path}: {source}")]
    Io {
        path: PathBuf,
        #[source]
        source: std::io::Error,
    },

    #[error("{path}: frontmatter is malformed: {reason}")]
    Frontmatter { path: PathBuf, reason: String },

    #[error("{0} is not a valid citation; expected the form r2-a91f2c33e1")]
    BadCitation(String),

    #[error("no note named {0}")]
    NoSuchNote(String),

    #[error("ring {0} does not exist; rings are 0 through 4")]
    BadRing(u8),

    /// Rings 0 and 1 are size-capped so they can be injected unconditionally (SPEC §3.2).
    /// This is raised at write time on purpose: discovering it at read time would mean
    /// silently truncating the invariants an agent is meant to obey.
    /// `ring` names the note being written; the cap itself is over rings 0 and 1 combined,
    /// and `actual` is that combined figure. Counted with the cheap approximation in
    /// `blocks::approx_tokens`, because this is enforced at write time where no tokenizer
    /// exists — so the cap is a guard rail, not an exact accounting.
    #[error(
        "writing to ring {ring} would put the resident rings at ~{actual} tokens, over the          cap of {cap}; shorten a note or move one to ring 2"
    )]
    RingCapExceeded { ring: u8, actual: usize, cap: usize },

    /// The notes tree contradicts itself: two notes with one name, a file whose frontmatter
    /// disagrees with its location, a duplicate id. Distinct from a parse error, because the
    /// individual file is fine and the store as a whole is not.
    #[error("store integrity: {0}")]
    StoreIntegrity(String),

    #[error("index: {0}")]
    Index(String),

    #[error("embedding: {0}")]
    Embed(String),

    /// The stored vectors were produced by a different model than the one configured.
    /// Comparing them would degrade retrieval with no visible error, so we refuse (SPEC §5).
    #[error(
        "stored vectors come from embedding profile {stored} but {configured} is configured; \
         run `cyberbrain scan --full` to reindex, semantic search is disabled until then"
    )]
    EmbeddingProfileMismatch { stored: String, configured: String },

    #[error("inference endpoint: {0}")]
    Llm(String),

    /// A policy refusal. Distinct from an error: the operation was understood and declined.
    /// Maps to exit code 3.
    #[error("refused by policy ({profile}): {reason}")]
    PolicyRefusal { profile: String, reason: String },

    #[error("{0}")]
    Config(String),
}

pub type Result<T> = std::result::Result<T, Error>;

impl Error {
    /// SPEC §8: 0 success, 1 user error, 2 internal error, 3 policy refusal.
    pub fn exit_code(&self) -> i32 {
        match self {
            Error::PolicyRefusal { .. } => 3,
            Error::BadCitation(_)
            | Error::NoSuchNote(_)
            | Error::BadRing(_)
            | Error::RingCapExceeded { .. }
            | Error::Frontmatter { .. }
            | Error::Config(_) => 1,
            _ => 2,
        }
    }
}
