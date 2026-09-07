//! The compliance screen (SPEC §12, §13.4): egress register with live state, the audit
//! log, PII findings and pending holds, the retention queue and its dry-run-first apply,
//! the model card, and subject access.

use super::error::{ApiError, ApiResult};
use super::extract::ApiQuery;
use super::notes::{Tree, forget_report, rel_path, render_findings};
use super::wire::{
    AuditPage, AuditParams, AuditRow, DryRun, EgressPath, EgressRegister, ModelCard, NoteRef,
    PiiReport, PiiReportEntry, RetentionApplyReport, RetentionApplyRequest, RetentionEntry,
    RetentionQueue, Searched, Skipped, SubjectAccessHit, SubjectAccessReport, SubjectParams,
};
use super::{ServeState, blocking};
use crate::app::App;
use axum::Json;
use axum::body::Bytes;
use axum::extract::State;
use cyberbrain_core::{Citation, EgressPurpose, NoteId, PiiState, Ring};
use cyberbrain_policy::egress::{Destination, Locality, purpose_name};
use cyberbrain_policy::{AuditEvent, AuditFilter, RetentionStatus};
use serde_json::Value;
use std::sync::Arc;

// ---------------------------------------------------------------------------------------
// egress

/// types.ts `EndpointClass`. The register's own vocabulary has one more class, overlay
/// (`100.64.0.0/10`), which needs its own consent flag exactly like a public address does,
/// so it is shown as public here and the `state` text says which it really is.
pub fn endpoint_class(url: &str) -> &'static str {
    match Destination::parse(url) {
        Ok(d) => match d.literal_locality() {
            Some(Locality::Loopback) => "loopback",
            Some(Locality::Private) => "private",
            Some(Locality::Overlay) | Some(Locality::Public) => "public",
            Some(Locality::NotUnicast) | None => "unresolved",
        },
        Err(_) => "unresolved",
    }
}

fn detail_u64(e: &AuditEvent, key: &str) -> u64 {
    e.detail.get(key).and_then(Value::as_u64).unwrap_or(0)
}

/// The obligation catalogue of the active profile. Straight from the library: the wire
/// shape is the library's `Obligation`, because inventing a second one here is how a field
/// ends up meaning two things.
pub async fn obligations(
    State(st): State<Arc<ServeState>>,
) -> ApiResult<Json<crate::app::ObligationsView>> {
    blocking(move || Ok(Json(st.app.policy_obligations()))).await
}

pub async fn egress(State(st): State<Arc<ServeState>>) -> ApiResult<Json<EgressRegister>> {
    blocking(move || {
        let app = &st.app;
        let cfg = app.config();
        let log = app.policy().audit();
        let all = log.read(&AuditFilter::default())?;
        let since = all
            .first()
            .map(|e| e.ts)
            .unwrap_or_else(jiff::Timestamp::now);
        let refused_total = all.iter().filter(|e| e.action == "policy.refusal").count();
        let mut paths = Vec::new();
        let mut register_text = String::new();
        for entry in app.policy_egress() {
            let name = purpose_name(entry.purpose);
            let prefix = format!("{name}:");
            let permitted: Vec<&AuditEvent> = all
                .iter()
                .filter(|e| e.action == "egress.permitted" && e.subject.starts_with(&prefix))
                .collect();
            let bytes_out_total: u64 = all
                .iter()
                .filter(|e| e.action == "egress.completed" && e.subject.starts_with(&prefix))
                .map(|e| detail_u64(e, "bytes_out"))
                .sum();
            let destination = match entry.purpose {
                EgressPurpose::ModelDownload => cfg
                    .embedding
                    .model_source
                    .clone()
                    .unwrap_or_else(|| "none configured".into()),
                EgressPurpose::LocalInference => cfg.inference.base_url.clone(),
                EgressPurpose::AuditSync => {
                    cfg.hub.url.clone().unwrap_or_else(|| "not enrolled".into())
                }
            };
            register_text.push_str(&format!("{name}|{}|{}\n", entry.data, entry.requires));
            paths.push(EgressPath {
                purpose: entry.purpose,
                description: entry.purpose.describe(),
                destination_class: endpoint_class(&destination),
                destination,
                data: entry.data,
                permitted_by: entry.permitted_by.to_vec(),
                enabled: entry.enabled,
                disabled_reason: (!entry.enabled).then(|| entry.state.clone()),
                last_used: permitted.last().map(|e| e.ts),
                uses_total: permitted.len(),
                bytes_out_total,
                state: entry.state,
                carries_note_content: entry.carries_note_content,
            });
        }
        // blake3 over the compile-time register, through core's own hasher: the statement
        // "purposes not on this list do not exist in the binary" as a checkable token.
        let register_hash = format!(
            "b3:{}",
            Citation::new(Ring::Invariant, NoteId::nil(), 0, &register_text).hex()
        );
        Ok(Json(EgressRegister {
            profile: app.policy().profile(),
            since,
            paths,
            refused_total,
            register_hash,
        }))
    })
    .await
}

// ---------------------------------------------------------------------------------------
// audit

/// types.ts names audit actions in its own vocabulary (`write`, `erase`, ...); the log
/// speaks the policy crate's (`note.write`, `note.erase.completed`, ...). A filter in
/// either vocabulary selects the right rows; the rows themselves carry the real names.
fn action_matches(filter: &str, e: &AuditEvent) -> bool {
    let a = e.action.as_str();
    match filter {
        "write" => a.starts_with("note.write"),
        "erase" => a.starts_with("note.erase"),
        "scan" => a.starts_with("index.") || a == "store.init",
        "model-download" => a.starts_with("egress.") && e.subject.starts_with("model-download:"),
        "inference" => a == "inference.call",
        "policy-refusal" | "egress-refused" => a == "policy.refusal",
        "egress-attempt" => a.starts_with("egress."),
        "retention-apply" => a == "retention.expired",
        "hold-resolved" => a == "note.write.resolved" || a == "note.write.discarded",
        "subject-access" => a == "subject.access",
        other => a == other || a.starts_with(&format!("{other}.")),
    }
}

/// `detail` without the chain block, flattened to one level: nested values become their
/// JSON text, which is what types.ts's `Record<string, scalar>` can carry.
fn flat_detail(e: &AuditEvent) -> serde_json::Map<String, Value> {
    let mut out = serde_json::Map::new();
    if let Value::Object(m) = e.detail_without_chain() {
        for (k, v) in m {
            out.insert(
                k,
                match v {
                    Value::Null | Value::Bool(_) | Value::Number(_) | Value::String(_) => v,
                    other => Value::String(other.to_string()),
                },
            );
        }
    }
    out
}

pub async fn audit(
    State(st): State<Arc<ServeState>>,
    ApiQuery(p): ApiQuery<AuditParams>,
) -> ApiResult<Json<AuditPage>> {
    let limit = p.limit.unwrap_or(100).clamp(1, 1000);
    blocking(move || {
        let log = st.app.policy().audit();
        // `seq` is the row's position in the append-only log. The store's own sequence is
        // AUTOINCREMENT with deletes refused by trigger, so the two are the same number.
        let all = log.read(&AuditFilter::default())?;
        let q = p.q.as_deref().map(str::to_lowercase).filter(|s| !s.is_empty());
        let actor = p.actor.as_deref().filter(|s| !s.is_empty());
        let action = p.action.as_deref().filter(|s| !s.is_empty());
        let mut rows: Vec<AuditRow> = all
            .iter()
            .enumerate()
            .rev()
            .filter(|(_, e)| action.is_none_or(|a| action_matches(a, e)))
            .filter(|(_, e)| actor.is_none_or(|a| e.actor.starts_with(a)))
            .filter(|(_, e)| {
                q.as_deref().is_none_or(|q| {
                    format!("{} {} {} {}", e.actor, e.action, e.subject, e.detail)
                        .to_lowercase()
                        .contains(q)
                })
            })
            .map(|(i, e)| AuditRow {
                seq: i + 1,
                ts: e.ts,
                actor: e.actor.clone(),
                action: e.action.clone(),
                subject: e.subject.clone(),
                detail: flat_detail(e),
            })
            .collect();
        // The CLI's rule (App::policy_audit): an action that matches nothing in a
        // non-empty log is refused with the names that are there, because "no such name"
        // and "that never happened" are opposite answers to the same empty table.
        if rows.is_empty()
            && let Some(a) = action
            && !all.is_empty()
            && q.is_none()
            && actor.is_none()
        {
            let mut present: Vec<&str> = all.iter().map(|e| e.action.as_str()).collect();
            present.sort_unstable();
            present.dedup();
            return Err(ApiError::bad_request(format!(
                "no audit action named {a:?}; the log contains: {}. Refused rather than answered with an empty table: a missing name and a thing that never happened are different answers",
                present.join(", ")
            )));
        }
        let total = rows.len();
        if let Some(b) = p.before {
            rows.retain(|r| r.seq < b);
        }
        let more = rows.len() > limit;
        rows.truncate(limit);
        let next_before = if more { rows.last().map(|r| r.seq) } else { None };
        Ok(Json(AuditPage {
            rows,
            total,
            next_before,
        }))
    })
    .await
}

// ---------------------------------------------------------------------------------------
// pii

pub async fn pii(State(st): State<Arc<ServeState>>) -> ApiResult<Json<PiiReport>> {
    blocking(move || {
        let tree = Tree::load(&st.app)?;
        let scan_enabled = st.app.policy().status().pii_scan_active;
        let mut entries = Vec::new();
        for n in &tree.notes {
            let f = &n.note.front;
            if f.pii == PiiState::None {
                continue;
            }
            let findings = cyberbrain_policy::pii::scan(&n.note.body);
            entries.push(PiiReportEntry {
                note: NoteRef {
                    id: f.id,
                    name: f.name.clone(),
                    ring: f.ring,
                },
                state: f.pii,
                findings: render_findings(&n.note.body, &findings),
                // The state is stamped by the write that set it; the frontmatter keeps no
                // separate review timestamp, so the write's is the nearest true thing.
                reviewed_at: f.pii.was_scanned().then_some(f.updated),
            });
        }
        entries.sort_by(|a, b| {
            let rank = |s: PiiState| match s {
                PiiState::Flagged => 0,
                PiiState::Reviewed => 1,
                PiiState::Unscanned => 2,
                PiiState::None => 3,
            };
            rank(a.state)
                .cmp(&rank(b.state))
                .then(a.note.name.cmp(&b.note.name))
        });
        Ok(Json(PiiReport {
            scan_enabled,
            entries,
            holds: st.holds.list(),
        }))
    })
    .await
}

// ---------------------------------------------------------------------------------------
// retention

fn queue_of(app: &App, apply: bool, dry_run: bool) -> ApiResult<crate::app::RetentionReport> {
    Ok(app.policy_retention(apply, dry_run)?)
}

pub async fn retention(State(st): State<Arc<ServeState>>) -> ApiResult<Json<RetentionQueue>> {
    blocking(move || {
        let r = queue_of(&st.app, false, false)?;
        let entries = r
            .queue
            .items
            .iter()
            .map(|i| {
                let (expires_at, due, invalid) = match &i.status {
                    RetentionStatus::Due { expired_at } => (*expired_at, true, None),
                    RetentionStatus::Pending { expires_at } => (*expires_at, false, None),
                    RetentionStatus::Invalid { reason } => (i.created, false, Some(reason.clone())),
                };
                RetentionEntry {
                    note: NoteRef {
                        id: i.note_id,
                        name: i.name.clone(),
                        ring: i.ring,
                    },
                    retention: i.retention.clone(),
                    expires_at,
                    due,
                    invalid,
                }
            })
            .collect();
        Ok(Json(RetentionQueue {
            entries,
            due: r.queue.due,
            indefinite: r.queue.indefinite,
        }))
    })
    .await
}

pub async fn retention_apply(
    State(st): State<Arc<ServeState>>,
    ApiQuery(dry): ApiQuery<DryRun>,
    body: Bytes,
) -> ApiResult<Json<RetentionApplyReport>> {
    let req: RetentionApplyRequest = if body.iter().all(u8::is_ascii_whitespace) {
        RetentionApplyRequest::default()
    } else {
        serde_json::from_slice(&body)
            .map_err(|e| ApiError::bad_request(format!("request body: {e}")))?
    };
    if let Some(names) = &req.names
        && !names.is_empty()
    {
        // The sweep is one path (SPEC §12.5, §8.2) and it erases what is due, all of it;
        // a subset would need its own path with its own audit shape. Refused rather than
        // quietly widened to everything.
        return Err(ApiError::bad_request(
            "applying retention to a subset (`names`) is not supported: the sweep erases every due note, and a partial sweep is not what the audit row `retention.expired` records; omit `names` to run the sweep, or use DELETE /api/v1/notes/{name} for a single note",
        ));
    }
    blocking(move || {
        let r = queue_of(&st.app, true, dry.is_on())?;
        let root = st.app.root().to_path_buf();
        let mut removed = Vec::new();
        let mut skipped = Vec::new();
        for a in r.applied {
            match a.result {
                Ok(rep) => removed.push(forget_report(rep, rel_path(&root, &a.item.path))),
                Err(reason) => skipped.push(Skipped {
                    name: a.name,
                    reason,
                }),
            }
        }
        for i in &r.queue.items {
            if let RetentionStatus::Invalid { reason } = &i.status {
                skipped.push(Skipped {
                    name: i.name.clone(),
                    reason: format!("retention is invalid and was not applied: {reason}"),
                });
            }
        }
        for u in r.unreadable {
            skipped.push(Skipped {
                name: u.clone(),
                reason: "unreadable note; not evaluated".into(),
            });
        }
        Ok(Json(RetentionApplyReport {
            dry_run: r.dry_run,
            removed,
            skipped,
        }))
    })
    .await
}

// ---------------------------------------------------------------------------------------
// model card

pub async fn model_cards(State(st): State<Arc<ServeState>>) -> ApiResult<Json<Vec<ModelCard>>> {
    blocking(move || {
        let report = st.app.policy_model_card();
        let cards = report
            .cards
            .into_iter()
            .map(|c| {
                let mut limitations = c.limitations.clone();
                limitations.extend(c.notes.iter().cloned());
                ModelCard {
                    role: c.role,
                    name: c.name,
                    source: c.source,
                    license: c.licence.unwrap_or_else(|| "not stated".into()),
                    hash: c.blake3,
                    format: c.format,
                    dim: c.dimension,
                    pooling: c.pooling,
                    bytes: c.size_bytes,
                    intended_use: c.intended_use,
                    limitations: limitations.join("; "),
                    // The hash is verified on every load, but no timestamp of that load is
                    // kept anywhere; `null` rather than "now", which would be a claim.
                    verified_at: None,
                    active: true,
                }
            })
            .collect();
        Ok(Json(cards))
    })
    .await
}

// ---------------------------------------------------------------------------------------
// subject access

pub async fn subject(
    State(st): State<Arc<ServeState>>,
    ApiQuery(p): ApiQuery<SubjectParams>,
) -> ApiResult<Json<SubjectAccessReport>> {
    let q =
        p.q.map(|q| q.trim().to_string())
            .filter(|q| !q.is_empty())
            .ok_or_else(|| ApiError::bad_request("`q` (the identifier) must not be empty"))?;
    blocking(move || {
        let r = st.app.policy_subject(&q)?;
        let tree = Tree::load(&st.app)?;
        let audit_rows = st.app.policy().audit().read(&AuditFilter::default())?.len();
        let mut hits = Vec::new();
        for h in &r.note_hits {
            hits.push(SubjectAccessHit {
                r#where: "block",
                r#ref: h.note_name.clone(),
                citation: Some(h.citation.clone()),
                excerpt: h.excerpt.clone(),
            });
        }
        for a in &r.audit_hits {
            hits.push(SubjectAccessHit {
                r#where: "audit",
                r#ref: format!("audit@{}", a.ts),
                citation: None,
                excerpt: format!(
                    "{} {} {} {}{}",
                    a.ts,
                    a.actor,
                    a.action,
                    a.subject,
                    if a.by_hash {
                        " (earlier request for this identifier)"
                    } else {
                        ""
                    }
                ),
            });
        }
        let mut caveats = r.caveats.clone();
        if r.trimmed > 0 {
            caveats.push(format!(
                "{} block(s) the index proposed did not contain the identifier and were dropped",
                r.trimmed
            ));
        }
        Ok(Json(SubjectAccessReport {
            identifier: r.identifier,
            hits,
            searched: Searched {
                notes: tree.notes.len(),
                blocks: tree.total_blocks(),
                audit_rows,
            },
            caveats,
            response_deadline: r.response_deadline,
        }))
    })
    .await
}
