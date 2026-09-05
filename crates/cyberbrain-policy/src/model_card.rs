//! Model transparency (SPEC §12.7, EU AI Act).
//!
//! `cyberbrain policy model-card` prints the identity, source, licence, hash, dimension and
//! intended use of every model artefact in use. The embedding crate knows its artefact and
//! the inference layer knows its endpoint; each implements [`ModelInventory`] and this
//! module renders the cards.
//!
//! Every field a crate cannot vouch for is `None` and renders as **not stated**. A model
//! card that fills a licence field with a guess is worse than one with a gap: the gap is
//! visible, the guess is a claim. The card also does not classify Cyberbrain under the AI
//! Act; the spec's "minimal-risk" reading is the deployer's call and is printed as such.

use serde::{Deserialize, Serialize};
use std::path::PathBuf;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum ModelRole {
    /// Produces the vectors in the index. Ships with, or is fetched by, Cyberbrain.
    Embedding,
    /// The operator's own local inference endpoint. Cyberbrain does not ship it.
    Inference,
}

impl ModelRole {
    pub fn as_str(self) -> &'static str {
        match self {
            ModelRole::Embedding => "embedding",
            ModelRole::Inference => "inference",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ModelCard {
    pub role: ModelRole,
    /// Model identity as its source names it, e.g. `minishlab/potion-base-8M`.
    pub name: String,
    pub version: Option<String>,
    /// Where it came from: a URL for a downloaded artefact, the base URL for an endpoint.
    pub source: String,
    /// SPDX identifier or licence name, exactly as the source states it. `None` if the
    /// crate did not read it from the artefact or its metadata.
    pub licence: Option<String>,
    /// blake3 of the artefact files, lowercase hex, as verified on load.
    pub blake3: Option<String>,
    /// Whether the hash above was checked against the manifest on the last load.
    pub hash_verified: Option<bool>,
    pub dimension: Option<usize>,
    pub pooling: Option<String>,
    /// e.g. `model2vec safetensors + tokenizer.json`, or `OpenAI-compatible HTTP`.
    pub format: Option<String>,
    pub size_bytes: Option<u64>,
    pub artefact_path: Option<PathBuf>,
    pub intended_use: String,
    /// Known limitations, in words.
    pub limitations: Vec<String>,
    /// Anything else the crate wants on paper.
    pub notes: Vec<String>,
}

impl ModelCard {
    /// A card with every optional field absent. Crates fill what they can vouch for.
    pub fn new(
        role: ModelRole,
        name: impl Into<String>,
        source: impl Into<String>,
        intended_use: impl Into<String>,
    ) -> Self {
        Self {
            role,
            name: name.into(),
            version: None,
            source: source.into(),
            licence: None,
            blake3: None,
            hash_verified: None,
            dimension: None,
            pooling: None,
            format: None,
            size_bytes: None,
            artefact_path: None,
            intended_use: intended_use.into(),
            limitations: Vec::new(),
            notes: Vec::new(),
        }
    }

    /// Fields a deployer will be asked for that this card cannot answer.
    pub fn gaps(&self) -> Vec<&'static str> {
        let mut g = Vec::new();
        if self.licence.is_none() {
            g.push("licence");
        }
        if self.role == ModelRole::Embedding {
            if self.blake3.is_none() {
                g.push("hash");
            }
            if self.hash_verified != Some(true) {
                g.push("hash verification");
            }
            if self.dimension.is_none() {
                g.push("dimension");
            }
        }
        g
    }
}

/// Implemented by the embedding backend and the inference client.
pub trait ModelInventory {
    fn model_cards(&self) -> Vec<ModelCard>;
}

fn opt<T: std::fmt::Display>(v: &Option<T>) -> String {
    v.as_ref()
        .map(|x| x.to_string())
        .unwrap_or_else(|| "not stated".into())
}

pub fn render_markdown(cards: &[ModelCard]) -> String {
    let mut s = String::from("# Model card\n\n");
    s.push_str("Cyberbrain records here every model artefact it uses. Fields marked *not stated* are unknown to the code and must not be assumed.\n\n");
    if cards.is_empty() {
        s.push_str(
            "No model is in use. Semantic search and the inference layer are both inactive.\n",
        );
    }
    for c in cards {
        s.push_str(&format!("## {} ({})\n\n", c.name, c.role.as_str()));
        s.push_str(&format!("- Version: {}\n", opt(&c.version)));
        s.push_str(&format!("- Source: {}\n", c.source));
        s.push_str(&format!("- Licence: {}\n", opt(&c.licence)));
        s.push_str(&format!("- Format: {}\n", opt(&c.format)));
        if c.role == ModelRole::Embedding {
            s.push_str(&format!("- blake3: {}\n", opt(&c.blake3)));
            s.push_str(&format!(
                "- Hash verified on load: {}\n",
                match c.hash_verified {
                    Some(true) => "yes",
                    Some(false) => "NO",
                    None => "not stated",
                }
            ));
            s.push_str(&format!("- Dimension: {}\n", opt(&c.dimension)));
            s.push_str(&format!("- Pooling: {}\n", opt(&c.pooling)));
            s.push_str(&format!(
                "- Size: {}\n",
                opt(&c.size_bytes.map(|b| format!("{b} bytes")))
            ));
            s.push_str(&format!(
                "- Artefact: {}\n",
                opt(&c.artefact_path.as_ref().map(|p| p.display().to_string()))
            ));
        }
        s.push_str(&format!("- Intended use: {}\n", c.intended_use));
        if !c.limitations.is_empty() {
            s.push_str("- Limitations:\n");
            for l in &c.limitations {
                s.push_str(&format!("  - {l}\n"));
            }
        }
        for n in &c.notes {
            s.push_str(&format!("- Note: {n}\n"));
        }
        let gaps = c.gaps();
        if !gaps.is_empty() {
            s.push_str(&format!("- **Gaps**: {}\n", gaps.join(", ")));
        }
        s.push('\n');
    }
    s.push_str("## Regulatory note\n\n");
    s.push_str("Cyberbrain is a local memory tool. Its specification describes it as a minimal-risk system under the EU AI Act; that classification depends on how it is deployed and is the deployer's determination, not this tool's. The inference model, if any, is operated by you on your own hardware; its provider's obligations are theirs.\n");
    s
}

pub fn render_json(cards: &[ModelCard]) -> String {
    serde_json::to_string_pretty(cards).expect("ModelCard serialises")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn unknown_fields_render_as_not_stated_and_are_listed_as_gaps() {
        let c = ModelCard::new(
            ModelRole::Embedding,
            "minishlab/potion-base-8M",
            "https://huggingface.co/minishlab/potion-base-8M",
            "static token embeddings for hybrid recall",
        );
        let md = render_markdown(std::slice::from_ref(&c));
        assert!(md.contains("Licence: not stated"));
        assert!(md.contains("Hash verified on load: not stated"));
        assert!(md.contains("**Gaps**: licence, hash, hash verification, dimension"));
        assert_eq!(
            c.gaps(),
            ["licence", "hash", "hash verification", "dimension"]
        );
    }

    #[test]
    fn a_complete_card_has_no_gaps() {
        let mut c = ModelCard::new(ModelRole::Embedding, "m", "https://x", "y");
        c.licence = Some("MIT".into());
        c.blake3 = Some("ab".repeat(32));
        c.hash_verified = Some(true);
        c.dimension = Some(256);
        assert!(c.gaps().is_empty());
        assert!(render_markdown(&[c]).contains("Hash verified on load: yes"));
    }

    #[test]
    fn an_inference_card_does_not_demand_a_hash() {
        let mut c = ModelCard::new(
            ModelRole::Inference,
            "qwen3:8b",
            "http://127.0.0.1:11434/v1",
            "summaries, contradiction checks",
        );
        c.licence = Some("Apache-2.0".into());
        assert!(c.gaps().is_empty());
        let md = render_markdown(&[c]);
        assert!(!md.contains("blake3"));
        assert!(md.contains("deployer's determination"));
    }

    #[test]
    fn empty_inventory_says_so() {
        assert!(render_markdown(&[]).contains("No model is in use"));
        assert_eq!(render_json(&[]), "[]");
    }
}
