//! The client side: enrol once, then deliver.
//!
//! # Where the token lives
//!
//! Not in the store. `cyberbrain.toml` sits inside the store, a store is meant to live in a
//! repository, and a credential in it gets committed by the second person who runs
//! `git add .`. The token goes into the user's own configuration directory, one file per
//! hub URL, or into `CYBERBRAIN_HUB_TOKEN` for a service account that would rather not have
//! files.
//!
//! # What "buffering" means here
//!
//! Nothing extra is stored. The store's own audit log *is* the buffer: it holds every row
//! already, in order, and the hub tells us where it stopped. A delivery is a period of that
//! log, and a failed delivery changes nothing — the next one covers the same ground plus
//! whatever happened since.

use cyberbrain_core::{Error, Result};
use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};

/// Env var for a token, for setups that would rather not have a file.
pub const TOKEN_ENV: &str = "CYBERBRAIN_HUB_TOKEN";

/// What `hub add --invite` wrote.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Invitation {
    pub kind: String,
    pub version: u32,
    pub device: String,
    pub name: String,
    pub token: String,
    pub hub_url: Option<String>,
    pub inference_url: Option<String>,
    /// The hub's certificate, SHA-256, when it serves with one of its own making. Absent for
    /// a hub behind a certificate the machine's own trust store already knows, which is the
    /// ordinary case in a company that has a certificate authority.
    #[serde(default)]
    pub hub_cert_sha256: Option<String>,
}

pub fn parse_invitation(text: &str) -> Result<Invitation> {
    let inv: Invitation = serde_json::from_str(text)
        .map_err(|e| Error::Config(format!("not an invitation file: {e}")))?;
    if inv.kind != "cyberbrain.hub.invitation" {
        return Err(Error::Config(format!(
            "file says it is {:?}, not an invitation",
            inv.kind
        )));
    }
    // Version 2 added the pin, and a version 1 file is still a valid invitation: it simply
    // has no pin in it. Refusing what we can read would mean upgrading every hub and every
    // machine on the same afternoon.
    if inv.version == 0 || inv.version > 2 {
        return Err(Error::Config(format!(
            "invitation version {} is newer than this program understands; upgrade it",
            inv.version
        )));
    }
    if inv.hub_url.is_none() {
        return Err(Error::Config(
            "the invitation names no hub address; ask for one issued with --hub-url".into(),
        ));
    }
    Ok(inv)
}

/// The user's configuration directory, where tokens, pins and pull positions live.
fn config_base() -> Option<PathBuf> {
    if cfg!(windows) {
        std::env::var_os("APPDATA").map(PathBuf::from)
    } else {
        std::env::var_os("XDG_CONFIG_HOME")
            .map(PathBuf::from)
            .or_else(|| std::env::var_os("HOME").map(|h| PathBuf::from(h).join(".config")))
    }
}

/// One file for one store's relationship with a hub, named by a hash so that a URL cannot be
/// turned into a directory traversal.
///
/// Keyed by the hub's address **and** the device. It used to be the address alone, and a
/// machine with two projects enrolled with one hub then had one token file between them: the
/// second enrolment overwrote the first, both stores delivered as the second device, and the
/// hub turned one of the two chains away at its anchor. The pull position and the list of
/// known notes were shared the same way. Without a device, which is a store enrolled before
/// this or a file about the hub itself (its pin), the key is the address alone, as before.
fn hub_file_in(base: &Path, hub_url: &str, device: Option<&str>, ext: &str) -> PathBuf {
    let key = match device {
        Some(d) => format!("{hub_url}\n{d}"),
        None => hub_url.to_string(),
    };
    let name = blake3::hash(key.as_bytes()).to_hex().to_string();
    base.join("cyberbrain")
        .join("hub-tokens")
        .join(format!("{}.{ext}", &name[..32]))
}

/// This store's file if it has one, else the one for the address alone: a store enrolled
/// before files were kept per device finds its state there until it writes its own.
fn existing_in(base: &Path, hub_url: &str, device: Option<&str>, ext: &str) -> PathBuf {
    let own = hub_file_in(base, hub_url, device, ext);
    if device.is_none() || own.exists() {
        own
    } else {
        hub_file_in(base, hub_url, None, ext)
    }
}

fn write_in(
    base: &Path,
    hub_url: &str,
    device: Option<&str>,
    ext: &str,
    text: &str,
) -> Result<PathBuf> {
    let path = hub_file_in(base, hub_url, device, ext);
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent).map_err(|e| Error::Io {
            path: parent.to_path_buf(),
            source: e,
        })?;
    }
    std::fs::write(&path, text).map_err(|e| Error::Io {
        path: path.clone(),
        source: e,
    })?;
    Ok(path)
}

/// The token for this store at this hub: environment first, then the file.
///
/// Environment first so a service can be handed one without touching disk, and so a
/// temporary override does not require moving a file somebody will forget to move back.
pub fn token_for(hub_url: &str, device: Option<&str>) -> Result<String> {
    if let Ok(t) = std::env::var(TOKEN_ENV) {
        let t = t.trim().to_string();
        if !t.is_empty() {
            return Ok(t);
        }
    }
    let base = config_base()
        .ok_or_else(|| Error::Config("no configuration directory to read a token from".into()))?;
    token_in(&base, hub_url, device)
}

fn token_in(base: &Path, hub_url: &str, device: Option<&str>) -> Result<String> {
    let path = existing_in(base, hub_url, device, "token");
    let text = std::fs::read_to_string(&path).map_err(|e| {
        Error::Config(format!(
            "no token for {hub_url}: {} ({e}). Enrol with `cyberbrain hub enrol <invitation>`, \
             or set {TOKEN_ENV}",
            path.display()
        ))
    })?;
    Ok(text.trim().to_string())
}

/// Where the pin for a hub is kept. One per hub rather than per device: it describes the hub's
/// certificate, which is the same for every store that delivers to it.
pub fn pin_path(hub_url: &str) -> Option<PathBuf> {
    Some(hub_file_in(&config_base()?, hub_url, None, "pin"))
}

/// The certificate this machine was invited to expect from a hub, if it was invited with one.
///
/// No environment override, unlike the token. A token is a credential a service account may
/// legitimately be handed without touching disk; a pin decides whether a certificate is
/// believed, and reading that from the environment would be a way to talk a client into
/// trusting something else by setting a variable.
pub fn pin_for(hub_url: &str) -> Option<String> {
    pin_at(&pin_path(hub_url)?)
}

/// The pin in a file, if there is one worth having. An empty file is not a pin: it would
/// otherwise parse as one and fail every delivery with a fingerprint error.
pub fn pin_at(path: &std::path::Path) -> Option<String> {
    let text = std::fs::read_to_string(path).ok()?;
    let text = text.trim().to_string();
    (!text.is_empty()).then_some(text)
}

pub fn save_pin(hub_url: &str, pin: &str) -> Result<PathBuf> {
    let path = pin_path(hub_url)
        .ok_or_else(|| Error::Config("no configuration directory to write a pin to".into()))?;
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent).map_err(|e| Error::Io {
            path: parent.to_path_buf(),
            source: e,
        })?;
    }
    std::fs::write(&path, format!("{pin}\n")).map_err(|e| Error::Io {
        path: path.clone(),
        source: e,
    })?;
    Ok(path)
}

/// Forget a pin, for a hub that has moved behind an ordinary certificate.
///
/// Enrolling again is the moment this is decided, and leaving an old pin in place would mean
/// a machine that keeps expecting a certificate nobody serves any more.
pub fn forget_pin(hub_url: &str) -> Result<()> {
    if let Some(path) = pin_path(hub_url)
        && path.exists()
    {
        std::fs::remove_file(&path).map_err(|e| Error::Io { path, source: e })?;
    }
    Ok(())
}

/// Keep a device's token, under that device: see [`hub_file_in`] for why not under the hub.
pub fn save_token(hub_url: &str, device: &str, token: &str) -> Result<PathBuf> {
    let base = config_base()
        .ok_or_else(|| Error::Config("no configuration directory to write a token to".into()))?;
    save_token_in(&base, hub_url, device, token)
}

fn save_token_in(base: &Path, hub_url: &str, device: &str, token: &str) -> Result<PathBuf> {
    let path = write_in(base, hub_url, Some(device), "token", &format!("{token}\n"))?;
    restrict(&path);
    Ok(path)
}

/// Owner-only, where the platform has such a thing. Best effort: a token that is readable
/// by others is worse than one that is not, but it is not worse than no token at all, and
/// failing the enrolment over file modes would be its own problem.
fn restrict(path: &std::path::Path) {
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let _ = std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o600));
    }
    #[cfg(not(unix))]
    let _ = path;
}

/// This machine's name, for the hub to count seats by: `COMPUTERNAME` on Windows, the kernel's
/// host name on Linux, `HOSTNAME` where that is exported. `None` where there is none, and the
/// hub then counts the device as a machine of its own.
pub fn machine_name() -> Option<String> {
    let raw = std::env::var("COMPUTERNAME")
        .ok()
        .or_else(|| std::fs::read_to_string("/proc/sys/kernel/hostname").ok())
        .or_else(|| std::env::var("HOSTNAME").ok())?;
    super::normalise_machine(&raw)
}

fn machine_headers<'a>(version: &'a str, machine: Option<&'a str>) -> Vec<(&'static str, &'a str)> {
    let mut headers = vec![("x-cyberbrain-version", version)];
    if let Some(m) = machine {
        headers.push(("x-cyberbrain-machine", m));
    }
    headers
}

/// What the hub answered.
#[derive(Debug, Clone, Serialize)]
pub struct Delivered {
    pub accepted: usize,
    pub total_rows: i64,
    pub hub: String,
    /// Notes the hub could not take because another machine had changed them. Empty for
    /// every path but the note delivery. Carried here rather than dropped: a client that
    /// swallows a conflict turns "two people disagree" into "nothing happened".
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub conflicts: Vec<serde_json::Value>,
}

/// Read the hub's reply. Written to be explicit about the three answers that are not
/// failures of ours: nothing new, not collecting, and a gap we can close by sending more.
#[derive(Debug)]
pub enum Reply {
    Ok(Delivered),
    /// The hub is not collecting (licence). Not an error on this side: keep the rows.
    NotCollecting(String),
    /// The hub has rows we did not send. Sending a wider period fixes it.
    Gap {
        expected: String,
    },
    Refused {
        status: u16,
        message: String,
    },
}

/// Where this store remembers how far it has pulled from a hub. Beside the token, keyed
/// the same way, because it is the same relationship: one store, one hub, one position.
pub fn read_cursor(hub_url: &str, device: Option<&str>) -> Option<String> {
    let base = config_base()?;
    std::fs::read_to_string(existing_in(&base, hub_url, device, "cursor"))
        .ok()
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty())
}

pub fn write_cursor(hub_url: &str, device: Option<&str>, cursor: &str) -> Result<()> {
    let Some(base) = config_base() else {
        return Ok(());
    };
    write_in(&base, hub_url, device, "cursor", cursor)
        .map(|_| ())
        .map_err(|e| Error::Config(format!("cannot record the pull position: {e}")))
}

/// What this store last knew the hub to hold, per note. This is what fills `based_on`, and
/// without it every second machine's delivery looks like a conflict.
pub fn known_path(hub_url: &str, device: Option<&str>) -> Option<PathBuf> {
    Some(hub_file_in(&config_base()?, hub_url, device, "known.json"))
}

pub fn read_known(
    hub_url: &str,
    device: Option<&str>,
) -> std::collections::BTreeMap<String, String> {
    config_base()
        .and_then(|base| {
            std::fs::read_to_string(existing_in(&base, hub_url, device, "known.json")).ok()
        })
        .and_then(|t| serde_json::from_str(&t).ok())
        .unwrap_or_default()
}

pub fn write_known(
    hub_url: &str,
    device: Option<&str>,
    map: &std::collections::BTreeMap<String, String>,
) -> Result<()> {
    let Some(p) = known_path(hub_url, device) else {
        return Ok(());
    };
    if let Some(dir) = p.parent() {
        let _ = std::fs::create_dir_all(dir);
    }
    std::fs::write(&p, serde_json::to_string(map).unwrap_or_default())
        .map_err(|e| Error::Config(format!("cannot record what the hub holds: {e}")))
}

/// Ask the hub what there is. Changes nothing anywhere; the caller decides what to keep.
pub async fn fetch_from_hub(
    egress: &cyberbrain_policy::Egress,
    actor: &cyberbrain_policy::Actor,
    hub_url: &str,
    token: &str,
    pin: Option<&str>,
    since: Option<&str>,
) -> Result<serde_json::Value> {
    let url = format!("{}/api/v1/fetch", hub_url.trim_end_matches('/'));
    let pin = pin
        .map(cyberbrain_policy::egress::transport::CertificatePin::parse)
        .transpose()?;
    // NoteSync: what comes back is note content, and the register must say so for the
    // direction that carries it, whichever way it flows.
    let ticket = egress.open(actor, cyberbrain_core::EgressPurpose::NoteSync, &url)?;
    let payload = serde_json::json!({ "since": since }).to_string();
    let resp =
        cyberbrain_policy::egress::transport::post_bearer(&ticket, &url, token, &[], payload, pin)
            .await?;
    let body = String::from_utf8_lossy(&resp.body).to_string();
    if resp.status != 200 {
        return Err(Error::Config(format!(
            "the hub refused the fetch ({}): {}",
            resp.status,
            body.trim()
        )));
    }
    serde_json::from_str(&body)
        .map_err(|e| Error::Config(format!("the hub's answer was not a fetch result: {e}")))
}

/// Ask the hub to erase one note. Its own egress purpose, and deliberately not behind
/// `allow_note_sync`: switching sharing off must not also switch off the ability to
/// withdraw what was shared while it was on.
pub async fn erase_at_hub(
    egress: &cyberbrain_policy::Egress,
    actor: &cyberbrain_policy::Actor,
    hub_url: &str,
    token: &str,
    pin: Option<&str>,
    bereich: &str,
    name: &str,
) -> Result<Reply> {
    let url = format!("{}/api/v1/erase", hub_url.trim_end_matches('/'));
    let pin = pin
        .map(cyberbrain_policy::egress::transport::CertificatePin::parse)
        .transpose()?;
    let ticket = egress.open(actor, cyberbrain_core::EgressPurpose::NoteErasure, &url)?;
    let payload = serde_json::json!({ "bereich": bereich, "name": name }).to_string();
    let resp =
        cyberbrain_policy::egress::transport::post_bearer(&ticket, &url, token, &[], payload, pin)
            .await?;
    let body = String::from_utf8_lossy(&resp.body).to_string();
    let json: serde_json::Value = serde_json::from_str(&body).unwrap_or(serde_json::Value::Null);
    let message = json
        .get("error")
        .and_then(|v| v.as_str())
        .unwrap_or(body.trim())
        .to_string();
    Ok(match resp.status {
        200 => Reply::Ok(Delivered {
            accepted: json.get("notes").and_then(|v| v.as_u64()).unwrap_or(0) as usize,
            total_rows: json.get("conflicts").and_then(|v| v.as_i64()).unwrap_or(0),
            hub: hub_url.to_string(),
            conflicts: Vec::new(),
        }),
        503 => Reply::NotCollecting(message),
        status => Reply::Refused { status, message },
    })
}

/// Deliver notes. The same shape as `deliver`, and deliberately a separate function with a
/// separate egress purpose: what leaves here is content, not evidence, and a reader of
/// `cyberbrain policy egress` must be able to tell the two apart.
///
/// The caller selects what to send. This function does not read the store, because the one
/// place that decides which notes a machine offers should be the one place a reviewer has
/// to read — not split between a selector here and a filter somewhere else.
pub async fn deliver_notes(
    egress: &cyberbrain_policy::Egress,
    actor: &cyberbrain_policy::Actor,
    hub_url: &str,
    token: &str,
    pin: Option<&str>,
    version: &str,
    batch: String,
) -> Result<Reply> {
    let url = format!("{}/api/v1/notes", hub_url.trim_end_matches('/'));
    let pin = pin
        .map(cyberbrain_policy::egress::transport::CertificatePin::parse)
        .transpose()?;
    // NoteSync, not AuditSync. The gate refuses this outright unless allow_note_sync is set,
    // so a store enrolled for audit cannot reach this path by accident.
    let ticket = egress.open(actor, cyberbrain_core::EgressPurpose::NoteSync, &url)?;
    let resp = cyberbrain_policy::egress::transport::post_bearer(
        &ticket,
        &url,
        token,
        &[("x-cyberbrain-version", version)],
        batch,
        pin,
    )
    .await?;

    let body = String::from_utf8_lossy(&resp.body).to_string();
    let json: serde_json::Value = serde_json::from_str(&body).unwrap_or(serde_json::Value::Null);
    let message = json
        .get("error")
        .and_then(|v| v.as_str())
        .unwrap_or(body.trim())
        .to_string();
    Ok(match resp.status {
        200 => Reply::Ok(Delivered {
            accepted: json
                .get("accepted")
                .and_then(|v| v.as_u64())
                .unwrap_or_default() as usize,
            // `stored` is how many were newer than what the hub held; the rest passed the
            // rules and changed nothing. Reported as rows so one number means one thing.
            total_rows: json
                .get("stored")
                .and_then(|v| v.as_i64())
                .unwrap_or_default(),
            hub: hub_url.to_string(),
            conflicts: json
                .get("conflicts")
                .and_then(|v| v.as_array())
                .cloned()
                .unwrap_or_default(),
        }),
        503 => Reply::NotCollecting(message),
        status => Reply::Refused { status, message },
    })
}

/// Send one bundle to the hub, through the egress gate.
///
/// The gate is not decoration here: it is what makes this path appear in
/// `cyberbrain policy egress`, what records an audit row for the call, and what refuses a
/// destination that is not the hub this store enrolled with — so an edited config file
/// cannot quietly redirect a company's audit trail somewhere else.
pub async fn deliver(
    egress: &cyberbrain_policy::Egress,
    actor: &cyberbrain_policy::Actor,
    hub_url: &str,
    token: &str,
    // What this machine was invited to expect, from `pin_for`. Passed in rather than read
    // here, for the same reason the token is: this function does what it is given, and the
    // caller is the one place that decides what this machine's credentials are.
    pin: Option<&str>,
    version: &str,
    bundle: String,
) -> Result<Reply> {
    let url = format!("{}/api/v1/ingest", hub_url.trim_end_matches('/'));
    let pin = pin
        .map(cyberbrain_policy::egress::transport::CertificatePin::parse)
        .transpose()?;
    let ticket = egress.open(actor, cyberbrain_core::EgressPurpose::AuditSync, &url)?;
    let machine = machine_name();
    let resp = cyberbrain_policy::egress::transport::post_bearer(
        &ticket,
        &url,
        token,
        // Operational chatter, kept out of the bundle: the bundle is evidence and its shape
        // is fixed, while this is what lets the fleet view show an out-of-date client, and the
        // machine name is what seats are counted by.
        &machine_headers(version, machine.as_deref()),
        bundle,
        pin,
    )
    .await?;

    let body = String::from_utf8_lossy(&resp.body).to_string();
    let json: serde_json::Value = serde_json::from_str(&body).unwrap_or(serde_json::Value::Null);
    let message = json
        .get("error")
        .and_then(|v| v.as_str())
        .unwrap_or(body.trim())
        .to_string();

    Ok(match resp.status {
        200 => Reply::Ok(Delivered {
            accepted: json
                .get("accepted")
                .and_then(|v| v.as_u64())
                .unwrap_or_default() as usize,
            total_rows: json
                .get("total_rows")
                .and_then(|v| v.as_i64())
                .unwrap_or_default(),
            hub: hub_url.to_string(),
            conflicts: Vec::new(),
        }),
        503 => Reply::NotCollecting(message),
        409 => Reply::Gap {
            expected: json
                .get("expected_anchor")
                .and_then(|v| v.as_str())
                .unwrap_or_default()
                .to_string(),
        },
        status => Reply::Refused { status, message },
    })
}

/// Put `url` and `device` into a store's config text, keeping everything else byte for byte.
///
/// Not a re-serialisation of the parsed config: that file is documented with comments, and
/// rewriting it from a struct would delete every one of them. Enrolment is rare and this is
/// the one edit it makes, so it edits text.
pub fn set_hub_in_config(text: &str, url: &str, device: &str) -> String {
    let mut out = String::with_capacity(text.len() + 128);
    let mut in_hub = false;
    let mut wrote_url = false;
    let mut wrote_device = false;

    for line in text.lines() {
        let trimmed = line.trim_start();
        if trimmed.starts_with('[') {
            // Leaving the hub section: add whatever it did not already have.
            if in_hub {
                if !wrote_url {
                    out.push_str(&format!("url = \"{url}\"\n"));
                    wrote_url = true;
                }
                if !wrote_device {
                    out.push_str(&format!("device = \"{device}\"\n"));
                    wrote_device = true;
                }
                out.push('\n');
            }
            in_hub = trimmed.starts_with("[hub]");
            out.push_str(line);
            out.push('\n');
            continue;
        }
        if in_hub {
            // A commented-out example is replaced rather than left to confuse the reader.
            let key = trimmed.trim_start_matches('#').trim_start();
            if key.starts_with("url") && key.contains('=') {
                out.push_str(&format!("url = \"{url}\"\n"));
                wrote_url = true;
                continue;
            }
            if key.starts_with("device") && key.contains('=') {
                out.push_str(&format!("device = \"{device}\"\n"));
                wrote_device = true;
                continue;
            }
        }
        out.push_str(line);
        out.push('\n');
    }

    if in_hub {
        if !wrote_url {
            out.push_str(&format!("url = \"{url}\"\n"));
            wrote_url = true;
        }
        if !wrote_device {
            out.push_str(&format!("device = \"{device}\"\n"));
            wrote_device = true;
        }
    }
    if !wrote_url || !wrote_device {
        out.push_str(&format!(
            "\n[hub]\nurl = \"{url}\"\ndevice = \"{device}\"\n"
        ));
    }
    out
}

/// Same, for the inference endpoint the invitation carried.
pub fn set_inference_url(text: &str, url: &str) -> String {
    let mut out = String::with_capacity(text.len() + 64);
    let mut in_inference = false;
    let mut wrote = false;
    for line in text.lines() {
        let trimmed = line.trim_start();
        if trimmed.starts_with('[') {
            if in_inference && !wrote {
                out.push_str(&format!("base_url = \"{url}\"\n"));
                wrote = true;
                out.push('\n');
            }
            in_inference = trimmed.starts_with("[inference]");
            out.push_str(line);
            out.push('\n');
            continue;
        }
        if in_inference {
            let key = trimmed.trim_start_matches('#').trim_start();
            if key.starts_with("base_url") && key.contains('=') {
                out.push_str(&format!("base_url = \"{url}\"\n"));
                wrote = true;
                continue;
            }
        }
        out.push_str(line);
        out.push('\n');
    }
    if in_inference && !wrote {
        out.push_str(&format!("base_url = \"{url}\"\n"));
        wrote = true;
    }
    if !wrote {
        out.push_str(&format!("\n[inference]\nbase_url = \"{url}\"\n"));
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    const SAMPLE: &str = "\
# A comment somebody wrote.
[rings]
resident_cap_tokens = 8192

[hub]
# Set by `cyberbrain hub enrol <invitation>`.
# url = \"https://example.invalid\"
allow_public_hub = false

[policy]
profile = \"eu\"
";

    #[test]
    fn enrolling_sets_the_url_and_keeps_every_comment() {
        let out = set_hub_in_config(SAMPLE, "https://hub.internal:7788", "dev_1");
        assert!(out.contains("# A comment somebody wrote."));
        assert!(out.contains("# Set by `cyberbrain hub enrol <invitation>`."));
        assert!(out.contains("url = \"https://hub.internal:7788\""));
        assert!(out.contains("device = \"dev_1\""));
        assert!(
            out.contains("allow_public_hub = false"),
            "other keys survive"
        );
        assert!(out.contains("profile = \"eu\""), "later sections survive");
        // The commented example is gone, not left next to the real value.
        assert!(!out.contains("https://example.invalid"));
        // And it still parses.
        let cfg: toml::Value = toml::from_str(&out).expect("valid toml");
        assert_eq!(
            cfg["hub"]["url"].as_str(),
            Some("https://hub.internal:7788")
        );
    }

    #[test]
    fn enrolling_twice_does_not_duplicate_the_key() {
        let once = set_hub_in_config(SAMPLE, "https://a.internal", "dev_1");
        let twice = set_hub_in_config(&once, "https://b.internal", "dev_2");
        assert_eq!(twice.matches("url = ").count(), 1);
        assert_eq!(twice.matches("device = ").count(), 1);
        assert!(twice.contains("https://b.internal"));
        assert!(!twice.contains("https://a.internal"));
        toml::from_str::<toml::Value>(&twice).expect("valid toml");
    }

    #[test]
    fn a_config_without_a_hub_section_gets_one() {
        let plain = "[rings]\nresident_cap_tokens = 8192\n";
        let out = set_hub_in_config(plain, "https://hub.internal", "dev_9");
        let cfg: toml::Value = toml::from_str(&out).expect("valid toml");
        assert_eq!(cfg["hub"]["url"].as_str(), Some("https://hub.internal"));
        assert_eq!(cfg["rings"]["resident_cap_tokens"].as_integer(), Some(8192));
    }

    #[test]
    fn the_inference_endpoint_from_an_invitation_replaces_the_default() {
        let text = "[inference]\nbase_url = \"http://127.0.0.1:11434/v1\"\ntimeout_ms = 30000\n";
        let out = set_inference_url(text, "http://192.168.1.50:11434/v1");
        let cfg: toml::Value = toml::from_str(&out).expect("valid toml");
        assert_eq!(
            cfg["inference"]["base_url"].as_str(),
            Some("http://192.168.1.50:11434/v1")
        );
        assert_eq!(cfg["inference"]["timeout_ms"].as_integer(), Some(30000));
    }

    #[test]
    fn an_invitation_must_say_which_hub() {
        let without = r#"{"kind":"cyberbrain.hub.invitation","version":1,"device":"d","name":"n","token":"t","hub_url":null,"inference_url":null}"#;
        let err = parse_invitation(without).unwrap_err().to_string();
        assert!(err.contains("names no hub address"), "{err}");
    }

    /// Two projects on one machine, enrolled with one hub as two devices, keep two tokens and
    /// two pull positions.
    #[test]
    fn two_stores_enrolled_with_one_hub_keep_their_own_token_and_position() {
        let dir = tempfile::tempdir().unwrap();
        let (base, hub) = (dir.path(), "https://hub.internal:7788");
        save_token_in(base, hub, "dev_a", "token-a").unwrap();
        save_token_in(base, hub, "dev_b", "token-b").unwrap();
        assert_eq!(token_in(base, hub, Some("dev_a")).unwrap(), "token-a");
        assert_eq!(token_in(base, hub, Some("dev_b")).unwrap(), "token-b");

        write_in(base, hub, Some("dev_a"), "cursor", "cursor-a").unwrap();
        write_in(base, hub, Some("dev_b"), "cursor", "cursor-b").unwrap();
        let read = |d| std::fs::read_to_string(existing_in(base, hub, Some(d), "cursor")).unwrap();
        assert_eq!(
            (read("dev_a"), read("dev_b")),
            ("cursor-a".into(), "cursor-b".into())
        );
    }

    /// A store enrolled before files were kept per device still finds its token, until it
    /// enrols again and gets one of its own.
    #[test]
    fn a_store_enrolled_before_keeps_its_token_until_it_enrols_again() {
        let dir = tempfile::tempdir().unwrap();
        let (base, hub) = (dir.path(), "https://hub.internal:7788");
        write_in(base, hub, None, "token", "old-token\n").unwrap();
        assert_eq!(token_in(base, hub, Some("dev_a")).unwrap(), "old-token");
        save_token_in(base, hub, "dev_a", "new-token").unwrap();
        assert_eq!(token_in(base, hub, Some("dev_a")).unwrap(), "new-token");
    }

    #[test]
    fn a_token_file_is_one_per_hub() {
        let base = std::path::Path::new("/cfg");
        let a = hub_file_in(base, "https://a.internal:7788", None, "token");
        let b = hub_file_in(base, "https://b.internal:7788", None, "token");
        assert_ne!(a, b, "two hubs, two tokens");
        let name = a.file_name().unwrap().to_string_lossy().to_string();
        assert!(
            !name.contains('/') && !name.contains(':'),
            "the file name is a hash, not the URL: {name}"
        );
    }
}
