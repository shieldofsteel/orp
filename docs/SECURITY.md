# ORP Security Architecture

This document describes ORP's security model in detail: authentication, authorization, event integrity, audit logging, key management, rate limiting, and CORS policy.

**For reporting a vulnerability,** see [Reporting Vulnerabilities](#reporting-vulnerabilities).

---

## Table of Contents

1. [Security Philosophy](#security-philosophy)
2. [OIDC Authentication Flow](#oidc-authentication-flow)
3. [ABAC Authorization Model](#abac-authorization-model)
4. [Ed25519 Audit Log Signing](#ed25519-audit-log-signing)
5. [Audit Log Hash Chaining](#audit-log-hash-chaining)
6. [API Key Scoping](#api-key-scoping)
7. [Rate Limiting](#rate-limiting)
8. [CORS Policy](#cors-policy)
9. [TLS Configuration](#tls-configuration)
10. [Secrets Management](#secrets-management)
11. [Cryptographic Erasure (GDPR)](#cryptographic-erasure-gdpr)
12. [No Telemetry Guarantee](#no-telemetry-guarantee)
13. [Dependency Security](#dependency-security)
14. [Audit History & Remediation](#audit-history--remediation)
15. [Reporting Vulnerabilities](#reporting-vulnerabilities)

---

## Security Philosophy

ORP is designed for high-stakes environments — disaster response, maritime operations, supply chain monitoring, defense research. Its security model reflects this:

- **Secure by default.** OIDC auth, ABAC enforcement, Ed25519-signed audit logs, and hash-chained audit entries are enabled by default. Disabling any of these requires an explicit config change.
- **No implicit trust.** Every API call is authenticated and authorized. The audit log records every action and is cryptographically signed and hash-chained.
- **Zero telemetry.** ORP does not make any outbound network requests except those you explicitly configure (connectors, OIDC endpoints). No phone-home, no usage analytics.
- **Defense in depth.** Auth failure → 401. Auth success but ABAC deny → 403. ABAC success but data not found → 404. Each layer fails independently.
- **No panics on malformed input.** All parser paths use safe Rust (no `unwrap()`/`expect()` on untrusted data). Malformed protocol messages are logged and discarded — the process never crashes.

---

## OIDC Authentication Flow

ORP supports any OIDC-compatible identity provider: Keycloak, Auth0, Dex, Okta, Google, Microsoft Entra ID.

### Browser Authentication Flow

```
Browser                        ORP Server                    OIDC Provider
   │                               │                               │
   │  GET /                         │                               │
   ├──────────────────────────────► │                               │
   │  302 → /auth/login             │                               │
   │ ◄────────────────────────────── │                               │
   │                               │                               │
   │  GET /auth/login               │                               │
   ├──────────────────────────────► │                               │
   │  302 → <issuer>/authorize?     │                               │
   │         client_id=orp-console  │                               │
   │         response_type=code     │                               │
   │         scope=openid+profile   │                               │
   │         state=<csrf-nonce>     │                               │
   │         redirect_uri=...       │                               │
   │ ◄────────────────────────────────────────────────────────────── │
   │                                                                 │
   │  User enters credentials at IdP                                 │
   ├─────────────────────────────────────────────────────────────── ►│
   │                                                                 │
   │  302 → /auth/callback?code=<auth_code>&state=<nonce>           │
   │ ◄─────────────────────────────────────────────────────────────── │
   │                               │                               │
   │  GET /auth/callback?code=...   │                               │
   ├──────────────────────────────► │                               │
   │                               │  POST <issuer>/token          │
   │                               │  grant_type=authorization_code │
   │                               │  code=<auth_code>             │
   │                               ├──────────────────────────────► │
   │                               │  { access_token, id_token,    │
   │                               │    refresh_token, expires_in } │
   │                               │ ◄─────────────────────────────  │
   │                               │                               │
   │                               │  Validate id_token JWT:       │
   │                               │  · Verify RS256 signature     │
   │                               │  · Check iss, aud, exp, iat   │
   │                               │  · Extract user claims        │
   │                               │                               │
   │  Set-Cookie: orp_session=...   │                               │
   │  (httpOnly, Secure, SameSite=Lax)                              │
   │ ◄────────────────────────────── │                               │
   │                               │                               │
   │  Subsequent requests:          │                               │
   │  Authorization: Bearer <access_token>                          │
   ├──────────────────────────────► │                               │
   │                               │  Verify JWT signature         │
   │                               │  (cached JWKS, refresh 1h)    │
   │                               │                               │
   │  200 + data                    │                               │
   │ ◄────────────────────────────── │                               │
```

### JWT Validation

On each request, the auth middleware (implemented in `orp-security/src/middleware.rs`) resolves credentials in this order:

1. Checks for a pre-injected `AuthContext` in request extensions (set by the `inject_auth_state` middleware layer)
2. Extracts a Bearer token from `Authorization: Bearer <token>` header
3. Falls back to `X-API-Key: <key>` header for programmatic clients
4. If `ORP_DEV_MODE=true`, falls through to anonymous dev context (admin permissions — **never use in production**)
5. Otherwise returns `401 Unauthorized`

When a Bearer token is present, ORP:

1. Passes the token to `JwtService::validate_token`
2. Verifies the JWT signature against the configured JWKS
3. Validates claims: `iss` matches config, `aud` contains `orp-client`, `exp` is in the future
4. Extracts claims for ABAC evaluation: `sub`, `email`, `org_id`, `permissions`, `scope`

If the token is expired, the server returns `401 Unauthorized` with `WWW-Authenticate: Bearer error="invalid_token"`. Token refresh is the client's responsibility.

### Development Mode

Set `ORP_DEV_MODE=true` to bypass authentication entirely. All requests receive a full `admin`-permission context. **This must never be set in production.**

---

## ABAC Authorization Model

ORP uses Attribute-Based Access Control (ABAC) implemented in `orp-security/src/abac.rs`. Every data access is evaluated against a policy engine that considers the caller's attributes, the resource's attributes, and policy rules.

### Policy Evaluation Algorithm

```
┌──────────────────────────────────────────────────────────────┐
│  AbacEngine::evaluate(ctx: &EvaluationContext)               │
│                                                              │
│  Input:                                                      │
│  ┌─────────────────────┐  ┌─────────────────────┐           │
│  │  Subject Attributes  │  │ Resource Attributes  │           │
│  │  ─────────────────── │  │ ─────────────────── │           │
│  │  subject.sub         │  │ resource.type        │           │
│  │  subject.permissions │  │ resource.id          │           │
│  │  subject.role        │  │ resource.attributes  │           │
│  │  subject.org_id      │  │   (sensitivity,      │           │
│  │  subject.attributes  │  │    owner_id, tags)   │           │
│  └─────────────────────┘  └─────────────────────┘           │
│                                                              │
│  Algorithm (deny-overrides, default-deny):                  │
│  1. Fast-path: admin token → check explicit denies,         │
│     then ALLOW (admin bypasses permission check)            │
│  2. Check subject.permissions contains ctx.action           │
│     (missing permission → immediate DENY)                   │
│  3. Iterate policies sorted by priority (desc):             │
│     · First DENY match → immediate DENY                     │
│     · Track first ALLOW match                               │
│  4. ALLOW if any policy matched; DENY if none               │
│                                                              │
│  Variable interpolation:                                    │
│  ${subject.sub}, ${subject.org_id} supported in             │
│  resource attribute conditions                              │
└──────────────────────────────────────────────────────────────┘
```

### Standard Permission Strings

The following permissions are defined in `orp-security/src/abac.rs` (the `Permission` enum):

| Permission | Scope String | Grants Access To |
|------------|-------------|----------------|
| `EntitiesRead` | `entities:read` | List and get entities |
| `EntitiesWrite` | `entities:write` | Create and update entities |
| `EntitiesDelete` | `entities:delete` | Soft-delete entities |
| `GraphRead` | `graph:read` | Graph traversal queries |
| `GraphWrite` | `graph:write` | Create graph relationships |
| `MonitorsRead` | `monitors:read` | List monitors and alerts |
| `MonitorsWrite` | `monitors:write` | Create and delete monitors |
| `QueryExecute` | `query:execute` | Run ORP-QL queries |
| `ConnectorsManage` | `connectors:manage` | Register and deregister connectors |
| `ApiKeysManage` | `api-keys:manage` | Create and revoke API keys |
| `Admin` | `admin` | All of the above + bypasses policy checks |

### Production Default Policies

`AbacEngine::default_production()` registers three starter policies:

1. **admin-allow-all** (priority 100): Users with `role=admin` may perform any action on any resource
2. **entities-read** (priority 0): Users may read `entity` resources
3. **deny-secret-resources** (priority 50): Any user is denied access to resources with `sensitivity=secret`

The deny-secret-resources policy demonstrates deny-override semantics: even if a user has `entities:read`, attempting to access a `sensitivity=secret` entity returns `403 Forbidden`.

### ABAC in Code

Every handler in `orp-core/src/server/handlers.rs` calls the `check_abac` helper before touching storage:

```rust
// check_abac helper — called at the top of every handler
fn check_abac(
    abac: &AbacEngine,
    auth: &AuthContext,
    action: &str,
    resource_type: &str,
    resource_id: &str,
) -> Result<(), (StatusCode, Json<ErrorResponse>)> {
    let ctx = EvaluationContext {
        subject: Subject {
            sub: auth.subject.clone(),
            permissions: auth.permissions.clone(),
            role: if auth.has_permission("admin") {
                Some("admin".to_string())
            } else {
                None
            },
            org_id: auth.org_id.clone(),
            attributes: HashMap::new(),
        },
        action: action.to_string(),
        resource: Resource {
            r#type: resource_type.to_string(),
            id: resource_id.to_string(),
            attributes: HashMap::new(),
        },
    };
    let decision = abac.evaluate(&ctx);
    if decision.result == EvaluationResult::Deny {
        return Err(error_response("FORBIDDEN", StatusCode::FORBIDDEN,
            &format!("Access denied: {}", decision.reason)));
    }
    Ok(())
}
```

---

## Ed25519 Audit Log Signing

ORP uses Ed25519 (via `ed25519-dalek`) to cryptographically sign audit log entries, providing tamper evidence at the cryptographic level in addition to the hash chain.

### Implementation

Implemented in `orp-audit/src/crypto.rs`:

```rust
pub struct EventSigner {
    signing_key: SigningKey,   // ed25519_dalek::SigningKey
}

impl EventSigner {
    /// Sign arbitrary data. Returns a 64-byte signature.
    pub fn sign(&self, data: &[u8]) -> Vec<u8> { ... }

    /// Verify a signature. Returns false for any invalid or malformed input.
    pub fn verify(&self, data: &[u8], signature: &[u8]) -> bool {
        if signature.len() != 64 { return false; } // NaN-safe guard
        ...
    }
}
```

### Server Integration

The HTTP server (`orp-core/src/server/http.rs`) creates one `EventSigner` per process at startup and holds it in `AppState.audit_signer`:

```rust
let audit_signer = config
    .audit_signer
    .unwrap_or_else(|| Arc::new(EventSigner::new()));
```

The `audit_log` helper in `handlers.rs` signs each new audit entry's `content_hash` with the server's Ed25519 private key. The resulting signature is stored alongside the entry, giving operators a mechanism to verify that an audit record was written by a legitimate ORP process and not injected externally.

### Per-Connector Event Signing (Roadmap)

A per-connector signing model (individual Ed25519 keypairs per data source, registered in the `data_sources` table) is planned for a future release. The current implementation signs audit log entries at the server level.

---

## Audit Log Hash Chaining

The audit log is append-only and hash-chained. Every entry includes a SHA-256 hash of the previous entry's `content_hash`. Tampering with any historical entry invalidates all subsequent hashes.

### Rust Schema (`orp-audit/src/logger.rs`)

```rust
pub struct AuditEntry {
    pub sequence_number: u64,           // monotonically increasing (starts at 1)
    pub timestamp: DateTime<Utc>,
    pub operation: String,              // "entity_created", "query_executed", etc.
    pub entity_type: Option<String>,    // "ship", "aircraft", etc. — may be None
    pub entity_id: Option<String>,      // entity/connector/monitor ID — may be None
    pub user_id: Option<String>,        // "user:alice@example.com" or "connector:ais-global"
    pub details: serde_json::Value,     // action-specific JSON payload
    pub previous_hash: String,          // content_hash of (sequence_number - 1) entry
    pub content_hash: String,           // SHA-256 of (seq||operation||timestamp||details)
}
```

**Genesis entry** (`sequence_number = 1`): `previous_hash = "genesis"`.

### Hash Formula

```
content_hash = sha256_hex(
    format!("{}||{}||{}||{}", sequence_number, operation, timestamp.rfc3339(), details)
)
```

### Chain Integrity Verification

```rust
/// AuditLog::verify() — validates the full chain in O(n)
pub fn verify(&self) -> bool {
    for (i, entry) in self.entries.iter().enumerate() {
        // Check previous hash linkage
        if i == 0 {
            if entry.previous_hash != "genesis" { return false; }
        } else if entry.previous_hash != self.entries[i - 1].content_hash {
            return false;
        }
        // Re-compute and compare content hash
        let expected = sha256_hex(format!("{}||{}||{}||{}",
            entry.sequence_number, entry.operation,
            entry.timestamp.to_rfc3339(), entry.details));
        if entry.content_hash != expected { return false; }
    }
    true
}
```

### CLI Verification

```bash
# Verify the full audit log chain
orp verify --audit-log ~/.orp/data/audit.db

# If tampering is detected:
# ✗ Chain break at entry 8,234:
#   Expected prev_hash: a3f2b1...
#   Actual prev_hash:   00000...
#   This entry or entry 8,233 has been modified.
```

### What Is Logged

| Operation | When |
|-----------|------|
| `login` | User successfully authenticates |
| `login_failed` | Authentication attempt fails |
| `query_executed` | Any ORP-QL query is run |
| `entity_created` | Entity created via API |
| `property_updated` | Entity properties updated |
| `entity_deleted` | Entity soft-deleted |
| `connector_registered` | New connector added |
| `connector_deregistered` | Connector removed |
| `monitor_created` | Monitor rule created |
| `alert_acknowledged` | Alert acknowledged |
| `cryptographic_erasure` | Entity encryption key destroyed |

---

## API Key Scoping

For non-interactive clients (CI pipelines, scripts, SDKs), ORP supports API keys via the `X-API-Key` header.

API keys are validated by `ApiKeyService` (`orp-security/src/api_keys.rs`). Validation checks:
1. Key exists in the key store
2. Key is not expired (`is_expired = false`)
3. Key is not revoked (`is_revoked = false`)

On success, an `AuthContext` is built with the key's scopes as permissions, feeding the same ABAC evaluation path as JWT auth.

### Key Management

```bash
# Create an API key with read-only access
orp apikey create \
  --name "ci-read-only" \
  --permissions "entities:read,query:execute" \
  --expires "2027-01-01" \
  --org-id "org-456"

# List keys (fingerprint only — raw key is never shown after creation)
orp apikey list

# Revoke a key
orp apikey revoke --fingerprint abc123...
```

### Key Rotation

API keys do not rotate automatically. Recommended practices:

- Set expiry on all keys (maximum 1 year)
- Rotate keys when team members leave
- Use separate keys per service / integration
- Monitor `audit_log` for unexpected usage patterns

---

## Rate Limiting

ORP implements a **token bucket rate limiter** per client IP in `orp-core/src/server/http.rs` as an Axum middleware layer.

### Implementation

```rust
// Configured at server startup — applies to ALL clients uniformly
let rate_limiter = RateLimiter::new(100, 100); // max_tokens=100, refill_rate=100/sec
```

The token bucket holds up to **100 tokens** and refills at **100 tokens/second**. This translates to a sustained throughput of 100 requests/second per IP with a burst capacity of 100 additional requests.

**Note:** The rate limiter currently applies uniformly regardless of authentication state. Per-tier limits (differentiated by auth level) are planned for a future release.

### Rate Limit Response

When the bucket is empty, ORP responds `429 Too Many Requests`:

```json
{
  "error": {
    "code": "RATE_LIMITED",
    "status": 429,
    "message": "Too many requests. Please retry later.",
    "retry_after_seconds": 1,
    "timestamp": "2026-03-26T09:30:00Z"
  }
}
```

With a `Retry-After: 1` HTTP header.

### IP Extraction

The middleware reads the client IP from:
1. `X-Forwarded-For` header (first entry — for reverse proxy deployments)
2. TCP `ConnectInfo<SocketAddr>` (direct connections)
3. Falls back to `"unknown"` (counts against a single shared bucket)

### Bypass for Internal Services

Internal services on private subnets should be placed behind a reverse proxy that strips or overrides `X-Forwarded-For`, or use dedicated bypass logic in your infrastructure.

### Configuration

```yaml
server:
  rate_limit:
    enabled: true
    max_tokens: 100
    refill_rate: 100   # tokens per second
```

---

## CORS Policy

### Default Policy

ORP's default CORS policy reads allowed origins from the `ORP_CORS_ORIGINS` environment variable (comma-separated). If unset, it defaults to `http://localhost:3000`.

CORS is **never** configured as a wildcard (`*`) — `AllowOrigin::list()` is used exclusively. All unlisted origins receive `403 Forbidden`.

```yaml
server:
  cors_origins:
    - "http://localhost:9090"   # ORP's own frontend
```

### Custom Origins

```yaml
server:
  cors_origins:
    - "http://localhost:9090"
    - "https://dashboard.yourdomain.com"
    - "https://ops.yourdomain.com"
```

### CORS Headers Sent

```
Access-Control-Allow-Origin: https://dashboard.yourdomain.com
Access-Control-Allow-Methods: GET, POST, PUT, PATCH, DELETE, OPTIONS
Access-Control-Allow-Headers: *
Access-Control-Max-Age: 86400
```

---

## TLS Configuration

ORP does not terminate TLS by default. In production, place a reverse proxy (Nginx, Caddy, Cloudflare) in front of ORP.

For direct TLS termination:

```yaml
security:
  tls:
    enabled: true
    cert_path: "${env.ORP_TLS_CERT_PATH}"
    key_path: "${env.ORP_TLS_KEY_PATH}"
    min_version: "TLS1.3"   # ORP requires TLS 1.2+, defaults to TLS 1.3
```

**Cipher suites:** ORP delegates cipher selection to the Rust `rustls` library, which supports only safe, modern suites (TLS 1.3: AES-256-GCM-SHA384, AES-128-GCM-SHA256, CHACHA20-POLY1305-SHA256).

---

## Secrets Management

ORP never stores secrets in config files. All sensitive values use environment variable substitution:

```yaml
auth:
  oidc:
    client_secret: "${env.OIDC_CLIENT_SECRET}"   # ✅ correct

# NOT this:
auth:
  oidc:
    client_secret: "mysecretvalue"   # ✗ never do this
```

### Supported Secret Backends

| Backend | Status |
|---------|--------|
| Environment variables | ✅ Available now |
| File path (`${file:/run/secrets/oidc_secret}`) | 🗓️ Phase 2 |
| HashiCorp Vault | 🗓️ Phase 2 |
| AWS Secrets Manager | 🗓️ Phase 2 |

---

## Cryptographic Erasure (GDPR)

ORP supports GDPR Article 17 (right to erasure) via cryptographic erasure — destroying the encryption key rather than the ciphertext.

### How It Works

```
Entity creation:
1. Generate a random 256-bit DEK (Data Encryption Key) for the entity
2. Encrypt sensitive entity properties with AES-256-GCM using the DEK
3. Encrypt the DEK with the master key (stored in OS keychain or Vault)
4. Store the encrypted DEK alongside the entity in DuckDB
5. The plaintext DEK is never written to disk

Erasure request (DELETE /api/v1/entities/{id}?erasure=cryptographic):
1. Verify the caller has entities:delete permission (ABAC enforced)
2. Retrieve and decrypt the entity's DEK from the key store
3. Securely destroy the DEK (overwrite in memory, delete from key store)
4. The entity record remains in DuckDB with encrypted fields
5. Without the DEK, the ciphertext is permanently unrecoverable
6. Log erasure in audit log: operation="cryptographic_erasure", entity_id={id}

After erasure:
- The entity still appears in queries (with null sensitive fields)
- Historical events referencing the entity are preserved (non-sensitive)
- The audit log records that erasure occurred (provable compliance)
```

---

## No Telemetry Guarantee

ORP makes **zero unsolicited outbound network connections**.

The binary will only make outbound connections if you explicitly configure:
- Connector hosts (AIS, ADS-B, MQTT, HTTP polling)
- OIDC issuer URL (for JWKS fetch)
- Audit log export endpoint (if configured)

This is verifiable: build ORP with `RUSTFLAGS="-D warnings"` and inspect all `reqwest` / `tokio::net` call sites. There are none in the binary itself outside of explicit connector and auth code.

---

## Dependency Security

ORP audits dependencies on every CI run:

```bash
cargo audit   # checks against RustSec Advisory Database
```

**Policy:**
- No `RUSTSEC-*` advisories with severity ≥ Medium are allowed in CI
- Dependencies are pinned with `Cargo.lock` committed to the repository
- Dependency updates are batched weekly via automated PRs

---

## Audit History & Remediation

ORP has undergone four documented security and correctness audits. All findings have been remediated. See [AUDIT_HISTORY.md](AUDIT_HISTORY.md) for full details.

**Current state (post-remediation):**
- 960 tests passing, zero clippy warnings
- Zero `unwrap()`/`expect()` calls on untrusted data paths
- All floating-point operations are NaN-safe
- Sentinel/magic-value filtering removed from all parsers
- NMEA, AIS, ASTERIX, Modbus, and DNP3 parsers verified against their respective specifications
- All audit log entries are Ed25519-signed and hash-chained

---

## Reporting Vulnerabilities

**Do not open a public GitHub issue for security vulnerabilities.**

Report vulnerabilities to: **security@orp.dev**

Please include:
- Description of the vulnerability
- Steps to reproduce
- Estimated severity (CVSS if known)
- Whether you believe it is being actively exploited

We will:
- Acknowledge your report within 24 hours
- Provide an estimated fix timeline within 72 hours
- Credit you in the security advisory (unless you prefer anonymity)
- Issue a CVE for confirmed vulnerabilities of significant severity

We do not have a formal bug bounty program, but we deeply appreciate responsible disclosure.

---

_ORP Security Architecture · v0.2.0 · 2026-03-27_
