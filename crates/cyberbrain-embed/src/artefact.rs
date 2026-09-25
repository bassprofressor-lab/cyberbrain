//! Local model artefacts and their verification (SPEC §6: "content-addressed by hash,
//! verified on every load; a hash mismatch is a hard failure").
//!
//! Nothing in this module can reach the network. It takes paths and returns bytes.

use cyberbrain_core::{Error, Result, Slash};
use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};

/// Length of a blake3 digest rendered as lowercase hex.
const HEX_LEN: usize = 64;

/// Where the two files of a model2vec-format model live.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ModelPaths {
    /// `model.safetensors`: one tensor named `embeddings`, shape `[vocab, dim]`.
    pub weights: PathBuf,
    /// `tokenizer.json` in HuggingFace tokenizers format.
    pub tokenizer: PathBuf,
}

impl ModelPaths {
    /// The conventional layout: `<dir>/model.safetensors` and `<dir>/tokenizer.json`.
    pub fn in_dir(dir: impl AsRef<Path>) -> Self {
        let dir = dir.as_ref();
        Self {
            weights: dir.join("model.safetensors"),
            tokenizer: dir.join("tokenizer.json"),
        }
    }
}

/// The expected blake3 digests of both files, lowercase hex. Produced once by whoever
/// obtained the artefact (the policy crate, after its registered download) and stored in
/// configuration; checked on every load.
///
/// Both files are covered because both change the meaning of a vector: a different
/// tokenizer maps the same text to different rows of the same matrix.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ArtefactManifest {
    pub weights_blake3: String,
    pub tokenizer_blake3: String,
}

impl ArtefactManifest {
    /// Rejects anything that is not exactly 64 lowercase hex characters per digest. A
    /// manifest that cannot match anything is a configuration error, not a load failure.
    pub fn validate(&self) -> Result<()> {
        for (what, hex) in [
            ("weights_blake3", &self.weights_blake3),
            ("tokenizer_blake3", &self.tokenizer_blake3),
        ] {
            if hex.len() != HEX_LEN
                || !hex
                    .bytes()
                    .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
            {
                return Err(Error::Config(format!(
                    "model manifest: {what} must be {HEX_LEN} lowercase hex characters, \
                     got {:?}",
                    hex
                )));
            }
        }
        Ok(())
    }
}

/// blake3 of a byte slice, lowercase hex.
pub fn hash_bytes(bytes: &[u8]) -> String {
    blake3::hash(bytes).to_hex().to_string()
}

/// blake3 of a file's contents, lowercase hex. This is what the policy crate calls after a
/// download to fill an [`ArtefactManifest`].
pub fn hash_file(path: impl AsRef<Path>) -> Result<String> {
    let bytes = read(path.as_ref())?;
    Ok(hash_bytes(&bytes))
}

pub(crate) fn read(path: &Path) -> Result<Vec<u8>> {
    std::fs::read(path).map_err(|source| Error::Io {
        path: path.to_path_buf(),
        source,
    })
}

/// Reads a file and verifies its digest. Returns the bytes only when they match, so the
/// bytes that were hashed are the bytes that get parsed: no window between check and use.
pub(crate) fn read_verified(path: &Path, expected_hex: &str, what: &str) -> Result<Vec<u8>> {
    let bytes = read(path)?;
    let actual = hash_bytes(&bytes);
    if actual != expected_hex {
        return Err(Error::Embed(format!(
            "{what} artefact {} does not match its manifest: expected blake3 {expected_hex}, \
             file has {actual}; refusing to load (SPEC §6: a hash mismatch is a hard failure)",
            Slash(path)
        )));
    }
    Ok(bytes)
}

/// Name of the verification record next to the model files.
pub const VERIFIED_FILE: &str = ".verified";

/// How old a file's mtime must be before its stamp is recorded (see `record_verified`).
const RACY_NS: u128 = 2_000_000_000;

/// What identifies a file on disk without reading it: length, modification time and inode.
#[derive(Debug, Clone, PartialEq, Eq)]
struct Stamp {
    len: u64,
    mtime_ns: u128,
    ino: u64,
}

fn stamp(path: &Path) -> Option<Stamp> {
    let meta = std::fs::metadata(path).ok()?;
    let mtime_ns = meta
        .modified()
        .ok()?
        .duration_since(std::time::UNIX_EPOCH)
        .ok()?
        .as_nanos();
    #[cfg(unix)]
    let ino = std::os::unix::fs::MetadataExt::ino(&meta);
    #[cfg(not(unix))]
    let ino = 0;
    Some(Stamp {
        len: meta.len(),
        mtime_ns,
        ino,
    })
}

/// Maps a file and makes sure it is the one the manifest names.
///
/// SPEC §6.5: "verification is staleness-checked, not repeated: a digest is recomputed only
/// when size or mtime changed". Until 2026-09-25 every load read and hashed the whole file
/// (512 MB, 0.39 s). Now a file whose length, mtime and inode match the `.verified` record,
/// AND whose recorded digest is the one the manifest expects, is taken as verified without
/// being read. Anything else is hashed in full, on every core. The second value says which
/// happened: `true` means the file passed a full load before and needs no content checks.
///
/// What this trades: a file rewritten in place with the same length and a restored mtime
/// would pass. That takes deliberate work by somebody who can also edit the manifest, which
/// is where the trust already sits. Replacing a model the ordinary way — new files, new
/// manifest — changes the inode or the digest and is caught. The accidental version of the
/// same thing, a rewrite within one mtime tick, is closed by `RACY_NS` in `record_verified`.
pub(crate) fn map_verified(
    path: &Path,
    expected_hex: &str,
    what: &str,
) -> Result<(memmap2::Mmap, bool)> {
    let file = std::fs::File::open(path).map_err(|source| Error::Io {
        path: path.to_path_buf(),
        source,
    })?;
    // SAFETY: the mapping is read-only. A model file truncated underneath a running process
    // would fault on access; model files are replaced, not rewritten, and the manifest
    // names the digest that a replacement must carry.
    let map = unsafe { memmap2::Mmap::map(&file) }.map_err(|source| Error::Io {
        path: path.to_path_buf(),
        source,
    })?;
    if recorded(path).is_some_and(|d| d == expected_hex) {
        return Ok((map, true));
    }
    let actual = blake3::Hasher::new()
        .update_rayon(&map)
        .finalize()
        .to_hex()
        .to_string();
    if actual != expected_hex {
        return Err(Error::Embed(format!(
            "{what} artefact {} does not match its manifest: expected blake3 {expected_hex}, \
             file has {actual}; refusing to load (SPEC §6: a hash mismatch is a hard failure)",
            Slash(path)
        )));
    }
    Ok((map, false))
}

/// The digest recorded for this file, if its stamp still matches.
fn recorded(path: &Path) -> Option<String> {
    let name = path.file_name()?.to_str()?;
    let now = stamp(path)?;
    let text = std::fs::read_to_string(path.with_file_name(VERIFIED_FILE)).ok()?;
    text.lines().find_map(|line| {
        let f: Vec<&str> = line.split('\t').collect();
        let [n, digest, len, mtime, ino] = f.as_slice() else {
            return None;
        };
        let then = Stamp {
            len: len.parse().ok()?,
            mtime_ns: mtime.parse().ok()?,
            ino: ino.parse().ok()?,
        };
        (*n == name && then == now && digest.len() == HEX_LEN).then(|| digest.to_string())
    })
}

/// Records that `path`, as it is now, passed a full load with this digest. Best effort: a
/// model directory the process may not write to only means the next load verifies again.
pub(crate) fn record_verified(path: &Path, digest: &str) {
    let (Some(name), Some(now)) = (path.file_name().and_then(|n| n.to_str()), stamp(path)) else {
        return;
    };
    // Racy files are not recorded (the problem git calls "racily clean"). The kernel keeps
    // mtime at a coarse tick of a few milliseconds, so a file rewritten in place right after
    // it was loaded can keep the very stamp the record holds — the embed tests did exactly
    // that on the first run. A file must be older than this before a stamp can stand for its
    // contents; until then every load hashes it in full.
    let since = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_nanos())
        .unwrap_or(0);
    if since.saturating_sub(now.mtime_ns) < RACY_NS {
        return;
    }
    let record = path.with_file_name(VERIFIED_FILE);
    let mut lines: Vec<String> = std::fs::read_to_string(&record)
        .unwrap_or_default()
        .lines()
        .filter(|l| l.split('\t').next() != Some(name))
        .map(str::to_string)
        .collect();
    lines.push(format!(
        "{name}\t{digest}\t{}\t{}\t{}",
        now.len, now.mtime_ns, now.ino
    ));
    let tmp = record.with_extension("tmp");
    if std::fs::write(&tmp, lines.join("\n") + "\n").is_ok() {
        let _ = std::fs::rename(&tmp, &record);
    }
}
