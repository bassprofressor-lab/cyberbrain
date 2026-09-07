//! The facade the CLI and the web UI call. Thin: it holds the configuration, the audit log
//! and the gate, and forwards to the modules with the actor filled in. Every operation that
//! changes state or reveals something records itself; the modules do that, this just wires
//! them.

use crate::audit::{Actor, AuditAction, AuditFilter, AuditLog, AuditSink, ExportFormat};
use crate::config::PolicyConfig;
use crate::egress::{Egress, EgressEntry};
use crate::erasure::{EraseRequest, Eraser, ErasureReport};
use crate::model_card::{ModelCard, ModelInventory};
use crate::pii::{self, Finding};
use crate::profile::{Profile, ProfileExt};
use crate::retention::{self, RetentionItem, RetentionQueue};
use crate::subject::{Identifier, SubjectAccessReport, SubjectSource};
use crate::write_gate::{self, OperatorChoice, ResolvedWrite, WriteVerdict};
use cyberbrain_core::{EgressGate, Frontmatter, Result};
use serde::Serialize;
use serde_json::json;
use std::path::Path;
use std::sync::Arc;

pub struct Policy {
    cfg: PolicyConfig,
    audit: AuditLog,
    egress: Egress,
    actor: Actor,
}

/// What `cyberbrain status` and the compliance screen show about the policy layer.
#[derive(Debug, Clone, Serialize)]
pub struct PolicyStatus {
    pub profile: Profile,
    pub law: &'static str,
    pub pii_scan_active: bool,
    pub egress: Vec<EgressEntry>,
    pub disclaimer: &'static str,
}

impl Policy {
    pub fn new(cfg: PolicyConfig, sink: Arc<dyn AuditSink>, actor: Actor) -> Self {
        let audit = AuditLog::new(sink);
        let egress = Egress::new(cfg.clone(), audit.clone(), actor.clone());
        Self {
            cfg,
            audit,
            egress,
            actor,
        }
    }

    pub fn profile(&self) -> Profile {
        self.cfg.profile
    }

    pub fn config(&self) -> &PolicyConfig {
        &self.cfg
    }

    pub fn audit(&self) -> &AuditLog {
        &self.audit
    }

    pub fn actor(&self) -> &Actor {
        &self.actor
    }

    /// The gate to hand to the inference and download paths.
    pub fn gate(&self) -> Arc<dyn EgressGate> {
        Arc::new(self.egress.clone())
    }

    pub fn egress(&self) -> &Egress {
        &self.egress
    }

    pub fn status(&self) -> PolicyStatus {
        PolicyStatus {
            profile: self.cfg.profile,
            law: self.cfg.profile.law(),
            pii_scan_active: self.cfg.profile.holds_pii_writes(),
            egress: self.egress.register(),
            disclaimer: pii::DISCLAIMER,
        }
    }

    // ---- writes (SPEC §12.4) ----

    /// Scan a body before writing `name`. A hold is recorded (kinds and offsets only).
    pub fn check_write(&self, name: &str, body: &str) -> Result<WriteVerdict> {
        let v = write_gate::check_write(self.cfg.profile, body);
        if let WriteVerdict::Held { findings } = &v {
            self.audit.record(
                &self.actor,
                AuditAction::NoteWriteHeld,
                format!("note:{name}"),
                json!({ "profile": self.cfg.profile, "findings": findings.iter().map(Finding::audit_view).collect::<Vec<_>>() }),
            )?;
        }
        Ok(v)
    }

    /// Apply the operator's choice to a held write and record it.
    pub fn resolve_hold(
        &self,
        name: &str,
        body: &str,
        findings: &[Finding],
        choice: OperatorChoice,
    ) -> Result<ResolvedWrite> {
        let r = write_gate::resolve_hold(body, findings, choice);
        self.audit.record(
            &self.actor,
            AuditAction::NoteWriteResolved,
            format!("note:{name}"),
            json!({ "choice": choice, "pii": r.pii, "redacted": r.redacted, "remaining": r.remaining.len() }),
        )?;
        Ok(r)
    }

    /// Record that a note was written. Call after the file is on disk.
    pub fn record_write(&self, front: &Frontmatter, bytes: usize) -> Result<()> {
        self.audit.record(
            &self.actor,
            AuditAction::NoteWrite,
            format!("note:{}", front.id),
            json!({ "name": front.name, "ring": front.ring, "kind": front.kind, "bytes": bytes, "pii": front.pii, "scanned": front.pii.was_scanned() }),
        )?;
        Ok(())
    }

    // ---- erasure and retention (SPEC §12.2, §12.5) ----

    pub fn forget(&self, eraser: &mut dyn Eraser, req: &EraseRequest) -> Result<ErasureReport> {
        crate::erasure::forget(&self.audit, &self.actor, self.cfg.profile, eraser, req)
    }

    pub fn retention_queue<'a>(
        &self,
        notes: impl IntoIterator<Item = (&'a Frontmatter, &'a Path)>,
    ) -> RetentionQueue {
        retention::evaluate(notes, jiff::Timestamp::now())
    }

    pub fn apply_retention(
        &self,
        eraser: &mut dyn Eraser,
        queue: &RetentionQueue,
        dry_run: bool,
    ) -> Vec<(RetentionItem, Result<ErasureReport>)> {
        retention::apply(
            &self.audit,
            &Actor::Retention,
            self.cfg.profile,
            eraser,
            queue,
            dry_run,
        )
    }

    // ---- access, audit, models (SPEC §12.3, §12.6, §12.7) ----

    pub fn subject_access(
        &self,
        source: &dyn SubjectSource,
        identifier: &str,
    ) -> Result<SubjectAccessReport> {
        let id = Identifier::new(identifier)?;
        crate::subject::subject_access(&self.audit, &self.actor, self.cfg.profile, source, &id)
    }

    pub fn export_audit(&self, filter: &AuditFilter, format: ExportFormat) -> Result<String> {
        self.audit.export(&self.actor, filter, format)
    }

    /// A period as a self-checking bundle (SPEC §12.6). Recorded like any other export.
    pub fn export_audit_bundle(&self, filter: &AuditFilter, tool: &str) -> Result<String> {
        self.audit.export_bundle(&self.actor, filter, tool)
    }

    pub fn verify_audit(&self) -> Result<usize> {
        self.audit.verify()
    }

    pub fn model_cards(&self, inventories: &[&dyn ModelInventory]) -> Vec<ModelCard> {
        inventories.iter().flat_map(|i| i.model_cards()).collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::audit::MemoryAuditSink;
    use cyberbrain_core::{NoteId, NoteKind, PiiState, Ring};

    fn policy(profile: Profile) -> (Policy, Arc<MemoryAuditSink>) {
        let sink = Arc::new(MemoryAuditSink::new());
        (
            Policy::new(
                PolicyConfig {
                    profile,
                    ..Default::default()
                },
                sink.clone(),
                Actor::Cli,
            ),
            sink,
        )
    }

    #[test]
    fn a_held_write_is_recorded_without_the_matched_text() {
        let (p, sink) = policy(Profile::Eu);
        let body = "reach bob@corp.example.org";
        let WriteVerdict::Held { findings } = p.check_write("contacts", body).unwrap() else {
            panic!()
        };
        let r = p
            .resolve_hold("contacts", body, &findings, OperatorChoice::Redact)
            .unwrap();
        assert_eq!(r.body, "reach [redacted:email]");
        assert_eq!(sink.actions(), ["note.write.held", "note.write.resolved"]);
        let log = serde_json::to_string(&sink.rows()).unwrap();
        assert!(!log.contains("bob@"), "{log}");
        assert!(log.contains("\"kind\":\"email\""));
    }

    #[test]
    fn off_writes_nothing_to_the_log_for_a_clean_pass() {
        let (p, sink) = policy(Profile::Off);
        assert!(matches!(
            p.check_write("x", "bob@corp.example.org").unwrap(),
            WriteVerdict::Proceed {
                pii: PiiState::Unscanned,
                ..
            }
        ));
        assert!(sink.is_empty());
    }

    #[test]
    fn record_write_says_whether_a_scan_ran() {
        let (p, sink) = policy(Profile::Eu);
        let f = Frontmatter {
            id: NoteId::generate(),
            name: "n".into(),
            ring: Ring::Knowledge,
            kind: NoteKind::Knowledge,
            created: jiff::Timestamp::now(),
            updated: jiff::Timestamp::now(),
            tags: vec![],
            links: vec![],
            retention: None,
            pii: PiiState::Unscanned,
        };
        p.record_write(&f, 120).unwrap();
        assert_eq!(sink.rows()[0].detail["scanned"], false);
    }

    #[test]
    fn status_and_gate() {
        let (p, _) = policy(Profile::Ch);
        let s = p.status();
        assert_eq!(s.profile, Profile::Ch);
        assert!(s.pii_scan_active);
        assert_eq!(s.egress.len(), 2);
        assert!(s.disclaimer.contains("seatbelt"));
        let g = p.gate();
        assert!(
            g.permit(
                cyberbrain_core::EgressPurpose::LocalInference,
                "http://127.0.0.1:11434/v1"
            )
            .is_ok()
        );
        assert_eq!(p.verify_audit().unwrap(), 1);
    }
}
