//! Compliance profiles (SPEC §12): `eu`, `ch`, `off`.
//!
//! `ch` is a separate profile, not "EU minus". The revised Swiss FADP (in force since
//! 1 September 2023) differs from the GDPR in substance: there is no 72-hour breach clock,
//! the breach threshold is *high* risk rather than *risk*, a private controller needs no
//! legal basis to process, the register-of-processing exemption is a plain headcount rule,
//! and there is no standalone "right to erasure" article. Folding the two together produces
//! statements that are wrong in one of the two countries.
//!
//! **Every claim below carries a [`Confidence`].** The author is an engineer, not counsel.
//! `High` means: the rule is stated in the primary text and the author has verified the
//! substance; the article number may still be worth a second look. `Medium` means: the
//! substance is right to the author's knowledge but the numbering, threshold or an
//! exception is from memory. `Low` means: do not repeat this to a regulator without
//! checking. Nothing at `Low` drives behaviour in code; it is printed with its label so an
//! operator sees the gap instead of a false certainty.

use crate::Confidence;
use serde::{Deserialize, Serialize};
use std::fmt;

/// The profile type is core's: every crate speaks the same vocabulary and nothing defines
/// its own copy. The behaviour lives here, on [`ProfileExt`].
pub use cyberbrain_core::PolicyProfile as Profile;

pub const ALL_PROFILES: [Profile; 3] = [Profile::Eu, Profile::Ch, Profile::Off];

/// `eu` | `ch` | `off`, case-insensitive. Core's type carries no `FromStr` of its own.
pub fn parse_profile(s: &str) -> cyberbrain_core::Result<Profile> {
    match s.trim().to_ascii_lowercase().as_str() {
        "eu" => Ok(Profile::Eu),
        "ch" => Ok(Profile::Ch),
        "off" => Ok(Profile::Off),
        other => Err(cyberbrain_core::Error::Config(format!(
            "unknown policy profile {other:?}; expected eu, ch or off"
        ))),
    }
}

/// What a profile says and does. Implemented for [`Profile`]; import this trait to call it.
pub trait ProfileExt: Copy {
    /// The hot-path question. One comparison, no allocation.
    fn is_off(self) -> bool;
    /// SPEC §12.4: under `eu` and `ch` a write with PII findings is held for the operator.
    fn holds_pii_writes(self) -> bool;
    fn law(self) -> &'static str;
    /// The statutory deadline for answering a subject access request.
    fn access_response_deadline(self) -> Deadline;
    /// What the erasure audit row cites as its basis.
    fn erasure_basis(self) -> &'static str;
    fn breach_rule(self) -> BreachRule;
    fn register_rule(self) -> RegisterRule;
    /// The full, printable list of what this profile encodes and how sure we are.
    fn obligations(self) -> Vec<Obligation>;
}

impl ProfileExt for Profile {
    #[inline(always)]
    fn is_off(self) -> bool {
        matches!(self, Profile::Off)
    }

    #[inline(always)]
    fn holds_pii_writes(self) -> bool {
        !self.is_off()
    }

    fn law(self) -> &'static str {
        match self {
            Profile::Eu => "GDPR, Regulation (EU) 2016/679",
            Profile::Ch => "FADP (revised), SR 235.1, in force 2023-09-01",
            Profile::Off => "no compliance profile",
        }
    }

    fn access_response_deadline(self) -> Deadline {
        match self {
            // Art. 12(3) GDPR: without undue delay and in any event within one month,
            // extendable by two further months for complex or numerous requests.
            Profile::Eu => Deadline::OneMonth,
            // Art. 25(7) FADP: as a rule within 30 days. Numbering: Medium confidence;
            // substance (30 days): High.
            Profile::Ch => Deadline::Days(30),
            Profile::Off => Deadline::None,
        }
    }

    fn erasure_basis(self) -> &'static str {
        match self {
            Profile::Eu => "GDPR Art. 17 (right to erasure); recipients informed per Art. 19",
            // The FADP has no article titled "right to erasure". Deletion follows from the
            // storage-limitation principle (Art. 6(4): destroy or anonymise once no longer
            // required) and the civil claim in Art. 32(2)(c). Medium confidence on numbering.
            Profile::Ch => {
                "FADP Art. 6(4) (destroy or anonymise when no longer required); \
                            Art. 32(2)(c) (claim for deletion)"
            }
            Profile::Off => "operator request, no profile active",
        }
    }

    fn breach_rule(self) -> BreachRule {
        match self {
            Profile::Eu => BreachRule {
                authority: "the competent supervisory authority",
                deadline: Deadline::Hours(72),
                threshold: "unless the breach is unlikely to result in a risk to the rights \
                            and freedoms of natural persons",
                subjects: "without undue delay when the breach is likely to result in a HIGH \
                           risk (Art. 34)",
                basis: "GDPR Art. 33, 34",
                confidence: Confidence::High,
            },
            Profile::Ch => BreachRule {
                authority: "the FDPIC (EDÖB / PFPDT)",
                // No fixed clock in the FADP. "As soon as possible" (so rasch als möglich).
                deadline: Deadline::AsSoonAsPossible,
                // Note the threshold is HIGH risk from the outset, unlike Art. 33 GDPR.
                threshold: "when the breach is likely to result in a HIGH risk to the \
                            personality or fundamental rights of the data subject",
                subjects: "when necessary for their protection or when the FDPIC requests it",
                basis: "FADP Art. 24",
                confidence: Confidence::High,
            },
            Profile::Off => BreachRule {
                authority: "none configured",
                deadline: Deadline::None,
                threshold: "no profile active",
                subjects: "no profile active",
                basis: "none",
                confidence: Confidence::High,
            },
        }
    }

    fn register_rule(self) -> RegisterRule {
        match self {
            Profile::Eu => RegisterRule {
                required: "yes, for controllers and processors",
                exemption: "organisations under 250 employees, but NOT if the processing is \
                            likely to result in a risk, is not occasional, or involves \
                            special-category or criminal data (Art. 30(5)). In practice a \
                            memory store that runs every day is 'not occasional', so treat \
                            the record as required.",
                basis: "GDPR Art. 30",
                confidence: Confidence::High,
            },
            Profile::Ch => RegisterRule {
                required: "yes, for controllers and processors",
                exemption: "companies with fewer than 250 employees, unless they process \
                            sensitive personal data on a large scale or carry out high-risk \
                            profiling (Art. 12(5) FADP with the Data Protection Ordinance). \
                            This is a plain headcount rule; the 'not occasional' hook of the \
                            GDPR does not exist here.",
                basis: "FADP Art. 12",
                // Substance High; the ordinance article that carries the threshold is
                // from memory (DPO Art. 24), hence Medium overall.
                confidence: Confidence::Medium,
            },
            Profile::Off => RegisterRule {
                required: "no profile active",
                exemption: "",
                basis: "none",
                confidence: Confidence::High,
            },
        }
    }

    fn obligations(self) -> Vec<Obligation> {
        match self {
            Profile::Eu => eu_obligations(),
            Profile::Ch => ch_obligations(),
            Profile::Off => vec![Obligation {
                topic: Topic::Scope,
                summary: "No compliance profile is active. PII scanning and write holds are \
                          disabled. The egress gate and the audit log stay on because \
                          local-first is a product property, not a compliance setting.",
                basis: "SPEC §12",
                confidence: Confidence::High,
                note: "",
            }],
        }
    }
}

/// A statutory time limit, kept structured so the UI can render a countdown.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "kebab-case")]
pub enum Deadline {
    Hours(u32),
    Days(u32),
    OneMonth,
    AsSoonAsPossible,
    None,
}

impl fmt::Display for Deadline {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Deadline::Hours(h) => write!(f, "within {h} hours"),
            Deadline::Days(d) => write!(f, "within {d} days"),
            Deadline::OneMonth => f.write_str("within one month (extendable by two months)"),
            Deadline::AsSoonAsPossible => f.write_str("as soon as possible (no fixed clock)"),
            Deadline::None => f.write_str("no deadline (no profile active)"),
        }
    }
}

#[derive(Debug, Clone, Serialize)]
pub struct BreachRule {
    pub authority: &'static str,
    pub deadline: Deadline,
    pub threshold: &'static str,
    pub subjects: &'static str,
    pub basis: &'static str,
    pub confidence: Confidence,
}

#[derive(Debug, Clone, Serialize)]
pub struct RegisterRule {
    pub required: &'static str,
    pub exemption: &'static str,
    pub basis: &'static str,
    pub confidence: Confidence,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum Topic {
    Scope,
    LegalBasis,
    Erasure,
    Access,
    BreachNotification,
    ProcessingRegister,
    DataProtectionOfficer,
    ImpactAssessment,
    CrossBorderTransfer,
    Sanctions,
    AiRegulation,
}

/// One thing the profile says about the law, with how sure we are and why.
#[derive(Debug, Clone, Serialize)]
pub struct Obligation {
    pub topic: Topic,
    pub summary: &'static str,
    pub basis: &'static str,
    pub confidence: Confidence,
    /// What is uncertain, or what the operator must decide for themselves. Empty when
    /// nothing needs saying.
    pub note: &'static str,
}

fn eu_obligations() -> Vec<Obligation> {
    vec![
        Obligation {
            topic: Topic::Scope,
            summary: "Applies to personal data of natural persons. A memory store is in scope \
                      as soon as a note names, describes or identifies a person.",
            basis: "GDPR Art. 2, Art. 4(1)",
            confidence: Confidence::High,
            note: "Whether a given store contains personal data at all is a fact about its \
                   content, not something this tool can assert.",
        },
        Obligation {
            topic: Topic::LegalBasis,
            summary: "Every processing operation needs one of the six legal bases.",
            basis: "GDPR Art. 6(1)",
            confidence: Confidence::High,
            note: "Which basis applies (legitimate interest is the usual candidate for an \
                   engineering notebook) is the operator's determination.",
        },
        Obligation {
            topic: Topic::Erasure,
            summary: "Data subjects may demand erasure; the controller erases without undue \
                      delay and informs recipients.",
            basis: "GDPR Art. 17, Art. 19",
            confidence: Confidence::High,
            note: "`cyberbrain forget` is the mechanism; the audit row is the evidence.",
        },
        Obligation {
            topic: Topic::Access,
            summary: "Data subjects may obtain a copy of their data. Respond within one \
                      month; extendable by two months for complex requests.",
            basis: "GDPR Art. 15, Art. 12(3)",
            confidence: Confidence::High,
            note: "",
        },
        Obligation {
            topic: Topic::BreachNotification,
            summary: "Notify the supervisory authority within 72 hours of becoming aware of \
                      a breach unless it is unlikely to result in a risk; inform data \
                      subjects without undue delay when the risk is high.",
            basis: "GDPR Art. 33, Art. 34",
            confidence: Confidence::High,
            note: "",
        },
        Obligation {
            topic: Topic::ProcessingRegister,
            summary: "Keep a record of processing activities. The under-250 exemption does \
                      not apply to processing that is 'not occasional'.",
            basis: "GDPR Art. 30",
            confidence: Confidence::High,
            note: "`cyberbrain policy register` prints Cyberbrain's own record entry.",
        },
        Obligation {
            topic: Topic::DataProtectionOfficer,
            summary: "A DPO is mandatory for public authorities and for large-scale regular \
                      monitoring or large-scale special-category processing.",
            basis: "GDPR Art. 37",
            confidence: Confidence::Medium,
            note: "'Large scale' is interpretive. A single developer's notebook is not it; a \
                   company-wide deployment might be.",
        },
        Obligation {
            topic: Topic::ImpactAssessment,
            summary: "A DPIA is required where processing is likely to result in a high risk.",
            basis: "GDPR Art. 35, Art. 36",
            confidence: Confidence::High,
            note: "",
        },
        Obligation {
            topic: Topic::CrossBorderTransfer,
            summary: "Cyberbrain performs no transfer: notes never leave the machine except \
                      through the two registered egress purposes, both of which stay on the \
                      operator's own network or fetch (not send) model weights.",
            basis: "GDPR Chapter V (Art. 44-49); SPEC §12.1",
            confidence: Confidence::High,
            note: "If the operator sets allow_public_endpoint the inference purpose becomes \
                   a transfer and Chapter V applies. The audit row says so.",
        },
        Obligation {
            topic: Topic::Sanctions,
            summary: "Administrative fines up to EUR 20 million or 4 % of worldwide turnover.",
            basis: "GDPR Art. 83(5)",
            confidence: Confidence::High,
            note: "",
        },
        Obligation {
            topic: Topic::AiRegulation,
            summary: "The EU AI Act applies in the EU. Cyberbrain ships model weights and \
                      prints a model card so a deployer inside a regulated workflow has the \
                      identity, source, licence and hash on paper.",
            basis: "Regulation (EU) 2024/1689",
            confidence: Confidence::Low,
            note: "The spec calls Cyberbrain 'minimal-risk'. That is a plausible reading, not \
                   a determination this tool can make; risk class depends on the deployer's \
                   use. GPAI obligations (Art. 53) fall on model providers, and whether a \
                   redistributed ~30 MB static embedding model triggers any of them is not \
                   something the author is confident about.",
        },
    ]
}

fn ch_obligations() -> Vec<Obligation> {
    vec![
        Obligation {
            topic: Topic::Scope,
            summary: "Applies to personal data of natural persons only. The revision dropped \
                      legal persons from scope.",
            basis: "FADP Art. 2(1), Art. 5(a)",
            confidence: Confidence::High,
            note: "",
        },
        Obligation {
            topic: Topic::LegalBasis,
            summary: "A private controller needs NO legal basis to process. Processing is \
                      lawful unless it unlawfully violates the data subject's personality; \
                      justification is needed only when the principles are breached.",
            basis: "FADP Art. 30, Art. 31",
            confidence: Confidence::High,
            note: "This is the single largest substantive difference from the GDPR and the \
                   main reason `ch` is not `eu` with fewer checks.",
        },
        Obligation {
            topic: Topic::Erasure,
            summary: "There is no standalone right-to-erasure article. Deletion follows from \
                      the storage-limitation principle (destroy or anonymise once no longer \
                      required) and from the civil claim for deletion. No fixed deadline.",
            basis: "FADP Art. 6(4), Art. 32(2)(c)",
            confidence: Confidence::Medium,
            note: "Substance is High; the paragraph numbering is from memory.",
        },
        Obligation {
            topic: Topic::Access,
            summary: "Data subjects may request information; provide it as a rule within 30 \
                      days. A right to data portability exists separately.",
            basis: "FADP Art. 25, Art. 28",
            confidence: Confidence::Medium,
            note: "30 days: High. That it sits in Art. 25(7): Medium.",
        },
        Obligation {
            topic: Topic::BreachNotification,
            summary: "Notify the FDPIC as soon as possible when a breach is likely to result \
                      in a HIGH risk to the data subject. No 72-hour clock. Inform data \
                      subjects when necessary for their protection or when the FDPIC asks.",
            basis: "FADP Art. 24",
            confidence: Confidence::High,
            note: "Both the timing and the threshold differ from GDPR Art. 33.",
        },
        Obligation {
            topic: Topic::ProcessingRegister,
            summary: "Keep a register of processing activities. Companies under 250 \
                      employees are exempt unless they process sensitive data on a large \
                      scale or perform high-risk profiling.",
            basis: "FADP Art. 12; Data Protection Ordinance",
            confidence: Confidence::Medium,
            note: "The required fields (Art. 12(2)) overlap with but are not identical to \
                   GDPR Art. 30(1): there is no DPO field, and security measures and \
                   retention are 'where possible'.",
        },
        Obligation {
            topic: Topic::DataProtectionOfficer,
            summary: "A data protection advisor is optional for private controllers.",
            basis: "FADP Art. 10",
            confidence: Confidence::High,
            note: "Appointing one lets the controller skip consulting the FDPIC after a DPIA \
                   (Art. 23(4)). Medium confidence on that paragraph.",
        },
        Obligation {
            topic: Topic::ImpactAssessment,
            summary: "A DPIA is required where processing may entail a high risk; the FDPIC \
                      is consulted if the residual risk stays high.",
            basis: "FADP Art. 22, Art. 23",
            confidence: Confidence::Medium,
            note: "",
        },
        Obligation {
            topic: Topic::CrossBorderTransfer,
            summary: "Cyberbrain performs no transfer (see the egress register). Disclosure \
                      abroad is otherwise permitted to states on the Federal Council's \
                      adequacy list, which includes the EU/EEA.",
            basis: "FADP Art. 16; Annex 1 Data Protection Ordinance",
            confidence: Confidence::High,
            note: "",
        },
        Obligation {
            topic: Topic::Sanctions,
            summary: "Criminal fines of up to CHF 250,000, imposed on the responsible \
                      NATURAL person, generally on complaint.",
            basis: "FADP Art. 60-63",
            confidence: Confidence::High,
            note: "A different enforcement model from the GDPR's corporate fines.",
        },
        Obligation {
            topic: Topic::AiRegulation,
            summary: "Switzerland has no AI act in force. The Federal Council opted for a \
                      sector-specific approach and ratification of the Council of Europe AI \
                      Convention.",
            basis: "Federal Council decision, February 2025",
            confidence: Confidence::Low,
            note: "Not encoded as an obligation. The model card is still printed because it \
                   costs nothing and a Swiss deployer may face EU customers.",
        },
    ]
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_case_insensitively_and_rejects_junk() {
        assert_eq!(parse_profile("EU").unwrap(), Profile::Eu);
        assert_eq!(parse_profile(" ch ").unwrap(), Profile::Ch);
        assert_eq!(parse_profile("off").unwrap(), Profile::Off);
        assert!(parse_profile("de").is_err());
        assert!(parse_profile("").is_err());
    }

    #[test]
    fn ch_is_not_eu_minus() {
        // The three differences the spec names must actually differ in the data.
        let eu = Profile::Eu.breach_rule();
        let ch = Profile::Ch.breach_rule();
        assert_eq!(eu.deadline, Deadline::Hours(72));
        assert_eq!(ch.deadline, Deadline::AsSoonAsPossible);
        assert_ne!(eu.threshold, ch.threshold);

        assert_ne!(Profile::Eu.erasure_basis(), Profile::Ch.erasure_basis());
        assert_ne!(
            Profile::Eu.register_rule().exemption,
            Profile::Ch.register_rule().exemption
        );
        assert_ne!(
            Profile::Eu.access_response_deadline(),
            Profile::Ch.access_response_deadline()
        );
    }

    #[test]
    fn every_obligation_carries_a_confidence_and_a_basis() {
        for p in ALL_PROFILES {
            for o in p.obligations() {
                assert!(
                    !o.basis.is_empty(),
                    "{}: {:?} has no basis",
                    p.as_str(),
                    o.topic
                );
                assert!(!o.summary.is_empty());
                if o.confidence == Confidence::Low {
                    assert!(
                        !o.note.is_empty(),
                        "{}: {:?} is Low confidence and says nothing about why",
                        p.as_str(),
                        o.topic
                    );
                }
            }
        }
    }

    #[test]
    fn low_confidence_claims_never_drive_behaviour() {
        // Only the typed accessors drive code. They must all be at least Medium.
        for p in [Profile::Eu, Profile::Ch] {
            assert!(p.breach_rule().confidence >= Confidence::Medium);
            assert!(p.register_rule().confidence >= Confidence::Medium);
        }
    }

    #[test]
    fn off_costs_one_comparison() {
        assert!(Profile::Off.is_off());
        assert!(!Profile::Off.holds_pii_writes());
        assert!(Profile::Eu.holds_pii_writes());
        assert!(Profile::Ch.holds_pii_writes());
    }
}
