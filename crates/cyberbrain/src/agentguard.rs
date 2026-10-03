//! An agent's proposal reported to AgentGuard, and its approval read back (A2, 2026-10-03).
//!
//! `propose` by an agent already waits for a person (`review`). Where a governance service is
//! configured (`[governance] url`, the same one the pre-tool-use hook asks), the proposal is
//! also sent there as an action of type `memory_write`: AgentGuard runs its gate chain over it,
//! the content check included, and opens an approval that an admin decides in the console.
//! `review <name> --from-agentguard` then reads that decision back and accepts on `approved`.
//!
//! The action id is recorded in the proposal's own `note.proposed` row, not taken from the
//! caller: whoever runs `review --from-agentguard` cannot point it at another approval.
//!
//! Requests go through the egress gate (`EgressPurpose::Governance`), one audit row each, and
//! run on a thread of their own with its own runtime, because `propose` is also reached from
//! the MCP server and the daemon, which already run one.

use crate::app::App;
use cyberbrain_core::{EgressPurpose, Error, Result, Ring};
use cyberbrain_policy::Actor;
use cyberbrain_policy::egress::Outcome;
use serde_json::{Value, json};
use std::time::Duration;

/// What AgentGuard answered to a reported proposal.
#[derive(Debug, Clone)]
pub struct Submitted {
    pub action_id: String,
    /// `escalate` when an approval is waiting; `block` when a gate refused it outright.
    pub outcome: String,
    pub approval_status: Option<String>,
}

/// Report a proposal. `Ok(None)` when no governance service is configured.
pub fn submit_memory_write(
    app: &App,
    actor: &Actor,
    name: &str,
    ring: Ring,
    body: &str,
    untrusted: Option<&str>,
) -> Result<Option<Submitted>> {
    let cfg = &app.config().governance;
    let Some(url) = cfg.url.clone() else {
        return Ok(None);
    };
    let key = crate::hook::governance::key().ok_or_else(|| {
        Error::Config(format!(
            "no AgentGuard key: set {} or put one in ~/.config/cyberbrain/agentguard.key",
            crate::hook::governance::KEY_ENV
        ))
    })?;
    // `cb-` and a fresh id: AgentGuard refuses an id it has seen for another action (K4).
    let action_id = format!("cb-{}", cyberbrain_core::NoteId::generate());
    let ts = jiff::Timestamp::now().as_millisecond() as f64 / 1000.0;
    let body = json!({
        "id": action_id,
        "ts": ts,
        "tenant_id": cfg.tenant,
        "agent_id": cfg.agent_id,
        "agent_name": actor.to_string(),
        "action_type": "memory_write",
        "scope": "memory_write",
        "target": format!("note:{name} ring {}", ring.as_u8()),
        "summary": format!(
            "Cyberbrain: {actor} schlaegt {name} fuer Ring {} vor{}",
            ring.as_u8(),
            untrusted.map(|s| format!(" (Quelle {s}, nicht vertrauenswuerdig)")).unwrap_or_default()
        ),
        "risk_hints": { "needs_confirm": true, "untrusted_source": untrusted },
        // Checked by AgentGuard's content gate and dropped before its audit row.
        "content": body,
    })
    .to_string();
    let endpoint = format!("{}/v1/actions", url.trim_end_matches('/'));
    let v = request(app, actor, &endpoint, &key, Some(body), cfg.timeout_ms)?;
    Ok(Some(Submitted {
        action_id,
        outcome: v["outcome"].as_str().unwrap_or("?").to_string(),
        approval_status: v["approval_status"].as_str().map(str::to_string),
    }))
}

/// The approval's status: `pending`, `approved`, `rejected`, `expired`.
pub fn approval_status(app: &App, actor: &Actor, action_id: &str) -> Result<String> {
    let cfg = &app.config().governance;
    let Some(url) = cfg.url.clone() else {
        return Err(Error::Config(
            "no governance service is configured ([governance] url), so there is no approval \
             to read"
                .into(),
        ));
    };
    let key = crate::hook::governance::key().ok_or_else(|| {
        Error::Config(format!(
            "no AgentGuard key: set {}",
            crate::hook::governance::KEY_ENV
        ))
    })?;
    let endpoint = format!(
        "{}/v1/actions/{action_id}/approval?tenant_id={}",
        url.trim_end_matches('/'),
        cfg.tenant
    );
    // Reading a decision is not a tool call: a longer wait than the hook's is fine here.
    let v = request(app, actor, &endpoint, &key, None, cfg.timeout_ms.max(5000))?;
    v["status"]
        .as_str()
        .map(str::to_string)
        .ok_or_else(|| Error::Index("AgentGuard answered without a status".into()))
}

fn request(
    app: &App,
    actor: &Actor,
    endpoint: &str,
    key: &str,
    post: Option<String>,
    timeout_ms: u64,
) -> Result<Value> {
    let ticket = app
        .policy()
        .egress()
        .open(actor, EgressPurpose::Governance, endpoint)?;
    let bearer = format!("Bearer {key}");
    let bytes_out = post.as_ref().map_or(0, |b| b.len() as u64);
    let url = endpoint.to_string();
    let sent = std::thread::scope(|s| {
        s.spawn(|| {
            let rt = tokio::runtime::Builder::new_current_thread()
                .enable_all()
                .build()
                .map_err(|e| Error::Index(format!("cannot start the async runtime: {e}")))?;
            rt.block_on(async {
                let headers = [("authorization", bearer.as_str())];
                let call = async {
                    match post {
                        Some(body) => {
                            cyberbrain_policy::egress::transport::post_json(
                                &ticket, &url, &headers, body, None,
                            )
                            .await
                        }
                        None => {
                            cyberbrain_policy::egress::transport::get_with_headers(
                                &ticket, &url, &headers,
                            )
                            .await
                        }
                    }
                };
                tokio::time::timeout(Duration::from_millis(timeout_ms.max(1)), call)
                    .await
                    .map_err(|_| Error::Index(format!("no answer within {timeout_ms} ms")))?
            })
        })
        .join()
        .map_err(|_| Error::Index("the AgentGuard request thread panicked".into()))?
    });
    let resp = match sent {
        Ok(resp) => {
            let outcome = if resp.status == 200 {
                Outcome::ok(Some(resp.status))
            } else {
                Outcome::failed(&format!("HTTP {}", resp.status))
            };
            ticket.close(outcome.bytes(bytes_out, resp.body.len() as u64))?;
            resp
        }
        Err(e) => {
            ticket.close(Outcome::failed(&e.to_string()))?;
            return Err(e);
        }
    };
    let v: Value = serde_json::from_slice(&resp.body).unwrap_or(Value::Null);
    if resp.status != 200 {
        let detail = v["detail"].as_str().unwrap_or("no detail");
        return Err(Error::Index(format!(
            "AgentGuard: HTTP {}: {detail}",
            resp.status
        )));
    }
    Ok(v)
}
