//! What the hub can tell you: which devices are behaving, and evidence for a period.
//!
//! Two jobs that look similar and are not. The fleet view answers "is this collection
//! working" and is read every day by an administrator. The report answers "what happened
//! between these dates" and is produced rarely, for somebody outside — so it comes out as a
//! file in the same format `verify-export` already checks, rather than as a screen somebody
//! photographs.

use super::store::{Device, HubStore};
use cyberbrain_core::{Error, Result};
use cyberbrain_policy::{AuditEvent, bundle};
use serde::Serialize;

/// How long a device may say nothing before that is worth showing. Two days: long enough to
/// survive a weekend laptop, short enough that a fortnight of silence is never a surprise.
pub const QUIET_AFTER_HOURS: i64 = 48;

/// What is worth a person's attention about one device. Absence of trouble is not listed:
/// a view that prints something for every device teaches people to skim it.
#[derive(Debug, Clone, PartialEq, Serialize, serde::Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum Concern {
    /// Registered, never delivered anything.
    NeverReported,
    /// Nothing heard for longer than the threshold.
    Quiet { hours: i64 },
    /// Its last delivery was turned away, and it has not managed one since.
    Refused { reason: String, at: String },
    /// Running an older version than the hub itself.
    Behind { version: String, hub: String },
}

impl Concern {
    pub fn line(&self) -> String {
        match self {
            Concern::NeverReported => "never reported".to_string(),
            Concern::Quiet { hours } => format!("quiet for {hours} h"),
            Concern::Refused { reason, at } => {
                let short = reason.split(':').next().unwrap_or(reason);
                format!("last delivery refused at {at}: {short}")
            }
            Concern::Behind { version, hub } => format!("version {version}, hub runs {hub}"),
        }
    }
}

#[derive(Debug, Clone, Serialize)]
pub struct FleetRow {
    #[serde(flatten)]
    pub device: Device,
    pub concerns: Vec<Concern>,
}

/// The fleet, with what is wrong first. Sorting by trouble rather than by name is the whole
/// difference between a list somebody reads and a list somebody scrolls past.
pub fn fleet(hub: &HubStore, now: jiff::Timestamp, hub_version: &str) -> Result<Vec<FleetRow>> {
    let mut rows: Vec<FleetRow> = hub
        .devices()?
        .into_iter()
        .map(|d| {
            let concerns = concerns_for(&d, now, hub_version);
            FleetRow {
                device: d,
                concerns,
            }
        })
        .collect();
    rows.sort_by_key(|r| (r.concerns.is_empty(), r.device.name.clone()));
    Ok(rows)
}

fn concerns_for(d: &Device, now: jiff::Timestamp, hub_version: &str) -> Vec<Concern> {
    // A revoked device is not a problem to solve; it is a decision somebody made.
    if !d.is_active() {
        return Vec::new();
    }
    let mut out = Vec::new();
    match &d.last_seen {
        None => out.push(Concern::NeverReported),
        Some(seen) => {
            if let Ok(t) = seen.parse::<jiff::Timestamp>() {
                let hours = (now.as_second() - t.as_second()) / 3_600;
                if hours >= QUIET_AFTER_HOURS {
                    out.push(Concern::Quiet { hours });
                }
            }
        }
    }
    if let (Some(reason), Some(at)) = (&d.last_refusal, &d.last_refusal_at) {
        out.push(Concern::Refused {
            reason: reason.clone(),
            at: at.clone(),
        });
    }
    if let Some(v) = &d.version
        && v != hub_version
        && is_older(v, hub_version)
    {
        out.push(Concern::Behind {
            version: v.clone(),
            hub: hub_version.to_string(),
        });
    }
    out
}

/// Compare two dotted versions numerically. Anything unparseable counts as not older —
/// guessing that a version we cannot read is out of date would nag about every fork and
/// every build somebody labelled by hand.
fn is_older(a: &str, b: &str) -> bool {
    let parts = |s: &str| -> Option<Vec<u32>> {
        s.split('.')
            .map(|p| p.split(['-', '+']).next().unwrap_or(p).parse::<u32>().ok())
            .collect()
    };
    match (parts(a), parts(b)) {
        (Some(x), Some(y)) => x < y,
        _ => false,
    }
}

/// The result of re-checking what the hub holds.
#[derive(Debug, Clone, Serialize)]
pub struct VerifyReport {
    pub devices: Vec<DeviceChain>,
    pub rows: i64,
    pub ok: bool,
}

#[derive(Debug, Clone, Serialize)]
pub struct DeviceChain {
    pub device: String,
    pub name: String,
    pub rows: usize,
    /// `Ok(rows checked)` or the first break.
    pub chain: std::result::Result<usize, String>,
}

/// Re-derive every device's chain from the rows as stored.
///
/// The hub checked each delivery as it arrived, so this asks a different question: is what
/// is on disk *now* still what arrived? That is the question a backup restore, a disk fault
/// or a clever administrator raises, and the append-only triggers are not an answer to it —
/// they stop the database from being asked, not from being replaced.
pub fn verify(hub: &HubStore) -> Result<VerifyReport> {
    let mut devices = Vec::new();
    let mut ok = true;
    for d in hub.devices()? {
        let rows = hub.rows_of(&d.id)?;
        let chain = cyberbrain_policy::verify_chain_from(super::store::GENESIS, &rows)
            .map_err(|e| e.to_string());
        if chain.is_err() {
            ok = false;
        }
        devices.push(DeviceChain {
            device: d.id,
            name: d.name,
            rows: rows.len(),
            chain,
        });
    }
    Ok(VerifyReport {
        devices,
        rows: hub.total_entries()?,
        ok,
    })
}

/// One device's rows for a period, as a bundle — the same format `verify-export` checks and
/// `scripts/verify-audit-export.py` can re-check without this program.
pub fn device_bundle(
    hub: &HubStore,
    device: &str,
    from: Option<&str>,
    to: Option<&str>,
    tool: &str,
) -> Result<(String, usize)> {
    let all = hub.rows_of(device)?;
    let rows: Vec<AuditEvent> = all
        .into_iter()
        .filter(|e| {
            let ts = e.ts.to_string();
            from.is_none_or(|f| ts.as_str() >= f) && to.is_none_or(|t| ts.as_str() <= t)
        })
        .collect();
    let n = rows.len();
    let text = bundle::render(
        &rows,
        from.map(str::to_owned),
        to.map(str::to_owned),
        tool,
        &jiff::Timestamp::now().to_string(),
    );
    Ok((text, n))
}

/// Write a report for a period: one bundle per device plus a summary naming what is in it.
///
/// A directory rather than one file, because a bundle is one chain and the hub holds one per
/// device. Merging them would produce a file that verifies as nothing.
pub fn write_report(
    hub: &HubStore,
    dir: &std::path::Path,
    from: Option<&str>,
    to: Option<&str>,
    tool: &str,
) -> Result<serde_json::Value> {
    std::fs::create_dir_all(dir).map_err(|e| Error::Io {
        path: dir.to_path_buf(),
        source: e,
    })?;

    let mut summary = String::new();
    summary.push_str(&format!(
        "Audit report\nperiod:   {} to {}\nwritten:  {} by {}\n\n",
        from.unwrap_or("the beginning"),
        to.unwrap_or("now"),
        jiff::Timestamp::now(),
        tool
    ));

    let mut files = Vec::new();
    let mut total = 0usize;
    for d in hub.devices()? {
        let (text, n) = device_bundle(hub, &d.id, from, to, tool)?;
        // Every device gets a file, including the ones with nothing in the period: "this
        // machine did nothing that week" is a finding, and its absence would read as an
        // oversight.
        let name = format!("{}.jsonl", d.id);
        std::fs::write(dir.join(&name), &text).map_err(|e| Error::Io {
            path: dir.join(&name),
            source: e,
        })?;
        // Checked as written, so a broken file is found here rather than by the recipient.
        let verdict = match bundle::verify(&text) {
            Ok(r) => format!("chain holds over {} row(s)", r.rows),
            Err(e) => format!("PROBLEM: {e}"),
        };
        summary.push_str(&format!(
            "{:<24} {:<20} {:>6} row(s)  {}\n  file: {}\n",
            d.name, d.id, n, verdict, name
        ));
        total += n;
        files.push(name);
    }
    summary.push_str(&format!("\n{total} row(s) in this period\n"));
    summary.push_str(
        "\nEach file verifies on its own:\n  cyberbrain verify-export <file>\n  \
         python3 scripts/verify-audit-export.py <file>\n",
    );

    let summary_path = dir.join("summary.txt");
    std::fs::write(&summary_path, &summary).map_err(|e| Error::Io {
        path: summary_path.clone(),
        source: e,
    })?;

    Ok(serde_json::json!({
        "directory": dir,
        "files": files,
        "rows": total,
        "summary": summary,
    }))
}

// ---------------------------------------------------------------------------------------
// Disclosure: the only route to activity rows, and it needs two people.

use super::access::{AccessRequest, Denied, Principal, RequestState, Role};

/// Hand out the rows an approved request covers.
///
/// Everything this checks is a separate way the rule could be got round, and each one is its
/// own answer rather than a shared "denied": the auditor role, that the request exists, that
/// somebody else approved it, and that the window is still open. The disclosure is recorded
/// before the rows are written, so a crash halfway leaves the record saying more happened
/// than did — which is the safe direction for a log about who looked at what.
pub fn disclose(
    hub: &HubStore,
    token: Option<&str>,
    request_id: &str,
    dir: &std::path::Path,
    now: jiff::Timestamp,
    tool: &str,
) -> std::result::Result<serde_json::Value, Denied> {
    let who: Principal = hub.principal_for(token, Role::Auditor)?;
    let req: AccessRequest = hub
        .request(request_id)
        .map_err(|e| Denied::NotAuthorised(e.to_string()))?
        .ok_or_else(|| Denied::NotApproved(request_id.to_string()))?;

    match req.state(now) {
        RequestState::Pending => return Err(Denied::NotApproved(req.id)),
        RequestState::Closed => return Err(Denied::WindowClosed(req.id)),
        RequestState::Open => {}
    }
    // The person who asked is the person who may read. A second auditor with the same role
    // is still a different person, and the request names who it was for.
    if req.requester != who.id {
        return Err(Denied::NotAuthorised(format!(
            "request {} was made by {}, not by you",
            req.id, req.requester_name
        )));
    }

    let devices: Vec<String> = match &req.device {
        Some(d) => vec![d.clone()],
        None => hub
            .devices()
            .map_err(|e| Denied::NotAuthorised(e.to_string()))?
            .into_iter()
            .map(|d| d.id)
            .collect(),
    };

    std::fs::create_dir_all(dir)
        .map_err(|e| Denied::NotAuthorised(format!("cannot write to {}: {e}", dir.display())))?;

    let mut files = Vec::new();
    let mut total = 0usize;
    for id in &devices {
        let (text, n) = device_bundle(hub, id, req.from.as_deref(), req.to.as_deref(), tool)
            .map_err(|e| Denied::NotAuthorised(e.to_string()))?;
        let name = format!("{id}.jsonl");
        std::fs::write(dir.join(&name), &text)
            .map_err(|e| Denied::NotAuthorised(format!("cannot write {name}: {e}")))?;
        total += n;
        files.push(name);
    }

    let _ = hub.note_disclosure(&req.id, &who.id, total, &now.to_string());

    Ok(serde_json::json!({
        "request": req.id,
        "auditor": who.name,
        "approved_by": req.approved_by_name,
        "devices": devices.len(),
        "rows": total,
        "files": files,
        "directory": dir,
        "expires_at": req.expires_at,
    }))
}
