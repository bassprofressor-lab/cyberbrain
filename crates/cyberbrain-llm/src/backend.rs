//! Which server answered. For the status screen only; nothing in the request path
//! depends on it, which is the point of speaking one API.
//!
//! Identification reads only what came back from `GET /models`: response headers and the
//! `owned_by` field of the listed models. The rules are a short table below. When the
//! evidence is absent or contradicts itself the answer is `Unknown`, with the evidence
//! listed, rather than a guess. A wrong label on a status screen is a small lie that gets
//! repeated in bug reports.

use serde::Serialize;
use std::time::Duration;

use crate::types::ModelInfo;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "kebab-case")]
pub enum Backend {
    Ollama,
    LmStudio,
    /// llama.cpp's `llama-server`.
    LlamaCpp,
    Vllm,
    /// NVIDIA Personal AI Router. Rarely seen as itself: PAIR 1.0 sits on the engines' own
    /// ports and passes their `/models` answer through unchanged, so behind it this table
    /// names the engine (checked against the PAIR 1.0.0 proxy source, 2026-10-23). Kept for a
    /// response that does name it.
    Pair,
    Unknown,
}

impl Backend {
    pub fn name(self) -> &'static str {
        match self {
            Backend::Ollama => "Ollama",
            Backend::LmStudio => "LM Studio",
            Backend::LlamaCpp => "llama.cpp",
            Backend::Vllm => "vLLM",
            Backend::Pair => "NVIDIA PAIR",
            Backend::Unknown => "unknown",
        }
    }
}

impl std::fmt::Display for Backend {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.name())
    }
}

/// Outcome of `LlmClient::probe`, shaped for `cyberbrain status`.
#[derive(Debug, Clone, Serialize)]
pub struct Probe {
    pub reachable: bool,
    pub backend: Backend,
    /// `ValidatedEndpoint::summary()`.
    pub endpoint: String,
    /// Model ids the endpoint lists.
    pub models: Vec<String>,
    pub configured_model: String,
    /// Whether `configured_model` appears in `models`. False with an empty configuration.
    pub configured_model_listed: bool,
    /// What the identification saw, one line each, so a wrong label can be argued with.
    pub evidence: Vec<String>,
    /// Anything the operator should read: unreachable, model missing, and so on.
    pub caveat: Option<String>,
    pub latency: Duration,
}

/// Apply the rule table. Returns the verdict and the evidence lines that produced it.
pub fn identify(headers: &[(String, String)], models: &[ModelInfo]) -> (Backend, Vec<String>) {
    let mut votes: Vec<(Backend, String)> = Vec::new();

    for (name, value) in headers {
        let n = name.to_ascii_lowercase();
        let v = value.to_ascii_lowercase();
        let relevant = n == "server" || n == "x-powered-by" || n.starts_with("x-");
        if !relevant {
            continue;
        }
        // llama-server sets exactly `Server: llama.cpp` (server-http.cpp, b11840). vLLM's
        // `server: uvicorn` is uvicorn's default and names nothing, so it is not read.
        if n == "server" && v.starts_with("llama.cpp") {
            votes.push((Backend::LlamaCpp, format!("header {name}: {value}")));
        } else if v.contains("ollama") {
            votes.push((Backend::Ollama, format!("header {name}: {value}")));
        } else if v.contains("lmstudio") || v.contains("lm-studio") || v.contains("lm studio") {
            votes.push((Backend::LmStudio, format!("header {name}: {value}")));
        } else if v.contains("nvidia") || token_match(&v, "pair") || n.contains("pair") {
            votes.push((Backend::Pair, format!("header {name}: {value}")));
        }
    }

    let mut owners: Vec<String> = models
        .iter()
        .filter_map(|m| m.owned_by.as_deref())
        .map(|o| o.to_ascii_lowercase())
        .collect();
    owners.sort();
    owners.dedup();
    for owner in &owners {
        match owner.as_str() {
            // Ollama reports the model name's namespace: "library" for a plain name like
            // `qwen2.5:7b`, the user or host for `user/model` (openai.go, 0.40.10). Only the
            // plain case identifies it; a store with only namespaced models reads Unknown.
            "library" => votes.push((Backend::Ollama, format!("models.owned_by = {owner:?}"))),
            // LM Studio reports "organization_owner" (the OpenAI placeholder it copied); one
            // guide spells it with a hyphen, accepted too.
            "organization_owner" | "organization-owner" => {
                votes.push((Backend::LmStudio, format!("models.owned_by = {owner:?}")))
            }
            "llamacpp" => votes.push((Backend::LlamaCpp, format!("models.owned_by = {owner:?}"))),
            "vllm" => votes.push((Backend::Vllm, format!("models.owned_by = {owner:?}"))),
            o if o.contains("nvidia") || token_match(o, "pair") => {
                votes.push((Backend::Pair, format!("models.owned_by = {owner:?}")))
            }
            _ => {}
        }
    }

    let mut evidence: Vec<String> = votes.iter().map(|(b, e)| format!("{e} -> {b}")).collect();
    let mut kinds: Vec<Backend> = votes.iter().map(|(b, _)| *b).collect();
    kinds.sort_by_key(|b| *b as u8);
    kinds.dedup();

    let verdict = match kinds.as_slice() {
        [one] => *one,
        [] => {
            evidence.push(if models.is_empty() {
                "no models listed and no identifying header".to_string()
            } else {
                format!(
                    "no identifying header; owned_by values {:?} match no known backend",
                    owners
                )
            });
            Backend::Unknown
        }
        many => {
            evidence.push(format!(
                "conflicting signals ({}); not guessing",
                many.iter().map(|b| b.name()).collect::<Vec<_>>().join(", ")
            ));
            Backend::Unknown
        }
    };
    (verdict, evidence)
}

/// True if `word` appears in `s` delimited by non-alphanumerics, so "repair" is not "pair".
fn token_match(s: &str, word: &str) -> bool {
    s.split(|c: char| !c.is_ascii_alphanumeric())
        .any(|t| t == word)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn m(id: &str, owner: Option<&str>) -> ModelInfo {
        ModelInfo {
            id: id.into(),
            owned_by: owner.map(String::from),
        }
    }
    fn h(k: &str, v: &str) -> (String, String) {
        (k.into(), v.into())
    }

    #[test]
    fn ollama_by_owned_by() {
        let (b, ev) = identify(
            &[h("content-type", "application/json")],
            &[m("llama3", Some("library"))],
        );
        assert_eq!(b, Backend::Ollama);
        assert!(ev.iter().any(|e| e.contains("library")), "{ev:?}");
    }

    #[test]
    fn lm_studio_by_owned_by() {
        let (b, _) = identify(&[], &[m("qwen", Some("organization_owner"))]);
        assert_eq!(b, Backend::LmStudio);
    }

    #[test]
    fn llama_cpp_by_server_header_and_owner() {
        let (b, ev) = identify(&[h("Server", "llama.cpp")], &[m("qwen", Some("llamacpp"))]);
        assert_eq!(b, Backend::LlamaCpp, "{ev:?}");
        let (b, _) = identify(&[], &[m("qwen", Some("llamacpp"))]);
        assert_eq!(b, Backend::LlamaCpp);
    }

    #[test]
    fn vllm_by_owner_not_by_uvicorn() {
        let (b, _) = identify(&[h("server", "uvicorn")], &[m("x", Some("vllm"))]);
        assert_eq!(b, Backend::Vllm);
        let (b, _) = identify(&[h("server", "uvicorn")], &[]);
        assert_eq!(b, Backend::Unknown, "uvicorn names no backend");
    }

    /// Hypothetical answers: PAIR 1.0 sets no identifying header and passes `owned_by`
    /// through. What is tested is the rule, should a response name it.
    #[test]
    fn pair_by_header_or_owner() {
        let (b, _) = identify(
            &[h("server", "nvidia-pair/0.1")],
            &[m("x", Some("library"))],
        );
        // Header says PAIR, owner says Ollama (PAIR proxies Ollama): conflicting → Unknown.
        assert_eq!(b, Backend::Unknown);
        let (b, _) = identify(&[h("x-pair-node", "spark-01")], &[m("x", None)]);
        assert_eq!(b, Backend::Pair);
        let (b, _) = identify(&[], &[m("x", Some("nvidia"))]);
        assert_eq!(b, Backend::Pair);
        let (b, _) = identify(&[h("server", "repair-bot")], &[]);
        assert_eq!(b, Backend::Unknown, "'repair' is not 'pair'");
    }

    #[test]
    fn nothing_identifying_is_unknown_and_says_why() {
        let (b, ev) = identify(
            &[h("content-type", "application/json")],
            &[m("x", Some("huggingface"))],
        );
        assert_eq!(b, Backend::Unknown);
        assert!(
            ev.last().unwrap().contains("match no known backend"),
            "{ev:?}"
        );
        let (b, ev) = identify(&[], &[]);
        assert_eq!(b, Backend::Unknown);
        assert!(ev.last().unwrap().contains("no models listed"));
    }

    #[test]
    fn conflicting_signals_are_unknown_not_a_coin_toss() {
        let (b, ev) = identify(
            &[],
            &[m("a", Some("library")), m("b", Some("organization_owner"))],
        );
        assert_eq!(b, Backend::Unknown);
        assert!(ev.last().unwrap().contains("conflicting"), "{ev:?}");
    }
}
