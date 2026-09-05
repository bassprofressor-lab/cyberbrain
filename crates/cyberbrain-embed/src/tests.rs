//! Tests run against a synthetic model in a temp dir. No model file, no network.

use crate::synthetic::{synthetic_weights, write_synthetic_model};
use crate::weights::{bf16_to_f32, f16_to_f32};
use crate::{ArtefactManifest, LoadOptions, ModelPaths, StaticEmbedder, hash_file, is_zero};
use cyberbrain_core::{Embedder, Error};
use std::path::Path;
use tempfile::TempDir;

const VOCAB: [&str; 8] = [
    "alpha", "beta", "gamma", "delta", "epsilon", "zeta", "eta", "theta",
];
const DIM: usize = 6;

fn model(seed: u64) -> (TempDir, StaticEmbedder, ModelPaths, ArtefactManifest) {
    let dir = TempDir::new().unwrap();
    let (paths, manifest) = write_synthetic_model(dir.path(), &VOCAB, DIM, seed).unwrap();
    let e = StaticEmbedder::load(&paths, &manifest).expect("synthetic model loads");
    (dir, e, paths, manifest)
}

fn norm(v: &[f32]) -> f32 {
    v.iter().map(|x| x * x).sum::<f32>().sqrt()
}

fn dot(a: &[f32], b: &[f32]) -> f32 {
    a.iter().zip(b).map(|(x, y)| x * y).sum()
}

fn corrupt_byte(path: &Path, offset_from_end: usize) {
    let mut bytes = std::fs::read(path).unwrap();
    let i = bytes.len() - offset_from_end;
    bytes[i] ^= 0x55;
    std::fs::write(path, bytes).unwrap();
}

#[test]
fn loads_and_reports_dimension() {
    let (_d, e, _, _) = model(1);
    assert_eq!(e.dim(), DIM);
    assert_eq!(e.vocab_rows(), VOCAB.len() + 1);
    assert_eq!(
        e.unk_id(),
        Some(0),
        "[UNK] sits at id 0 in the synthetic tokenizer"
    );
    let info = e.info();
    assert_eq!(info.weights_dtype, "f32");
    assert_eq!(info.pooling, "mean");
    assert_eq!(info.profile_id, e.profile_id());
}

#[test]
fn same_text_twice_gives_identical_vector() {
    let (_d, e, _, _) = model(1);
    let a = e.embed(&["alpha beta gamma"]).unwrap();
    let b = e.embed(&["alpha beta gamma"]).unwrap();
    assert_eq!(a, b, "embedding must be bit-for-bit deterministic");
    // And across the serial and parallel paths.
    let many: Vec<&str> = std::iter::repeat_n("alpha beta gamma", 200).collect();
    let par = e.embed_all_impl(&many, true).unwrap();
    let ser = e.embed_all_impl(&many, false).unwrap();
    assert_eq!(par, ser, "rayon path must match the serial path exactly");
    assert_eq!(par[0].vector, a[0]);
}

#[test]
fn vectors_are_unit_length() {
    let (_d, e, _, _) = model(1);
    for text in [
        "alpha",
        "Alpha BETA",
        "gamma delta epsilon zeta eta theta alpha beta",
    ] {
        let v = &e.embed(&[text]).unwrap()[0];
        assert_eq!(v.len(), DIM);
        assert!((norm(v) - 1.0).abs() < 1e-5, "{text:?}: norm {}", norm(v));
        assert!(v.iter().all(|x| x.is_finite()));
    }
}

#[test]
fn matches_hand_computed_mean_pool() {
    let (_d, e, _, _) = model(7);
    let w = synthetic_weights(VOCAB.len() + 1, DIM, 7);
    // "beta" is id 2, "delta" is id 4.
    let mut expect = vec![0.0f32; DIM];
    for id in [2usize, 4] {
        for k in 0..DIM {
            expect[k] += w[id * DIM + k];
        }
    }
    for x in expect.iter_mut() {
        *x /= 2.0;
    }
    let n = norm(&expect);
    for x in expect.iter_mut() {
        *x /= n;
    }
    let got = &e.embed(&["beta delta"]).unwrap()[0];
    for (g, x) in got.iter().zip(&expect) {
        assert!((g - x).abs() < 1e-6, "got {got:?}, expected {expect:?}");
    }
}

#[test]
fn order_of_tokens_does_not_matter_but_membership_does() {
    let (_d, e, _, _) = model(1);
    let ab = &e.embed(&["alpha beta"]).unwrap()[0];
    let ba = &e.embed(&["beta alpha"]).unwrap()[0];
    let ag = &e.embed(&["alpha gamma"]).unwrap()[0];
    assert!(
        (dot(ab, ba) - 1.0).abs() < 1e-5,
        "mean pooling is order-free"
    );
    assert!(
        dot(ab, ag) < 0.9999,
        "different tokens give a different vector"
    );
}

#[test]
fn empty_string_gives_zero_vector_not_nan() {
    let (_d, e, _, _) = model(1);
    let emb = e.embed_one("").unwrap();
    assert!(emb.is_empty());
    assert_eq!(emb.tokens_seen, 0);
    assert_eq!(emb.tokens_known, 0);
    assert_eq!(emb.vector.len(), DIM);
    assert!(is_zero(&emb.vector));
    assert!(emb.vector.iter().all(|x| x.is_finite()), "never NaN");

    let ws = e.embed_one("   \n\t ").unwrap();
    assert!(ws.is_empty());
    assert!(is_zero(&ws.vector));
}

#[test]
fn all_unknown_tokens_give_zero_vector_and_say_so() {
    let (_d, e, _, _) = model(1);
    let emb = e.embed_one("omega sigma xyzzy").unwrap();
    assert_eq!(emb.tokens_seen, 3, "the tokenizer saw three words");
    assert_eq!(emb.tokens_known, 0, "none had a row");
    assert!(is_zero(&emb.vector));
    assert!(emb.vector.iter().all(|x| x.is_finite()));
    // Through the trait, the same zero vector comes back and cosine against anything is 0.
    let v = e.embed(&["omega sigma"]).unwrap();
    let real = e.embed(&["alpha"]).unwrap();
    assert!(is_zero(&v[0]));
    assert_eq!(dot(&v[0], &real[0]), 0.0);
}

#[test]
fn unknown_tokens_are_skipped_not_pooled() {
    let (_d, e, _, _) = model(1);
    let clean = &e.embed(&["alpha beta"]).unwrap()[0];
    let noisy = e.embed_one("alpha xyzzy beta plugh").unwrap();
    assert_eq!(noisy.tokens_seen, 4);
    assert_eq!(noisy.tokens_known, 2);
    assert_eq!(
        &noisy.vector, clean,
        "the unk row must not leak into the mean"
    );
}

#[test]
fn max_tokens_truncates_the_id_list() {
    let dir = TempDir::new().unwrap();
    let (paths, manifest) = write_synthetic_model(dir.path(), &VOCAB, DIM, 3).unwrap();
    let e = StaticEmbedder::load_with(
        &paths,
        &manifest,
        LoadOptions {
            max_tokens: Some(2),
            unk_token: None,
        },
    )
    .unwrap();
    let cut = e.embed_one("alpha beta gamma delta").unwrap();
    let two = e.embed_one("alpha beta").unwrap();
    assert_eq!(cut.tokens_seen, 2);
    assert_eq!(cut.vector, two.vector);
}

#[test]
fn batch_preserves_input_order_and_length() {
    let (_d, e, _, _) = model(1);
    let texts: Vec<String> = (0..300)
        .map(|i| {
            format!(
                "{} {}",
                VOCAB[i % VOCAB.len()],
                VOCAB[(i * 3) % VOCAB.len()]
            )
        })
        .collect();
    let refs: Vec<&str> = texts.iter().map(String::as_str).collect();
    let batch = e.embed(&refs).unwrap();
    assert_eq!(batch.len(), refs.len());
    for (i, t) in refs.iter().enumerate() {
        assert_eq!(batch[i], e.embed(&[t]).unwrap()[0], "row {i} out of order");
    }
}

#[test]
fn profile_id_changes_when_weights_change() {
    let (_d1, a, _, _) = model(1);
    let (_d2, b, _, _) = model(2);
    assert_ne!(a.profile_id(), b.profile_id());
    assert!(
        a.profile_id().starts_with("m2v-mean-d6-"),
        "{}",
        a.profile_id()
    );
}

#[test]
fn profile_id_changes_when_tokenizer_changes() {
    let d1 = TempDir::new().unwrap();
    let d2 = TempDir::new().unwrap();
    // Same seed, same shape, so identical weights; only the vocabulary differs.
    let (p1, m1) = write_synthetic_model(d1.path(), &VOCAB, DIM, 5).unwrap();
    let mut other = VOCAB;
    other[0] = "omega";
    let (p2, m2) = write_synthetic_model(d2.path(), &other, DIM, 5).unwrap();
    assert_eq!(
        m1.weights_blake3, m2.weights_blake3,
        "precondition: same weights"
    );
    let a = StaticEmbedder::load(&p1, &m1).unwrap();
    let b = StaticEmbedder::load(&p2, &m2).unwrap();
    assert_ne!(
        a.profile_id(),
        b.profile_id(),
        "a different tokenizer is a different profile"
    );
}

#[test]
fn profile_id_is_stable_across_loads() {
    let (_d, a, paths, manifest) = model(9);
    let b = StaticEmbedder::load(&paths, &manifest).unwrap();
    assert_eq!(a.profile_id(), b.profile_id());
}

#[test]
fn corrupted_weights_fail_to_load() {
    let (_d, _e, paths, manifest) = model(1);
    corrupt_byte(&paths.weights, 3); // inside the tensor data, after the header
    let err = StaticEmbedder::load(&paths, &manifest).unwrap_err();
    assert!(matches!(err, Error::Embed(_)), "{err}");
    let msg = err.to_string();
    assert!(msg.contains("does not match its manifest"), "{msg}");
    assert!(
        msg.contains(&manifest.weights_blake3),
        "names the expected digest: {msg}"
    );
}

#[test]
fn corrupted_tokenizer_fails_to_load() {
    let (_d, _e, paths, manifest) = model(1);
    corrupt_byte(&paths.tokenizer, 2);
    let err = StaticEmbedder::load(&paths, &manifest).unwrap_err();
    assert!(err.to_string().contains("tokenizer artefact"), "{err}");
}

#[test]
fn wrong_manifest_fails_even_when_files_are_fine() {
    let (_d, _e, paths, manifest) = model(1);
    let mut wrong = manifest.clone();
    wrong.weights_blake3 = manifest.tokenizer_blake3.clone();
    assert!(StaticEmbedder::load(&paths, &wrong).is_err());
}

#[test]
fn malformed_manifest_is_a_config_error() {
    let (_d, _e, paths, _) = model(1);
    let bad = ArtefactManifest {
        weights_blake3: "DEADBEEF".into(),
        tokenizer_blake3: "x".repeat(64),
    };
    let err = StaticEmbedder::load(&paths, &bad).unwrap_err();
    assert!(matches!(err, Error::Config(_)), "{err}");
    assert_eq!(err.exit_code(), 1);
}

#[test]
fn hash_matches_but_content_is_garbage_fails_on_parse() {
    // A manifest that honestly describes a broken file must still refuse: the hash proves
    // identity, not sanity.
    let (_d, _e, paths, mut manifest) = model(1);
    std::fs::write(&paths.weights, b"not a safetensors file").unwrap();
    manifest.weights_blake3 = hash_file(&paths.weights).unwrap();
    let err = StaticEmbedder::load(&paths, &manifest).unwrap_err();
    assert!(err.to_string().contains("not a valid safetensors"), "{err}");
}

#[test]
fn non_finite_weight_is_refused() {
    let dir = TempDir::new().unwrap();
    let (paths, mut manifest) = write_synthetic_model(dir.path(), &VOCAB, DIM, 1).unwrap();
    // Rebuild the tensor with one NaN planted in row 3.
    let mut w = synthetic_weights(VOCAB.len() + 1, DIM, 1);
    w[3 * DIM + 1] = f32::NAN;
    let raw: Vec<u8> = w.iter().flat_map(|f| f.to_le_bytes()).collect();
    let view = safetensors::tensor::TensorView::new(
        safetensors::Dtype::F32,
        vec![VOCAB.len() + 1, DIM],
        &raw,
    )
    .unwrap();
    let bytes = safetensors::serialize([("embeddings", view)], None).unwrap();
    std::fs::write(&paths.weights, &bytes).unwrap();
    manifest.weights_blake3 = hash_file(&paths.weights).unwrap();
    let err = StaticEmbedder::load(&paths, &manifest).unwrap_err();
    assert!(err.to_string().contains("non-finite"), "{err}");
}

#[test]
fn tokenizer_with_more_ids_than_rows_is_refused() {
    let dir = TempDir::new().unwrap();
    let (paths, mut manifest) = write_synthetic_model(dir.path(), &VOCAB, DIM, 1).unwrap();
    // Shrink the matrix to fewer rows than the tokenizer has ids.
    let w = synthetic_weights(4, DIM, 1);
    let raw: Vec<u8> = w.iter().flat_map(|f| f.to_le_bytes()).collect();
    let view =
        safetensors::tensor::TensorView::new(safetensors::Dtype::F32, vec![4, DIM], &raw).unwrap();
    let bytes = safetensors::serialize([("embeddings", view)], None).unwrap();
    std::fs::write(&paths.weights, &bytes).unwrap();
    manifest.weights_blake3 = hash_file(&paths.weights).unwrap();
    let err = StaticEmbedder::load(&paths, &manifest).unwrap_err();
    assert!(
        err.to_string().contains("do not belong to the same model"),
        "{err}"
    );
}

#[test]
fn f16_and_bf16_weights_load_and_match_f32() {
    let dir = TempDir::new().unwrap();
    let (paths, manifest) = write_synthetic_model(dir.path(), &VOCAB, DIM, 4).unwrap();
    let f32_model = StaticEmbedder::load(&paths, &manifest).unwrap();
    let reference = &f32_model.embed(&["alpha gamma theta"]).unwrap()[0];

    let w = synthetic_weights(VOCAB.len() + 1, DIM, 4);
    // bf16 is the top 16 bits of f32; write it that way and expect the same ranking-scale
    // agreement (bf16 keeps ~3 significant digits).
    let raw: Vec<u8> = w
        .iter()
        .flat_map(|f| ((f.to_bits() >> 16) as u16).to_le_bytes())
        .collect();
    let view = safetensors::tensor::TensorView::new(
        safetensors::Dtype::BF16,
        vec![VOCAB.len() + 1, DIM],
        &raw,
    )
    .unwrap();
    let bytes = safetensors::serialize([("embeddings", view)], None).unwrap();
    std::fs::write(&paths.weights, &bytes).unwrap();
    let m2 = ArtefactManifest {
        weights_blake3: hash_file(&paths.weights).unwrap(),
        ..manifest.clone()
    };
    let bf = StaticEmbedder::load(&paths, &m2).unwrap();
    assert_eq!(bf.info().weights_dtype, "bf16");
    let v = &bf.embed(&["alpha gamma theta"]).unwrap()[0];
    assert!(
        dot(v, reference) > 0.999,
        "bf16 cosine to f32: {}",
        dot(v, reference)
    );
    assert_ne!(
        bf.profile_id(),
        f32_model.profile_id(),
        "different bytes, different profile"
    );
}

#[test]
fn f16_conversion_is_bit_exact() {
    for (bits, want) in [
        (0x3C00u16, 1.0f32),
        (0xC000, -2.0),
        (0x3800, 0.5),
        (0x7BFF, 65504.0),
        (0x0001, 5.960_464_5e-8), // smallest subnormal, 2^-24
        (0x03FF, 6.097_555e-5),   // largest subnormal
        (0x0400, 6.103_515_6e-5), // smallest normal
        (0x0000, 0.0),
        (0x8000, -0.0),
    ] {
        let got = f16_to_f32(bits);
        assert_eq!(
            got.to_bits(),
            want.to_bits(),
            "0x{bits:04x}: got {got}, want {want}"
        );
    }
    assert!(f16_to_f32(0x7C00).is_infinite());
    assert!(f16_to_f32(0xFC00).is_infinite());
    assert!(f16_to_f32(0x7E00).is_nan());
    assert_eq!(bf16_to_f32(0x3F80), 1.0);
    assert_eq!(bf16_to_f32(0xC000), -2.0);
}

#[test]
fn missing_files_are_io_errors_with_the_path() {
    let dir = TempDir::new().unwrap();
    let paths = ModelPaths::in_dir(dir.path());
    let manifest = ArtefactManifest {
        weights_blake3: "0".repeat(64),
        tokenizer_blake3: "0".repeat(64),
    };
    let err = StaticEmbedder::load(&paths, &manifest).unwrap_err();
    assert!(matches!(err, Error::Io { .. }), "{err}");
    assert!(err.to_string().contains("tokenizer.json"), "{err}");
}

#[test]
fn embedder_is_send_and_sync() {
    fn assert_send_sync<T: Send + Sync>() {}
    assert_send_sync::<StaticEmbedder>();
}

/// Not a correctness test: prints the cost of embedding a few thousand strings serially
/// and through rayon. Run with `cargo test -p cyberbrain-embed --release -- --ignored
/// --nocapture batch_timing`.
#[test]
#[ignore]
fn batch_timing() {
    use std::time::Instant;
    let vocab: Vec<String> = (0..5000).map(|i| format!("w{i}")).collect();
    let vocab_refs: Vec<&str> = vocab.iter().map(String::as_str).collect();
    let dir = TempDir::new().unwrap();
    let t0 = Instant::now();
    let (paths, manifest) = write_synthetic_model(dir.path(), &vocab_refs, 256, 1).unwrap();
    let e = StaticEmbedder::load(&paths, &manifest).unwrap();
    println!(
        "load (5000x256 f32, {} KB): {:?}",
        5000 * 256 * 4 / 1024,
        t0.elapsed()
    );

    let texts: Vec<String> = (0..3000)
        .map(|i| {
            (0..60)
                .map(|j| vocab[(i * 31 + j * 7) % vocab.len()].as_str())
                .collect::<Vec<_>>()
                .join(" ")
        })
        .collect();
    let refs: Vec<&str> = texts.iter().map(String::as_str).collect();
    for _ in 0..2 {
        let t = Instant::now();
        let a = e.embed_all_impl(&refs, false).unwrap();
        let serial = t.elapsed();
        let t = Instant::now();
        let b = e.embed_all_impl(&refs, true).unwrap();
        let parallel = t.elapsed();
        assert_eq!(a, b);
        println!(
            "3000 texts x 60 tokens: serial {serial:?}, rayon {parallel:?} ({} threads)",
            rayon::current_num_threads()
        );
    }
    let t = Instant::now();
    for _ in 0..1000 {
        e.embed_one("w1 w2 w3 w4 w5 w6 w7 w8").unwrap();
    }
    println!("single 8-token query: {:?} each", t.elapsed() / 1000);
}
