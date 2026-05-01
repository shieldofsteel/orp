# OIDC / OAuth 2.0 Integration

ORP supports two distinct authentication models for Bearer tokens:

| Mode | Token signing | Validated against | Use case |
|------|---------------|-------------------|----------|
| **OIDC** | RS256 / ES256 / EdDSA | External IdP's JWKS (`jwks_uri`) | Real users, federated identity, SSO |
| **HS256-legacy** | HS256 | Local `JWT_SECRET` shared secret | Internal API tokens, service-to-service, federation peers |

The two modes coexist. When an `OidcValidator` is configured, the
middleware routes by token algorithm:

- `alg=HS256` → legacy local `JwtService` (`JWT_SECRET`)
- `alg=RS256` / `ES256` / `EdDSA` → matched by `iss` against a configured
  OIDC provider, then verified against that provider's JWKS

Tokens with an unrecognised `iss` are **rejected** rather than silently
falling back to the legacy path.

---

## How verification works

When a request arrives with an OIDC-signed Bearer token:

1. `decode_header` extracts `alg` and `kid`
2. The validator looks up the matching `OidcClient` by `iss`
3. The client looks up the JWK by `kid` in its in-memory cache (TTL =
   `ORP_OIDC_DISCOVERY_TTL_SECS`, default 1h)
4. On cache miss, the JWKS is re-fetched once from `jwks_uri`; if the
   `kid` is still missing, the request is rejected with `JwksMissingKid`
5. `jsonwebtoken::decode` validates signature + `iss` + `aud` + `exp`
   (+ `nbf` if present), pinning `alg` to what the JWK declares

Algorithm-confusion is blocked: an HS256-signed token cannot be
smuggled through the OIDC path even if its claims look right.

---

## Configuration

Each OIDC provider needs four values: `provider_url`, `client_id`,
`client_secret`, `redirect_uri`. The `provider_url` is the IdP's issuer
URL (where `${provider_url}/.well-known/openid-configuration` lives).

```yaml
oidc:
  enabled: true
  provider_url: https://accounts.google.com
  client_id: 1234567890-abc.apps.googleusercontent.com
  client_secret: GOCSPX-...
  redirect_uri: https://orp.example.com/auth/callback
  extra_scopes: ["profile", "email"]
```

Multi-IdP setups attach multiple `OidcClient` instances to one
`OidcValidator`; each client primes its own discovery + JWKS cache.

---

## Provider walkthroughs

### Keycloak

1. **Realm**: create a realm (e.g. `orp`) or reuse `master`.
2. **Client**: Clients → Create. Client ID `orp-backend`, Type
   `OpenID Connect`, Access type `confidential`. Save.
3. **Settings**:
   - Valid Redirect URIs: `https://orp.example.com/auth/callback`
   - Web Origins: `+` (or your origin)
4. **Credentials tab**: copy the secret.
5. **Endpoint URL**:
   `https://keycloak.example.com/realms/orp` — this is `provider_url`.
   Discovery is at `.../realms/orp/.well-known/openid-configuration`
   and JWKS at `.../realms/orp/protocol/openid-connect/certs`.
6. **Scopes / claims**: for ORP RBAC, configure the client to add a
   `permissions` claim (Client Scopes → Mappers → Hardcoded claim or
   User Attribute) — values like `entities:read`, `entities:write`,
   `admin`. ORP also accepts a space-separated `scope` claim.
7. **Test**:
   ```bash
   curl -X POST https://keycloak.example.com/realms/orp/protocol/openid-connect/token \
     -d 'grant_type=password' -d 'username=alice' -d 'password=...' \
     -d 'client_id=orp-backend' -d 'client_secret=...'
   ```

### Auth0

1. **Application**: Auth0 dashboard → Applications → Create →
   Regular Web Application.
2. **Settings**: copy Domain, Client ID, Client Secret. Allowed
   Callback URLs: `https://orp.example.com/auth/callback`.
3. **API**: APIs → Create API. Identifier `https://orp.example.com/`
   (this becomes `aud`). Enable RBAC + "Add Permissions in Access Token".
4. Define permissions on the API: `entities:read`, `entities:write`,
   `admin`, etc. Auth0 emits them in the `permissions` array claim.
5. **provider_url**: `https://YOUR_TENANT.auth0.com/` (trailing slash
   matters — that's exactly what Auth0 puts in the `iss` claim).
6. **Token request**:
   ```bash
   curl -X POST https://YOUR_TENANT.auth0.com/oauth/token \
     -H 'content-type: application/json' \
     -d '{"client_id":"...","client_secret":"...","audience":"https://orp.example.com/","grant_type":"client_credentials"}'
   ```

Note: Auth0 omits `nbf` from access tokens. ORP's OIDC validation
accepts this (only `exp`, `iss`, `aud`, `sub` are required).

### Okta

1. **Application**: Okta admin → Applications → Create App Integration
   → OIDC - Web Application.
2. **Sign-in redirect URI**: `https://orp.example.com/auth/callback`.
3. **Authorization Server**: Security → API → Authorization Servers.
   Use `default` or create a custom one.
4. **Scopes / Claims**: add a custom claim mapping `permissions` →
   `Groups` (filter by regex matching your ORP role groups), or use
   `groups` and adjust the ORP RBAC mapping accordingly.
5. **provider_url**: `https://${OKTA_DOMAIN}/oauth2/default` (or your
   custom auth-server path).
6. **Test**:
   ```bash
   curl https://${OKTA_DOMAIN}/oauth2/default/.well-known/openid-configuration | jq .issuer
   ```
   The `issuer` you see here MUST equal `provider_url` exactly —
   otherwise ORP will reject the token's `iss`.

### Azure AD (Microsoft Entra ID)

1. **App registration**: Azure portal → App registrations → New.
2. **Redirect URI** (Web): `https://orp.example.com/auth/callback`.
3. **Certificates & secrets**: New client secret, copy the value.
4. **Expose an API**: set Application ID URI (e.g.
   `api://orp-backend`). Add app roles for ORP permissions.
5. **API permissions**: grant the app role to the calling client.
6. **provider_url**:
   `https://login.microsoftonline.com/${TENANT_ID}/v2.0`
   (the `v2.0` suffix matters — Azure AD has v1 and v2 issuers and
   ORP must match the one your tokens carry in `iss`).
7. **Note on `aud`**: Azure tokens for v2 carry `aud` equal to your
   client ID OR your Application ID URI, depending on how the token
   was requested. ORP validates against the configured `client_id`.

---

## Token claim shape

ORP validates these claims:

| Claim | Required | Notes |
|-------|----------|-------|
| `sub` | yes | Subject — surfaced as `AuthContext.subject` |
| `iss` | yes | Must equal `discovery.issuer` |
| `aud` | yes | Must contain `client_id` (string or array) |
| `exp` | yes | Validated with 60s clock-skew leeway |
| `nbf` | no | Validated when present |
| `iat` | no | Used as fallback when other timestamps missing |
| `permissions` | no | Array of permission strings — used by RBAC |
| `scope` | no | Space-separated; falls back to `permissions` if absent |
| `email`, `name`, `org_id` | no | Surfaced on `AuthContext` |

If your IdP doesn't emit a `permissions` array, ORP will split `scope`
on whitespace and use those entries.

---

## Operational notes

- **JWKS cache TTL**: defaults to 1 hour. Tune with
  `ORP_OIDC_DISCOVERY_TTL_SECS`. On `kid` cache miss, ORP refreshes
  JWKS once before failing — so steady-state key rotation propagates
  without operator action.
- **Discovery max-staleness**: `ORP_OIDC_DISCOVERY_MAX_STALENESS_SECS`
  (default `discovery_ttl × 24`). Beyond this window with refresh
  failing, ORP fails closed rather than trust a possibly-retired key.
- **Algorithm pinning**: only `RS256`, `ES256`, `EdDSA` are accepted on
  the OIDC path. `HS256` is reserved for the legacy local-JWT path.
- **Failure modes**:
  - `kid` not in JWKS → 401 `JwksMissingKid`
  - alg in header doesn't match the JWK → 401 `TokenValidationFailed`
  - `iss` doesn't match any configured provider → 401
  - signature invalid / token expired → 401

---

## Migration from HS256-legacy

You can run both modes simultaneously while migrating:

```rust
let validator = OidcValidator::new()
    .with_provider(idp_client)        // new RS256 path
    .with_legacy_jwt(jwt_service);    // existing HS256 path

let auth_state = AuthState::production_with_oidc(validator.into(), api_keys);
```

HS256 tokens continue to validate against `JWT_SECRET`; OIDC tokens
validate against the IdP's JWKS. Once all clients have migrated, drop
`with_legacy_jwt` and stop minting HS256 tokens.
