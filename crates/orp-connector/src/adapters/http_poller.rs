use crate::traits::{Connector, ConnectorConfig, ConnectorError, ConnectorStats, SourceEvent};
use async_trait::async_trait;
use chrono::Utc;
use std::collections::HashMap;
use std::net::{IpAddr, SocketAddr};
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::Arc;

/// HTTP REST polling connector — periodically fetches JSON data from a URL
pub struct HttpPollerConnector {
    config: ConnectorConfig,
    running: Arc<AtomicBool>,
    events_count: Arc<AtomicU64>,
    errors_count: Arc<AtomicU64>,
}

/// Reject URLs whose host resolves to an internal/private network or a known
/// cloud-metadata endpoint. Operators can opt back in by setting the connector
/// property `allow_private_targets = true`.
///
/// SSRF defence — without this an HTTP poller pointed at
/// `http://169.254.169.254/latest/meta-data/` (AWS) or `http://localhost:6379`
/// (Redis) would happily exfiltrate credentials or pivot into internal
/// services.
fn is_url_safe(url: &str, allow_private: bool) -> Result<(), String> {
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
    // called once per poll, at low frequency.
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

fn is_internal_ip(ip: &IpAddr) -> bool {
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

/// A pluggable host resolver. Production uses `std_resolver`; tests inject a
/// stub that simulates DNS rebinding (different IPs per call).
pub(crate) trait HostResolver: Send + Sync {
    fn resolve(&self, host: &str, port: u16) -> Result<Vec<SocketAddr>, String>;
}

/// Default resolver — wraps `std::net::ToSocketAddrs::to_socket_addrs`.
struct StdResolver;
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
pub(crate) fn safe_resolve_with(
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
pub(crate) fn safe_resolve(
    url: &str,
    allow_private: bool,
) -> Result<(String, Vec<SocketAddr>), String> {
    safe_resolve_with(url, allow_private, &StdResolver)
}

/// Build a reqwest `Client` whose DNS resolution is pinned to the IPs
/// `safe_resolve` just validated. Redirects are still policed by the existing
/// SSRF guard, AND a custom DNS resolver is installed so every redirect-hop
/// host goes through the same validate-then-pin primitive — eliminating the
/// rebinding window for the redirect target as well.
pub(crate) fn build_safe_client(
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
mod ssrf_tests {
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
        // First lookup hands out 8.8.8.8 (passes); a malicious DNS server
        // would then return 127.0.0.1 on the next lookup. The point of
        // `safe_resolve_with` is to capture the first set of addrs so
        // anything downstream uses *those*, not whatever the next lookup
        // returns. We then poke the resolver a second time directly to
        // prove the rebind occurred — and that our pinned tuple is still
        // the public IP.
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
        // Second invocation of the underlying resolver — this is the
        // moment a non-pinned client would be vulnerable.
        let rebound = resolver.resolve("attacker.example", 80).unwrap();
        assert_eq!(rebound[0].ip().to_string(), "127.0.0.1");
        // Our pinned copy is still the original public IP.
        assert_eq!(pinned[0].ip().to_string(), "8.8.8.8");
    }

    #[test]
    fn safe_resolve_blocks_internal_ip() {
        // Literal-IP fast path: localhost / 127.0.0.1 must be rejected
        // before any DNS happens. Tests both the hostname and the literal.
        assert!(safe_resolve("http://127.0.0.1/", false).is_err());
        assert!(safe_resolve("http://localhost/", false).is_err());
        // And opting in lets it through.
        assert!(safe_resolve("http://127.0.0.1/", true).is_ok());
    }

    #[test]
    fn safe_resolve_returns_one_socketaddr_per_a_record() {
        // Use the stub resolver so this test does not rely on a network.
        // Three public IPs all pass validation and all come back as
        // SocketAddrs ready to feed into reqwest's `resolve_to_addrs`.
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
        // If any one resolved IP is internal the whole resolution must fail.
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
        // Smoke-check the production path: build_safe_client over a literal
        // public IP must succeed and return the same SocketAddr we'd
        // expect to be pinned. Using a literal IP avoids relying on real
        // DNS in this unit test.
        let (_client, addrs) = build_safe_client("http://8.8.8.8/", false).unwrap();
        assert_eq!(addrs.len(), 1);
        assert_eq!(addrs[0].ip().to_string(), "8.8.8.8");
        // A loopback URL must be rejected at this layer too.
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
        // `.invalid` is reserved by IANA (RFC 2606) precisely for
        // "guaranteed not to resolve" tests. If the local resolver does
        // resolve it (rare misconfigurations), skip rather than fail.
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
        // The `reqwest::Url` parser percent-decodes hostnames before exposing
        // them as `host_str`, so this must still resolve to "localhost"
        // and trip the loopback check.
        assert!(is_url_safe("http://%6c%6f%63%61%6c%68%6f%73%74/", false).is_err());
    }
    #[test]
    fn port_does_not_bypass_check() {
        // Picking a non-default port for a "real" service (Redis on 6379)
        // must not bypass the loopback rejection — host is still 127.0.0.1.
        assert!(is_url_safe("http://127.0.0.1:6379/", false).is_err());
    }
}

impl HttpPollerConnector {
    pub fn new(config: ConnectorConfig) -> Self {
        Self {
            config,
            running: Arc::new(AtomicBool::new(false)),
            events_count: Arc::new(AtomicU64::new(0)),
            errors_count: Arc::new(AtomicU64::new(0)),
        }
    }

    /// Extract entities from a JSON array response
    pub fn extract_entities(
        json: &serde_json::Value,
        entity_type: &str,
        connector_id: &str,
        id_field: &str,
        lat_field: &str,
        lon_field: &str,
    ) -> Vec<SourceEvent> {
        let items = if let Some(arr) = json.as_array() {
            arr.clone()
        } else if let Some(data) = json.get("data").and_then(|d| d.as_array()) {
            data.clone()
        } else if let Some(features) = json.get("features").and_then(|f| f.as_array()) {
            // GeoJSON format
            features.clone()
        } else {
            vec![json.clone()]
        };

        items
            .iter()
            .filter_map(|item| {
                let entity_id = item
                    .get(id_field)
                    .and_then(|v| {
                        v.as_str()
                            .map(String::from)
                            .or_else(|| v.as_i64().map(|n| n.to_string()))
                    })
                    .unwrap_or_default();
                if entity_id.is_empty() {
                    return None;
                }

                let latitude = item.get(lat_field).and_then(|v| v.as_f64());
                let longitude = item.get(lon_field).and_then(|v| v.as_f64());

                let mut properties = HashMap::new();
                if let Some(obj) = item.as_object() {
                    for (k, v) in obj {
                        if k != id_field && k != lat_field && k != lon_field {
                            properties.insert(k.clone(), v.clone());
                        }
                    }
                }

                Some(SourceEvent {
                    connector_id: connector_id.to_string(),
                    entity_id: format!("{}:{}", entity_type, entity_id),
                    entity_type: entity_type.to_string(),
                    properties,
                    timestamp: Utc::now(),
                    latitude,
                    longitude,
                })
            })
            .collect()
    }
}

#[async_trait]
impl Connector for HttpPollerConnector {
    fn connector_id(&self) -> &str {
        &self.config.connector_id
    }

    async fn start(
        &self,
        tx: tokio::sync::mpsc::Sender<SourceEvent>,
    ) -> Result<(), ConnectorError> {
        self.running.store(true, Ordering::SeqCst);
        tracing::info!(
            connector_id = %self.config.connector_id,
            "HTTP poller connector started"
        );

        let running = self.running.clone();
        let events_count = self.events_count.clone();
        let errors_count = self.errors_count.clone();
        let config = self.config.clone();
        let connector_id = self.config.connector_id.clone();

        let poll_interval_secs: u64 = config
            .properties
            .get("poll_interval_secs")
            .and_then(|v| v.as_u64())
            .unwrap_or(60);

        let id_field = config
            .properties
            .get("id_field")
            .and_then(|v| v.as_str())
            .unwrap_or("id")
            .to_string();
        let lat_field = config
            .properties
            .get("lat_field")
            .and_then(|v| v.as_str())
            .unwrap_or("latitude")
            .to_string();
        let lon_field = config
            .properties
            .get("lon_field")
            .and_then(|v| v.as_str())
            .unwrap_or("longitude")
            .to_string();

        tokio::spawn(async move {
            let mut interval =
                tokio::time::interval(tokio::time::Duration::from_secs(poll_interval_secs));

            while running.load(Ordering::SeqCst) {
                interval.tick().await;

                if let Some(ref url) = config.url {
                    let allow_private = config
                        .properties
                        .get("allow_private_targets")
                        .and_then(|v| v.as_bool())
                        .unwrap_or(false);
                    // `build_safe_client` validates the URL, resolves DNS
                    // exactly once, validates each resolved IP, and pins
                    // those IPs into the reqwest client so the connect step
                    // cannot be redirected by an attacker-controlled DNS
                    // server (DNS rebinding). Redirects are policed by the
                    // same primitive via a custom Resolve implementation.
                    let client = match build_safe_client(url, allow_private) {
                        Ok((c, _addrs)) => c,
                        Err(reason) => {
                            tracing::warn!(
                                connector_id = %connector_id,
                                url = %url,
                                reason = %reason,
                                "SSRF guard blocked HTTP poller request"
                            );
                            errors_count.fetch_add(1, Ordering::Relaxed);
                            continue;
                        }
                    };
                    // Try real HTTP request via the policy-aware client.
                    match client.get(url).send().await {
                        Ok(resp) => match resp.json::<serde_json::Value>().await {
                            Ok(json) => {
                                let events = HttpPollerConnector::extract_entities(
                                    &json,
                                    &config.entity_type,
                                    &connector_id,
                                    &id_field,
                                    &lat_field,
                                    &lon_field,
                                );
                                for event in events {
                                    if tx.send(event).await.is_err() {
                                        return;
                                    }
                                    events_count.fetch_add(1, Ordering::Relaxed);
                                }
                            }
                            Err(e) => {
                                tracing::warn!("HTTP poller JSON parse error: {}", e);
                                errors_count.fetch_add(1, Ordering::Relaxed);
                            }
                        },
                        Err(e) => {
                            tracing::warn!("HTTP poller request error: {}", e);
                            errors_count.fetch_add(1, Ordering::Relaxed);
                        }
                    }
                } else {
                    // Demo mode: generate synthetic weather data
                    let demo_weather = vec![
                        ("storm-atlantic-1", 45.0, -30.0, "tropical_storm", "warning"),
                        ("low-pressure-north", 58.0, 5.0, "low_pressure", "info"),
                        ("high-pressure-med", 38.0, 15.0, "high_pressure", "info"),
                    ];

                    for (id, lat, lon, sys_type, severity) in &demo_weather {
                        let mut properties = HashMap::new();
                        properties.insert("system_type".to_string(), serde_json::json!(sys_type));
                        properties.insert("severity".to_string(), serde_json::json!(severity));
                        properties.insert("radius_km".to_string(), serde_json::json!(200.0));

                        let event = SourceEvent {
                            connector_id: connector_id.clone(),
                            entity_id: format!("weather:{}", id),
                            entity_type: "weather_system".to_string(),
                            properties,
                            timestamp: Utc::now(),
                            latitude: Some(*lat),
                            longitude: Some(*lon),
                        };

                        if tx.send(event).await.is_err() {
                            return;
                        }
                        events_count.fetch_add(1, Ordering::Relaxed);
                    }
                }
            }
        });

        Ok(())
    }

    async fn stop(&self) -> Result<(), ConnectorError> {
        self.running.store(false, Ordering::SeqCst);
        tracing::info!(
            connector_id = %self.config.connector_id,
            "HTTP poller connector stopped"
        );
        Ok(())
    }

    async fn health_check(&self) -> Result<(), ConnectorError> {
        if self.running.load(Ordering::SeqCst) {
            Ok(())
        } else {
            Err(ConnectorError::ConnectionError(
                "HTTP poller connector not running".to_string(),
            ))
        }
    }

    fn config(&self) -> &ConnectorConfig {
        &self.config
    }

    fn stats(&self) -> ConnectorStats {
        // `Some(Utc::now())` would lie to ops dashboards. Report None
        // until per-event timestamp tracking is wired in.
        ConnectorStats {
            events_processed: self.events_count.load(Ordering::Relaxed),
            errors: self.errors_count.load(Ordering::Relaxed),
            last_event_timestamp: None,
            uptime_seconds: 0,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_extract_entities_array() {
        let json = serde_json::json!([
            {"id": "vessel-1", "latitude": 51.92, "longitude": 4.48, "name": "Ship A"},
            {"id": "vessel-2", "latitude": 52.00, "longitude": 4.50, "name": "Ship B"},
        ]);

        let events = HttpPollerConnector::extract_entities(
            &json,
            "ship",
            "http-1",
            "id",
            "latitude",
            "longitude",
        );
        assert_eq!(events.len(), 2);
        assert_eq!(events[0].entity_id, "ship:vessel-1");
        assert!((events[0].latitude.unwrap() - 51.92).abs() < 0.01);
    }

    #[test]
    fn test_extract_entities_nested() {
        let json = serde_json::json!({
            "data": [
                {"id": "port-1", "latitude": 51.92, "longitude": 4.48},
            ]
        });

        let events = HttpPollerConnector::extract_entities(
            &json,
            "port",
            "http-1",
            "id",
            "latitude",
            "longitude",
        );
        assert_eq!(events.len(), 1);
    }

    #[test]
    fn test_extract_entities_empty() {
        let json = serde_json::json!([]);
        let events =
            HttpPollerConnector::extract_entities(&json, "ship", "http-1", "id", "lat", "lon");
        assert!(events.is_empty());
    }
}
