//! `cyberbrain bench-embedding`: which embedding model fits this machine.
//!
//! Retrieval quality is a property of the model and the notes, not of the machine, so it is
//! not measured here; it comes from a table of models measured on real notes, labelled with
//! where it came from. What differs from one machine to the next is the price: how long a
//! query waits for its vector and how long a full rebuild of the index takes. That is
//! measured here, on a sample of this store's own blocks, for every candidate that is
//! actually installed.
//!
//! Nothing is downloaded. A candidate that is not installed is listed with the command that
//! would install it, for the operator to run or not.

use super::{App, lock_index};
use cyberbrain_core::{Embedder, Error, Result};
use cyberbrain_embed::StaticEmbedder;
use cyberbrain_llm::{OllamaEmbedConfig, OllamaEmbedder};
use serde::Serialize;
use std::sync::Arc;
use std::time::{Duration, Instant};

/// A query that waits longer than this for its vector is felt by the person who asked.
pub const QUERY_BUDGET_MS: f64 = 200.0;
/// A full rebuild that takes longer than this is something an operator avoids running.
pub const REBUILD_BUDGET_MIN: f64 = 60.0;
/// Quality differences smaller than this are inside the noise of the measurement (one of 34
/// questions is 0.03), so candidates within it count as equally good and speed decides.
pub const QUALITY_NOISE: f32 = 0.04;

/// Where the quality figures come from, printed with them.
pub const QUALITY_SOURCE: &str = "top-3 hit rate, 34 questions over 1 300 German project notes, \
     dense retrieval only, measured by Krynex Labs on 2026-10-08; your notes may rank \
     differently, the order between models rarely does";

struct Known {
    model: &'static str,
    query_prefix: &'static str,
    document_prefix: &'static str,
    r_at_3: f32,
}

const GEMMA_Q: &str = "task: search result | query: ";
const GEMMA_D: &str = "title: none | text: ";

/// Ollama models with a measured quality, best value first.
const KNOWN: &[Known] = &[
    Known {
        model: "embeddinggemma:300m-qat-q4_0",
        query_prefix: GEMMA_Q,
        document_prefix: GEMMA_D,
        r_at_3: 0.94,
    },
    Known {
        model: "embeddinggemma:300m-qat-q8_0",
        query_prefix: GEMMA_Q,
        document_prefix: GEMMA_D,
        r_at_3: 0.94,
    },
    Known {
        model: "embeddinggemma:300m",
        query_prefix: GEMMA_Q,
        document_prefix: GEMMA_D,
        r_at_3: 0.94,
    },
    Known {
        model: "bge-m3",
        query_prefix: "",
        document_prefix: "",
        r_at_3: 0.97,
    },
];

/// The in-process static model, as measured in the same run.
const MODEL2VEC_R_AT_3: f32 = 0.65;

const QUERIES: &[&str] = &[
    "wie setze ich den Stop beim Trailing nach",
    "Ollama Container neu starten ohne Datenverlust",
    "why does the scan skip notes",
    "Datenschutz Kamera Betriebsrat Einwilligung",
    "Backup wiederherstellen nach Upgrade",
    "welche Engine ist aktiv",
    "rollback after a failed deploy",
    "Rechnung SEPA Lastschrift Fehler",
    "Telegram Bot meldet nichts mehr",
    "Index veraltet, recall findet neue Notiz nicht",
];

#[derive(Debug, Clone, Serialize)]
pub struct Hardware {
    pub logical_cpus: usize,
    pub cpu_model: Option<String>,
    pub avx2: Option<bool>,
    pub avx512: Option<bool>,
    pub memory_gib: Option<f64>,
}

#[derive(Debug, Clone, Serialize)]
pub struct BenchCandidate {
    pub model: String,
    pub backend: &'static str,
    pub installed: bool,
    /// From [`QUALITY_SOURCE`], not from this machine.
    pub quality_r_at_3: f32,
    pub dim: Option<usize>,
    pub weights_digest: Option<String>,
    pub license: Option<String>,
    pub cold_ms: Option<f64>,
    pub blocks_per_s: Option<f64>,
    pub rebuild_min: Option<f64>,
    pub query_p50_ms: Option<f64>,
    pub query_p95_ms: Option<f64>,
    pub within_budget: bool,
    pub error: Option<String>,
}

#[derive(Debug, Clone, Serialize)]
pub struct Recommendation {
    pub model: String,
    pub why: String,
    /// What to put in `cyberbrain.toml` to switch; empty when nothing would change.
    pub config: String,
    pub already_configured: bool,
}

#[derive(Debug, Clone, Serialize)]
pub struct EmbedBenchReport {
    pub hardware: Hardware,
    pub ollama: String,
    pub index_blocks: usize,
    pub sample_blocks: usize,
    pub quality_source: &'static str,
    pub candidates: Vec<BenchCandidate>,
    pub recommendation: Option<Recommendation>,
    /// Better candidates that are not installed, with the command that would install them.
    pub not_installed: Vec<String>,
    pub notes: Vec<String>,
}

fn hardware() -> Hardware {
    let logical_cpus = std::thread::available_parallelism()
        .map(|n| n.get())
        .unwrap_or(1);
    let cpuinfo = std::fs::read_to_string("/proc/cpuinfo").ok();
    let flags = cpuinfo.as_deref().and_then(|c| {
        c.lines()
            .find(|l| l.starts_with("flags"))
            .map(|l| l.to_string())
    });
    let cpu_model = cpuinfo.as_deref().and_then(|c| {
        c.lines()
            .find(|l| l.starts_with("model name"))
            .and_then(|l| l.split(':').nth(1))
            .map(|s| s.trim().to_string())
    });
    let memory_gib = std::fs::read_to_string("/proc/meminfo").ok().and_then(|m| {
        m.lines()
            .find(|l| l.starts_with("MemTotal:"))
            .and_then(|l| l.split_whitespace().nth(1))
            .and_then(|kb| kb.parse::<f64>().ok())
            .map(|kb| (kb / 1024.0 / 1024.0 * 10.0).round() / 10.0)
    });
    Hardware {
        logical_cpus,
        cpu_model,
        avx2: flags
            .as_ref()
            .map(|f| f.split_whitespace().any(|x| x == "avx2")),
        avx512: flags
            .as_ref()
            .map(|f| f.split_whitespace().any(|x| x == "avx512f")),
        memory_gib,
    }
}

fn percentile(sorted: &[f64], p: f64) -> f64 {
    if sorted.is_empty() {
        return 0.0;
    }
    let i = ((sorted.len() - 1) as f64 * p).round() as usize;
    sorted[i.min(sorted.len() - 1)]
}

fn ms(d: Duration) -> f64 {
    (d.as_secs_f64() * 10_000.0).round() / 10.0
}

/// Time `embedder` on the sample: one cold call, the sample in batches, then each query twice.
fn measure(
    embedder: &dyn Embedder,
    sample: &[String],
    index_blocks: usize,
    c: &mut BenchCandidate,
) -> Result<()> {
    let t = Instant::now();
    embedder.embed_query("warm up")?;
    c.cold_ms = Some(ms(t.elapsed()));

    let texts: Vec<&str> = sample.iter().map(String::as_str).collect();
    let t = Instant::now();
    for chunk in texts.chunks(32) {
        embedder.embed(chunk)?;
    }
    let secs = t.elapsed().as_secs_f64().max(1e-6);
    let rate = texts.len() as f64 / secs;
    c.blocks_per_s = Some((rate * 10.0).round() / 10.0);
    c.rebuild_min = Some(((index_blocks as f64 / rate) / 60.0 * 10.0).round() / 10.0);

    let mut lat = Vec::new();
    for _ in 0..2 {
        for q in QUERIES {
            let t = Instant::now();
            embedder.embed_query(q)?;
            lat.push(ms(t.elapsed()));
        }
    }
    lat.sort_by(|a, b| a.total_cmp(b));
    c.query_p50_ms = Some(percentile(&lat, 0.5));
    c.query_p95_ms = Some(percentile(&lat, 0.95));
    c.within_budget = percentile(&lat, 0.95) <= QUERY_BUDGET_MS
        && c.rebuild_min.unwrap_or(f64::MAX) <= REBUILD_BUDGET_MIN;
    Ok(())
}

impl App {
    /// Measure every installed candidate on `sample` blocks of this store and say which one
    /// fits this machine (SPEC §6). Asks the local Ollama through the registered inference
    /// path; downloads nothing.
    pub fn bench_embedding(&self, sample: usize) -> Result<EmbedBenchReport> {
        let (texts, index_blocks) = lock_index(&self.index)?.sample_block_texts(sample.max(8))?;
        if texts.is_empty() {
            return Err(Error::Config(
                "the index holds no blocks to measure with; run `cyberbrain scan` first".into(),
            ));
        }
        let mut report = EmbedBenchReport {
            hardware: hardware(),
            ollama: String::new(),
            index_blocks,
            sample_blocks: texts.len(),
            quality_source: QUALITY_SOURCE,
            candidates: Vec::new(),
            recommendation: None,
            not_installed: Vec::new(),
            notes: Vec::new(),
        };

        // The in-process model, if its artefact is here.
        let mut m2v = BenchCandidate {
            model: "model2vec (in process)".into(),
            backend: "model2vec",
            installed: false,
            quality_r_at_3: MODEL2VEC_R_AT_3,
            dim: None,
            weights_digest: None,
            license: None,
            cold_ms: None,
            blocks_per_s: None,
            rebuild_min: None,
            query_p50_ms: None,
            query_p95_ms: None,
            within_budget: false,
            error: None,
        };
        match self.model_artefact() {
            Ok((paths, manifest)) => {
                m2v.installed = true;
                let t = Instant::now();
                match StaticEmbedder::load(&paths, &manifest) {
                    Ok(e) => {
                        let load = ms(t.elapsed());
                        m2v.dim = Some(e.dim());
                        m2v.weights_digest = Some(manifest.weights_blake3.clone());
                        if let Err(err) = measure(&e, &texts, index_blocks, &mut m2v) {
                            m2v.error = Some(err.to_string());
                        }
                        // A CLI recall pays the load; the daemon pays it once.
                        m2v.cold_ms = Some(load);
                    }
                    Err(e) => m2v.error = Some(e.to_string()),
                }
            }
            Err(reason) => m2v.error = Some(reason),
        }
        report.candidates.push(m2v);

        // The Ollama models.
        let base_url = self
            .config
            .embedding
            .ollama
            .as_ref()
            .map(|o| o.base_url.clone())
            .unwrap_or_else(|| {
                self.config
                    .inference
                    .base_url
                    .trim_end_matches('/')
                    .trim_end_matches("/v1")
                    .to_string()
            });
        report.ollama = base_url.clone();
        let audit: Arc<dyn cyberbrain_llm::AuditSink> = Arc::new(self.policy.audit().clone());
        let probe_cfg = |model: &str, digest: &str, dim: usize, k: &Known| OllamaEmbedConfig {
            base_url: base_url.clone(),
            model: model.to_string(),
            digest: digest.to_string(),
            dim,
            query_prefix: k.query_prefix.to_string(),
            document_prefix: k.document_prefix.to_string(),
            num_ctx: 512,
            batch: 32,
            timeout: Duration::from_secs(300),
            keep_alive: "1m".into(),
            allow_public_endpoint: self.config.inference.allow_public_endpoint,
            allow_overlay_network: self.config.inference.allow_overlay_network,
            profile_id: format!("bench-{model}"),
        };
        let lister = OllamaEmbedder::new(
            probe_cfg("", "", 0, &KNOWN[0]),
            audit.clone(),
            self.gate.clone(),
        )?;
        let installed = match lister.installed() {
            Ok(list) => Some(list),
            Err(e) => {
                report.notes.push(format!(
                    "no Ollama answered at {base_url} ({e}); only the in-process model was measured"
                ));
                None
            }
        };
        if let Some(list) = &installed {
            for k in KNOWN {
                let mut c = BenchCandidate {
                    model: k.model.to_string(),
                    backend: "ollama",
                    installed: list
                        .iter()
                        .any(|m| cyberbrain_llm::embed::same_model(&m.name, k.model)),
                    quality_r_at_3: k.r_at_3,
                    dim: None,
                    weights_digest: None,
                    license: None,
                    cold_ms: None,
                    blocks_per_s: None,
                    rebuild_min: None,
                    query_p50_ms: None,
                    query_p95_ms: None,
                    within_budget: false,
                    error: None,
                };
                if !c.installed {
                    report.candidates.push(c);
                    continue;
                }
                let shown = match lister.show(k.model) {
                    Ok(s) => s,
                    Err(e) => {
                        c.error = Some(e.to_string());
                        report.candidates.push(c);
                        continue;
                    }
                };
                c.dim = shown.embedding_length;
                c.weights_digest = shown.weights_digest.clone();
                c.license = shown.license.clone();
                let (Some(digest), Some(dim)) = (shown.weights_digest, shown.embedding_length)
                else {
                    c.error = Some(
                        "Ollama did not state the weights digest or the embedding length".into(),
                    );
                    report.candidates.push(c);
                    continue;
                };
                match OllamaEmbedder::new(
                    probe_cfg(k.model, &digest, dim, k),
                    audit.clone(),
                    self.gate.clone(),
                ) {
                    Ok(e) => {
                        if let Err(err) = measure(&e, &texts, index_blocks, &mut c) {
                            c.error = Some(err.to_string());
                        }
                    }
                    Err(e) => c.error = Some(e.to_string()),
                }
                report.candidates.push(c);
            }
        }

        self.recommend(&mut report);
        Ok(report)
    }

    fn recommend(&self, report: &mut EmbedBenchReport) {
        let fits: Vec<&BenchCandidate> = report
            .candidates
            .iter()
            .filter(|c| c.within_budget && c.error.is_none())
            .collect();
        let best_quality = fits.iter().map(|c| c.quality_r_at_3).fold(0.0f32, f32::max);
        let pick = fits
            .iter()
            .filter(|c| c.quality_r_at_3 >= best_quality - QUALITY_NOISE)
            .max_by(|a, b| {
                a.blocks_per_s
                    .unwrap_or(0.0)
                    .total_cmp(&b.blocks_per_s.unwrap_or(0.0))
            })
            .copied();

        // What would be better than the pick, if it were installed.
        let ceiling = pick.map(|c| c.quality_r_at_3).unwrap_or(0.0);
        if ceiling + QUALITY_NOISE < KNOWN[0].r_at_3 {
            let missing: Vec<&BenchCandidate> = report
                .candidates
                .iter()
                .filter(|c| !c.installed && c.backend == "ollama")
                .collect();
            if let Some(m) = missing.first() {
                report.not_installed.push(format!(
                    "{} is not installed (top-3 {:.0} % against {:.0} %); `ollama pull {}`, then \
                     run this again to see whether it fits this machine",
                    m.model,
                    m.quality_r_at_3 * 100.0,
                    ceiling * 100.0,
                    m.model
                ));
            }
        }

        let Some(c) = pick else {
            report.notes.push(format!(
                "no candidate stayed within {QUERY_BUDGET_MS} ms per query (p95) and \
                 {REBUILD_BUDGET_MIN} minutes per full rebuild"
            ));
            return;
        };
        let current = self.embedding_profile().map(|p| p.id);
        if c.backend == "model2vec" {
            let already = current
                .as_deref()
                .is_some_and(|p| !p.starts_with("ollama-"));
            report.recommendation = Some(Recommendation {
                model: c.model.clone(),
                why: "the only candidate that fits this machine's budget".into(),
                config: if already {
                    String::new()
                } else {
                    "[embedding]\nbackend = \"model2vec\"".into()
                },
                already_configured: already,
            });
            return;
        }
        let k = KNOWN
            .iter()
            .find(|k| k.model == c.model)
            .expect("ollama candidates come from KNOWN");
        let ocfg = cyberbrain_core::config::OllamaEmbeddingConfig {
            base_url: report.ollama.clone(),
            model: c.model.clone(),
            digest: c.weights_digest.clone().unwrap_or_default(),
            dim: c.dim.unwrap_or_default(),
            query_prefix: k.query_prefix.into(),
            document_prefix: k.document_prefix.into(),
            ..Default::default()
        };
        let already = current.as_deref() == Some(ocfg.profile_id().as_str());
        let config = format!(
            "[embedding]\nbackend = \"ollama\"\n\n[embedding.ollama]\nbase_url = {:?}\nmodel = {:?}\n\
             digest = {:?}\ndim = {}\nquery_prefix = {:?}\ndocument_prefix = {:?}",
            ocfg.base_url,
            ocfg.model,
            ocfg.digest,
            ocfg.dim,
            ocfg.query_prefix,
            ocfg.document_prefix
        );
        report.recommendation = Some(Recommendation {
            model: c.model.clone(),
            why: format!(
                "the fastest of the candidates within {:.0} points of the best quality that fit \
                 the budget ({QUERY_BUDGET_MS} ms per query, {REBUILD_BUDGET_MIN} min per rebuild)",
                QUALITY_NOISE * 100.0
            ),
            config: if already { String::new() } else { config },
            already_configured: already,
        });
        if !already {
            report.notes.push(
                "switching re-embeds every block at the next `cyberbrain scan`; recall stays \
                 lexical for blocks not yet done"
                    .into(),
            );
        }
        if let Some(l) = &c.license {
            report.notes.push(format!(
                "{} declares licence {l:?}; read it before shipping the model to anyone else",
                c.model
            ));
        }
    }
}
