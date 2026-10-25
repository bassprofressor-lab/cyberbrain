//! The store's audit key (F2): an Ed25519 key pair that signs new audit rows, signed
//! bundles and erasure reports.
//!
//! # Why a key, when there is already a chain
//!
//! The hash chain shows that a row was changed. It cannot show *who* may have changed it:
//! anybody who can write `audit.db` can also recompute every hash after the row they edited,
//! and the chain then holds again. A signature closes that, for everyone who knows which
//! public key to expect — which is why the fingerprint is printed, put into every signed
//! bundle, and meant to be written down somewhere the store's owner does not control (a
//! contract, a ticket, an auditor's file).
//!
//! What it does not close: whoever can read the private key can sign. The key lives in the
//! store, readable by the owner only, so it raises the bar from "can write a file" to "can
//! read the owner's secrets", and it makes a rewrite after an export visible to anybody
//! holding that export. It is not a hardware anchor and does not pretend to be.
//!
//! # Formats, kept small on purpose
//!
//! - The private key on disk: 64 hex digits (the 32-byte seed) and a newline.
//! - The public key: 64 hex digits.
//! - The fingerprint: the first 32 hex digits of blake3 over the 32 public-key bytes. Short
//!   enough to read over the phone, long enough (128 bits) that nobody makes a second key to
//!   match it.
//! - A signature: 128 hex digits, Ed25519 over a message that starts with a domain tag, so a
//!   signature made for one purpose cannot be replayed as another.

use cyberbrain_core::{Error, Result};
use ed25519_dalek::{Signature, Signer, SigningKey, Verifier, VerifyingKey};

/// Domain tag of a signed audit row. The message is this, then the row's hash in hex.
pub const ROW_DOMAIN: &str = "cyberbrain.audit.row.v2\n";
/// Domain tag of a signed bundle. The message is this, then every byte before the footer.
pub const BUNDLE_DOMAIN: &str = "cyberbrain.audit.export.v2\n";
/// Domain tag of a signed erasure report. The message is this, then the canonical JSON.
pub const ERASURE_DOMAIN: &str = "cyberbrain.erasure.report.v1\n";

/// A store's signing key. Never printed: `Debug` shows the fingerprint only.
#[derive(Clone)]
pub struct AuditKey {
    key: SigningKey,
}

impl std::fmt::Debug for AuditKey {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "AuditKey({})", self.fingerprint())
    }
}

impl AuditKey {
    /// A new key from the operating system's randomness.
    pub fn generate() -> Result<Self> {
        let mut seed = [0u8; 32];
        getrandom::fill(&mut seed)
            .map_err(|e| Error::Config(format!("no randomness available to make a key: {e}")))?;
        Ok(Self {
            key: SigningKey::from_bytes(&seed),
        })
    }

    /// Read the private key as it is stored: 64 hex digits, surrounding whitespace ignored.
    pub fn from_hex(text: &str) -> Result<Self> {
        let bytes = decode_hex(text.trim())
            .ok_or_else(|| Error::Config("the audit key is not hex".into()))?;
        let seed: [u8; 32] = bytes
            .try_into()
            .map_err(|_| Error::Config("the audit key is not 32 bytes".into()))?;
        Ok(Self {
            key: SigningKey::from_bytes(&seed),
        })
    }

    /// The private key in the form [`from_hex`](Self::from_hex) reads. Only for writing it
    /// to its file.
    pub fn secret_hex(&self) -> String {
        to_hex(self.key.as_bytes())
    }

    pub fn public_hex(&self) -> String {
        to_hex(self.key.verifying_key().as_bytes())
    }

    pub fn fingerprint(&self) -> String {
        fingerprint_bytes(self.key.verifying_key().as_bytes())
    }

    /// Sign `domain` followed by `body`; the signature in hex.
    pub fn sign(&self, domain: &str, body: &[u8]) -> String {
        let mut msg = Vec::with_capacity(domain.len() + body.len());
        msg.extend_from_slice(domain.as_bytes());
        msg.extend_from_slice(body);
        to_hex(&self.key.sign(&msg).to_bytes())
    }
}

/// The fingerprint of a public key given in hex.
pub fn fingerprint_of(public_hex: &str) -> Result<String> {
    Ok(fingerprint_bytes(verifying_key(public_hex)?.as_bytes()))
}

fn fingerprint_bytes(public: &[u8; 32]) -> String {
    blake3::hash(public).to_hex()[..32].to_string()
}

/// Whether `expected` names this public key: either its fingerprint or the whole key, in
/// any case, with `:` or spaces between groups allowed, because people copy it from a PDF.
pub fn key_matches(expected: &str, public_hex: &str) -> bool {
    let want: String = expected
        .chars()
        .filter(|c| !c.is_whitespace() && *c != ':')
        .collect::<String>()
        .to_ascii_lowercase();
    if want.eq_ignore_ascii_case(public_hex) {
        return true;
    }
    fingerprint_of(public_hex).is_ok_and(|fp| fp == want)
}

/// Check a signature made by [`AuditKey::sign`].
pub fn verify_signature(public_hex: &str, domain: &str, body: &[u8], sig_hex: &str) -> Result<()> {
    let key = verifying_key(public_hex)?;
    let sig_bytes =
        decode_hex(sig_hex).ok_or_else(|| Error::Index("the signature is not hex".into()))?;
    let sig_arr: [u8; 64] = sig_bytes
        .try_into()
        .map_err(|_| Error::Index("the signature is not 64 bytes".into()))?;
    let mut msg = Vec::with_capacity(domain.len() + body.len());
    msg.extend_from_slice(domain.as_bytes());
    msg.extend_from_slice(body);
    key.verify(&msg, &Signature::from_bytes(&sig_arr))
        .map_err(|_| Error::Index("the signature does not match".into()))
}

fn verifying_key(public_hex: &str) -> Result<VerifyingKey> {
    let bytes =
        decode_hex(public_hex).ok_or_else(|| Error::Index("the public key is not hex".into()))?;
    let arr: [u8; 32] = bytes
        .try_into()
        .map_err(|_| Error::Index("the public key is not 32 bytes".into()))?;
    VerifyingKey::from_bytes(&arr)
        .map_err(|e| Error::Index(format!("not a valid Ed25519 public key: {e}")))
}

fn to_hex(bytes: &[u8]) -> String {
    use std::fmt::Write;
    let mut s = String::with_capacity(bytes.len() * 2);
    for b in bytes {
        let _ = write!(s, "{b:02x}");
    }
    s
}

fn decode_hex(s: &str) -> Option<Vec<u8>> {
    if !s.len().is_multiple_of(2) || !s.is_ascii() {
        return None;
    }
    (0..s.len())
        .step_by(2)
        .map(|i| u8::from_str_radix(&s[i..i + 2], 16).ok())
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_key_survives_its_own_file_format() {
        let k = AuditKey::generate().unwrap();
        let back = AuditKey::from_hex(&format!("{}\n", k.secret_hex())).unwrap();
        assert_eq!(back.public_hex(), k.public_hex());
        assert_eq!(k.fingerprint().len(), 32);
        assert!(!format!("{k:?}").contains(&k.secret_hex()), "Debug leaks");
    }

    #[test]
    fn a_signature_holds_for_its_domain_only() {
        let k = AuditKey::generate().unwrap();
        let sig = k.sign(ROW_DOMAIN, b"abc");
        verify_signature(&k.public_hex(), ROW_DOMAIN, b"abc", &sig).unwrap();
        assert!(verify_signature(&k.public_hex(), ROW_DOMAIN, b"abd", &sig).is_err());
        assert!(verify_signature(&k.public_hex(), BUNDLE_DOMAIN, b"abc", &sig).is_err());
        let other = AuditKey::generate().unwrap();
        assert!(verify_signature(&other.public_hex(), ROW_DOMAIN, b"abc", &sig).is_err());
    }

    #[test]
    fn a_key_is_named_by_fingerprint_or_in_full_however_it_was_copied() {
        let k = AuditKey::generate().unwrap();
        let fp = k.fingerprint();
        assert!(key_matches(&fp, &k.public_hex()));
        assert!(key_matches(&fp.to_uppercase(), &k.public_hex()));
        let grouped: Vec<String> = fp
            .as_bytes()
            .chunks(4)
            .map(|c| String::from_utf8(c.to_vec()).unwrap())
            .collect();
        assert!(key_matches(&grouped.join(":"), &k.public_hex()));
        assert!(key_matches(&k.public_hex(), &k.public_hex()));
        let other = AuditKey::generate().unwrap();
        assert!(!key_matches(&other.fingerprint(), &k.public_hex()));
    }
}
