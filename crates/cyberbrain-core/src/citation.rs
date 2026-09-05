//! Citations. Every retrieved statement carries one and it resolves back to the exact
//! block that produced it (SPEC §3.3).
//!
//! Form: `r{ring}-{12 hex}`, e.g. `r2-a91f2c33e1bd`. The hash covers note id, block index
//! and block text, so it is stable across reindexing while the text is unchanged, and it
//! changes when the text does — which is the point. A citation that silently keeps
//! pointing at edited text is worse than one that stops resolving.
//!
//! **Width.** 48 bits, not the 40 this started with. At 100k blocks the birthday collision
//! probability falls from roughly 5e-3 to 2e-5, which is the difference between "will happen
//! to somebody" and "will not". A collision surfaces loudly as a uniqueness violation at
//! index time rather than as wrong data, but a baffling scan failure is still a bad day, and
//! two extra characters in something people paste around is a cheap way to buy it off.

use crate::error::{Error, Result};
use crate::types::{NoteId, Ring};
use serde::{Deserialize, Serialize};
use std::fmt;

const HEX_LEN: usize = 12;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(try_from = "String", into = "String")]
pub struct Citation {
    pub ring: Ring,
    /// 6 bytes rendered as 12 hex characters.
    bytes: [u8; 6],
}

impl Citation {
    pub fn new(ring: Ring, note_id: NoteId, block_idx: u32, block_text: &str) -> Self {
        let mut h = blake3::Hasher::new();
        h.update(&note_id.to_bytes());
        h.update(&block_idx.to_le_bytes());
        h.update(block_text.as_bytes());
        let digest = h.finalize();
        let mut bytes = [0u8; 6];
        bytes.copy_from_slice(&digest.as_bytes()[..6]);
        Self { ring, bytes }
    }

    pub fn hex(&self) -> String {
        self.bytes.iter().map(|b| format!("{b:02x}")).collect()
    }
}

impl fmt::Display for Citation {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}-{}", self.ring, self.hex())
    }
}

impl std::str::FromStr for Citation {
    type Err = Error;

    fn from_str(s: &str) -> Result<Self> {
        let bad = || Error::BadCitation(s.to_string());
        let (ring_part, hex_part) = s.split_once('-').ok_or_else(bad)?;
        let digit = ring_part.strip_prefix('r').ok_or_else(bad)?;
        let ring = Ring::try_from(digit.parse::<u8>().map_err(|_| bad())?)?;
        if hex_part.len() != HEX_LEN || !hex_part.bytes().all(|b| b.is_ascii_hexdigit()) {
            return Err(bad());
        }
        let mut bytes = [0u8; 6];
        for (i, b) in bytes.iter_mut().enumerate() {
            *b = u8::from_str_radix(&hex_part[i * 2..i * 2 + 2], 16).map_err(|_| bad())?;
        }
        Ok(Self { ring, bytes })
    }
}

impl TryFrom<String> for Citation {
    type Error = Error;
    fn try_from(s: String) -> Result<Self> {
        s.parse()
    }
}

impl From<Citation> for String {
    fn from(c: Citation) -> String {
        c.to_string()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::str::FromStr;

    fn id() -> NoteId {
        NoteId::from_string("01ARZ3NDEKTSV4RRFFQ69G5FAV").unwrap()
    }

    #[test]
    fn renders_and_parses_back() {
        let c = Citation::new(Ring::Knowledge, id(), 3, "the text");
        let s = c.to_string();
        assert!(s.starts_with("r2-"), "{s}");
        assert_eq!(s.len(), 3 + HEX_LEN);
        assert_eq!(Citation::from_str(&s).unwrap(), c);
    }

    #[test]
    fn is_stable_for_identical_input() {
        let a = Citation::new(Ring::Session, id(), 0, "same");
        let b = Citation::new(Ring::Session, id(), 0, "same");
        assert_eq!(a, b, "a citation must survive reindexing unchanged");
    }

    #[test]
    fn changes_when_the_text_changes() {
        let a = Citation::new(Ring::Session, id(), 0, "before");
        let b = Citation::new(Ring::Session, id(), 0, "after");
        assert_ne!(a, b, "an edited block must not keep its old citation");
    }

    #[test]
    fn changes_with_block_index() {
        let a = Citation::new(Ring::Session, id(), 0, "same");
        let b = Citation::new(Ring::Session, id(), 1, "same");
        assert_ne!(a, b);
    }

    #[test]
    fn rejects_malformed() {
        for s in [
            "r2a91f2c33e1",     // no separator
            "x2-a91f2c33e1bd",  // no r
            "r9-a91f2c33e1bd",  // ring out of range
            "r2-a91f2c33e1",    // too short
            "r2-a91f2c33e1bde", // too long
            "r2-zzzzzzzzzzzz",  // not hex
            "",
        ] {
            assert!(Citation::from_str(s).is_err(), "{s} should not parse");
        }
    }
}
