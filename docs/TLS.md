# TLS for the ORP HTTP Server

ORP terminates TLS with [rustls](https://github.com/rustls/rustls) via the
`axum-server` crate. There is **no OpenSSL** in the build — everything is pure
Rust, ring-backed, and FIPS-friendly when `rustls` is configured for it.

This document covers the three deployment paths:

1. [Local development with a self-signed cert](#1-local-development)
2. [Production with Let's Encrypt](#2-lets-encrypt)
3. [Corporate / private PKI](#3-corporate-pki)

Plus mTLS, HSTS, and the HTTP→HTTPS redirector.

---

## TL;DR

```bash
# Dev: generate a self-signed cert and start HTTPS
orp gen-cert
orp start --tls-cert cert.pem --tls-key key.pem
# → https://localhost:9443/

# Production with Let's Encrypt
orp start \
  --tls-cert /etc/letsencrypt/live/orp.example.com/fullchain.pem \
  --tls-key  /etc/letsencrypt/live/orp.example.com/privkey.pem \
  --port 443 \
  --redirect-http 80

# Zero-trust mTLS (clients must present a cert)
orp start \
  --tls-cert server-cert.pem \
  --tls-key  server-key.pem \
  --tls-client-ca client-ca-bundle.pem
```

---

## CLI flags

| Flag | Purpose |
| ---- | ------- |
| `--tls-cert PATH` | PEM-encoded server certificate (chain). |
| `--tls-key PATH` | PEM-encoded server private key. |
| `--tls-client-ca PATH` | When set, requires every client to present a certificate signed by one of the CAs in the bundle (mTLS). |
| `--redirect-http PORT` | Spawn a second listener that 301-redirects every request to the HTTPS origin. Common values: `80`. |
| `--port PORT` | Listen port. Default is `9090` for HTTP, `9443` for HTTPS. |

Both `--tls-cert` and `--tls-key` must be supplied together. `--tls-client-ca`
and `--redirect-http` require `--tls-cert`+`--tls-key`.

When TLS is **not** active, ORP logs a `WARN` line on startup so the operator
can't miss it.

---

## 1. Local development

Generate a self-signed cert with the bundled `gen-cert` helper:

```bash
orp gen-cert
# Writes:
#   cert.pem   (server certificate, valid 365 days)
#   key.pem    (PKCS#8 private key, mode 0600 on Unix)
```

Customise:

```bash
orp gen-cert \
  --cn orp.dev.local \
  --san orp.dev.local --san 10.0.0.42 --san 127.0.0.1 \
  --days 30 \
  --out-dir ./tls
```

Start the server:

```bash
orp start --tls-cert ./tls/cert.pem --tls-key ./tls/key.pem
```

Hit it with `curl -k` (insecure — accepts the self-signed cert):

```bash
curl -k https://localhost:9443/api/v1/health
```

Browsers will show a warning. Click through; or import `cert.pem` into your
trust store for a clean dev experience.

> **Never use `gen-cert` output in production.** It produces a self-signed
> cert with no chain back to a public CA, and the key has no HSM/KMS
> protection.

---

## 2. Let's Encrypt

[Let's Encrypt](https://letsencrypt.org/) issues free, browser-trusted certs
via the ACME protocol. ORP does not embed an ACME client — instead, use
[`certbot`](https://certbot.eff.org/) (or [`lego`](https://go-acme.github.io/lego/),
[`acme.sh`](https://acme.sh)) and point ORP at the resulting files.

### Standalone HTTP-01 (single host, port 80 free)

```bash
# Stop ORP if it's holding port 80
sudo certbot certonly --standalone \
  -d orp.example.com \
  --email ops@example.com \
  --agree-tos --no-eff-email

# certbot writes to /etc/letsencrypt/live/orp.example.com/
sudo orp start \
  --tls-cert /etc/letsencrypt/live/orp.example.com/fullchain.pem \
  --tls-key  /etc/letsencrypt/live/orp.example.com/privkey.pem \
  --port 443 \
  --redirect-http 80
```

### Webroot (ORP keeps serving on port 80 via the redirector)

The `--redirect-http 80` listener serves the `/.well-known/acme-challenge/`
path with a 301, which **breaks** HTTP-01 webroot validation. For webroot
mode, route challenges through nginx/caddy in front of ORP — or use DNS-01.

### DNS-01 (preferred — no port-80 dance, supports wildcards)

```bash
sudo certbot certonly --manual --preferred-challenges=dns \
  -d orp.example.com -d "*.orp.example.com"
# Add the TXT record certbot prints, then continue.
```

### Auto-renewal

`certbot` installs a systemd timer (or cron entry) that renews 30 days before
expiry. ORP picks up the new cert on the **next process restart** — there is
no live reload. Wire a renewal hook:

```bash
# /etc/letsencrypt/renewal-hooks/deploy/orp-restart
#!/bin/sh
systemctl reload orp.service || systemctl restart orp.service
```

(Requires that your systemd unit is set up to handle the signal — see below.)

---

## 3. Corporate / private PKI

Many enterprises issue server certs from an internal CA (Active Directory
Certificate Services, HashiCorp Vault PKI, smallstep, etc.).

The flow is identical — point `--tls-cert` at the issued PEM bundle and
`--tls-key` at the private key:

```bash
orp start \
  --tls-cert /etc/orp/tls/orp-corp-cert.pem \
  --tls-key  /etc/orp/tls/orp-corp-key.pem
```

**The cert PEM should contain the full chain** (server cert → intermediate(s)),
in that order. If clients see "unknown issuer" errors, the chain is missing
intermediates. Verify with:

```bash
openssl s_client -connect orp.corp.local:9443 -showcerts < /dev/null
```

### Vault PKI example

```bash
vault write -field=certificate pki/issue/orp-server \
  common_name=orp.corp.local ttl=720h > cert.pem
vault write -field=private_key pki/issue/orp-server \
  common_name=orp.corp.local ttl=720h > key.pem
chmod 600 key.pem
```

### smallstep CA example

```bash
step ca certificate orp.corp.local cert.pem key.pem
```

---

## mTLS (mutual TLS)

When `--tls-client-ca <bundle.pem>` is passed, ORP requires every client to
present a certificate signed by one of the CAs in `bundle.pem`. This is the
zero-trust pattern for service-to-service traffic.

### Build a client CA bundle

```bash
# Trust two issuers
cat partner-ca.pem internal-ca.pem > client-ca-bundle.pem
```

### Start the server

```bash
orp start \
  --tls-cert server-cert.pem \
  --tls-key  server-key.pem \
  --tls-client-ca client-ca-bundle.pem
```

### Test with curl

```bash
curl --cacert server-ca.pem \
     --cert client.pem --key client-key.pem \
     https://orp.example.com/api/v1/health
```

A client without a cert (or with a cert signed by an untrusted CA) will fail
the TLS handshake — there is **no application-layer error**, the connection
is closed during the handshake.

### What about user identity?

`--tls-client-ca` proves the **caller's TLS identity**. ORP still expects a
JWT, OIDC token, or API key in the `Authorization` header for **user
identity**. Treat mTLS as transport-layer authentication — it pins which
clients can even talk to the server — and the bearer token as the user/role
proof.

---

## HSTS

When TLS is active, ORP automatically attaches:

```
Strict-Transport-Security: max-age=31536000
```

to every response. Browsers that have seen this header will refuse plain
HTTP for one year, mitigating downgrade attacks.

`includeSubDomains` and `preload` are **not** added automatically — they are
domain-scoped commitments that should be made deliberately, ideally at your
edge proxy (nginx, Cloudflare, etc.).

---

## HTTP-to-HTTPS redirector

`--redirect-http 80` spawns a second listener that responds to every HTTP
request with `301 Moved Permanently` to the HTTPS origin. The `Host` header
is preserved (minus any port suffix). Useful when ORP is exposed directly to
the public internet without nginx in front.

When ORP is behind a reverse proxy that already terminates TLS, **do not**
use `--redirect-http` — let the proxy own that flow.

---

## TLS protocol versions and ciphers

`rustls`'s default protocol set is **TLS 1.2 and TLS 1.3**. TLS 1.0 / 1.1 /
SSLv3 are not available. The cipher suite list is the rustls / ring default
— modern AEAD suites only:

* TLS 1.3: `TLS13_AES_128_GCM_SHA256`, `TLS13_AES_256_GCM_SHA384`,
  `TLS13_CHACHA20_POLY1305_SHA256`
* TLS 1.2: ECDHE-only with AES-GCM and ChaCha20-Poly1305

There is no CLI flag to weaken these — by design.

---

## Operational checklist

* [ ] Cert + key files are mode `0600` (key) / `0644` (cert), owned by the
  ORP service user.
* [ ] PEM bundle includes the full intermediate chain.
* [ ] Renewal hook reloads / restarts ORP after each renewal.
* [ ] Monitoring alerts fire ≥30 days before expiry.
* [ ] If using mTLS: client CA bundle is reviewed when partners rotate CAs.
* [ ] HSTS preload list submission has been considered (only after stable
  HTTPS for ≥6 months).

---

## Troubleshooting

**"Connection refused" on HTTPS port** — ORP failed to bind. Common cause:
the port is privileged (<1024) and ORP isn't running as root or doesn't
have `CAP_NET_BIND_SERVICE`. Use a higher port + reverse proxy, or grant the
capability with `setcap cap_net_bind_service=+ep $(which orp)`.

**"unknown issuer" / "self-signed"** — chain is missing intermediates, or
the client doesn't trust the issuer. Run:

```bash
openssl verify -CAfile <known-roots.pem> -untrusted <intermediates.pem> cert.pem
```

**Browser shows the cert but the API refuses** — when `--tls-client-ca` is
set, every connection requires a client cert. Browsers can't present one
without manual setup; either drop mTLS or import a client cert into the
browser's keychain.

**Slow handshake** — TLS 1.3 with rustls + ring is fast. If you're seeing
multi-hundred-ms handshakes, check for clock skew (cert validity windows)
and confirm hardware AES is available (`grep aes /proc/cpuinfo`).
