//! One hook call per event, even when the harness runs the hook twice (2026-10-04).
//!
//! Claude Code merges hooks from every source: a plugin's `hooks/hooks.json` and the
//! `settings.json` entries that `cyberbrain install` writes. Someone who installed the binary
//! first and the plugin later would get every event twice: two session-start injections,
//! two pre-tool-use verdicts, two governance tickets for one tool call. `install` removes its
//! own entries once it sees the plugin, but a hand-written entry or an older binary on the
//! PATH would still double up, so the hook guards itself as well.
//!
//! Both runs receive the same payload byte for byte, and they start within milliseconds of
//! each other. The first to create `sessions/.claims/<hash>` does the work; the second finds
//! the file and stands down with empty stdout, which the harness reads as "no opinion", so
//! the first run's verdict is the only one. A claim older than [`WINDOW`] does not count:
//! the same prompt typed again later is a new event, not a duplicate.

use std::path::{Path, PathBuf};
use std::time::{Duration, SystemTime};

use super::session::sessions_dir;

/// How long a claim blocks an identical payload. Duplicates arrive within milliseconds,
/// because the harness starts both hooks at once. Two seconds, not more: a `stop` payload
/// carries no turn number, so two genuine turns can look identical, and a turn that ends
/// within two seconds of the previous one is not one a person typed.
pub const WINDOW: Duration = Duration::from_secs(2);

/// Claims older than this are removed when a new one is written.
const SWEEP_AFTER: Duration = Duration::from_secs(600);

fn claims_dir(root: &Path) -> PathBuf {
    sessions_dir(root).join(".claims")
}

/// `true`: this run goes ahead. `false`: an identical run claimed this payload within
/// [`WINDOW`]; stand down. Any I/O failure answers `true`: a hook that does its work twice
/// is a nuisance, a hook that silently does nothing is a hole.
pub fn claim(root: &Path, event: &str, stdin: &str, now: SystemTime) -> bool {
    let dir = claims_dir(root);
    if std::fs::create_dir_all(&dir).is_err() {
        return true;
    }
    let mut h = blake3::Hasher::new();
    h.update(event.as_bytes());
    h.update(b"\0");
    h.update(stdin.as_bytes());
    let path = dir.join(h.finalize().to_hex().as_str());

    match std::fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(&path)
    {
        Ok(_) => {
            sweep(&dir, now);
            true
        }
        Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists => {
            // A claim stamped after `now` is age zero: macOS and Windows keep file times finer
            // than the `now` read before the write (CI, 2026-10-04).
            let stamped = std::fs::metadata(&path).and_then(|m| m.modified()).ok();
            let fresh = stamped
                .map(|t| now.duration_since(t).unwrap_or(Duration::ZERO))
                .is_some_and(|age| age < WINDOW);
            if fresh {
                return false;
            }
            // Stale: the same payload long ago. Only one of the duplicates that found it may
            // take it over. Removing and recreating it let every one of them go ahead, so the
            // takeover is itself a claim, named after the stale stamp: every run that saw the
            // same old claim races for the same name, and `create_new` lets exactly one win.
            let Some(stamp) = stamped.and_then(|t| t.duration_since(SystemTime::UNIX_EPOCH).ok())
            else {
                return true;
            };
            let mut takeover = path.clone().into_os_string();
            takeover.push(format!(".{}", stamp.as_nanos()));
            match std::fs::OpenOptions::new()
                .write(true)
                .create_new(true)
                .open(&takeover)
            {
                Ok(_) => {
                    // Fresh again, so a duplicate that arrives after this one stands down on
                    // the ordinary path above.
                    let _ = std::fs::File::options()
                        .write(true)
                        .open(&path)
                        .and_then(|f| f.set_modified(SystemTime::now()));
                    true
                }
                Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists => false,
                Err(_) => true,
            }
        }
        Err(_) => true,
    }
}

fn sweep(dir: &Path, now: SystemTime) {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return;
    };
    for e in entries.flatten() {
        let old = e
            .metadata()
            .and_then(|m| m.modified())
            .ok()
            .and_then(|t| now.duration_since(t).ok())
            .is_some_and(|age| age > SWEEP_AFTER);
        if old {
            let _ = std::fs::remove_file(e.path());
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_second_identical_call_stands_down_and_a_different_one_does_not() {
        let tmp = tempfile::tempdir().unwrap();
        let now = SystemTime::now();
        let p = r#"{"session_id":"s1","tool_use_id":"t1"}"#;
        assert!(claim(tmp.path(), "pre-tool-use", p, now));
        assert!(
            !claim(tmp.path(), "pre-tool-use", p, now),
            "duplicate must stand down"
        );
        assert!(
            claim(tmp.path(), "post-tool-use", p, now),
            "other event, same payload"
        );
        assert!(claim(
            tmp.path(),
            "pre-tool-use",
            r#"{"session_id":"s1","tool_use_id":"t2"}"#,
            now
        ));
    }

    #[test]
    fn a_claim_stamped_after_now_still_blocks() {
        // CI 2026-10-04: macOS and Windows store file times finer than Linux, so the claim
        // file can be younger than the `now` taken before it was written. That is age zero,
        // not "stale".
        let tmp = tempfile::tempdir().unwrap();
        let earlier = SystemTime::now() - Duration::from_secs(1);
        let p = r#"{"session_id":"s1","tool_use_id":"t9"}"#;
        assert!(claim(tmp.path(), "pre-tool-use", p, earlier));
        assert!(
            !claim(tmp.path(), "pre-tool-use", p, earlier),
            "duplicate must stand down"
        );
    }

    #[test]
    fn a_claim_outside_the_window_does_not_block_a_genuine_repeat() {
        let tmp = tempfile::tempdir().unwrap();
        let p = r#"{"session_id":"s1","prompt":"weiter"}"#;
        assert!(claim(
            tmp.path(),
            "user-prompt-submit",
            p,
            SystemTime::now()
        ));
        let later = SystemTime::now() + WINDOW + Duration::from_secs(1);
        assert!(claim(tmp.path(), "user-prompt-submit", p, later));
    }

    /// Two duplicates that both find an old claim must not both take it over. The takeover was
    /// remove-then-create with the second create's failure ignored, so both went ahead.
    #[test]
    fn two_duplicates_racing_for_a_stale_claim_run_once() {
        for round in 0..50 {
            let tmp = tempfile::tempdir().unwrap();
            let p = format!(r#"{{"session_id":"s1","tool_use_id":"race{round}"}}"#);
            assert!(claim(tmp.path(), "pre-tool-use", &p, SystemTime::now()));
            // The same payload long ago: age the claim instead of moving the clock, so that a
            // claim taken over during the race is as fresh as it would really be.
            let file = std::fs::read_dir(claims_dir(tmp.path()))
                .unwrap()
                .flatten()
                .next()
                .unwrap()
                .path();
            std::fs::File::options()
                .write(true)
                .open(&file)
                .unwrap()
                .set_modified(SystemTime::now() - WINDOW - Duration::from_secs(5))
                .unwrap();
            let barrier = std::sync::Arc::new(std::sync::Barrier::new(6));
            let runs: Vec<_> = (0..6)
                .map(|_| {
                    let (root, p, barrier) = (tmp.path().to_path_buf(), p.clone(), barrier.clone());
                    std::thread::spawn(move || {
                        barrier.wait();
                        claim(&root, "pre-tool-use", &p, SystemTime::now())
                    })
                })
                .collect();
            let went = runs
                .into_iter()
                .map(|h| h.join().unwrap())
                .filter(|went| *went)
                .count();
            assert_eq!(went, 1, "round {round}: {went} duplicates ran");
        }
    }

    #[test]
    fn an_unwritable_store_never_stops_the_hook() {
        let tmp = tempfile::tempdir().unwrap();
        // A file where the sessions directory should be: create_dir_all fails.
        std::fs::write(tmp.path().join("sessions"), b"x").unwrap();
        let p = "{}";
        assert!(claim(tmp.path(), "session-start", p, SystemTime::now()));
        assert!(claim(tmp.path(), "session-start", p, SystemTime::now()));
    }
}
