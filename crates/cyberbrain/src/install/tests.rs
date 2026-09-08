//! What these check is the part that cannot be tried here: every file this command writes
//! belongs to a program that is not installed on a build machine. So the environment is
//! handed in, the directory trees a Windows and a macOS machine would have are built in a
//! temporary directory, and the answers are checked against them.

use super::*;
use serde_json::Value;
use std::fs;

fn project(tmp: &Path) -> PathBuf {
    let dir = tmp.join("proj");
    fs::create_dir_all(dir.join(".cyberbrain").join("notes")).unwrap();
    dir
}

fn opts(tmp: &Path, env: Env) -> Options {
    let project = project(tmp);
    Options {
        clients: Vec::new(),
        store: project.join(".cyberbrain"),
        project,
        name: "cyberbrain".to_string(),
        undo: false,
        dry_run: false,
        env,
    }
}

/// A machine with nothing else on it, so a test about one client hears about no other.
fn bare_env(tmp: &Path) -> Env {
    Env {
        os: paths::Os::Other,
        home: Some(tmp.join("empty-home")),
        appdata: None,
        local_appdata: None,
    }
}

fn read(path: &Path) -> Value {
    serde_json::from_str(&fs::read_to_string(path).unwrap()).unwrap()
}

fn only(report: &Report, client: Client) -> &ClientReport {
    report
        .clients
        .iter()
        .find(|c| c.client == client.name())
        .expect("the client was asked for")
}

// -----------------------------------------------------------------------------------
// Claude Code

#[test]
fn the_hooks_land_in_the_project_and_come_back_out() {
    let tmp = tempfile::tempdir().unwrap();
    let mut o = opts(tmp.path(), bare_env(tmp.path()));
    o.clients = vec![Client::ClaudeCode];

    let r = run(&o).unwrap();
    let file = o.project.join(".claude").join("settings.json");
    let v = read(&file);
    let hooks = v["hooks"].as_object().unwrap();
    assert_eq!(hooks.len(), 6, "one entry per lifecycle event: {v:#}");
    for (event, arg, matcher) in EVENTS {
        let entry = &hooks[event][0];
        assert_eq!(entry["matcher"], matcher, "{event}");
        assert_eq!(entry["_managedBy"], MANAGED_BY);
        assert_eq!(entry["hooks"][0]["args"], serde_json::json!(["hook", arg]));
        let command = entry["hooks"][0]["command"].as_str().unwrap();
        assert!(
            Path::new(command).is_absolute(),
            "the client runs this from somewhere else: {command}"
        );
    }
    assert_eq!(
        only(&r, Client::ClaudeCode).changes[0].action,
        Action::Added
    );

    o.undo = true;
    let r = run(&o).unwrap();
    assert_eq!(
        only(&r, Client::ClaudeCode).changes[0].action,
        Action::Removed
    );
    let v = read(&file);
    assert!(
        v.get("hooks").is_none(),
        "an empty `hooks` left behind is litter in somebody else's file: {v:#}"
    );
}

#[test]
fn a_second_run_changes_nothing_and_makes_no_backup() {
    let tmp = tempfile::tempdir().unwrap();
    let mut o = opts(tmp.path(), bare_env(tmp.path()));
    o.clients = vec![Client::ClaudeCode];
    run(&o).unwrap();
    let file = o.project.join(".claude").join("settings.json");
    let before = fs::read_to_string(&file).unwrap();

    let r = run(&o).unwrap();
    let change = &only(&r, Client::ClaudeCode).changes[0];
    assert_eq!(change.action, Action::Unchanged);
    assert!(change.backup.is_none());
    assert_eq!(fs::read_to_string(&file).unwrap(), before);
    assert!(
        !file.with_file_name("settings.json.bak").exists(),
        "nothing changed, so there is no previous file to keep"
    );
}

#[test]
fn everything_that_is_not_ours_survives() {
    let tmp = tempfile::tempdir().unwrap();
    let mut o = opts(tmp.path(), bare_env(tmp.path()));
    o.clients = vec![Client::ClaudeCode];
    let file = o.project.join(".claude").join("settings.json");
    fs::create_dir_all(file.parent().unwrap()).unwrap();
    fs::write(
        &file,
        r#"{
          "permissions": { "allow": ["Bash(ls:*)"] },
          "hooks": {
            "SessionStart": [
              { "matcher": "", "hooks": [{ "type": "command", "command": "/opt/theirs" }] }
            ]
          }
        }"#,
    )
    .unwrap();

    run(&o).unwrap();
    let v = read(&file);
    assert_eq!(v["permissions"]["allow"][0], "Bash(ls:*)");
    let starts = v["hooks"]["SessionStart"].as_array().unwrap();
    assert_eq!(starts.len(), 2, "theirs and ours: {v:#}");
    assert_eq!(starts[0]["hooks"][0]["command"], "/opt/theirs");

    // And an undo takes only ours back out.
    o.undo = true;
    run(&o).unwrap();
    let v = read(&file);
    let starts = v["hooks"]["SessionStart"].as_array().unwrap();
    assert_eq!(starts.len(), 1);
    assert_eq!(starts[0]["hooks"][0]["command"], "/opt/theirs");
    assert_eq!(v["permissions"]["allow"][0], "Bash(ls:*)");
}

#[test]
fn a_file_that_does_not_parse_is_refused_rather_than_replaced() {
    let tmp = tempfile::tempdir().unwrap();
    let mut o = opts(tmp.path(), bare_env(tmp.path()));
    o.clients = vec![Client::ClaudeCode];
    let file = o.project.join(".claude").join("settings.json");
    fs::create_dir_all(file.parent().unwrap()).unwrap();
    let theirs = "{ \"permissions\": { \"allow\": [\"Bash(ls:*)\",] } }";
    fs::write(&file, theirs).unwrap();

    let e = run(&o).unwrap_err();
    assert!(
        e.to_string().contains("not valid JSON"),
        "the message has to say what is wrong with it: {e}"
    );
    assert_eq!(
        fs::read_to_string(&file).unwrap(),
        theirs,
        "a file we could not read is a file we do not write"
    );
}

#[test]
fn a_dry_run_says_what_would_happen_and_writes_nothing() {
    let tmp = tempfile::tempdir().unwrap();
    let mut o = opts(tmp.path(), bare_env(tmp.path()));
    o.clients = vec![Client::ClaudeCode];
    o.dry_run = true;
    let r = run(&o).unwrap();
    assert_eq!(
        only(&r, Client::ClaudeCode).changes[0].action,
        Action::Added
    );
    assert!(!o.project.join(".claude").exists());
}

/// The comment on [`EVENTS`] claims the matcher is the same list as the tools the hook
/// reads. If somebody teaches `edited_file` a new tool, this fails rather than the hook
/// quietly never seeing it.
#[test]
fn the_matcher_covers_every_tool_the_hook_reads() {
    use crate::hook::payload::Payload;
    let payload = |tool: &str| {
        Payload::parse(&format!(
            r#"{{"tool_name":"{tool}","tool_input":{{"file_path":"/x","notebook_path":"/x"}}}}"#
        ))
    };
    for tool in TOOL_MATCHER.split('|') {
        assert!(
            payload(tool).edited_file().is_some(),
            "{tool} is in the matcher, so the hook has to have something to do with it"
        );
    }
    for tool in ["Bash", "Read", "Grep", "WebFetch", "Task"] {
        assert!(
            payload(tool).edited_file().is_none(),
            "{tool} is not in the matcher; if the hook now reads it, the matcher is wrong"
        );
    }
}

// -----------------------------------------------------------------------------------
// Claude Desktop

/// A Windows machine with the Microsoft Store build installed, and the file the app's own
/// Edit Config button creates sitting in the place the app does not read.
fn windows_with_both(tmp: &Path) -> Env {
    let local = tmp.join("Local");
    let roaming = tmp.join("Roaming");
    fs::create_dir_all(
        local
            .join("Packages")
            .join("Claude_pzs8sxrjxfjjc")
            .join("LocalCache")
            .join("Roaming")
            .join("Claude"),
    )
    .unwrap();
    fs::create_dir_all(roaming.join("Claude")).unwrap();
    Env {
        os: paths::Os::Windows,
        home: Some(tmp.join("empty-home")),
        appdata: Some(roaming),
        local_appdata: Some(local),
    }
}

#[test]
fn both_claude_desktop_installations_get_the_entry() {
    let tmp = tempfile::tempdir().unwrap();
    let mut o = opts(tmp.path(), windows_with_both(tmp.path()));
    o.clients = vec![Client::ClaudeDesktop];

    let r = run(&o).unwrap();
    let c = only(&r, Client::ClaudeDesktop);
    assert!(c.found);
    assert_eq!(
        c.changes.len(),
        2,
        "the packaged build and the installer build read different files: {:#?}",
        c.changes
    );
    assert!(
        c.note.is_some(),
        "two files is surprising enough to say out loud"
    );
    for change in &c.changes {
        assert_eq!(change.action, Action::Added);
        let v = read(&change.path);
        let entry = &v["mcpServers"]["cyberbrain"];
        assert_eq!(entry["args"][0], "mcp");
        assert_eq!(entry["args"][1], "--store");
        assert_eq!(
            Path::new(entry["args"][2].as_str().unwrap()),
            o.store.canonicalize().unwrap(),
            "a desktop client has no working directory of ours, so the store is named"
        );
    }
    // The packaged path is the one the application actually reads. It has to be among
    // them; a run that wrote only the documented path is the bug this exists to prevent.
    assert!(
        c.changes
            .iter()
            .any(|ch| ch.path.to_string_lossy().contains("LocalCache")),
        "{:#?}",
        c.changes
    );
}

#[test]
fn nothing_is_written_where_claude_desktop_is_not_installed() {
    let tmp = tempfile::tempdir().unwrap();
    let mut o = opts(tmp.path(), bare_env(tmp.path()));
    o.clients = vec![Client::ClaudeDesktop];
    let r = run(&o).unwrap();
    let c = only(&r, Client::ClaudeDesktop);
    assert!(!c.found);
    assert!(c.changes.is_empty());
    assert!(c.note.as_ref().unwrap().contains("not installed"));
}

#[test]
fn the_desktop_entry_is_updated_in_place_and_undone_by_name() {
    let tmp = tempfile::tempdir().unwrap();
    let mut o = opts(tmp.path(), windows_with_both(tmp.path()));
    o.clients = vec![Client::ClaudeDesktop];
    let config = tmp
        .path()
        .join("Roaming")
        .join("Claude")
        .join("claude_desktop_config.json");
    fs::write(
        &config,
        r#"{"mcpServers":{"theirs":{"command":"npx"},"cyberbrain":{"command":"old"}}}"#,
    )
    .unwrap();

    let r = run(&o).unwrap();
    let change = only(&r, Client::ClaudeDesktop)
        .changes
        .iter()
        .find(|c| c.path == config)
        .unwrap();
    assert_eq!(change.action, Action::Updated);
    assert!(change.backup.is_some(), "the previous file is kept");
    let v = read(&config);
    assert_eq!(v["mcpServers"]["theirs"]["command"], "npx");
    assert_ne!(v["mcpServers"]["cyberbrain"]["command"], "old");

    o.undo = true;
    run(&o).unwrap();
    let v = read(&config);
    assert_eq!(v["mcpServers"]["theirs"]["command"], "npx");
    assert!(v["mcpServers"].get("cyberbrain").is_none());
}

#[test]
fn a_second_store_can_have_its_own_entry() {
    let tmp = tempfile::tempdir().unwrap();
    let mut o = opts(tmp.path(), windows_with_both(tmp.path()));
    o.clients = vec![Client::ClaudeDesktop];
    run(&o).unwrap();
    o.name = "cyberbrain-other".to_string();
    run(&o).unwrap();

    let config = tmp
        .path()
        .join("Roaming")
        .join("Claude")
        .join("claude_desktop_config.json");
    let v = read(&config);
    let servers = v["mcpServers"].as_object().unwrap();
    assert_eq!(servers.len(), 2, "{v:#}");
}

#[test]
fn macos_has_one_place_and_it_is_the_library_one() {
    let tmp = tempfile::tempdir().unwrap();
    let home = tmp.path().join("home");
    fs::create_dir_all(
        home.join("Library")
            .join("Application Support")
            .join("Claude"),
    )
    .unwrap();
    let env = Env {
        os: paths::Os::Mac,
        home: Some(home.clone()),
        appdata: None,
        local_appdata: None,
    };
    let found = paths::resolve(&paths::claude_desktop(&env));
    assert_eq!(found.len(), 1);
    assert_eq!(
        found[0].path,
        home.join("Library")
            .join("Application Support")
            .join("Claude")
            .join("claude_desktop_config.json")
    );
}

// -----------------------------------------------------------------------------------
// Codex

#[test]
fn codex_is_shown_the_lines_and_its_file_is_not_touched() {
    let tmp = tempfile::tempdir().unwrap();
    let home = tmp.path().join("home");
    fs::create_dir_all(home.join(".codex")).unwrap();
    let config = home.join(".codex").join("config.toml");
    let theirs = "# mine\nmodel = \"gpt-5\"\n";
    fs::write(&config, theirs).unwrap();

    let mut o = opts(
        tmp.path(),
        Env {
            os: paths::Os::Other,
            home: Some(home),
            appdata: None,
            local_appdata: None,
        },
    );
    o.clients = vec![Client::Codex];
    let r = run(&o).unwrap();
    let c = only(&r, Client::Codex);
    assert!(c.found);
    assert!(c.changes.is_empty(), "it is written by hand, not by us");
    let snippet = c.snippet.as_ref().unwrap();
    assert!(snippet.contains("codex mcp add cyberbrain"), "{snippet}");
    assert!(snippet.contains("[mcp_servers.cyberbrain]"), "{snippet}");
    assert_eq!(
        fs::read_to_string(&config).unwrap(),
        theirs,
        "their comments and their settings are still there"
    );
}

#[test]
fn codex_is_not_mentioned_where_it_has_never_run() {
    let tmp = tempfile::tempdir().unwrap();
    let mut o = opts(tmp.path(), bare_env(tmp.path()));
    o.clients = vec![Client::Codex];
    let r = run(&o).unwrap();
    assert!(!only(&r, Client::Codex).found);
}

#[test]
fn asking_for_nothing_asks_for_everything() {
    let tmp = tempfile::tempdir().unwrap();
    let o = opts(tmp.path(), bare_env(tmp.path()));
    let r = run(&o).unwrap();
    assert_eq!(r.clients.len(), Client::ALL.len());
}

/// The hook command lands in `settings.json`, and on Windows `canonicalize` writes it as
/// `\\?\D:\a\cyberbrain.exe`. That is a valid path to every Windows API and it is not what
/// belongs in a file people read — CI found it as a launcher test that could not match the
/// path against the binary it had started.
///
/// The table runs on every platform on purpose: the code is Windows-only, the mistake in it
/// would not be.
#[test]
fn the_extended_length_prefix_comes_off_a_path_that_does_not_need_it() {
    for (given, want) in [
        (
            r"\\?\C:\Program Files\Cyberbrain\cyberbrain.exe",
            Some(r"C:\Program Files\Cyberbrain\cyberbrain.exe"),
        ),
        (
            r"\\?\D:\a\cyberbrain\target\debug\cyberbrain.exe",
            Some(r"D:\a\cyberbrain\target\debug\cyberbrain.exe"),
        ),
        // A share keeps both of its leading separators, or it names a local path instead.
        (
            r"\\?\UNC\server\share\cyberbrain.exe",
            Some(r"\\server\share\cyberbrain.exe"),
        ),
        // Not a drive letter: the prefix is the only thing making this name a device, so it
        // stays. Stripping it here would be the one case that changes which file is meant.
        (r"\\?\Volume{9f3a}\cyberbrain.exe", None),
        (r"\\?\GLOBALROOT\Device\HarddiskVolume2\x.exe", None),
        // Ordinary paths are left exactly as they are, including a real UNC share.
        (r"C:\Program Files\Cyberbrain\cyberbrain.exe", None),
        (r"\\server\share\cyberbrain.exe", None),
        ("/usr/local/bin/cyberbrain", None),
    ] {
        assert_eq!(without_verbatim_prefix(given).as_deref(), want, "{given}");
    }
}
