//! The egress register and the gate (SPEC §12.1).
//!
//! [`Egress`] implements [`cyberbrain_core::EgressGate`]. Every crate that makes a request
//! holds a `dyn EgressGate` and calls `permit(purpose, destination)` first; the binary wires
//! this type in, and `DenyAllEgress` from core is what a path gets until it does.
//!
//! **`permit` decides and records in one call.** There is no separate check and no
//! separate audit; the only way to get `Ok(())` is through a function that has already
//! appended the `egress.permitted` row, and a refusal appends `policy.refusal` before it
//! returns `Err`. If the row cannot be appended the answer is `Err`: no record, no egress.
//!
//! `destination` is what will really be contacted: a full URL, or `host:port`, with the
//! host an IP literal where the caller has already resolved it. A hostname is accepted too;
//! the gate then resolves it itself (through the OS resolver, which the register declares)
//! and records what it resolved to.
//!
//! For this crate's own model-download transport there is a richer form, [`Egress::open`],
//! which returns an [`EgressTicket`]: the same decision-and-record, plus a completion row
//! when the ticket is closed and an `egress.abandoned` row if it is dropped unclosed. The
//! ticket cannot be constructed anywhere else and only covers its own scheme, host and port.
//!
//! **Telemetry does not exist.** The register is an exhaustive `match` over
//! [`EgressPurpose`]: adding a variant to the core enum fails this crate's build until the
//! new purpose is described here, and `register_is_exactly_the_spec_list` fails until
//! someone changes the spec and the test together.
//!
//! The gate is active under every profile including `off`. Local-first is a defining
//! property (SPEC §1), not a compliance option.

#[cfg(feature = "http")]
pub mod transport;

use crate::audit::{Actor, AuditAction, AuditEvent, AuditLog};
use crate::config::PolicyConfig;
use crate::profile::{ALL_PROFILES, Profile};
use cyberbrain_core::{EgressGate, EgressPurpose, Error, Result};
use serde::Serialize;
use serde_json::json;
use std::net::{IpAddr, Ipv4Addr, Ipv6Addr, ToSocketAddrs};
use std::sync::Arc;
use std::time::Instant;
use url::Url;

/// The complete list. If a variant is missing here the exhaustive match in `describe`
/// fails to compile, and the test below fails if this list and the match disagree.
pub const PURPOSES: [EgressPurpose; 7] = [
    EgressPurpose::ModelDownload,
    EgressPurpose::LocalInference,
    EgressPurpose::AuditSync,
    EgressPurpose::NoteSync,
    EgressPurpose::NoteErasure,
    EgressPurpose::HubEnrolment,
    EgressPurpose::Terminal,
];

/// One line of `cyberbrain policy egress`. Plain data so the CLI and the UI print the same
/// thing.
#[derive(Debug, Clone, Serialize)]
pub struct EgressEntry {
    pub purpose: EgressPurpose,
    /// Destination shape, with the currently configured concrete destination if any.
    pub destination: String,
    /// What leaves the machine on this path.
    pub data: &'static str,
    /// Whether note content can be in the request.
    pub carries_note_content: bool,
    /// What has to be true for the path to be usable.
    pub requires: &'static str,
    /// Which profiles permit it. All three: the gate is not a profile feature.
    pub permitted_by: [Profile; 3],
    /// Whether the path can be used right now, given the configuration.
    pub enabled: bool,
    /// Why it is enabled or disabled, in words.
    pub state: String,
}

pub fn purpose_name(p: EgressPurpose) -> &'static str {
    match p {
        EgressPurpose::Terminal => "terminal",
        EgressPurpose::ModelDownload => "model-download",
        EgressPurpose::LocalInference => "local-inference",
        EgressPurpose::AuditSync => "audit-sync",
        EgressPurpose::NoteSync => "note-sync",
        EgressPurpose::NoteErasure => "note-erasure",
        EgressPurpose::HubEnrolment => "hub-enrolment",
    }
}

/// Where TLS trust comes from, in one sentence, for anyone who asks.
///
/// A question a customer asks before signing something, and it should be answerable from the
/// program rather than from a document that can drift — which is exactly what happened once:
/// SPEC §12.1 claimed no trust anchors were available at all, while the client had been
/// using the platform's store all along.
pub const TLS_TRUST: &str = concat!(
    "TLS trust: the operating system's certificate store. No bundle is compiled into this ",
    "binary, so a CA your organisation adds or removes there applies here too. It decides ",
    "which certificate is acceptable for a destination, never which destinations are allowed ",
    "— that is this register."
);

/// The register, computed from the configuration. Exhaustive on purpose.
pub fn register(cfg: &PolicyConfig) -> Vec<EgressEntry> {
    PURPOSES.iter().map(|p| describe(*p, cfg)).collect()
}

fn describe(purpose: EgressPurpose, cfg: &PolicyConfig) -> EgressEntry {
    match purpose {
        // The one entry the gate does not mediate. It is here so the register is true, not
        // so it can be enforced: `permit` is never called for a terminal, because what a
        // program somebody started does is not ours to allow or refuse. Saying that plainly
        // is worth more than a register that is complete only because it left this out.
        EgressPurpose::Terminal => EgressEntry {
            purpose,
            destination: "anywhere the program you started connects to".to_string(),
            data: "whatever you type and whatever that program sends; it can read this store, because you can",
            carries_note_content: true,
            requires: "`cyberbrain serve --terminal`, and the token from the address it prints",
            permitted_by: [Profile::Off, Profile::Eu, Profile::Ch],
            enabled: false,
            state: concat!(
                "not mediated by this gate. A terminal exists only while `serve --terminal` ",
                "is running; what runs in one is yours, in your name, and neither permitted ",
                "nor recorded here. Listed so this register is not read as a complete list ",
                "of what can leave the machine while a shell is one keystroke away."
            )
            .to_string(),
        },
        EgressPurpose::ModelDownload => {
            let (enabled, state) = match (&cfg.model_source, cfg.model_download_consent) {
                (None, _) => (false, "disabled: no model_source configured".to_string()),
                (Some(_), false) => (
                    false,
                    "disabled: model_download_consent is false".to_string(),
                ),
                (Some(src), true) => match Url::parse(src) {
                    Ok(u) if u.scheme() == "https" => {
                        (true, format!("enabled: consent given, source {src}"))
                    }
                    Ok(u) => (
                        false,
                        format!(
                            "disabled: model_source scheme is {} but only https is permitted",
                            u.scheme()
                        ),
                    ),
                    Err(e) => (false, format!("disabled: model_source is not a URL: {e}")),
                },
            };
            EgressEntry {
                purpose,
                destination: format!(
                    "the configured model source over HTTPS ({}), plus HTTPS redirect targets \
                     it returns (one audited hop each); DNS lookup of those hostnames via the \
                     OS resolver",
                    cfg.model_source.as_deref().unwrap_or("none configured")
                ),
                data: "an HTTP GET for a model artefact: the request carries the artefact \
                       path and a user agent, nothing from the store",
                carries_note_content: false,
                requires: "explicit consent, once; the artefact hash is verified after download",
                permitted_by: ALL_PROFILES,
                enabled,
                state,
            }
        }
        EgressPurpose::LocalInference => {
            let (enabled, state) = match Destination::parse(&cfg.inference_endpoint) {
                Err(e) => (
                    false,
                    format!("disabled: inference endpoint is not usable: {e}"),
                ),
                Ok(d) => match d.literal_locality() {
                    Some(Locality::Public) if !cfg.allow_public_endpoint => (
                        false,
                        format!(
                            "disabled: {} is a public address and allow_public_endpoint is false",
                            d.host
                        ),
                    ),
                    Some(Locality::Public) => (
                        true,
                        format!(
                            "enabled WITH allow_public_endpoint: {} is public; every call is a transfer off this machine",
                            cfg.inference_endpoint
                        ),
                    ),
                    Some(Locality::Overlay) if !cfg.allow_overlay_network => (
                        false,
                        format!(
                            "disabled: {} is in 100.64.0.0/10 and allow_overlay_network is false",
                            d.host
                        ),
                    ),
                    Some(Locality::Overlay) => (
                        true,
                        format!(
                            "enabled WITH allow_overlay_network: {} is in 100.64.0.0/10",
                            d.host
                        ),
                    ),
                    Some(Locality::NotUnicast) => (
                        false,
                        format!("disabled: {} is not a unicast address", d.host),
                    ),
                    Some(_) => (
                        true,
                        format!("enabled: {} is loopback or private", cfg.inference_endpoint),
                    ),
                    None => (
                        true,
                        format!(
                            "enabled if {} resolves to loopback or private addresses (checked on every call)",
                            d.host
                        ),
                    ),
                },
            };
            EgressEntry {
                purpose,
                destination: format!(
                    "the configured inference endpoint ({}) on loopback or a private range; \
                     DNS lookup of its hostname via the OS resolver if it is not an IP literal",
                    cfg.inference_endpoint
                ),
                data: "HTTP POST with note text (the blocks being summarised, compared or \
                       classified) and the model name; token counts are audited per call",
                carries_note_content: true,
                requires: "every address the endpoint resolves to is loopback or private-range, \
                           or the operator set allow_overlay_network (100.64/10) or \
                           allow_public_endpoint",
                permitted_by: ALL_PROFILES,
                enabled,
                state,
            }
        }
        EgressPurpose::AuditSync => {
            let (enabled, state) = match &cfg.hub_endpoint {
                None => (
                    false,
                    "disabled: this store is not enrolled with a hub".to_string(),
                ),
                Some(url) => match Destination::parse(url) {
                    Err(e) => (false, format!("disabled: hub endpoint is not usable: {e}")),
                    Ok(d) => match d.literal_locality() {
                        Some(Locality::Public) if !cfg.allow_public_hub => (
                            false,
                            format!(
                                "disabled: {} is a public address and allow_public_hub is false",
                                d.host
                            ),
                        ),
                        Some(Locality::Public) => (
                            true,
                            format!(
                                concat!(
                                    "enabled WITH allow_public_hub: {} is public; ",
                                    "audit rows leave this network"
                                ),
                                url
                            ),
                        ),
                        Some(Locality::Overlay) if !cfg.allow_overlay_network => (
                            false,
                            format!(
                                concat!(
                                    "disabled: {} is in 100.64.0.0/10 and ",
                                    "allow_overlay_network is false"
                                ),
                                d.host
                            ),
                        ),
                        Some(Locality::NotUnicast) => (
                            false,
                            format!("disabled: {} is not a unicast address", d.host),
                        ),
                        _ => (true, format!("enabled: delivering to {url}")),
                    },
                },
            };
            EgressEntry {
                purpose,
                destination: format!(
                    concat!(
                        "the hub this store was enrolled with ({}); DNS lookup of its ",
                        "hostname via the OS resolver if it is not an IP literal"
                    ),
                    cfg.hub_endpoint.as_deref().unwrap_or("none configured")
                ),
                // The distinction this whole path is built around, stated where somebody
                // reading the register will see it.
                data: concat!(
                    "HTTP POST of audit rows: timestamp, actor, action, subject and the ",
                    "chain hashes. What a note said is not in them"
                ),
                carries_note_content: false,
                requires: concat!(
                    "the store was enrolled with a hub, and the hub is loopback or ",
                    "private-range unless allow_public_hub is set"
                ),
                permitted_by: ALL_PROFILES,
                enabled,
                state,
            }
        }
        EgressPurpose::HubEnrolment => EgressEntry {
            purpose,
            destination: concat!(
                "the hub address in the fleet invitation given to `cyberbrain hub enrol`; DNS ",
                "lookup of its hostname via the OS resolver if it is not an IP literal"
            )
            .to_string(),
            data: concat!(
                "HTTP POST of the invitation's code, this machine's name and the project ",
                "folder's name. No note and no audit row"
            ),
            carries_note_content: false,
            requires: concat!(
                "somebody runs `cyberbrain hub enrol` with a fleet invitation; the hub is ",
                "loopback or private-range unless allow_public_hub is set"
            ),
            permitted_by: ALL_PROFILES,
            // Not a standing path: nothing uses it until a person enrols with an invitation,
            // and then once. "Enabled" says it may be used when asked, which is true.
            enabled: true,
            state: concat!(
                "available on request: used once, when a fleet invitation is enrolled, ",
                "never on a timer"
            )
            .to_string(),
        },
        EgressPurpose::NoteErasure => {
            // Only enrolment, on purpose. This is the one hub path that stays open when
            // sharing is switched off: a setting that can leave data somewhere it may no
            // longer be would break Art. 17 by configuration.
            let (enabled, state) = match &cfg.hub_endpoint {
                None => (
                    false,
                    "disabled: this store is not enrolled with a hub".to_string(),
                ),
                Some(url) => match Destination::parse(url) {
                    Err(e) => (false, format!("disabled: hub endpoint is not usable: {e}")),
                    Ok(d) => match d.literal_locality() {
                        Some(Locality::Public) if !cfg.allow_public_hub => (
                            false,
                            format!(
                                "disabled: {} is a public address and allow_public_hub is false",
                                d.host
                            ),
                        ),
                        Some(Locality::NotUnicast) => (
                            false,
                            format!("disabled: {} is not a unicast address", d.host),
                        ),
                        _ => (true, format!("enabled: erasures reach {url}")),
                    },
                },
            };
            EgressEntry {
                purpose,
                destination: format!(
                    "the hub this store was enrolled with ({})",
                    cfg.hub_endpoint.as_deref().unwrap_or("none configured")
                ),
                data: concat!(
                    "HTTP POST of a bereich and a note name, so the hub can remove its ",
                    "copy. The note itself is not in the request"
                ),
                carries_note_content: false,
                requires: concat!(
                    "the store was enrolled with a hub. Deliberately not gated on ",
                    "allow_note_sync: withdrawing what was shared must not depend on ",
                    "sharing still being on"
                ),
                permitted_by: ALL_PROFILES,
                enabled,
                state,
            }
        }
        EgressPurpose::NoteSync => {
            // Stricter than AuditSync by one condition, and that condition is the point:
            // being enrolled with a hub is a decision about evidence, sharing notes is a
            // decision about content. An upgrade must not turn the first into the second.
            let (enabled, state) = match (&cfg.hub_endpoint, cfg.allow_note_sync) {
                (None, _) => (
                    false,
                    "disabled: this store is not enrolled with a hub".to_string(),
                ),
                (Some(_), false) => (
                    false,
                    "disabled: allow_note_sync is false; enrolment alone does not share notes"
                        .to_string(),
                ),
                (Some(url), true) => match Destination::parse(url) {
                    Err(e) => (false, format!("disabled: hub endpoint is not usable: {e}")),
                    Ok(d) => match d.literal_locality() {
                        Some(Locality::Public) if !cfg.allow_public_hub => (
                            false,
                            format!(
                                "disabled: {} is a public address and allow_public_hub is false",
                                d.host
                            ),
                        ),
                        Some(Locality::Public) => (
                            true,
                            format!(
                                concat!(
                                    "enabled WITH allow_public_hub: {} is public; ",
                                    "note content leaves this network"
                                ),
                                url
                            ),
                        ),
                        Some(Locality::Overlay) if !cfg.allow_overlay_network => (
                            false,
                            format!(
                                concat!(
                                    "disabled: {} is in 100.64.0.0/10 and ",
                                    "allow_overlay_network is false"
                                ),
                                d.host
                            ),
                        ),
                        Some(Locality::NotUnicast) => (
                            false,
                            format!("disabled: {} is not a unicast address", d.host),
                        ),
                        _ => (true, format!("enabled: delivering notes to {url}")),
                    },
                },
            };
            EgressEntry {
                purpose,
                destination: format!(
                    concat!(
                        "the hub this store was enrolled with ({}); DNS lookup of its ",
                        "hostname via the OS resolver if it is not an IP literal"
                    ),
                    cfg.hub_endpoint.as_deref().unwrap_or("none configured")
                ),
                data: concat!(
                    "HTTP POST of whole notes: frontmatter and body, for the bereiche this ",
                    "device was granted. Never rings 0 or 1, never a note without a bereich"
                ),
                carries_note_content: true,
                requires: concat!(
                    "the store was enrolled with a hub AND allow_note_sync is set; the hub ",
                    "is loopback or private-range unless allow_public_hub is set"
                ),
                permitted_by: ALL_PROFILES,
                enabled,
                state,
            }
        }
    }
}

/// Where an address sits on the network. Matches SPEC §12.1's definition of "private":
/// loopback, RFC 1918, RFC 4193 unique-local, link-local. `100.64.0.0/10` is its own class.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize)]
#[serde(rename_all = "kebab-case")]
pub enum Locality {
    Loopback,
    /// RFC 1918, RFC 4193 unique-local, link-local (v4 and v6).
    Private,
    /// RFC 6598 shared address space, `100.64.0.0/10`: carrier-grade NAT, but also what
    /// Tailscale-style overlays hand out. Needs `allow_overlay_network`.
    Overlay,
    /// Routable on the public internet. Needs `allow_public_endpoint`.
    Public,
    /// Unspecified, multicast, broadcast, reserved. Never a destination.
    NotUnicast,
}

pub fn locality(ip: IpAddr) -> Locality {
    match ip {
        IpAddr::V4(v4) => locality_v4(v4),
        IpAddr::V6(v6) => locality_v6(v6),
    }
}

fn locality_v4(v4: Ipv4Addr) -> Locality {
    let [a, b, _, _] = v4.octets();
    if v4.is_unspecified() || v4.is_broadcast() || (224..=239).contains(&a) || a >= 240 {
        Locality::NotUnicast
    } else if v4.is_loopback() {
        Locality::Loopback
    } else if v4.is_private() || v4.is_link_local() {
        Locality::Private
    } else if a == 100 && (64..=127).contains(&b) {
        Locality::Overlay
    } else {
        Locality::Public
    }
}

fn locality_v6(v6: Ipv6Addr) -> Locality {
    if let Some(v4) = v6.to_ipv4_mapped() {
        return locality_v4(v4);
    }
    let seg = v6.segments();
    // NAT64 well-known prefix 64:ff9b::/96 embeds an IPv4 address; classify by it.
    if seg[0] == 0x64 && seg[1] == 0xff9b && seg[2..6].iter().all(|s| *s == 0) {
        return locality_v4(Ipv4Addr::new(
            (seg[6] >> 8) as u8,
            seg[6] as u8,
            (seg[7] >> 8) as u8,
            seg[7] as u8,
        ));
    }
    if v6.is_unspecified() || v6.is_multicast() {
        Locality::NotUnicast
    } else if v6.is_loopback() {
        Locality::Loopback
    } else if v6.is_unique_local() || v6.is_unicast_link_local() {
        Locality::Private
    } else {
        Locality::Public
    }
}

/// A parsed, normalised destination. `addrs` is filled by the gate after resolution.
#[derive(Debug, Clone, Serialize)]
pub struct Destination {
    /// `http`, `https`, or `tcp` when the caller gave a bare `host:port`.
    pub scheme: String,
    /// Lower-case, without IPv6 brackets.
    pub host: String,
    pub port: u16,
    /// Path of the URL, without query or fragment. Empty for `host:port`.
    pub path: String,
    pub addrs: Vec<IpAddr>,
}

impl Destination {
    /// Accepts `scheme://host[:port]/path`, `host:port`, `[v6]:port`.
    pub fn parse(s: &str) -> Result<Self> {
        let s = s.trim();
        if s.contains("://") {
            let u = Url::parse(s).map_err(|e| Error::Config(format!("{s:?}: {e}")))?;
            let scheme = u.scheme().to_ascii_lowercase();
            if scheme != "http" && scheme != "https" {
                return Err(Error::Config(format!(
                    "{s:?}: scheme {scheme} is not http or https"
                )));
            }
            if !u.username().is_empty() || u.password().is_some() {
                return Err(Error::Config(format!(
                    "{s:?}: credentials in the URL are not permitted"
                )));
            }
            let host = u
                .host_str()
                .ok_or_else(|| Error::Config(format!("{s:?}: no host")))?
                .trim_matches(|c| c == '[' || c == ']')
                .trim_end_matches('.')
                .to_ascii_lowercase();
            let port = u
                .port_or_known_default()
                .ok_or_else(|| Error::Config(format!("{s:?}: no port")))?;
            return Ok(Self {
                scheme,
                host,
                port,
                path: u.path().to_string(),
                addrs: Vec::new(),
            });
        }
        // host:port forms
        let (host, port) = if let Some(rest) = s.strip_prefix('[') {
            let (h, p) = rest
                .split_once(']')
                .ok_or_else(|| Error::Config(format!("{s:?}: unterminated [")))?;
            let p = p
                .strip_prefix(':')
                .ok_or_else(|| Error::Config(format!("{s:?}: expected [host]:port")))?;
            (h.to_string(), p)
        } else {
            let (h, p) = s
                .rsplit_once(':')
                .ok_or_else(|| Error::Config(format!("{s:?}: expected host:port or a URL")))?;
            if h.contains(':') {
                return Err(Error::Config(format!(
                    "{s:?}: bracket an IPv6 literal as [addr]:port"
                )));
            }
            (h.to_string(), p)
        };
        let port: u16 = port
            .parse()
            .map_err(|_| Error::Config(format!("{s:?}: port {port:?} is not a number")))?;
        if host.is_empty() {
            return Err(Error::Config(format!("{s:?}: no host")));
        }
        Ok(Self {
            scheme: "tcp".into(),
            host: host.to_ascii_lowercase(),
            port,
            path: String::new(),
            addrs: Vec::new(),
        })
    }

    /// Locality without touching the resolver: known for IP literals and `localhost`.
    pub fn literal_locality(&self) -> Option<Locality> {
        if self.host == "localhost" {
            return Some(Locality::Loopback);
        }
        self.host.parse::<IpAddr>().ok().map(locality)
    }

    /// Worst locality across all resolved addresses; `None` if unresolved.
    pub fn resolved_locality(&self) -> Option<Locality> {
        self.addrs.iter().map(|a| locality(*a)).max()
    }

    /// Same scheme, host and port. `tcp` (bare `host:port`) matches either HTTP scheme.
    pub fn same_endpoint(&self, other: &Destination) -> bool {
        let scheme_ok =
            self.scheme == other.scheme || self.scheme == "tcp" || other.scheme == "tcp";
        scheme_ok && self.host == other.host && self.port == other.port
    }

    pub fn origin(&self) -> String {
        format!("{}://{}:{}", self.scheme, self.host, self.port)
    }
}

/// Name resolution seam so the gate can be tested without DNS. The OS resolver is the
/// default and is itself declared in the register text.
pub trait Resolver: Send + Sync {
    fn resolve(&self, host: &str, port: u16) -> std::io::Result<Vec<IpAddr>>;
}

pub struct OsResolver;

impl Resolver for OsResolver {
    fn resolve(&self, host: &str, port: u16) -> std::io::Result<Vec<IpAddr>> {
        if let Ok(ip) = host.parse::<IpAddr>() {
            return Ok(vec![ip]);
        }
        if host == "localhost" {
            return Ok(vec![
                IpAddr::V4(Ipv4Addr::LOCALHOST),
                IpAddr::V6(Ipv6Addr::LOCALHOST),
            ]);
        }
        Ok((host, port).to_socket_addrs()?.map(|a| a.ip()).collect())
    }
}

/// The gate. Cheap to clone. Implements [`EgressGate`].
#[derive(Clone)]
pub struct Egress {
    cfg: Arc<PolicyConfig>,
    audit: AuditLog,
    resolver: Arc<dyn Resolver>,
    /// The actor recorded on rows produced through the trait, which carries no actor of
    /// its own. The binary sets this to whoever is running (cli, hook, mcp, agent).
    actor: Actor,
}

impl Egress {
    pub fn new(cfg: PolicyConfig, audit: AuditLog, actor: Actor) -> Self {
        Self::with_resolver(cfg, audit, actor, Arc::new(OsResolver))
    }

    pub fn with_resolver(
        cfg: PolicyConfig,
        audit: AuditLog,
        actor: Actor,
        resolver: Arc<dyn Resolver>,
    ) -> Self {
        Self {
            cfg: Arc::new(cfg),
            audit,
            resolver,
            actor,
        }
    }

    pub fn config(&self) -> &PolicyConfig {
        &self.cfg
    }

    pub fn audit(&self) -> &AuditLog {
        &self.audit
    }

    pub fn actor(&self) -> &Actor {
        &self.actor
    }

    pub fn register(&self) -> Vec<EgressEntry> {
        register(&self.cfg)
    }

    /// The trait form with an explicit actor. Decides and records in one call.
    pub fn permit_as(
        &self,
        actor: &Actor,
        purpose: EgressPurpose,
        destination: &str,
    ) -> Result<()> {
        self.decide_and_record(actor, purpose, destination, None)
            .map(|_| ())
    }

    /// Decide, record, and hand out a ticket that tracks completion. Used by the
    /// model-download transport in this crate.
    pub fn open(
        &self,
        actor: &Actor,
        purpose: EgressPurpose,
        destination: &str,
    ) -> Result<EgressTicket> {
        let (dest, id) = self.decide_and_record(actor, purpose, destination, None)?;
        Ok(EgressTicket {
            id,
            purpose,
            dest,
            actor: actor.clone(),
            opened: Instant::now(),
            audit: self.audit.clone(),
            closed: false,
        })
    }

    /// A redirect hop. Only `ModelDownload` may follow redirects (a model host handing off
    /// to a CDN); each hop is its own decision and its own audit row, and `via` names the
    /// ticket that received the redirect.
    pub fn open_redirect(&self, from: &EgressTicket, location: &str) -> Result<EgressTicket> {
        let (dest, id) = self.decide_and_record(&from.actor, from.purpose, location, Some(from))?;
        Ok(EgressTicket {
            id,
            purpose: from.purpose,
            dest,
            actor: from.actor.clone(),
            opened: Instant::now(),
            audit: self.audit.clone(),
            closed: false,
        })
    }

    /// The one path to `Ok`. Order is fixed and tested: refusal writes `policy.refusal`
    /// then returns `Err`; permission writes `egress.permitted` then returns `Ok`. If the
    /// permitted row cannot be written, the answer is that error and nothing is permitted.
    fn decide_and_record(
        &self,
        actor: &Actor,
        purpose: EgressPurpose,
        destination: &str,
        via: Option<&EgressTicket>,
    ) -> Result<(Destination, ulid::Ulid)> {
        let mut dest = match Destination::parse(destination) {
            Ok(d) => d,
            Err(e) => {
                return self.refuse(
                    actor,
                    &format!("{}:{destination}", purpose_name(purpose)),
                    purpose,
                    &e.to_string(),
                );
            }
        };
        let subject = format!("{}:{}", purpose_name(purpose), dest.origin());
        let notes = match self.check(purpose, &mut dest, via) {
            Ok(notes) => notes,
            Err(reason) => return self.refuse(actor, &subject, purpose, &reason),
        };
        let id = ulid::Ulid::generate();
        let mut detail = json!({
            "ticket": id.to_string(),
            "purpose": purpose,
            "destination": format!("{}{}", dest.origin(), dest.path),
            "addrs": dest.addrs,
            "locality": dest.resolved_locality(),
            "resolved_by": if dest.literal_locality().is_some() { "caller (literal)" } else { "gate (OS resolver)" },
            "profile": self.cfg.profile,
            "notes": notes,
        });
        if let Some(v) = via {
            detail["via"] = json!(v.id.to_string());
        }
        self.audit
            .record(actor, AuditAction::EgressPermitted, subject, detail)?;
        Ok((dest, id))
    }

    fn refuse<T>(
        &self,
        actor: &Actor,
        subject: &str,
        purpose: EgressPurpose,
        reason: &str,
    ) -> Result<T> {
        // Best-effort on purpose: a refusal must not turn into a different error just
        // because the log is unavailable, and nothing left the machine either way.
        let _ = self.audit.record(
            actor,
            AuditAction::PolicyRefusal,
            subject,
            json!({ "purpose": purpose, "reason": reason, "profile": self.cfg.profile }),
        );
        Err(Error::PolicyRefusal {
            profile: self.cfg.profile.as_str().to_string(),
            reason: reason.to_string(),
        })
    }

    /// The rules. Returns notes for the audit row, or the refusal reason.
    fn check(
        &self,
        purpose: EgressPurpose,
        dest: &mut Destination,
        via: Option<&EgressTicket>,
    ) -> std::result::Result<Vec<String>, String> {
        let mut notes = Vec::new();
        if let Some(v) = via
            && v.purpose != purpose
        {
            return Err(format!(
                "a {} ticket cannot be redirected into a {} request",
                purpose_name(v.purpose),
                purpose_name(purpose)
            ));
        }
        match purpose {
            // The gate is never asked about a terminal; if it ever is, that is a mistake in
            // the caller and not a decision to make quietly.
            EgressPurpose::Terminal => {
                return Err(
                    "the terminal is not a gated path: nothing may take a ticket for it".into(),
                );
            }
            EgressPurpose::ModelDownload => {
                if !self.cfg.model_download_consent {
                    return Err(
                        "model download requires consent; none has been given for this run".into(),
                    );
                }
                if dest.scheme != "https" {
                    return Err(format!(
                        "model downloads go over https only, not {}",
                        dest.scheme
                    ));
                }
                let Some(src) = &self.cfg.model_source else {
                    return Err("no model_source is configured".into());
                };
                let source = Destination::parse(src)
                    .map_err(|e| format!("configured model_source is unusable: {e}"))?;
                match via {
                    None if !dest.same_endpoint(&source) => {
                        return Err(format!(
                            "{} is not the configured model source {}",
                            dest.origin(),
                            source.origin()
                        ));
                    }
                    None => {}
                    Some(v) => notes.push(format!("redirect hop from ticket {}", v.id)),
                }
            }
            EgressPurpose::LocalInference => {
                if via.is_some() {
                    return Err("inference requests do not follow redirects".into());
                }
            }
            EgressPurpose::HubEnrolment => {
                if via.is_some() {
                    return Err("enrolment requests do not follow redirects".into());
                }
                // The gate for this one call is built with the invitation's address as its
                // hub, because the store has no hub of its own yet. From there the rule is the
                // delivery rule: that address and no other.
                let Some(hub) = &self.cfg.hub_endpoint else {
                    return Err("there is no hub address to enrol with".into());
                };
                let named = Destination::parse(hub)
                    .map_err(|e| format!("the invitation's hub address is unusable: {e}"))?;
                if !dest.same_endpoint(&named) {
                    return Err(format!(
                        "{} is not the hub named in the invitation ({})",
                        dest.origin(),
                        named.origin()
                    ));
                }
            }
            EgressPurpose::NoteErasure => {
                if via.is_some() {
                    return Err("erasure requests do not follow redirects".into());
                }
                let Some(hub) = &self.cfg.hub_endpoint else {
                    return Err("this store is not enrolled with a hub".into());
                };
                let configured = Destination::parse(hub)
                    .map_err(|e| format!("configured hub endpoint is unusable: {e}"))?;
                if !dest.same_endpoint(&configured) {
                    return Err(format!(
                        "{} is not the hub this store is enrolled with ({})",
                        dest.origin(),
                        configured.origin()
                    ));
                }
            }
            EgressPurpose::NoteSync => {
                if via.is_some() {
                    return Err("note deliveries do not follow redirects".into());
                }
                // Checked here and not only in the register, because the register describes
                // and this decides. A setting that only shows up in a description is the
                // kind of guard that reads like one and permits everything.
                if !self.cfg.allow_note_sync {
                    return Err(
                        "allow_note_sync is false: this store shares audit rows, not notes".into(),
                    );
                }
                let Some(hub) = &self.cfg.hub_endpoint else {
                    return Err("this store is not enrolled with a hub".into());
                };
                let configured = Destination::parse(hub)
                    .map_err(|e| format!("configured hub endpoint is unusable: {e}"))?;
                if !dest.same_endpoint(&configured) {
                    return Err(format!(
                        "{} is not the hub this store is enrolled with ({})",
                        dest.origin(),
                        configured.origin()
                    ));
                }
            }
            EgressPurpose::AuditSync => {
                if via.is_some() {
                    return Err("audit deliveries do not follow redirects".into());
                }
                let Some(hub) = &self.cfg.hub_endpoint else {
                    return Err("this store is not enrolled with a hub".into());
                };
                // The destination has to be the hub this store enrolled with, not merely
                // some address that passes the locality test. Otherwise the ticket would
                // permit delivering the audit trail to whatever host a config edit named.
                let configured = Destination::parse(hub)
                    .map_err(|e| format!("configured hub endpoint is unusable: {e}"))?;
                if !dest.same_endpoint(&configured) {
                    return Err(format!(
                        "{} is not the hub this store is enrolled with ({})",
                        dest.origin(),
                        configured.origin()
                    ));
                }
            }
        }

        dest.addrs = self
            .resolver
            .resolve(&dest.host, dest.port)
            .map_err(|e| format!("{} does not resolve: {e}", dest.host))?;
        if dest.addrs.is_empty() {
            return Err(format!(
                "{} resolved to no addresses; refusing to guess",
                dest.host
            ));
        }

        // The same locality rule for both outbound paths, with the setting that relaxes it
        // named per purpose: one says "note text may go to a public endpoint", the other
        // "audit rows may leave this network". They are different decisions.
        if matches!(
            purpose,
            EgressPurpose::LocalInference | EgressPurpose::AuditSync | EgressPurpose::HubEnrolment
        ) {
            let allow_public = match purpose {
                EgressPurpose::AuditSync | EgressPurpose::HubEnrolment => self.cfg.allow_public_hub,
                _ => self.cfg.allow_public_endpoint,
            };
            for a in &dest.addrs {
                match locality(*a) {
                    Locality::Loopback | Locality::Private => {}
                    Locality::Overlay if self.cfg.allow_overlay_network => {
                        notes.push(format!(
                            "{a} is in 100.64.0.0/10, permitted by allow_overlay_network"
                        ));
                    }
                    Locality::Overlay => {
                        return Err(format!(
                            "{} resolves to {a}, which is in 100.64.0.0/10 (carrier-grade NAT or an overlay \
                             such as Tailscale). Set allow_overlay_network only if that machine is yours",
                            dest.host
                        ));
                    }
                    Locality::Public if allow_public => {
                        notes.push(format!(
                            "{a} is PUBLIC, permitted by the setting for {}; this call is a transfer off this machine",
                            purpose_name(purpose)
                        ));
                    }
                    Locality::Public => {
                        return Err(format!(
                            "{} resolves to {a}, a public address; the inference endpoint must be loopback or \
                             private-range unless allow_public_endpoint is set",
                            dest.host
                        ));
                    }
                    Locality::NotUnicast => {
                        return Err(format!(
                            "{} resolves to {a}, which is not a unicast address",
                            dest.host
                        ));
                    }
                }
            }
        }
        Ok(notes)
    }
}

impl EgressGate for Egress {
    fn permit(&self, purpose: EgressPurpose, destination: &str) -> Result<()> {
        self.permit_as(&self.actor, purpose, destination)
    }
}

/// What happened on the wire, for the completion row of a ticket.
#[derive(Debug, Clone, Serialize)]
pub struct Outcome {
    pub ok: bool,
    pub status: Option<u16>,
    pub bytes_out: u64,
    pub bytes_in: u64,
    pub error: Option<String>,
}

impl Outcome {
    pub fn ok(status: Option<u16>) -> Self {
        Self {
            ok: true,
            status,
            bytes_out: 0,
            bytes_in: 0,
            error: None,
        }
    }
    pub fn failed(error: &str) -> Self {
        Self {
            ok: false,
            status: None,
            bytes_out: 0,
            bytes_in: 0,
            error: Some(error.to_string()),
        }
    }
    pub fn bytes(mut self, out: u64, r#in: u64) -> Self {
        self.bytes_out = out;
        self.bytes_in = r#in;
        self
    }
}

/// Proof that the gate ran, for this crate's own transport. Cannot be forged: private
/// fields, no constructor, no `Clone`, no `Default`.
///
/// ```compile_fail
/// let t = cyberbrain_policy::egress::EgressTicket { id: todo!(), purpose: todo!() };
/// ```
pub struct EgressTicket {
    id: ulid::Ulid,
    purpose: EgressPurpose,
    dest: Destination,
    actor: Actor,
    opened: Instant,
    audit: AuditLog,
    closed: bool,
}

impl EgressTicket {
    pub fn id(&self) -> ulid::Ulid {
        self.id
    }

    pub fn purpose(&self) -> EgressPurpose {
        self.purpose
    }

    pub fn destination(&self) -> &Destination {
        &self.dest
    }

    /// Whether a concrete request URL is covered by this ticket: same scheme, host and port.
    /// Path and query are the caller's business; the host is not.
    pub fn permits(&self, url: &Url) -> bool {
        Destination::parse(url.as_str()).is_ok_and(|d| d.same_endpoint(&self.dest))
    }

    /// Record the completion and consume the ticket.
    pub fn close(mut self, outcome: Outcome) -> Result<AuditEvent> {
        self.closed = true;
        let action = if outcome.ok {
            AuditAction::EgressCompleted
        } else {
            AuditAction::EgressFailed
        };
        self.audit.record(
            &self.actor,
            action,
            self.subject(),
            json!({
                "ticket": self.id.to_string(),
                "purpose": self.purpose,
                "status": outcome.status,
                "bytes_out": outcome.bytes_out,
                "bytes_in": outcome.bytes_in,
                "elapsed_ms": self.opened.elapsed().as_millis() as u64,
                "error": outcome.error,
            }),
        )
    }

    fn subject(&self) -> String {
        format!("{}:{}", purpose_name(self.purpose), self.dest.origin())
    }

    /// Record a refusal against this ticket (used by the transport when a URL does not
    /// match the ticket).
    pub(crate) fn refuse(&self, reason: &str) -> Error {
        let _ = self.audit.record(
            &self.actor,
            AuditAction::PolicyRefusal,
            self.subject(),
            json!({ "ticket": self.id.to_string(), "purpose": self.purpose, "reason": reason }),
        );
        Error::PolicyRefusal {
            profile: "egress".into(),
            reason: reason.to_string(),
        }
    }
}

impl Drop for EgressTicket {
    fn drop(&mut self) {
        if !self.closed {
            let _ = self.audit.record(
                &self.actor,
                AuditAction::EgressAbandoned,
                self.subject(),
                json!({
                    "ticket": self.id.to_string(),
                    "purpose": self.purpose,
                    "elapsed_ms": self.opened.elapsed().as_millis() as u64,
                    "note": "ticket dropped without close(); whether bytes went out is unknown",
                }),
            );
        }
    }
}

impl std::fmt::Debug for EgressTicket {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("EgressTicket")
            .field("id", &self.id)
            .field("purpose", &self.purpose)
            .field("dest", &self.dest.origin())
            .finish()
    }
}

/// Render the register for a terminal.
pub fn render_register(entries: &[EgressEntry]) -> String {
    let mut s = String::from("Egress register (SPEC §12.1). This is the complete list.\n");
    for e in entries {
        s.push_str(&format!(
            "\n{}  [{}]\n  destination: {}\n  data: {}\n  note content: {}\n  requires: {}\n  permitted by: {}\n  state: {}\n",
            purpose_name(e.purpose),
            if e.enabled { "ENABLED" } else { "disabled" },
            e.destination,
            e.data,
            if e.carries_note_content { "yes" } else { "no" },
            e.requires,
            e.permitted_by.iter().map(|p| p.as_str()).collect::<Vec<_>>().join(", "),
            e.state,
        ));
    }
    s.push_str("\nTelemetry: none. There is no such purpose and no way to add one without changing the core enum and this register together.\n");
    s
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::audit::{AuditFilter, MemoryAuditSink};
    use std::sync::Mutex;

    /// A resolver that answers from a table and never touches DNS.
    struct TableResolver(Mutex<Vec<(String, Vec<IpAddr>)>>);
    impl TableResolver {
        fn new(entries: &[(&str, &[&str])]) -> Arc<Self> {
            Arc::new(Self(Mutex::new(
                entries
                    .iter()
                    .map(|(h, ips)| {
                        (
                            h.to_string(),
                            ips.iter().map(|s| s.parse().unwrap()).collect(),
                        )
                    })
                    .collect(),
            )))
        }
    }
    impl Resolver for TableResolver {
        fn resolve(&self, host: &str, _port: u16) -> std::io::Result<Vec<IpAddr>> {
            if let Ok(ip) = host.parse::<IpAddr>() {
                return Ok(vec![ip]);
            }
            self.0
                .lock()
                .unwrap()
                .iter()
                .find(|(h, _)| h == host)
                .map(|(_, ips)| ips.clone())
                .ok_or_else(|| std::io::Error::other("no such host"))
        }
    }

    fn cfg() -> PolicyConfig {
        PolicyConfig {
            model_source: Some("https://models.example.org/".into()),
            model_download_consent: true,
            ..Default::default()
        }
    }

    fn gate(cfg: PolicyConfig) -> (Egress, Arc<MemoryAuditSink>) {
        let (log, sink) = AuditLog::in_memory();
        let r = TableResolver::new(&[
            ("ollama.lan", &["192.168.1.20"]),
            ("models.example.org", &["203.0.113.10"]),
            ("cdn.example.org", &["203.0.113.11"]),
            ("evil.example.org", &["198.51.100.5"]),
            ("mixed.lan", &["10.0.0.5", "203.0.113.9"]),
            ("tail.lan", &["100.100.1.1"]),
            ("ghost.lan", &[]),
        ]);
        (Egress::with_resolver(cfg, log, Actor::Cli, r), sink)
    }

    const LOCAL: &str = "http://127.0.0.1:11434/v1/chat/completions";

    /// Enrolling with a fleet invitation reaches the address in the invitation and no other,
    /// keeps the delivery rule for public addresses, and has nowhere to go without an address.
    #[test]
    fn enrolment_reaches_only_the_hub_the_invitation_names() {
        let named = PolicyConfig {
            hub_endpoint: Some("http://192.168.1.30:7788".into()),
            ..cfg()
        };
        let (g, _) = gate(named);
        assert!(
            g.open(
                &Actor::Cli,
                EgressPurpose::HubEnrolment,
                "http://192.168.1.30:7788/api/v1/enrol"
            )
            .is_ok()
        );
        let err = g
            .open(
                &Actor::Cli,
                EgressPurpose::HubEnrolment,
                "http://ollama.lan:7788/api/v1/enrol",
            )
            .unwrap_err()
            .to_string();
        assert!(err.contains("not the hub named in the invitation"), "{err}");

        let public = PolicyConfig {
            hub_endpoint: Some("https://evil.example.org".into()),
            ..cfg()
        };
        let (g, _) = gate(public.clone());
        let err = g
            .open(
                &Actor::Cli,
                EgressPurpose::HubEnrolment,
                "https://evil.example.org/api/v1/enrol",
            )
            .unwrap_err()
            .to_string();
        assert!(err.contains("public address"), "{err}");
        let (g, _) = gate(PolicyConfig {
            allow_public_hub: true,
            ..public
        });
        assert!(
            g.open(
                &Actor::Cli,
                EgressPurpose::HubEnrolment,
                "https://evil.example.org/api/v1/enrol"
            )
            .is_ok()
        );

        let (g, _) = gate(cfg());
        let err = g
            .open(
                &Actor::Cli,
                EgressPurpose::HubEnrolment,
                "http://192.168.1.30:7788/api/v1/enrol",
            )
            .unwrap_err()
            .to_string();
        assert!(err.contains("no hub address"), "{err}");
    }

    // ---- the register ----

    #[test]
    fn register_is_exactly_the_spec_list() {
        let reg = register(&PolicyConfig::default());
        let names: Vec<&str> = reg.iter().map(|e| purpose_name(e.purpose)).collect();
        assert_eq!(
            names,
            [
                "model-download",
                "local-inference",
                "audit-sync",
                "note-sync",
                "note-erasure",
                "hub-enrolment",
                "terminal"
            ],
            "the register is the whole list; adding a purpose is a decision, not a detail"
        );
        for e in &reg {
            // Exhaustive: a new core variant stops this compiling.
            match e.purpose {
                EgressPurpose::ModelDownload
                | EgressPurpose::LocalInference
                | EgressPurpose::AuditSync
                | EgressPurpose::NoteSync
                | EgressPurpose::NoteErasure
                | EgressPurpose::HubEnrolment
                | EgressPurpose::Terminal => {}
            }
        }
        // Audit sync is off until a store is enrolled — a path that exists is not a path
        // that is open. (Local inference is "enabled" by default in the sense that loopback
        // is permitted; whether anything answers there is a different question.)
        let sync = reg
            .iter()
            .find(|e| e.purpose == EgressPurpose::AuditSync)
            .expect("audit sync is in the register");
        assert!(!sync.enabled, "not enrolled means nothing is sent");
        assert!(!sync.carries_note_content, "rows only, never note text");

        // The terminal is in the register precisely because it is not gated. Both halves
        // matter: it must be listed, and it must not read as something this program permits.
        let term = reg
            .iter()
            .find(|e| e.purpose == EgressPurpose::Terminal)
            .expect("the terminal is in the register");
        assert!(
            !term.enabled,
            "a path this gate does not mediate is not `enabled`"
        );
        assert!(
            term.state.contains("not mediated"),
            "the state has to say so in words, not leave it to be inferred: {}",
            term.state
        );
        assert!(
            term.carries_note_content,
            "somebody in a shell can read this store, and the register must not suggest otherwise"
        );
    }

    #[test]
    fn nothing_in_the_register_is_telemetry() {
        for e in register(&cfg()) {
            let blob = format!("{} {} {}", e.destination, e.data, e.requires).to_lowercase();
            for word in ["telemetry", "analytics", "crash report", "usage statistics"] {
                assert!(!blob.contains(word), "{:?} mentions {word}", e.purpose);
            }
        }
    }

    #[test]
    fn register_reflects_config_state() {
        let reg = register(&PolicyConfig::default());
        assert!(!reg[0].enabled, "no source, no consent");
        assert!(reg[0].state.contains("no model_source"));
        assert!(reg[1].enabled, "default endpoint is loopback");

        assert!(register(&cfg())[0].enabled);

        let c = PolicyConfig {
            inference_endpoint: "http://203.0.113.1:8000/v1".into(),
            ..cfg()
        };
        assert!(!register(&c)[1].enabled);
        let reg = register(&PolicyConfig {
            allow_public_endpoint: true,
            ..c
        });
        assert!(reg[1].enabled && reg[1].state.contains("allow_public_endpoint"));

        let c = PolicyConfig {
            inference_endpoint: "http://100.100.1.1:8000/v1".into(),
            ..cfg()
        };
        assert!(!register(&c)[1].enabled);
        assert!(register(&c)[1].state.contains("allow_overlay_network"));
        assert!(
            register(&PolicyConfig {
                allow_overlay_network: true,
                ..c
            })[1]
                .enabled
        );

        let c = PolicyConfig {
            model_source: Some("http://models.example.org/".into()),
            ..cfg()
        };
        assert!(
            !register(&c)[0].enabled,
            "plaintext model source is not enabled"
        );
    }

    /// The reason `NoteSync` is a separate purpose with its own setting. A store enrolled
    /// with a hub delivers audit rows; it must not begin delivering note content because a
    /// new version knows how to. Calibrated: flip `allow_note_sync` and it turns on.
    #[test]
    fn enrolment_alone_does_not_share_notes() {
        let enrolled = PolicyConfig {
            hub_endpoint: Some("https://hub.example.internal/".into()),
            ..cfg()
        };
        let reg = register(&enrolled);
        let note_sync = reg
            .iter()
            .find(|e| e.purpose == EgressPurpose::NoteSync)
            .expect("NoteSync is in the register");
        assert!(
            !note_sync.enabled,
            "a store enrolled for audit must not share notes: {}",
            note_sync.state
        );
        assert!(note_sync.state.contains("allow_note_sync"));

        // Audit rows, on the same config, do travel. The two are independent.
        let audit = reg
            .iter()
            .find(|e| e.purpose == EgressPurpose::AuditSync)
            .unwrap();
        assert!(audit.enabled, "audit sync should be on: {}", audit.state);

        // And with the setting, note sync comes on.
        let sharing = PolicyConfig {
            allow_note_sync: true,
            ..enrolled
        };
        let on = register(&sharing)
            .into_iter()
            .find(|e| e.purpose == EgressPurpose::NoteSync)
            .unwrap();
        assert!(on.enabled, "{}", on.state);
    }

    /// Art. 17 must not depend on a sharing switch. A store that shared notes and then
    /// turned sharing off can still ask the hub to erase what it sent.
    #[test]
    fn erasure_survives_sharing_being_turned_off() {
        let shared_then_stopped = PolicyConfig {
            hub_endpoint: Some("https://hub.example.internal/".into()),
            allow_note_sync: false,
            ..cfg()
        };
        let reg = register(&shared_then_stopped);
        let by = |p: EgressPurpose| reg.iter().find(|e| e.purpose == p).unwrap();
        assert!(
            !by(EgressPurpose::NoteSync).enabled,
            "sharing is off, as configured"
        );
        assert!(
            by(EgressPurpose::NoteErasure).enabled,
            "but withdrawing must still work: {}",
            by(EgressPurpose::NoteErasure).state
        );
        assert!(!by(EgressPurpose::NoteErasure).carries_note_content);
    }

    /// The register's own claim about each path. `carries_note_content` is what a reader
    /// checks first, so the two hub paths must not agree about it.
    #[test]
    fn only_note_sync_admits_to_carrying_notes_to_the_hub() {
        let reg = register(&PolicyConfig {
            hub_endpoint: Some("https://hub.example.internal/".into()),
            allow_note_sync: true,
            ..cfg()
        });
        let by = |p: EgressPurpose| reg.iter().find(|e| e.purpose == p).unwrap();
        assert!(!by(EgressPurpose::AuditSync).carries_note_content);
        assert!(by(EgressPurpose::NoteSync).carries_note_content);
        // The old promise still reads true of the path it was made about.
        assert!(
            by(EgressPurpose::AuditSync)
                .data
                .contains("What a note said is not in them")
        );
    }

    #[test]
    fn register_is_the_same_under_every_profile() {
        for p in ALL_PROFILES {
            let reg = register(&PolicyConfig {
                profile: p,
                ..cfg()
            });
            assert!(reg[0].enabled && reg[1].enabled, "{}", p.as_str());
        }
    }

    // ---- locality ----

    #[test]
    fn locality_matches_the_spec_definition() {
        let l = |s: &str| locality(s.parse().unwrap());
        assert_eq!(l("127.0.0.1"), Locality::Loopback);
        assert_eq!(l("::1"), Locality::Loopback);
        assert_eq!(l("10.1.2.3"), Locality::Private);
        assert_eq!(l("172.16.0.1"), Locality::Private);
        assert_eq!(l("172.32.0.1"), Locality::Public);
        assert_eq!(l("192.168.0.1"), Locality::Private);
        assert_eq!(l("169.254.1.1"), Locality::Private);
        assert_eq!(l("fd12::1"), Locality::Private);
        assert_eq!(l("fe80::1"), Locality::Private);
        assert_eq!(
            l("100.64.0.1"),
            Locality::Overlay,
            "CGNAT/Tailscale is its own class, not private"
        );
        assert_eq!(l("100.127.255.255"), Locality::Overlay);
        assert_eq!(l("100.128.0.1"), Locality::Public);
        assert_eq!(l("2001:db8::1"), Locality::Public);
        assert_eq!(l("::ffff:192.168.1.1"), Locality::Private);
        assert_eq!(l("::ffff:8.8.8.8"), Locality::Public);
        assert_eq!(l("64:ff9b::192.168.0.1"), Locality::Private);
        assert_eq!(l("8.8.8.8"), Locality::Public);
        assert_eq!(l("0.0.0.0"), Locality::NotUnicast);
        assert_eq!(l("224.0.0.1"), Locality::NotUnicast);
        assert_eq!(l("255.255.255.255"), Locality::NotUnicast);
        assert_eq!(l("ff02::1"), Locality::NotUnicast);
        assert_eq!(l("::"), Locality::NotUnicast);
    }

    #[test]
    fn destination_parses_urls_and_host_port() {
        let d = Destination::parse("http://Ollama.LAN:11434/v1/x?y=1").unwrap();
        assert_eq!(
            (d.scheme.as_str(), d.host.as_str(), d.port, d.path.as_str()),
            ("http", "ollama.lan", 11434, "/v1/x")
        );
        let d = Destination::parse("https://models.example.org/m.bin").unwrap();
        assert_eq!(d.port, 443);
        let d = Destination::parse("10.0.0.5:8000").unwrap();
        assert_eq!(
            (d.scheme.as_str(), d.host.as_str(), d.port),
            ("tcp", "10.0.0.5", 8000)
        );
        let d = Destination::parse("[fd00::1]:8000").unwrap();
        assert_eq!((d.host.as_str(), d.port), ("fd00::1", 8000));
        for bad in [
            "fd00::1:8000",
            "nohost",
            "ftp://x/",
            "http://u:p@h/",
            ":80",
            "h:port",
        ] {
            assert!(Destination::parse(bad).is_err(), "{bad}");
        }
    }

    // ---- the gate: the regression tests the task asked for ----

    /// The trait's contract. Would fail if `permit` returned `Ok` without the row, or
    /// wrote the row after returning.
    #[test]
    fn permit_records_before_it_answers() {
        let (g, sink) = gate(cfg());
        let gate: &dyn EgressGate = &g;
        gate.permit(EgressPurpose::LocalInference, LOCAL).unwrap();
        assert_eq!(sink.actions(), ["egress.permitted"]);
        let row = &sink.rows()[0];
        assert_eq!(row.actor, "cli");
        assert_eq!(row.subject, "local-inference:http://127.0.0.1:11434");
        assert_eq!(row.detail["addrs"][0], "127.0.0.1");
        assert_eq!(row.detail["locality"], "loopback");
        assert_eq!(
            row.detail["destination"],
            "http://127.0.0.1:11434/v1/chat/completions"
        );
    }

    /// Would fail if a refusal came back without its row, or as anything but exit code 3.
    #[test]
    fn a_refusal_is_recorded_and_is_exit_code_3() {
        let (g, sink) = gate(PolicyConfig::default()); // no consent, no source
        let err = g
            .permit(
                EgressPurpose::ModelDownload,
                "https://models.example.org/x.safetensors",
            )
            .unwrap_err();
        assert!(matches!(err, Error::PolicyRefusal { .. }), "{err}");
        assert_eq!(err.exit_code(), 3);
        assert_eq!(sink.actions(), ["policy.refusal"]);
        assert!(
            sink.rows()[0].detail["reason"]
                .as_str()
                .unwrap()
                .contains("consent")
        );
    }

    /// Would fail if the gate said `Ok` when it could not write the row.
    #[test]
    fn no_record_means_no_permission() {
        let (g, sink) = gate(cfg());
        sink.fail_next_append();
        let r = g.permit(EgressPurpose::LocalInference, LOCAL);
        assert!(r.is_err(), "fail closed");
        assert!(sink.is_empty());
        // And the gate is not wedged afterwards.
        g.permit(EgressPurpose::LocalInference, LOCAL).unwrap();
        assert_eq!(sink.actions(), ["egress.permitted"]);
    }

    #[test]
    fn a_ticket_records_completion_and_abandonment() {
        let (g, sink) = gate(cfg());
        let t = g
            .open(&Actor::Operator, EgressPurpose::LocalInference, LOCAL)
            .unwrap();
        t.close(Outcome::ok(Some(200)).bytes(10, 20)).unwrap();
        {
            let _t = g
                .open(&Actor::Operator, EgressPurpose::LocalInference, LOCAL)
                .unwrap();
        }
        assert_eq!(
            sink.actions(),
            [
                "egress.permitted",
                "egress.completed",
                "egress.permitted",
                "egress.abandoned"
            ]
        );
        assert_eq!(sink.rows()[1].detail["bytes_in"], 20);
        assert_eq!(sink.rows()[0].actor, "operator");
    }

    #[test]
    fn inference_refuses_public_unless_allowed_and_names_the_offender() {
        let (g, sink) = gate(cfg());
        let err = g
            .permit(EgressPurpose::LocalInference, "http://mixed.lan:8000/v1")
            .unwrap_err();
        assert!(err.to_string().contains("203.0.113.9"), "{err}");
        assert!(err.to_string().contains("allow_public_endpoint"), "{err}");
        assert_eq!(sink.actions(), ["policy.refusal"]);

        let (g, sink) = gate(PolicyConfig {
            allow_public_endpoint: true,
            ..cfg()
        });
        g.permit(EgressPurpose::LocalInference, "http://mixed.lan:8000/v1")
            .unwrap();
        let notes = sink.rows()[0].detail["notes"].to_string();
        assert!(
            notes.contains("PUBLIC") && notes.contains("transfer"),
            "{notes}"
        );
    }

    #[test]
    fn overlay_range_has_its_own_switch() {
        let (g, _) = gate(cfg());
        let err = g
            .permit(EgressPurpose::LocalInference, "http://tail.lan:11434/v1")
            .unwrap_err();
        assert!(err.to_string().contains("allow_overlay_network"), "{err}");
        assert!(
            !err.to_string().contains("allow_public_endpoint"),
            "must not point at the wrong switch: {err}"
        );
        // allow_public_endpoint does NOT unlock the overlay range.
        let (g, _) = gate(PolicyConfig {
            allow_public_endpoint: true,
            ..cfg()
        });
        assert!(
            g.permit(EgressPurpose::LocalInference, "http://tail.lan:11434/v1")
                .is_err()
        );
        let (g, sink) = gate(PolicyConfig {
            allow_overlay_network: true,
            ..cfg()
        });
        g.permit(EgressPurpose::LocalInference, "http://tail.lan:11434/v1")
            .unwrap();
        assert!(
            sink.rows()[0].detail["notes"]
                .to_string()
                .contains("allow_overlay_network")
        );
        // A literal in that range, passed already-resolved, is judged the same way.
        assert!(
            g.permit(EgressPurpose::LocalInference, "100.100.1.1:11434")
                .is_ok()
        );
    }

    #[test]
    fn empty_and_failed_resolution_fail_closed() {
        let (g, _) = gate(cfg());
        assert!(
            g.permit(EgressPurpose::LocalInference, "http://ghost.lan:8000/v1")
                .is_err()
        );
        assert!(
            g.permit(EgressPurpose::LocalInference, "http://nxdomain.lan:8000/v1")
                .is_err()
        );
        assert!(
            g.permit(EgressPurpose::LocalInference, "http://0.0.0.0:8000/v1")
                .is_err()
        );
        assert!(
            g.permit(EgressPurpose::LocalInference, "http://224.0.0.1:8000/v1")
                .is_err()
        );
    }

    #[test]
    fn already_resolved_destinations_are_accepted_in_host_port_form() {
        let (g, sink) = gate(cfg());
        g.permit(EgressPurpose::LocalInference, "192.168.1.20:11434")
            .unwrap();
        g.permit(EgressPurpose::LocalInference, "[fd00::1]:8000")
            .unwrap();
        assert_eq!(sink.rows()[0].detail["resolved_by"], "caller (literal)");
        assert!(
            g.permit(EgressPurpose::LocalInference, "203.0.113.5:11434")
                .is_err()
        );
    }

    #[test]
    fn model_download_only_from_the_source_over_https() {
        let (g, _) = gate(cfg());
        assert!(
            g.permit(
                EgressPurpose::ModelDownload,
                "https://evil.example.org/m.bin"
            )
            .is_err()
        );
        assert!(
            g.permit(
                EgressPurpose::ModelDownload,
                "http://models.example.org/m.bin"
            )
            .is_err()
        );
        assert!(
            g.permit(EgressPurpose::ModelDownload, "models.example.org:443")
                .is_err(),
            "a download needs a URL"
        );
        g.permit(
            EgressPurpose::ModelDownload,
            "https://models.example.org/m.bin?x=1",
        )
        .unwrap();
    }

    #[test]
    fn redirects_are_separate_audited_hops_and_https_only() {
        let (g, sink) = gate(cfg());
        let first = g
            .open(
                &Actor::Operator,
                EgressPurpose::ModelDownload,
                "https://models.example.org/m.bin",
            )
            .unwrap();
        let hop = g
            .open_redirect(&first, "https://cdn.example.org/blob/abc")
            .unwrap();
        assert_eq!(sink.rows()[1].detail["via"], first.id().to_string());
        assert!(
            g.open_redirect(&first, "http://cdn.example.org/blob/abc")
                .is_err()
        );
        first.close(Outcome::ok(Some(302))).unwrap();
        hop.close(Outcome::ok(Some(200))).unwrap();
        let t = g
            .open(&Actor::Cli, EgressPurpose::LocalInference, LOCAL)
            .unwrap();
        assert!(
            g.open_redirect(&t, "http://127.0.0.1:11434/v1/y").is_err(),
            "inference never follows redirects"
        );
        t.close(Outcome::ok(Some(200))).unwrap();
    }

    #[test]
    fn a_ticket_permits_only_its_own_endpoint() {
        let (g, _) = gate(cfg());
        let t = g
            .open(&Actor::Cli, EgressPurpose::LocalInference, LOCAL)
            .unwrap();
        assert!(t.permits(&Url::parse("http://127.0.0.1:11434/v1/other/path?q=1").unwrap()));
        assert!(!t.permits(&Url::parse("http://127.0.0.1:11435/v1/x").unwrap()));
        assert!(!t.permits(&Url::parse("https://127.0.0.1:11434/v1/x").unwrap()));
        assert!(!t.permits(&Url::parse("http://10.0.0.1:11434/v1/x").unwrap()));
        t.close(Outcome::ok(Some(200))).unwrap();
    }

    #[test]
    fn the_audit_row_never_carries_the_query_string() {
        let (g, sink) = gate(cfg());
        g.permit(
            EgressPurpose::ModelDownload,
            "https://models.example.org/m.bin?token=SECRET123",
        )
        .unwrap();
        let row = serde_json::to_string(&sink.rows()[0]).unwrap();
        assert!(!row.contains("SECRET123"), "{row}");
    }

    #[test]
    fn the_gate_runs_under_profile_off() {
        let (g, sink) = gate(PolicyConfig {
            profile: Profile::Off,
            ..PolicyConfig::default()
        });
        let err = g
            .permit(
                EgressPurpose::ModelDownload,
                "https://models.example.org/m.bin",
            )
            .unwrap_err();
        assert!(matches!(err, Error::PolicyRefusal { ref profile, .. } if profile == "off"));
        assert_eq!(sink.actions(), ["policy.refusal"]);
    }

    #[test]
    fn urls_with_credentials_or_odd_schemes_are_refused() {
        let (g, _) = gate(cfg());
        assert!(
            g.permit(
                EgressPurpose::ModelDownload,
                "https://user:pw@models.example.org/m"
            )
            .is_err()
        );
        assert!(
            g.permit(EgressPurpose::LocalInference, "ftp://127.0.0.1/x")
                .is_err()
        );
        assert!(
            g.permit(EgressPurpose::LocalInference, "not a url")
                .is_err()
        );
    }

    #[test]
    fn refusals_are_readable_from_the_log() {
        let (g, _) = gate(PolicyConfig::default());
        let _ = g.permit(EgressPurpose::ModelDownload, "https://models.example.org/m");
        let rows = g
            .audit()
            .read(&AuditFilter {
                action: Some("policy.refusal".into()),
                ..Default::default()
            })
            .unwrap();
        assert_eq!(rows.len(), 1);
        assert_eq!(
            rows[0].subject,
            "model-download:https://models.example.org:443"
        );
    }

    #[test]
    fn deny_all_from_core_is_the_unwired_default() {
        // Not ours, but the contract we slot into: the fallback refuses and says why.
        let err = cyberbrain_core::DenyAllEgress
            .permit(EgressPurpose::LocalInference, LOCAL)
            .unwrap_err();
        assert!(matches!(err, Error::PolicyRefusal { .. }));
    }

    #[test]
    fn render_prints_every_entry() {
        let out = render_register(&register(&cfg()));
        assert!(out.contains("model-download"));
        assert!(out.contains("local-inference"));
        assert!(out.contains("Telemetry: none"));
    }
}
