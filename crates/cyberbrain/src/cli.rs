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
        /// Restrict to notes of this department, team or domain.
        #[arg(long)]
        bereich: Option<String>,
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
        /// Which department, team or domain this note belongs to. Filters recall; never
        /// changes ranking. Absent is fine and stays the default.
        #[arg(long)]
        bereich: Option<String>,
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

    /// Write a note into `proposals/` for somebody else to accept.
    ///
    /// The same arguments as `write`, and the same PII gate — what differs is where it
    /// lands. A proposal is outside the notes tree, so it is not indexed and `recall`
    /// cannot return one: an unapproved ring 0 note that an agent could find would be an
    /// invariant nobody agreed to.
    Propose {
        #[arg(long, value_parser = clap::value_parser!(u8).range(0..=4))]
        ring: u8,
        #[arg(long)]
        kind: NoteKindArg,
        #[arg(long)]
        name: String,
        /// The body as Markdown. Omit to read it from stdin.
        #[arg(long)]
        body: Option<String>,
        #[arg(long)]
        tags: Vec<String>,
        /// Which department, team or domain this note belongs to.
        #[arg(long)]
        bereich: Option<String>,
        #[arg(long)]
        retention: Option<String>,
        /// Propose despite PII findings, recording them as flagged.
        #[arg(long)]
        force: bool,
        #[arg(long)]
        dry_run: bool,
    },

    /// What is waiting, and accepting or rejecting it.
    ///
    /// A proposal cannot be reviewed by the person who made it. Who that was comes from the
    /// audit log rather than from the file, because the log is hash-chained and the file is
    /// a line of YAML anybody can edit. This is a workflow with a record, not an
    /// authentication: it stops a mistake, not a determined person.
    Review {
        /// The proposal. Omit it to list what is waiting.
        target: Option<String>,
        /// Take it: the note moves into its ring and is indexed.
        #[arg(long, conflicts_with = "reject")]
        accept: bool,
        /// Turn it down. Needs `--reason`, and the proposal file goes.
        #[arg(long)]
        reject: bool,
        /// Why it was turned down. The only place the proposer will look for it.
        #[arg(long)]
        reason: Option<String>,
        /// Accept even though the note changed after this was proposed.
        #[arg(long)]
        force: bool,
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

    /// Write the `manifest.json` that a model artefact needs, from the files themselves.
    ///
    /// Semantic search needs a model, the model needs a manifest naming the blake3 digest of
    /// each file, and until now there was no way to produce one: the field names appeared in
    /// no documentation and the hashing existed only inside tests. People were left to guess
    /// a JSON shape from a deserialisation error.
    Manifest {
        /// The folder holding `model.safetensors` and `tokenizer.json`. Defaults to
        /// `<store>/models/model2vec`, which is where the loader looks.
        #[arg(long)]
        path: Option<PathBuf>,
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
        /// Offer a terminal in the page: a real shell, `ssh`, whatever you would run at a
        /// prompt, in the project's directory.
        ///
        /// Off by default, and that is a decision rather than caution. Every other route
        /// this serves reads or writes notes; this one starts a process, so a store served
        /// for reading cannot be talked into starting one. With it on, the address printed
        /// at startup carries a token in its fragment — open that address and no other.
        #[arg(long)]
        terminal: bool,
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

    /// Put this binary into an agent's own configuration: the hooks for Claude Code, the
    /// MCP server for a desktop client.
    ///
    /// Everything it writes belongs to another program, so nothing that is not ours is
    /// touched, our entries are marked, and the previous file is kept beside the new one.
    Install {
        /// Which client. Repeat the flag for several; omit for every one on this machine.
        #[arg(long, value_enum)]
        client: Vec<InstallClient>,
        /// The project whose `.claude/settings.json` gets the hooks. Defaults to the
        /// working directory.
        #[arg(long)]
        project: Option<PathBuf>,
        /// Name of the MCP entry. A desktop client has no project of its own, so a second
        /// store on this machine needs a second name.
        #[arg(long, default_value = "cyberbrain")]
        name: String,
        /// Take our entries out again.
        #[arg(long)]
        undo: bool,
        /// Say what would change, and write nothing.
        #[arg(long)]
        dry_run: bool,
    },

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

/// The clients `install` knows. Not every one is written to: see `install`'s module
/// documentation for why ChatGPT is absent and Codex is printed rather than edited.
#[derive(Debug, Clone, Copy, ValueEnum)]
pub enum InstallClient {
    ClaudeCode,
    ClaudeDesktop,
    Codex,
}

impl From<InstallClient> for crate::install::Client {
    fn from(c: InstallClient) -> Self {
        use crate::install::Client as C;
        match c {
            InstallClient::ClaudeCode => C::ClaudeCode,
            InstallClient::ClaudeDesktop => C::ClaudeDesktop,
            InstallClient::Codex => C::Codex,
        }
    }
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
pub enum AdminCommand {
    /// Forget the password. The next visit from the hub's own machine sets a new one.
    ///
    /// The way back in when somebody leaves or a password is lost. It needs access to the
    /// record, which is to say access to the machine — the same thing that would let anyone
    /// read the record with a SQLite tool, so this hands out nothing new.
    Reset {
        #[arg(long, value_name = "PATH")]
        data: Option<PathBuf>,
    },
}

#[derive(Debug, Subcommand)]
pub enum ServiceCommand {
    /// Register the service and start it. Needs an elevated prompt.
    Install {
        /// Where the record lives. Defaults to %PROGRAMDATA%\\Cyberbrain\\hub.db — outside
        /// the install directory, because the record outlives the program.
        #[arg(long, value_name = "PATH")]
        data: Option<PathBuf>,
        /// What to listen on. The default reaches the network: a hub only its own machine
        /// can talk to collects nothing.
        #[arg(long, default_value = "0.0.0.0:7788")]
        addr: String,
        /// PEM certificate chain the service serves with. Kept in the registration, so an
        /// upgrade does not quietly drop back to plain text.
        #[arg(long, value_name = "PATH", requires = "tls_key")]
        tls_cert: Option<PathBuf>,
        /// The private key for --tls-cert, PEM.
        #[arg(long, value_name = "PATH", requires = "tls_cert")]
        tls_key: Option<PathBuf>,
        /// Register it anyway with no certificate, listening to the network in the clear.
        /// Deliberately ugly to type: it is a decision, not a default.
        #[arg(long)]
        insecure_http: bool,
    },
    /// Stop the service and remove the registration. The record is left alone.
    Uninstall,
    /// Start it now. It starts by itself on boot; this is for after a licence was dropped in.
    Start,
    /// Stop it. The record stays, and clients buffer until it is back.
    Stop,
    /// Whether it is registered and what it is doing.
    Status,
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
        /// PEM certificate chain to serve with. Given together with --tls-key, the hub
        /// speaks https and a password may be typed into it from a desk rather than only
        /// at the machine.
        #[arg(long, value_name = "PATH", requires = "tls_key")]
        tls_cert: Option<PathBuf>,
        /// The private key for --tls-cert, PEM.
        #[arg(long, value_name = "PATH", requires = "tls_cert")]
        tls_key: Option<PathBuf>,
        /// Make a certificate of its own beside the record if there is not one yet, and
        /// serve with it. For a network with no certificate authority: invitations issued
        /// afterwards carry its fingerprint, and the machines that get one trust it.
        #[arg(long, conflicts_with = "tls_cert")]
        tls_generate: bool,
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
    /// The administrator account for the hub's page.
    Admin {
        #[command(subcommand)]
        command: AdminCommand,
    },
    /// Register the hub as a Windows service, or control the one that is registered.
    ///
    /// The installer does this on its own when the box is ticked. It is here for
    /// administrators who would rather see the command than a wizard, and because an
    /// unattended rollout needs something to call.
    Service {
        #[command(subcommand)]
        command: ServiceCommand,
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
        /// Send notes instead of audit rows: the ones carrying a bereich, in rings 2 to 4.
        /// A separate flag and a separate egress purpose, because content and evidence are
        /// different decisions. Refused unless allow_note_sync is set.
        #[arg(long)]
        notes: bool,
        /// Only this bereich. Without it, every bereich this store has notes for is offered
        /// and the hub keeps what this device was granted.
        #[arg(long)]
        bereich: Option<String>,
        /// Say what would be sent and send nothing.
        #[arg(long)]
        dry_run: bool,
    },
    /// Install, inspect or issue a licence.
    Licence {
        #[command(subcommand)]
        command: LicenceCommand,
    },
    /// The certificate this hub serves with: what it is, and how to make a browser accept it.
    Cert {
        #[command(subcommand)]
        command: CertCommand,
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
    /// Which bereich a device may share, and why. Without a grant a device delivers audit
    /// rows and nothing else; rings 0 and 1 are never eligible, whatever is granted.
    Grant {
        #[command(subcommand)]
        command: GrantCommand,
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
pub enum CertCommand {
    /// Where the certificate is, what its fingerprint is, and what to do with it.
    Show {
        #[arg(long, value_name = "PATH")]
        data: Option<PathBuf>,
    },
    /// Write a copy somewhere it can be handed out: a share, a group policy, an email.
    /// Only the certificate; the key stays where it is.
    Export {
        /// Where to write it. The file is a public thing — it is what the hub shows every
        /// machine that connects to it.
        to: PathBuf,
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
pub enum GrantCommand {
    /// Grant a device one bereich, in one direction.
    Add {
        #[arg(long, value_name = "DEVICE")]
        device: String,
        #[arg(long)]
        bereich: String,
        /// send, receive or both. Receiving admits foreign content and sending discloses
        /// your own; they are different risks, so they are separate rights.
        #[arg(long, default_value = "both")]
        direction: String,
        /// Why this department may share with that one. A countersigner or an auditor reads
        /// this later, and under Art. 5(1)(b) GDPR a purpose nobody wrote down is one that
        /// cannot be shown.
        #[arg(long)]
        reason: String,
        #[arg(long, value_name = "PATH")]
        data: Option<PathBuf>,
    },
    /// What has been granted, to whom, and what was withdrawn.
    List {
        /// One device, or all of them when omitted.
        #[arg(long, value_name = "DEVICE")]
        device: Option<String>,
        #[arg(long, value_name = "PATH")]
        data: Option<PathBuf>,
    },
    /// Withdraw a grant. The record of it stays; what stops is future delivery.
    Revoke {
        id: String,
        #[arg(long, value_name = "PATH")]
        data: Option<PathBuf>,
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
