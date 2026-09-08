//! Tests for the half of the launcher that is not Windows.
//!
//! The ones that matter start the real `cyberbrain serve`, on the platform this is built
//! on, and talk HTTP to it. A launcher whose only evidence is "it compiles" is exactly the
//! kind of program that fails on the one machine nobody could try it on.

use super::*;
use std::io::{Read, Write};
use std::net::TcpStream;

#[test]
fn an_address_is_read_out_of_the_line_serve_prints() {
    let line = "cyberbrain serve: http://127.0.0.1:52341/  (loopback only, no authentication; API at /api/v1)";
    assert_eq!(
        parse_serve_url(line).as_deref(),
        Some("http://127.0.0.1:52341/")
    );
}

#[test]
fn lines_without_an_address_are_not_one() {
    for line in [
        "cyberbrain serve: the web UI bundle is not embedded",
        "",
        "http://",
        "listening",
    ] {
        assert!(parse_serve_url(line).is_none(), "parsed {line:?}");
    }
}

#[test]
fn the_store_and_the_project_are_both_accepted_answers() {
    let project = Path::new("/home/x/proj");
    assert_eq!(normalise_project_dir(project), project);
    assert_eq!(normalise_project_dir(&project.join(".cyberbrain")), project);
}

#[test]
fn a_store_is_found_from_a_subdirectory_the_way_the_cli_finds_it() {
    let tmp = tempfile::tempdir().unwrap();
    let deep = tmp.path().join("a/b/c");
    std::fs::create_dir_all(&deep).unwrap();
    assert!(!has_store(&deep));
    std::fs::create_dir_all(tmp.path().join(".cyberbrain/notes")).unwrap();
    assert!(has_store(&deep));
    assert!(has_store(tmp.path()));
}

#[test]
fn one_project_fills_the_window() {
    assert_eq!(tile(1, 1000, 800), vec![(0, 0, 1000, 800)]);
}

#[test]
fn two_projects_are_side_by_side() {
    assert_eq!(
        tile(2, 1000, 800),
        vec![(0, 0, 500, 800), (500, 0, 500, 800)]
    );
}

/// The panes have to add up to the window. A rounding error here is a seam of unpainted
/// window down the middle, and it only shows at sizes nobody thought to try.
#[test]
fn the_panes_cover_the_window_exactly_at_awkward_sizes() {
    for count in 1..=9 {
        for (w, h) in [(1000, 800), (1001, 799), (7, 5), (1365, 767)] {
            let panes = tile(count, w, h);
            assert_eq!(panes.len(), count, "{count} panes at {w}x{h}");
            let area: i64 = panes
                .iter()
                .map(|(_, _, pw, ph)| *pw as i64 * *ph as i64)
                .sum();
            assert_eq!(
                area,
                w as i64 * h as i64,
                "{count} panes at {w}x{h} do not cover it: {panes:?}"
            );
            for (x, y, pw, ph) in &panes {
                assert!(*pw > 0 && *ph > 0, "empty pane in {panes:?}");
                assert!(x + pw <= w && y + ph <= h, "pane outside in {panes:?}");
            }
        }
    }
}

/// Three is two on top and one below, and that one spreads rather than leaving a gap where
/// a fourth would have been.
#[test]
fn the_last_row_spreads_to_fill_the_width() {
    let panes = tile(3, 1000, 800);
    assert_eq!(panes[2], (0, 400, 1000, 400), "{panes:?}");
}

#[test]
fn a_window_with_no_size_yet_gets_no_panes() {
    assert!(tile(2, 0, 800).is_empty());
    assert!(tile(0, 1000, 800).is_empty());
}

/// A project is chosen by folder and a store is found by walking upwards, so two different
/// folders can be one store. The "already open" check compares stores for that reason: with
/// folders, choosing a subdirectory of an open project started a second server on the same
/// index.
#[test]
fn a_subdirectory_resolves_to_the_same_store_as_its_project() {
    let tmp = tempfile::tempdir().unwrap();
    let project = tmp.path().join("api");
    std::fs::create_dir_all(project.join(".cyberbrain/notes")).unwrap();
    let deep = project.join("src/inner");
    std::fs::create_dir_all(&deep).unwrap();

    let a = store_of(&project).expect("the project has a store");
    let b = store_of(&deep).expect("so does anything inside it");
    assert_eq!(a, b, "a subdirectory has to resolve to the project's store");

    // Somewhere else entirely is not the same store, or the check would refuse everything.
    let other = tmp.path().join("site");
    std::fs::create_dir_all(other.join(".cyberbrain/notes")).unwrap();
    assert_ne!(store_of(&other).unwrap(), a);
    assert!(store_of(tmp.path()).is_none(), "no store above these two");
}

#[test]
fn a_project_is_named_by_its_folder() {
    let names = labels(&[
        PathBuf::from("/home/x/orderflow"),
        PathBuf::from("/home/x/my-app"),
    ]);
    assert_eq!(names, vec!["orderflow", "my-app"]);
}

/// Every action in this launcher is per project. Two entries reading the same thing is not
/// a cosmetic problem: it is how somebody sets up Claude for the wrong store.
#[test]
fn names_that_would_collide_grow_until_they_do_not() {
    let sep = std::path::MAIN_SEPARATOR_STR;
    let names = labels(&[
        PathBuf::from("/home/x/work/api"),
        PathBuf::from("/home/x/personal/api"),
        PathBuf::from("/home/x/site"),
    ]);
    assert_eq!(
        names,
        vec![
            format!("work{sep}api"),
            format!("personal{sep}api"),
            // The one that never collided is left short.
            "site".to_string(),
        ]
    );
}

#[test]
fn a_name_that_runs_out_of_folders_stops_rather_than_looping() {
    // Not reachable through the folder picker, and the point is that it terminates.
    let names = labels(&[PathBuf::from("/"), PathBuf::from("")]);
    assert_eq!(names.len(), 2);
}

/// [`start`], retried past the one failure that is this test module's own doing.
///
/// The fakes below are shell scripts the tests write and then execute. On Linux a program
/// cannot be executed while any file descriptor to it is open for writing, and with tests
/// running in parallel one thread's `fork` inherits another thread's still-open write
/// handle — so an unlucky run gets `ETXTBSY` from a file that is perfectly fine. It failed
/// perhaps one run in ten, which is worse than failing always, because a flake gets blamed
/// on whatever was being changed at the time. The launcher never writes the program it
/// starts, so this belongs here and not in `start`. Unix only, because the two tests that
/// write a fake are: `ETXTBSY` has no Windows counterpart.
#[cfg(unix)]
fn start_fake(server: &Path, dir: &Path) -> Result<Server, StartError> {
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(10);
    loop {
        match start(server, dir) {
            Err(StartError::Failed { message, output })
                if message.contains("Text file busy") && std::time::Instant::now() < deadline =>
            {
                let _ = output;
                std::thread::sleep(std::time::Duration::from_millis(25));
            }
            other => return other,
        }
    }
}

/// The binary under test, built by the same `cargo test` run that gets here.
fn server_binary() -> PathBuf {
    // target/<profile>/deps/<test binary> -> target/<profile>/cyberbrain
    let mut dir = std::env::current_exe().expect("test binary path");
    dir.pop();
    if dir.ends_with("deps") {
        dir.pop();
    }
    let exe = dir.join(if cfg!(windows) {
        "cyberbrain.exe"
    } else {
        "cyberbrain"
    });
    assert!(
        exe.is_file(),
        "cyberbrain is not built at {}; run `cargo build -p cyberbrain` first",
        exe.display()
    );

    exe
}

/// The tests below start the real `cyberbrain` and talk HTTP to it, which needs a binary
/// with the page compiled in. `--no-default-features` is a supported build whose `serve`
/// refuses to start, and it is exactly what CI's `test` job produces — so those runs
/// reported six defects that were not there while the same tests were green on a checkout
/// with `ui/dist` built. They stand down here and say why; the `ui` job, which builds the
/// page first, is where they actually run.
fn start_or_skip(server: &Path, dir: &Path, what: &str) -> Option<Server> {
    match start(server, dir) {
        Ok(running) => Some(running),
        Err(StartError::NoPage) => {
            eprintln!(
                "skipped {what}: this cyberbrain was built without its page \
                 (build it with --features ui to run this test)"
            );
            None
        }
        Err(e) => panic!("{what}: {e}"),
    }
}

fn get(url: &str, path: &str) -> String {
    let hostport = url
        .trim_start_matches("http://")
        .trim_end_matches('/')
        .to_string();
    let mut s = TcpStream::connect(&hostport).expect("connect to the server we started");
    write!(
        s,
        "GET {path} HTTP/1.1\r\nHost: {hostport}\r\nConnection: close\r\n\r\n"
    )
    .unwrap();
    let mut body = String::new();
    s.read_to_string(&mut body).unwrap();
    body
}

#[test]
fn it_starts_the_real_server_and_the_api_answers() {
    let server = server_binary();
    let tmp = tempfile::tempdir().unwrap();
    init_store(&server, tmp.path()).expect("init a store to serve");

    let Some(mut running) = start_or_skip(&server, tmp.path(), "the api answers") else {
        return;
    };
    assert!(
        running.url.starts_with("http://127.0.0.1:"),
        "{}",
        running.url
    );
    // Port 0 means the OS picks, so it is never the default one.
    assert!(!running.url.contains(":7777"), "{}", running.url);

    let response = get(&running.url, "/api/v1/status");
    assert!(response.starts_with("HTTP/1.1 200"), "{response}");
    assert!(response.contains("\"index\""), "{response}");

    assert!(
        running.exit_status().is_none(),
        "it died while we talked to it"
    );
    running.stop();
}

/// The launcher starts `serve --terminal`, so a project it opens has one. Which means it
/// has to read the *second* address `serve` prints — the one carrying the token — and open
/// that, while `url` stays the plain address that goes in the menu and the instance file.
#[test]
fn a_started_server_offers_a_terminal_and_the_launcher_finds_its_address() {
    let server = server_binary();
    let tmp = tempfile::tempdir().unwrap();
    init_store(&server, tmp.path()).expect("init a store to serve");
    let Some(mut running) = start_or_skip(&server, tmp.path(), "the terminal address") else {
        return;
    };

    assert!(
        running.open_url.contains("#/?t="),
        "the launcher has to open the address with the token in it: {}",
        running.open_url
    );
    assert!(
        !running.url.contains("t="),
        "and the plain address must stay plain: it goes in the instance file: {}",
        running.url
    );
    assert!(
        running
            .open_url
            .starts_with(running.url.trim_end_matches('/'))
    );
    running.stop();
}

#[test]
fn a_line_without_a_token_is_not_a_terminal_address() {
    assert!(parse_token_url("cyberbrain serve: http://127.0.0.1:7777/").is_none());
    assert_eq!(
        parse_token_url("open this: http://127.0.0.1:7777/#/?t=abc123").as_deref(),
        Some("http://127.0.0.1:7777/#/?t=abc123")
    );
}

#[test]
fn two_projects_can_be_open_at_once() {
    let server = server_binary();
    let a = tempfile::tempdir().unwrap();
    let b = tempfile::tempdir().unwrap();
    init_store(&server, a.path()).unwrap();
    init_store(&server, b.path()).unwrap();

    let (Some(one), Some(two)) = (
        start_or_skip(&server, a.path(), "two at once (first)"),
        start_or_skip(&server, b.path(), "two at once (second)"),
    ) else {
        return;
    };
    assert_ne!(one.url, two.url, "both landed on the same port");
}

#[test]
fn a_folder_without_a_store_is_the_recoverable_error() {
    let server = server_binary();
    let tmp = tempfile::tempdir().unwrap();
    match start(&server, tmp.path()) {
        Err(StartError::NoStore { dir }) => assert_eq!(dir, tmp.path()),
        Err(e) => panic!("expected NoStore, got {e}"),
        Ok(_) => panic!("expected NoStore, but it started in a folder with no store"),
    }
}

#[test]
fn init_makes_that_same_folder_servable() {
    let server = server_binary();
    let tmp = tempfile::tempdir().unwrap();
    assert!(start(&server, tmp.path()).is_err());
    init_store(&server, tmp.path()).expect("init");
    let Some(mut running) = start_or_skip(&server, tmp.path(), "serve after init") else {
        return;
    };
    running.stop();
}

#[test]
fn stopping_it_actually_stops_it() {
    let server = server_binary();
    let tmp = tempfile::tempdir().unwrap();
    init_store(&server, tmp.path()).unwrap();
    let Some(mut running) = start_or_skip(&server, tmp.path(), "stop() stops it") else {
        return;
    };
    let url = running.url.clone();
    running.stop();
    assert!(
        running.exit_status().is_some(),
        "still running after stop()"
    );
    let hostport = url
        .trim_start_matches("http://")
        .trim_end_matches('/')
        .to_string();
    assert!(
        TcpStream::connect(&hostport).is_err(),
        "the port still answers after stop()"
    );
}

#[test]
fn the_address_a_real_server_reports_is_accepted() {
    let server = server_binary();
    let tmp = tempfile::tempdir().unwrap();
    init_store(&server, tmp.path()).unwrap();
    let Some(mut running) = start_or_skip(&server, tmp.path(), "its own address is accepted")
    else {
        return;
    };
    assert_eq!(
        loopback_url(&running.url).as_deref(),
        Some(running.url.as_str()),
        "the launcher would refuse an address it produced itself"
    );
    running.stop();
}

#[test]
fn only_loopback_survives_the_instance_file() {
    // The point of the check: someone able to write the state file must not be able to send
    // the browser anywhere they like.
    for url in [
        "http://example.com:80/",
        "http://10.0.0.5:7777/",
        "http://127.0.0.1.example.com:80/",
        "https://127.0.0.1:7777/",
        "file:///etc/passwd",
        "http://127.0.0.1:0/",
        "http://127.0.0.1:notaport/",
        "http://127.0.0.1/",
        "http://",
        "",
    ] {
        assert!(loopback_url(url).is_none(), "{url:?} was accepted");
    }
    assert_eq!(
        loopback_url("http://127.0.0.1:54312/").as_deref(),
        Some("http://127.0.0.1:54312/")
    );
    // With or without the trailing slash, and with whitespace a file might carry.
    assert_eq!(
        loopback_url(" http://127.0.0.1:54312 ").as_deref(),
        Some("http://127.0.0.1:54312/")
    );
}

#[test]
fn the_binary_beside_us_wins_over_one_on_the_path() {
    let tmp = tempfile::tempdir().unwrap();
    let name = if cfg!(windows) {
        "cyberbrain.exe"
    } else {
        "cyberbrain"
    };
    let beside = tmp.path().join(name);
    std::fs::write(&beside, b"not really a binary").unwrap();
    assert_eq!(locate_server(Some(tmp.path())), Some(beside));
}

#[test]
fn without_one_beside_us_the_path_is_searched() {
    let tmp = tempfile::tempdir().unwrap();
    let empty = tmp.path().join("empty");
    std::fs::create_dir_all(&empty).unwrap();
    // Whatever this machine has on PATH is not our business; what matters is that an
    // empty neighbour directory does not stop the search.
    if let Some(p) = locate_server(Some(&empty)) {
        assert!(p.is_file());
    }
}

/// The menu entry behind "Set up Claude on this computer…", against the real binary.
///
/// The point of the entry is that a person never types a command, so what is checked here
/// is the thing they cannot: that the launcher's idea of the command matches the CLI's, and
/// that the store it lands on is the project the tray icon has open. A stale argument here
/// would show up on a customer's machine as a message box full of clap's help text.
#[test]
fn the_menu_entry_sets_up_the_project_the_launcher_has_open() {
    let server = server_binary();
    let tmp = tempfile::tempdir().unwrap();
    init_store(&server, tmp.path()).expect("init a store to set up");

    let said = set_up_clients(&server, tmp.path()).expect("the CLI accepted the command");
    assert!(said.contains("claude-code"), "{said}");

    let settings = tmp.path().join(".claude").join("settings.json");
    let text = std::fs::read_to_string(&settings).expect("the hooks were written");
    assert!(text.contains("session-start"), "{text}");
    // `text` is JSON, and JSON writes every backslash twice. Comparing against the raw path
    // means this can only ever hold where paths have no backslashes — it passed here and
    // failed on Windows, which is the platform the whole entry exists for.
    let named = server.display().to_string().replace('\\', "\\\\");
    assert!(
        text.contains(&named),
        "the hook has to name the binary the launcher is running, not whatever is on PATH: {text}"
    );
}

/// A folder with no store is the one failure this can actually hit: the tray icon can be
/// pointed anywhere. It has to come back as a message, not as a silent nothing.
#[test]
fn setting_up_without_a_store_says_so() {
    let server = server_binary();
    let tmp = tempfile::tempdir().unwrap();
    let why = set_up_clients(&server, tmp.path()).expect_err("there is no store here");
    assert!(why.contains("cyberbrain init"), "{why}");
}

// ---- the delivery schedule ----
//
// Whether an enrolled machine actually sends anything is a matter of arithmetic in a loop
// that ticks once a second, and arithmetic in a loop that ticks once a second is exactly
// the sort of thing that quietly never fires. So the clock is testable without a hub, a
// network or Windows: what starts a delivery is handed in.

fn primed(p: Pushed) -> std::sync::mpsc::Receiver<Pushed> {
    let (tx, rx) = std::sync::mpsc::channel();
    tx.send(p).unwrap();
    rx
}

/// Run `n` ticks, counting how many deliveries were started, each answering `answer`.
fn ticks(d: &mut Delivery, n: u32, answer: Pushed) -> u32 {
    let mut started = 0;
    for _ in 0..n {
        d.tick_with(|| {
            started += 1;
            primed(answer.clone())
        });
    }
    started
}

#[test]
fn a_fresh_launcher_delivers_soon_and_then_every_quarter_hour() {
    let mut d = Delivery::default();
    // The first half minute is quiet: the store is still opening.
    assert_eq!(ticks(&mut d, 29, Pushed::Delivered), 0);
    // Then one, and not a second one straight after it.
    assert_eq!(ticks(&mut d, 1, Pushed::Delivered), 1);
    assert_eq!(ticks(&mut d, 60, Pushed::Delivered), 0);
    // A quarter of an hour after the first, the next.
    assert_eq!(ticks(&mut d, 15 * 60, Pushed::Delivered), 1);
}

#[test]
fn a_store_that_belongs_to_no_hub_is_asked_once() {
    let mut d = Delivery::default();
    assert_eq!(ticks(&mut d, 30, Pushed::NotEnrolled), 1);
    // Most machines are not enrolled. Spawning a process every quarter hour for the rest of
    // the day to be told so again is work nobody asked for.
    assert_eq!(ticks(&mut d, 4 * 60 * 60, Pushed::NotEnrolled), 0);
}

#[test]
fn a_hub_that_did_not_answer_is_tried_again() {
    let mut d = Delivery::default();
    assert_eq!(ticks(&mut d, 30, Pushed::Failed), 1);
    // A train, a hotel wifi, a server being patched. None of those mean "not enrolled",
    // and a launcher that gave up on the first failure would deliver nothing all week.
    assert_eq!(ticks(&mut d, 15 * 60, Pushed::Failed), 1);
}

#[test]
fn enrolling_from_the_menu_delivers_at_once() {
    let mut d = Delivery::now();
    // The person who just clicked Connect is the one who wants to see it arrive, so this
    // does not wait for the next quarter hour — or even the first half minute.
    assert_eq!(ticks(&mut d, 1, Pushed::Delivered), 1);
}

#[test]
fn a_slow_delivery_does_not_start_a_second_one() {
    let mut d = Delivery::default();
    // The sender stays alive for the whole test, so this receiver is "in flight" rather
    // than "the thread died" — the two are different and the code treats them differently.
    let (_hold, never) = std::sync::mpsc::channel::<Pushed>();
    let mut never = Some(never);
    let mut started = 0;
    // Half a minute to the first attempt, then an hour of ticks while it hangs. Still one
    // attempt: a hub that stopped answering must not collect a queue of pushes aimed at it.
    for _ in 0..(30 + 3600) {
        d.tick_with(|| {
            started += 1;
            never
                .take()
                .expect("only the first tick may start a delivery here")
        });
    }
    assert_eq!(started, 1);
}

// ---- a cyberbrain that has no page to open ----
//
// `--no-default-features` builds a working CLI with no web UI in it. That is a supported
// build, and the CI installer was made from one: clicking the Start menu entry opened a
// browser on a JSON error where a program should have been. The launcher's whole job is to
// open that page, so it has to notice and say so.

#[test]
fn the_marker_matches_what_serve_prints() {
    let src = Path::new(env!("CARGO_MANIFEST_DIR")).join("../cyberbrain/src/serve/mod.rs");
    let Ok(text) = std::fs::read_to_string(&src) else {
        // Built on its own, without its neighbour in the tree. Nothing to compare against,
        // and inventing a pass would be worse than not looking.
        return;
    };
    // These are two programs, not two modules: the launcher runs whichever cyberbrain.exe
    // is beside it. A shared constant would be a compile-time promise about a runtime
    // relationship, so the promise is checked here instead.
    let want = format!("NO_PAGE_MARKER: &str = {NO_PAGE_MARKER:?}");
    assert!(
        text.contains(&want),
        "serve no longer prints what the launcher listens for.\nlauncher expects: {want}"
    );
}

#[cfg(unix)]
#[test]
fn a_build_without_a_page_is_reported_instead_of_opened() {
    use std::os::unix::fs::PermissionsExt;
    let tmp = tempfile::tempdir().unwrap();
    std::fs::create_dir_all(tmp.path().join(".cyberbrain/notes")).unwrap();

    // A stand-in for a cyberbrain built without its page: it says so, then prints an
    // address exactly as the real one would. The address must not win.
    let fake = tmp.path().join("cyberbrain");
    std::fs::write(
        &fake,
        format!(
            "#!/bin/sh\necho '{NO_PAGE_MARKER}'\n\
             echo 'cyberbrain serve: http://127.0.0.1:44444/  (loopback only)'\n\
             sleep 20\n"
        ),
    )
    .unwrap();
    std::fs::set_permissions(&fake, std::fs::Permissions::from_mode(0o755)).unwrap();

    match start_fake(&fake, tmp.path()) {
        Err(StartError::NoPage) => {}
        Err(other) => panic!("expected the missing page to be reported, got {other}"),
        Ok(mut running) => {
            running.stop();
            panic!("the launcher opened a browser at a build with no page");
        }
    }
}

#[cfg(unix)]
#[test]
fn an_ordinary_build_is_still_opened() {
    use std::os::unix::fs::PermissionsExt;
    let tmp = tempfile::tempdir().unwrap();
    std::fs::create_dir_all(tmp.path().join(".cyberbrain/notes")).unwrap();
    // The same stand-in without the marker. Calibration for the test above: if the launcher
    // had simply started refusing to open anything, this would fail too.
    let fake = tmp.path().join("cyberbrain");
    std::fs::write(
        &fake,
        "#!/bin/sh\necho 'cyberbrain serve: http://127.0.0.1:44444/  (loopback only)'\nsleep 20\n",
    )
    .unwrap();
    std::fs::set_permissions(&fake, std::fs::Permissions::from_mode(0o755)).unwrap();

    let mut running = start_fake(&fake, tmp.path()).expect("an address, and no complaint");
    assert_eq!(running.url, "http://127.0.0.1:44444/");
    running.stop();
}

/// The rule behind the comparison above, stated where it can be seen. On Linux the paths in
/// that file have no backslashes, so the assertion held for runs that proved nothing about
/// the platform the menu entry is for; on Windows it could not hold at all.
#[test]
fn a_windows_path_is_not_in_json_unless_its_backslashes_are_doubled() {
    let path = r"D:\a\cyberbrain\target\debug\cyberbrain.exe";
    let json = r#"{"command": "D:\\a\\cyberbrain\\target\\debug\\cyberbrain.exe"}"#;
    assert!(
        !json.contains(path),
        "a raw Windows path is not what the file holds"
    );
    assert!(
        json.contains(&path.replace('\\', "\\\\")),
        "and the doubled one is"
    );
}
