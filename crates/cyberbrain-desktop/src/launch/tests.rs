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

    let mut running = start(&server, tmp.path()).expect("serve reports an address");
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

#[test]
fn two_projects_can_be_open_at_once() {
    let server = server_binary();
    let a = tempfile::tempdir().unwrap();
    let b = tempfile::tempdir().unwrap();
    init_store(&server, a.path()).unwrap();
    init_store(&server, b.path()).unwrap();

    let one = start(&server, a.path()).expect("first");
    let two = start(&server, b.path()).expect("second");
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
    let mut running = start(&server, tmp.path()).expect("serve after init");
    running.stop();
}

#[test]
fn stopping_it_actually_stops_it() {
    let server = server_binary();
    let tmp = tempfile::tempdir().unwrap();
    init_store(&server, tmp.path()).unwrap();
    let mut running = start(&server, tmp.path()).unwrap();
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
