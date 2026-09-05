//! What the policy layer needs to know, gathered from `cyberbrain.toml` (SPEC §11, §12)
//! plus two facts the config file does not carry.
//!
//! [`PolicyConfig::from_core`] takes the parsed [`cyberbrain_core::Config`] and copies the
//! profile and the inference endpoint settings. Two fields have no home in core's config:
//!
//! - `model_source`: where model artefacts are fetched from. Not an operator setting today;
//!   the binary fills it from its model registry.
//! - `model_download_consent`: SPEC §6/§12.1 say a download happens "once, on explicit
//!   consent". Core's `Config` rejects unknown keys, so consent cannot be persisted there
//!   without a core change. The binary sets it at runtime after asking, and this crate
//!   refuses `ModelDownload` while it is false. See the report: this is a gap in core.

use crate::profile::Profile;
use serde::{Deserialize, Serialize};

pub use cyberbrain_core::config::DEFAULT_INFERENCE_URL as DEFAULT_INFERENCE_ENDPOINT;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct PolicyConfig {
    /// `eu` | `ch` | `off`. Defaults to `eu`: a compliance layer that defaults to off is
    /// decoration.
    pub profile: Profile,

    /// Base URL of the OpenAI-compatible local inference server (SPEC §11). Used by the
    /// register to say whether `LocalInference` is currently enabled; the gate itself judges
    /// the destination it is handed, not this string.
    pub inference_endpoint: String,

    /// SPEC §11: the inference endpoint must be loopback or private-range unless the
    /// operator sets this explicitly. Every permitted call to a public address is recorded
    /// as a transfer.
    pub allow_public_endpoint: bool,

    /// SPEC §12.1: permits `100.64.0.0/10` (carrier-grade NAT, also Tailscale-style
    /// overlays). Its own switch, never folded into `allow_public_endpoint`.
    pub allow_overlay_network: bool,

    /// Where model artefacts come from. `None` means `ModelDownload` is disabled: there is
    /// nowhere registered to fetch from.
    pub model_source: Option<String>,

    /// SPEC §6/§12.1: model artefacts are fetched once, on explicit consent. `false` means
    /// `ModelDownload` is refused.
    pub model_download_consent: bool,
}

impl Default for PolicyConfig {
    fn default() -> Self {
        Self {
            profile: Profile::Eu,
            inference_endpoint: DEFAULT_INFERENCE_ENDPOINT.to_string(),
            allow_public_endpoint: false,
            allow_overlay_network: false,
            model_source: None,
            model_download_consent: false,
        }
    }
}

impl PolicyConfig {
    /// Copy what the store's config file says. Consent and model source stay at their
    /// disabled defaults; set them with the builders below.
    pub fn from_core(cfg: &cyberbrain_core::Config) -> Self {
        Self {
            profile: cfg.policy.profile,
            inference_endpoint: cfg.inference.base_url.clone(),
            allow_public_endpoint: cfg.inference.allow_public_endpoint,
            allow_overlay_network: cfg.inference.allow_overlay_network,
            ..Self::default()
        }
    }

    pub fn with_model_source(mut self, url: impl Into<String>) -> Self {
        self.model_source = Some(url.into());
        self
    }

    /// Record that the operator agreed to the download. Callers must have actually asked.
    pub fn with_model_download_consent(mut self, consent: bool) -> Self {
        self.model_download_consent = consent;
        self
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn defaults_are_the_safe_ones() {
        let c = PolicyConfig::default();
        assert_eq!(c.profile, Profile::Eu);
        assert!(!c.allow_public_endpoint);
        assert!(!c.allow_overlay_network);
        assert!(!c.model_download_consent);
        assert!(c.model_source.is_none());
        assert_eq!(c.inference_endpoint, DEFAULT_INFERENCE_ENDPOINT);
    }

    #[test]
    fn from_core_copies_the_file_and_keeps_consent_off() {
        let core = cyberbrain_core::Config::parse(
            "[inference]\nbase_url = \"http://10.0.0.5:8000/v1\"\nallow_overlay_network = true\n[policy]\nprofile = \"ch\"\n",
            std::path::Path::new("/store/cyberbrain.toml"),
        )
        .unwrap();
        let c = PolicyConfig::from_core(&core);
        assert_eq!(c.profile, Profile::Ch);
        assert_eq!(c.inference_endpoint, "http://10.0.0.5:8000/v1");
        assert!(c.allow_overlay_network);
        assert!(!c.allow_public_endpoint);
        assert!(
            !c.model_download_consent,
            "consent is never implied by a config file"
        );
        assert!(c.model_source.is_none());
    }

    #[test]
    fn a_partial_table_is_complete() {
        let c: PolicyConfig = serde_json::from_str(r#"{"profile":"ch"}"#).unwrap();
        assert_eq!(c.profile, Profile::Ch);
        assert_eq!(c.inference_endpoint, DEFAULT_INFERENCE_ENDPOINT);
    }

    #[test]
    fn unknown_keys_are_rejected_not_ignored() {
        let r: Result<PolicyConfig, _> =
            serde_json::from_str(r#"{"profile":"eu","allow_public_endpiont":true}"#);
        assert!(r.is_err());
    }
}
