//! Tests for the index. Every regression test here was first run against the broken
//! state it covers (SPEC §14.2); `erasure_assertions_have_teeth` keeps that demonstration
//! in-tree for the one failure the design is most prone to.

use crate::{AuditFilter, AuditStore, NewAuditEntry};
use crate::{EmbeddingProfile, Index, RecallOptions, content_hash, vectors};
use cyberbrain_core::{
    Block, Citation, Embedder, Error, Frontmatter, Note, NoteId, NoteKind, PiiState, Result, Ring,
};
use std::path::PathBuf;

// ----- fixtures ---------------------------------------------------------------------------

/// Deterministic bag-of-words embedder: every word hashes to one dimension with a sign.
/// Good enough for "the block that shares words with the query scores highest".
struct HashEmbedder {
    id: String,
    dim: usize,
}

impl HashEmbedder {
    fn new(id: &str, dim: usize) -> Self {
        Self { id: id.into(), dim }
    }
}

fn fnv(s: &str) -> u64 {
    let mut h: u64 = 0xcbf2_9ce4_8422_2325;
    for b in s.bytes() {
        h ^= b as u64;
        h = h.wrapping_mul(0x0000_0100_0000_01b3);
    }
    h
}

impl Embedder for HashEmbedder {
    fn dim(&self) -> usize {
        self.dim
    }
    fn profile_id(&self) -> &str {
        &self.id
    }
    fn embed(&self, texts: &[&str]) -> Result<Vec<Vec<f32>>> {
        Ok(texts
            .iter()
            .map(|t| {
                let mut v = vec![0f32; self.dim];
                for w in t
                    .split(|c: char| !c.is_alphanumeric())
                    .filter(|w| !w.is_empty())
                {
                    let h = fnv(&w.to_lowercase());
                    let sign = if (h >> 32) & 1 == 0 { 1.0 } else { -1.0 };
                    v[(h % self.dim as u64) as usize] += sign;
                }
                vectors::normalize(&mut v);
                v
            })
            .collect())
    }
}

fn profile_of(e: &HashEmbedder) -> EmbeddingProfile {
    EmbeddingProfile {
        id: e.id.clone(),
        dim: e.dim,
        model_hash: format!("hash-of-{}", e.id),
    }
}

/// Deterministic ids so two builds of the same fixture agree.
fn id_for(name: &str) -> NoteId {
    let h = fnv(name);
    NoteId::from_parts(1_700_000_000_000 + (h % 1_000_000), h as u128)
}

fn note(name: &str, ring: Ring, body: &str, links: &[&str]) -> Note {
    let ts: jiff::Timestamp = "2026-09-05T09:12:03Z".parse().unwrap();
    Note {
        front: Frontmatter {
            id: id_for(name),
            name: name.into(),
            ring,
            kind: NoteKind::Knowledge,
            created: ts,
            updated: ts,
            tags: vec!["t".into()],
            links: links.iter().map(|s| s.to_string()).collect(),
            retention: None,
            pii: PiiState::None,
        },
        body: body.into(),
        path: PathBuf::from(format!("/nonexistent/{}/{name}.md", ring.dir())),
    }
}

/// Split at blank lines, the way the real splitter would at paragraph boundaries.
fn blocks_of(n: &Note) -> Vec<Block> {
    n.body
        .split("\n\n")
        .map(str::trim)
        .filter(|p| !p.is_empty())
        .enumerate()
        .map(|(i, p)| Block {
            citation: Citation::new(n.front.ring, n.front.id, i as u32, p),
            note_id: n.front.id,
            idx: i as u32,
            text: p.to_string(),
            token_count: p.split_whitespace().count() as u32,
        })
        .collect()
}

fn embed_blocks(e: &dyn Embedder, blocks: &[Block]) -> Vec<Vec<f32>> {
    let texts: Vec<&str> = blocks.iter().map(|b| b.text.as_str()).collect();
    e.embed(&texts).unwrap()
}

fn put(ix: &mut Index, e: &dyn Embedder, n: &Note) -> Vec<Block> {
    let blocks = blocks_of(n);
    let vecs = embed_blocks(e, &blocks);
    ix.upsert_note(n, &blocks, Some(&vecs)).unwrap();
    blocks
}

fn count(ix: &Index, sql: &str) -> i64 {
    ix.conn.query_row(sql, [], |r| r.get(0)).unwrap()
}

fn corpus() -> Vec<Note> {
    vec![
        note(
            "pg18-moves-pgdata",
            Ring::Knowledge,
            "Postgres 18 moves PGDATA to a versioned subdirectory.\n\n\
             An old bind mount then points at an empty directory and the service stays green \
             while data lands inside the container.",
            &["docker-bind-mount-inode-drift"],
        ),
        note(
            "docker-bind-mount-inode-drift",
            Ring::Knowledge,
            "A bind mount follows the inode, not the path.\n\n\
             Replacing a file with mv gives the container the old inode.",
            &[],
        ),
        note(
            "never-copy-config-to-s2",
            Ring::Invariant,
            "Never copy config.py to server two. Server two has its own.",
            &["pg18-moves-pgdata", "does-not-exist-yet"],
        ),
        note(
            "session-2026-09-05",
            Ring::Session,
            "Looked at postgres upgrade and the bind mount again today.\n\nNothing decided.",
            &[],
        ),
    ]
}

fn seeded(e: &HashEmbedder) -> Index {
    let mut ix = Index::open_in_memory().unwrap();
    ix.set_embedding_profile(&profile_of(e)).unwrap();
    for n in corpus() {
        put(&mut ix, e, &n);
    }
    ix
}

// ----- schema -----------------------------------------------------------------------------

#[test]
fn schema_migrates_and_is_idempotent() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("cyberbrain.db");
    {
        let ix = Index::open(&path).unwrap();
        assert_eq!(ix.schema_version().unwrap(), crate::SCHEMA_VERSION);
        assert_eq!(ix.generation().unwrap(), 0);
    }
    let ix = Index::open(&path).unwrap();
    assert_eq!(ix.schema_version().unwrap(), crate::SCHEMA_VERSION);
    let tables: Vec<String> = {
        let mut s = ix
            .conn
            .prepare("SELECT name FROM sqlite_master WHERE type = 'table' ORDER BY name")
            .unwrap();
        s.query_map([], |r| r.get(0))
            .unwrap()
            .map(|r| r.unwrap())
            .collect()
    };
    for t in ["notes", "blocks", "blocks_fts", "vectors", "links", "meta"] {
        assert!(
            tables.contains(&t.to_string()),
            "missing table {t}: {tables:?}"
        );
    }
    assert!(
        !tables.contains(&"audit".to_string()),
        "the audit record must not live in the cache: {tables:?}"
    );
}

#[test]
fn newer_schema_is_refused() {
    let ix = Index::open_in_memory().unwrap();
    ix.conn
        .execute(
            "UPDATE meta SET value = '999' WHERE key = 'schema_version'",
            [],
        )
        .unwrap();
    let mut conn = ix.conn;
    let err = crate::schema::migrate(&mut conn).unwrap_err();
    assert!(err.to_string().contains("999"), "{err}");
}

// ----- upsert / recall --------------------------------------------------------------------

#[test]
fn upsert_then_recall_hybrid() {
    let e = HashEmbedder::new("test-v1", 256);
    let ix = seeded(&e);
    let st = ix.stats().unwrap();
    assert_eq!(st.notes, 4);
    assert_eq!(st.blocks, 7);
    assert_eq!(st.fts_rows, 7);
    assert_eq!(st.vectors, 7);
    assert_eq!(st.links, 3);
    assert_eq!(st.dangling_links, 1);

    let r = ix
        .recall("bind mount inode", Some(&e), &RecallOptions::default())
        .unwrap();
    assert!(r.caveats.is_empty(), "{:?}", r.caveats);
    assert!(!r.hits.is_empty());
    assert_eq!(r.hits[0].note_name, "docker-bind-mount-inode-drift");
    assert!(r.hits[0].text.contains("inode"));
    for h in &r.hits {
        assert!(
            h.citation.parse::<Citation>().is_ok(),
            "hit without a citation"
        );
        assert!(h.score > 0.0);
    }
    // A hit resolves back to its block.
    let c: Citation = r.hits[0].citation.parse().unwrap();
    let (b, n) = ix.resolve(&c).unwrap().expect("resolves");
    assert_eq!(b.text, r.hits[0].text);
    assert_eq!(n.front.name, "docker-bind-mount-inode-drift");
}

#[test]
fn lexical_only_when_no_embedder_and_says_so() {
    let e = HashEmbedder::new("test-v1", 256);
    let ix = seeded(&e);
    let r = ix
        .recall("PGDATA", None, &RecallOptions::default())
        .unwrap();
    assert_eq!(r.hits.len(), 1);
    assert_eq!(r.hits[0].note_name, "pg18-moves-pgdata");
    assert!(
        r.caveats
            .iter()
            .any(|c| c.contains("semantic search skipped")),
        "{:?}",
        r.caveats
    );
}

#[test]
fn semantic_finds_what_lexical_cannot() {
    // Lexical needs the token; the hash embedder scores shared words, so ask with a word
    // that FTS will not match after our tokeniser but the embedder will.
    let e = HashEmbedder::new("test-v1", 256);
    let ix = seeded(&e);
    let r = ix
        .recall(
            "versioned subdirectory",
            Some(&e),
            &RecallOptions::default(),
        )
        .unwrap();
    assert_eq!(r.hits[0].note_name, "pg18-moves-pgdata");
    // And with k_lex = 0 the semantic half alone still answers.
    let r = ix
        .recall(
            "versioned subdirectory",
            Some(&e),
            &RecallOptions {
                k_lex: 0,
                ..Default::default()
            },
        )
        .unwrap();
    assert_eq!(r.hits[0].note_name, "pg18-moves-pgdata");
}

#[test]
fn ring_weight_orders_equal_matches() {
    let e = HashEmbedder::new("test-v1", 256);
    let mut ix = Index::open_in_memory().unwrap();
    ix.set_embedding_profile(&profile_of(&e)).unwrap();
    let text = "the quick brown fox";
    put(&mut ix, &e, &note("low", Ring::External, text, &[]));
    put(&mut ix, &e, &note("high", Ring::Invariant, text, &[]));
    put(&mut ix, &e, &note("mid", Ring::Knowledge, text, &[]));
    let r = ix
        .recall("quick fox", Some(&e), &RecallOptions::default())
        .unwrap();
    let names: Vec<&str> = r.hits.iter().map(|h| h.note_name.as_str()).collect();
    assert_eq!(names, ["high", "mid", "low"]);
    // Identical texts tie on BM25 and cosine and are then ordered by citation, so each
    // sits at a different rank in each list; check against the exact RRF sum instead.
    let rrf = |rl: f32, rs: f32| 1.0 / (60.0 + rl) + 1.0 / (60.0 + rs);
    let mut lex: Vec<&str> = ["high", "mid", "low"].to_vec();
    let cit_of = |n: &str| {
        r.hits
            .iter()
            .find(|h| h.note_name == n)
            .unwrap()
            .citation
            .clone()
    };
    lex.sort_by_key(|n| cit_of(n));
    let rank = |n: &str| lex.iter().position(|x| *x == n).unwrap() as f32 + 1.0;
    // Derived from Ring::weight(), never a literal: a copy of the weights here is a second
    // source of truth and it drifts the moment the real ones are retuned.
    let weights = [
        Ring::Invariant.weight(),
        Ring::Knowledge.weight(),
        Ring::External.weight(),
    ];
    for (h, w) in r.hits.iter().zip(weights) {
        let expect = w * rrf(rank(&h.note_name), rank(&h.note_name));
        assert!(
            (h.score - expect).abs() < 1e-6,
            "{}: {} vs {expect}",
            h.note_name,
            h.score
        );
    }
}

#[test]
fn ring_filter_restricts_both_halves() {
    let e = HashEmbedder::new("test-v1", 256);
    let ix = seeded(&e);
    let r = ix
        .recall(
            "postgres bind mount",
            Some(&e),
            &RecallOptions {
                ring: Some(Ring::Session),
                ..Default::default()
            },
        )
        .unwrap();
    assert!(!r.hits.is_empty());
    assert!(r.hits.iter().all(|h| h.ring == Ring::Session), "{r:?}");
}

#[test]
fn n_and_k_are_honoured() {
    let e = HashEmbedder::new("test-v1", 256);
    let ix = seeded(&e);
    let r = ix
        .recall(
            "postgres bind mount container",
            Some(&e),
            &RecallOptions {
                n: 2,
                ..Default::default()
            },
        )
        .unwrap();
    assert_eq!(r.hits.len(), 2);
}

/// A note whose distinguishing word lives only in its title used to be unfindable by that
/// word. Calibrated against the state before the name column existed, where this returned
/// nothing at all.
#[test]
fn a_word_that_only_the_title_carries_still_finds_the_note() {
    let e = HashEmbedder::new("test-v1", 256);
    let mut ix = Index::open(&tempfile::tempdir().unwrap().path().join("i.db")).unwrap();
    ix.set_embedding_profile(&profile_of(&e)).unwrap();
    put(
        &mut ix,
        &e,
        &note(
            "inferenz-setup-2026",
            Ring::Knowledge,
            "Lokales Modell fuer den Widerspruchs-Check.\n\nOllama im Container, Loopback.\n\nQwen, 7b, kein GPU.",
            &[],
        ),
    );
    let hits = ix
        .recall("inferenz", None, &RecallOptions::default())
        .unwrap()
        .hits;
    assert_eq!(hits.len(), 1, "the title word must reach the note");
    assert_eq!(hits[0].note_name, "inferenz-setup-2026");
    // ...once. The name is on the opening block, not on all three, or one note would fill
    // the whole result. `setup` appears nowhere in the body either, so every hit it
    // returns can only have come through the name column.
    let all = ix
        .recall("setup", None, &RecallOptions::default())
        .unwrap()
        .hits;
    assert_eq!(
        all.len(),
        1,
        "a title match brings the note in once, not per block"
    );
}

/// The reason for prefix terms: `unicode61` does no stemming, so without them the reader
/// has to type the exact word form that somebody else wrote months ago. Calibrated against
/// the state before the change — with exact terms only, this query returned nothing.
#[test]
fn a_short_query_word_finds_the_longer_written_one() {
    let e = HashEmbedder::new("test-v1", 256);
    let mut ix = Index::open(&tempfile::tempdir().unwrap().path().join("i.db")).unwrap();
    ix.set_embedding_profile(&profile_of(&e)).unwrap();
    put(
        &mut ix,
        &e,
        &note(
            "pg18",
            Ring::Knowledge,
            "PostgreSQL 18 puts the data directory somewhere else entirely.",
            &[],
        ),
    );
    let hits = ix
        .recall("postgres", None, &RecallOptions::default())
        .unwrap()
        .hits;
    assert_eq!(hits.len(), 1, "`postgres` must reach `PostgreSQL`");
    // Two characters stay exact, or every query would drag half the store in.
    assert_eq!(crate::recall::fts_query("pg").unwrap(), "\"pg\"");
}

#[test]
fn fts_query_never_breaks_on_syntax() {
    let e = HashEmbedder::new("test-v1", 256);
    let ix = seeded(&e);
    for q in [
        "config.py",
        "r2-a91f2c33e1",
        "\"unbalanced",
        "a:b OR NOT (",
        "server-two*",
        "über ätzend",
        "   ",
        "",
    ] {
        let r = ix.recall(q, Some(&e), &RecallOptions::default());
        assert!(r.is_ok(), "query {q:?} failed: {:?}", r.err());
    }
    // Four characters and up become prefix terms, shorter ones stay exact.
    assert_eq!(
        crate::recall::fts_query("config.py").unwrap(),
        "\"config\"* OR \"py\""
    );
    assert_eq!(crate::recall::fts_query("  ,, "), None);
    let r = ix.recall("", Some(&e), &RecallOptions::default()).unwrap();
    assert!(
        r.caveats
            .iter()
            .any(|c| c.contains("lexical search skipped"))
    );
}

#[test]
fn upsert_replaces_old_blocks_completely() {
    let e = HashEmbedder::new("test-v1", 256);
    let mut ix = Index::open_in_memory().unwrap();
    ix.set_embedding_profile(&profile_of(&e)).unwrap();
    let n1 = note(
        "n",
        Ring::Knowledge,
        "first version zebra\n\nsecond paragraph",
        &[],
    );
    let old = put(&mut ix, &e, &n1);
    let n2 = note("n", Ring::Knowledge, "rewritten entirely", &[]);
    let out = ix
        .upsert_note(
            &n2,
            &blocks_of(&n2),
            Some(&embed_blocks(&e, &blocks_of(&n2))),
        )
        .unwrap();
    assert!(out.replaced);
    let st = ix.stats().unwrap();
    assert_eq!((st.notes, st.blocks, st.fts_rows, st.vectors), (1, 1, 1, 1));
    for b in &old {
        assert!(
            ix.resolve(&b.citation).unwrap().is_none(),
            "old citation survived"
        );
    }
    let r = ix
        .recall("zebra", Some(&e), &RecallOptions::default())
        .unwrap();
    assert!(
        r.hits
            .iter()
            .all(|h| old.iter().all(|b| b.citation.to_string() != h.citation)),
        "old block still retrievable: {r:?}"
    );
    assert!(!r.hits.iter().any(|h| h.text.contains("zebra")));
    assert!(ix.integrity().unwrap().is_empty());
}

#[test]
fn upsert_validates_its_input() {
    let e = HashEmbedder::new("test-v1", 256);
    let mut ix = Index::open_in_memory().unwrap();
    let n = note("n", Ring::Knowledge, "text", &[]);
    let blocks = blocks_of(&n);
    let vecs = embed_blocks(&e, &blocks);

    // Vectors before a profile is recorded.
    let err = ix.upsert_note(&n, &blocks, Some(&vecs)).unwrap_err();
    assert!(err.to_string().contains("set_embedding_profile"), "{err}");

    ix.set_embedding_profile(&profile_of(&e)).unwrap();
    // Wrong dimension.
    let err = ix
        .upsert_note(&n, &blocks, Some(&[vec![1.0; 3]]))
        .unwrap_err();
    assert!(err.to_string().contains("dim"), "{err}");
    // Wrong count.
    let err = ix.upsert_note(&n, &blocks, Some(&[])).unwrap_err();
    assert!(err.to_string().contains("0 vectors for 1 blocks"), "{err}");
    // Block from another note.
    let other = note("other", Ring::Knowledge, "text", &[]);
    let err = ix.upsert_note(&n, &blocks_of(&other), None).unwrap_err();
    assert!(err.to_string().contains("belongs to note"), "{err}");
    // Block citation ring disagrees with the note ring.
    let mut wrong_ring = blocks.clone();
    wrong_ring[0].citation = Citation::new(Ring::Session, n.front.id, 0, "text");
    let err = ix.upsert_note(&n, &wrong_ring, None).unwrap_err();
    assert!(err.to_string().contains("ring"), "{err}");
    // Duplicate name under a different id.
    ix.upsert_note(&n, &blocks, Some(&vecs)).unwrap();
    let mut clash = note("clash", Ring::Knowledge, "text", &[]);
    clash.front.name = "n".into();
    let err = ix
        .upsert_note(&clash, &blocks_of(&clash), None)
        .unwrap_err();
    assert!(err.to_string().contains("already used"), "{err}");
    assert!(ix.integrity().unwrap().is_empty());
}

#[test]
fn note_record_round_trips_frontmatter() {
    let e = HashEmbedder::new("test-v1", 256);
    let ix = seeded(&e);
    let rec = ix.note_by_name("never-copy-config-to-s2").unwrap().unwrap();
    assert_eq!(rec.front.ring, Ring::Invariant);
    assert_eq!(rec.front.kind, NoteKind::Knowledge);
    assert_eq!(rec.front.tags, vec!["t".to_string()]);
    assert_eq!(
        rec.front.links,
        vec![
            "pg18-moves-pgdata".to_string(),
            "does-not-exist-yet".to_string()
        ]
    );
    assert_eq!(rec.front.created.to_string(), "2026-09-05T09:12:03Z");
    assert_eq!(rec.block_count, 1);
    assert_eq!(rec.vector_count, 1);
    assert!(rec.stamp.is_none(), "path does not exist, so no stamp");
    let (hash, _) = ix.fingerprint(&rec.front.id).unwrap().unwrap();
    assert_eq!(hash, content_hash(&corpus()[2]));
    assert_eq!(ix.notes().unwrap().len(), 4);
    assert_eq!(ix.notes_in_ring(Ring::Knowledge).unwrap().len(), 2);
    assert_eq!(ix.blocks_of(&id_for("pg18-moves-pgdata")).unwrap().len(), 2);
}

// ----- links ------------------------------------------------------------------------------

#[test]
fn links_resolve_forward_backward_and_on_rename() {
    let e = HashEmbedder::new("test-v1", 256);
    let mut ix = Index::open_in_memory().unwrap();
    ix.set_embedding_profile(&profile_of(&e)).unwrap();
    // Link to a note that does not exist yet: dangling, valid.
    put(&mut ix, &e, &note("a", Ring::Knowledge, "a", &["b"]));
    assert_eq!(ix.dangling_links().unwrap().len(), 1);
    // Now it exists: resolved without touching `a`.
    put(&mut ix, &e, &note("b", Ring::Knowledge, "b", &[]));
    assert!(ix.dangling_links().unwrap().is_empty());
    assert_eq!(ix.links_to(&id_for("b")).unwrap()[0].from_note, id_for("a"));
    // Rename b (same id, new name): a's link to "b" dangles again.
    let mut b2 = note("b", Ring::Knowledge, "b", &[]);
    b2.front.name = "b-renamed".into();
    ix.upsert_note(&b2, &blocks_of(&b2), None).unwrap();
    assert_eq!(ix.dangling_links().unwrap().len(), 1);
    assert!(ix.links_to(&id_for("b")).unwrap().is_empty());
    // Delete b: nothing left pointing at it, a's row stays as intent.
    ix.delete_note(&id_for("b")).unwrap();
    assert_eq!(ix.links_from(&id_for("a")).unwrap().len(), 1);
    assert_eq!(ix.links().unwrap().len(), 1);
}

// ----- erasure (SPEC §12.2) ----------------------------------------------------------------

/// The assertions an erasure must satisfy. Shared with the teeth test below.
fn assert_no_trace(ix: &Index, id: &NoteId, blocks: &[Block]) {
    let id_s = id.to_string();
    let cits: Vec<String> = blocks.iter().map(|b| b.citation.to_string()).collect();
    assert_eq!(
        count(
            ix,
            &format!("SELECT count(*) FROM notes WHERE id = '{id_s}'")
        ),
        0,
        "notes row survived"
    );
    assert_eq!(
        count(
            ix,
            &format!("SELECT count(*) FROM blocks WHERE note_id = '{id_s}'")
        ),
        0,
        "blocks survived"
    );
    for c in &cits {
        assert_eq!(
            count(
                ix,
                &format!("SELECT count(*) FROM vectors WHERE citation = '{c}'")
            ),
            0,
            "vector for {c} survived erasure"
        );
        assert_eq!(
            count(
                ix,
                &format!("SELECT count(*) FROM blocks_fts WHERE citation = '{c}'")
            ),
            0,
            "FTS row for {c} survived erasure"
        );
    }
    assert_eq!(
        count(
            ix,
            &format!("SELECT count(*) FROM links WHERE from_note = '{id_s}'")
        ),
        0,
        "outbound links survived"
    );
    assert_eq!(
        count(
            ix,
            &format!("SELECT count(*) FROM links WHERE resolved_note_id = '{id_s}'")
        ),
        0,
        "inbound links still resolve to the erased note"
    );
}

#[test]
fn erasure_removes_every_trace() {
    let e = HashEmbedder::new("test-v1", 256);
    let ix_blocks;
    let mut ix = {
        let mut ix = Index::open_in_memory().unwrap();
        ix.set_embedding_profile(&profile_of(&e)).unwrap();
        for n in corpus() {
            put(&mut ix, &e, &n);
        }
        ix_blocks = ix.blocks_of(&id_for("pg18-moves-pgdata")).unwrap();
        ix
    };
    let id = id_for("pg18-moves-pgdata");
    assert_eq!(ix_blocks.len(), 2);

    // Before: retrievable by both halves.
    let r = ix
        .recall("PGDATA", Some(&e), &RecallOptions::default())
        .unwrap();
    assert_eq!(r.hits[0].note_id, id);

    let erasure = ix.delete_note(&id).unwrap();
    assert_eq!(erasure.id, id);
    assert_eq!(erasure.name, "pg18-moves-pgdata");
    assert_eq!(erasure.ring, Ring::Knowledge);
    assert!(erasure.path.ends_with("pg18-moves-pgdata.md"));
    let erased = erasure.counts;
    assert_eq!(erased.notes, 1);
    assert_eq!(erased.blocks, 2);
    assert_eq!(erased.fts_rows, 2);
    assert_eq!(erased.vectors, 2);
    assert_eq!(erased.links_out, 1);
    assert_eq!(
        erased.links_in_unresolved, 1,
        "never-copy-config-to-s2 linked here"
    );

    assert_no_trace(&ix, &id, &ix_blocks);

    // Through the public API: neither half of recall can see it any more.
    let r = ix
        .recall("PGDATA", Some(&e), &RecallOptions::default())
        .unwrap();
    assert!(
        r.hits.iter().all(|h| h.note_id != id),
        "lexical still finds it: {r:?}"
    );
    let r = ix
        .recall(
            "versioned subdirectory",
            Some(&e),
            &RecallOptions {
                k_lex: 0,
                ..Default::default()
            },
        )
        .unwrap();
    assert!(
        r.hits.iter().all(|h| h.note_id != id),
        "semantic still finds it: {r:?}"
    );
    for b in &ix_blocks {
        assert!(ix.resolve(&b.citation).unwrap().is_none());
    }
    assert!(ix.integrity().unwrap().is_empty(), "{:?}", ix.integrity());

    // Deleting again is an error, not a silent no-op.
    assert!(matches!(ix.delete_note(&id), Err(Error::NoSuchNote(_))));
}

/// SPEC §14.2: the erasure assertions are shown to fail on the defect they cover. Delete
/// only the `notes` row, the way a careless `forget` would, and confirm every other check
/// fires and `integrity()` reports the leftovers.
#[test]
fn erasure_assertions_have_teeth() {
    let e = HashEmbedder::new("test-v1", 256);
    let ix = seeded(&e);
    let id = id_for("pg18-moves-pgdata");
    let blocks = ix.blocks_of(&id).unwrap();

    ix.conn.pragma_update(None, "foreign_keys", "OFF").unwrap();
    ix.conn
        .execute("DELETE FROM notes WHERE id = ?1", [id.to_string()])
        .unwrap();

    let caught = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        assert_no_trace(&ix, &id, &blocks)
    }));
    assert!(caught.is_err(), "assertions passed on a half-deleted note");
    let msg = caught
        .unwrap_err()
        .downcast_ref::<String>()
        .cloned()
        .unwrap_or_default();
    assert!(msg.contains("blocks survived"), "{msg}");

    // The individual traces are each still there, which is what the full test denies.
    let c = blocks[0].citation.to_string();
    assert_eq!(
        count(
            &ix,
            &format!("SELECT count(*) FROM vectors WHERE citation = '{c}'")
        ),
        1
    );
    assert_eq!(
        count(
            &ix,
            &format!("SELECT count(*) FROM blocks_fts WHERE citation = '{c}'")
        ),
        1
    );

    let problems = ix.integrity().unwrap();
    assert!(
        problems.iter().any(|p| p.contains("blocks without a note")),
        "doctor would not notice: {problems:?}"
    );
    // The stale FTS row is still matched, and materialising the hit needs the notes row:
    // recall fails loudly rather than returning a citation with no note behind it.
    let r = ix.recall("PGDATA", None, &RecallOptions::default());
    assert!(
        r.is_err(),
        "a hit without a note must not be returned: {r:?}"
    );
    // The stale vector is still scanned, and the same loud failure follows on the
    // semantic-only path: a citation with nothing behind it is never returned.
    let r = ix.recall(
        "PGDATA",
        Some(&e),
        &RecallOptions {
            k_lex: 0,
            ..Default::default()
        },
    );
    assert!(r.is_err(), "stale vector produced a hit: {r:?}");
}

#[test]
fn clear_keeps_profile() {
    let e = HashEmbedder::new("test-v1", 256);
    let mut ix = seeded(&e);
    ix.delete_note(&id_for("session-2026-09-05")).unwrap();
    let e2 = ix.clear().unwrap();
    assert_eq!(e2.notes, 3);
    assert_eq!(e2.blocks, 5);
    let st = ix.stats().unwrap();
    assert_eq!(
        (st.notes, st.blocks, st.fts_rows, st.vectors, st.links),
        (0, 0, 0, 0, 0)
    );
    assert_eq!(st.embedding, Some(profile_of(&e)));
}

// ----- embedding profile guard (SPEC §5) ---------------------------------------------------

#[test]
fn profile_guard_disables_semantic_and_says_so() {
    let e1 = HashEmbedder::new("model-a", 256);
    let ix = seeded(&e1);

    // Same id, other dim.
    let e2 = HashEmbedder::new("model-a", 128);
    let err = ix.check_embedder(&e2).unwrap_err();
    assert!(
        matches!(err, Error::EmbeddingProfileMismatch { .. }),
        "{err}"
    );
    assert!(err.to_string().contains("scan --full"), "{err}");

    // Other id, same dim: this is the dangerous one, the vectors would compare fine.
    let e3 = HashEmbedder::new("model-b", 256);
    let err = ix.check_embedder(&e3).unwrap_err();
    assert!(matches!(err, Error::EmbeddingProfileMismatch { .. }));
    let r = ix
        .recall("bind mount inode", Some(&e3), &RecallOptions::default())
        .unwrap();
    assert!(
        r.caveats
            .iter()
            .any(|c| c.starts_with("semantic search disabled") && c.contains("model-b")),
        "{:?}",
        r.caveats
    );
    // Lexical still answers.
    assert_eq!(r.hits[0].note_name, "docker-bind-mount-inode-drift");
    // But nothing that only semantic could find.
    let r = ix
        .recall(
            "versioned subdirectory",
            Some(&e3),
            &RecallOptions {
                k_lex: 0,
                ..Default::default()
            },
        )
        .unwrap();
    assert!(r.hits.is_empty(), "compared vectors across models: {r:?}");

    // Full profile check also sees a differing model hash.
    let mut p = profile_of(&e1);
    p.model_hash = "other".into();
    assert!(matches!(
        ix.check_embedding_profile(&p),
        Err(Error::EmbeddingProfileMismatch { .. })
    ));
    assert!(ix.check_embedding_profile(&profile_of(&e1)).is_ok());
    assert!(ix.check_embedder(&e1).is_ok());
}

#[test]
fn profile_change_wipes_vectors_and_reports_it() {
    let e1 = HashEmbedder::new("model-a", 256);
    let mut ix = seeded(&e1);
    let same = ix.set_embedding_profile(&profile_of(&e1)).unwrap();
    assert!(!same.changed, "unchanged: no-op");
    assert_eq!(same.vectors_wiped, 0);
    let e2 = HashEmbedder::new("model-b", 256);
    let change = ix.set_embedding_profile(&profile_of(&e2)).unwrap();
    assert!(change.changed);
    assert_eq!(
        change.vectors_wiped, 7,
        "the caller needs this for the audit row"
    );
    assert_eq!(change.previous, Some(profile_of(&e1)));
    assert_eq!(change.current, profile_of(&e2));
    assert_eq!(ix.stats().unwrap().vectors, 0);
    // Blocks without vectors are reported, not hidden.
    let r = ix
        .recall("bind mount", Some(&e2), &RecallOptions::default())
        .unwrap();
    assert!(
        r.caveats
            .iter()
            .any(|c| c.contains("covered 0 of 7 blocks")),
        "{:?}",
        r.caveats
    );
    assert!(
        ix.integrity()
            .unwrap()
            .iter()
            .any(|p| p.contains("without a vector"))
    );
    // No profile at all, but an embedder offered: disabled and said.
    let ix2 = Index::open_in_memory().unwrap();
    let r = ix2
        .recall("x", Some(&e2), &RecallOptions::default())
        .unwrap();
    assert!(r.caveats.iter().any(|c| c.contains("no embedding profile")));
}

// ----- rebuildability (SPEC §3.1, §4) -----------------------------------------------------

#[test]
fn rebuild_is_lossless() {
    let e = HashEmbedder::new("test-v1", 256);
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("cyberbrain.db");
    let queries = [
        "bind mount inode",
        "PGDATA",
        "server two config",
        "versioned subdirectory",
        "nothing matches this at all",
        "",
    ];
    let opts = [
        RecallOptions::default(),
        RecallOptions {
            n: 3,
            ring: Some(Ring::Knowledge),
            ..Default::default()
        },
        RecallOptions {
            k_lex: 0,
            ..Default::default()
        },
    ];
    let snapshot = |ix: &Index| -> String {
        let mut out = String::new();
        for q in &queries {
            for o in &opts {
                let r = ix.recall(q, Some(&e), o).unwrap();
                out.push_str(&serde_json::to_string(&r).unwrap());
                out.push('\n');
            }
        }
        let notes = ix.notes().unwrap();
        out.push_str(&serde_json::to_string(&notes).unwrap());
        for n in &notes {
            out.push_str(&serde_json::to_string(&ix.links_from(&n.front.id).unwrap()).unwrap());
            for b in ix.blocks_of(&n.front.id).unwrap() {
                out.push_str(&format!("{}|{}|{}\n", b.citation, b.idx, b.text));
            }
        }
        out
    };

    let first = {
        let mut ix = Index::open(&path).unwrap();
        ix.set_embedding_profile(&profile_of(&e)).unwrap();
        for n in corpus() {
            put(&mut ix, &e, &n);
        }
        // A write and a delete, so the surviving state is not just "inserted in order".
        put(&mut ix, &e, &note("gone", Ring::Session, "temporary", &[]));
        ix.delete_note(&id_for("gone")).unwrap();
        snapshot(&ix)
    };
    assert!(first.contains("docker-bind-mount-inode-drift"));

    std::fs::remove_file(&path).unwrap();
    for suffix in ["-wal", "-shm"] {
        let _ = std::fs::remove_file(dir.path().join(format!("cyberbrain.db{suffix}")));
    }

    let second = {
        let mut ix = Index::open(&path).unwrap();
        ix.set_embedding_profile(&profile_of(&e)).unwrap();
        for n in corpus().into_iter().rev() {
            put(&mut ix, &e, &n);
        }
        snapshot(&ix)
    };
    assert_eq!(first, second, "rebuilt index answers differently");
}

// ----- audit (SPEC §12.6) ------------------------------------------------------------------

#[test]
fn audit_is_append_only_and_filterable() {
    let mut store = AuditStore::open_in_memory().unwrap();
    assert_eq!(store.schema_version().unwrap(), crate::AUDIT_SCHEMA_VERSION);
    assert_eq!(store.count().unwrap(), 0);
    assert!(store.last().unwrap().is_none());

    let first = store
        .append(&NewAuditEntry {
            ts: None,
            actor: "cyberbrain-policy".into(),
            action: "write".into(),
            subject: Some("01ARZ3NDEKTSV4RRFFQ69G5FAV".into()),
            detail: Some(r#"{"ring": 2, "name": "pg18-moves-pgdata"}"#.into()),
        })
        .unwrap();
    store
        .append(&NewAuditEntry {
            ts: None,
            actor: "cyberbrain-policy".into(),
            action: "policy.refusal".into(),
            ..Default::default()
        })
        .unwrap();
    let rows = store.read(&AuditFilter::default()).unwrap();
    assert_eq!(rows.len(), 2);
    assert_eq!(rows[0], first);
    assert_eq!(rows[0].seq, 1);
    assert_eq!(rows[0].actor, "cyberbrain-policy");
    assert_eq!(
        rows[0].ts.len(),
        "2026-09-05T09:12:03.123Z".len(),
        "{}",
        rows[0].ts
    );
    assert!(rows[0].ts.ends_with('Z'));
    assert_eq!(store.count().unwrap(), 2);
    assert_eq!(store.last().unwrap().unwrap().seq, 2);
    assert_eq!(store.get(1).unwrap().unwrap(), first);
    assert!(store.get(3).unwrap().is_none());
    assert_eq!(store.after(1).unwrap().len(), 1);

    let by_action = store
        .read(&AuditFilter {
            action: Some("policy.refusal".into()),
            ..Default::default()
        })
        .unwrap();
    assert_eq!(by_action.len(), 1);
    let by_text = store
        .read(&AuditFilter {
            contains: Some("PGDATA".into()),
            ..Default::default()
        })
        .unwrap();
    assert_eq!(by_text.len(), 1, "case-insensitive substring over detail");
    let since = store
        .read(&AuditFilter {
            since: Some("2999-01-01T00:00:00.000Z".into()),
            ..Default::default()
        })
        .unwrap();
    assert!(since.is_empty());
    let limited = store
        .read(&AuditFilter {
            limit: Some(1),
            ..Default::default()
        })
        .unwrap();
    assert_eq!(limited.len(), 1);

    for sql in ["UPDATE audit SET actor = 'x'", "DELETE FROM audit"] {
        let err = store.conn.execute(sql, []).unwrap_err();
        assert!(err.to_string().contains("append-only"), "{sql}: {err}");
    }
    assert_eq!(store.count().unwrap(), 2);

    // Rejected rows leave nothing behind.
    for bad in [
        NewAuditEntry {
            ts: None,
            actor: "".into(),
            action: "x".into(),
            ..Default::default()
        },
        NewAuditEntry {
            ts: None,
            actor: "a".into(),
            action: " ".into(),
            ..Default::default()
        },
        NewAuditEntry {
            ts: None,
            actor: "a".into(),
            action: "x".into(),
            detail: Some("{not json".into()),
            ..Default::default()
        },
    ] {
        assert!(store.append(&bad).is_err(), "{bad:?}");
    }
    assert_eq!(store.count().unwrap(), 2);
}

/// `detail` carries the hash chain. It must come back byte for byte: key order, spacing,
/// number formatting, everything.
#[test]
fn audit_detail_is_stored_verbatim() {
    let mut store = AuditStore::open_in_memory().unwrap();
    let ugly =
        "{\"z\":1.0,  \"_chain\":\"abc\",\n\"a\": [1,2 ,3],\"b\":{\"y\":null,\"x\":\"\\u00e9\"}}";
    let stored = store
        .append(&NewAuditEntry {
            ts: None,
            actor: "policy".into(),
            action: "write".into(),
            subject: None,
            detail: Some(ugly.into()),
        })
        .unwrap();
    assert_eq!(stored.detail.as_deref(), Some(ugly));
    assert_eq!(store.last().unwrap().unwrap().detail.as_deref(), Some(ugly));
    assert_eq!(
        store.read(&AuditFilter::default()).unwrap()[0]
            .detail
            .as_deref(),
        Some(ugly)
    );
    // Sanity: a normalising store would have failed the assertions above.
    let normalised =
        serde_json::to_string(&serde_json::from_str::<serde_json::Value>(ugly).unwrap()).unwrap();
    assert_ne!(normalised, ugly);
}

/// Two writers, one chain: `append_after` reads the predecessor and appends under one
/// write lock, so a second process cannot slip in between and fork the chain.
#[test]
fn audit_append_after_holds_the_write_lock() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("audit.db");
    let mut a = AuditStore::open(&path).unwrap();
    let mut b = AuditStore::open(&path).unwrap();
    b.set_busy_timeout(std::time::Duration::from_millis(50))
        .unwrap();

    a.append(&NewAuditEntry {
        ts: None,
        actor: "p".into(),
        action: "first".into(),
        ..Default::default()
    })
    .unwrap();

    let mut b_result = None;
    let stored = a
        .append_after(|last| {
            let last = last.expect("sees the first row");
            assert_eq!(last.action, "first");
            // While the lock is held, the other writer must wait and then fail, never
            // interleave.
            b_result = Some(b.append(&NewAuditEntry {
                ts: None,
                actor: "q".into(),
                action: "intruder".into(),
                ..Default::default()
            }));
            Ok(NewAuditEntry {
                ts: None,
                actor: "p".into(),
                action: "second".into(),
                subject: None,
                detail: Some(format!("{{\"_chain\":\"after-{}\"}}", last.seq)),
            })
        })
        .unwrap();
    assert_eq!(stored.seq, 2);
    assert_eq!(stored.detail.as_deref(), Some("{\"_chain\":\"after-1\"}"));
    let b_result = b_result.unwrap();
    assert!(b_result.is_err(), "the other writer got in: {b_result:?}");
    assert!(b_result.unwrap_err().to_string().contains("write lock"));

    // After the lock is released, the other writer continues from the real tail.
    let third = b
        .append_after(|last| {
            assert_eq!(last.unwrap().seq, 2);
            Ok(NewAuditEntry {
                ts: None,
                actor: "q".into(),
                action: "third".into(),
                ..Default::default()
            })
        })
        .unwrap();
    assert_eq!(third.seq, 3);
    let actions: Vec<String> = a
        .read(&AuditFilter::default())
        .unwrap()
        .into_iter()
        .map(|e| e.action)
        .collect();
    assert_eq!(actions, ["first", "second", "third"]);

    // A failing builder writes nothing and releases the lock.
    let err = a
        .append_after(|_| Err(Error::Index("no".into())))
        .unwrap_err();
    assert!(err.to_string().contains("no"));
    assert_eq!(a.count().unwrap(), 3);
    b.append(&NewAuditEntry {
        ts: None,
        actor: "q".into(),
        action: "fourth".into(),
        ..Default::default()
    })
    .unwrap();
}

#[test]
fn blocks_containing_finds_identifiers_for_subject_access() {
    let e = HashEmbedder::new("test-v1", 256);
    let ix = seeded(&e);
    let found = ix.blocks_containing("pgdata").unwrap();
    assert_eq!(found.len(), 1);
    assert_eq!(found[0].1, "pg18-moves-pgdata");
}

// ----- vectors ----------------------------------------------------------------------------

#[test]
fn vector_encoding_round_trips() {
    let v = vec![0.5f32, -1.25, 3.0e-9, f32::MAX];
    assert_eq!(vectors::decode(&vectors::encode(&v)).unwrap(), v);
    assert!(vectors::decode(&[1, 2, 3]).is_err());
    let mut z = vec![0.0f32; 4];
    assert!(!vectors::normalize(&mut z));
    let mut u = vec![3.0f32, 4.0];
    assert!(vectors::normalize(&mut u));
    assert!((vectors::dot(&u, &u) - 1.0).abs() < 1e-6);
    let a: Vec<f32> = (0..19).map(|i| i as f32).collect();
    let expect: f32 = a.iter().map(|x| x * x).sum();
    assert_eq!(vectors::dot(&a, &a), expect, "remainder lanes are included");
}

#[test]
fn cache_reloads_when_another_handle_writes() {
    let e = HashEmbedder::new("test-v1", 256);
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("cyberbrain.db");
    let mut writer = Index::open(&path).unwrap();
    writer.set_embedding_profile(&profile_of(&e)).unwrap();
    put(&mut writer, &e, &note("a", Ring::Knowledge, "alpha", &[]));
    let reader = Index::open(&path).unwrap();
    let r = reader
        .recall(
            "alpha",
            Some(&e),
            &RecallOptions {
                k_lex: 0,
                ..Default::default()
            },
        )
        .unwrap();
    assert_eq!(r.hits.len(), 1);
    put(
        &mut writer,
        &e,
        &note("b", Ring::Knowledge, "alpha beta", &[]),
    );
    let r = reader
        .recall(
            "alpha",
            Some(&e),
            &RecallOptions {
                k_lex: 0,
                ..Default::default()
            },
        )
        .unwrap();
    assert_eq!(
        r.hits.len(),
        2,
        "stale vector cache after an external write"
    );
}

// ----- performance (SPEC §15: recall over 25k blocks < 50 ms) -----------------------------

/// Build ~25k synthetic blocks and time hybrid recall. Ignored by default because the
/// debug build spends most of its time in the unoptimised vector scan and in inserting
/// 25k rows; run it with `cargo test -p cyberbrain-index --release -- --ignored`.
#[test]
#[ignore = "benchmark: run with --release -- --ignored"]
fn recall_over_25k_blocks_within_budget() {
    const NOTES: usize = 2_500;
    const BLOCKS_PER_NOTE: usize = 10;
    const DIM: usize = 256;
    const VOCAB: u64 = 3_000;

    let e = HashEmbedder::new("bench", DIM);
    let dir = tempfile::tempdir().unwrap();
    let mut ix = Index::open(&dir.path().join("cyberbrain.db")).unwrap();
    ix.set_embedding_profile(&profile_of(&e)).unwrap();

    let mut seed: u64 = 0x9E37_79B9_7F4A_7C15;
    let mut next = || {
        seed ^= seed << 13;
        seed ^= seed >> 7;
        seed ^= seed << 17;
        seed
    };
    let t0 = std::time::Instant::now();
    for i in 0..NOTES {
        let ring = Ring::ALL[i % 5];
        let mut paras = Vec::with_capacity(BLOCKS_PER_NOTE);
        for _ in 0..BLOCKS_PER_NOTE {
            let words: Vec<String> = (0..40).map(|_| format!("w{}", next() % VOCAB)).collect();
            paras.push(words.join(" "));
        }
        let n = note(&format!("note-{i:05}"), ring, &paras.join("\n\n"), &[]);
        put(&mut ix, &e, &n);
    }
    let st = ix.stats().unwrap();
    assert_eq!(st.blocks, NOTES * BLOCKS_PER_NOTE);
    eprintln!("indexed {} blocks in {:?}", st.blocks, t0.elapsed());

    let queries = ["w17 w42 w99", "w1000 w2000", "w5 w6 w7 w8 w9", "w2999"];
    let opts = RecallOptions::default();
    // Warm once: loads the vector cache, which is a one-off per process.
    let warm = std::time::Instant::now();
    ix.recall(queries[0], Some(&e), &opts).unwrap();
    eprintln!("first recall incl. cache load: {:?}", warm.elapsed());

    let mut times = Vec::new();
    for round in 0..25 {
        let q = queries[round % queries.len()];
        let t = std::time::Instant::now();
        let r = ix.recall(q, Some(&e), &opts).unwrap();
        times.push(t.elapsed());
        assert_eq!(r.hits.len(), 8);
        assert!(r.caveats.is_empty(), "{:?}", r.caveats);
    }
    times.sort();
    let p50 = times[times.len() / 2];
    let max = *times.last().unwrap();
    eprintln!("recall over {} blocks: p50 {p50:?}, max {max:?}", st.blocks);
    assert!(
        max < std::time::Duration::from_millis(50),
        "recall budget is 50 ms, worst observed {max:?}"
    );
}

// ----- the split (SPEC §4): the cache is disposable, the record is not -------------------

#[test]
fn audit_survives_cache_deletion() {
    let e = HashEmbedder::new("test-v1", 256);
    let dir = tempfile::tempdir().unwrap();
    let cache = dir.path().join("cyberbrain.db");
    let record = dir.path().join("audit.db");
    let mut audit = AuditStore::open(&record).unwrap();
    let row = |action: &str| NewAuditEntry {
        ts: None,
        actor: "cyberbrain-policy".into(),
        action: action.into(),
        subject: Some("x".into()),
        detail: Some(r#"{"_chain":"00"}"#.into()),
    };

    let before = {
        let mut ix = Index::open(&cache).unwrap();
        audit.append(&row("write")).unwrap();
        // Every index operation that used to write an audit row now writes none: the
        // record has exactly one writer, and it is not the cache.
        let n = audit.count().unwrap();
        ix.set_embedding_profile(&profile_of(&e)).unwrap();
        for n in corpus() {
            put(&mut ix, &e, &n);
        }
        let erasure = ix.delete_note(&id_for("session-2026-09-05")).unwrap();
        let change = ix
            .set_embedding_profile(&EmbeddingProfile {
                id: "other".into(),
                dim: 256,
                model_hash: "h".into(),
            })
            .unwrap();
        ix.clear().unwrap();
        assert_eq!(
            audit.count().unwrap(),
            n,
            "the index wrote to the audit record"
        );
        // The facts are still there for the caller to log.
        assert_eq!(erasure.counts.vectors, 2);
        assert_eq!(change.vectors_wiped, 5);
        audit.append(&row("forget")).unwrap();
        audit.read(&AuditFilter::default()).unwrap()
    };
    assert_eq!(before.len(), 2);

    // The cache is disposable.
    std::fs::remove_file(&cache).unwrap();
    for suffix in ["-wal", "-shm"] {
        let _ = std::fs::remove_file(dir.path().join(format!("cyberbrain.db{suffix}")));
    }
    let mut ix = Index::open(&cache).unwrap();
    ix.set_embedding_profile(&profile_of(&e)).unwrap();
    for n in corpus() {
        put(&mut ix, &e, &n);
    }
    assert_eq!(ix.stats().unwrap().notes, 4);
    assert!(ix.integrity().unwrap().is_empty());

    // The record is not.
    assert!(record.exists());
    let after = audit.read(&AuditFilter::default()).unwrap();
    assert_eq!(after, before, "deleting the cache touched the audit record");
    drop(audit);
    let reopened = AuditStore::open(&record).unwrap();
    assert_eq!(reopened.read(&AuditFilter::default()).unwrap(), before);
    assert_eq!(reopened.count().unwrap(), 2);
}
