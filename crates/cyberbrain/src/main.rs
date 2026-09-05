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
mod import;
mod mcp;
mod render;
mod serve;
mod usage;
mod writers;

use app::{App, RecallRequest, ScanOptions, WriteOutcome, WriteRequest};
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

        Command::Serve { port, .. } => {
            let app = std::sync::Arc::new(App::open(cli.store.as_deref(), Actor::Operator)?);
            runtime()?.block_on(serve::serve(app, port))?;
            return Ok(0);
        }
        Command::Mcp => {
            let app = std::sync::Arc::new(App::open(cli.store.as_deref(), Actor::Mcp)?);
            runtime()?.block_on(mcp::serve_stdio(app))?;
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
        Command::Init { .. } | Command::Hook { .. } | Command::Serve { .. } | Command::Mcp => {
            unreachable!("handled before the store was opened")
        }
    }
    Ok(0)
}

fn run_policy(app: &App, command: PolicyCommand, out: Out) -> Result<i32> {
    match command {
        PolicyCommand::Egress => {
            let r = app.policy_egress();
            out.emit(&r, |e| render::egress(e))?;
        }
        PolicyCommand::Audit {
            limit,
            action,
            subject,
            verify,
        } => {
            let filter = AuditFilter {
                action,
                subject,
                limit: Some(limit),
                ..Default::default()
            };
            let format = if out.json {
                cyberbrain_policy::ExportFormat::Json
            } else {
                cyberbrain_policy::ExportFormat::Text
            };
            let r = app.policy_audit(&filter, verify, format)?;
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
