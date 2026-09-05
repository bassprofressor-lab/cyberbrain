//! Write-time PII detection (SPEC §12.4).
//!
//! # This is a heuristic. Read that sentence twice.
//!
//! The scanner looks for the *shapes* of email addresses, IPv4 and IPv6 addresses,
//! API-key-like strings, IBANs and phone numbers. It will miss things (a name is not a
//! shape; a phone number written "oh three seven one" is not a shape) and it will flag
//! things that are not personal data (a documentation address, a loopback IP). Every
//! finding carries a [`Confidence`] and a one-line `why`, and the caller is expected to
//! show them to a human. **It is a seatbelt, not a guarantee**, and nothing built on top of
//! it may claim that a note "contains no personal data" because this scan came back empty.
//!
//! What it does well: it catches the copy-paste accidents that actually happen in an
//! engineering notebook — a pasted curl command with a bearer token, a log line with a
//! customer's IP, an invoice snippet with an IBAN.
//!
//! # Offsets
//!
//! `start..end` are **byte** offsets into the scanned string, half-open, always on char
//! boundaries. A UI highlights `&text[start..end]`.
//!
//! # Hot path
//!
//! The regexes are compiled lazily on first use, so a process that never scans (profile
//! `off`, or a hook that only reads) never pays for them.

use crate::Confidence;
use regex::Regex;
use serde::{Deserialize, Serialize};
use std::fmt;
use std::net::{Ipv4Addr, Ipv6Addr};
use std::sync::LazyLock;

/// The sentence every caller-facing surface must carry.
pub const DISCLAIMER: &str = "PII detection is heuristic: it finds the shapes of emails, IP \
    addresses, API keys, IBANs and phone numbers. It misses what has no shape and flags some \
    things that are harmless. It is a seatbelt, not a guarantee.";

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum PiiKind {
    Email,
    Ipv4,
    Ipv6,
    ApiKey,
    Iban,
    Phone,
}

impl PiiKind {
    pub const ALL: [PiiKind; 6] = [
        PiiKind::Email,
        PiiKind::ApiKey,
        PiiKind::Iban,
        PiiKind::Ipv6,
        PiiKind::Ipv4,
        PiiKind::Phone,
    ];

    pub fn as_str(self) -> &'static str {
        match self {
            PiiKind::Email => "email",
            PiiKind::Ipv4 => "ipv4",
            PiiKind::Ipv6 => "ipv6",
            PiiKind::ApiKey => "api-key",
            PiiKind::Iban => "iban",
            PiiKind::Phone => "phone",
        }
    }

    /// When two detections overlap, the lower number wins. An IBAN's digit groups look
    /// like phone numbers; an assignment value may contain an email.
    fn priority(self) -> u8 {
        match self {
            PiiKind::Email => 0,
            PiiKind::ApiKey => 1,
            PiiKind::Iban => 2,
            PiiKind::Ipv6 => 3,
            PiiKind::Ipv4 => 4,
            PiiKind::Phone => 5,
        }
    }

    pub fn placeholder(self) -> &'static str {
        match self {
            PiiKind::Email => "[redacted:email]",
            PiiKind::Ipv4 => "[redacted:ipv4]",
            PiiKind::Ipv6 => "[redacted:ipv6]",
            PiiKind::ApiKey => "[redacted:api-key]",
            PiiKind::Iban => "[redacted:iban]",
            PiiKind::Phone => "[redacted:phone]",
        }
    }
}

impl fmt::Display for PiiKind {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}

/// One detection. `matched` is included for the UI; **it must never be written to the
/// audit log** — see [`Finding::audit_view`].
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Finding {
    pub kind: PiiKind,
    /// Byte offset, inclusive.
    pub start: usize,
    /// Byte offset, exclusive.
    pub end: usize,
    pub matched: String,
    pub confidence: Confidence,
    /// Why the detector believes (or half-believes) this.
    pub why: &'static str,
}

impl Finding {
    /// The finding without the matched text: kind, offsets, confidence. Safe to log.
    pub fn audit_view(&self) -> serde_json::Value {
        serde_json::json!({
            "kind": self.kind,
            "start": self.start,
            "end": self.end,
            "confidence": self.confidence,
        })
    }
}

/// Scan for every kind. Findings are sorted by `start` and do not overlap.
pub fn scan(text: &str) -> Vec<Finding> {
    scan_kinds(text, &PiiKind::ALL)
}

/// Scan for a subset of kinds.
pub fn scan_kinds(text: &str, kinds: &[PiiKind]) -> Vec<Finding> {
    if text.is_empty() {
        return Vec::new();
    }
    let mut raw: Vec<Finding> = Vec::new();
    for k in kinds {
        match k {
            PiiKind::Email => find_emails(text, &mut raw),
            PiiKind::Ipv4 => find_ipv4(text, &mut raw),
            PiiKind::Ipv6 => find_ipv6(text, &mut raw),
            PiiKind::ApiKey => find_api_keys(text, &mut raw),
            PiiKind::Iban => find_ibans(text, &mut raw),
            PiiKind::Phone => find_phones(text, &mut raw),
        }
    }
    resolve_overlaps(raw)
}

/// Replace every finding with its kind's placeholder. `findings` must come from a scan of
/// this same `text` (offsets are trusted). Returns the new text and how many were replaced.
pub fn redact(text: &str, findings: &[Finding]) -> (String, usize) {
    let mut sorted: Vec<&Finding> = findings.iter().collect();
    sorted.sort_by_key(|f| f.start);
    let mut out = String::with_capacity(text.len());
    let mut cursor = 0;
    let mut n = 0;
    for f in sorted {
        if f.start < cursor
            || f.end > text.len()
            || !text.is_char_boundary(f.start)
            || !text.is_char_boundary(f.end)
        {
            // Overlapping or stale offsets: skip rather than corrupt the text. The caller
            // rescans after redaction, so a skipped finding resurfaces instead of vanishing.
            continue;
        }
        out.push_str(&text[cursor..f.start]);
        out.push_str(f.kind.placeholder());
        cursor = f.end;
        n += 1;
    }
    out.push_str(&text[cursor..]);
    (out, n)
}

fn resolve_overlaps(mut raw: Vec<Finding>) -> Vec<Finding> {
    // Higher-priority kinds claim their ranges first; within a kind, earlier and longer wins.
    raw.sort_by(|a, b| {
        a.kind
            .priority()
            .cmp(&b.kind.priority())
            .then(a.start.cmp(&b.start))
            .then(b.end.cmp(&a.end))
    });
    let mut accepted: Vec<Finding> = Vec::with_capacity(raw.len());
    for f in raw {
        let overlaps = accepted.iter().any(|a| f.start < a.end && a.start < f.end);
        if !overlaps {
            accepted.push(f);
        }
    }
    accepted.sort_by_key(|f| f.start);
    accepted
}

// ---- boundary helpers ----

fn prev_char(text: &str, at: usize) -> Option<char> {
    text[..at].chars().next_back()
}

fn next_char(text: &str, at: usize) -> Option<char> {
    text[at..].chars().next()
}

fn is_word(c: Option<char>) -> bool {
    c.is_some_and(|c| c.is_alphanumeric() || c == '_')
}

/// True when the match is followed by `.` and another digit — a longer dotted number.
fn continues_dotted(text: &str, end: usize) -> bool {
    next_char(text, end) == Some('.')
        && text[end + 1..]
            .chars()
            .next()
            .is_some_and(|c| c.is_ascii_digit())
}

fn push(
    out: &mut Vec<Finding>,
    kind: PiiKind,
    text: &str,
    start: usize,
    end: usize,
    confidence: Confidence,
    why: &'static str,
) {
    out.push(Finding {
        kind,
        start,
        end,
        matched: text[start..end].to_string(),
        confidence,
        why,
    });
}

// ---- email ----

static EMAIL: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(
        r"(?i)[a-z0-9][a-z0-9._%+'-]*@[a-z0-9](?:[a-z0-9-]*[a-z0-9])?(?:\.[a-z0-9](?:[a-z0-9-]*[a-z0-9])?)*\.[a-z]{2,24}\b",
    )
    .unwrap()
});

fn find_emails(text: &str, out: &mut Vec<Finding>) {
    for m in EMAIL.find_iter(text) {
        if is_word(prev_char(text, m.start())) {
            continue;
        }
        let s = m.as_str();
        let (local, domain) = s.split_once('@').unwrap();
        let domain_l = domain.to_ascii_lowercase();
        let local_l = local.to_ascii_lowercase();
        let (conf, why) = if domain_l == "example.com"
            || domain_l == "example.org"
            || domain_l == "example.net"
            || domain_l.ends_with(".example")
            || domain_l.ends_with(".invalid")
            || domain_l.ends_with(".test")
        {
            (
                Confidence::Low,
                "email shape, but a documentation domain (RFC 2606)",
            )
        } else if local_l == "git" {
            (
                Confidence::Low,
                "email shape, but git@host is usually an SSH remote",
            )
        } else if local_l == "noreply" || local_l == "no-reply" || local_l.starts_with("noreply") {
            (Confidence::Low, "email shape, but a no-reply sender")
        } else {
            (
                Confidence::High,
                "email address shape with a real-looking domain",
            )
        };
        push(out, PiiKind::Email, text, m.start(), m.end(), conf, why);
    }
}

// ---- IPv4 ----

static IPV4: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(
        r"\b(?:25[0-5]|2[0-4]\d|1\d\d|[1-9]?\d)(?:\.(?:25[0-5]|2[0-4]\d|1\d\d|[1-9]?\d)){3}\b",
    )
    .unwrap()
});

fn find_ipv4(text: &str, out: &mut Vec<Finding>) {
    for m in IPV4.find_iter(text) {
        if prev_char(text, m.start()) == Some('.') || continues_dotted(text, m.end()) {
            continue; // part of a longer dotted number such as 1.2.3.4.5
        }
        let Ok(ip) = m.as_str().parse::<Ipv4Addr>() else {
            continue;
        };
        let (conf, why) = ipv4_confidence(ip);
        push(out, PiiKind::Ipv4, text, m.start(), m.end(), conf, why);
    }
}

fn ipv4_confidence(ip: Ipv4Addr) -> (Confidence, &'static str) {
    if ip.is_loopback() || ip.is_unspecified() || ip.is_broadcast() {
        (
            Confidence::Low,
            "IPv4 shape, but loopback/unspecified: not about a person",
        )
    } else if ip.is_private() || ip.is_link_local() {
        (
            Confidence::Low,
            "IPv4 shape on a private range: usually infrastructure, rarely a person",
        )
    } else if ip.is_documentation() {
        (
            Confidence::Low,
            "IPv4 shape on a documentation range (RFC 5737)",
        )
    } else {
        (
            Confidence::High,
            "public IPv4 address; IP addresses can be personal data",
        )
    }
}

// ---- IPv6 ----

static IPV6: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"[0-9A-Fa-f]{0,4}(?::[0-9A-Fa-f]{0,4}){2,7}(?:(?:\.\d{1,3}){3})?").unwrap()
});

fn find_ipv6(text: &str, out: &mut Vec<Finding>) {
    for m in IPV6.find_iter(text) {
        let prev = prev_char(text, m.start());
        let next = next_char(text, m.end());
        if is_word(prev) || prev == Some(':') || is_word(next) || next == Some(':') {
            continue;
        }
        let s = m.as_str();
        if s == "::" {
            continue;
        }
        let Ok(ip) = s.parse::<Ipv6Addr>() else {
            continue;
        };
        let hex_digits = s.chars().filter(|c| c.is_ascii_hexdigit()).count();
        let has_decimal = s.chars().any(|c| c.is_ascii_digit());
        // `a::b`, `fee::add`, `dead::beef` parse as addresses but are far more often Rust
        // paths or prose. Real addresses carry digits and more than a couple of them.
        if !has_decimal || (hex_digits < 4 && s != "::1") {
            continue;
        }
        let (conf, why) = if let Some(v4) = ip.to_ipv4_mapped() {
            ipv4_confidence(v4)
        } else if ip.is_loopback() || ip.is_unspecified() {
            (
                Confidence::Low,
                "IPv6 shape, but loopback/unspecified: not about a person",
            )
        } else if ip.is_unique_local() || ip.is_unicast_link_local() {
            (
                Confidence::Low,
                "IPv6 shape on a local range: usually infrastructure",
            )
        } else if (ip.segments()[0] == 0x2001) && (ip.segments()[1] == 0x0db8) {
            (
                Confidence::Low,
                "IPv6 shape on the documentation prefix 2001:db8::/32",
            )
        } else {
            (
                Confidence::High,
                "global IPv6 address; IP addresses can be personal data",
            )
        };
        push(out, PiiKind::Ipv6, text, m.start(), m.end(), conf, why);
    }
}

// ---- API keys ----

static KNOWN_KEY: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(
        r"(?x)
        \bsk-(?:ant-|proj-|or-v1-|svcacct-)?[A-Za-z0-9_-]{20,}
        | \b(?:ghp|gho|ghu|ghs|ghr)_[A-Za-z0-9]{36}\b
        | \bgithub_pat_[A-Za-z0-9_]{22,}\b
        | \bglpat-[A-Za-z0-9_-]{20,}\b
        | \bxox[abprs]-[A-Za-z0-9-]{10,}\b
        | \bAKIA[0-9A-Z]{16}\b
        | \bAIza[0-9A-Za-z_-]{35}\b
        | \b(?:sk|rk)_(?:live|test)_[0-9A-Za-z]{20,}\b
        | \bSG\.[A-Za-z0-9_-]{22}\.[A-Za-z0-9_-]{43}\b
        | \bhf_[A-Za-z0-9]{30,}\b
        | \bnpm_[A-Za-z0-9]{36}\b
        | \bpypi-AgEIcHlwaS5vcmc[A-Za-z0-9_-]{20,}
        | \bdop_v1_[a-f0-9]{64}\b
        | \bxai-[A-Za-z0-9]{20,}\b
        | \bgsk_[A-Za-z0-9]{20,}\b
        | \bshpat_[a-fA-F0-9]{32}\b
        | \beyJ[A-Za-z0-9_-]{8,}\.eyJ[A-Za-z0-9_-]{8,}\.[A-Za-z0-9_-]{8,}\b
        ",
    )
    .unwrap()
});

static PEM_BEGIN: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"-----BEGIN (?:[A-Z]+ )*PRIVATE KEY-----").unwrap());
static PEM_END: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"-----END (?:[A-Z]+ )*PRIVATE KEY-----").unwrap());

static ASSIGNED_SECRET: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(
        r#"(?ix)
        \b(?:api[_-]?key|apikey|secret[_-]?key|client[_-]?secret|secret|access[_-]?token
           |auth[_-]?token|refresh[_-]?token|token|password|passwd|pwd|private[_-]?key
           |fernet[_-]?key|encryption[_-]?key|signing[_-]?key|api[_-]?secret)
        \b \s* [:=] \s* ["']? ([A-Za-z0-9_\-./+=]{12,})
        "#,
    )
    .unwrap()
});

static BEARER: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"(?i)\bbearer\s+([A-Za-z0-9_\-./+=]{16,})").unwrap());

static LONG_TOKEN: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"[A-Za-z0-9_\-+=]{32,}").unwrap());

fn find_api_keys(text: &str, out: &mut Vec<Finding>) {
    for m in KNOWN_KEY.find_iter(text) {
        push(
            out,
            PiiKind::ApiKey,
            text,
            m.start(),
            m.end(),
            Confidence::High,
            "matches a known vendor key prefix",
        );
    }
    for m in PEM_BEGIN.find_iter(text) {
        let end = PEM_END
            .find_at(text, m.end())
            .map(|e| e.end())
            .unwrap_or(m.end());
        push(
            out,
            PiiKind::ApiKey,
            text,
            m.start(),
            end,
            Confidence::High,
            "PEM private key block",
        );
    }
    for c in ASSIGNED_SECRET.captures_iter(text) {
        let v = c.get(1).unwrap();
        if looks_like_placeholder(v.as_str()) {
            continue;
        }
        push(
            out,
            PiiKind::ApiKey,
            text,
            v.start(),
            v.end(),
            Confidence::High,
            "value assigned to a key/secret/token/password name",
        );
    }
    for c in BEARER.captures_iter(text) {
        let v = c.get(1).unwrap();
        if looks_like_placeholder(v.as_str()) {
            continue;
        }
        push(
            out,
            PiiKind::ApiKey,
            text,
            v.start(),
            v.end(),
            Confidence::High,
            "bearer token",
        );
    }
    for m in LONG_TOKEN.find_iter(text) {
        if is_word(prev_char(text, m.start())) || is_word(next_char(text, m.end())) {
            continue;
        }
        if looks_like_secret(m.as_str()) {
            push(
                out,
                PiiKind::ApiKey,
                text,
                m.start(),
                m.end(),
                Confidence::Medium,
                "long high-entropy token with no known prefix",
            );
        }
    }
}

fn looks_like_placeholder(v: &str) -> bool {
    let l = v.to_ascii_lowercase();
    if l.starts_with('/') || l.starts_with("./") || l.starts_with("../") {
        return true; // a path to a key file, not a key
    }
    if l.chars().all(|c| c == l.chars().next().unwrap()) {
        return true; // xxxxxxxxxxxx
    }
    for w in [
        "example",
        "changeme",
        "change-me",
        "change_me",
        "placeholder",
        "redacted",
        "your-",
        "your_",
        "xxxx",
        "dummy",
        "sample",
        "secret-here",
        "<",
        ">",
    ] {
        if l.contains(w) {
            return true;
        }
    }
    // A short value with no digit is a word, not a key.
    !(l.chars().any(|c| c.is_ascii_digit()) || l.len() >= 20)
}

/// Shape test for a bare token: not a hash, not a UUID, not an identifier.
fn looks_like_secret(t: &str) -> bool {
    let has_digit = t.chars().any(|c| c.is_ascii_digit());
    let has_lower = t.chars().any(|c| c.is_ascii_lowercase());
    let has_upper = t.chars().any(|c| c.is_ascii_uppercase());
    let has_letter = has_lower || has_upper;
    if !has_digit || !has_letter {
        return false;
    }
    // Git SHA, blake3, sha256, UUID: hex with optional dashes. Not secrets.
    if t.chars().all(|c| c.is_ascii_hexdigit() || c == '-') {
        return false;
    }
    // Identifiers are word-shaped: many separators.
    let separators = t.chars().filter(|c| *c == '_' || *c == '-').count();
    if separators >= 3 {
        return false;
    }
    if shannon_bits_per_char(t) < 3.5 {
        return false;
    }
    (has_lower && has_upper) || t.len() >= 40
}

fn shannon_bits_per_char(t: &str) -> f64 {
    let mut counts = [0u32; 256];
    for b in t.bytes() {
        counts[b as usize] += 1;
    }
    let n = t.len() as f64;
    counts
        .iter()
        .filter(|&&c| c > 0)
        .map(|&c| {
            let p = c as f64 / n;
            -p * p.log2()
        })
        .sum()
}

// ---- IBAN ----

static IBAN: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"\b[A-Z]{2}[0-9]{2}(?: ?[A-Z0-9]{4}){2,7}(?: ?[A-Z0-9]{1,4})?\b").unwrap()
});

/// IBAN lengths per country (ISO 13616 registry, the common entries). An unknown country
/// code still passes if the checksum holds, at Medium confidence.
fn iban_length(cc: &str) -> Option<usize> {
    Some(match cc {
        "AL" => 28,
        "AD" => 24,
        "AT" => 20,
        "AZ" => 28,
        "BH" => 22,
        "BE" => 16,
        "BA" => 20,
        "BR" => 29,
        "BG" => 22,
        "CR" => 22,
        "HR" => 21,
        "CY" => 28,
        "CZ" => 24,
        "DK" => 18,
        "DO" => 28,
        "EE" => 20,
        "FO" => 18,
        "FI" => 18,
        "FR" => 27,
        "GE" => 22,
        "DE" => 22,
        "GI" => 23,
        "GR" => 27,
        "GL" => 18,
        "GT" => 28,
        "HU" => 28,
        "IS" => 26,
        "IE" => 22,
        "IL" => 23,
        "IT" => 27,
        "JO" => 30,
        "KZ" => 20,
        "XK" => 20,
        "KW" => 30,
        "LV" => 21,
        "LB" => 28,
        "LI" => 21,
        "LT" => 20,
        "LU" => 20,
        "MK" => 19,
        "MT" => 31,
        "MR" => 27,
        "MU" => 30,
        "MC" => 27,
        "MD" => 24,
        "ME" => 22,
        "NL" => 18,
        "NO" => 15,
        "PK" => 24,
        "PS" => 29,
        "PL" => 28,
        "PT" => 25,
        "QA" => 29,
        "RO" => 24,
        "SM" => 27,
        "SA" => 24,
        "RS" => 22,
        "SK" => 24,
        "SI" => 19,
        "ES" => 24,
        "SE" => 24,
        "CH" => 21,
        "TN" => 24,
        "TR" => 26,
        "AE" => 23,
        "GB" => 22,
        "VA" => 22,
        "VG" => 24,
        "UA" => 29,
        _ => return None,
    })
}

fn iban_mod97_ok(compact: &str) -> bool {
    if compact.len() < 15 {
        return false;
    }
    let rearranged: String = format!("{}{}", &compact[4..], &compact[..4]);
    let mut rem: u32 = 0;
    for c in rearranged.chars() {
        let v = match c {
            '0'..='9' => c as u32 - '0' as u32,
            'A'..='Z' => c as u32 - 'A' as u32 + 10,
            _ => return false,
        };
        rem = if v >= 10 {
            (rem * 100 + v) % 97
        } else {
            (rem * 10 + v) % 97
        };
    }
    rem == 1
}

fn find_ibans(text: &str, out: &mut Vec<Finding>) {
    for m in IBAN.find_iter(text) {
        let s = m.as_str();
        // Map compact characters back to byte offsets so a trimmed candidate still has
        // the right end.
        let mut compact = String::new();
        let mut ends: Vec<usize> = Vec::new();
        for (i, c) in s.char_indices() {
            if c != ' ' {
                compact.push(c);
                ends.push(m.start() + i + c.len_utf8());
            }
        }
        let cc = &compact[..2];
        let candidates: Vec<usize> = match iban_length(cc) {
            Some(len) => vec![len],
            None => (15..=34.min(compact.len())).rev().collect(),
        };
        for len in candidates {
            if len > compact.len() {
                continue;
            }
            let c = &compact[..len];
            if !iban_mod97_ok(c) {
                continue;
            }
            let end = ends[len - 1];
            if is_word(next_char(text, end)) {
                continue;
            }
            let (conf, why) = if iban_length(cc).is_some() {
                (
                    Confidence::High,
                    "IBAN: known country, correct length, checksum verified",
                )
            } else {
                (
                    Confidence::Medium,
                    "IBAN shape with a valid checksum but an unknown country code",
                )
            };
            push(out, PiiKind::Iban, text, m.start(), end, conf, why);
            break;
        }
    }
}

// ---- phone ----

static PHONE_INTL: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"(?:\+|\b00)[1-9]\d{0,2}[ \t./-]?(?:\(\d{1,5}\)[ \t./-]?)?\d(?:[ \t./-]?\d){6,13}")
        .unwrap()
});
static PHONE_NATIONAL: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"\b0[1-9]\d{0,4}[ \t/-]?\(?\d{2,5}\)?[ \t/-]?\d{2,}(?:[ \t-]\d{2,}){0,3}\b")
        .unwrap()
});
static PHONE_NANP: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"\(?\b[2-9]\d{2}\)?[ \t.-]\d{3}[ \t.-]\d{4}\b").unwrap());

fn digit_count(s: &str) -> usize {
    s.chars().filter(|c| c.is_ascii_digit()).count()
}

fn phone_boundaries_ok(text: &str, start: usize, end: usize) -> bool {
    let prev = prev_char(text, start);
    let next = next_char(text, end);
    if is_word(prev) || matches!(prev, Some('.') | Some('+') | Some('-') | Some(',')) {
        return false;
    }
    if is_word(next)
        || continues_dotted(text, end)
        || matches!(next, Some(',') | Some('-'))
            && text[end..]
                .chars()
                .nth(1)
                .is_some_and(|c| c.is_ascii_digit())
    {
        return false;
    }
    true
}

fn find_phones(text: &str, out: &mut Vec<Finding>) {
    for m in PHONE_INTL.find_iter(text) {
        let n = digit_count(m.as_str());
        if !(8..=15).contains(&n) || !phone_boundaries_ok(text, m.start(), m.end()) {
            continue;
        }
        let (conf, why) = if m.as_str().starts_with('+') {
            (Confidence::High, "international phone number with + prefix")
        } else {
            (
                Confidence::Medium,
                "international phone number with 00 prefix",
            )
        };
        push(out, PiiKind::Phone, text, m.start(), m.end(), conf, why);
    }
    for m in PHONE_NATIONAL.find_iter(text) {
        let n = digit_count(m.as_str());
        if !(9..=13).contains(&n) || !phone_boundaries_ok(text, m.start(), m.end()) {
            continue;
        }
        push(
            out,
            PiiKind::Phone,
            text,
            m.start(),
            m.end(),
            Confidence::Medium,
            "national phone number shape (leading 0)",
        );
    }
    for m in PHONE_NANP.find_iter(text) {
        if !phone_boundaries_ok(text, m.start(), m.end()) {
            continue;
        }
        push(
            out,
            PiiKind::Phone,
            text,
            m.start(),
            m.end(),
            Confidence::Medium,
            "North American phone number shape",
        );
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn only(text: &str, kind: PiiKind) -> Vec<String> {
        scan(text)
            .into_iter()
            .filter(|f| f.kind == kind)
            .map(|f| f.matched)
            .collect()
    }

    fn none_of(text: &str, kind: PiiKind) {
        let hits = only(text, kind);
        assert!(
            hits.is_empty(),
            "{text:?} should not yield {kind}: {hits:?}"
        );
    }

    // ---- email ----

    #[test]
    fn email_true_positives() {
        assert_eq!(
            only(
                "contact alice.smith+tag@example-corp.de today",
                PiiKind::Email
            ),
            ["alice.smith+tag@example-corp.de"]
        );
        assert_eq!(
            only("(bob@sub.domain.co.uk)", PiiKind::Email),
            ["bob@sub.domain.co.uk"]
        );
        let f = &scan("x o'brien@irish.ie")[0];
        assert_eq!(f.matched, "o'brien@irish.ie");
        assert_eq!(f.confidence, Confidence::High);
    }

    #[test]
    fn email_false_positive_candidates() {
        none_of("pkg foo@1.2.3 pinned", PiiKind::Email); // npm version spec
        none_of("ping @handle on slack", PiiKind::Email);
        none_of("user@localhost", PiiKind::Email); // no TLD
        none_of("a@b", PiiKind::Email);
        none_of("root@[10.0.0.1]", PiiKind::Email);
        // Documentation and system senders are reported, but at Low confidence.
        for s in ["user@example.com", "git@github.com", "noreply@github.com"] {
            let f = scan(s);
            assert_eq!(f.len(), 1, "{s}");
            assert_eq!(f[0].confidence, Confidence::Low, "{s}");
        }
    }

    // ---- IPv4 ----

    #[test]
    fn ipv4_true_positives() {
        let f = scan("client 203.0.113.7 connected");
        assert_eq!(f[0].kind, PiiKind::Ipv4);
        assert_eq!(f[0].matched, "203.0.113.7");
        assert_eq!(
            &"client 203.0.113.7 connected"[f[0].start..f[0].end],
            "203.0.113.7"
        );
        assert_eq!(only("dns 8.8.8.8.", PiiKind::Ipv4), ["8.8.8.8"]); // sentence-ending dot
        assert_eq!(scan("via 85.214.132.117")[0].confidence, Confidence::High);
        assert_eq!(scan("host 192.168.1.20")[0].confidence, Confidence::Low);
        assert_eq!(scan("127.0.0.1")[0].confidence, Confidence::Low);
    }

    #[test]
    fn ipv4_false_positive_candidates() {
        none_of("next 16.3.1 and 1.2.3", PiiKind::Ipv4); // three-part versions
        none_of("v1.2.3.4 tagged", PiiKind::Ipv4); // v-prefixed four-part version
        none_of("Windows 10.0.19045.1", PiiKind::Ipv4); // octet > 255
        none_of("999.1.1.1", PiiKind::Ipv4);
        none_of("1.2.3.4.5", PiiKind::Ipv4); // five parts
        none_of("01.02.03.04", PiiKind::Ipv4); // leading zeros
        none_of("build 2026.09.05.1", PiiKind::Ipv4);
    }

    // ---- IPv6 ----

    #[test]
    fn ipv6_true_positives() {
        assert_eq!(
            only("from 2a02:8108:8a80:1234::7f3 port 5", PiiKind::Ipv6),
            ["2a02:8108:8a80:1234::7f3"]
        );
        assert_eq!(
            scan("from 2a02:8108:8a80:1234::7f3")[0].confidence,
            Confidence::High
        );
        assert_eq!(
            only("[2001:db8:85a3::8a2e:370:7334]", PiiKind::Ipv6),
            ["2001:db8:85a3::8a2e:370:7334"]
        );
        assert_eq!(scan("x 2001:db8::1")[0].confidence, Confidence::Low); // documentation prefix
        assert_eq!(
            only("mapped ::ffff:203.0.113.5 end", PiiKind::Ipv6),
            ["::ffff:203.0.113.5"]
        );
        assert_eq!(only("route 2001:db8::/32", PiiKind::Ipv6), ["2001:db8::"]);
        assert_eq!(scan("fe80::1")[0].confidence, Confidence::Low);
        assert_eq!(scan("::1")[0].confidence, Confidence::Low);
    }

    #[test]
    fn ipv6_false_positive_candidates() {
        none_of("at 12:30:45 today", PiiKind::Ipv6);
        none_of("mac de:ad:be:ef:00:01", PiiKind::Ipv6);
        none_of("use std::net::Ipv6Addr and fee::add()", PiiKind::Ipv6);
        none_of("a::b", PiiKind::Ipv6);
        none_of("dead::beef", PiiKind::Ipv6); // no decimal digit
        none_of("ratio 1:2:3", PiiKind::Ipv6);
        none_of("::", PiiKind::Ipv6);
        none_of("Vec::<u8>::new()", PiiKind::Ipv6);
    }

    // ---- API keys ----

    #[test]
    fn api_key_true_positives() {
        let cases = [
            "sk-abcdefghijklmnopqrstuvwxyz1234567890",
            "sk-ant-api03-abcdefghijklmnopqrstuvwxyz1234567890",
            "ghp_ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghij",
            "AKIAIOSFODNN7EXAMPLE",
            "xoxb-123456789012-abcdefghijkl",
            "glpat-abcdefghijklmnopqrstuvwx",
            "eyJhbGciOiJIUzI1NiJ9.eyJzdWIiOiIxMjM0In0.SflKxwRJSMeKKF2QT4fwpMeJf36POk6yJV_adQssw5c",
        ];
        for c in cases {
            let f = scan(&format!("key: {c} end"));
            assert_eq!(f.len(), 1, "{c}: {f:?}");
            assert_eq!(f[0].kind, PiiKind::ApiKey, "{c}");
            assert_eq!(f[0].matched, c, "{c}");
            assert_eq!(f[0].confidence, Confidence::High, "{c}");
        }
    }

    #[test]
    fn api_key_from_assignment_context_and_bearer() {
        let f = scan("API_KEY=a1b2c3d4e5f6g7h8i9j0 rest");
        assert_eq!(f.len(), 1);
        assert_eq!(
            f[0].matched, "a1b2c3d4e5f6g7h8i9j0",
            "only the value is flagged, so redaction keeps the key name"
        );
        let f = scan(r#"password: "Tr0ub4dor3xyz9&more""#);
        assert_eq!(f.len(), 1);
        assert_eq!(
            f[0].matched, "Tr0ub4dor3xyz9",
            "the value stops at the first non-token character"
        );
        let f = scan("curl -H 'Authorization: Bearer AbCdEf0123456789xyzXYZ' https://x");
        assert!(
            f.iter()
                .any(|f| f.kind == PiiKind::ApiKey && f.matched == "AbCdEf0123456789xyzXYZ"),
            "{f:?}"
        );
        let f = scan("FERNET_KEY=Zm9vYmFyYmF6cXV4MTIzNDU2Nzg5MGFiY2RlZmdoaWo=");
        assert_eq!(f[0].kind, PiiKind::ApiKey);
    }

    #[test]
    fn pem_block_is_one_finding() {
        let text = "before\n-----BEGIN RSA PRIVATE KEY-----\nMIIEow\nABCD\n-----END RSA PRIVATE KEY-----\nafter";
        let f = scan(text);
        assert_eq!(f.len(), 1);
        assert!(f[0].matched.starts_with("-----BEGIN") && f[0].matched.ends_with("KEY-----"));
        let (red, n) = redact(text, &f);
        assert_eq!(n, 1);
        assert_eq!(red, "before\n[redacted:api-key]\nafter");
    }

    #[test]
    fn generic_high_entropy_token_is_medium() {
        let f = scan("token was Xk9pQ2mZ7vB4nR8tW1yC5dF3gH6jL0aS2eU4iO7");
        assert_eq!(f.len(), 1, "{f:?}");
        assert_eq!(f[0].kind, PiiKind::ApiKey);
        assert_eq!(f[0].confidence, Confidence::Medium);
    }

    #[test]
    fn api_key_false_positive_candidates() {
        // A git SHA is not an API key.
        none_of(
            "commit 4943b50a1c2e3f4d5b6a7c8d9e0f1a2b3c4d5e6f",
            PiiKind::ApiKey,
        );
        // Nor a sha256, a blake3 hash, a UUID or a ULID.
        none_of(
            "sha256 e3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b855",
            PiiKind::ApiKey,
        );
        none_of("id 123e4567-e89b-12d3-a456-426614174000", PiiKind::ApiKey);
        none_of("note 01ARZ3NDEKTSV4RRFFQ69G5FAV", PiiKind::ApiKey);
        // Nor a version, a long identifier, a placeholder, an env reference or a path.
        none_of("cyberbrain 0.1.0 with rusqlite 0.40.2", PiiKind::ApiKey);
        none_of(
            "fn very_long_snake_case_identifier_name_for_a_test_2024() {}",
            PiiKind::ApiKey,
        );
        none_of("password = changeme", PiiKind::ApiKey);
        none_of("api_key = <your-api-key>", PiiKind::ApiKey);
        none_of("token = ${GITHUB_TOKEN}", PiiKind::ApiKey);
        none_of("private_key = /etc/ssl/private/server.key", PiiKind::ApiKey);
        none_of("secret: xxxxxxxxxxxxxxxxxxxx", PiiKind::ApiKey);
        none_of("token = os.getenv(\"TOKEN\")", PiiKind::ApiKey);
        // A citation is short and hex.
        none_of("see r2-a91f2c33e1 for detail", PiiKind::ApiKey);
        // Base64 of a hash-shaped thing: 44 chars, but no mixed case? This one is mixed and
        // will be flagged Medium, which is the documented behaviour, so it is not asserted.
    }

    // ---- IBAN ----

    #[test]
    fn iban_true_positives() {
        for (s, cc) in [
            ("DE89 3704 0044 0532 0130 00", "DE"),
            ("DE89370400440532013000", "DE"),
            ("GB82 WEST 1234 5698 7654 32", "GB"),
            ("CH93 0076 2011 6238 5295 7", "CH"),
            ("FR14 2004 1010 0505 0001 3M02 606", "FR"),
        ] {
            let f = scan(&format!("pay to {s} thanks"));
            assert_eq!(f.len(), 1, "{s}: {f:?}");
            assert_eq!(f[0].kind, PiiKind::Iban, "{s}");
            assert_eq!(f[0].matched, s, "{s}");
            assert_eq!(f[0].confidence, Confidence::High, "{cc}");
        }
    }

    #[test]
    fn iban_with_trailing_uppercase_word_is_trimmed_to_its_real_length() {
        let f = only(
            "IBAN DE89 3704 0044 0532 0130 00 BIC COBADEFF",
            PiiKind::Iban,
        );
        assert_eq!(f, ["DE89 3704 0044 0532 0130 00"]);
    }

    #[test]
    fn iban_false_positive_candidates() {
        none_of("version 1.2.3 and 16.3.1", PiiKind::Iban);
        none_of("DE89370400440532013001", PiiKind::Iban); // wrong length for DE
        none_of("GB82WEST12345698765433", PiiKind::Iban); // checksum off by one
        none_of("XY12ABCD1234EFGH5678", PiiKind::Iban); // unknown country, bad checksum
        none_of("CH93 0076 2011 6238 5295 8", PiiKind::Iban);
        none_of("AB12", PiiKind::Iban);
    }

    #[test]
    fn iban_digit_groups_are_not_also_reported_as_phone_numbers() {
        let f = scan("DE89 3704 0044 0532 0130 00");
        assert_eq!(f.len(), 1);
        assert_eq!(f[0].kind, PiiKind::Iban);
    }

    // ---- phone ----

    #[test]
    fn phone_true_positives() {
        assert_eq!(
            only("call +41 44 668 18 00 now", PiiKind::Phone),
            ["+41 44 668 18 00"]
        );
        assert_eq!(
            only("tel +49 (0) 371 1234567.", PiiKind::Phone),
            ["+49 (0) 371 1234567"]
        );
        assert_eq!(only("+15551234567", PiiKind::Phone), ["+15551234567"]);
        assert_eq!(only("0041446681800", PiiKind::Phone), ["0041446681800"]);
        assert_eq!(scan("+41 44 668 18 00")[0].confidence, Confidence::High);
        assert_eq!(only("Büro 0371 1234567", PiiKind::Phone), ["0371 1234567"]);
        assert_eq!(only("044 668 18 00", PiiKind::Phone), ["044 668 18 00"]);
        assert_eq!(scan("0371 1234567")[0].confidence, Confidence::Medium);
        assert_eq!(
            only("call (555) 123-4567 or 800.555.1234", PiiKind::Phone),
            ["(555) 123-4567", "800.555.1234"]
        );
    }

    #[test]
    fn phone_false_positive_candidates() {
        none_of("Halten +21,05 vs +16,53 R, System +22.54 R", PiiKind::Phone); // signed amounts
        none_of(
            "2026-09-05T09:12:03Z and 05.09.2026 at 08:00",
            PiiKind::Phone,
        );
        none_of("versions 1.0.0, 0.4.33, 16.3.1", PiiKind::Phone);
        none_of("ulid 01JQZ8ABCDEFGHJKMNPQRS", PiiKind::Phone);
        none_of("order 0123 shipped", PiiKind::Phone);
        none_of("0.15 s", PiiKind::Phone);
        none_of("port 11434 and 7777", PiiKind::Phone);
        none_of("ip 192.168.1.20", PiiKind::Phone);
        none_of("1234567890123", PiiKind::Phone); // no prefix, no separators
        none_of("+1 2", PiiKind::Phone);
        none_of("2026-09-05 09:12", PiiKind::Phone);
    }

    // ---- composition ----

    #[test]
    fn findings_are_sorted_and_non_overlapping_with_correct_offsets() {
        let text = "Alice <alice@corp.example.org> from 203.0.113.9, IBAN DE89 3704 0044 0532 0130 00, tel +41 44 668 18 00, key sk-abcdefghijklmnopqrstuvwxyz0123";
        let f = scan(text);
        let ks: Vec<PiiKind> = f.iter().map(|f| f.kind).collect();
        assert_eq!(
            ks,
            [
                PiiKind::Email,
                PiiKind::Ipv4,
                PiiKind::Iban,
                PiiKind::Phone,
                PiiKind::ApiKey
            ]
        );
        for w in f.windows(2) {
            assert!(w[0].end <= w[1].start);
        }
        for x in &f {
            assert_eq!(&text[x.start..x.end], x.matched);
        }
    }

    #[test]
    fn offsets_are_byte_offsets_and_survive_multibyte_text() {
        let text = "Grüße aus Zürich — mail müller@zürich-firma.ch, IP 203.0.113.9";
        let f = scan(text);
        for x in &f {
            assert!(text.is_char_boundary(x.start) && text.is_char_boundary(x.end));
            assert_eq!(&text[x.start..x.end], x.matched);
        }
        assert!(f.iter().any(|x| x.kind == PiiKind::Ipv4));
        // The umlaut local part is not ASCII, so the email detector starts at "ller@..." at
        // best and refuses because it is preceded by a word character. That is a known
        // limitation of the ASCII email shape, documented rather than hidden.
        let (red, _) = redact(text, &f);
        assert!(red.contains("[redacted:ipv4]"));
        assert!(red.starts_with("Grüße aus Zürich"));
    }

    #[test]
    fn redaction_replaces_and_rescan_is_clean() {
        let text = "mail bob@corp.de ip 203.0.113.9 iban DE89 3704 0044 0532 0130 00";
        let f = scan(text);
        let (red, n) = redact(text, &f);
        assert_eq!(n, 3);
        assert_eq!(
            red,
            "mail [redacted:email] ip [redacted:ipv4] iban [redacted:iban]"
        );
        assert!(scan(&red).is_empty());
    }

    #[test]
    fn redaction_ignores_stale_offsets_instead_of_corrupting() {
        let f = vec![Finding {
            kind: PiiKind::Email,
            start: 3,
            end: 999,
            matched: String::new(),
            confidence: Confidence::High,
            why: "",
        }];
        let (red, n) = redact("abc", &f);
        assert_eq!(n, 0);
        assert_eq!(red, "abc");
    }

    #[test]
    fn audit_view_never_carries_the_matched_text() {
        let f = &scan("bob@corp.de")[0];
        let v = f.audit_view().to_string();
        assert!(!v.contains("bob"), "{v}");
        assert!(v.contains("\"start\":0"));
    }

    #[test]
    fn empty_and_clean_text() {
        assert!(scan("").is_empty());
        assert!(scan("A perfectly ordinary note about Postgres 18 moving PGDATA.").is_empty());
    }

    #[test]
    fn iban_mod97_reference_values() {
        assert!(iban_mod97_ok("GB82WEST12345698765432"));
        assert!(iban_mod97_ok("DE89370400440532013000"));
        assert!(!iban_mod97_ok("DE89370400440532013001"));
        assert!(!iban_mod97_ok("DE8937"));
    }
}
