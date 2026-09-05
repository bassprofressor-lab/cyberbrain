//! Endpoint address validation. The most important code in this crate (SPEC §11).
//!
//! The rule: the configured base URL must resolve to loopback or private-range addresses
//! only, unless the operator set `allow_public_endpoint = true`. We check **what we are
//! about to connect to**, not what the config string looks like:
//!
//! - a hostname is resolved and *every* returned address is classified; one public address
//!   among private ones is a refusal, because we cannot control which one the OS picks;
//! - an empty answer is a refusal, not a pass — fail closed;
//! - the addresses that passed are handed back so the HTTP client can be pinned to them.
//!   Validating one DNS answer and connecting on another (DNS rebinding) is exactly the
//!   shape of bug this module exists to prevent.
//!
//! Classification is done by octet arithmetic on purpose, rather than through the standard
//! library's partly-unstable `is_*` helpers, so the table below is the whole truth and can
//! be reviewed as text.

use cyberbrain_core::{Error, Result};
use reqwest::Url;
use std::fmt;
use std::future::Future;
use std::net::{IpAddr, Ipv4Addr, Ipv6Addr, SocketAddr};
use std::pin::Pin;
use std::time::Duration;

/// The profile name written into `Error::PolicyRefusal` by this crate. The compliance
/// profile (`eu`/`ch`/`off`) is owned by the policy crate; the local-inference rule applies
/// under every profile, so it carries its own name.
pub const POLICY_NAME: &str = "local-inference";

/// Where an IP address lives. Only the first four are acceptable destinations.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum AddressClass {
    /// 127.0.0.0/8, ::1
    Loopback,
    /// RFC 1918: 10/8, 172.16/12, 192.168/16
    Private,
    /// RFC 4193 unique local: fc00::/7
    UniqueLocal,
    /// 169.254/16, fe80::/10
    LinkLocal,
    /// RFC 6598 carrier-grade NAT, 100.64/10. Used by mesh VPNs (e.g. Tailscale). Not in
    /// the spec's allowed set, so refused; the reason names the range so the operator knows
    /// what to flip.
    SharedAddressSpace,
    /// 0.0.0.0, ::. Not a destination.
    Unspecified,
    /// 224/4, ff00::/8, 255.255.255.255. Not a unicast destination.
    Multicast,
    /// 240/4 and other reserved blocks.
    Reserved,
    /// Everything else: routable on the public internet.
    Public,
}

impl AddressClass {
    /// True for the classes the spec permits without `allow_public_endpoint`.
    pub fn is_local(self) -> bool {
        matches!(
            self,
            AddressClass::Loopback
                | AddressClass::Private
                | AddressClass::UniqueLocal
                | AddressClass::LinkLocal
        )
    }

    pub fn describe(self) -> &'static str {
        match self {
            AddressClass::Loopback => "loopback",
            AddressClass::Private => "private (RFC 1918)",
            AddressClass::UniqueLocal => "unique local (RFC 4193)",
            AddressClass::LinkLocal => "link-local",
            AddressClass::SharedAddressSpace => "shared address space (RFC 6598, 100.64/10)",
            AddressClass::Unspecified => "unspecified",
            AddressClass::Multicast => "multicast or broadcast",
            AddressClass::Reserved => "reserved",
            AddressClass::Public => "public",
        }
    }
}

impl fmt::Display for AddressClass {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.describe())
    }
}

/// Classify one address. IPv6 forms that embed an IPv4 address (v4-mapped `::ffff:a.b.c.d`,
/// the deprecated v4-compatible `::a.b.c.d`, and NAT64 `64:ff9b::a.b.c.d`) are classified by
/// the embedded IPv4 address, because that is where the packets go.
pub fn classify(ip: IpAddr) -> AddressClass {
    match ip {
        IpAddr::V4(v4) => classify_v4(v4),
        IpAddr::V6(v6) => classify_v6(v6),
    }
}

fn classify_v4(ip: Ipv4Addr) -> AddressClass {
    let [a, b, _, _] = ip.octets();
    if ip.is_unspecified() {
        AddressClass::Unspecified
    } else if a == 127 {
        AddressClass::Loopback
    } else if a == 10 || (a == 172 && (16..=31).contains(&b)) || (a == 192 && b == 168) {
        AddressClass::Private
    } else if a == 169 && b == 254 {
        AddressClass::LinkLocal
    } else if a == 100 && (64..=127).contains(&b) {
        AddressClass::SharedAddressSpace
    } else if ip.is_broadcast() || (224..=239).contains(&a) {
        AddressClass::Multicast
    } else if a >= 240 {
        AddressClass::Reserved
    } else {
        AddressClass::Public
    }
}

fn classify_v6(ip: Ipv6Addr) -> AddressClass {
    let seg = ip.segments();
    if ip.is_unspecified() {
        return AddressClass::Unspecified;
    }
    if ip.is_loopback() {
        return AddressClass::Loopback;
    }
    // v4-mapped ::ffff:a.b.c.d
    if let Some(v4) = ip.to_ipv4_mapped() {
        return classify_v4(v4);
    }
    // Deprecated v4-compatible ::a.b.c.d (everything but the last 32 bits zero).
    if seg[..6].iter().all(|s| *s == 0) {
        return classify_v4(Ipv4Addr::new(
            (seg[6] >> 8) as u8,
            seg[6] as u8,
            (seg[7] >> 8) as u8,
            seg[7] as u8,
        ));
    }
    // NAT64 well-known prefix 64:ff9b::/96
    if seg[0] == 0x64 && seg[1] == 0xff9b && seg[2..6].iter().all(|s| *s == 0) {
        return classify_v4(Ipv4Addr::new(
            (seg[6] >> 8) as u8,
            seg[6] as u8,
            (seg[7] >> 8) as u8,
            seg[7] as u8,
        ));
    }
    if seg[0] & 0xff00 == 0xff00 {
        return AddressClass::Multicast;
    }
    if seg[0] & 0xffc0 == 0xfe80 {
        return AddressClass::LinkLocal;
    }
    if seg[0] & 0xfe00 == 0xfc00 {
        return AddressClass::UniqueLocal;
    }
    AddressClass::Public
}

/// Name resolution as this crate sees it. Injected so tests can hand back any answer,
/// including the split private/public answer that must be refused.
pub trait Resolver: Send + Sync {
    fn resolve<'a>(
        &'a self,
        host: &'a str,
    ) -> Pin<Box<dyn Future<Output = std::io::Result<Vec<IpAddr>>> + Send + 'a>>;
}

/// The operating system's resolver, via `getaddrinfo` on tokio's blocking pool.
#[derive(Debug, Default, Clone, Copy)]
pub struct SystemResolver;

impl Resolver for SystemResolver {
    fn resolve<'a>(
        &'a self,
        host: &'a str,
    ) -> Pin<Box<dyn Future<Output = std::io::Result<Vec<IpAddr>>> + Send + 'a>> {
        Box::pin(async move {
            let addrs = tokio::net::lookup_host((host, 0u16)).await?;
            Ok(addrs.map(|sa| sa.ip()).collect())
        })
    }
}

/// What the operator allowed.
#[derive(Debug, Clone)]
pub struct EndpointPolicy {
    /// `allow_public_endpoint` from the config. Default false. Setting it is the operator's
    /// explicit act; nothing in this crate sets it.
    pub allow_public_endpoint: bool,
    /// `allow_overlay_network` from the config. Default false. Permits `100.64.0.0/10`
    /// and nothing else.
    ///
    /// It is a separate switch rather than part of `allow_public_endpoint` because the two
    /// agreements are not the same. That range is carrier-grade NAT, where an address is
    /// usually somebody else's machine, and it is also what Tailscale and similar overlays
    /// hand out for your own GPU box. Asking an operator to tick "allow public endpoints"
    /// in order to reach their own laptop would be asking them to agree to something the
    /// setting's name misdescribes.
    pub allow_overlay_network: bool,
    /// Bound on name resolution. A resolver that hangs must not hang the caller.
    pub dns_timeout: Duration,
}

impl Default for EndpointPolicy {
    fn default() -> Self {
        Self {
            allow_public_endpoint: false,
            allow_overlay_network: false,
            dns_timeout: Duration::from_secs(3),
        }
    }
}

/// A base URL that passed validation, together with the exact addresses that were checked.
/// The client is pinned to `addrs`; nothing else is ever dialled for this endpoint.
#[derive(Debug, Clone)]
pub struct ValidatedEndpoint {
    /// Normalised base URL, no trailing slash.
    pub base_url: Url,
    /// Host as written in the URL (IPv6 literals without brackets).
    pub host: String,
    pub port: u16,
    /// Every address the host resolves to, all of which passed (or were waived).
    pub addrs: Vec<SocketAddr>,
    /// Class of each address, in `addrs` order.
    pub classes: Vec<AddressClass>,
    /// True when at least one address is not local and the operator waived the rule.
    /// Callers surface this as a caveat: a waiver is not the same as a local endpoint.
    pub public_waived: bool,
    /// True when an address in `100.64.0.0/10` was accepted because the operator set
    /// `allow_overlay_network`. Recorded separately from `public_waived` so the status
    /// screen and the audit log can say which agreement was actually relied on.
    pub overlay_waived: bool,
}

impl ValidatedEndpoint {
    /// True if the host was an IP literal (no DNS involved).
    pub fn is_literal(&self) -> bool {
        self.host.parse::<IpAddr>().is_ok()
    }

    /// One-line description for status output and audit rows.
    pub fn summary(&self) -> String {
        let addrs: Vec<String> = self
            .addrs
            .iter()
            .zip(&self.classes)
            .map(|(a, c)| format!("{} ({})", a.ip(), c))
            .collect();
        format!("{} -> [{}]", self.base_url, addrs.join(", "))
    }
}

/// Validate a base URL against the policy. Returns `Error::PolicyRefusal` for any public
/// address (unless waived), `Error::Config` for a URL we cannot even interpret, and
/// `Error::Llm` when resolution itself failed.
pub async fn validate_endpoint(
    base_url: &str,
    policy: &EndpointPolicy,
    resolver: &dyn Resolver,
) -> Result<ValidatedEndpoint> {
    let mut url = Url::parse(base_url.trim())
        .map_err(|e| Error::Config(format!("llm.base_url {base_url:?} is not a URL: {e}")))?;

    match url.scheme() {
        "http" | "https" => {}
        other => {
            return Err(Error::Config(format!(
                "llm.base_url must use http or https, not {other:?}"
            )));
        }
    }
    if !url.username().is_empty() || url.password().is_some() {
        return Err(Error::Config(
            "llm.base_url must not carry credentials; a local endpoint needs none".into(),
        ));
    }

    let host = match (url.domain(), url.host_str()) {
        (Some(d), _) => d.trim_end_matches('.').to_string(),
        // IP literal; IPv6 comes back bracketed.
        (None, Some(h)) => h.trim_start_matches('[').trim_end_matches(']').to_string(),
        (None, None) => return Err(Error::Config("llm.base_url has no host".into())),
    };
    let port = url
        .port_or_known_default()
        .ok_or_else(|| Error::Config("llm.base_url has no port".into()))?;

    // Normalise: drop trailing slash, drop query/fragment.
    url.set_query(None);
    url.set_fragment(None);
    let trimmed = url.path().trim_end_matches('/').to_string();
    url.set_path(if trimmed.is_empty() { "/" } else { &trimmed });

    // Resolve. IP literals skip DNS; names go through the injected resolver under a timeout.
    let ips: Vec<IpAddr> = match host.parse::<IpAddr>() {
        Ok(ip) => vec![ip],
        Err(_) => {
            let fut = resolver.resolve(&host);
            match tokio::time::timeout(policy.dns_timeout, fut).await {
                Ok(Ok(ips)) => ips,
                Ok(Err(e)) => {
                    return Err(Error::Llm(format!("could not resolve {host}: {e}")));
                }
                Err(_) => {
                    return Err(Error::Llm(format!(
                        "resolving {host} took longer than {:?}",
                        policy.dns_timeout
                    )));
                }
            }
        }
    };

    if ips.is_empty() {
        // Fail closed. "No answer" is not "no public address".
        return Err(Error::PolicyRefusal {
            profile: POLICY_NAME.into(),
            reason: format!(
                "{host} resolved to no addresses; refusing to guess where {base_url} leads"
            ),
        });
    }

    let classes: Vec<AddressClass> = ips.iter().map(|ip| classify(*ip)).collect();

    // Anything not permitted outright. Overlay addresses are counted separately, because
    // the switch that unlocks them is a different one and naming the wrong switch in a
    // refusal pushes the operator into granting far more than they needed to.
    let offenders: Vec<String> = ips
        .iter()
        .zip(&classes)
        .filter(|(_, c)| !c.is_local() && **c != AddressClass::SharedAddressSpace)
        .map(|(ip, c)| format!("{ip} is {c}"))
        .collect();
    let overlay: Vec<String> = ips
        .iter()
        .zip(&classes)
        .filter(|(_, c)| **c == AddressClass::SharedAddressSpace)
        .map(|(ip, c)| format!("{ip} is {c}"))
        .collect();

    let overlay_waived = if overlay.is_empty() {
        false
    } else if policy.allow_overlay_network || policy.allow_public_endpoint {
        true
    } else {
        return Err(Error::PolicyRefusal {
            profile: POLICY_NAME.into(),
            reason: format!(
                "llm.base_url {base_url} resolves into the shared address space ({}). That \
                 range is carrier-grade NAT, where the address is usually somebody else's \
                 machine, but it is also what Tailscale and similar overlays hand out for \
                 your own. Set allow_overlay_network = true if this endpoint is yours",
                overlay.join(", ")
            ),
        });
    };

    let public_waived = if offenders.is_empty() {
        false
    } else if policy.allow_public_endpoint {
        true
    } else {
        return Err(Error::PolicyRefusal {
            profile: POLICY_NAME.into(),
            reason: format!(
                "llm.base_url {base_url} resolves to a non-local address ({}); every address \
                 must be loopback, RFC 1918, RFC 4193 or link-local. Set \
                 allow_public_endpoint = true only if you mean to send note text there",
                offenders.join(", ")
            ),
        });
    };

    let addrs = ips.iter().map(|ip| SocketAddr::new(*ip, port)).collect();
    Ok(ValidatedEndpoint {
        base_url: url,
        host,
        port,
        addrs,
        classes,
        public_waived,
        overlay_waived,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashMap;

    /// A resolver with a fixed answer table. Anything not in the table is NXDOMAIN.
    pub(crate) struct StaticResolver(pub HashMap<String, Vec<IpAddr>>);

    impl StaticResolver {
        pub(crate) fn single(host: &str, ips: &[&str]) -> Self {
            let mut m = HashMap::new();
            m.insert(
                host.to_string(),
                ips.iter().map(|s| s.parse().unwrap()).collect(),
            );
            Self(m)
        }
    }

    impl Resolver for StaticResolver {
        fn resolve<'a>(
            &'a self,
            host: &'a str,
        ) -> Pin<Box<dyn Future<Output = std::io::Result<Vec<IpAddr>>> + Send + 'a>> {
            let answer = self.0.get(host).cloned();
            Box::pin(async move {
                answer.ok_or_else(|| {
                    std::io::Error::new(std::io::ErrorKind::NotFound, "no such host")
                })
            })
        }
    }

    /// A resolver that never answers, for the timeout path.
    struct HangingResolver;
    impl Resolver for HangingResolver {
        fn resolve<'a>(
            &'a self,
            _host: &'a str,
        ) -> Pin<Box<dyn Future<Output = std::io::Result<Vec<IpAddr>>> + Send + 'a>> {
            Box::pin(std::future::pending())
        }
    }

    fn ip(s: &str) -> IpAddr {
        s.parse().unwrap()
    }

    #[test]
    fn classification_table() {
        use AddressClass::*;
        let cases = [
            ("127.0.0.1", Loopback),
            ("127.255.255.254", Loopback),
            ("::1", Loopback),
            ("10.0.0.1", Private),
            ("10.255.255.255", Private),
            ("172.16.0.1", Private),
            ("172.31.255.255", Private),
            ("172.15.255.255", Public),
            ("172.32.0.1", Public),
            ("192.168.1.1", Private),
            ("192.169.0.1", Public),
            ("169.254.10.10", LinkLocal),
            ("fe80::1", LinkLocal),
            ("febf::1", LinkLocal),
            ("fec0::1", Public), // site-local is deprecated and not in the allowed set
            ("fc00::1", UniqueLocal),
            ("fd12:3456::1", UniqueLocal),
            ("fe00::1", Public),
            ("100.64.0.1", SharedAddressSpace),
            ("100.127.255.255", SharedAddressSpace),
            ("100.128.0.1", Public),
            ("0.0.0.0", Unspecified),
            ("::", Unspecified),
            ("224.0.0.1", Multicast),
            ("255.255.255.255", Multicast),
            ("ff02::1", Multicast),
            ("240.0.0.1", Reserved),
            ("8.8.8.8", Public),
            ("203.0.113.7", Public),
            ("2001:4860:4860::8888", Public),
            ("2002:0808:0808::1", Public), // 6to4 embeds a public v4; stays public
            // IPv4 embedded in IPv6: classified by the embedded address
            ("::ffff:10.0.0.1", Private),
            ("::ffff:8.8.8.8", Public),
            ("::ffff:127.0.0.1", Loopback),
            ("::10.0.0.1", Private),
            ("::8.8.8.8", Public),
            ("64:ff9b::192.168.0.1", Private),
            ("64:ff9b::8.8.8.8", Public),
        ];
        for (s, want) in cases {
            assert_eq!(classify(ip(s)), want, "{s}");
        }
    }

    #[tokio::test]
    async fn literal_loopback_v4_and_v6_pass() {
        let p = EndpointPolicy::default();
        let r = StaticResolver(HashMap::new());
        for u in [
            "http://127.0.0.1:11434/v1",
            "http://[::1]:11434/v1",
            "http://127.0.0.1:11434/v1/",
            "https://127.0.0.1:8443/v1",
        ] {
            let v = validate_endpoint(u, &p, &r)
                .await
                .unwrap_or_else(|e| panic!("{u}: {e}"));
            assert!(!v.public_waived);
            assert!(v.is_literal());
            assert_eq!(v.addrs.len(), 1);
            assert_eq!(v.classes, vec![AddressClass::Loopback]);
            assert!(!v.base_url.as_str().ends_with('/'), "{}", v.base_url);
        }
    }

    #[tokio::test]
    async fn literal_private_ula_and_link_local_pass() {
        let p = EndpointPolicy::default();
        let r = StaticResolver(HashMap::new());
        for (u, class) in [
            ("http://10.0.0.5:8000/v1", AddressClass::Private),
            ("http://172.16.4.4:8000/v1", AddressClass::Private),
            ("http://192.168.1.20:1234/v1", AddressClass::Private),
            ("http://[fd00::20]:1234/v1", AddressClass::UniqueLocal),
            ("http://169.254.1.1:1234/v1", AddressClass::LinkLocal),
        ] {
            let v = validate_endpoint(u, &p, &r)
                .await
                .unwrap_or_else(|e| panic!("{u}: {e}"));
            assert_eq!(v.classes, vec![class], "{u}");
        }
    }

    #[tokio::test]
    async fn literal_public_is_refused() {
        let p = EndpointPolicy::default();
        let r = StaticResolver(HashMap::new());
        for u in [
            "http://8.8.8.8/v1",
            "https://203.0.113.7:443/v1",
            "http://[2001:4860:4860::8888]:11434/v1",
            "http://[::ffff:8.8.8.8]:11434/v1",
            "http://0.0.0.0:11434/v1",
            "http://224.0.0.1:11434/v1",
        ] {
            match validate_endpoint(u, &p, &r).await {
                Err(Error::PolicyRefusal { profile, reason }) => {
                    assert_eq!(profile, POLICY_NAME);
                    assert!(reason.contains("allow_public_endpoint"), "{u}: {reason}");
                }
                other => panic!("{u}: expected PolicyRefusal, got {other:?}"),
            }
        }
    }

    /// `100.64/10` is refused by default like anything else non-local, but the refusal must
    /// name `allow_overlay_network` and not `allow_public_endpoint`. Naming the wider switch
    /// would push an operator into granting far more than the case needs, which is a consent
    /// failure even though the bytes would have gone to the same machine either way.
    #[tokio::test]
    async fn shared_address_space_is_refused_by_the_switch_that_actually_covers_it() {
        let p = EndpointPolicy::default();
        let r = StaticResolver(HashMap::new());
        for u in [
            "http://100.64.0.1:11434/v1",
            "http://100.100.1.1:11434/v1",
            "http://100.127.255.254:11434/v1",
        ] {
            match validate_endpoint(u, &p, &r).await {
                Err(Error::PolicyRefusal { reason, .. }) => {
                    assert!(
                        reason.contains("allow_overlay_network"),
                        "{u} must name the switch that covers it: {reason}"
                    );
                    assert!(
                        !reason.contains("allow_public_endpoint"),
                        "{u} must not push the operator at the wider switch: {reason}"
                    );
                }
                other => panic!("{u}: expected PolicyRefusal, got {other:?}"),
            }
        }
    }

    /// The narrow switch opens the narrow door and nothing else. A Tailscale endpoint works;
    /// a genuinely public one still does not.
    #[tokio::test]
    async fn allow_overlay_network_permits_cgnat_and_nothing_more() {
        let p = EndpointPolicy {
            allow_overlay_network: true,
            ..Default::default()
        };
        let r = StaticResolver(HashMap::new());

        let ok = validate_endpoint("http://100.100.1.1:11434/v1", &p, &r)
            .await
            .expect("an overlay address the operator allowed");
        assert!(ok.overlay_waived, "the waiver must be recorded, not silent");
        assert!(!ok.public_waived, "this is not a public waiver");

        assert!(
            validate_endpoint("http://8.8.8.8:11434/v1", &p, &r)
                .await
                .is_err(),
            "allow_overlay_network must not open the public door"
        );
    }

    #[tokio::test]
    async fn hostname_resolving_to_private_passes_and_pins_all_addresses() {
        let p = EndpointPolicy::default();
        let r = StaticResolver::single("gpu.lan", &["10.0.0.5", "fd00::5"]);
        let v = validate_endpoint("http://gpu.lan:8000/v1", &p, &r)
            .await
            .unwrap();
        assert_eq!(v.host, "gpu.lan");
        assert_eq!(v.port, 8000);
        assert_eq!(
            v.addrs.len(),
            2,
            "every resolved address is pinned, not just the first"
        );
        assert!(v.addrs.iter().all(|a| a.port() == 8000));
        assert!(!v.is_literal());
    }

    #[tokio::test]
    async fn split_private_and_public_answer_is_refused() {
        // The case that matters most: a name that resolves to one private and one public
        // address. The OS may pick either. We refuse the whole endpoint.
        let p = EndpointPolicy::default();
        let r = StaticResolver::single("gpu.lan", &["10.0.0.5", "203.0.113.7"]);
        match validate_endpoint("http://gpu.lan:8000/v1", &p, &r).await {
            Err(Error::PolicyRefusal { reason, .. }) => {
                assert!(reason.contains("203.0.113.7 is public"), "{reason}");
                assert!(
                    !reason.contains("10.0.0.5 is"),
                    "only offenders are listed: {reason}"
                );
            }
            other => panic!("expected PolicyRefusal, got {other:?}"),
        }
        // Order must not matter either.
        let r = StaticResolver::single("gpu.lan", &["203.0.113.7", "10.0.0.5"]);
        assert!(matches!(
            validate_endpoint("http://gpu.lan:8000/v1", &p, &r).await,
            Err(Error::PolicyRefusal { .. })
        ));
    }

    #[tokio::test]
    async fn hostname_resolving_to_public_only_is_refused() {
        let p = EndpointPolicy::default();
        let r = StaticResolver::single("api.example.com", &["203.0.113.7"]);
        assert!(matches!(
            validate_endpoint("https://api.example.com/v1", &p, &r).await,
            Err(Error::PolicyRefusal { .. })
        ));
    }

    #[tokio::test]
    async fn empty_dns_answer_is_refused_not_passed() {
        let p = EndpointPolicy::default();
        let r = StaticResolver::single("ghost.lan", &[]);
        match validate_endpoint("http://ghost.lan/v1", &p, &r).await {
            Err(Error::PolicyRefusal { reason, .. }) => assert!(reason.contains("no addresses")),
            other => panic!("expected PolicyRefusal, got {other:?}"),
        }
    }

    #[tokio::test]
    async fn nxdomain_is_an_llm_error_not_a_pass() {
        let p = EndpointPolicy::default();
        let r = StaticResolver(HashMap::new());
        assert!(matches!(
            validate_endpoint("http://nope.lan/v1", &p, &r).await,
            Err(Error::Llm(_))
        ));
    }

    #[tokio::test]
    async fn hanging_resolver_is_bounded() {
        let p = EndpointPolicy {
            allow_public_endpoint: false,
            allow_overlay_network: false,
            dns_timeout: Duration::from_millis(50),
        };
        let started = std::time::Instant::now();
        let out = validate_endpoint("http://slow.lan/v1", &p, &HangingResolver).await;
        assert!(matches!(out, Err(Error::Llm(_))), "{out:?}");
        assert!(started.elapsed() < Duration::from_secs(2));
    }

    #[tokio::test]
    async fn waiver_admits_public_but_marks_it() {
        let p = EndpointPolicy {
            allow_public_endpoint: true,
            ..Default::default()
        };
        let r = StaticResolver::single("api.example.com", &["203.0.113.7"]);
        let v = validate_endpoint("https://api.example.com/v1", &p, &r)
            .await
            .unwrap();
        assert!(v.public_waived, "a waiver is recorded, never silent");
        // A waiver does not turn a local endpoint into a waived one.
        let r = StaticResolver(HashMap::new());
        let v = validate_endpoint("http://127.0.0.1:11434/v1", &p, &r)
            .await
            .unwrap();
        assert!(!v.public_waived);
        // And it still cannot rescue a URL that resolves to nothing.
        let r = StaticResolver::single("ghost.lan", &[]);
        assert!(
            validate_endpoint("http://ghost.lan/v1", &p, &r)
                .await
                .is_err()
        );
    }

    #[tokio::test]
    async fn bad_urls_are_config_errors() {
        let p = EndpointPolicy::default();
        let r = StaticResolver(HashMap::new());
        for u in [
            "not a url",
            "ftp://127.0.0.1/v1",
            "file:///etc/passwd",
            "http://user:pw@127.0.0.1/v1",
            "http://",
        ] {
            assert!(
                matches!(validate_endpoint(u, &p, &r).await, Err(Error::Config(_))),
                "{u}"
            );
        }
    }

    // ------------------------------------------------------------------------------------
    // SPEC §14.2: a check has not passed until it has been run against the broken state.
    // Below is the naive implementation this module replaces: it looks at the config
    // string. The test shows, case by case, where it gives the wrong answer and where the
    // real validator gives the right one.
    // ------------------------------------------------------------------------------------

    fn naive_is_local(base_url: &str) -> bool {
        base_url.contains("127.0.0.1") || base_url.contains("localhost")
    }

    #[tokio::test]
    async fn naive_string_match_fails_where_the_real_validator_does_not() {
        let p = EndpointPolicy::default();

        // (url, resolver answer, what the answer must be, whether the naive matcher gets
        // it wrong)
        let cases: Vec<(&str, StaticResolver, bool, bool)> = vec![
            // Looks local to a string match, resolves to a public host.
            (
                "http://127.0.0.1.nip.example/v1",
                StaticResolver::single("127.0.0.1.nip.example", &["203.0.113.7"]),
                false,
                true,
            ),
            (
                "http://localhost.evil.example/v1",
                StaticResolver::single("localhost.evil.example", &["203.0.113.7"]),
                false,
                true,
            ),
            // "localhost" itself can be pointed anywhere by /etc/hosts or a rogue resolver.
            (
                "http://localhost:11434/v1",
                StaticResolver::single("localhost", &["203.0.113.7"]),
                false,
                true,
            ),
            // Split answer: the naive matcher refuses this one, but only because the string
            // lacks "127.0.0.1"; it would accept the same split answer behind "localhost".
            (
                "http://gpu.lan:8000/v1",
                StaticResolver::single("gpu.lan", &["10.0.0.5", "203.0.113.7"]),
                false,
                false,
            ),
            (
                "http://localhost:11434/v1",
                StaticResolver::single("localhost", &["127.0.0.1", "203.0.113.7"]),
                false,
                true,
            ),
            // Perfectly good local endpoints the string match would wrongly refuse.
            (
                "http://[::1]:11434/v1",
                StaticResolver(HashMap::new()),
                true,
                true,
            ),
            (
                "http://10.0.0.5:8000/v1",
                StaticResolver(HashMap::new()),
                true,
                true,
            ),
            (
                "http://[fd00::20]:1234/v1",
                StaticResolver(HashMap::new()),
                true,
                true,
            ),
            (
                "http://gpu.lan:8000/v1",
                StaticResolver::single("gpu.lan", &["192.168.1.20"]),
                true,
                true,
            ),
        ];

        let expected_naive_wrong = cases.iter().filter(|c| c.3).count();
        assert!(
            expected_naive_wrong >= 7,
            "the demonstration needs real breadth"
        );
        let mut naive_wrong = 0;
        for (url, resolver, want, naive_should_be_wrong) in &cases {
            let real = validate_endpoint(url, &p, resolver).await.is_ok();
            assert_eq!(real, *want, "real validator on {url}");
            let naive_is_wrong = naive_is_local(url) != *want;
            assert_eq!(
                naive_is_wrong, *naive_should_be_wrong,
                "naive matcher on {url}"
            );
            if naive_is_wrong {
                naive_wrong += 1;
            }
        }
        assert_eq!(naive_wrong, expected_naive_wrong);
    }
}
