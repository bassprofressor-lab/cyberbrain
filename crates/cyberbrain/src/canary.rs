//! Canaries: notes that hold a credential nobody uses, so that its use somewhere is a signal.
//!
//! `canary plant` makes a value that looks like an API key, an IBAN, a password or a token
//! and is certainly not one: the IBAN has a valid check sum and a bank code no German bank
//! has (German bank codes do not start with 0), the key and the token carry a prefix no
//! provider issues. It files the value in a note that reads like the real thing, and keeps
//! the value in a register beside the store's audit key (`keys/canaries.json`, owner-only,
//! kept out of a repository by the `.gitignore` there).
//!
//! The value is then watched for in three places on this machine: every tool call the
//! pre-tool-use hook sees, and every note written or proposed. Used anywhere but in its own
//! note, it is refused, counted and recorded. With AgentGuard configured, `canary push` hands
//! the values to it, and it watches every agent it governs as well.
//!
//! # What never happens
//!
//! The value is never written into the audit log, a refusal reason, a hub alert or anything
//! printed. Those carry the fingerprint: the first 12 hex digits of SHA-256 over the value,
//! the same fingerprint AgentGuard names a canary by.
//!
//! # Not misleading people
//!
//! The note carries the tag `canary`, which a person sees in every list and on the note
//! itself, and nothing in its text says so: an agent that reads it should find a credential,
//! a person who opens it should not use one.

use crate::app::{App, WriteOutcome, WriteRequest};
use cyberbrain_core::{Error, NoteKind, Result, Ring};
use cyberbrain_policy::audit::Actor;
use cyberbrain_policy::write_gate::OperatorChoice;
use serde::{Deserialize, Serialize};
use serde_json::json;
use std::path::{Path, PathBuf};

/// The register, relative to the store root, beside the audit key.
pub const REGISTER_FILE: &str = "keys/canaries.json";
/// The tag a person recognises a canary by.
pub const TAG: &str = "canary";

/// What a canary looks like.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, clap::ValueEnum)]
#[serde(rename_all = "kebab-case")]
pub enum Kind {
    ApiKey,
    Iban,
    Password,
    Token,
}

impl Kind {
    pub fn as_str(self) -> &'static str {
        match self {
            Kind::ApiKey => "api-key",
            Kind::Iban => "iban",
            Kind::Password => "password",
            Kind::Token => "token",
        }
    }

    /// The name AgentGuard uses for it (`art`).
    pub fn agentguard_art(self) -> &'static str {
        match self {
            Kind::ApiKey => "api_key",
            Kind::Iban => "iban",
            Kind::Password => "passwort",
            Kind::Token => "token",
        }
    }
}

/// One canary in the register.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Canary {
    pub fingerprint: String,
    pub kind: Kind,
    pub label: String,
    /// The note that holds it. Using the value there is not a hit.
    pub note: String,
    pub ring: u8,
    pub created_at: String,
    #[serde(default)]
    pub hits: u64,
    #[serde(default)]
    pub last_hit_at: Option<String>,
    #[serde(default)]
    pub pushed_at: Option<String>,
    /// The value itself. Only here and in the note; never in a row, a reason or an answer.
    pub value: String,
}

/// A canary as it may be shown: everything but the value.
#[derive(Debug, Clone, Serialize)]
pub struct Shown {
    pub fingerprint: String,
    pub kind: Kind,
    pub label: String,
    pub note: String,
    pub ring: u8,
    pub created_at: String,
    pub hits: u64,
    pub last_hit_at: Option<String>,
    pub pushed_at: Option<String>,
}

impl From<&Canary> for Shown {
    fn from(c: &Canary) -> Self {
        Shown {
            fingerprint: c.fingerprint.clone(),
            kind: c.kind,
            label: c.label.clone(),
            note: c.note.clone(),
            ring: c.ring,
            created_at: c.created_at.clone(),
            hits: c.hits,
            last_hit_at: c.last_hit_at.clone(),
            pushed_at: c.pushed_at.clone(),
        }
    }
}

/// What the status card shows.
#[derive(Debug, Clone, Default, Serialize)]
pub struct Summary {
    pub count: usize,
    pub hits: u64,
    pub last_hit_at: Option<String>,
}

pub fn summary(list: &[Canary]) -> Summary {
    Summary {
        count: list.len(),
        hits: list.iter().map(|c| c.hits).sum(),
        last_hit_at: list.iter().filter_map(|c| c.last_hit_at.clone()).max(),
    }
}

/// The fingerprint a canary is named by: the first 12 hex digits of SHA-256 over the value,
/// as AgentGuard computes it.
pub fn fingerprint(value: &str) -> String {
    use sha2::Digest;
    let d = sha2::Sha256::digest(value.as_bytes());
    d.iter().take(6).map(|b| format!("{b:02x}")).collect()
}

/// What is compared: letters and digits only, lower case. An IBAN written in groups of four,
/// a key in quotes or in a JSON string, a password with its punctuation: all the same value.
pub fn normalise(s: &str) -> String {
    s.chars()
        .filter(char::is_ascii_alphanumeric)
        .map(|c| c.to_ascii_lowercase())
        .collect()
}

/// The canary whose value occurs in `text`, if one does.
pub fn find<'a>(register: &'a [Canary], text: &str) -> Option<&'a Canary> {
    if register.is_empty() {
        return None;
    }
    let hay = normalise(text);
    register.iter().find(|c| {
        let needle = normalise(&c.value);
        // A value too short to be told from chance is never matched; plant never makes one.
        needle.len() >= 12 && hay.contains(&needle)
    })
}

fn register_path(root: &Path) -> PathBuf {
    root.join(REGISTER_FILE)
}

/// The register. A store without one has no canaries.
pub fn load(root: &Path) -> Result<Vec<Canary>> {
    let path = register_path(root);
    match std::fs::read_to_string(&path) {
        Ok(text) => serde_json::from_str(&text)
            .map_err(|e| Error::Config(format!("{}: {e}", path.display()))),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(Vec::new()),
        Err(e) => Err(Error::Io { path, source: e }),
    }
}

fn save(root: &Path, list: &[Canary]) -> Result<()> {
    let path = register_path(root);
    let dir = path.parent().expect("the register has a directory");
    std::fs::create_dir_all(dir).map_err(|e| Error::Io {
        path: dir.to_path_buf(),
        source: e,
    })?;
    let ignore = dir.join(".gitignore");
    if !ignore.exists() {
        std::fs::write(&ignore, "# Private keys. Never commit these.\n*\n").map_err(|e| {
            Error::Io {
                path: ignore.clone(),
                source: e,
            }
        })?;
    }
    let text = serde_json::to_string_pretty(list)
        .map_err(|e| Error::Index(format!("canary register: {e}")))?;
    crate::hub::client::write_owner_only(&path, &format!("{text}\n"))
        .map_err(|e| Error::Io { path, source: e })
}

fn random_bytes(n: usize) -> Result<Vec<u8>> {
    let mut b = vec![0u8; n];
    getrandom::fill(&mut b).map_err(|e| Error::Index(format!("no randomness: {e}")))?;
    Ok(b)
}

fn pick(bytes: &[u8], alphabet: &[u8]) -> String {
    bytes
        .iter()
        .map(|b| alphabet[*b as usize % alphabet.len()] as char)
        .collect()
}

const ALNUM: &[u8] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789";
const DIGITS: &[u8] = b"0123456789";

/// A German IBAN with a valid check sum whose bank code starts with 0, which no German bank
/// code does: it passes every format check and reaches no account.
pub fn fake_iban(digits: &[u8]) -> String {
    let bban = format!("0{}", pick(&digits[..17], DIGITS));
    let check = 98 - mod97(&format!("{bban}131400"));
    format!("DE{check:02}{bban}")
}

fn mod97(numeric: &str) -> u32 {
    numeric
        .bytes()
        .fold(0u32, |acc, b| (acc * 10 + u32::from(b - b'0')) % 97)
}

/// A new value of `kind`.
pub fn generate(kind: Kind) -> Result<String> {
    Ok(match kind {
        // `kx_live_`: shaped like a payment provider's live key, issued by none.
        Kind::ApiKey => format!("kx_live_{}", pick(&random_bytes(32)?, ALNUM)),
        Kind::Iban => fake_iban(&random_bytes(17)?),
        Kind::Password => {
            const WORDS: [&str; 8] = [
                "Sommer",
                "Winter",
                "Herbst",
                "Fruehling",
                "Lager",
                "Hafen",
                "Kontor",
                "Werk",
            ];
            let b = random_bytes(8)?;
            format!(
                "{}{}!{}",
                WORDS[b[0] as usize % WORDS.len()],
                2020 + u32::from(b[1]) % 10,
                pick(&b[2..8], ALNUM)
            )
        }
        Kind::Token => format!("dtk_{}", pick(&random_bytes(40)?, ALNUM)),
    })
}

/// The note a canary lives in: a name and a text a reader takes for a working credential.
fn note_text(kind: Kind, value: &str) -> (&'static str, String) {
    match kind {
        Kind::ApiKey => (
            "payments-api-key-production",
            format!(
                "Payments provider API key, production.\n\nKey: `{value}`\n\nUsed by the \
                 nightly billing export. Rotate together with the webhook secret.\n"
            ),
        ),
        Kind::Iban => {
            let grouped = value
                .as_bytes()
                .chunks(4)
                .map(|c| String::from_utf8_lossy(c).into_owned())
                .collect::<Vec<_>>()
                .join(" ");
            (
                "supplier-refund-account",
                format!(
                    "Account for supplier refunds.\n\nIBAN: {grouped}\n\nAccount holder as on \
                     the invoice header. Refunds above 5,000 EUR need a second signature.\n"
                ),
            )
        }
        Kind::Password => (
            "backup-console-admin-login",
            format!(
                "Admin login for the old backup console.\n\nUser: `backup-admin`\nPassword: \
                 `{value}`\n\nOnly needed when the nightly restore test fails.\n"
            ),
        ),
        Kind::Token => (
            "release-pipeline-deploy-token",
            format!(
                "Deploy token for the release pipeline.\n\nToken: `{value}`\n\nScoped to the \
                 production environment.\n"
            ),
        ),
    }
}

/// What `plant` made.
#[derive(Debug, Clone, Serialize)]
pub struct Planted {
    pub fingerprint: String,
    pub kind: Kind,
    pub label: String,
    pub note: String,
    pub ring: u8,
}

/// Make a canary: the value, its note, the register entry and an audit row.
pub fn plant(app: &App, kind: Kind, label: Option<&str>, ring: Ring) -> Result<Planted> {
    // The note is written with the PII hold answered "reviewed", which only the operator may
    // do. Said here, before anything exists, rather than as the write gate's refusal about
    // a note the agent never named.
    if !matches!(app.actor(), Actor::Operator | Actor::Cli) {
        return Err(Error::Config(
            "a canary is planted by the operator: run `cyberbrain canary plant` in your own \
             terminal. An agent that could plant one would know where every decoy is"
                .into(),
        ));
    }
    if !matches!(ring, Ring::Knowledge | Ring::Session | Ring::External) {
        return Err(Error::Config(
            "a canary goes into ring 2, 3 or 4: rings 0 and 1 are injected into every \
             session, and a credential there would be in every prompt"
                .into(),
        ));
    }
    let mut list = load(app.root())?;
    let value = generate(kind)?;
    let fp = fingerprint(&value);
    let (base, body) = note_text(kind, &value);
    // A suffix from the fingerprint, so a second canary of the same kind gets a name of its
    // own; four hex digits read like a version or an environment.
    let name = format!("{base}-{}", &fp[..4]);
    let label = label
        .map(str::trim)
        .filter(|l| !l.is_empty())
        .unwrap_or(kind.as_str())
        .chars()
        .take(80)
        .collect::<String>();
    // Written as the operator would, with the PII hold answered "reviewed": a scanner that
    // redacted or refused the value would have removed the canary. The text is unchanged.
    let outcome = app.write(WriteRequest {
        ring,
        kind: NoteKind::Reference,
        name: name.clone(),
        body,
        tags: vec![TAG.to_string()],
        bereich: None,
        retention: None,
        supersedes: None,
        valid_from: None,
        invalid_at: None,
        force: false,
        choice: Some(OperatorChoice::MarkReviewed),
        expected_updated: None,
        arriving: None,
        dry_run: false,
    })?;
    match outcome {
        WriteOutcome::Written(_) => {}
        other => {
            return Err(Error::Config(format!(
                "the canary note was not written: {}",
                match other {
                    WriteOutcome::Held { .. } => "held by the PII gate",
                    WriteOutcome::Conflict { .. } => "a note of that name changed meanwhile",
                    WriteOutcome::Quarantined(_) => "it went to proposals/",
                    WriteOutcome::Written(_) => unreachable!(),
                }
            )));
        }
    }
    let c = Canary {
        fingerprint: fp.clone(),
        kind,
        label: label.clone(),
        note: name.clone(),
        ring: ring.as_u8(),
        created_at: jiff::Timestamp::now().to_string(),
        hits: 0,
        last_hit_at: None,
        pushed_at: None,
        value,
    };
    list.push(c);
    save(app.root(), &list)?;
    app.policy().audit().record_raw(
        &app.actor().to_string(),
        "canary.planted",
        format!("canary:{fp}"),
        json!({ "fingerprint": fp, "kind": kind, "label": label, "note": name }),
    )?;
    Ok(Planted {
        fingerprint: fp,
        kind,
        label,
        note: name,
        ring: ring.as_u8(),
    })
}

/// The register as it may be shown.
pub fn list(app: &App) -> Result<Vec<Shown>> {
    Ok(load(app.root())?.iter().map(Shown::from).collect())
}

/// Take a canary away: its note is erased like any other (`forget`), the entry goes, and the
/// log says so. `which` is the fingerprint or the note's name.
pub fn remove(app: &App, which: &str) -> Result<Shown> {
    let mut list = load(app.root())?;
    let Some(i) = list
        .iter()
        .position(|c| c.fingerprint == which || c.note == which)
    else {
        return Err(Error::Config(format!(
            "no canary {which:?}; `cyberbrain canary list` shows them"
        )));
    };
    let c = list.remove(i);
    match app.forget(&c.note, false) {
        Ok(_) | Err(Error::NoSuchNote(_)) => {}
        Err(e) => return Err(e),
    }
    save(app.root(), &list)?;
    app.policy().audit().record_raw(
        &app.actor().to_string(),
        "canary.removed",
        format!("canary:{}", c.fingerprint),
        json!({ "fingerprint": c.fingerprint, "kind": c.kind, "label": c.label, "note": c.note }),
    )?;
    Ok(Shown::from(&c))
}

/// A canary turned up. Counted in the register and recorded, by fingerprint; `place` says
/// where (`tool:<name>`, `write`, `propose`). Best effort on the register: a refusal must not
/// turn into an error because a counter could not be written.
pub fn record_hit(app: &App, c: &Canary, place: &str, session: Option<&str>) {
    let now = jiff::Timestamp::now().to_string();
    if let Ok(mut list) = load(app.root()) {
        if let Some(e) = list.iter_mut().find(|e| e.fingerprint == c.fingerprint) {
            e.hits += 1;
            e.last_hit_at = Some(now.clone());
        }
        let _ = save(app.root(), &list);
    }
    if let Err(e) = app.policy().audit().record_raw(
        &app.actor().to_string(),
        "canary.hit",
        format!("canary:{}", c.fingerprint),
        json!({
            "fingerprint": c.fingerprint, "kind": c.kind, "label": c.label,
            "place": place, "session": session,
        }),
    ) {
        eprintln!(
            "cyberbrain: canary {} was hit, and the audit row failed: {e}",
            c.fingerprint
        );
    }
}

/// Refuse `text` going into note `name` when it carries a decoy that does not live there.
/// `place` is `write` or `propose`. A store whose register cannot be read refuses nothing:
/// the register is advisory, and a broken file must not stop every write.
pub fn guard_note(app: &App, name: &str, text: &str, place: &str) -> Result<()> {
    let register = load(app.root()).unwrap_or_default();
    match find(&register, text) {
        Some(c) if c.note != name => {
            record_hit(app, c, place, None);
            Err(Error::PolicyRefusal {
                profile: "canary".into(),
                reason: refusal(c),
            })
        }
        _ => Ok(()),
    }
}

/// Why a use of a canary was refused, for the agent and the person. Names the fingerprint,
/// never the value.
pub fn refusal(c: &Canary) -> String {
    format!(
        "canary:{fp} — this input contains a value this store keeps as a decoy ({kind}, \
         \"{label}\"). It is not a working credential, and using it is recorded. If you are \
         a person and copied it by mistake, nothing happened; tell the operator.",
        fp = c.fingerprint,
        kind = c.kind.as_str(),
        label = c.label,
    )
}

/// What `push` did.
#[derive(Debug, Clone, Serialize)]
pub struct Pushed {
    pub sent: usize,
    pub created: Option<u64>,
    pub already: Option<u64>,
}

/// Hand every canary to AgentGuard, so it watches its agents for them too. Only with a
/// governance service configured; over its egress path and its key, which needs the scope
/// `koeder:write` there.
pub fn push(app: &App) -> Result<Pushed> {
    let mut list = load(app.root())?;
    if list.is_empty() {
        return Ok(Pushed {
            sent: 0,
            created: None,
            already: None,
        });
    }
    let items: Vec<serde_json::Value> = list
        .iter()
        .map(|c| json!({ "wert": c.value, "art": c.kind.agentguard_art(), "label": c.label }))
        .collect();
    let v = crate::agentguard::push_koeder(app, app.actor(), items)?;
    let now = jiff::Timestamp::now().to_string();
    for c in &mut list {
        c.pushed_at = Some(now.clone());
    }
    save(app.root(), &list)?;
    let fps: Vec<&str> = list.iter().map(|c| c.fingerprint.as_str()).collect();
    app.policy().audit().record_raw(
        &app.actor().to_string(),
        "canary.pushed",
        "canary:agentguard",
        json!({ "fingerprints": fps, "created": v["angelegt"], "already": v["schon_da"] }),
    )?;
    Ok(Pushed {
        sent: list.len(),
        created: v["angelegt"].as_u64(),
        already: v["schon_da"].as_u64(),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn entry(value: &str, note: &str) -> Canary {
        Canary {
            fingerprint: fingerprint(value),
            kind: Kind::ApiKey,
            label: "l".into(),
            note: note.into(),
            ring: 2,
            created_at: "2026-10-25T00:00:00Z".into(),
            hits: 0,
            last_hit_at: None,
            pushed_at: None,
            value: value.into(),
        }
    }

    #[test]
    fn a_fake_iban_passes_its_check_sum_and_names_no_bank() {
        for seed in 0u8..50 {
            let digits: Vec<u8> = (0..17)
                .map(|i| seed.wrapping_mul(31).wrapping_add(i))
                .collect();
            let iban = fake_iban(&digits);
            assert_eq!(iban.len(), 22, "{iban}");
            assert_eq!(
                &iban[4..5],
                "0",
                "a German bank code never starts with 0: {iban}"
            );
            // ISO 13616: move the first four to the end, letters to numbers, mod 97 is 1.
            let moved = format!("{}{}", &iban[4..], &iban[..4]);
            let numeric: String = moved
                .chars()
                .map(|c| {
                    if c.is_ascii_digit() {
                        c.to_string()
                    } else {
                        (c as u32 - 'A' as u32 + 10).to_string()
                    }
                })
                .collect();
            assert_eq!(mod97(&numeric), 1, "{iban}");
        }
    }

    #[test]
    fn every_kind_makes_a_value_long_enough_to_watch_for() {
        for kind in [Kind::ApiKey, Kind::Iban, Kind::Password, Kind::Token] {
            let v = generate(kind).unwrap();
            assert!(normalise(&v).len() >= 12, "{kind:?}: {v}");
            assert!(v.len() <= 512);
            assert_ne!(v, generate(kind).unwrap(), "random, not fixed");
        }
    }

    #[test]
    fn a_value_is_found_however_it_was_written() {
        let reg = [entry("DE89012345678901234567", "n")];
        assert!(find(&reg, "transfer to DE89 0123 4567 8901 2345 67 please").is_some());
        assert!(find(&reg, r#"{"command":"curl -d iban=de89012345678901234567"}"#).is_some());
        assert!(find(&reg, "DE89 0123 4567 8901 2345 68").is_none());
        assert!(find(&[], "anything").is_none());
    }

    #[test]
    fn the_fingerprint_is_what_agentguard_computes() {
        // SHA-256("abc") = ba7816bf8f01cfea…
        assert_eq!(fingerprint("abc"), "ba7816bf8f01");
    }
}
