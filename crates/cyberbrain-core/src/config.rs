//! `<store>/cyberbrain.toml` (SPEC §4, §7, §11, §12).
//!
//! A missing file yields [`Config::default`]. A malformed file is `Error::Config` and the
//! message names the file and the offending key. Unknown keys are errors too: a typo in
//! `allow_public_endpoint` that silently reverts to the default is exactly the kind of
//! quiet failure this project exists to avoid — even if, in that particular case, the
//! default is the safe one.
//!
//! [`DEFAULT_TOML`] is the documented file `init` writes; it parses to `Config::default()`
//! and a test pins that equivalence, so the comments in it cannot drift from the code.

use crate::error::{Error, Result};
use crate::types::Ring;
use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};

/// File name inside the store.
pub const CONFIG_FILE: &str = "cyberbrain.toml";

/// Default store directory, relative to the project root (SPEC §4).
pub const DEFAULT_STORE_DIR: &str = ".cyberbrain";

/// The inference base URL every OpenAI-compatible local server answers on by default
/// (SPEC §11).
pub const DEFAULT_INFERENCE_URL: &str = "http://127.0.0.1:11434/v1";

/// Combined size cap for rings 0 and 1, in approximate tokens (SPEC §3.2).
pub const DEFAULT_RESIDENT_CAP_TOKENS: usize = 8192;

/// Compliance profile (SPEC §12). `Off` compiles the checks in and disables them.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum PolicyProfile {
    Eu,
    Ch,
    Off,
}

impl PolicyProfile {
    pub fn as_str(self) -> &'static str {
        match self {
            PolicyProfile::Eu => "eu",
            PolicyProfile::Ch => "ch",
            PolicyProfile::Off => "off",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct Config {
    /// Where this configuration lives: the store root. Not a key in the file — a file
    /// inside the store naming a different store would be nonsense — but filled in by
    /// [`Config::load`] so every consumer has one place to ask.
    #[serde(skip)]
    pub store: PathBuf,
    pub rings: RingsConfig,
    pub retrieval: RetrievalConfig,
    pub embedding: EmbeddingConfig,
    pub inference: InferenceConfig,
    pub policy: PolicyConfig,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct RingsConfig {
    /// Rings 0 and 1 together may not exceed this many (approximate) tokens. Enforced at
    /// write time (SPEC §3.2).
    pub resident_cap_tokens: usize,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct RetrievalConfig {
    /// Lexical candidates from FTS5 (SPEC §7 step 1).
    pub k_lex: usize,
    /// Semantic candidates by cosine (SPEC §7 step 2).
    pub k_sem: usize,
    /// Results returned (SPEC §7 step 5).
    pub n: usize,
    /// The constant in reciprocal rank fusion, `1 / (rrf_k + rank)` (SPEC §7 step 3).
    pub rrf_k: u32,
    /// Multiplier per ring 0..4 after fusion (SPEC §7 step 4).
    pub ring_weights: [f32; 5],
}

impl RetrievalConfig {
    pub fn weight(&self, ring: Ring) -> f32 {
        self.ring_weights[ring.as_u8() as usize]
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct EmbeddingConfig {
    /// Directory holding the model artefact. Relative paths are relative to the store.
    pub model_path: PathBuf,
    /// Embedding profile the vectors must match (SPEC §5). Interpreted by
    /// `cyberbrain-embed`; the index refuses to compare vectors from a different one.
    pub profile: String,
    /// Where the model artefact may be fetched from, if it is missing. The egress gate
    /// permits `ModelDownload` only towards this exact host, so an empty value means the
    /// artefact must be placed by hand and nothing may be downloaded at all.
    pub model_source: Option<String>,
    /// Whether the operator has agreed to that one download (SPEC §12.1: "once, on
    /// explicit consent"). Persisted rather than held in memory, because consent that is
    /// forgotten on restart gets asked for again until somebody clicks it away.
    pub model_download_consent: bool,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct InferenceConfig {
    /// OpenAI-compatible base URL (SPEC §11).
    pub base_url: String,
    /// Off by default: the base URL must be loopback or a private range unless the
    /// operator says otherwise, explicitly, here.
    pub allow_public_endpoint: bool,
    /// Permits `100.64.0.0/10`. That range is carrier-grade NAT, where an address is
    /// usually somebody else's machine, but it is also what overlay networks such as
    /// Tailscale hand out to reach your own GPU box. The two are indistinguishable from
    /// the address alone, so this is its own switch rather than being folded into
    /// `allow_public_endpoint`: a setting whose name misdescribes what the operator is
    /// agreeing to is a consent failure, not a convenience.
    #[serde(default)]
    pub allow_overlay_network: bool,
    /// Model name sent to the endpoint. `None` lets the server pick its default.
    pub model: Option<String>,
    /// Per-request timeout. The LLM layer is optional; it must never hang a core path.
    pub timeout_ms: u64,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct PolicyConfig {
    pub profile: PolicyProfile,
}

impl Default for Config {
    fn default() -> Self {
        Self {
            store: PathBuf::from(DEFAULT_STORE_DIR),
            rings: RingsConfig::default(),
            retrieval: RetrievalConfig::default(),
            embedding: EmbeddingConfig::default(),
            inference: InferenceConfig::default(),
            policy: PolicyConfig::default(),
        }
    }
}

impl Default for RingsConfig {
    fn default() -> Self {
        Self {
            resident_cap_tokens: DEFAULT_RESIDENT_CAP_TOKENS,
        }
    }
}

impl Default for RetrievalConfig {
    fn default() -> Self {
        Self {
            k_lex: 50,
            k_sem: 50,
            n: 8,
            rrf_k: 60,
            ring_weights: [
                Ring::Invariant.weight(),
                Ring::Protocol.weight(),
                Ring::Knowledge.weight(),
                Ring::Session.weight(),
                Ring::External.weight(),
            ],
        }
    }
}

impl Default for EmbeddingConfig {
    fn default() -> Self {
        Self {
            model_path: PathBuf::from("models/model2vec"),
            profile: "static-model2vec".to_string(),
            model_source: None,
            model_download_consent: false,
        }
    }
}

impl Default for InferenceConfig {
    fn default() -> Self {
        Self {
            base_url: DEFAULT_INFERENCE_URL.to_string(),
            allow_public_endpoint: false,
            allow_overlay_network: false,
            model: None,
            timeout_ms: 30_000,
        }
    }
}

impl Default for PolicyConfig {
    fn default() -> Self {
        Self {
            profile: PolicyProfile::Eu,
        }
    }
}

/// The documented default configuration, as written by `cyberbrain init`. Parses to
/// `Config::default()` — pinned by a test.
pub const DEFAULT_TOML: &str = r#"# Cyberbrain configuration. Every key below is optional; a missing key takes the
# default shown here. Unknown keys are an error, so a typo cannot silently revert a
# setting to its default.

[rings]
# Rings 0 and 1 are injected into every session, so together they carry a hard cap,
# in approximate tokens. Exceeding it fails the write; nothing is truncated at read time.
resident_cap_tokens = 8192

[retrieval]
# Hybrid search: lexical (BM25) and semantic candidates, fused by reciprocal rank
# fusion, weighted by ring, top n returned.
k_lex = 50
k_sem = 50
n = 8
rrf_k = 60
# Multiplier per ring 0..4 after fusion. Lower rings are more trusted.
ring_weights = [1.15, 1.10, 1.00, 0.92, 0.80]

[embedding]
# Directory of the model artefact, relative to the store unless absolute.
model_path = "models/model2vec"
# Profile id the stored vectors must match; a mismatch disables semantic search loudly.
profile = "static-model2vec"
# Where the artefact may be fetched from if it is missing. The egress gate permits a model
# download towards this exact host and nowhere else. Left unset, nothing may be downloaded
# and the artefact has to be placed by hand.
# model_source = "https://example.org/potion-base-8M"
# Your agreement to that one download. It is stored rather than asked each run, because
# consent that is forgotten on restart just gets clicked away.
model_download_consent = false

[inference]
# OpenAI-compatible endpoint for the optional local LLM layer (Ollama, LM Studio,
# NVIDIA PAIR, vLLM, llama.cpp ...). Must be loopback or a private range unless
# allow_public_endpoint is set. Every call is recorded in the audit log.
base_url = "http://127.0.0.1:11434/v1"
allow_public_endpoint = false

# Permits 100.64.0.0/10. That is carrier-grade NAT, where an address usually belongs to
# somebody else, but it is also the range Tailscale and similar overlays hand out for your
# own machines. Turn this on only if you know the endpoint is yours.
allow_overlay_network = false
# model = "qwen3:8b"
timeout_ms = 30000

[policy]
# eu | ch | off. "off" compiles the checks in and disables them.
profile = "eu"
"#;

impl Config {
    /// Path of the config file inside a store.
    pub fn path_in(store: &Path) -> PathBuf {
        store.join(CONFIG_FILE)
    }

    /// Load `<store>/cyberbrain.toml`. Missing file: defaults. Malformed file:
    /// `Error::Config` naming the file and the key.
    pub fn load(store: &Path) -> Result<Config> {
        let path = Self::path_in(store);
        let text = match std::fs::read_to_string(&path) {
            Ok(t) => t,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
                return Ok(Config {
                    store: store.to_path_buf(),
                    ..Config::default()
                });
            }
            Err(e) => {
                return Err(Error::Io { path, source: e });
            }
        };
        let mut cfg = Self::parse(&text, &path)?;
        cfg.store = store.to_path_buf();
        Ok(cfg)
    }

    /// Parse config text. `path` only names the file in errors.
    pub fn parse(text: &str, path: &Path) -> Result<Config> {
        let cfg: Config = toml::from_str(text).map_err(|e| {
            Error::Config(format!(
                "{}: {}{}",
                path.display(),
                key_at(text, e.span())
                    .map(|k| format!("key `{k}`: "))
                    .unwrap_or_default(),
                e
            ))
        })?;
        cfg.validate(path)?;
        Ok(cfg)
    }

    /// Semantic checks that a type system cannot express. Each failure names its key.
    pub fn validate(&self, path: &Path) -> Result<()> {
        let bad = |key: &str, why: String| {
            Error::Config(format!("{}: key `{key}`: {why}", path.display()))
        };
        if self.rings.resident_cap_tokens == 0 {
            return Err(bad(
                "rings.resident_cap_tokens",
                "must be greater than 0".into(),
            ));
        }
        for (key, v) in [
            ("retrieval.k_lex", self.retrieval.k_lex),
            ("retrieval.k_sem", self.retrieval.k_sem),
            ("retrieval.n", self.retrieval.n),
        ] {
            if v == 0 {
                return Err(bad(key, "must be greater than 0".into()));
            }
        }
        if self.retrieval.rrf_k == 0 {
            return Err(bad("retrieval.rrf_k", "must be greater than 0".into()));
        }
        for (i, w) in self.retrieval.ring_weights.iter().enumerate() {
            if !w.is_finite() || *w < 0.0 {
                return Err(bad(
                    "retrieval.ring_weights",
                    format!("entry {i} is {w}; weights must be finite and not negative"),
                ));
            }
        }
        if self.embedding.profile.trim().is_empty() {
            return Err(bad("embedding.profile", "must not be empty".into()));
        }
        if self.embedding.model_path.as_os_str().is_empty() {
            return Err(bad("embedding.model_path", "must not be empty".into()));
        }
        if self.inference.timeout_ms == 0 {
            return Err(bad("inference.timeout_ms", "must be greater than 0".into()));
        }
        let host = endpoint_host(&self.inference.base_url)
            .map_err(|why| bad("inference.base_url", why.to_string()))?;
        if !self.inference.allow_public_endpoint && !host_is_local(&host) {
            return Err(bad(
                "inference.base_url",
                format!(
                    "host `{host}` is not loopback or a private range; project notes would \
                     be sent off the machine. Set inference.allow_public_endpoint = true \
                     only if that is what you want (SPEC §11)"
                ),
            ));
        }
        Ok(())
    }

    /// Serialise. Comments from [`DEFAULT_TOML`] are not preserved; use that constant when
    /// writing a fresh file.
    pub fn to_toml(&self) -> Result<String> {
        toml::to_string_pretty(self).map_err(|e| Error::Config(format!("cannot serialise: {e}")))
    }

    /// Write [`DEFAULT_TOML`] into a store. Refuses to overwrite an existing file.
    pub fn write_default(store: &Path) -> Result<PathBuf> {
        let path = Self::path_in(store);
        let io = |source| Error::Io {
            path: path.clone(),
            source,
        };
        let mut f = std::fs::File::create_new(&path).map_err(io)?;
        std::io::Write::write_all(&mut f, DEFAULT_TOML.as_bytes()).map_err(io)?;
        f.sync_all().map_err(io)?;
        Ok(path)
    }

    /// The embedding model directory, absolute or store-relative resolved.
    pub fn model_dir(&self) -> PathBuf {
        if self.embedding.model_path.is_absolute() {
            self.embedding.model_path.clone()
        } else {
            self.store.join(&self.embedding.model_path)
        }
    }
}

/// Best effort: the key on the line where a TOML error points. The error message from
/// `toml` already shows the line; this puts the key name up front where a script or a
/// grep can find it.
fn key_at(text: &str, span: Option<std::ops::Range<usize>>) -> Option<String> {
    let start = span?.start.min(text.len());
    let line_start = text[..start].rfind('\n').map_or(0, |i| i + 1);
    let line_end = text[start..].find('\n').map_or(text.len(), |i| start + i);
    let line = text[line_start..line_end].trim();
    if line.starts_with('[') {
        return Some(line.trim_matches(['[', ']']).trim().to_string());
    }
    let key = line.split('=').next()?.trim();
    if key.is_empty() || key.starts_with('#') {
        return None;
    }
    // Prefix with the enclosing table, if any.
    let table = text[..line_start]
        .lines()
        .rev()
        .map(str::trim)
        .find(|l| l.starts_with('['))
        .map(|l| l.trim_matches(['[', ']']).trim().to_string());
    Some(match table {
        Some(t) => format!("{t}.{key}"),
        None => key.to_string(),
    })
}

/// The host part of `scheme://host[:port]/...`, without brackets. Only `http` and
/// `https` are accepted.
pub fn endpoint_host(url: &str) -> std::result::Result<String, String> {
    let (scheme, rest) = url
        .split_once("://")
        .ok_or_else(|| format!("`{url}` is not a URL; expected http://host:port/v1"))?;
    if scheme != "http" && scheme != "https" {
        return Err(format!("scheme `{scheme}` is not http or https"));
    }
    let authority = rest.split(['/', '?', '#']).next().unwrap_or("");
    let authority = authority.rsplit('@').next().unwrap_or(authority);
    let host = if let Some(stripped) = authority.strip_prefix('[') {
        stripped
            .split_once(']')
            .map(|(h, _)| h)
            .ok_or_else(|| "unterminated `[` in IPv6 host".to_string())?
    } else {
        authority.split(':').next().unwrap_or("")
    };
    if host.is_empty() {
        return Err(format!("`{url}` has no host"));
    }
    Ok(host.to_ascii_lowercase())
}

/// Loopback, link-local or RFC 1918 / ULA private, judged from the literal only. A host
/// *name* other than `localhost` is treated as public: we do not resolve names, and a
/// name that resolves to a private address today may not tomorrow. Fail closed.
pub fn host_is_local(host: &str) -> bool {
    use std::net::{IpAddr, Ipv4Addr};
    if host == "localhost" || host.ends_with(".localhost") {
        return true;
    }
    match host.parse::<IpAddr>() {
        Ok(IpAddr::V4(v4)) => v4.is_loopback() || v4.is_private() || v4.is_link_local(),
        Ok(IpAddr::V6(v6)) => {
            v6.is_loopback()
                || v6.is_unique_local()
                || v6.is_unicast_link_local()
                || v6.to_ipv4_mapped().is_some_and(|v4: Ipv4Addr| {
                    v4.is_loopback() || v4.is_private() || v4.is_link_local()
                })
            // `::` is deliberately NOT local. It is the unspecified address: legitimate
            // as a bind target, meaningless as a destination. Treating it as local here
            // disagreed with the egress gate, which refuses it as non-unicast, and two
            // components disagreeing about what counts as local is how a hole opens.
        }
        Err(_) => false,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn p() -> &'static Path {
        Path::new("/store/cyberbrain.toml")
    }

    #[test]
    fn documented_default_file_parses_to_the_defaults() {
        let cfg = Config::parse(DEFAULT_TOML, p()).unwrap();
        assert_eq!(cfg, Config::default());
    }

    #[test]
    fn defaults_are_what_the_spec_says() {
        let c = Config::default();
        assert_eq!(c.store, PathBuf::from(".cyberbrain"));
        assert_eq!(c.rings.resident_cap_tokens, 8192);
        assert_eq!(
            (c.retrieval.k_lex, c.retrieval.k_sem, c.retrieval.n),
            (50, 50, 8)
        );
        assert_eq!(c.retrieval.rrf_k, 60);
        assert_eq!(c.retrieval.ring_weights, [1.15, 1.10, 1.00, 0.92, 0.80]);
        // Pinned against Ring::weight() rather than a literal: the config derives its
        // defaults from there, and a second literal is a second source of truth that drifts.
        assert_eq!(c.retrieval.weight(Ring::Invariant), Ring::Invariant.weight());
        for r in Ring::ALL {
            assert_eq!(c.retrieval.weight(r), r.weight(), "{r} drifted from Ring::weight()");
        }
        assert_eq!(c.inference.base_url, "http://127.0.0.1:11434/v1");
        assert!(!c.inference.allow_public_endpoint);
        assert!(!c.inference.allow_overlay_network);
        assert_eq!(c.policy.profile, PolicyProfile::Eu);
    }

    #[test]
    fn missing_file_yields_defaults_with_the_store_filled_in() {
        let dir = tempfile::tempdir().unwrap();
        let cfg = Config::load(dir.path()).unwrap();
        assert_eq!(cfg.store, dir.path());
        assert_eq!(cfg.rings, RingsConfig::default());
        assert_eq!(cfg.model_dir(), dir.path().join("models/model2vec"));
    }

    #[test]
    fn partial_file_fills_the_rest_with_defaults() {
        let cfg = Config::parse("[retrieval]\nn = 3\n[policy]\nprofile = \"ch\"\n", p()).unwrap();
        assert_eq!(cfg.retrieval.n, 3);
        assert_eq!(cfg.retrieval.k_lex, 50);
        assert_eq!(cfg.policy.profile, PolicyProfile::Ch);
        assert_eq!(cfg.inference, InferenceConfig::default());
    }

    #[test]
    fn empty_file_is_the_defaults() {
        let mut cfg = Config::parse("", p()).unwrap();
        cfg.store = Config::default().store;
        assert_eq!(cfg, Config::default());
    }

    #[test]
    fn write_default_then_load_round_trips() {
        let dir = tempfile::tempdir().unwrap();
        let path = Config::write_default(dir.path()).unwrap();
        assert_eq!(std::fs::read_to_string(&path).unwrap(), DEFAULT_TOML);
        let cfg = Config::load(dir.path()).unwrap();
        assert_eq!(cfg.rings, Config::default().rings);
        assert!(
            Config::write_default(dir.path()).is_err(),
            "must not overwrite"
        );
    }

    #[test]
    fn to_toml_round_trips() {
        let mut cfg = Config::default();
        cfg.inference.model = Some("qwen3:8b".into());
        cfg.retrieval.n = 12;
        let text = cfg.to_toml().unwrap();
        let mut back = Config::parse(&text, p()).unwrap();
        back.store = cfg.store.clone();
        assert_eq!(back, cfg);
    }

    fn expect_config_error(text: &str, needle: &str) {
        match Config::parse(text, p()) {
            Err(Error::Config(msg)) => {
                assert!(msg.contains("/store/cyberbrain.toml"), "{msg}");
                assert!(msg.contains(needle), "message {msg:?} lacks {needle:?}");
            }
            other => panic!("expected Config error containing {needle:?}, got {other:?}"),
        }
    }

    #[test]
    fn malformed_file_names_the_key() {
        expect_config_error("[retrieval]\nk_lex = \"fifty\"\n", "retrieval.k_lex");
        expect_config_error("[retrieval]\nring_weights = [1.0, 2.0]\n", "ring_weights");
        expect_config_error("[policy]\nprofile = \"us\"\n", "policy.profile");
        expect_config_error(
            "[inference]\nallow_public_endpoint = \"yes\"\n",
            "allow_public_endpoint",
        );
        expect_config_error(
            "[inference]\nalow_public_endpoint = true\n",
            "alow_public_endpoint",
        );
        expect_config_error("[retrival]\nn = 3\n", "retrival");
        expect_config_error("[retrieval]\nn = 0\n", "retrieval.n");
        expect_config_error(
            "[rings]\nresident_cap_tokens = 0\n",
            "rings.resident_cap_tokens",
        );
        expect_config_error(
            "[retrieval]\nring_weights = [1.0, 1.0, -1.0, 1.0, 1.0]\n",
            "retrieval.ring_weights",
        );
        expect_config_error("this is not toml", "cyberbrain.toml");
    }

    #[test]
    fn public_endpoint_is_refused_unless_allowed() {
        expect_config_error(
            "[inference]\nbase_url = \"https://api.openai.com/v1\"\n",
            "inference.base_url",
        );
        expect_config_error("[inference]\nbase_url = \"http://8.8.8.8/v1\"\n", "8.8.8.8");
        // A bare hostname is public until proven otherwise: we do not resolve names.
        expect_config_error(
            "[inference]\nbase_url = \"http://ollama.local:11434/v1\"\n",
            "ollama.local",
        );
        let ok = Config::parse(
            "[inference]\nbase_url = \"https://api.openai.com/v1\"\nallow_public_endpoint = true\n",
            p(),
        )
        .unwrap();
        assert!(ok.inference.allow_public_endpoint);
        for local in [
            "http://127.0.0.1:11434/v1",
            "http://localhost:1234/v1",
            "http://[::1]:11434/v1",
            "http://10.0.0.5:8000/v1",
            "http://192.168.1.20:11434/v1",
            "http://172.16.4.4/v1",
            "http://[fd00::1]:11434/v1",
        ] {
            Config::parse(&format!("[inference]\nbase_url = \"{local}\"\n"), p())
                .unwrap_or_else(|e| panic!("{local} should be accepted: {e}"));
        }
        expect_config_error("[inference]\nbase_url = \"11434\"\n", "not a URL");
        expect_config_error("[inference]\nbase_url = \"ftp://127.0.0.1/\"\n", "scheme");
    }

    #[test]
    fn endpoint_host_extraction() {
        assert_eq!(
            endpoint_host("http://127.0.0.1:11434/v1").unwrap(),
            "127.0.0.1"
        );
        assert_eq!(endpoint_host("http://[::1]:11434/v1").unwrap(), "::1");
        assert_eq!(
            endpoint_host("http://user:pw@Host.Example/v1").unwrap(),
            "host.example"
        );
        assert_eq!(endpoint_host("http://localhost").unwrap(), "localhost");
        assert!(endpoint_host("http:///v1").is_err());
        assert!(host_is_local("localhost"));
        assert!(!host_is_local("localhost.evil.com"));
        assert!(!host_is_local("100.64.0.1"), "CGNAT is not private");
        // `::` and `0.0.0.0` are bind addresses, not destinations. The egress gate refuses
        // them as non-unicast, and core has to agree: two components with different ideas
        // of "local" is how a hole opens between them.
        assert!(!host_is_local("::"), "the unspecified address is not a destination");
        assert!(!host_is_local("0.0.0.0"), "the unspecified address is not a destination");
        assert!(host_is_local("::1"));
        assert!(host_is_local("fd00::1"));
        assert!(host_is_local("192.168.1.10"));
        assert!(!host_is_local("203.0.113.7"));
    }
}
