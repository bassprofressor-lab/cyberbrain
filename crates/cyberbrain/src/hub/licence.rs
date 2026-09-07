//! Licences: what lets a machine be a hub, and for how many devices.
//!
//! # Signed, and checked offline
//!
//! A licence is one line of JSON and one line of signature. The public key is compiled in;
//! the private half never leaves the issuer. A hub therefore validates a licence with
//! nothing but itself — no call home, which matters because the networks this runs on are
//! often deliberately closed, and because a licence server is a way for someone else's
//! outage to stop your compliance evidence.
//!
//! # What expiry does, and does not do
//!
//! From 30 days out, everything that displays state says so. After the end date the hub
//! **stops accepting new rows and nothing else**: the record stays readable, exportable and
//! intact, devices keep working locally, and clients buffer. Renewing takes the buffered
//! rows and the chain closes without a gap.
//!
//! That asymmetry is deliberate. Evidence somebody paid to collect is theirs; software that
//! holds it hostage at renewal time is not a product, it is a hostage situation, and the one
//! moment it would bite is an audit.

use cyberbrain_core::{Error, Result};
use ed25519_dalek::{Signature, SigningKey, Verifier, VerifyingKey};
use serde::{Deserialize, Serialize};

/// Days before the end date at which warnings start.
pub const WARN_DAYS: i64 = 30;

/// The issuer's public key, hex, compiled in.
///
/// Replacing this is replacing the trust anchor: a binary built with a different key accepts
/// a different issuer's licences and no longer accepts this one's. It is deliberately a
/// constant rather than a file next to the binary, because a trust anchor somebody can swap
/// without rebuilding is not one.
pub const ISSUER_PUBLIC_KEY: &str =
    "afe47e2521a88be62cd385008ab2604c99b92ceaf641131ff8383ee743c19da3";

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Licence {
    /// Format version, so an older hub refuses a newer licence instead of guessing at it.
    pub version: u32,
    pub id: String,
    /// Who it is for. Shown in the hub's own output, never checked against anything —
    /// a licence tied to a hostname breaks when the machine is replaced at 3am.
    pub customer: String,
    /// Devices that may be registered. Seats are devices, not people.
    pub seats: usize,
    /// RFC 3339, inclusive.
    pub valid_from: String,
    pub valid_until: String,
    pub issued_at: String,
}

/// A licence and its signature, as the file holds them.
#[derive(Debug, Clone, PartialEq)]
pub struct SignedLicence {
    pub licence: Licence,
    /// The exact bytes that were signed. Kept rather than re-serialised: a round trip
    /// through a struct is a chance for the two to differ, and then a valid licence fails.
    canonical: String,
    signature: String,
}

impl SignedLicence {
    pub fn licence(&self) -> &Licence {
        &self.licence
    }

    /// Days until the end date, negative once it has passed.
    pub fn days_left(&self, now: jiff::Timestamp) -> i64 {
        let Ok(end) = self.licence.valid_until.parse::<jiff::Timestamp>() else {
            return -1;
        };
        (end.as_second() - now.as_second()) / 86_400
    }

    pub fn expired(&self, now: jiff::Timestamp) -> bool {
        match self.licence.valid_until.parse::<jiff::Timestamp>() {
            Ok(end) => now > end,
            // A licence whose end date cannot be read is not a licence to keep collecting.
            Err(_) => true,
        }
    }

    pub fn not_yet_valid(&self, now: jiff::Timestamp) -> bool {
        match self.licence.valid_from.parse::<jiff::Timestamp>() {
            Ok(start) => now < start,
            Err(_) => true,
        }
    }

    /// The warning a person should see, if any. `None` when there is nothing to say.
    pub fn warning(&self, now: jiff::Timestamp) -> Option<String> {
        if self.expired(now) {
            return Some(format!(
                "the licence for {} ended on {}. New rows are no longer accepted; the record \
                 stays readable and exportable, and clients keep buffering. Renewing takes \
                 what they held.",
                self.licence.customer, self.licence.valid_until
            ));
        }
        let days = self.days_left(now);
        if days <= WARN_DAYS {
            return Some(format!(
                "the licence for {} ends on {} — {} day(s) left. After that the hub stops \
                 accepting rows; nothing is deleted.",
                self.licence.customer, self.licence.valid_until, days
            ));
        }
        None
    }

    pub fn render(&self) -> String {
        format!("{}\n{}\n", self.canonical, self.signature)
    }
}

fn key_from_hex(hex: &str) -> Result<VerifyingKey> {
    let bytes = decode_hex(hex).ok_or_else(|| Error::Config("issuer key is not hex".into()))?;
    let arr: [u8; 32] = bytes
        .try_into()
        .map_err(|_| Error::Config("issuer key is not 32 bytes".into()))?;
    VerifyingKey::from_bytes(&arr)
        .map_err(|e| Error::Config(format!("issuer key is not a valid ed25519 key: {e}")))
}

fn decode_hex(s: &str) -> Option<Vec<u8>> {
    if !s.len().is_multiple_of(2) {
        return None;
    }
    (0..s.len())
        .step_by(2)
        .map(|i| u8::from_str_radix(&s[i..i + 2], 16).ok())
        .collect()
}

fn to_hex(bytes: &[u8]) -> String {
    bytes.iter().map(|b| format!("{b:02x}")).collect()
}

/// Read a licence file and check its signature against the compiled-in key.
pub fn parse(text: &str) -> Result<SignedLicence> {
    parse_with_key(text, ISSUER_PUBLIC_KEY)
}

pub fn parse_with_key(text: &str, issuer_hex: &str) -> Result<SignedLicence> {
    let bad = |m: String| Error::Config(format!("licence: {m}"));
    let mut lines = text.lines().filter(|l| !l.trim().is_empty());
    let canonical = lines
        .next()
        .ok_or_else(|| bad("file is empty".into()))?
        .to_string();
    let signature = lines
        .next()
        .ok_or_else(|| bad("no signature line".into()))?
        .trim()
        .to_string();

    let licence: Licence = serde_json::from_str(&canonical)
        .map_err(|e| bad(format!("first line is not a licence: {e}")))?;
    if licence.version != 1 {
        return Err(bad(format!(
            "format version {} is not 1; this hub cannot read it",
            licence.version
        )));
    }

    let key = key_from_hex(issuer_hex)?;
    let sig_bytes = decode_hex(&signature).ok_or_else(|| bad("signature is not hex".into()))?;
    let sig_arr: [u8; 64] = sig_bytes
        .try_into()
        .map_err(|_| bad("signature is not 64 bytes".into()))?;
    let sig = Signature::from_bytes(&sig_arr);
    key.verify(canonical.as_bytes(), &sig).map_err(|_| {
        bad("signature does not match; this licence was not issued for this product".into())
    })?;

    Ok(SignedLicence {
        licence,
        canonical,
        signature,
    })
}

/// Sign a licence. Only the issuer can do this, and only with the private key.
pub fn issue(licence: &Licence, signing_key_hex: &str) -> Result<SignedLicence> {
    let bytes = decode_hex(signing_key_hex)
        .ok_or_else(|| Error::Config("signing key is not hex".into()))?;
    let arr: [u8; 32] = bytes
        .try_into()
        .map_err(|_| Error::Config("signing key is not 32 bytes".into()))?;
    let key = SigningKey::from_bytes(&arr);

    // Serialised once, and that string is what gets signed and written. Signing a
    // re-serialisation would make the check depend on two libraries agreeing about key
    // order forever.
    let canonical = serde_json::to_string(licence)
        .map_err(|e| Error::Config(format!("licence does not serialise: {e}")))?;
    let signature = {
        use ed25519_dalek::Signer;
        to_hex(&key.sign(canonical.as_bytes()).to_bytes())
    };
    Ok(SignedLicence {
        licence: licence.clone(),
        canonical,
        signature,
    })
}

/// Generate a key pair: (private, public), both hex.
///
/// The private half is handed straight to the caller and never written by this program. What
/// happens to it after that is an operational decision, and one this code should not quietly
/// make on somebody's behalf.
pub fn generate_key() -> Result<(String, String)> {
    let mut seed = [0u8; 32];
    getrandom::fill(&mut seed)
        .map_err(|e| Error::Config(format!("no randomness available to make a key: {e}")))?;
    let key = SigningKey::from_bytes(&seed);
    Ok((
        to_hex(key.as_bytes()),
        to_hex(key.verifying_key().as_bytes()),
    ))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn key_pair() -> (String, String) {
        generate_key().expect("this machine has randomness")
    }

    fn licence(seats: usize, from: &str, until: &str) -> Licence {
        Licence {
            version: 1,
            id: "lic_test".into(),
            customer: "Beispiel GmbH".into(),
            seats,
            valid_from: from.into(),
            valid_until: until.into(),
            issued_at: "2026-01-01T00:00:00Z".into(),
        }
    }

    #[test]
    fn a_signed_licence_verifies_and_a_touched_one_does_not() {
        let (private, public) = key_pair();
        let signed = issue(
            &licence(5, "2026-01-01T00:00:00Z", "2027-01-01T00:00:00Z"),
            &private,
        )
        .unwrap();
        let text = signed.render();
        let back = parse_with_key(&text, &public).unwrap();
        assert_eq!(back.licence().seats, 5);

        // The whole point: more seats than were paid for must not verify.
        let tampered = text.replace("\"seats\":5", "\"seats\":500");
        let err = parse_with_key(&tampered, &public).unwrap_err().to_string();
        assert!(err.contains("signature does not match"), "{err}");
    }

    #[test]
    fn another_issuers_licence_is_not_accepted() {
        let (private, _) = key_pair();
        let (_, other_public) = key_pair();
        let signed = issue(
            &licence(5, "2026-01-01T00:00:00Z", "2027-01-01T00:00:00Z"),
            &private,
        )
        .unwrap();
        assert!(parse_with_key(&signed.render(), &other_public).is_err());
    }

    #[test]
    fn expiry_is_a_date_not_a_grace_period() {
        let (private, public) = key_pair();
        let signed = issue(
            &licence(5, "2026-01-01T00:00:00Z", "2026-06-30T23:59:59Z"),
            &private,
        )
        .unwrap();
        let l = parse_with_key(&signed.render(), &public).unwrap();

        let before: jiff::Timestamp = "2026-06-30T12:00:00Z".parse().unwrap();
        let after: jiff::Timestamp = "2026-07-01T00:00:01Z".parse().unwrap();
        assert!(!l.expired(before));
        assert!(l.expired(after));
    }

    #[test]
    fn the_warning_starts_thirty_days_out_and_says_what_happens() {
        let (private, public) = key_pair();
        let signed = issue(
            &licence(5, "2026-01-01T00:00:00Z", "2026-07-01T00:00:00Z"),
            &private,
        )
        .unwrap();
        let l = parse_with_key(&signed.render(), &public).unwrap();

        let quiet: jiff::Timestamp = "2026-05-01T00:00:00Z".parse().unwrap();
        assert_eq!(l.warning(quiet), None, "no nagging two months out");

        let warned: jiff::Timestamp = "2026-06-10T00:00:00Z".parse().unwrap();
        let w = l.warning(warned).expect("30 days out there is a warning");
        assert!(w.contains("21 day(s) left"), "{w}");
        assert!(w.contains("nothing is deleted"), "{w}");

        let over: jiff::Timestamp = "2026-07-02T00:00:00Z".parse().unwrap();
        let w = l.warning(over).expect("after the end there is a message");
        assert!(w.contains("no longer accepted"), "{w}");
        assert!(w.contains("stays readable and exportable"), "{w}");
    }

    #[test]
    fn a_licence_that_has_not_started_yet_is_not_valid_either() {
        let (private, public) = key_pair();
        let signed = issue(
            &licence(5, "2027-01-01T00:00:00Z", "2028-01-01T00:00:00Z"),
            &private,
        )
        .unwrap();
        let l = parse_with_key(&signed.render(), &public).unwrap();
        let now: jiff::Timestamp = "2026-09-07T00:00:00Z".parse().unwrap();
        assert!(l.not_yet_valid(now));
        assert!(!l.expired(now));
    }

    #[test]
    fn an_unreadable_end_date_counts_as_expired() {
        // Fail closed: a licence nobody can date is not a licence to keep collecting. The
        // opposite default would make "valid_until": "soon" an unlimited licence.
        let (private, public) = key_pair();
        let mut l = licence(5, "2026-01-01T00:00:00Z", "whenever");
        l.valid_until = "whenever".into();
        let signed = issue(&l, &private).unwrap();
        let parsed = parse_with_key(&signed.render(), &public).unwrap();
        assert!(parsed.expired("2026-01-02T00:00:00Z".parse().unwrap()));
    }

    #[test]
    fn a_file_that_is_not_a_licence_says_so_rather_than_crashing() {
        let (_, public) = key_pair();
        for text in ["", "not json\nsig\n", "{\"version\":1}\n", "{}\nzz\n"] {
            assert!(parse_with_key(text, &public).is_err(), "{text:?} parsed");
        }
    }
}
