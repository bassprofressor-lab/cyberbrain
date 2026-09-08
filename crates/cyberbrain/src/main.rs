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
mod install;
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

        // A store is needed for its path, not its contents: the entry this writes names
        // the store so that a desktop client, which starts the process wherever it likes,
        // talks to this project and not to whichever one it lands in.
        Command::Install {
            ref client,
            ref project,
            ref name,
            undo,
            dry_run,
        } => {
            let project = match project {
                Some(p) => p.clone(),
                None => std::env::current_dir().map_err(|e| Error::Io {
                    path: ".".into(),
                    source: e,
                })?,
            };
            let store = match cli.store.as_deref() {
                Some(_) => app::discover_store(cli.store.as_deref())?,
                None => app::discover_store_from(Some(&project))?,
            };
            let opts = install::Options {
                clients: client.iter().copied().map(Into::into).collect(),
                project,
                store,
                name: name.clone(),
                undo,
                dry_run,
                env: install::Env::current(),
            };
            let r = install::run(&opts)?;
            out.emit(&r, render::install)?;
            return Ok(0);
        }

        // No store either: the hub keeps its own record of other machines' rows, and the
        // notes on this machine are none of its business.
        Command::Hub { ref command } => return run_hub(command, cli.store.as_deref(), out),

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
        | Command::Install { .. }
        | Command::Hub { .. }
        | Command::VerifyExport { .. } => {
            unreachable!("handled before the store was opened")
        }
    }
    Ok(0)
}

/// Licence handling. `keygen` and `issue` are the issuer's side; the rest is a customer's.
fn run_licence(command: &cli::LicenceCommand, out: Out) -> Result<i32> {
    use cli::LicenceCommand;

    match command {
        LicenceCommand::Install { path, data } => {
            let text = std::fs::read_to_string(path).map_err(|e| Error::Io {
                path: path.clone(),
                source: e,
            })?;
            // Checked before it is stored: an unreadable licence in the record would turn
            // every later command into the same complaint about a file nobody can fix.
            let signed = hub::licence::parse(&text)?;
            let store = hub::HubStore::open(&hub::data_path(data.clone()))?;
            store.set_licence(&text)?;
            let state = hub::LicenceState::read(&store, jiff::Timestamp::now());
            out.emit(
                &serde_json::json!({
                    "licence": signed.licence(),
                    "state": state.line(),
                    "collecting": state.may_collect(),
                }),
                |v| {
                    format!(
                        "installed: {} — {} seat(s), until {}\n{}\n",
                        v["licence"]["customer"].as_str().unwrap_or_default(),
                        v["licence"]["seats"].as_u64().unwrap_or_default(),
                        v["licence"]["valid_until"].as_str().unwrap_or_default(),
                        v["state"].as_str().unwrap_or_default()
                    )
                },
            )?;
            Ok(0)
        }

        LicenceCommand::Show { data } => {
            let store = hub::HubStore::open(&hub::data_path(data.clone()))?;
            let state = hub::LicenceState::read(&store, jiff::Timestamp::now());
            let devices = store.active_device_count()?;
            out.emit(
                &serde_json::json!({
                    "state": state.line(),
                    "collecting": state.may_collect(),
                    "seats": state.seats(),
                    "devices_in_use": devices,
                }),
                |v| {
                    let mut s = format!("{}\n", v["state"].as_str().unwrap_or_default());
                    if let Some(seats) = v["seats"].as_u64() {
                        s.push_str(&format!(
                            "seats: {} of {} in use\n",
                            v["devices_in_use"].as_u64().unwrap_or_default(),
                            seats
                        ));
                    }
                    s
                },
            )?;
            // Non-zero when the hub is not collecting, so a monitoring check is one line.
            Ok(if state.may_collect() { 0 } else { 1 })
        }

        LicenceCommand::Keygen => {
            let (private, public) = hub::licence::generate_key()?;
            out.emit(
                &serde_json::json!({ "private_key": private, "public_key": public }),
                |v| {
                    format!(
                        "private key (keep it, never commit it, back it up):\n  {}\n\n\
                         public key (belongs in ISSUER_PUBLIC_KEY, needs a rebuild):\n  {}\n\n\
                         Whoever holds the private key can issue licences for this product.\n\
                         Losing it means no new licences; leaking it means anyone can make them.\n",
                        v["private_key"].as_str().unwrap_or_default(),
                        v["public_key"].as_str().unwrap_or_default()
                    )
                },
            )?;
            Ok(0)
        }

        LicenceCommand::Issue {
            key_file,
            customer,
            seats,
            from,
            until,
            out: out_path,
        } => {
            let key = std::fs::read_to_string(key_file)
                .map_err(|e| Error::Io {
                    path: key_file.clone(),
                    source: e,
                })?
                .trim()
                .to_string();
            let now = jiff::Timestamp::now();
            let valid_from = from.clone().unwrap_or_else(|| now.to_string());
            // Parsed here so a typo is caught while issuing, not by the customer's hub.
            for (what, value) in [("--from", &valid_from), ("--until", until)] {
                value.parse::<jiff::Timestamp>().map_err(|e| {
                    Error::Config(format!(
                        "{what}: {value:?} is not an RFC 3339 timestamp: {e}"
                    ))
                })?;
            }
            let licence = hub::licence::Licence {
                version: 1,
                id: format!("lic_{}", cyberbrain_core::NoteId::generate()),
                customer: customer.clone(),
                seats: *seats,
                valid_from,
                valid_until: until.clone(),
                issued_at: now.to_string(),
            };
            let signed = hub::licence::issue(&licence, &key)?;
            let text = signed.render();
            match out_path {
                Some(p) => {
                    std::fs::write(p, &text).map_err(|e| Error::Io {
                        path: p.clone(),
                        source: e,
                    })?;
                    out.emit(&serde_json::json!({ "licence": licence, "path": p }), |v| {
                        format!(
                            "issued {} for {} ({} seats, until {}) -> {}\n",
                            v["licence"]["id"].as_str().unwrap_or_default(),
                            v["licence"]["customer"].as_str().unwrap_or_default(),
                            v["licence"]["seats"].as_u64().unwrap_or_default(),
                            v["licence"]["valid_until"].as_str().unwrap_or_default(),
                            v["path"].as_str().unwrap_or_default()
                        )
                    })?;
                }
                None => print!("{text}"),
            }
            Ok(0)
        }
    }
}

/// The hub's commands. Most run a hub and open no store; `enrol` and `push` are the
/// client's side of the same feature and do open one, which is why the store path comes in
/// here rather than being reached for globally.
/// Register the hub as a Windows service, or control the one that is registered.
///
/// The installer calls `install` with the same defaults, so a customer who ticks the box and
/// an administrator who types the command end up with exactly the same registration.
fn run_hub_service(command: &cli::ServiceCommand, out: Out) -> Result<i32> {
    use cli::ServiceCommand;
    use hub::service;

    match command {
        ServiceCommand::Install { data, addr } => {
            // Checked before anything is registered: a service that will not start because
            // of a typo in an address is diagnosed from services.msc, which is a bad place
            // to find out.
            let parsed = hub::parse_addr(addr)?;
            let path = data
                .clone()
                .unwrap_or_else(|| service::default_data_dir().join("hub.db"));
            if let Some(dir) = path.parent() {
                std::fs::create_dir_all(dir).map_err(|e| Error::Io {
                    path: dir.to_path_buf(),
                    source: e,
                })?;
            }
            let exe = std::env::current_exe()
                .map_err(|e| Error::Config(format!("cannot find this program on disk: {e}")))?;
            service::install(&exe, &path, addr)?;
            let drop =
                service::licence_drop_path(path.parent().unwrap_or(std::path::Path::new(".")));
            out.emit(
                &serde_json::json!({
                    "service": service::SERVICE_NAME,
                    "data": path,
                    "addr": parsed.to_string(),
                    "licence_drop": drop,
                    "state": "running",
                }),
                |v| {
                    format!(
                        "{} registered and started.\nrecord:  {}\nlistens: {}\n\nPut a \
                         licence file at {} and restart the service; without one nothing is \
                         collected.\n",
                        service::DISPLAY_NAME,
                        v["data"].as_str().unwrap_or_default(),
                        v["addr"].as_str().unwrap_or_default(),
                        v["licence_drop"].as_str().unwrap_or_default(),
                    )
                },
            )?;
            Ok(0)
        }
        ServiceCommand::Uninstall => {
            service::uninstall()?;
            // Said out loud because the opposite would be the surprise: removing the
            // software must not remove the evidence it was collecting.
            out.emit(
                &serde_json::json!({ "service": service::SERVICE_NAME, "state": "removed" }),
                |_| {
                    format!(
                        "{} removed. The record and the log are untouched.\n",
                        service::DISPLAY_NAME
                    )
                },
            )?;
            Ok(0)
        }
        ServiceCommand::Start | ServiceCommand::Stop => {
            let start = matches!(command, ServiceCommand::Start);
            service::set_state(start)?;
            out.emit(
                &serde_json::json!({
                    "service": service::SERVICE_NAME,
                    "state": if start { "starting" } else { "stopping" },
                }),
                |v| {
                    format!(
                        "{} {}\n",
                        service::DISPLAY_NAME,
                        v["state"].as_str().unwrap_or("")
                    )
                },
            )?;
            Ok(0)
        }
        ServiceCommand::Status => {
            let state = service::status()?;
            let running = state == "running";
            out.emit(
                &serde_json::json!({ "service": service::SERVICE_NAME, "state": state }),
                |v| {
                    format!(
                        "{} is {}\n",
                        service::DISPLAY_NAME,
                        v["state"].as_str().unwrap_or("")
                    )
                },
            )?;
            // Non-zero when it is not running, so a monitoring check is one line.
            Ok(if running { 0 } else { 1 })
        }
    }
}

fn run_hub(command: &cli::HubCommand, store: Option<&std::path::Path>, out: Out) -> Result<i32> {
    use cli::HubCommand;
    let now = || jiff::Timestamp::now().to_string();

    match command {
        HubCommand::Enrol { invitation } => {
            let text = std::fs::read_to_string(invitation).map_err(|e| Error::Io {
                path: invitation.clone(),
                source: e,
            })?;
            let inv = hub::client::parse_invitation(&text)?;
            let hub_url = inv.hub_url.clone().expect("checked while parsing");

            let app = App::open(store, Actor::Operator)?;
            let token_at = hub::client::save_token(&hub_url, &inv.token)?;
            let inference =
                app.enrol_with_hub(&hub_url, &inv.device, inv.inference_url.as_deref())?;

            out.emit(
                &serde_json::json!({
                    "hub": hub_url,
                    "device": inv.device,
                    "token_stored_at": token_at,
                    "inference_endpoint": inference,
                }),
                |v| {
                    let mut s = format!(
                        "enrolled with {} as {}\ntoken stored at {}\n",
                        v["hub"].as_str().unwrap_or_default(),
                        v["device"].as_str().unwrap_or_default(),
                        v["token_stored_at"].as_str().unwrap_or_default()
                    );
                    if let Some(e) = v["inference_endpoint"].as_str() {
                        s.push_str(&format!("inference endpoint set to {e}\n"));
                    }
                    s.push_str(
                        "\nDelete the invitation file: it carries the token.\n\
                         Deliver with `cyberbrain hub push`, on a timer.\n",
                    );
                    s
                },
            )?;
            Ok(0)
        }

        HubCommand::Push { since } => {
            let app = App::open(store, Actor::Operator)?;
            let since = since
                .as_ref()
                .map(|s| {
                    s.parse::<jiff::Timestamp>().map_err(|e| {
                        Error::Config(format!("--since {s:?} is not an RFC 3339 timestamp: {e}"))
                    })
                })
                .transpose()?;
            let (report, code) = runtime()?.block_on(app.push_to_hub(since))?;
            out.emit(&report, |v| {
                format!("{}\n", v["message"].as_str().unwrap_or_default())
            })?;
            Ok(code)
        }

        HubCommand::Serve { addr, data } => {
            let path = hub::data_path(data.clone());
            let addr = hub::parse_addr(addr)?;
            // Set before anything can go wrong: started by the service control manager there
            // is no console, so a message that only reaches stdout reaches nobody — including
            // the one saying why the thing will not start.
            hub::service::set_log_path(&path);

            let serve: hub::service::Serve = {
                let path = path.clone();
                Box::new(move |stop| {
                    let store = hub::HubStore::open(&path)?;
                    // A licence dropped next to the record is taken on start, so licensing a
                    // hub is copying a file rather than typing a command with a path in it.
                    let dir = path.parent().unwrap_or(std::path::Path::new("."));
                    let note = hub::service::adopt_dropped_licence(&store, dir);
                    // Read once at startup, so whoever starts the hub sees the state without
                    // having to ask a second command.
                    let state = hub::LicenceState::read(&store, jiff::Timestamp::now());
                    let licence_line = state.line();
                    use hub::service::Dropped;
                    match note {
                        Dropped::Installed(m) | Dropped::Problem(m) => hub::service::log(&m),
                        // Silence is fine for a hub that was licensed months ago. For one
                        // that has no licence at all it is the opposite of fine: that is
                        // exactly the reader who needs to know where it looked.
                        Dropped::None if state == hub::LicenceState::Missing => {
                            hub::service::log(&hub::service::where_it_looked(dir));
                        }
                        Dropped::None | Dropped::Unchanged => {}
                    }
                    let path = path.clone();
                    runtime()?.block_on(async move {
                        let listener = tokio::net::TcpListener::bind(addr)
                            .await
                            .map_err(|e| Error::Config(format!("cannot bind {addr}: {e}")))?;
                        let bound = listener.local_addr().map_err(|e| {
                            Error::Config(format!("cannot read the bound address: {e}"))
                        })?;
                        // Built after binding, so the address the page suggests for
                        // invitations is the one actually being listened on rather than the
                        // one that was asked for.
                        let state = std::sync::Arc::new(hub::api::HubState {
                            hub: std::sync::Mutex::new(store),
                            record: path.clone(),
                            port: bound.port(),
                            sessions: Default::default(),
                            flash: std::sync::Mutex::new(None),
                        });
                        let hello = format!(
                            "cyberbrain hub: http://{bound}/  (record: {}; devices \
                             authenticate with a bearer token)",
                            path.display()
                        );
                        println!("{hello}");
                        println!("{licence_line}");
                        hub::service::log(&hello);
                        hub::service::log(&licence_line);
                        // With connect info, because the page and the fleet view are shown
                        // only to the machine the hub runs on.
                        axum::serve(
                            listener,
                            hub::api::router(state)
                                .into_make_service_with_connect_info::<std::net::SocketAddr>(),
                        )
                        // The stop signal arrives on a plain channel from the service
                        // control handler, which is not async and must answer at once.
                        .with_graceful_shutdown(async move {
                            let _ = tokio::task::spawn_blocking(move || stop.recv()).await;
                            hub::service::log("stop requested; closing the listener");
                        })
                        .await
                        .map_err(|e| Error::Config(format!("hub: {e}")))
                    })
                })
            };

            // Started by the service control manager this takes over and returns when the
            // service stops; started from a prompt it comes back false and we carry on as an
            // ordinary console server. One binary, no flag to remember.
            hub::service::set_serve(serve);
            if hub::service::try_dispatch()? {
                return Ok(0);
            }
            // No sender is ever used here, and `_never` holds the other end open so the
            // shutdown future waits rather than firing on a disconnected channel.
            let (_never, stop) = std::sync::mpsc::channel();
            hub::service::run_serve(stop)?;
            Ok(0)
        }

        HubCommand::Service { command } => run_hub_service(command, out),

        HubCommand::Admin { command } => {
            let cli::AdminCommand::Reset { data } = command;
            let store = hub::HubStore::open(&hub::data_path(data.clone()))?;
            store.set_setting("admin_password", "")?;
            out.emit(&serde_json::json!({ "admin": "reset" }), |_| {
                concat!(
                    "The administrator password is cleared. Open the hub's page on this ",
                    "machine to set a new one; from anywhere else it now says the hub has ",
                    "not been set up.\n"
                )
                .to_string()
            })?;
            Ok(0)
        }

        HubCommand::Add {
            name,
            data,
            invite,
            hub_url,
            inference_url,
        } => {
            let store = hub::HubStore::open(&hub::data_path(data.clone()))?;
            // Seats are checked here rather than at delivery time. A device that was allowed
            // to enrol and is then refused every night is the worst of both: it looks
            // registered and collects nothing.
            let state = hub::LicenceState::read(&store, jiff::Timestamp::now());
            match state.seats() {
                None => {
                    return Err(Error::Config(format!(
                        "{}\nNo device can be registered without one.",
                        state.line()
                    )));
                }
                Some(seats) => {
                    let active = store.active_device_count()?;
                    if active >= seats {
                        return Err(Error::Config(format!(
                            "the licence covers {seats} seat(s) and {active} are in use. \
                             Revoke a device that is gone, or extend the licence — its rows \
                             are kept either way."
                        )));
                    }
                }
            }
            let (device, token) = store.add_device(name, &now())?;

            if let Some(path) = invite {
                let invitation = serde_json::json!({
                    "kind": "cyberbrain.hub.invitation",
                    "version": 1,
                    "device": device.id,
                    "name": device.name,
                    "token": token,
                    "hub_url": hub_url,
                    "inference_url": inference_url,
                });
                let text = serde_json::to_string_pretty(&invitation)
                    .map_err(|e| Error::Config(format!("invitation does not serialise: {e}")))?;
                std::fs::write(path, format!("{text}\n")).map_err(|e| Error::Io {
                    path: path.clone(),
                    source: e,
                })?;
                out.emit(&invitation, |v| {
                    format!(
                        "device {} registered as {:?}\ninvitation written to {}\n\n\
                         It carries the token. Hand it over the way you would a password, \
                         and delete it once the machine has been set up.\n{}",
                        v["device"].as_str().unwrap_or_default(),
                        v["name"].as_str().unwrap_or_default(),
                        path.display(),
                        if v["hub_url"].is_null() {
                            "\nNo --hub-url was given, so the device still has to be told \
                             where to deliver.\n"
                        } else {
                            ""
                        }
                    )
                })?;
                return Ok(0);
            }

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

        HubCommand::Licence { command } => run_licence(command, out),

        HubCommand::Fleet { data } => {
            let store = hub::HubStore::open(&hub::data_path(data.clone()))?;
            let version = env!("CARGO_PKG_VERSION");
            let rows = hub::report::fleet(&store, jiff::Timestamp::now(), version)?;
            let total = store.total_entries()?;
            let licence = hub::LicenceState::read(&store, jiff::Timestamp::now());
            let troubled = rows.iter().filter(|r| !r.concerns.is_empty()).count();

            out.emit(
                &serde_json::json!({
                    "devices": rows,
                    "total_rows": total,
                    "needs_attention": troubled,
                    "licence": licence.line(),
                    "collecting": licence.may_collect(),
                }),
                |v| {
                    let list = v["devices"].as_array().cloned().unwrap_or_default();
                    if list.is_empty() {
                        return "no devices registered yet; `cyberbrain hub add <name>`\n"
                            .to_string();
                    }
                    let mut s = String::new();
                    for d in list {
                        let concerns: Vec<String> = d["concerns"]
                            .as_array()
                            .map(|c| {
                                c.iter()
                                    .filter_map(|x| {
                                        serde_json::from_value::<hub::report::Concern>(x.clone())
                                            .ok()
                                    })
                                    .map(|c| c.line())
                                    .collect()
                            })
                            .unwrap_or_default();
                        // The marker is the first thing on the line, so a screen of devices
                        // can be scanned down one column.
                        let marker = if d["revoked_at"].is_string() {
                            "-"
                        } else if concerns.is_empty() {
                            "ok"
                        } else {
                            "!!"
                        };
                        s.push_str(&format!(
                            "{:<3} {:<22} {:>8} rows  {}\n",
                            marker,
                            d["name"].as_str().unwrap_or_default(),
                            d["rows"].as_i64().unwrap_or_default(),
                            if d["revoked_at"].is_string() {
                                "revoked".to_string()
                            } else if concerns.is_empty() {
                                format!("last seen {}", d["last_seen"].as_str().unwrap_or("never"))
                            } else {
                                concerns.join("; ")
                            }
                        ));
                    }
                    s.push_str(&format!(
                        "\n{} row(s) in the record, {} device(s) need attention\n{}\n",
                        v["total_rows"].as_i64().unwrap_or_default(),
                        v["needs_attention"].as_i64().unwrap_or_default(),
                        v["licence"].as_str().unwrap_or_default()
                    ));
                    s
                },
            )?;
            Ok(0)
        }

        HubCommand::Principal { command } => {
            use cli::PrincipalCommand;
            match command {
                PrincipalCommand::Add { name, role, data } => {
                    let store = hub::HubStore::open(&hub::data_path(data.clone()))?;
                    let role = hub::access::Role::parse(role)?;
                    let (who, token) = store.add_principal(name, role, &now())?;
                    out.emit(
                        &serde_json::json!({ "principal": who, "token": token }),
                        |v| {
                            format!(
                                "{} registered as {} ({})\ncredential: {}\n\n\
                                 Shown once; the record keeps a hash. Granting a role is \
                                 itself an entry in the hub's log.\n",
                                v["principal"]["name"].as_str().unwrap_or_default(),
                                v["principal"]["role"].as_str().unwrap_or_default(),
                                v["principal"]["id"].as_str().unwrap_or_default(),
                                v["token"].as_str().unwrap_or_default()
                            )
                        },
                    )?;
                    Ok(0)
                }
                PrincipalCommand::List { data } => {
                    let store = hub::HubStore::open(&hub::data_path(data.clone()))?;
                    let people = store.principals()?;
                    out.emit(&serde_json::json!({ "principals": people }), |v| {
                        let list = v["principals"].as_array().cloned().unwrap_or_default();
                        if list.is_empty() {
                            return "nobody registered yet\n".to_string();
                        }
                        let mut s = String::new();
                        for p in list {
                            s.push_str(&format!(
                                "{:<16} {:<24} {}\n",
                                p["role"].as_str().unwrap_or_default(),
                                p["name"].as_str().unwrap_or_default(),
                                if p["revoked_at"].is_string() {
                                    "revoked"
                                } else {
                                    p["id"].as_str().unwrap_or_default()
                                }
                            ));
                        }
                        s
                    })?;
                    Ok(0)
                }
                PrincipalCommand::Revoke { id, data } => {
                    let store = hub::HubStore::open(&hub::data_path(data.clone()))?;
                    let done = store.revoke_principal(id, &now())?;
                    out.emit(
                        &serde_json::json!({ "revoked": done, "principal": id }),
                        |v| {
                            if v["revoked"].as_bool().unwrap_or(false) {
                                format!("{id} may no longer act\n")
                            } else {
                                format!("{id} is unknown or was already revoked\n")
                            }
                        },
                    )?;
                    Ok(0)
                }
            }
        }

        HubCommand::Request {
            reason,
            device,
            from,
            to,
            as_,
            data,
        } => {
            let store = hub::HubStore::open(&hub::data_path(data.clone()))?;
            let who = store
                .principal_for(as_.as_deref(), hub::access::Role::Auditor)
                .map_err(|d| Error::Config(d.to_string()))?;
            let req = store.create_request(
                &who,
                device.as_deref(),
                from.as_deref(),
                to.as_deref(),
                reason,
                &now(),
            )?;
            let id = req.id.clone();
            out.emit(&req, move |r| {
                format!(
                    "request {} recorded\n\nIt gives access to nothing until somebody else \
                     countersigns it:\n  cyberbrain hub approve {} --as <countersigner>\n",
                    r.id, id
                )
            })?;
            Ok(0)
        }

        HubCommand::Approve {
            request,
            hours,
            as_,
            data,
        } => {
            let store = hub::HubStore::open(&hub::data_path(data.clone()))?;
            let who = store
                .principal_for(as_.as_deref(), hub::access::Role::Countersigner)
                .map_err(|d| Error::Config(d.to_string()))?;
            let expires = (jiff::Timestamp::now()
                + std::time::Duration::from_secs((*hours).max(1) as u64 * 3600))
            .to_string();
            let req = store
                .approve_request(request, &who, &expires, &now())
                .map_err(|d| Error::Config(d.to_string()))?;
            let name = who.name.clone();
            out.emit(&req, move |r| {
                format!(
                    "request {} countersigned by {}\nopen until {}\n",
                    r.id,
                    name,
                    r.expires_at.as_deref().unwrap_or("unknown")
                )
            })?;
            Ok(0)
        }

        HubCommand::Requests { data } => {
            let store = hub::HubStore::open(&hub::data_path(data.clone()))?;
            let reqs = store.requests()?;
            let now_ts = jiff::Timestamp::now();
            let text: String = reqs.iter().map(|r| r.line(now_ts)).collect();
            out.emit(&serde_json::json!({ "requests": reqs }), move |_| {
                if text.is_empty() {
                    "no requests have been made\n".to_string()
                } else {
                    text.clone()
                }
            })?;
            Ok(0)
        }

        HubCommand::Disclose {
            request,
            out_dir,
            as_,
            data,
        } => {
            let store = hub::HubStore::open(&hub::data_path(data.clone()))?;
            let tool = concat!("cyberbrain hub ", env!("CARGO_PKG_VERSION"));
            let result = hub::report::disclose(
                &store,
                as_.as_deref(),
                request,
                out_dir,
                jiff::Timestamp::now(),
                tool,
            )
            .map_err(|d| Error::Config(d.to_string()))?;
            out.emit(&result, |v| {
                format!(
                    "{} row(s) from {} device(s) written to {}\n\n\
                     This disclosure is in the hub's log: request {}, read by {}, approved \
                     by {}.\n",
                    v["rows"].as_i64().unwrap_or_default(),
                    v["devices"].as_i64().unwrap_or_default(),
                    v["directory"].as_str().unwrap_or_default(),
                    v["request"].as_str().unwrap_or_default(),
                    v["auditor"].as_str().unwrap_or_default(),
                    v["approved_by"].as_str().unwrap_or("nobody")
                )
            })?;
            Ok(0)
        }

        HubCommand::AccessLog { limit, data } => {
            let store = hub::HubStore::open(&hub::data_path(data.clone()))?;
            let events = store.hub_events(*limit)?;
            let chain = store.verify_hub_chain().map_err(|e| e.to_string());
            out.emit(
                &serde_json::json!({
                    "events": events,
                    "chain": match &chain {
                        Ok(n) => serde_json::json!({ "rows": n }),
                        Err(e) => serde_json::json!({ "broken": e }),
                    },
                }),
                |v| {
                    let mut s = String::new();
                    for e in v["events"].as_array().cloned().unwrap_or_default() {
                        s.push_str(&format!(
                            "{}  {:<18} {}  {}\n",
                            e["ts"].as_str().unwrap_or_default(),
                            e["action"].as_str().unwrap_or_default(),
                            e["actor"].as_str().unwrap_or_default(),
                            e["detail"]
                        ));
                    }
                    match v["chain"]["rows"].as_i64() {
                        Some(n) => s.push_str(&format!("\nchain holds over {n} entr(ies)\n")),
                        None => s.push_str(&format!(
                            "\nCHAIN BROKEN: {}\n",
                            v["chain"]["broken"].as_str().unwrap_or_default()
                        )),
                    }
                    s
                },
            )?;
            Ok(if chain.is_ok() { 0 } else { 1 })
        }

        HubCommand::Verify { data } => {
            let store = hub::HubStore::open(&hub::data_path(data.clone()))?;
            let report = hub::report::verify(&store)?;
            let ok = report.ok;
            // Rendered from the JSON shape rather than the struct so the text and `--json`
            // cannot describe two different things.
            let as_json = serde_json::to_value(&report)
                .map_err(|e| Error::Index(format!("verify report does not serialise: {e}")))?;
            out.emit(&as_json, |v| {
                let mut s = String::new();
                for d in v["devices"].as_array().cloned().unwrap_or_default() {
                    let verdict = match d["chain"].get("Ok") {
                        Some(n) => format!("chain holds over {} row(s)", n),
                        None => format!(
                            "BROKEN: {}",
                            d["chain"]["Err"].as_str().unwrap_or("unknown")
                        ),
                    };
                    s.push_str(&format!(
                        "{:<24} {}\n",
                        d["name"].as_str().unwrap_or_default(),
                        verdict
                    ));
                }
                s.push_str(&format!(
                    "\n{} row(s) checked; {}\n",
                    v["rows"].as_i64().unwrap_or_default(),
                    if v["ok"].as_bool().unwrap_or(false) {
                        "everything the hub holds is as it arrived"
                    } else {
                        "AT LEAST ONE CHAIN DOES NOT HOLD"
                    }
                ));
                s
            })?;
            Ok(if ok { 0 } else { 1 })
        }

        HubCommand::Report {
            out_dir,
            from,
            to,
            data,
        } => {
            let store = hub::HubStore::open(&hub::data_path(data.clone()))?;
            let tool = concat!("cyberbrain hub ", env!("CARGO_PKG_VERSION"));
            let report =
                hub::report::write_report(&store, out_dir, from.as_deref(), to.as_deref(), tool)?;
            out.emit(&report, |v| {
                format!(
                    "{}\nwritten to {}\n",
                    v["summary"].as_str().unwrap_or_default(),
                    v["directory"].as_str().unwrap_or_default()
                )
            })?;
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
