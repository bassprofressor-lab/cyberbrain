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
}

#[derive(Debug, Subcommand)]
pub enum PolicyCommand {
    /// Every path by which bytes may leave this machine, and whether it is enabled.
    Egress,
    /// The audit log.
    Audit {
        #[arg(long, default_value_t = 50)]
        limit: usize,
        #[arg(long)]
        action: Option<String>,
        #[arg(long)]
        subject: Option<String>,
        /// Verify the hash chain and name the first altered row.
        #[arg(long)]
        verify: bool,
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
