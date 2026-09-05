//! Status, recall, citation expansion, doctor and scan over HTTP.

use super::error::{ApiError, ApiResult};
use super::extract::{ApiPath, ApiQuery};
use super::notes::{Tree, detail_in_tree, note_from_view};
use super::policy::endpoint_class;
use super::wire::{
    CitationExpansion, DoctorFinding, DoctorReport, DryRun, EmbeddingStatus, Hit, IndexStatus,
    InferenceStatus, PolicyStatus, RecallEcho, RecallParams, RecallResult, ResidentCap, RingCount,
    ScanParams, ScanReport, StatusReport, StoreStatus, UsageParams, UsageReport,
};
use super::{ServeState, blocking};
use crate::app::{RecallRequest, ScanOptions};
use axum::Json;
use axum::extract::State;
use cyberbrain_core::store::{DB_FILE, NOTES_DIR};
use cyberbrain_core::{Ring, slash};
use cyberbrain_llm::Backend;
use cyberbrain_policy::{AuditFilter, ModelRole};
use std::path::Path;
use std::sync::Arc;
use std::time::Instant;

const MAX_N: usize = 100;

// ---------------------------------------------------------------------------------------
// recall

/// `hybrid` unless the index said the semantic half did not run. Both wordings are the
/// index's and `App`'s own; nothing here infers the mode from anything else.
fn mode_of(caveats: &[String]) -> &'static str {
    if caveats
        .iter()
        .any(|c| c.starts_with("semantic search skipped") || c.starts_with("embedder not loaded"))
    {
        "lexical"
    } else {
        "hybrid"
    }
}

pub async fn recall(
    State(st): State<Arc<ServeState>>,
    ApiQuery(p): ApiQuery<RecallParams>,
) -> ApiResult<Json<RecallResult>> {
    let q =
        p.q.map(|q| q.trim().to_string())
            .filter(|q| !q.is_empty())
            .ok_or_else(|| ApiError::bad_request("`q` must not be empty"))?;
    if let Some(n) = p.n
        && !(1..=MAX_N).contains(&n)
    {
        return Err(ApiError::bad_request(format!(
            "`n` must be between 1 and {MAX_N}, not {n}"
        )));
    }
    let ring = p.ring.map(Ring::try_from).transpose()?;
    let started = Instant::now();
    let req = RecallRequest { n: p.n, ring };
    let result = st.app.recall(&q, &req).await?;
    let retrieval = &st.app.config().retrieval;
    let n = p.n.unwrap_or(retrieval.n);
    let mode = mode_of(&result.caveats);
    let mut caveats = result.caveats;
    let app = st.app.clone();
    let core_hits = result.hits;
    let hits = blocking(move || {
        let mut out = Vec::with_capacity(core_hits.len());
        for h in core_hits {
            // The index's hit carries no block index; its citation resolves to one.
            let block_idx = app.recall_id(&h.citation).map(|e| e.block.idx).unwrap_or(0);
            out.push(Hit {
                citation: h.citation,
                note_id: h.note_id,
                note_name: h.note_name,
                ring: h.ring,
                score: h.score,
                text: h.text,
                block_idx,
                sources: if mode == "lexical" {
                    vec!["lexical"]
                } else {
                    Vec::new()
                },
            });
        }
        Ok(out)
    })
    .await?;
    if mode == "hybrid" && !hits.is_empty() {
        caveats.push(
            "per-hit candidate sources are not reported by the index; `sources` is empty for a hybrid result"
                .into(),
        );
    }
    Ok(Json(RecallResult {
        hits,
        conflicts: result.conflicts,
        caveats,
        mode,
        elapsed_ms: started.elapsed().as_secs_f64() * 1000.0,
        params: RecallEcho {
            q,
            n,
            ring,
            k_lex: retrieval.k_lex,
            k_sem: retrieval.k_sem,
        },
    }))
}

pub async fn expand(
    State(st): State<Arc<ServeState>>,
    ApiPath(citation): ApiPath<String>,
) -> ApiResult<Json<CitationExpansion>> {
    blocking(move || {
        let e = st.app.recall_id(&citation)?;
        let cit: cyberbrain_core::Citation = e.citation.parse()?;
        let note = note_from_view(e.note);
        let tree = Tree::load(&st.app)?;
        let loaded = super::notes::load_one(st.app.root(), note);
        Ok(Json(CitationExpansion {
            citation: e.citation,
            ring: cit.ring,
            block_idx: e.block.idx,
            block_text: e.block.text,
            token_count: e.block.token_count,
            note: detail_in_tree(&tree, &loaded, false),
        }))
    })
    .await
}

// ---------------------------------------------------------------------------------------
// status

fn dir_bytes(p: &Path) -> u64 {
    let Ok(rd) = std::fs::read_dir(p) else {
        return 0;
    };
    rd.flatten()
        .map(|e| {
            let path = e.path();
            if path.is_dir() {
                dir_bytes(&path)
            } else {
                std::fs::metadata(&path).map(|m| m.len()).unwrap_or(0)
            }
        })
        .sum()
}

fn file_bytes(p: &Path) -> u64 {
    std::fs::metadata(p).map(|m| m.len()).unwrap_or(0)
}

fn backend_name(b: Backend) -> &'static str {
    match b {
        Backend::Ollama => "ollama",
        Backend::LmStudio => "lm-studio",
        Backend::Pair => "nvidia-pair",
        Backend::Unknown => "unknown",
    }
}

pub async fn status(State(st): State<Arc<ServeState>>) -> ApiResult<Json<StatusReport>> {
    let probed_at = jiff::Timestamp::now();
    let s = st.app.status().await?;
    blocking(move || {
        let app = &st.app;
        let root = app.root().to_path_buf();
        let cfg = app.config();
        let mut caveats: Vec<String> = Vec::new();

        // The tree, for per-ring sizes. Files are authoritative; blocks come from core's
        // splitter, which is the one the index uses.
        let tree = Tree::load(app)?;
        let mut rings: Vec<RingCount> = Ring::ALL
            .iter()
            .map(|r| RingCount {
                ring: *r,
                notes: 0,
                blocks: 0,
                tokens: 0,
                bytes: 0,
            })
            .collect();
        for n in &tree.notes {
            let rc = &mut rings[n.note.front.ring.as_u8() as usize];
            rc.notes += 1;
            rc.blocks += n.blocks.len();
            rc.tokens += n.blocks.iter().map(|b| b.token_count as usize).sum::<usize>();
            rc.bytes += n.size;
        }
        let notes_bytes = dir_bytes(&root.join(NOTES_DIR));
        let db_bytes = file_bytes(&root.join(DB_FILE));
        let models_bytes = dir_bytes(&app.store().models_dir());

        // Staleness as a count: the real scan with no-op writers is the one comparison
        // that can say how many files differ from the index (App::status gives a bool).
        let dry = app.scan(ScanOptions {
            full: false,
            dry_run: true,
        })?;
        let stale_notes = dry.indexed_new + dry.reindexed_changed + dry.dropped_missing_file.len();

        // Orphan vectors and FTS health come from the index's own integrity check, which
        // `doctor` runs; the counts are read out of its findings.
        let doc = app.doctor()?;
        let mut orphan_vectors = 0usize;
        let mut fts_ok = true;
        for f in doc.findings.iter().filter(|f| f.check == "index integrity") {
            if f.detail.contains("vectors without a block") {
                orphan_vectors += f
                    .detail
                    .split_whitespace()
                    .next()
                    .and_then(|n| n.parse::<usize>().ok())
                    .unwrap_or(0);
            }
            if f.detail.contains("FTS") {
                fts_ok = false;
            }
        }
        if !tree.unreadable.is_empty() {
            caveats.push(format!(
                "{} file(s) under notes/ could not be read and are not counted in the ring totals; `doctor` names them",
                tree.unreadable.len()
            ));
        }
        if !doc.checks_run.contains(&"index integrity") {
            caveats.push("index integrity was not checked; orphan_vectors and fts_ok are unmeasured".into());
        }

        let (last_scan, last_full_scan) = {
            let g = st.scans.lock().unwrap_or_else(|p| p.into_inner());
            (g.last_scan, g.last_full_scan)
        };
        let last_full_scan = last_full_scan.or_else(|| {
            app.policy()
                .audit()
                .read(&AuditFilter {
                    action: Some("index.cleared".into()),
                    ..Default::default()
                })
                .ok()
                .and_then(|rows| {
                    rows.iter()
                        .rev()
                        .find(|r| r.detail.get("dry_run") != Some(&serde_json::Value::Bool(true)))
                        .map(|r| r.ts)
                })
        });
        if last_scan.is_none() {
            caveats.push(
                "last_scan is known only for scans run through this server since it started; CLI scans leave no timestamp"
                    .into(),
            );
        }

        // Embedding: the loaded model if there is one, else what the index remembers.
        let cards = app.policy_model_card().cards;
        let embed_card = cards.iter().find(|c| c.role == ModelRole::Embedding);
        let e = &s.embedding;
        let loaded = e.embedder.loaded;
        let profile_id = e
            .embedder
            .profile_id
            .clone()
            .or_else(|| e.index_profile.as_ref().map(|p| p.id.clone()))
            .unwrap_or_else(|| cfg.embedding.profile.clone());
        let dim = e
            .embedder
            .dim
            .or_else(|| e.index_profile.as_ref().map(|p| p.dim))
            .unwrap_or(0);
        let model_hash = embed_card
            .and_then(|c| c.blake3.clone())
            .or_else(|| e.index_profile.as_ref().map(|p| p.model_hash.clone()))
            .unwrap_or_default();
        if !loaded {
            caveats.push(format!(
                "no embedding model is loaded ({}); search is lexical only and the embedding block describes the index's last profile, if any",
                e.embedder.reason.clone().unwrap_or_default()
            ));
        }
        let matches_index = e.matches_index;
        if matches_index.is_none() {
            caveats.push(
                "matches_index is null: nothing to compare (no model loaded or no vectors stored)"
                    .into(),
            );
        }
        caveats.push("model_verified_at: the artefact hash is verified on every load but no timestamp of that load is kept".into());

        // Inference.
        let inf = &s.inference;
        let last_call = app
            .policy()
            .audit()
            .read(&AuditFilter {
                action: Some("inference.call".into()),
                ..Default::default()
            })
            .ok()
            .and_then(|rows| rows.last().map(|r| r.ts));
        let inference = InferenceStatus {
            configured: cfg.inference.model.is_some(),
            base_url: cfg.inference.base_url.clone(),
            endpoint_class: endpoint_class(&cfg.inference.base_url),
            allow_public_endpoint: cfg.inference.allow_public_endpoint,
            model: cfg.inference.model.clone(),
            last_backend: inf
                .probe
                .as_ref()
                .map(|p| backend_name(p.backend))
                .unwrap_or("unknown"),
            last_backend_evidence: inf.probe.as_ref().map(|p| p.evidence.join("; ")),
            last_call,
            reachable: inf.probe.as_ref().map(|p| p.reachable),
            reachable_checked_at: inf.probe.as_ref().map(|_| probed_at),
        };
        if inf.probe.is_none() {
            caveats.push(format!("inference: {}", inf.state));
        }

        Ok(Json(StatusReport {
            version: env!("CARGO_PKG_VERSION"),
            store: StoreStatus {
                path: slash(&root),
                bytes: notes_bytes + db_bytes,
                notes_bytes,
                db_bytes,
                models_bytes,
                notes: s.notes_on_disk,
                blocks: s.index.blocks,
                vectors: s.index.vectors,
                rings,
                resident_cap: ResidentCap {
                    tokens: s.resident_cap,
                    used: s.resident_tokens,
                },
            },
            index: IndexStatus {
                schema_version: s.index.schema_version,
                last_scan,
                last_full_scan,
                stale_notes,
                orphan_vectors,
                dangling_links: s.index.dangling_links,
                fts_ok,
            },
            embedding: EmbeddingStatus {
                profile_id,
                model: embed_card
                    .map(|c| c.name.clone())
                    .unwrap_or_else(|| slash(&e.model_dir)),
                dim,
                pooling: embed_card
                    .and_then(|c| c.pooling.clone())
                    .unwrap_or_else(|| "mean, L2-normalised (SPEC §6.2)".into()),
                backend: "static",
                matches_index,
                model_hash,
                model_verified_at: None,
                loaded,
            },
            inference,
            policy: PolicyStatus {
                profile: s.policy.profile,
                pii_scan: s.policy.pii_scan_active,
                audit_rows: s.audit.rows,
            },
            caveats,
        }))
    })
    .await
}

// ---------------------------------------------------------------------------------------
// doctor, scan

fn doctor_check(check: &str) -> String {
    match check {
        // Both singular on the wire. The fallback below would have made this one
        // `unresolvable-links` while its sibling is `dangling-link`, which is the kind of
        // difference a client discovers by having a switch fall through.
        "dangling links" => "dangling-link".into(),
        "unresolvable links" => "unresolvable-link".into(),
        "ring cap" => "ring-cap".into(),
        "stale index" => "stale-index".into(),
        "index integrity" => "orphan-vector".into(),
        "embedding profile" => "profile-mismatch".into(),
        other => other.replace(' ', "-"),
    }
}

fn doctor_subject(check: &str, detail: &str) -> String {
    match check {
        // The two link checks share a detail shape, so they share an arm. Splitting them
        // when `doctor` gained the second check left the new one falling through to
        // "store", and the operator lost the name of the note the link came from —
        // exactly the column they need to go fix it.
        "dangling links" | "unresolvable links" => {
            detail.split(" links to ").next().unwrap_or("").to_string()
        }
        "unreadable note" | "notes tree" | "retention" => {
            detail.split(": ").next().unwrap_or("").to_string()
        }
        "ring cap" => "r0+r1".into(),
        "stale index" | "index integrity" | "embedding profile" => "index".into(),
        "audit chain" => "audit.db".into(),
        _ => "store".into(),
    }
}

/// Retrieval ledger and model cost in one answer, because a saving without its price is
/// half a number.
pub async fn usage(
    State(st): State<Arc<ServeState>>,
    ApiQuery(params): ApiQuery<UsageParams>,
) -> ApiResult<Json<UsageReport>> {
    // Asked before the blocking half: it is a network call, and it is the one field here
    // that can be stale by the time the page renders, since a model is unloaded on a timer.
    let loaded_models = st.app.loaded_models().await;
    let days = params.days.unwrap_or(30).clamp(1, 365);
    blocking(move || {
        Ok(Json(UsageReport {
            retrieval: st.app.usage_summary(),
            inference: st.app.inference_usage(),
            load: st.app.load_summary(),
            loaded_models,
            days: st.app.usage_by_day(days),
        }))
    })
    .await
}

pub async fn doctor(State(st): State<Arc<ServeState>>) -> ApiResult<Json<DoctorReport>> {
    blocking(move || {
        let r = st.app.doctor()?;
        let findings = r
            .findings
            .into_iter()
            .map(|f| DoctorFinding {
                severity: match f.severity {
                    "error" => "error",
                    "warning" => "warn",
                    _ => "info",
                },
                check: doctor_check(f.check),
                subject: doctor_subject(f.check, &f.detail),
                message: f.detail,
            })
            .collect();
        Ok(Json(DoctorReport {
            ok: r.clean,
            checked_at: jiff::Timestamp::now(),
            findings,
            checks_run: r.checks_run,
        }))
    })
    .await
}

pub async fn scan(
    State(st): State<Arc<ServeState>>,
    ApiQuery(p): ApiQuery<ScanParams>,
    ApiQuery(dry): ApiQuery<DryRun>,
) -> ApiResult<Json<ScanReport>> {
    let full = p.full.unwrap_or(false);
    let dry_run = dry.is_on();
    blocking(move || {
        let r = st.app.scan(ScanOptions { full, dry_run })?;
        if !dry_run {
            let now = jiff::Timestamp::now();
            let mut g = st.scans.lock().unwrap_or_else(|p| p.into_inner());
            g.last_scan = Some(now);
            if full {
                g.last_full_scan = Some(now);
            }
        }
        let mut caveats = Vec::new();
        // `App::scan` counts notes, not blocks. For a full rebuild every block in the
        // index was written; for an incremental scan the per-note block counts are not
        // reported, so the index totals after the scan are shown and named as such.
        let (blocks_written, vectors_written) = (r.index.blocks, r.index.vectors);
        if !full {
            caveats.push(
                "blocks_written and vectors_written are the index totals after the scan; the scan reports how many notes it touched (see detail), not how many blocks".into(),
            );
        }
        if dry_run {
            caveats.push("dry run: nothing was written; counts are what a real run would do".into());
        }
        Ok(Json(ScanReport {
            full,
            elapsed_ms: r.elapsed_ms,
            scanned: r.files_listed,
            changed: r.reindexed_changed + r.revectorised,
            added: r.indexed_new,
            removed: r.dropped_missing_file.len(),
            blocks_written,
            vectors_written,
            dry_run,
            detail: r,
            caveats,
        }))
    })
    .await
}
