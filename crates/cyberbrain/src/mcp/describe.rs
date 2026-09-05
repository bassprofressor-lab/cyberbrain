//! Tool descriptions, read from the CLI's own help (SPEC §9.2: one source, so the two
//! cannot drift).
//!
//! The source is the doc comments in `cli.rs`, as clap has already parsed them. A tool's
//! description is the `about` of the subcommand it mirrors — the first paragraph of the
//! doc comment; later paragraphs stay CLI-only and appear in `--help`. A tool argument's
//! description is the help of the CLI argument it mirrors. Where the CLI argument has no
//! help text, the schema property carries no description at all rather than a second,
//! hand-written one: the JSON-schema constraints still say what is accepted, and a missing
//! line in `--help` is now visible over MCP too instead of being papered over here.
//!
//! Arguments that exist only over MCP (`choice`, `expected_updated`) have no CLI twin and
//! are described in `tools.rs`; that text is the only copy, so it is not a duplicate.

use crate::cli::Cli;
use clap::CommandFactory;

/// The `about` of `cyberbrain <cmd>`, or a loud placeholder if the subcommand has none.
/// The placeholder is deliberately ugly: a tool whose description reads like that gets
/// fixed, whereas an empty string gets shipped.
pub fn about(cmd: &str) -> String {
    let cli = Cli::command();
    match cli.find_subcommand(cmd).and_then(|s| s.get_about()) {
        Some(a) => a.to_string(),
        None => format!("(no description: `cyberbrain {cmd}` has no help text in cli.rs)"),
    }
}

/// The help of argument `arg` of `cyberbrain <cmd>`, if the CLI documents it.
pub fn arg_help(cmd: &str, arg: &str) -> Option<String> {
    let cli = Cli::command();
    let sub = cli.find_subcommand(cmd)?;
    sub.get_arguments()
        .find(|a| a.get_id().as_str() == arg)
        .and_then(|a| a.get_help())
        .map(|h| h.to_string())
}
