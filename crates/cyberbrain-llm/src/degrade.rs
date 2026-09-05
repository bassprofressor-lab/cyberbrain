//! Degradation. Every feature in this crate is optional; when it cannot run, the caller
//! gets a sentence to put into `RecallResult::caveats` and carries on (SPEC §11, §7).
//!
//! The caveat is written for the person reading the recall output: it names the feature,
//! says it did not run, and says why. "Silent inaction is indistinguishable from a broken
//! hook" (SPEC §9.1) applies here too.

use cyberbrain_core::Error;
use std::fmt;

/// A feature that did not run, and the sentence that says so.
#[derive(Debug)]
pub struct Degraded {
    /// Feature name, e.g. `"contradiction check"`.
    pub feature: &'static str,
    /// The sentence for `RecallResult::caveats`.
    pub caveat: String,
    /// What actually happened, for logs and `--json`.
    pub cause: Error,
}

impl Degraded {
    pub fn new(feature: &'static str, cause: Error) -> Self {
        let why = match &cause {
            Error::PolicyRefusal { reason, .. } => format!("endpoint refused by policy: {reason}"),
            Error::Llm(msg) => msg.clone(),
            Error::Config(msg) => format!("not configured: {msg}"),
            other => other.to_string(),
        };
        Self {
            feature,
            caveat: format!("{feature} skipped: {why}"),
            cause,
        }
    }

    /// The feature was never configured. Distinct from a failure so the status screen can
    /// say "off" rather than "broken".
    pub fn not_configured(feature: &'static str) -> Self {
        Self {
            feature,
            caveat: format!("{feature} skipped: no local inference endpoint is configured"),
            cause: Error::Config("no local inference endpoint is configured".into()),
        }
    }

    pub fn is_policy_refusal(&self) -> bool {
        matches!(self.cause, Error::PolicyRefusal { .. })
    }
}

impl fmt::Display for Degraded {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.caveat)
    }
}

impl std::error::Error for Degraded {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        Some(&self.cause)
    }
}

/// Result of an optional feature: the value, or the caveat explaining its absence.
pub type Feature<T> = std::result::Result<T, Degraded>;

/// Attach a feature name to an error from the client.
pub(crate) fn degrade(feature: &'static str) -> impl FnOnce(Error) -> Degraded {
    move |e| Degraded::new(feature, e)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn caveat_names_feature_and_reason() {
        let d = Degraded::new(
            "contradiction check",
            Error::Llm("connection refused".into()),
        );
        assert_eq!(d.caveat, "contradiction check skipped: connection refused");
        assert!(!d.is_policy_refusal());

        let d = Degraded::new(
            "session summary",
            Error::PolicyRefusal {
                profile: "local-inference".into(),
                reason: "x is public".into(),
            },
        );
        assert!(
            d.caveat
                .starts_with("session summary skipped: endpoint refused by policy")
        );
        assert!(d.is_policy_refusal());

        let d = Degraded::not_configured("ring suggestion");
        assert!(
            d.caveat
                .contains("no local inference endpoint is configured")
        );
    }
}
