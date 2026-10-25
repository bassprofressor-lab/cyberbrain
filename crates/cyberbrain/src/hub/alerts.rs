//! Telling the hub's operators when something needs them.
//!
//! A hub that waits for somebody to open its page is a hub whose countersignatures wait a
//! month, whose licence runs out on a Saturday, and whose quiet laptops are noticed at the
//! audit. This posts what needs a person to the one webhook the administrator set, and to
//! nothing else.
//!
//! # What it is not
//!
//! Not telemetry: nothing is sent until an administrator sets a URL (`hub alerts set`), and
//! then only there. The request goes through the egress gate like every other path
//! (`EgressPurpose::HubAlert`): that URL and no other, no redirects, no proxy, loopback or
//! private range unless the administrator allowed a public address. What it carries is the
//! state the administrator's page already shows (counts, kinds, device names), never a
//! note and never an activity row. Every attempt, sent or failed, is in the hub's own chain.
//!
//! # How often
//!
//! The hub looks every [`INTERVAL`]. A condition is posted when it first appears and again
//! every [`REMIND_AFTER_HOURS`] while it lasts; one that has gone away is forgotten, so it is
//! posted again if it comes back.

use super::store::HubStore;
use super::{LicenceState, report};
use cyberbrain_core::{EgressPurpose, Error, Result};
use serde::Serialize;

/// The webhook, as the administrator set it.
pub const URL_SETTING: &str = "alert_webhook";
/// "1" when the webhook may be on a public address.
pub const PUBLIC_SETTING: &str = "alert_allow_public";
/// When each condition was last posted, under this prefix and its key.
const SENT_PREFIX: &str = "alert_sent:";

/// How often the running hub looks.
pub const INTERVAL: std::time::Duration = std::time::Duration::from_secs(15 * 60);
/// How long a condition that lasts waits before it is posted again.
pub const REMIND_AFTER_HOURS: i64 = 24;
/// How far back failed governance calls count.
pub const GOVERNANCE_WINDOW_HOURS: i64 = 24;

/// One thing that needs a person.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct Alert {
    /// `countersignature`, `licence`, `quiet-device`, `governance`.
    pub kind: &'static str,
    /// What identifies this condition across rounds, so it is posted once and then reminded.
    pub key: String,
    /// One line a person reads in a chat.
    pub text: String,
}

/// The webhook settings. `url` is `None` while alerts are off, which is the default.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct Settings {
    pub url: Option<String>,
    pub allow_public: bool,
}

pub fn settings(hub: &HubStore) -> Result<Settings> {
    Ok(Settings {
        url: hub.setting(URL_SETTING)?.filter(|u| !u.is_empty()),
        allow_public: hub.setting(PUBLIC_SETTING)?.as_deref() == Some("1"),
    })
}

/// Turn alerts on, or point them elsewhere. The URL is checked with the same parser the gate
/// uses, so a URL that would be refused at the first round is refused here instead. The
/// chain records that it was set and the origin, not the URL: a chat webhook's path is
/// usually its secret.
pub fn set(hub: &HubStore, url: &str, allow_public: bool, by: &str, now: &str) -> Result<()> {
    let d = cyberbrain_policy::egress::Destination::parse(url)?;
    hub.set_setting(URL_SETTING, url)?;
    hub.set_setting(PUBLIC_SETTING, if allow_public { "1" } else { "0" })?;
    hub.record(
        by,
        "alerts.configured",
        serde_json::json!({ "destination": d.origin(), "allow_public": allow_public }),
        now,
    )?;
    Ok(())
}

/// Turn alerts off. What was remembered about earlier posts goes too.
pub fn clear(hub: &HubStore, by: &str, now: &str) -> Result<()> {
    hub.clear_setting(URL_SETTING)?;
    hub.clear_setting(PUBLIC_SETTING)?;
    for (key, _) in hub.settings_with_prefix(SENT_PREFIX)? {
        hub.clear_setting(&key)?;
    }
    hub.record(by, "alerts.off", serde_json::json!({}), now)?;
    Ok(())
}

/// Everything that needs a person right now, whether or not it was posted before.
pub fn collect(hub: &HubStore, now: jiff::Timestamp, hub_version: &str) -> Result<Vec<Alert>> {
    let mut out = Vec::new();

    // Countersignatures. One alert per thing waiting, so a reminder names what still waits.
    for d in hub.devices()? {
        for g in hub.grants_for_device(&d.id)? {
            if g.is_pending() {
                out.push(Alert {
                    kind: "countersignature",
                    key: format!("grant:{}", g.id),
                    text: format!(
                        "Grant {} ({} for device {}) waits for a countersignature.",
                        g.id, g.bereich, d.name
                    ),
                });
            }
        }
    }
    for r in hub.requests()? {
        if r.state(now) == super::access::RequestState::Pending {
            out.push(Alert {
                kind: "countersignature",
                key: format!("request:{}", r.id),
                text: format!(
                    "Access request {} by {} waits for a countersignature.",
                    r.id, r.requester_name
                ),
            });
        }
    }
    for p in hub.purges()? {
        if p.is_pending() {
            out.push(Alert {
                kind: "countersignature",
                key: format!("purge:{}", p.id),
                text: format!(
                    "Purge {} (rows before {}) waits for a countersignature.",
                    p.id, p.cutoff
                ),
            });
        }
    }

    // The licence. Keyed by its state and end date, so a renewal is a new condition.
    let licence = LicenceState::read(hub, now);
    let licence_alert = match &licence {
        LicenceState::Valid {
            warning: Some(w),
            valid_until,
            ..
        } => Some((format!("licence:ending:{valid_until}"), w.clone())),
        LicenceState::Valid { .. } => None,
        LicenceState::Expired { valid_until, .. } => {
            Some((format!("licence:expired:{valid_until}"), licence.line()))
        }
        LicenceState::Missing => Some(("licence:missing".to_string(), licence.line())),
        LicenceState::Invalid(_) => Some(("licence:invalid".to_string(), licence.line())),
    };
    if let Some((key, text)) = licence_alert {
        out.push(Alert {
            kind: "licence",
            key,
            text,
        });
    }

    // Devices that went quiet. "Never reported" is left out: a device registered this
    // morning has not had the chance, and the fleet page shows it.
    for row in report::fleet(hub, now, hub_version)? {
        for c in &row.concerns {
            if let report::Concern::Quiet { hours } = c {
                out.push(Alert {
                    kind: "quiet-device",
                    key: format!("quiet:{}", row.device.id),
                    text: format!(
                        "Device {} has not delivered for {hours} h.",
                        row.device.name
                    ),
                });
            }
        }
    }

    // Governance that fails: calls to AgentGuard that did not get an answer, as the devices
    // reported them in their own audit rows.
    let since = now - jiff::SignedDuration::from_hours(GOVERNANCE_WINDOW_HOURS);
    for (id, name, n) in hub.governance_failures_since(&since.to_string())? {
        out.push(Alert {
            kind: "governance",
            key: format!("governance:{id}"),
            text: format!(
                "Device {name}: {n} governance call(s) failed in the last \
                 {GOVERNANCE_WINDOW_HOURS} h."
            ),
        });
    }
    Ok(out)
}

/// The alerts that are due: new, or last posted more than [`REMIND_AFTER_HOURS`] ago.
pub fn due(hub: &HubStore, all: &[Alert], now: jiff::Timestamp) -> Result<Vec<Alert>> {
    let mut out = Vec::new();
    for a in all {
        let last = hub
            .setting(&format!("{SENT_PREFIX}{}", a.key))?
            .and_then(|t| t.parse::<jiff::Timestamp>().ok());
        let fresh = match last {
            None => true,
            Some(t) => (now.as_second() - t.as_second()) >= REMIND_AFTER_HOURS * 3_600,
        };
        if fresh {
            out.push(a.clone());
        }
    }
    Ok(out)
}

/// Remember what was posted, and forget conditions that are gone, so they are posted again
/// should they come back.
pub fn remember(hub: &HubStore, current: &[Alert], posted: &[Alert], now: &str) -> Result<()> {
    for a in posted {
        hub.set_setting(&format!("{SENT_PREFIX}{}", a.key), now)?;
    }
    for (key, _) in hub.settings_with_prefix(SENT_PREFIX)? {
        let k = &key[SENT_PREFIX.len()..];
        if !current.iter().any(|a| a.key == k) {
            hub.clear_setting(&key)?;
        }
    }
    Ok(())
}

/// The body that is posted. `text` at the top level is what Slack, Mattermost and Teams
/// incoming webhooks display; the rest is for anything that reads JSON.
pub fn payload(hub_label: &str, alerts: &[Alert]) -> String {
    let lines: Vec<String> = alerts.iter().map(|a| format!("- {}", a.text)).collect();
    serde_json::json!({
        "kind": "cyberbrain.hub.alert",
        "version": 1,
        "hub": hub_label,
        "text": format!("cyberbrain hub {hub_label}:\n{}", lines.join("\n")),
        "alerts": alerts,
    })
    .to_string()
}

/// Post `alerts` to the webhook in `settings`, through the egress gate. Returns the HTTP
/// status; anything but 2xx is an error.
pub async fn post(settings: &Settings, hub_label: &str, alerts: &[Alert]) -> Result<u16> {
    use cyberbrain_policy::{Actor, AuditLog, Egress, PolicyConfig, egress::Outcome};
    let Some(url) = &settings.url else {
        return Err(Error::Config(
            "alerts are off; `cyberbrain hub alerts set <url>` turns them on".into(),
        ));
    };
    let cfg = PolicyConfig {
        alert_endpoint: Some(url.clone()),
        allow_public_alert: settings.allow_public,
        ..Default::default()
    };
    // The gate needs a log for its decision rows. A hub has no store log of its own; the
    // decision is recorded in the hub's chain by the caller instead, with the outcome.
    let (log, _rows) = AuditLog::in_memory();
    let actor = Actor::System("hub".into());
    let egress = Egress::new(cfg, log, actor.clone());
    let ticket = egress.open(&actor, EgressPurpose::HubAlert, url)?;
    let body = payload(hub_label, alerts);
    let bytes_out = body.len() as u64;
    let sent = cyberbrain_policy::egress::transport::post_json(&ticket, url, &[], body, None).await;
    match sent {
        Ok(resp) if (200..300).contains(&resp.status) => {
            let _ = ticket.close(Outcome::ok(Some(resp.status)).bytes(bytes_out, 0));
            Ok(resp.status)
        }
        Ok(resp) => {
            let _ = ticket.close(Outcome::failed(&format!("HTTP {}", resp.status)));
            Err(Error::Index(format!(
                "the alert webhook answered HTTP {}",
                resp.status
            )))
        }
        Err(e) => {
            let _ = ticket.close(Outcome::failed(&e.to_string()));
            Err(e)
        }
    }
}

/// One round: look, post what is due, record the attempt. Returns how many were posted.
/// Nothing at all happens while alerts are off.
pub async fn round(
    state: &super::api::HubState,
    hub_version: &str,
    now: jiff::Timestamp,
) -> Result<usize> {
    // The record is held to read and released for the request: a webhook that takes its
    // time must not hold up every device's delivery.
    let (settings, current, due) = {
        let hub = state.hub().map_err(Error::Index)?;
        let settings = settings(&hub)?;
        if settings.url.is_none() {
            return Ok(0);
        }
        let current = collect(&hub, now, hub_version)?;
        let due = due(&hub, &current, now)?;
        (settings, current, due)
    };
    let label = format!("port {}", state.port);
    let outcome = if due.is_empty() {
        None
    } else {
        Some(post(&settings, &label, &due).await)
    };
    let hub = state.hub().map_err(Error::Index)?;
    let stamp = now.to_string();
    let kinds: Vec<&str> = due.iter().map(|a| a.kind).collect();
    match outcome {
        None => {
            remember(&hub, &current, &[], &stamp)?;
            Ok(0)
        }
        Some(Ok(status)) => {
            remember(&hub, &current, &due, &stamp)?;
            hub.record(
                "hub",
                "alerts.sent",
                serde_json::json!({ "count": due.len(), "kinds": kinds, "status": status }),
                &stamp,
            )?;
            Ok(due.len())
        }
        // Not remembered as sent, so the next round tries again.
        Some(Err(e)) => {
            hub.record(
                "hub",
                "alerts.failed",
                serde_json::json!({ "count": due.len(), "kinds": kinds, "error": e.to_string() }),
                &stamp,
            )?;
            Err(e)
        }
    }
}

/// The running hub's loop. Started with the server; does nothing while alerts are off.
pub async fn run(state: std::sync::Arc<super::api::HubState>, hub_version: String) {
    // A minute in, so a hub that was just restarted has its devices' first deliveries
    // before anything is judged quiet.
    tokio::time::sleep(std::time::Duration::from_secs(60)).await;
    loop {
        if let Err(e) = round(&state, &hub_version, jiff::Timestamp::now()).await {
            super::service::log(&format!("alerts: {e}"));
        }
        tokio::time::sleep(INTERVAL).await;
    }
}
