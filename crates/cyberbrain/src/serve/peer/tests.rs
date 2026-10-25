//! The peer check, against made-up tables and against real connections.

use super::*;

fn addr(s: &str) -> SocketAddr {
    s.parse().unwrap()
}

fn row(local: &str, remote: &str, uid: u32) -> String {
    format!(
        "   7: {local} {remote} 01 00000000:00000000 00:00000000 00000000  {uid}        0 4242 1 0 20 4 30 10 -1"
    )
}

/// The table as the kernel prints it on this machine, for an address and a port.
fn hex(a: &str) -> String {
    let SocketAddr::V4(a) = addr(a) else {
        unreachable!()
    };
    format!(
        "{:08X}:{:04X}",
        u32::from_ne_bytes(a.ip().octets()),
        a.port()
    )
}

#[test]
fn the_client_half_of_the_connection_names_the_account() {
    let table = format!(
        "  sl  local_address rem_address   st tx_queue rx_queue tr tm->when retrnsmt   uid  timeout inode\n{}\n{}\n{}\n",
        // Our own listening socket and our half of the connection: not the answer.
        row(&hex("127.0.0.1:7777"), "00000000:0000", 1000),
        row(&hex("127.0.0.1:7777"), &hex("127.0.0.1:50000"), 1000),
        // The client's half.
        row(&hex("127.0.0.1:50000"), &hex("127.0.0.1:7777"), 1001),
    );
    let owner = uid_in(&table, addr("127.0.0.1:50000"), addr("127.0.0.1:7777"));
    assert_eq!(owner, Some(1001));
    assert!(decide(1000, owner).unwrap_err().contains("uid 1001"));
    assert!(decide(1001, owner).is_ok());
    assert!(decide(1000, Some(0)).is_ok(), "root reads the files anyway");
    assert!(decide(1000, None).is_err(), "unknown is refused");
    assert_eq!(
        uid_in(&table, addr("127.0.0.1:50001"), addr("127.0.0.1:7777")),
        None
    );
}

/// The real table, for a real connection from this process: our own account.
#[tokio::test]
async fn a_connection_from_this_account_is_found_and_admitted() {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let local = listener.local_addr().unwrap();
    let client = TcpStream::connect(local).await.unwrap();
    let (_server, peer) = listener.accept().await.unwrap();
    assert_eq!(peer, client.local_addr().unwrap());
    let table = std::fs::read_to_string("/proc/net/tcp").unwrap();
    let own = own_uid().unwrap();
    assert_eq!(uid_in(&table, peer, local), Some(own));
    assert!(admit(own, peer, local).is_ok());
}

/// The finding itself, end to end: another account connects to a served store and gets
/// nothing. Needs root to become another account, and bash for `/dev/tcp`; without
/// either it says so and checks nothing.
#[tokio::test]
async fn another_account_gets_no_answer() {
    use std::os::unix::process::CommandExt;
    if own_uid() != Some(0) || !std::path::Path::new("/bin/bash").exists() {
        eprintln!("skipped: needs root and /bin/bash to connect as another account");
        return;
    }
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let port = listener.local_addr().unwrap().port();
    let app = axum::Router::new().route("/", axum::routing::get(|| async { "the store" }));
    tokio::spawn(async move { axum::serve(own_user_only(listener), app).await });

    let fetch = move |uid: Option<u32>| {
        let mut cmd = std::process::Command::new("/bin/bash");
        cmd.arg("-c").arg(format!(
            "exec 3<>/dev/tcp/127.0.0.1/{port} || exit 0; \
             printf 'GET / HTTP/1.0\\r\\nHost: 127.0.0.1:{port}\\r\\n\\r\\n' >&3; \
             timeout 5 cat <&3"
        ));
        if let Some(uid) = uid {
            cmd.uid(uid).gid(uid);
        }
        String::from_utf8_lossy(&cmd.output().unwrap().stdout).into_owned()
    };
    let mine = tokio::task::spawn_blocking(move || fetch(None))
        .await
        .unwrap();
    assert!(mine.contains("the store"), "{mine:?}");
    let theirs = tokio::task::spawn_blocking(move || fetch(Some(65534)))
        .await
        .unwrap();
    assert!(!theirs.contains("the store"), "{theirs:?}");
}
