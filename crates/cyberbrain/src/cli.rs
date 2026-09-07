//! The command surface (SPEC §8). Definitions only: every command's behaviour lives in
//! `commands/`, and the composition of the six crates lives in `app.rs`.
//!
//! Exit codes are the spec's: 0 success, 1 user error, 2 internal error, 3 policy refusal.
//! `cyberbrain-core`'s `Error::exit_code()` is the single mapping, shared with the HTTP API
//! (§8.1) so that one failure taxonomy serves both front ends.

use clap::{Parser, Subcommand, ValueEnum};
use std::path::PathBuf;

#[derive(Debug, Parser)]
#[command(
    name = "cyberbrain",
    version,
    about = "Cited, trust-tiered, local-first memory for AI coding agents",
    long_about = None,
)]
pub struct Cli {
    /// Store to operate on. Defaults to `.cyberbrain` found from the working directory
    /// upwards, so a command works anywhere inside a project.
    #[arg(long, global = true, env = "CYBERBRAIN_STORE")]
    pub store: Option<PathBuf>,

    /// Machine-readable output. Every command that prints anything supports it.
    #[arg(long, global = true)]
    pub json: bool,

    /// Print less. Errors still go to stderr.
    #[arg(long, short, global = true)]
    pub quiet: bool,

    #[command(subcommand)]
    pub command: Command,
}

#[derive(Debug, Subcommand)]
pub enum Command {
    /// Create a store and write a commented configuration file.
    Init {
        /// Where to create it. Defaults to `.cyberbrain` in the working directory.
        #[arg(long)]
        path: Option<PathBuf>,
    },

    /// Rebuild the index from the notes tree.
    Scan {
        /// Discard the index and rebuild everything. The audit log is untouched: it lives
        /// in its own file precisely so that this is safe.
        #[arg(long)]
        full: bool,
        #[arg(long)]
        dry_run: bool,
    },

    /// Search the store. Hybrid by default; there is no lexical-only mode, because a
    /// retrieval tool whose best mode is opt-in gets used in its worst mode.
    Recall {
        /// The query. Omit when using --id.
        query: Option<String>,
        /// Expand a citation to its full note instead of searching.
        #[arg(long, value_name = "CITATION", conflicts_with = "query")]
        id: Option<String>,
        /// How many hits to return.
        #[arg(long, short, default_value_t = 8)]
        n: usize,
        /// Restrict to exactly this ring.
        #[arg(long, value_parser = clap::value_parser!(u8).range(0..=4))]
        ring: Option<u8>,
    },

    /// Exact line ranges for a symbol, so the agent reads a slice and not a file.
    Find {
        /// The symbol to locate: a function, type, class, constant or table name.
        symbol: String,
        /// Most hits to return before the result is reported as truncated.
        #[arg(long, short, default_value_t = 20)]
        limit: usize,
    },

    /// Write a note.
    ///
    /// The body comes from stdin unless --body is given. Only the first paragraph is
    /// shared with the MCP tool description (SPEC §9.2); anything after it is CLI-only,
    /// which is what lets this sentence mention stdin without leaking into a protocol
    /// where stdin means something else entirely.
    Write {
        /// Trust tier, 0 to 4. Lower is more trusted and wins a contradiction.
        #[arg(long, value_parser = clap::value_parser!(u8).range(0..=4))]
        ring: u8,
        /// What the note records. Drives filtering and the write template, not retrieval.
        #[arg(long)]
        kind: NoteKindArg,
        /// kebab-case slug, unique in the store.
        #[arg(long)]
        name: String,
        /// The note body as Markdown. Omit to read it from stdin.
        #[arg(long)]
        body: Option<String>,
        /// A tag. Repeat the flag for several.
        #[arg(long)]
        tags: Vec<String>,
        /// ISO-8601 duration, e.g. P2Y. Absent means keep indefinitely.
        #[arg(long)]
        retention: Option<String>,
        /// Write despite PII findings, recording them as flagged rather than reviewed.
        #[arg(long)]
        force: bool,
        /// Run the real path with no-op writers and report what would have happened.
        #[arg(long)]
        dry_run: bool,
    },

    /// Erase a note and everything derived from it (GDPR Art. 17, SPEC §12.2).
    Forget {
        /// Note name or id.
        target: String,
        #[arg(long)]
        dry_run: bool,
    },

    /// Report dangling links, ring-cap pressure, a stale index and orphaned vectors.
    Doctor,

    /// Store health, embedding profile, inference endpoint, compliance profile.
    Status,

    /// Print a note.
    Export {
        target: String,
        #[arg(long, value_enum, default_value_t = ExportFormat::Md)]
        format: ExportFormat,
    },

    /// Bring an existing tree of Markdown notes into the store, driven by a mapping file.
    ///
    /// Deliberately generic. It handles a tree because the tree is Markdown, not because it
    /// knows what wrote it: every source-specific detail lives in the mapping file and none
    /// of it in the code.
    Import {
        /// TOML mapping file: what to take, what to skip, how to split, which ring.
        #[arg(long)]
        plan: PathBuf,
        /// Accept every PII finding in bulk. Holding several hundred imports one at a time
        /// is unusable, and an unusable gate gets bypassed rather than obeyed.
        #[arg(long)]
        accept_pii: bool,
        #[arg(long)]
        dry_run: bool,
    },

    /// Serve the web UI and the HTTP API on loopback (SPEC §8.1, §13).
    Serve {
        #[arg(long, default_value_t = 7777)]
        port: u16,
        /// Do not open a browser.
        #[arg(long)]
        no_open: bool,
    },

    /// Agent harness integration. Reads the payload on stdin, answers on stdout.
    ///
    /// A hook never fails the harness: any internal error is recorded and the process
    /// still exits 0 with empty output (SPEC §9.1).
    Hook {
        #[arg(value_enum)]
        event: HookEvent,
    },

    /// Serve the same operations over MCP on stdio.
    Mcp,

    /// Compliance operations (SPEC §12).
    Policy {
        #[command(subcommand)]
        command: PolicyCommand,
    },

    /// Collect audit rows from other machines (SPEC §8.2 does not apply: this is a
    /// different surface with its own, authenticated access).
    Hub {
        #[command(subcommand)]
        command: HubCommand,
    },

    /// Check an audit export somebody handed you.
    ///
    /// Needs no store, no configuration and no network: everything the check uses is in the
    /// file. Exits non-zero when the chain does not hold.
    VerifyExport {
        #[arg(value_name = "PATH")]
        path: PathBuf,
    },
}

#[derive(Debug, Subcommand)]
pub enum PolicyCommand {
    /// Every path by which bytes may leave this machine, and whether it is enabled.
    Egress,
    /// What the active profile claims about the law, with the basis and the confidence.
    Obligations,
    /// The audit log.
    Audit {
        /// Most rows to show. Defaults to 50 for the listing; an export without this flag
        /// covers the whole period, because a silently truncated period is the one mistake
        /// an auditor cannot see in the file.
        #[arg(long)]
        limit: Option<usize>,
        #[arg(long)]
        action: Option<String>,
        #[arg(long)]
        subject: Option<String>,
        /// Verify the hash chain and name the first altered row.
        #[arg(long)]
        verify: bool,
        /// Start of the period, inclusive. RFC 3339, e.g. 2026-01-01T00:00:00Z.
        #[arg(long, value_name = "TIMESTAMP")]
        since: Option<String>,
        /// End of the period, inclusive.
        #[arg(long, value_name = "TIMESTAMP")]
        until: Option<String>,
        /// Write the selected rows to a file as a self-checking bundle, for somebody else
        /// to verify. Implies no limit unless --limit is given explicitly.
        #[arg(long, value_name = "PATH")]
        export: Option<PathBuf>,
    },
    /// Everything stored about an identifier (GDPR Art. 15).
    Subject { identifier: String },
    /// Notes whose retention has expired.
    Retention {
        /// Erase what is due, through the same path as `forget`.
        #[arg(long)]
        apply: bool,
        #[arg(long)]
        dry_run: bool,
    },
    /// Identity, source, licence and hash of every model artefact in use.
    ModelCard,
    /// Record consent for the one model download, or withdraw it.
    Consent {
        #[arg(long)]
        withdraw: bool,
    },
}

#[derive(Debug, Clone, Copy, ValueEnum)]
pub enum HookEvent {
    SessionStart,
    UserPromptSubmit,
    PreToolUse,
    PostToolUse,
    Stop,
    PreCompact,
}

#[derive(Debug, Clone, Copy, ValueEnum)]
pub enum ExportFormat {
    Md,
    Json,
}

#[derive(Debug, Clone, Copy, ValueEnum)]
pub enum NoteKindArg {
    Knowledge,
    Bug,
    Lesson,
    Decision,
    Reference,
    Session,
}

impl From<NoteKindArg> for cyberbrain_core::NoteKind {
    fn from(k: NoteKindArg) -> Self {
        use cyberbrain_core::NoteKind as N;
        match k {
            NoteKindArg::Knowledge => N::Knowledge,
            NoteKindArg::Bug => N::Bug,
            NoteKindArg::Lesson => N::Lesson,
            NoteKindArg::Decision => N::Decision,
            NoteKindArg::Reference => N::Reference,
            NoteKindArg::Session => N::Session,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use clap::CommandFactory;

    #[test]
    fn the_command_surface_is_well_formed() {
        Cli::command().debug_assert();
    }

    /// SPEC §8 lists the commands; this pins the list so one cannot quietly disappear.
    #[test]
    fn every_command_in_the_spec_is_reachable() {
        let cmd = Cli::command();
        let names: Vec<&str> = cmd.get_subcommands().map(|s| s.get_name()).collect();
        for expected in [
            "init", "scan", "recall", "find", "write", "forget", "doctor", "status", "export",
            "import", "serve", "hook", "mcp", "policy",
        ] {
            assert!(
                names.contains(&expected),
                "{expected} is missing from the CLI"
            );
        }
    }

    /// The compliance surface is only worth something if it can be printed. The obligation
    /// catalogue existed from the first profile and was reachable from nowhere for months.
    #[test]
    fn the_policy_surface_can_print_what_it_claims() {
        let cmd = Cli::command();
        let policy = cmd.find_subcommand("policy").unwrap();
        let subs: Vec<&str> = policy.get_subcommands().map(|s| s.get_name()).collect();
        for expected in [
            "egress",
            "obligations",
            "audit",
            "subject",
            "retention",
            "model-card",
        ] {
            assert!(subs.contains(&expected), "policy {expected} is missing");
        }
    }

    /// Every state-changing command takes --dry-run (SPEC §8).
    #[test]
    fn state_changing_commands_offer_dry_run() {
        let cmd = Cli::command();
        for name in ["scan", "write", "forget", "import"] {
            let sub = cmd.find_subcommand(name).unwrap();
            assert!(
                sub.get_arguments().any(|a| a.get_long() == Some("dry-run")),
                "{name} must offer --dry-run"
            );
        }
        let policy = cmd.find_subcommand("policy").unwrap();
        let retention = policy.find_subcommand("retention").unwrap();
        assert!(
            retention
                .get_arguments()
                .any(|a| a.get_long() == Some("dry-run"))
        );
    }

    #[test]
    fn all_six_hook_events_exist() {
        assert_eq!(HookEvent::value_variants().len(), 6);
    }
}

#[derive(Debug, Subcommand)]
pub enum HubCommand {
    /// Run the collector. Unlike `serve`, the address is a parameter — a hub only its own
    /// machine can reach is not a hub, which is why this surface authenticates.
    Serve {
        #[arg(long, default_value = "127.0.0.1:7788")]
        addr: String,
        /// Where the record lives.
        #[arg(long, value_name = "PATH")]
        data: Option<PathBuf>,
    },
    /// Register a machine and print its token. The token is shown once and stored only as a
    /// hash, so a copy of the record is not a set of working credentials.
    Add {
        name: String,
        #[arg(long, value_name = "PATH")]
        data: Option<PathBuf>,
        /// Write an invitation file for this device instead of printing the token loose.
        /// It carries everything the machine needs, so nobody assembles it by hand.
        #[arg(long, value_name = "PATH")]
        invite: Option<PathBuf>,
        /// Address the device should deliver to, for the invitation.
        #[arg(long, value_name = "URL")]
        hub_url: Option<String>,
        /// Address of the shared inference endpoint, if there is one. This is the setting
        /// from docs/SHARED-INFERENCE.md, carried along so it is not typed into every store
        /// by hand.
        #[arg(long, value_name = "URL")]
        inference_url: Option<String>,
    },
    /// Set this store up to deliver to a hub, from an invitation file.
    Enrol { invitation: PathBuf },
    /// Deliver this store's audit rows to the hub it was enrolled with.
    ///
    /// Nothing is buffered separately: the audit log is the buffer, and a failed delivery
    /// changes nothing here. Meant for a timer — once an hour is plenty.
    Push {
        /// Send rows from this point rather than the whole log. The hub skips what it
        /// already has, so a generous window costs bandwidth and nothing else.
        #[arg(long, value_name = "TIMESTAMP")]
        since: Option<String>,
    },
    /// Install, inspect or issue a licence.
    Licence {
        #[command(subcommand)]
        command: LicenceCommand,
    },
    /// Devices, when they were last heard from, and what is wrong with any of them.
    Fleet {
        #[arg(long, value_name = "PATH")]
        data: Option<PathBuf>,
    },
    /// People and their roles: who administers, who may ask to see activity, who approves.
    Principal {
        #[command(subcommand)]
        command: PrincipalCommand,
    },
    /// Ask to see activity. Needs an auditor credential, and somebody else to approve it.
    Request {
        /// Why. The countersigner reads this and nothing else decides for them.
        #[arg(long)]
        reason: String,
        /// One device, or all of them when omitted.
        #[arg(long, value_name = "DEVICE")]
        device: Option<String>,
        #[arg(long, value_name = "TIMESTAMP")]
        from: Option<String>,
        #[arg(long, value_name = "TIMESTAMP")]
        to: Option<String>,
        #[arg(long, value_name = "TOKEN", env = "CYBERBRAIN_HUB_PRINCIPAL_TOKEN")]
        as_: Option<String>,
        #[arg(long, value_name = "PATH")]
        data: Option<PathBuf>,
    },
    /// Countersign a request. Needs a countersigner credential, and cannot be your own.
    Approve {
        request: String,
        /// How long the window stays open.
        #[arg(long, default_value_t = crate::hub::access::DEFAULT_WINDOW_HOURS)]
        hours: i64,
        #[arg(long, value_name = "TOKEN", env = "CYBERBRAIN_HUB_PRINCIPAL_TOKEN")]
        as_: Option<String>,
        #[arg(long, value_name = "PATH")]
        data: Option<PathBuf>,
    },
    /// Every request ever made, and what became of it.
    Requests {
        #[arg(long, value_name = "PATH")]
        data: Option<PathBuf>,
    },
    /// Read the activity an approved request covers, into a directory.
    Disclose {
        request: String,
        #[arg(long, value_name = "PATH")]
        out_dir: PathBuf,
        #[arg(long, value_name = "TOKEN", env = "CYBERBRAIN_HUB_PRINCIPAL_TOKEN")]
        as_: Option<String>,
        #[arg(long, value_name = "PATH")]
        data: Option<PathBuf>,
    },
    /// The hub's own log: roles granted, requests, approvals, disclosures.
    AccessLog {
        #[arg(long, default_value_t = 50)]
        limit: usize,
        #[arg(long, value_name = "PATH")]
        data: Option<PathBuf>,
    },
    /// Re-check every chain the hub holds, against the rows as they are on disk now.
    ///
    /// Exits non-zero when a chain does not hold. Meant for a nightly job: the arrival
    /// check cannot answer whether the database was replaced afterwards.
    Verify {
        #[arg(long, value_name = "PATH")]
        data: Option<PathBuf>,
    },
    /// Write a period as evidence: one verifiable file per device, plus a summary.
    Report {
        /// Directory to write into. Created if it does not exist.
        #[arg(long, value_name = "PATH")]
        out_dir: PathBuf,
        #[arg(long, value_name = "TIMESTAMP")]
        from: Option<String>,
        #[arg(long, value_name = "TIMESTAMP")]
        to: Option<String>,
        #[arg(long, value_name = "PATH")]
        data: Option<PathBuf>,
    },
    /// Stop a device from sending. Its rows stay: revoking is not a deletion.
    Revoke {
        id: String,
        #[arg(long, value_name = "PATH")]
        data: Option<PathBuf>,
    },
}

#[derive(Debug, Subcommand)]
pub enum LicenceCommand {
    /// Put a licence into this hub's record.
    Install {
        path: PathBuf,
        #[arg(long, value_name = "PATH")]
        data: Option<PathBuf>,
    },
    /// What the installed licence says, and how long it has left.
    Show {
        #[arg(long, value_name = "PATH")]
        data: Option<PathBuf>,
    },
    /// Make a key pair. The issuer keeps the private half; the public half belongs in the
    /// binary, which means a rebuild — that is the point of a trust anchor.
    Keygen,
    /// Sign a licence. Needs the private key, so this is the issuer's command, not a
    /// customer's.
    Issue {
        /// File holding the signing key as hex. Never passed on the command line, where it
        /// would end up in the shell history and in `ps`.
        #[arg(long, value_name = "PATH")]
        key_file: PathBuf,
        #[arg(long)]
        customer: String,
        #[arg(long)]
        seats: usize,
        /// RFC 3339. Defaults to now.
        #[arg(long, value_name = "TIMESTAMP")]
        from: Option<String>,
        /// RFC 3339, inclusive.
        #[arg(long, value_name = "TIMESTAMP")]
        until: String,
        /// Where to write it. Prints to stdout when absent.
        #[arg(long, value_name = "PATH")]
        out: Option<PathBuf>,
    },
}

#[derive(Debug, Subcommand)]
pub enum PrincipalCommand {
    /// Register a person and print their credential once.
    Add {
        name: String,
        /// admin | auditor | countersigner
        #[arg(long)]
        role: String,
        #[arg(long, value_name = "PATH")]
        data: Option<PathBuf>,
    },
    List {
        #[arg(long, value_name = "PATH")]
        data: Option<PathBuf>,
    },
    Revoke {
        id: String,
        #[arg(long, value_name = "PATH")]
        data: Option<PathBuf>,
    },
}
