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
use std::path::PathBuf;

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

/// Where the token for a hub is kept: one file per hub, named after the URL's shape rather
/// than the URL itself, so a path cannot be turned into a directory traversal.
pub fn token_path(hub_url: &str) -> Option<PathBuf> {
    let base = if cfg!(windows) {
        std::env::var_os("APPDATA").map(PathBuf::from)
    } else {
        std::env::var_os("XDG_CONFIG_HOME")
            .map(PathBuf::from)
            .or_else(|| std::env::var_os("HOME").map(|h| PathBuf::from(h).join(".config")))
    }?;
    let name = blake3::hash(hub_url.as_bytes()).to_hex().to_string();
    Some(
        base.join("cyberbrain")
            .join("hub-tokens")
            .join(format!("{}.token", &name[..32])),
    )
}

/// The token for this hub: environment first, then the file.
///
/// Environment first so a service can be handed one without touching disk, and so a
/// temporary override does not require moving a file somebody will forget to move back.
pub fn token_for(hub_url: &str) -> Result<String> {
    if let Ok(t) = std::env::var(TOKEN_ENV) {
        let t = t.trim().to_string();
        if !t.is_empty() {
            return Ok(t);
        }
    }
    let path = token_path(hub_url)
        .ok_or_else(|| Error::Config("no configuration directory to read a token from".into()))?;
    let text = std::fs::read_to_string(&path).map_err(|e| {
        Error::Config(format!(
            "no token for {hub_url}: {} ({e}). Enrol with `cyberbrain hub enrol <invitation>`, \
             or set {TOKEN_ENV}",
            path.display()
        ))
    })?;
    Ok(text.trim().to_string())
}

/// Where the pin for a hub is kept: beside its token, one file per hub, same naming.
pub fn pin_path(hub_url: &str) -> Option<PathBuf> {
    token_path(hub_url).map(|p| p.with_extension("pin"))
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

pub fn save_token(hub_url: &str, token: &str) -> Result<PathBuf> {
    let path = token_path(hub_url)
        .ok_or_else(|| Error::Config("no configuration directory to write a token to".into()))?;
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent).map_err(|e| Error::Io {
            path: parent.to_path_buf(),
            source: e,
        })?;
    }
    std::fs::write(&path, format!("{token}\n")).map_err(|e| Error::Io {
        path: path.clone(),
        source: e,
    })?;
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

/// What the hub answered.
#[derive(Debug, Clone, Serialize)]
pub struct Delivered {
    pub accepted: usize,
    pub total_rows: i64,
    pub hub: String,
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
    let resp = cyberbrain_policy::egress::transport::post_bearer(
        &ticket,
        &url,
        token,
        // Operational chatter, kept out of the bundle: the bundle is evidence and its shape
        // is fixed, while this is what lets the fleet view show an out-of-date client.
        &[("x-cyberbrain-version", version)],
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

    #[test]
    fn a_token_file_is_one_per_hub() {
        let a = token_path("https://a.internal:7788");
        let b = token_path("https://b.internal:7788");
        assert_ne!(a, b, "two hubs, two tokens");
        if let Some(p) = a {
            let name = p.file_name().unwrap().to_string_lossy().to_string();
            assert!(
                !name.contains('/') && !name.contains(':'),
                "the file name is a hash, not the URL: {name}"
            );
        }
    }
}
