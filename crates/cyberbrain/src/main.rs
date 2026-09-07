//! Cyberbrain: cited, trust-tiered, local-first memory for AI coding agents.
//!
//! Original work, copyright 2026 Krynex Labs, licensed FSL-1.1-ALv2.
//! See `docs/SPEC.md` §0 for the clean-room boundary this project is built under.
//!
//! This file is dispatch. Every command is a thin adapter over [`app::App`]; the wiring of
//! the six crates lives in `app.rs` and nowhere else.

mod app;
mod audit_bridge;
mod cli;
mod hook;
mod hostload;
mod hub;
mod import;
mod mcp;
mod render;
mod serve;
mod usage;
mod writers;

use app::{App, AuditView, RecallRequest, ScanOptions, WriteOutcome, WriteRequest};
use clap::Parser;
use cli::{Cli, Command, ExportFormat, PolicyCommand};
use cyberbrain_core::{Error, Result, Ring};
use cyberbrain_policy::{Actor, AuditFilter};
use serde::Serialize;
use std::io::{Read, Write};

/// How the command wants its output.
#[derive(Clone, Copy)]
struct Out {
    json: bool,
    quiet: bool,
}

impl Out {
    fn emit<T: Serialize>(self, value: &T, human: impl FnOnce(&T) -> String) -> Result<()> {
        if self.quiet {
            return Ok(());
        }
        let text = if self.json {
            serde_json::to_string_pretty(value)
                .map_err(|e| Error::Index(format!("report does not serialise: {e}")))?
        } else {
            human(value)
        };
        let mut stdout = std::io::stdout().lock();
        let _ = stdout.write_all(text.as_bytes());
        if !text.ends_with('\n') {
            let _ = stdout.write_all(b"\n");
        }
        Ok(())
    }
}

/// SPEC §8.1: one failure taxonomy, three front ends. Delegates to the core, where the
/// names live beside the exit codes; kept as a crate-local alias so `serve` and `mcp` do
/// not each reach for a different spelling of the same thing.
fn error_code(e: &Error) -> &'static str {
    e.code()
}

fn report_error(e: &Error, json: bool) {
    let code = e.exit_code();
    if json {
        let v = serde_json::json!({
            "error": { "code": error_code(e), "message": e.to_string(), "exit_code": code }
        });
        eprintln!("{v}");
    } else {
        let prefix = if code == 3 { "refused" } else { "error" };
        eprintln!("cyberbrain: {prefix}: {e}");
    }
}

fn runtime() -> Result<tokio::runtime::Runtime> {
    tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .map_err(|e| Error::Index(format!("cannot start the async runtime: {e}")))
}

fn read_stdin() -> Result<String> {
    let mut s = String::new();
    std::io::stdin()
        .read_to_string(&mut s)
        .map_err(|e| Error::Io {
            path: "<stdin>".into(),
            source: e,
        })?;
    Ok(s)
}

fn main() {
    let cli = Cli::parse();
    let out = Out {
        json: cli.json,
        quiet: cli.quiet,
    };
    let code = match run(cli, out) {
        Ok(code) => code,
        Err(e) => {
            report_error(&e, out.json);
            e.exit_code()
        }
    };
    std::process::exit(code);
}

/// Returns the exit code. `Ok(3)` is a policy decision that is not an error: the write was
/// held and printed, and the caller must answer.
fn run(cli: Cli, out: Out) -> Result<i32> {
    match cli.command {
        Command::Init { path } => {
            let root = match path.or(cli.store) {
                Some(p) => p,
                None => std::env::current_dir()
                    .map_err(|e| Error::Io {
                        path: ".".into(),
                        source: e,
                    })?
                    .join(cyberbrain_core::config::DEFAULT_STORE_DIR),
            };
            let r = App::init(&root, &Actor::Operator)?;
            out.emit(&r, render::init)?;
            return Ok(0);
        }
        // SPEC §9.1: a hook never fails the harness. Until it is wired, it stands down
        // audibly on stderr and exits 0 with empty output, which is also what the Windows
        // CI job invoking it through cmd.exe requires.
        Command::Hook { event } => {
            hook::install_never_fail_guard();
            // Not `read_stdin()`: that returns an error on invalid UTF-8, and a hook that
            // errors on a malformed payload is a hook that failed the harness (SPEC §9.1).
            let mut raw = Vec::new();
            let _ = std::io::Read::read_to_end(&mut std::io::stdin(), &mut raw);
            let stdin = String::from_utf8_lossy(&raw);
            // The session says which project it is in; that beats the directory this
            // process happens to have been started in. They agree today, and relying on
            // that would mean reading another project's memory the day they do not.
            let session_cwd = serde_json::from_str::<serde_json::Value>(&stdin)
                .ok()
                .and_then(|v| Some(std::path::PathBuf::from(v.get("cwd")?.as_str()?)));
            let opened = App::open_from(
                cli.store.as_deref(),
                session_cwd.as_deref(),
                Actor::Hook(hook::event_name(event).into()),
            );
            let out = hook::run_with(opened.as_ref().ok(), opened.as_ref().err(), event, &stdin);
            out.emit();
            // Always 0. An unreachable store, a malformed payload and an internal error are
            // all reported through stdout, never through the exit code.
            return Ok(out.exit_code);
        }

        Command::Serve { port, no_open } => {
            let app = std::sync::Arc::new(App::open(cli.store.as_deref(), Actor::Operator)?);
            runtime()?.block_on(serve::serve(app, port, !no_open))?;
            return Ok(0);
        }
        Command::Mcp => {
            let app = std::sync::Arc::new(App::open(cli.store.as_deref(), Actor::Mcp)?);
            runtime()?.block_on(mcp::serve_stdio(app))?;
            return Ok(0);
        }

        // No store either: the hub keeps its own record of other machines' rows, and the
        // notes on this machine are none of its business.
        Command::Hub { ref command } => return run_hub(command, out),

        // No store: the point of this one is that a person who was handed a file can check
        // it with nothing but the binary. Opening a store first would make it useless
        // exactly where it is needed.
        Command::VerifyExport { ref path } => {
            let text = std::fs::read_to_string(path).map_err(|e| Error::Io {
                path: path.clone(),
                source: e,
            })?;
            let report = cyberbrain_policy::bundle::verify(&text)?;
            out.emit(&report, render::verify_export)?;
            return Ok(0);
        }

        _ => {}
    }

    let app = App::open(cli.store.as_deref(), Actor::Operator)?;
    match cli.command {
        Command::Scan { full, dry_run } => {
            let r = app.scan(ScanOptions { full, dry_run })?;
            out.emit(&r, render::scan)?;
        }
        Command::Recall { query, id, n, ring } => {
            if let Some(id) = id {
                let r = app.recall_id(&id)?;
                out.emit(&r, render::expanded)?;
            } else {
                let query = query.ok_or_else(|| {
                    Error::Config("recall needs a query, or --id <citation>".into())
                })?;
                let ring = ring.map(Ring::try_from).transpose()?;
                let req = RecallRequest { n: Some(n), ring };
                let r = runtime()?.block_on(app.recall(&query, &req))?;
                out.emit(&r, render::recall)?;
            }
        }
        Command::Find { symbol, limit } => {
            let r = app.find(&symbol, limit)?;
            out.emit(&r, render::find)?;
        }
        Command::Write {
            ring,
            kind,
            name,
            body,
            tags,
            retention,
            force,
            dry_run,
        } => {
            let body = match body {
                Some(b) => b,
                None => read_stdin()?,
            };
            let req = WriteRequest {
                ring: Ring::try_from(ring)?,
                kind: kind.into(),
                name,
                body,
                tags,
                retention,
                force,
                choice: None,
                expected_updated: None,
                dry_run,
            };
            let outcome = app.write(req)?;
            out.emit(&outcome, |o| match o {
                WriteOutcome::Written(w) => render::written(w),
                WriteOutcome::Held { rendered, .. } => format!(
                    "{rendered}Nothing was written. Re-run with --force to write it flagged, \
                     or edit the body.\n"
                ),
                WriteOutcome::Conflict { name, current_updated } => {
                    format!("{name} changed at {current_updated} since it was read; nothing was written\n")
                }
            })?;
            match outcome {
                WriteOutcome::Written(_) => {}
                WriteOutcome::Held { .. } => return Ok(3),
                WriteOutcome::Conflict { .. } => {
                    return Err(Error::StoreIntegrity(
                        "the note changed since it was read".into(),
                    ));
                }
            }
        }
        Command::Forget { target, dry_run } => {
            let r = app.forget(&target, dry_run)?;
            out.emit(&r, cyberbrain_policy::erasure::render)?;
        }
        Command::Import {
            plan,
            accept_pii,
            dry_run,
        } => {
            let mut plan = import::load_plan(&plan)?;
            if accept_pii {
                plan.accept_pii = true;
            }
            let r = import::import(&app, &plan, dry_run)?;
            out.emit(&r, import::render)?;
            // 0 clean, 1 something did not make it, 2 the ledger does not close,
            // 3 only PII holds remain. The ledger failing is an internal error on purpose:
            // it means the importer cannot account for the corpus it just read.
            return Ok(import::exit_code(&r));
        }
        Command::Doctor => {
            let r = app.doctor()?;
            out.emit(&r, render::doctor)?;
        }
        Command::Status => {
            let r = runtime()?.block_on(app.status())?;
            out.emit(&r, render::status)?;
        }
        Command::Export { target, format } => {
            let r = app.export(&target)?;
            match format {
                ExportFormat::Md => out.emit(&r, |n| {
                    cyberbrain_core::frontmatter::render(&n.front, &n.body)
                        .unwrap_or_else(|e| format!("cannot render: {e}"))
                })?,
                ExportFormat::Json => Out { json: true, ..out }.emit(&r, render::note)?,
            }
        }
        Command::Policy { command } => return run_policy(&app, command, out),
        Command::Init { .. }
        | Command::Hook { .. }
        | Command::Serve { .. }
        | Command::Mcp
        | Command::Hub { .. }
        | Command::VerifyExport { .. } => {
            unreachable!("handled before the store was opened")
        }
    }
    Ok(0)
}

/// The hub's commands. None of them opens a store.
fn run_hub(command: &cli::HubCommand, out: Out) -> Result<i32> {
    use cli::HubCommand;
    let now = || jiff::Timestamp::now().to_string();

    match command {
        HubCommand::Serve { addr, data } => {
            let path = hub::data_path(data.clone());
            let store = hub::HubStore::open(&path)?;
            let addr = hub::parse_addr(addr)?;
            let state = std::sync::Arc::new(hub::api::HubState {
                hub: std::sync::Mutex::new(store),
            });
            runtime()?.block_on(async move {
                let listener = tokio::net::TcpListener::bind(addr)
                    .await
                    .map_err(|e| Error::Config(format!("cannot bind {addr}: {e}")))?;
                let bound = listener
                    .local_addr()
                    .map_err(|e| Error::Config(format!("cannot read the bound address: {e}")))?;
                println!(
                    "cyberbrain hub: http://{bound}/  (record: {}; devices authenticate with \
                     a bearer token)",
                    path.display()
                );
                axum::serve(listener, hub::api::router(state))
                    .await
                    .map_err(|e| Error::Config(format!("hub: {e}")))
            })?;
            Ok(0)
        }

        HubCommand::Add { name, data } => {
            let store = hub::HubStore::open(&hub::data_path(data.clone()))?;
            let (device, token) = store.add_device(name, &now())?;
            out.emit(
                &serde_json::json!({ "device": device, "token": token }),
                |v| {
                    format!(
                        "device {} registered as {:?}\ntoken: {}\n\nThis is the only time the \
                         token is shown. The record keeps a hash of it.\n",
                        v["device"]["id"].as_str().unwrap_or_default(),
                        v["device"]["name"].as_str().unwrap_or_default(),
                        v["token"].as_str().unwrap_or_default()
                    )
                },
            )?;
            Ok(0)
        }

        HubCommand::Fleet { data } => {
            let store = hub::HubStore::open(&hub::data_path(data.clone()))?;
            let devices = store.devices()?;
            let total = store.total_entries()?;
            out.emit(
                &serde_json::json!({ "devices": devices, "total_rows": total }),
                |v| {
                    let list = v["devices"].as_array().cloned().unwrap_or_default();
                    if list.is_empty() {
                        return "no devices registered yet; `cyberbrain hub add <name>`\n"
                            .to_string();
                    }
                    let mut s = String::new();
                    for d in list {
                        // The state is what a person scans for, so it comes before the
                        // numbers: a device that never reported and one that was revoked are
                        // both "not sending", for opposite reasons.
                        let state = if d["revoked_at"].is_string() {
                            "revoked"
                        } else if d["last_seen"].is_string() {
                            "seen"
                        } else {
                            "never reported"
                        };
                        s.push_str(&format!(
                            "{:<14} {:<28} {:>8} rows  last seen {}  version {}\n",
                            state,
                            d["name"].as_str().unwrap_or_default(),
                            d["rows"].as_i64().unwrap_or_default(),
                            d["last_seen"].as_str().unwrap_or("never"),
                            d["version"].as_str().unwrap_or("unknown"),
                        ));
                    }
                    s.push_str(&format!(
                        "\n{} row(s) in the record\n",
                        v["total_rows"].as_i64().unwrap_or_default()
                    ));
                    s
                },
            )?;
            Ok(0)
        }

        HubCommand::Revoke { id, data } => {
            let store = hub::HubStore::open(&hub::data_path(data.clone()))?;
            let done = store.revoke(id, &now())?;
            out.emit(&serde_json::json!({ "revoked": done, "device": id }), |v| {
                if v["revoked"].as_bool().unwrap_or(false) {
                    format!("{id} may no longer send; its rows are kept\n")
                } else {
                    format!("{id} is unknown or was already revoked\n")
                }
            })?;
            Ok(0)
        }
    }
}

fn run_policy(app: &App, command: PolicyCommand, out: Out) -> Result<i32> {
    match command {
        PolicyCommand::Egress => {
            let r = app.policy_egress();
            out.emit(&r, |e| render::egress(e))?;
        }
        PolicyCommand::Obligations => {
            let r = app.policy_obligations();
            out.emit(&r, render::obligations)?;
        }
        PolicyCommand::Audit {
            limit,
            action,
            subject,
            verify,
            since,
            until,
            export,
        } => {
            let stamp = |s: Option<String>, what: &str| -> Result<Option<jiff::Timestamp>> {
                s.map(|v| {
                    v.parse::<jiff::Timestamp>().map_err(|e| {
                        Error::Config(format!("--{what}: {v:?} is not an RFC 3339 timestamp: {e}"))
                    })
                })
                .transpose()
            };
            let filter = AuditFilter {
                action,
                subject,
                since: stamp(since, "since")?,
                until: stamp(until, "until")?,
                // An export answers "what happened in this period", and a limit silently
                // cutting that short is the one failure an auditor cannot see. The listing
                // keeps its default; the file does not get one unless it was asked for.
                limit: if export.is_some() {
                    limit
                } else {
                    Some(limit.unwrap_or(50))
                },
                ..Default::default()
            };
            if let Some(path) = export {
                let text = app.export_audit_bundle(&filter)?;
                std::fs::write(&path, &text).map_err(|e| Error::Io {
                    path: path.clone(),
                    source: e,
                })?;
                let report = cyberbrain_policy::bundle::verify(&text)?;
                out.emit(&report, |r| {
                    format!(
                        "wrote {} row(s) to {}\nchecked as written: chain holds from anchor {}\n",
                        r.rows,
                        path.display(),
                        &r.anchor[..r.anchor.len().min(12)]
                    )
                })?;
                return Ok(0);
            }
            let format = if out.json {
                cyberbrain_policy::ExportFormat::Json
            } else {
                cyberbrain_policy::ExportFormat::Text
            };
            let r = app.policy_audit(&filter, verify, format)?;
            let code = audit_exit_code(&r);
            out.emit(&r, |v| {
                let mut s = String::new();
                if let Some(ver) = &v.verified {
                    s.push_str(&match ver {
                        Ok(n) => format!("chain verified over {n} rows\n"),
                        Err(e) => format!("CHAIN BROKEN: {e}\n"),
                    });
                }
                s.push_str(&format!(
                    "{} rows shown (ts\tactor\taction\tsubject\tdetail)\n",
                    v.rows
                ));
                s.push_str(&v.rendered);
                s
            })?;
            return Ok(code);
        }
        PolicyCommand::Subject { identifier } => {
            let r = app.policy_subject(&identifier)?;
            out.emit(&r, |r| r.render_markdown())?;
        }
        PolicyCommand::Retention { apply, dry_run } => {
            let r = app.policy_retention(apply, dry_run)?;
            out.emit(&r, render::retention)?;
        }
        PolicyCommand::ModelCard => {
            let r = app.policy_model_card();
            out.emit(&r, |r| render::model_cards(&r.cards, &r.absent))?;
        }
        PolicyCommand::Consent { withdraw } => {
            let r = app.policy_consent(!withdraw)?;
            out.emit(&r, render::consent)?;
        }
    }
    Ok(0)
}

/// The exit code of `policy audit --verify`.
///
/// A broken chain must not exit 0. This is the one command whose whole purpose is to fail
/// when the log was tampered with, and a check that cannot fail a script is decoration:
/// a nightly `cyberbrain policy audit --verify` would have reported success over an edited
/// log. The rows are printed either way, so the evidence is on screen before the process
/// leaves. Reporting commands (`doctor`, plain `audit`) keep exiting 0; findings there are
/// advisory, a broken hash chain is not.
fn audit_exit_code(view: &AuditView) -> i32 {
    match &view.verified {
        Some(Err(_)) => 1,
        // Not asked to verify, or verified and intact.
        _ => 0,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn view(verified: Option<std::result::Result<usize, String>>) -> AuditView {
        AuditView {
            rows: 3,
            verified,
            rendered: String::new(),
        }
    }

    /// Calibrated against the broken state first: this is the case the exit code exists for.
    #[test]
    fn a_broken_chain_exits_non_zero() {
        let broken = view(Some(Err(
            "audit chain broken at row 2: content does not match its hash".into(),
        )));
        assert_eq!(audit_exit_code(&broken), 1);
    }

    #[test]
    fn an_intact_chain_and_an_unverified_listing_both_exit_zero() {
        assert_eq!(audit_exit_code(&view(Some(Ok(3)))), 0);
        assert_eq!(audit_exit_code(&view(None)), 0);
    }
}
