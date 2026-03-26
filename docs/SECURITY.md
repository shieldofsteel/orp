# ORP Security Architecture

This document describes ORP's security model in detail: authentication, authorization, event integrity, audit logging, key management, rate limiting, and CORS policy.

**For reporting a vulnerability,** see [Reporting Vulnerabilities](#reporting-vulnerabilities).

---

## Table of Contents

1. [Security Philosophy](#security-philosophy)
2. [OIDC Authentication Flow](#oidc-authentication-flow)
3. [ABAC Authorization Model](#abac-authorization-model)
4. [Ed25519 Event Signing Chain](#ed25519-event-signing-chain)
5. [Audit Log Hash Verification](#audit-log-hash-verification)
6. [API Key Scoping](#api-key-scoping)
7. [Rate Limiting](#rate-limiting)
8. [CORS Policy](#cors-policy)
9. [TLS Configuration](#tls-configuration)
10. [Secrets Management](#secrets-management)
11. [Cryptographic Erasure (GDPR)](#cryptographic-erasure-gdpr)
12. [No Telemetry Guarantee](#no-telemetry-guarantee)
13. [Dependency Security](#dependency-security)
14. [Reporting Vulnerabilities](#reporting-vulnerabilities)

---

## Security Philosophy

ORP is designed for high-stakes environments — disaster response, maritime operations, supply chain monitoring, defense research. Its security model reflects this:

- **Secure by default.** OIDC auth, ABAC enforcement, event signing, and audit logging are enabled by default. Disabling any of these requires an explicit config change.
- **No implicit trust.** Every API call is authenticated and authorized. Every connector event is signed. Data provenance is always verifiable.
- **Zero telemetry.** ORP does not make any outbound network requests except those you explicitly configure (connectors, OIDC endpoints). No phone-home, no usage analytics.
- **Defense in depth.** Auth failure → 401. Auth success but ABAC deny → 403. ABAC success but data not found → 404. Each layer fails independently.

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

On each request, ORP:

1. Extracts the Bearer token from `Authorization` header (or `orp_session` cookie for browser requests)
2. Decodes the JWT header to get the `kid` (key ID)
3. Fetches the JWKS from `<issuer>/.well-known/jwks.json` (cached in memory, refreshed every hour)
4. Verifies the JWT signature using the matching public key
5. Validates claims: `iss` matches config, `aud` contains `orp-console`, `exp` is in the future
6. Extracts claims for ABAC evaluation: `sub`, `email`, `groups`, custom claims

Token validation adds < 1 ms overhead (cached key lookup + in-memory HMAC/RSA verify).

### Token Refresh

- ORP does not store refresh tokens server-side. Token refresh is the client's responsibility.
- When a token expires, the API returns `401 Unauthorized` with `WWW-Authenticate: Bearer error="invalid_token"`.
- The frontend silently refreshes using the OIDC client library.

---

## ABAC Authorization Model

ORP uses Attribute-Based Access Control (ABAC). Every data access is evaluated against a policy engine that considers the caller's attributes, the resource's attributes, and environmental context.

### Policy Evaluation

```
┌──────────────────────────────────────────────────────────────┐
│  Policy Decision Point (PDP)                                 │
│                                                              │
│  Input:                                                      │
│  ┌─────────────────────┐  ┌─────────────────────┐           │
│  │  Subject Attributes  │  │ Resource Attributes  │           │
│  │  ─────────────────── │  │ ─────────────────── │           │
│  │  user.id             │  │ entity.type          │           │
│  │  user.email          │  │ entity.sensitivity   │           │
│  │  user.org_id         │  │ entity.tags          │           │
│  │  user.permissions[]  │  │ entity.org_id        │           │
│  │  user.clearance      │  │ entity.classification│           │
│  └─────────────────────┘  └─────────────────────┘           │
│                                                              │
│  ┌─────────────────────┐                                    │
│  │ Environment Attrs   │                                    │
│  │ ─────────────────── │                                    │
│  │ time.utc            │                                    │
│  │ request.ip          │                                    │
│  │ request.path        │                                    │
│  └─────────────────────┘                                    │
│                                                              │
│  Policy (evaluated in order; first match wins):              │
│  1. DENY: user.is_suspended = true                           │
│  2. ALLOW: user.permissions CONTAINS "admin"                 │
│  3. ALLOW: user.permissions CONTAINS "entities:read"         │
│         AND entity.sensitivity IN ["public", "internal"]    │
│         AND (entity.org_id = user.org_id                    │
│              OR user.permissions CONTAINS "cross-org:read") │
│  4. DENY: (implicit default deny)                           │
└──────────────────────────────────────────────────────────────┘
```

### Standard Permission Strings

| Permission | Grants Access To |
|------------|----------------|
| `entities:read` | List and get entities |
| `entities:write` | Create and update entities |
| `entities:delete` | Soft-delete entities |
| `graph:read` | Graph traversal queries |
| `query:execute` | Run ORP-QL queries |
| `query:natural` | Natural language queries (Phase 2) |
| `connectors:read` | List connectors and metrics |
| `connectors:manage` | Register and deregister connectors |
| `monitors:read` | List monitors and alerts |
| `monitors:manage` | Create and delete monitors |
| `audit:read` | Read audit log entries |
| `admin` | All of the above + user management |
| `cross-org:read` | Read entities from all organizations |

### ABAC in Code

Every handler in `orp-core` calls the ABAC enforcer before returning data:

```rust
// Example: entity list handler
async fn list_entities(
    State(state): State<AppState>,
    AuthContext(claims): AuthContext,
    Query(params): Query<EntityListParams>,
) -> Result<Json<Vec<Entity>>, ApiError> {
    // ABAC: require entities:read permission
    state.abac.require(&claims, "entities:read")?;  // → 403 if denied

    let entities = state.storage.list_entities(&params).await?;

    // Filter result set to only entities the caller can see
    // (defense in depth: even if a bug in query params slips through,
    //  the ABAC filter on the result set catches it)
    let filtered = state.abac.filter_entities(entities, &claims);

    Ok(Json(filtered))
}
```

---

## Ed25519 Event Signing Chain

Every event produced by a connector is signed before entering the stream processor. This creates an unbroken chain of custody from data source to storage.

### Key Generation (per connector)

```bash
# Generate a signing keypair for a connector
orp keygen --output ~/.orp/keys/ais-connector.key

# Public key is registered in DuckDB data_sources table on connector startup
# Private key stays on the machine running the connector; never transmitted
```

### Signing Algorithm

```
1. Connector produces an OrpEvent with all fields populated

2. Serialization (deterministic):
   fields = sort_keys_alphabetically({
     entity_id: "mmsi:123456789",
     event_type: "position_update",
     source_id: "ais-global",
     timestamp: "2026-03-26T09:30:00.000Z",
     payload: { lat: 51.92, lon: 4.47, speed: 14.2, course: 275 }
   })
   canonical_bytes = utf8(json_serialize(fields))

3. Sign:
   signature = ed25519_sign(private_key, sha256(canonical_bytes))
   event.signature = base64url(signature)
   event.signer_key_id = key_fingerprint(public_key)

4. Attach and forward to stream processor
```

### Verification

```
Verifier receives event:
1. Load public_key from data_sources WHERE key_id = event.signer_key_id
2. Re-serialize event fields using same deterministic algorithm
3. signature_valid = ed25519_verify(public_key, sha256(canonical_bytes), base64url_decode(event.signature))
4. If invalid:
   - Log warning: "Signature verification failed for event {event_id} from {source_id}"
   - Set event.confidence = 0.0 (data still stored, but flagged)
   - Increment connector metrics: signature_failures++
   - Do NOT drop the event (operator decides what to do with low-confidence data)
```

### Verification CLI

```bash
# Verify all events from a connector are properly signed
orp verify --connector ais-global --since "2026-03-25T00:00:00Z"
# ✓ Verified 2,847,392 events. 0 signature failures.

# Verify a specific event by ID
orp verify --event-id "01HWKX5EQVP3NBYJ4ZK8DJFRTE"
# ✓ Event signature valid. Signed by key: abc123def456 (ais-global)
```

---

## Audit Log Hash Verification

The audit log is append-only and hash-chained. Every entry includes the SHA-256 of the previous entry. Tampering with any historical entry invalidates all subsequent hashes.

### Schema

```sql
CREATE TABLE audit_log (
    seq_id      BIGINT PRIMARY KEY,       -- monotonically increasing
    timestamp   TIMESTAMPTZ NOT NULL,
    actor       TEXT NOT NULL,            -- "user:alice@example.com" or "connector:ais-global"
    action      TEXT NOT NULL,            -- "query_executed", "entity_created", "login", etc.
    target_type TEXT,                     -- "entities", "connectors", "monitors", NULL
    target_id   TEXT,                     -- entity/connector/monitor ID, NULL
    details     JSON,                     -- action-specific details
    prev_hash   TEXT NOT NULL,            -- SHA-256 of (seq_id-1) row
    hash        TEXT NOT NULL             -- SHA-256 of (seq_id || actor || action || details || prev_hash)
);

-- Genesis entry (seq_id = 0) has prev_hash = '0000...0000' (64 zeroes)
```

### Chain Integrity Verification

```bash
# Verify the full audit log chain
orp verify --audit-log ~/.orp/data/audit.db

# Output:
# Checking audit log integrity...
# ✓ Entry 0: genesis (prev_hash = 0000...0000)
# ✓ Entry 1 – 10,000: chain valid
# ✓ Entry 10,001 – 42,891: chain valid
# ✓ Audit log integrity verified: 42,891 entries, chain unbroken.
# Last entry: 2026-03-26T09:30:14Z | actor: user:alice@example.com | action: query_executed

# If tampering is detected:
# ✗ Chain break at entry 8,234:
#   Expected prev_hash: a3f2b1...
#   Actual prev_hash:   00000...
#   This entry or entry 8,233 has been modified.
```

### What Is Logged

| Action | Logged When |
|--------|-------------|
| `login` | User successfully authenticates |
| `login_failed` | Authentication attempt fails |
| `query_executed` | Any ORP-QL query is run (query text + result count) |
| `entity_created` | Entity manually created via API |
| `entity_updated` | Entity properties updated via API |
| `entity_deleted` | Entity soft-deleted |
| `connector_registered` | New connector added |
| `connector_deregistered` | Connector removed |
| `monitor_created` | Monitor rule created |
| `alert_acknowledged` | Alert acknowledged |
| `cryptographic_erasure` | Entity encryption key destroyed |
| `audit_log_accessed` | Audit log queried (meta-audit) |

---

## API Key Scoping

For non-interactive clients (CI pipelines, scripts, SDKs), ORP supports API keys as an alternative to OIDC tokens.

### Key Structure

```
orp_<environment>_<base58-random-32-bytes>
```

Examples:
- `orp_prod_3xK9mPqR7vL3nW8sJd4Qb...`
- `orp_dev_8mNpXyZ2vKj6Lw9qRt1Au...`

### Scoping

API keys are created with explicit permission scopes and an optional expiry:

```bash
# Create an API key with read-only access
orp apikey create \
  --name "ci-read-only" \
  --permissions "entities:read,query:execute" \
  --expires "2027-01-01" \
  --org-id "org-456"

# Output:
# API Key created: orp_prod_3xK9mPqR7vL3nW8sJd4Qb...
# Permissions: entities:read, query:execute
# Org: org-456
# Expires: 2027-01-01
# (This key is shown only once. Store it securely.)

# List keys (shows fingerprint only, never the raw key)
orp apikey list

# Revoke a key
orp apikey revoke --fingerprint abc123...
```

### Key Rotation

API keys do not rotate automatically. Key rotation is the operator's responsibility. Recommended practices:

- Set expiry on all keys (maximum 1 year)
- Rotate keys when team members leave
- Use separate keys per service / integration
- Monitor `audit_log` for unexpected usage patterns

---

## Rate Limiting

ORP uses a token bucket rate limiter (per-client IP or API key) implemented in Tower middleware.

### Default Limits

| Client Type | Requests / minute | Burst |
|------------|------------------|-------|
| Unauthenticated | 30 | 10 |
| Authenticated (user) | 600 | 100 |
| Authenticated (API key) | 1,200 | 200 |
| Admin | Unlimited | — |

### Configuration

```yaml
server:
  rate_limit:
    enabled: true
    unauthenticated:
      requests_per_minute: 30
      burst: 10
    authenticated_user:
      requests_per_minute: 600
      burst: 100
    authenticated_api_key:
      requests_per_minute: 1200
      burst: 200
    # Bypass for specific IPs (e.g. internal services)
    bypass_ips:
      - "10.0.0.0/8"
      - "172.16.0.0/12"
```

### Rate Limit Headers

Responses include standard rate limit headers:

```
X-RateLimit-Limit: 600
X-RateLimit-Remaining: 487
X-RateLimit-Reset: 1743000660
Retry-After: 23   (only on 429 responses)
```

---

## CORS Policy

### Default Policy

ORP's default CORS policy is **restrictive** (appropriate for production):

```yaml
server:
  cors_origins:
    - "http://localhost:9090"   # ORP's own frontend
```

All cross-origin requests from unlisted origins are rejected with `403 Forbidden`.

### Custom Origins

Add origins for external frontends, dashboards, or integrations:

```yaml
server:
  cors_origins:
    - "http://localhost:9090"
    - "https://dashboard.yourdomain.com"
    - "https://ops.yourdomain.com"
```

**Do not use `*` in production.** Wildcard origins allow any website to make credentialed requests to your ORP instance.

### CORS Headers Sent

```
Access-Control-Allow-Origin: https://dashboard.yourdomain.com
Access-Control-Allow-Methods: GET, POST, PATCH, DELETE, OPTIONS
Access-Control-Allow-Headers: Authorization, Content-Type, X-Request-ID
Access-Control-Allow-Credentials: true
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

### Supported Secret Backends (Phase 2)

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
1. Verify the caller has entities:delete permission
2. Retrieve and decrypt the entity's DEK from the key store
3. Securely destroy the DEK (overwrite in memory, delete from key store)
4. The entity record remains in DuckDB with encrypted fields
5. Without the DEK, the ciphertext is permanently unrecoverable
6. Log erasure event in audit log: action="cryptographic_erasure", target_id={id}

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

_ORP Security Architecture · v0.1.0 · 2026-03-26_
