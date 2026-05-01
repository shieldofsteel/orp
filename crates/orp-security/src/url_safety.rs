//! URL safety / SSRF defence primitives.
//!
//! This module is the single source of truth for "before we POST/GET to a
//! user-supplied URL, refuse to talk to internal hosts" — the defence used
//! by every outbound HTTP path in ORP (HTTP poller, webhook notifications,
//! Slack/Telegram alert channels, and any future federation hop).
//!
//! Two layers of defence:
//!
//! 1. [`is_url_safe`] — fast pre-flight scheme + cloud-metadata + RFC1918
//!    rejection. Calls DNS once.
//! 2. [`build_safe_client`] — produces a `reqwest::Client` whose DNS resolver
//!    is pinned to the IPs we already validated, closing the DNS-rebinding
//!    window where an attacker's resolver could swap a public IP for a
//!    loopback address between the check and the actual TCP connect. The
//!    client also installs a custom redirect policy that re-validates every
//!    hop with `is_url_safe`.
//!
//! Operators can opt out (e.g. for legitimate localhost integrations) by
//! passing `allow_private = true`; that is the **only** way to reach an
//! internal target through this module.
//!
//! The implementation here is the same one that previously lived inside the
//! HTTP poller adapter; it was extracted so the notification engine and any
//! other outbound caller can reuse it without depending on `orp-connector`.

use std::net::{IpAddr, SocketAddr};
use std::sync::Arc;

/// Reject URLs whose host resolves to an internal/private network or a known
/// cloud-metadata endpoint. Operators can opt back in by setting
/// `allow_private = true`.
///
/// SSRF defence — without this, a webhook channel pointed at
/// `http://169.254.169.254/latest/meta-data/` (AWS) or `http://localhost:6379`
/// (Redis) would happily exfiltrate credentials or pivot into internal
/// services.
pub fn is_url_safe(url: &str, allow_private: bool) -> Result<(), String> {
    if allow_private {
        return Ok(());
    }
    let parsed = reqwest::Url::parse(url).map_err(|e| format!("invalid URL: {e}"))?;
    let scheme = parsed.scheme();
    if scheme != "http" && scheme != "https" {
        return Err(format!("scheme '{scheme}' not allowed (use http/https)"));
    }
    let host = parsed
        .host_str()
        .ok_or_else(|| "URL has no host".to_string())?;

    // Block known cloud-metadata hostnames before resolution.
    let lowered = host.to_ascii_lowercase();
    if matches!(
        lowered.as_str(),
        "metadata.google.internal" | "metadata" | "instance-data"
    ) {
        return Err(format!("host '{host}' is a cloud-metadata endpoint"));
    }

    // Resolve and check every IP. We do a synchronous resolve since this is
    // called once per request, at low frequency.
    use std::net::ToSocketAddrs;
    let port = parsed.port_or_known_default().unwrap_or(80);
    let addrs = (host, port)
        .to_socket_addrs()
        .map_err(|e| format!("DNS resolution failed: {e}"))?;
    for addr in addrs {
        let ip = addr.ip();
        if is_internal_ip(&ip) {
            return Err(format!("host '{host}' resolves to internal address {ip}"));
        }
    }
    Ok(())
}

/// Returns true if the given IP is loopback / private / link-local / etc.
/// — anything we should refuse to connect to from a user-controlled URL.
pub fn is_internal_ip(ip: &IpAddr) -> bool {
    match ip {
        IpAddr::V4(v4) => {
            v4.is_loopback()
                || v4.is_private()
                || v4.is_link_local()
                || v4.is_broadcast()
                || v4.is_unspecified()
                || v4.is_documentation()
                // 169.254.169.254 covered by is_link_local; also cover the
                // wider 169.254/16 range and 100.64/10 (CGNAT).
                || (v4.octets()[0] == 100 && (64..128).contains(&v4.octets()[1]))
        }
        IpAddr::V6(v6) => {
            v6.is_loopback()
                || v6.is_unspecified()
                || v6.is_unique_local()
                // fe80::/10 link-local
                || (v6.segments()[0] & 0xffc0) == 0xfe80
        }
    }
}

/// A pluggable host resolver. Production uses [`StdResolver`]; tests inject a
/// stub that simulates DNS rebinding (different IPs per call).
pub trait HostResolver: Send + Sync {
    fn resolve(&self, host: &str, port: u16) -> Result<Vec<SocketAddr>, String>;
}

/// Default resolver — wraps `std::net::ToSocketAddrs::to_socket_addrs`.
pub struct StdResolver;
impl HostResolver for StdResolver {
    fn resolve(&self, host: &str, port: u16) -> Result<Vec<SocketAddr>, String> {
        use std::net::ToSocketAddrs;
        (host, port)
            .to_socket_addrs()
            .map_err(|e| format!("DNS resolution failed: {e}"))
            .map(|iter| iter.collect())
    }
}

/// SSRF-safe DNS resolution: validate URL → resolve once → re-validate every
/// resolved IP → return all of them so reqwest can be pinned via
/// `Client::builder().resolve_to_addrs(host, &addrs)`.
///
/// Single source of truth for DNS rebinding defence: the IPs reqwest sees on
/// the wire are exactly the ones we just validated, eliminating the
/// double-resolution window where an attacker's DNS server can swap a public
/// IP for an internal one between check and connect.
pub fn safe_resolve_with(
    url: &str,
    allow_private: bool,
    resolver: &dyn HostResolver,
) -> Result<(String, Vec<SocketAddr>), String> {
    // URL-shape & cloud-metadata checks first; this path does NOT do DNS so
    // injected stub resolvers can fully replace `is_url_safe`'s own lookup.
    let parsed = reqwest::Url::parse(url).map_err(|e| format!("invalid URL: {e}"))?;
    let scheme = parsed.scheme();
    if scheme != "http" && scheme != "https" {
        return Err(format!("scheme '{scheme}' not allowed (use http/https)"));
    }
    let host = parsed
        .host_str()
        .ok_or_else(|| "URL has no host".to_string())?
        .to_string();
    if !allow_private {
        let lowered = host.to_ascii_lowercase();
        if matches!(
            lowered.as_str(),
            "metadata.google.internal" | "metadata" | "instance-data"
        ) {
            return Err(format!("host '{host}' is a cloud-metadata endpoint"));
        }
    }
    let port = parsed.port_or_known_default().unwrap_or(80);
    // Literal-IP fast path: skip DNS entirely.
    if let Ok(literal) = host.parse::<IpAddr>() {
        if !allow_private && is_internal_ip(&literal) {
            return Err(format!("host '{host}' is an internal address"));
        }
        return Ok((host, vec![SocketAddr::new(literal, port)]));
    }
    let addrs = resolver.resolve(&host, port)?;
    if addrs.is_empty() {
        return Err(format!("DNS resolution returned no addresses for '{host}'"));
    }
    if !allow_private {
        for sa in &addrs {
            let ip = sa.ip();
            if is_internal_ip(&ip) {
                return Err(format!("host '{host}' resolves to internal address {ip}"));
            }
        }
    }
    Ok((host, addrs))
}

/// Convenience wrapper using the standard resolver.
pub fn safe_resolve(url: &str, allow_private: bool) -> Result<(String, Vec<SocketAddr>), String> {
    safe_resolve_with(url, allow_private, &StdResolver)
}

/// Build a reqwest `Client` whose DNS resolution is pinned to the IPs
/// `safe_resolve` just validated. Redirects are still policed by the existing
/// SSRF guard, AND a custom DNS resolver is installed so every redirect-hop
/// host goes through the same validate-then-pin primitive — eliminating the
/// rebinding window for the redirect target as well.
pub fn build_safe_client(
    url: &str,
    allow_private: bool,
) -> Result<(reqwest::Client, Vec<SocketAddr>), String> {
    let (host, addrs) = safe_resolve(url, allow_private)?;

    // Re-check every redirect target against `is_url_safe`. The DNS pinning
    // for redirect hosts is handled by the custom Resolve impl below (a 302
    // to a hostname we've never seen still gets validate-then-pin).
    let allow_private_for_redirects = allow_private;
    let policy = reqwest::redirect::Policy::custom(move |attempt| {
        if attempt.previous().len() >= 5 {
            return attempt.error("redirect chain exceeded 5 hops");
        }
        let target = attempt.url().to_string();
        match is_url_safe(&target, allow_private_for_redirects) {
            Ok(()) => attempt.follow(),
            Err(reason) => attempt.error(format!(
                "SSRF guard blocked redirect to '{target}': {reason}"
            )),
        }
    });

    let resolver = Arc::new(SsrfDnsResolver { allow_private });
    let mut builder = reqwest::Client::builder()
        .timeout(std::time::Duration::from_secs(15))
        .redirect(policy)
        .dns_resolver(resolver);
    // Pin the initial host's name so the first request connects to one of
    // the IPs we already validated, even if the kernel cache disagrees.
    builder = builder.resolve_to_addrs(&host, &addrs);
    builder
        .build()
        .map(|c| (c, addrs))
        .map_err(|e| format!("client build failed: {e}"))
}

/// Custom reqwest DNS resolver that runs every name through the SSRF
/// validate-then-return primitive. Returning the addrs directly closes the
/// double-resolution gap that makes DNS rebinding work.
struct SsrfDnsResolver {
    allow_private: bool,
}

impl reqwest::dns::Resolve for SsrfDnsResolver {
    fn resolve(&self, name: reqwest::dns::Name) -> reqwest::dns::Resolving {
        let host = name.as_str().to_string();
        let allow_private = self.allow_private;
        Box::pin(async move {
            // Use the std resolver on a blocking thread — std::net DNS is
            // blocking; tokio::task::spawn_blocking keeps the runtime sane.
            let res = tokio::task::spawn_blocking(move || {
                StdResolver.resolve(&host, 0).and_then(|addrs| {
                    if !allow_private {
                        for sa in &addrs {
                            let ip = sa.ip();
                            if is_internal_ip(&ip) {
                                return Err(format!(
                                    "redirect host resolves to internal address {ip}"
                                ));
                            }
                        }
                    }
                    Ok(addrs)
                })
            })
            .await
            .map_err(|e| Box::<dyn std::error::Error + Send + Sync>::from(e.to_string()))?;
            let addrs = res.map_err(Box::<dyn std::error::Error + Send + Sync>::from)?;
            let iter: reqwest::dns::Addrs = Box::new(addrs.into_iter());
            Ok(iter)
        })
    }
}

#[cfg(test)]
mod tests {
    use super::{build_safe_client, is_url_safe, safe_resolve, safe_resolve_with, HostResolver};
    use std::net::SocketAddr;
    use std::sync::atomic::{AtomicUsize, Ordering};

    #[test]
    fn allows_public_https() {
        assert!(is_url_safe("https://example.com/api", false).is_ok());
    }
    #[test]
    fn blocks_loopback() {
        assert!(is_url_safe("http://127.0.0.1:8080/", false).is_err());
        assert!(is_url_safe("http://localhost/", false).is_err());
    }
    #[test]
    fn blocks_link_local_and_cloud_metadata() {
        assert!(is_url_safe("http://169.254.169.254/latest/meta-data", false).is_err());
        assert!(is_url_safe("http://metadata.google.internal/", false).is_err());
    }
    #[test]
    fn blocks_rfc1918() {
        assert!(is_url_safe("http://10.0.0.5/", false).is_err());
        assert!(is_url_safe("http://192.168.1.1/", false).is_err());
        assert!(is_url_safe("http://172.16.0.1/", false).is_err());
    }
    #[test]
    fn rejects_non_http_schemes() {
        assert!(is_url_safe("file:///etc/passwd", false).is_err());
        assert!(is_url_safe("gopher://example.com/", false).is_err());
    }
    #[test]
    fn allows_private_when_opted_in() {
        assert!(is_url_safe("http://10.0.0.5/", true).is_ok());
    }

    /// Stub resolver that flips its answer between calls — emulates an
    /// attacker DNS server that hands out a public IP first (passes SSRF
    /// check) then a loopback IP on the next lookup (would land reqwest on
    /// an internal service).
    struct RebindResolver {
        calls: AtomicUsize,
        first: Vec<SocketAddr>,
        second: Vec<SocketAddr>,
    }
    impl HostResolver for RebindResolver {
        fn resolve(&self, _host: &str, _port: u16) -> Result<Vec<SocketAddr>, String> {
            let n = self.calls.fetch_add(1, Ordering::SeqCst);
            Ok(if n == 0 {
                self.first.clone()
            } else {
                self.second.clone()
            })
        }
    }

    #[test]
    fn dns_rebind_simulation_pin_to_first_resolution() {
        let resolver = RebindResolver {
            calls: AtomicUsize::new(0),
            first: vec!["8.8.8.8:80".parse().unwrap()],
            second: vec!["127.0.0.1:80".parse().unwrap()],
        };
        let (host, pinned) =
            safe_resolve_with("http://attacker.example/", false, &resolver).unwrap();
        assert_eq!(host, "attacker.example");
        assert_eq!(pinned.len(), 1);
        assert_eq!(pinned[0].ip().to_string(), "8.8.8.8");
        let rebound = resolver.resolve("attacker.example", 80).unwrap();
        assert_eq!(rebound[0].ip().to_string(), "127.0.0.1");
        assert_eq!(pinned[0].ip().to_string(), "8.8.8.8");
    }

    #[test]
    fn safe_resolve_blocks_internal_ip() {
        assert!(safe_resolve("http://127.0.0.1/", false).is_err());
        assert!(safe_resolve("http://localhost/", false).is_err());
        assert!(safe_resolve("http://127.0.0.1/", true).is_ok());
    }

    #[test]
    fn safe_resolve_returns_one_socketaddr_per_a_record() {
        struct MultiResolver;
        impl HostResolver for MultiResolver {
            fn resolve(&self, _host: &str, port: u16) -> Result<Vec<SocketAddr>, String> {
                Ok(vec![
                    SocketAddr::new("8.8.8.8".parse().unwrap(), port),
                    SocketAddr::new("1.1.1.1".parse().unwrap(), port),
                    SocketAddr::new("9.9.9.9".parse().unwrap(), port),
                ])
            }
        }
        let (host, addrs) =
            safe_resolve_with("http://multi.example/", false, &MultiResolver).unwrap();
        assert_eq!(host, "multi.example");
        assert_eq!(addrs.len(), 3);
        let ips: Vec<String> = addrs.iter().map(|sa| sa.ip().to_string()).collect();
        assert!(ips.contains(&"8.8.8.8".to_string()));
        assert!(ips.contains(&"1.1.1.1".to_string()));
        assert!(ips.contains(&"9.9.9.9".to_string()));
        struct MixedResolver;
        impl HostResolver for MixedResolver {
            fn resolve(&self, _host: &str, port: u16) -> Result<Vec<SocketAddr>, String> {
                Ok(vec![
                    SocketAddr::new("8.8.8.8".parse().unwrap(), port),
                    SocketAddr::new("10.0.0.1".parse().unwrap(), port),
                ])
            }
        }
        assert!(safe_resolve_with("http://mixed.example/", false, &MixedResolver).is_err());
    }

    #[test]
    fn client_with_pinned_resolution_uses_correct_ip() {
        let (_client, addrs) = build_safe_client("http://8.8.8.8/", false).unwrap();
        assert_eq!(addrs.len(), 1);
        assert_eq!(addrs[0].ip().to_string(), "8.8.8.8");
        assert!(build_safe_client("http://127.0.0.1/", false).is_err());
    }

    // ── IPv6 paths ────────────────────────────────────────────────────────
    #[test]
    fn blocks_ipv6_loopback() {
        assert!(is_url_safe("http://[::1]/", false).is_err());
    }
    #[test]
    fn blocks_ipv6_link_local() {
        assert!(is_url_safe("http://[fe80::1]/", false).is_err());
    }
    #[test]
    fn blocks_ipv6_unique_local() {
        assert!(is_url_safe("http://[fc00::1]/", false).is_err());
    }
    #[test]
    fn blocks_ipv6_unspecified() {
        assert!(is_url_safe("http://[::]/", false).is_err());
    }

    // ── DNS failure must fail-closed (Err), never panic ───────────────────
    #[test]
    fn dns_failure_returns_error_not_panic() {
        let result = is_url_safe(
            "http://this-host-definitely-does-not-resolve.invalid/",
            false,
        );
        match result {
            Err(msg) => assert!(
                msg.contains("DNS resolution failed"),
                "expected DNS resolution failure, got: {}",
                msg
            ),
            Ok(()) => {
                eprintln!("skipping dns_failure test: local resolver returned a hit for .invalid")
            }
        }
    }

    // ── Encoding / port-number bypass attempts ────────────────────────────
    #[test]
    fn urlencoded_localhost_still_blocked() {
        assert!(is_url_safe("http://%6c%6f%63%61%6c%68%6f%73%74/", false).is_err());
    }
    #[test]
    fn port_does_not_bypass_check() {
        assert!(is_url_safe("http://127.0.0.1:6379/", false).is_err());
    }
}
