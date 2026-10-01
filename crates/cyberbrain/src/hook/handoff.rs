//! The newest handoff note, shown at session start (2026-10-01).
//!
//! A handoff is what one session leaves for the next: what was done, what to run first,
//! what is open. It lives in ring 2 or 3, so `recall` finds it, but only for somebody who
//! already thinks to ask, and a fresh session does not. The case that prompted this: after
//! a reboot the next session checked the machine by hand and never ran the check script
//! the handoff named, because nothing told it the handoff existed.
//!
//! Found by file name, with `read_dir` over `notes/r2` and `notes/r3`, not through the
//! index: the hook has a budget of milliseconds, the index may be behind the tree, and the
//! tree is authoritative. Only the files whose names match are read and parsed.
//!
//! What is shown is labelled as what it is. It is not resident: it does not outrank rings 0
//! and 1, it is not re-injected on a prompt, and it is dated so the agent can tell how much
//! may have happened since.

use cyberbrain_core::blocks::{MAX_BLOCK_TOKENS, approx_tokens, blocks_of};
use cyberbrain_core::config::HandoffConfig;
use cyberbrain_core::{Block, Note, Ring, Store};

pub struct Handoff {
    pub note: Note,
    pub blocks: Vec<Block>,
    /// Seconds between the note's `updated` and now; never negative.
    pub age_secs: i64,
}

/// The newest note in ring 2 or 3 whose file name matches, if one is young enough. A note
/// whose `invalid_at` has passed is not a handoff any more, whatever its name says.
pub fn latest(store: &Store, cfg: &HandoffConfig, now: jiff::Timestamp) -> Option<Handoff> {
    let patterns: Vec<String> = cfg
        .name_contains
        .iter()
        .map(|p| p.trim().to_lowercase())
        .filter(|p| !p.is_empty())
        .collect();
    if patterns.is_empty() {
        return None;
    }
    let max_age = i64::from(cfg.max_age_days) * 86_400;
    let mut best: Option<(Note, i64)> = None;
    for ring in [Ring::Knowledge, Ring::Session] {
        let Ok(entries) = std::fs::read_dir(store.ring_dir(ring)) else {
            continue;
        };
        for entry in entries.flatten() {
            let path = entry.path();
            if path.extension().and_then(|e| e.to_str()) != Some("md") {
                continue;
            }
            let Some(stem) = path.file_stem().and_then(|s| s.to_str()) else {
                continue;
            };
            let stem = stem.to_lowercase();
            if stem.starts_with('.') || !patterns.iter().any(|p| stem.contains(p.as_str())) {
                continue;
            }
            // A file that does not parse is not a handoff; recall and doctor report it.
            let Ok(note) = store.read_path(&path) else {
                continue;
            };
            if note.front.invalid_at.is_some_and(|t| t <= now) {
                continue;
            }
            let age = (now.as_second() - note.front.updated.as_second()).max(0);
            if age > max_age {
                continue;
            }
            let newer = match &best {
                None => true,
                Some((b, _)) => {
                    (note.front.updated, &note.front.name) > (b.front.updated, &b.front.name)
                }
            };
            if newer {
                best = Some((note, age));
            }
        }
    }
    let (note, age_secs) = best?;
    let (blocks, _oversized) = blocks_of(&note, MAX_BLOCK_TOKENS);
    Some(Handoff {
        note,
        blocks,
        age_secs,
    })
}

fn age_words(secs: i64) -> String {
    match secs {
        s if s < 3_600 => format!("{} min ago", s / 60),
        s if s < 2 * 86_400 => format!("{} h ago", s / 3_600),
        s => format!("{} days ago", s / 86_400),
    }
}

pub fn render(h: &Handoff, cfg: &HandoffConfig, out: &mut String) {
    let f = &h.note.front;
    out.push_str(&format!("## Latest handoff ({}, not resident)\n\n", f.ring));
    out.push_str(&format!(
        "The newest note in rings 2/3 whose name contains {}, updated {} ({}). It is what an \
         earlier session left for the next one, not an invariant: rings 0 and 1 outrank it, \
         and what it calls open may have been done since. Before acting on it, check for \
         newer notes with `cyberbrain recall`.\n\n",
        cfg.name_contains
            .iter()
            .map(|p| format!("`{p}`"))
            .collect::<Vec<_>>()
            .join(" or "),
        f.updated.strftime("%Y-%m-%d %H:%M UTC"),
        age_words(h.age_secs)
    ));
    out.push_str(&format!(
        "### {} `{}` (kind: {}, updated: {})\n",
        f.ring,
        f.name,
        super::resident::kind_name(f.kind),
        f.updated.strftime("%Y-%m-%d")
    ));
    let mut used = 0usize;
    let mut left_out: Vec<String> = Vec::new();
    for b in &h.blocks {
        let t = approx_tokens(&b.text) as usize;
        if !left_out.is_empty() || (used > 0 && used + t > cfg.max_tokens) {
            left_out.push(b.citation.to_string());
            continue;
        }
        used += t;
        out.push_str(&format!("[{}]\n{}\n\n", b.citation, b.text.trim_end()));
    }
    if h.blocks.is_empty() {
        out.push_str("(empty body)\n\n");
    }
    if !left_out.is_empty() {
        out.push_str(&format!(
            "Left out over handoff.max_tokens ({}): {}. Expand with `cyberbrain recall --id`.\n\n",
            cfg.max_tokens,
            left_out.join(", ")
        ));
    }
}
