# ORP BUILD: TEAM STRUCTURE & SPRINT PLAN

**For:** 490 senior engineers (ex-Microsoft, Apple, Palantir, NVIDIA)
**Project:** ORP — Single-binary data fusion engine in Rust
**Timeline:** Phase 0 (6 weeks) + Phase 1 (9 months) + Phase 2 (12 months)
**Status:** Ready for team assignment

---

## PART 1: TEAM BREAKDOWN (490 ENGINEERS)

### Organizational Hierarchy

```
Chief Architect (1)
├── Core Systems Lead (1)
├── Product Lead (1)
└── Engineering Operations Lead (1)

└── 486 Engineers across 19 teams
    ├── Core (8) — DuckDB/Kuzu/RocksDB integration
    ├── Stream Processing (35) — Ingest, dedup, CEP, backpressure
    ├── Connectors (60) — Built-in + plugin system
    ├── Entity Resolution (40) — Structural + probabilistic matching
    ├── Query Engine (45) — ORP-QL parser, planner, executor
    ├── Graph Engine (30) — Kuzu sync, graph-specific queries
    ├── AI/ML (35) — llama.cpp, NL translation, anomaly detection
    ├── Frontend (50) — Console UI, map rendering, query bar
    ├── API (30) — REST, GraphQL, WebSocket, auth
    ├── Security (25) — OIDC, ABAC, Ed25519, audit log, pen testing
    ├── DevOps/Infrastructure (40) — CI/CD, cross-compilation, Kubernetes
    ├── Documentation (20) — Architecture, API ref, tutorials, community guides
    ├── Testing/QA (25) — Integration, load, chaos, benchmarks
    ├── Enterprise Features (20) — SSO, compliance, clustering, audit exports
    ├── Cloud Platform (30) — Multi-tenant SaaS, scaling, billing
    ├── Simulation (18) — Agent models, scenario forking, Ray integration
    ├── Mobile (15) — React Native/Flutter companion app
    ├── Edge Deployment (12) — ARM optimization, lightweight binary
    ├── SDK/Developer Experience (22) — Python, JavaScript, Go, WASM SDKs
    └── Community/DevRel (18) — OSS governance, contributor tooling, registry
```

---

### TEAM 1: CORE ENGINE (8 engineers)

**Lead:** 1 Staff Engineer (ex-database systems, 15+ yrs)
**ICs:** 7 Senior Engineers

**Mission:** Build the single unified binary. Integrate DuckDB, Kuzu, RocksDB, tokio runtime. Stabilize the foundation.

**Owned Crates/Modules:**
- `orp-core` — main binary, CLI, config parsing
- `orp-storage` — DuckDB embedding, schema initialization
- `orp-graph` — Kuzu integration, schema sync
- `orp-stream-state` — RocksDB wrapper for dedup/windowing
- `orp-crypto` — Ed25519 signing, audit log chain
- `orp-config` — YAML parsing, validation, environment substitution

**Key Deliverables (Phase 0):**
- Binary compiles to <350MB
- DuckDB + Kuzu schema designed and tested with 10M entities
- Sync loop (DuckDB → Kuzu every 30s) verified at scale
- Ed25519 signing infrastructure working
- Startup time <5s, memory <3GB for 1M entities

**Key Deliverables (Phase 1):**
- Binary stable on Linux + macOS
- All streaming connectors can write to storage layer without data loss
- Query layer receives requests from all API styles (REST, GraphQL, WebSocket)
- Storage layer handles 500K entity updates per sync cycle
- Audit log immutable and tamper-evident

**Dependencies:** None (foundational)

**Success Metrics:**
- Zero data corruption in load tests
- Query latency at 1M entities: p50 <200ms, p99 <1s
- Binary doesn't grow beyond 350MB
- Memory doesn't exceed 3GB at 1M entities

---

### TEAM 2: STREAM PROCESSING (35 engineers)

**Lead:** 1 Staff Engineer (ex-Kafka/Flink, streaming systems)
**ICs:** 34 Senior Engineers

**Mission:** Ingest data from anywhere. Deduplicate. Detect changes. Handle backpressure. Achieve 500K events/sec throughput.

**Owned Crates/Modules:**
- `orp-ingest` — unified connector interface, async task spawning
- `orp-dedup` — window-based deduplication (RocksDB-backed)
- `orp-change-detection` — identify new/modified/deleted facts
- `orp-backpressure` — adaptive flow control, buffering strategy
- `orp-schema-inference` — auto-detect entity types and property types
- `orp-batch-insert` — Tokio tasks → DuckDB batch writes (every 1-5s)

**Key Deliverables (Phase 0):**
- Dedup logic tested: 100K events/sec, <10ms latency, zero false positives
- Change detection: can identify if a ship's position changed vs. duplicate
- Backpressure mechanism prevents memory explosion under load
- RocksDB state survives binary restart
- Performance benchmark: full pipeline (TCP → dedup → DuckDB) at 100K/sec

**Key Deliverables (Phase 1):**
- 500K events/sec sustained throughput (measured at p50, p99)
- AIS ingest at 30K events/sec with zero data loss
- CEP (Complex Event Processing) for pattern detection (ship entered zone, deviation, speed spike)
- Metrics: dedup window size, backpressure buffer depth, insert batch latency

**Dependencies:** Core Engine (storage), Connectors (data sources)

**Success Metrics:**
- 500K events/sec on modern hardware, <1GB extra memory
- Dedup accuracy 99.9%+
- Change detection recalls >99% of real changes
- No data loss under backpressure
- Median batch-insert latency <50ms

---

### TEAM 3: CONNECTORS (60 engineers)

**Sub-teams:**

**3A: Built-in Connectors (20 engineers)**
- Lead: Senior Engineer
- **Crates:**
  - `orp-connector-ais` — AIS TCP/NMEA parsing, position updates
  - `orp-connector-adsb` — ADS-B TCP receiver, aircraft positions
  - `orp-connector-weather` — NOAA API polling, weather observations
  - `orp-connector-osm` — OpenStreetMap geometry, POI data
  - `orp-connector-http` — Generic HTTP/REST polling framework
  - `orp-connector-mqtt` — MQTT client for IoT sensors
- **Deliverables (Phase 0):** AIS + ADS-B + HTTP framework working
- **Deliverables (Phase 1):** All 6 built-in connectors working, tested, documented

**3B: Connector Framework & Plugin System (20 engineers)**
- Lead: Senior Engineer
- **Crates:**
  - `orp-connector-trait` — trait definition, lifecycle, error handling
  - `orp-plugin-wasm` — WASM runtime (wasmtime), sandbox isolation
  - `orp-plugin-loader` — dynamic WASM plugin loading, versioning
  - `orp-sandbox` — resource limits (CPU, memory, network), anomaly detection
- **Deliverables (Phase 1):** WASM plugin system working, example plugins in Python/Rust/Go
- **Deliverables (Phase 2):** Tap Registry launched with 20+ community connectors

**3C: Connector Tooling & SDKs (20 engineers)**
- Lead: Senior Engineer
- **Crates:**
  - `orp-connector-cli` — `orp connector new`, `orp connector test`, `orp connector build`
  - `orp-connector-py-sdk` — Python bindings for writing connectors
  - `orp-connector-go-sdk` — Go bindings for writing connectors
  - `orp-connector-docs` — Tutorial, examples, best practices
- **Deliverables (Phase 1):** CLI + Python SDK working
- **Deliverables (Phase 2):** Go SDK, 10+ reference implementations

**Dependencies:** Core Engine (ingest interface), Stream Processing (dedup)

**Success Metrics:**
- Phase 1: 6 built-in connectors, 99.9%+ uptime in tests
- Phase 2: WASM system can load 50+ plugins without crashes
- Connector latency (data arrival → DuckDB) <1s
- Plugin isolation enforced: one broken plugin doesn't crash ORP

---

### TEAM 4: ENTITY RESOLUTION (40 engineers)

**Sub-teams:**

**4A: Structural Matching (15 engineers)**
- Lead: Senior Engineer
- **Crates:**
  - `orp-resolver-structural` — rule-based matching by unique ID (MMSI, ICAO, etc.)
  - `orp-resolver-dedup-state` — maintains current master entities
  - `orp-resolver-merge-logic` — properties merge when two IDs resolve to same entity
- **Deliverables (Phase 0):** Can match ships by MMSI with 99.9%+ accuracy
- **Deliverables (Phase 1):** Handles all major entity types (ship, aircraft, port, weather)

**4B: Probabilistic Matching (15 engineers)**
- Lead: Senior Engineer (ML background)
- **Crates:**
  - `orp-resolver-probabilistic` — XGBoost classifier for fuzzy matching
  - `orp-resolver-clustering` — group candidate matches, find connected components
  - `orp-resolver-features` — name similarity, geospatial distance, time correlation
  - `orp-resolver-training` — dataset curation, model fine-tuning
- **Deliverables (Phase 2):** Can match entities across sources with 95%+ accuracy
- **Deliverables (Phase 2):** Human-in-the-loop correction UI for ambiguous cases

**4C: Data Quality & Confidence (10 engineers)**
- Lead: Senior Engineer
- **Crates:**
  - `orp-quality-freshness` — track when each property was last updated
  - `orp-quality-consistency` — detect when sources disagree on a fact
  - `orp-quality-confidence` — assign reliability scores (0.0–1.0) to sources
  - `orp-quality-ui` — show data quality in console
- **Deliverables (Phase 1):** Every entity property carries freshness, confidence, consistency metadata
- **Deliverables (Phase 2):** Users can filter queries by quality thresholds

**Dependencies:** Core Engine (storage), Stream Processing (change detection)

**Success Metrics:**
- Phase 1: Structural matching 99.9%+ accurate on AIS/ADS-B
- Phase 2: Probabilistic matching 95%+ accurate on diverse sources
- Human review rate <5% (most matches auto-accepted)
- False positive (two entities merged that shouldn't be): <0.1%

---

### TEAM 5: QUERY ENGINE (45 engineers)

**Sub-teams:**

**5A: Parser & Planner (15 engineers)**
- Lead: Senior Engineer (compiler/DB background)
- **Crates:**
  - `orp-ql-parser` — ORP-QL grammar (LALRPOP), AST generation
  - `orp-ql-planner` — convert AST → query plan (estimate cost, reorder operations)
  - `orp-ql-optimizer` — push filters, reuse predicates, cost-based optimization
- **Deliverables (Phase 0):** Parser handles basic queries: "ships near X" "ships where speed > Y"
- **Deliverables (Phase 1):** Planner optimizes for <500ms latency

**5B: Execution Engine (15 engineers)**
- Lead: Senior Engineer
- **Crates:**
  - `orp-executor` — physical operator implementations (scan, filter, join, aggregate)
  - `orp-executor-duckdb` — DuckDB-backed execution for OLAP queries
  - `orp-executor-kuzu` — Kuzu-backed execution for graph queries
  - `orp-executor-hybrid` — combines DuckDB + Kuzu (e.g., filter entities in DuckDB, then walk graph in Kuzu)
- **Deliverables (Phase 1):** Can execute scans, filters, joins, aggregations, geospatial predicates
- **Deliverables (Phase 2):** Temporal queries (where was X at time T?)

**5C: API Translation (15 engineers)**
- Lead: Senior Engineer
- **Crates:**
  - `orp-api-rest` — translate REST query params → ORP-QL
  - `orp-api-graphql` — GraphQL query → ORP-QL (Phase 2)
  - `orp-api-websocket` — WebSocket subscriptions → continuous queries
- **Deliverables (Phase 1):** REST API fully functional
- **Deliverables (Phase 2):** GraphQL + WebSocket subscriptions

**Dependencies:** Core Engine (storage), Stream Processing (change detection)

**Success Metrics:**
- Phase 1: Simple queries <200ms, complex queries <1s at 1M entities
- p99 latency <3s (even for complex geospatial + time-based queries)
- Parser handles 95% of intended query patterns
- No query planner infinite loops or crashes

---

### TEAM 6: GRAPH ENGINE (30 engineers)

**Sub-teams:**

**6A: Kuzu Integration (12 engineers)**
- Lead: Senior Engineer (graph DB background)
- **Crates:**
  - `orp-graph-schema` — entity type definitions, relationship types
  - `orp-graph-kuzu-wrapper` — Kuzu Rust bindings, schema management
  - `orp-graph-sync` — DuckDB → Kuzu bidirectional sync (every 30s)
- **Deliverables (Phase 0):** Kuzu loaded with 10M entities, queries <1s
- **Deliverables (Phase 1):** Sync is atomic, no data loss, handles 100K entity updates per sync

**6B: Graph Query Language (10 engineers)**
- Lead: Senior Engineer
- **Crates:**
  - `orp-gql` — Graph Query Language (Cypher-like for path traversal)
  - `orp-gql-planner` — translate to Kuzu native queries
  - `orp-gql-executor` — execute on Kuzu
- **Deliverables (Phase 1):** Support basic path queries: "ships heading to Rotterdam"
- **Deliverables (Phase 2):** Reachability, shortest path, cycle detection

**6C: Graph Visualization & UI (8 engineers)**
- Lead: Senior Engineer (frontend background)
- **Crates:**
  - `orp-graph-viz` — React component for graph rendering (Cytoscape.js or similar)
  - `orp-graph-inspection` — entity detail view, relationship explorer
- **Deliverables (Phase 1):** Can render graphs with 100+ nodes and edges
- **Deliverables (Phase 2):** Interactive filtering, layout optimization

**Dependencies:** Core Engine (storage), Query Engine (query interface)

**Success Metrics:**
- Phase 1: 3-hop path queries on 10M entities <1s
- Sync latency <100ms (DuckDB → Kuzu)
- No corruption when syncing high-velocity data
- Graph visualization renders 500 nodes/edges fluidly in browser

---

### TEAM 7: AI/ML (35 engineers)

**Sub-teams:**

**7A: LLM Integration (12 engineers)**
- Lead: Senior Engineer (ML systems background)
- **Crates:**
  - `orp-llm-loader` — download + cache Phi-2 on first use
  - `orp-llm-inference` — llama.cpp wrapper, batch inference
  - `orp-llm-quantization` — Q4_K quantization for 1.6GB footprint
- **Deliverables (Phase 2):** Model downloads, loads, inference works
- **Deliverables (Phase 2):** Batch queries infer in <2s

**7B: NL→Query Translation (12 engineers)**
- Lead: Senior Engineer (NLP background)
- **Crates:**
  - `orp-nl-templates` — 100+ pre-built query templates (fast, <100ms)
  - `orp-nl-finetuning` — fine-tune Phi-2 on ORP-QL examples
  - `orp-nl-fallback` — when model uncertain, show candidate interpretations
- **Deliverables (Phase 1):** Template-based NL (80% of queries, <100ms)
- **Deliverables (Phase 2):** Model-based NL (15% more, 1-2s)

**7C: Anomaly Detection & Analysis (11 engineers)**
- Lead: Senior Engineer
- **Crates:**
  - `orp-anomaly-rules` — rule-based detection (speed spike, course deviation, etc.)
  - `orp-anomaly-isolation-forest` — unsupervised anomaly scoring
  - `orp-anomaly-alerts` — fire alerts when anomalies detected
- **Deliverables (Phase 1):** Rule-based anomalies (speed > threshold, deviation from course)
- **Deliverables (Phase 2):** ML-based anomaly detection

**Dependencies:** Core Engine, Query Engine

**Success Metrics:**
- Phase 2: NL query coverage 95%+ (accurate interpretation)
- False alarm rate <10% on anomaly detection
- Model inference latency <2s for 99% of queries
- Anomaly precision >80% (when we flag it, it's real)

---

### TEAM 8: FRONTEND (50 engineers)

**Sub-teams:**

**8A: Core UI & Maps (20 engineers)**
- Lead: Senior Engineer (frontend architecture)
- **Crates:**
  - `orp-console` — React app, Vite bundler
  - `orp-map-2d` — Deck.gl map renderer, entity positions
  - `orp-map-3d` — CesiumJS 3D globe
  - `orp-theme` — design system, dark mode, accessibility
- **Deliverables (Phase 0):** Map renders 100 ships, pans/zooms smoothly
- **Deliverables (Phase 1):** Real-time position updates via WebSocket, <100ms latency end-to-end

**8B: Entity Inspector & Relationships (15 engineers)**
- Lead: Senior Engineer
- **Crates:**
  - `orp-inspector` — detail view for entity, all properties, relationships, history
  - `orp-history-timeline` — temporal slider showing state at past times
  - `orp-relationship-explorer` — navigate from entity → related entities
- **Deliverables (Phase 1):** Click ship → see full details + timeline + related ports

**8C: Query & Alert UX (15 engineers)**
- Lead: Senior Engineer
- **Crates:**
  - `orp-query-bar` — text input, autocomplete, query history
  - `orp-query-results` — display query results in table/map/graph
  - `orp-alert-feed` — real-time alert notifications, acknowledgment
  - `orp-saved-queries` — save/load favorite queries
- **Deliverables (Phase 1):** Query bar works, results render, alerts fire

**Dependencies:** Core Engine (HTTP), Query Engine (results), Stream Processing (real-time updates)

**Success Metrics:**
- Page load <2s
- Map render 1000 entities smoothly (60fps)
- WebSocket latency <100ms (data update → UI change)
- Query results display in <200ms for simple queries
- Accessibility: WCAG 2.1 AA compliant

---

### TEAM 9: API (30 engineers)

**Sub-teams:**

**9A: REST API (12 engineers)**
- Lead: Senior Engineer (API design)
- **Crates:**
  - `orp-api-rest` — Axum HTTP handlers
  - `orp-api-openapi` — OpenAPI 3.0 spec, auto-generated docs
  - `orp-api-versioning` — v1/, v2/ routes, backward compatibility
  - `orp-api-pagination` — cursor-based pagination for large result sets
- **Deliverables (Phase 1):** Full REST API: GET ships, POST queries, WebSocket upgrades

**9B: Authentication & Rate Limiting (10 engineers)**
- Lead: Senior Engineer (security background)
- **Crates:**
  - `orp-auth-oidc` — OIDC client, token validation
  - `orp-auth-apikey` — scoped API key management
  - `orp-ratelimit` — per-user/per-IP rate limits
  - `orp-audit-api` — log all API calls for compliance
- **Deliverables (Phase 1):** OIDC auth working, API keys issued
- **Deliverables (Phase 1):** Rate limits prevent abuse

**9C: GraphQL & WebSocket (8 engineers)**
- Lead: Senior Engineer
- **Crates:**
  - `orp-graphql` — async-graphql schema, resolvers
  - `orp-websocket` — WebSocket subscriptions for real-time updates
- **Deliverables (Phase 2):** GraphQL queries, subscriptions

**Dependencies:** Core Engine (HTTP server), Query Engine, Security (auth)

**Success Metrics:**
- API p50 latency <200ms, p99 <1s
- Rate limit enforcement prevents bot attacks
- OIDC token validation <10ms overhead
- 10K concurrent WebSocket connections without degradation

---

### TEAM 10: SECURITY (25 engineers)

**Sub-teams:**

**10A: Authentication & Authorization (10 engineers)**
- Lead: Staff Engineer (security architecture)
- **Crates:**
  - `orp-auth` — OIDC + embedded Keycloak-lite + API keys
  - `orp-abac` — Attribute-Based Access Control rules engine
  - `orp-rbac` — Role-Based Access Control (admin, analyst, viewer)
  - `orp-permission-cache` — caching to avoid latency penalty
- **Deliverables (Phase 1):** OIDC, ABAC, RBAC working
- **Deliverables (Phase 1):** Users can't see entities they shouldn't

**10B: Data Integrity & Signing (10 engineers)**
- Lead: Senior Engineer
- **Crates:**
  - `orp-crypto-signing` — Ed25519 signing on all ingested data
  - `orp-audit-log` — append-only, hash-chained, tamper-evident
  - `orp-audit-verification` — verify audit log integrity periodically
  - `orp-erasure` — cryptographic erasure for GDPR (destroy key = unrecoverable)
- **Deliverables (Phase 1):** All data signed, audit log working
- **Deliverables (Phase 1):** Audit log verified as tamper-evident

**10C: Security Testing & Hardening (5 engineers)**
- Lead: Senior Engineer (pen testing)
- **Crates:**
  - `orp-security-tests` — unit + integration tests for auth/crypto
  - `orp-fuzzing` — fuzzing critical paths (parser, crypto, query engine)
- **Deliverables (Phase 1):** Security review of auth, signing, audit log
- **Deliverables (Phase 1):** Pen test by external team, findings remediated

**Dependencies:** Core Engine, API, Connectors

**Success Metrics:**
- Zero auth bypass vulnerabilities
- Audit log proven tamper-evident (can detect tampering)
- Permission checks have <10ms latency
- No data accessible outside user's ABAC permissions
- Fuzzing runs continuously, finds 0 critical issues in release builds

---

### TEAM 11: DEVOPS/INFRASTRUCTURE (40 engineers)

**Sub-teams:**

**11A: CI/CD & Build (15 engineers)**
- Lead: Staff Engineer (platform engineering)
- **Crates:**
  - `orp-build-cli` — `cargo build` wrappers, optimizations
  - `orp-build-cross` — cross-compile for Linux, macOS, Windows, ARM
  - `orp-build-release` — signed binaries, reproducible builds
  - `orp-ci-github-actions` — GitHub Actions workflows
- **Deliverables (Phase 0):** CI runs on every commit, builds all targets
- **Deliverables (Phase 1):** Release builds automated, reproducible, signed
- **Deliverables (Phase 2):** ARM Linux support for edge deployments

**11B: Binary Distribution & Updates (15 engineers)**
- Lead: Senior Engineer
- **Crates:**
  - `orp-installer` — shell script installer (`curl -fsSL | sh`)
  - `orp-auto-update` — check for updates, download + install atomically
  - `orp-package-managers` — brew, apt, yum support (Phase 2)
  - `orp-release-channel` — stable / beta / nightly channels
- **Deliverables (Phase 1):** Installer works on Linux + macOS
- **Deliverables (Phase 1):** 5-minute install experience end-to-end

**11C: Kubernetes & Deployment (10 engineers)**
- Lead: Senior Engineer
- **Crates:**
  - `orp-helm-chart` — Helm chart for Kubernetes deployment
  - `orp-docker` — Multi-stage Dockerfile, minimal images
  - `orp-observability` — Prometheus metrics, OpenTelemetry tracing
  - `orp-health-checks` — liveness/readiness probes, graceful shutdown
- **Deliverables (Phase 1):** Helm chart for single-node deployment
- **Deliverables (Phase 2):** Helm chart for multi-node cluster

**Dependencies:** None (foundational infrastructure)

**Success Metrics:**
- CI runs in <10 minutes, green on every commit
- Release builds reproducible (same hash for same source)
- Installer completes in <5 minutes on slow network
- Kubernetes deployment scales to 100+ nodes
- Binary size stays <350MB

---

### TEAM 12: DOCUMENTATION (20 engineers)

**Lead:** 1 Staff Technical Writer (ex-Palantir, Google)
**ICs:** 19 Senior Engineers

**Mission:** Make ORP understandable. Write docs so clear that a smart person with zero ORP knowledge can deploy it and use it.

**Owned Crates/Modules:**
- `orp-docs` — mdBook source, hosted on docs.orp.dev
- `orp-examples` — maritime, supply chain, climate examples with code
- `orp-tutorials` — step-by-step guides (10-50 pages each)

**Key Deliverables (Phase 0):**
- Architecture decision records (ADRs) for all major design choices
- Technical architecture diagram: data flow, component interaction
- Why we chose DuckDB + Kuzu + RocksDB (vs. alternatives)

**Key Deliverables (Phase 1):**
- Getting Started Guide (15 pages): install, run, see ships
- API Reference (50 pages): every REST endpoint documented with examples
- Query Language Guide (30 pages): ORP-QL syntax, examples, edge cases
- Connector Development Guide (40 pages): how to write a custom connector
- Security Guide (25 pages): OIDC, ABAC, signing, audit log
- Architecture docs (60 pages): detailed explanation of every team's subsystem
- FAQ (10 pages): common questions, troubleshooting
- Contributing Guide (15 pages): how to contribute to ORP
- 3 step-by-step tutorials (50 pages each):
  - Maritime monitoring (ships, weather, ports)
  - Supply chain (shipping routes, inventory)
  - Climate (temperature + shipping routes correlation)

**Key Deliverables (Phase 2):**
- WASM Plugin Development Guide (50 pages)
- Simulation Framework Guide (40 pages)
- Advanced Query Patterns (30 pages)
- Video tutorials (5-10 videos, 10-20 min each)

**Dependencies:** All other teams (docs must keep up)

**Success Metrics:**
- Phase 1: 200+ pages, searchable, >95% code examples tested
- External person can deploy ORP in 5 minutes using only docs
- Zero unanswered questions in first 30 GitHub issues
- API reference coverage 100%

---

### TEAM 13: TESTING/QA (25 engineers)

**Sub-teams:**

**13A: Integration Testing (10 engineers)**
- Lead: Senior Engineer (QA architecture)
- **Crates:**
  - `orp-test-harness` — test fixtures, synthetic data generators
  - `orp-test-maritime` — AIS simulator, realistic ship movement
  - `orp-test-scenarios` — 20+ end-to-end test scenarios
- **Deliverables (Phase 1):** 100+ integration tests, all passing
- **Deliverables (Phase 1):** Nightly full regression test suite

**13B: Load & Performance Testing (10 engineers)**
- Lead: Senior Engineer (performance engineering)
- **Crates:**
  - `orp-bench` — microbenchmarks for critical paths (query, dedup, ingest)
  - `orp-load-test` — sustained load testing (500K events/sec, 1M entities)
  - `orp-profile` — perf flamegraphs, memory profiling
- **Deliverables (Phase 1):** Benchmark suite runs on every commit
- **Deliverables (Phase 1):** Performance regressions caught before release

**13C: Chaos & Resilience Testing (5 engineers)**
- Lead: Senior Engineer
- **Crates:**
  - `orp-chaos` — fault injection (network partition, process crash, disk full)
  - `orp-recovery-tests` — verify recovery after failures
- **Deliverables (Phase 2):** ORP survives common failure modes

**Dependencies:** All teams (integration tests for every feature)

**Success Metrics:**
- 100+ integration tests, pass rate >99.9%
- Performance benchmarks stable, regressions flagged
- Load test: 500K events/sec sustained for 1 hour, zero data loss
- Chaos test: recover from crash within 30s, no data corruption

---

### TEAM 14: ENTERPRISE FEATURES (20 engineers)

**Sub-teams:**

**14A: SSO & Compliance (10 engineers)**
- Lead: Senior Engineer (identity/compliance background)
- **Crates:**
  - `orp-sso-saml` — SAML 2.0 support (Okta, Azure AD, etc.)
  - `orp-compliance-soc2` — SOC 2 controls, audit trails
  - `orp-compliance-gdpr` — data minimization, retention, erasure
  - `orp-audit-export` — export audit logs for compliance review
- **Deliverables (Phase 2):** SAML SSO working, SOC 2 guide published

**14B: Clustering & High Availability (10 engineers)**
- Lead: Senior Engineer (distributed systems)
- **Crates:**
  - `orp-cluster` — multi-node coordination (Raft for consensus)
  - `orp-replication` — replicate DuckDB + Kuzu across nodes
  - `orp-failover` — automatic failover on node death
- **Deliverables (Phase 3):** 3+ node cluster with 99.99% uptime

**Dependencies:** Core Engine, Security, DevOps

**Success Metrics:**
- Phase 2: SAML auth <50ms overhead
- Phase 3: Cluster survives 1 node death, automatic failover <30s
- SOC 2 audit completed with 0 critical findings

---

### TEAM 15: CLOUD PLATFORM (30 engineers)

**Sub-teams:**

**15A: ORP Cloud SaaS (18 engineers)**
- Lead: Staff Engineer (cloud architecture)
- **Crates:**
  - `orp-cloud-api` — multi-tenant API, user management
  - `orp-cloud-provisioning` — spin up isolated ORP instances
  - `orp-cloud-auth` — cloud-specific auth (cloud API keys, oauth)
  - `orp-cloud-storage` — persistent storage on cloud (S3, GCS)
  - `orp-cloud-compute` — right-size containers based on load
- **Deliverables (Phase 2):** Beta SaaS launched, 50+ users
- **Deliverables (Phase 3):** Production SaaS, 1000+ users

**15B: Billing & Metering (12 engineers)**
- Lead: Senior Engineer
- **Crates:**
  - `orp-billing` — usage metering (entities stored, queries, data ingestion)
  - `orp-pricing` — pricing model, SKUs
  - `orp-invoicing` — monthly invoices, payment processing
  - `orp-usage-limits` — enforce usage quotas
- **Deliverables (Phase 2):** Billing working, can charge customers

**Dependencies:** Core Engine (wrapped for cloud), API, Security

**Success Metrics:**
- Phase 2: ORP Cloud runs 50+ isolated instances
- Billing accuracy 99.99%, no dispute issues
- Instance spin-up time <5 minutes
- Multi-tenancy: perfect data isolation, zero cross-tenant leakage

---

### TEAM 16: SIMULATION (18 engineers)

**Sub-teams:**

**16A: Agent-Based Models (12 engineers)**
- Lead: Senior Engineer (simulation/physics background)
- **Crates:**
  - `orp-sim-core` — discrete event simulation engine
  - `orp-sim-agents` — agent framework, behavior scripting
  - `orp-sim-transport` — ship routing, traffic flow models
  - `orp-sim-weather` — weather system simulation, storm propagation
- **Deliverables (Phase 2):** Can simulate 1000 ships moving over 7 days in <10 seconds
- **Deliverables (Phase 2):** Results match real historical data within 5%

**16B: Scenario Forking (6 engineers)**
- Lead: Senior Engineer
- **Crates:**
  - `orp-fork-scenario` — branch at point in time, simulate alternative futures
  - `orp-fork-comparison` — compare two scenarios, highlight divergences
- **Deliverables (Phase 2):** Can fork scenario, show "what if" analysis
- **Deliverables (Phase 3):** Visual comparison of scenarios

**Dependencies:** Core Engine, Query Engine, AI/ML

**Success Metrics:**
- Simulation 1000 agents in <10s
- Accuracy within 5% of historical reference data
- Scenario fork/compare <1s latency

---

### TEAM 17: MOBILE (15 engineers)

**Sub-teams:**

**17A: React Native App (8 engineers)**
- Lead: Senior Engineer (React Native)
- **Crates:**
  - `orp-mobile-app` — React Native app (iOS + Android)
  - `orp-mobile-map` — map component for phones, efficient rendering
  - `orp-mobile-query` — query bar UI, results on mobile
- **Deliverables (Phase 2):** Beta app, core features on mobile
- **Deliverables (Phase 3):** Production app, 10K+ downloads

**17B: Offline Sync (7 engineers)**
- Lead: Senior Engineer
- **Crates:**
  - `orp-mobile-sync` — sync data when online, cache offline
  - `orp-mobile-conflict-resolution` — handle conflicts when offline
- **Deliverables (Phase 3):** Can use app offline, sync when reconnected

**Dependencies:** Core Engine (API), Query Engine

**Success Metrics:**
- Phase 2: App launches in <3s, queries return in <500ms
- Phase 3: Offline mode works, sync robust
- App store rating >4.5 stars

---

### TEAM 18: EDGE DEPLOYMENT (12 engineers)

**Sub-teams:**

**18A: ARM Optimization (7 engineers)**
- Lead: Senior Engineer (embedded systems)
- **Crates:**
  - `orp-arm-build` — cross-compile for ARM64, ARMv7
  - `orp-arm-optimize` — reduce binary size for edge, tune memory
- **Deliverables (Phase 2):** ORP runs on Raspberry Pi 4, <2GB memory
- **Deliverables (Phase 3):** ORP runs on edge devices with <500MB memory

**18B: Lightweight Binary (5 engineers)**
- Lead: Senior Engineer
- **Crates:**
  - `orp-slim-binary` — optional feature flags, slim down to <200MB if features disabled
  - `orp-modular-connectors` — download only needed connectors
- **Deliverables (Phase 3):** Slim binary <200MB, still functional

**Dependencies:** Core Engine, DevOps

**Success Metrics:**
- Phase 2: ORP runs on Raspberry Pi 4, <30s startup
- Phase 3: ARM binary <200MB for minimal config

---

### TEAM 19: SDK/DEVELOPER EXPERIENCE (22 engineers)

**Sub-teams:**

**19A: Python SDK (8 engineers)**
- Lead: Senior Engineer (Python)
- **Crates:**
  - `orp-python-sdk` — PyPI package, type hints, idiomatic Python
  - `orp-python-examples` — 10+ example notebooks
- **Deliverables (Phase 2):** Python SDK on PyPI, 100+ downloads

**19B: JavaScript/TypeScript SDK (8 engineers)**
- Lead: Senior Engineer (JavaScript)
- **Crates:**
  - `orp-js-sdk` — npm package, TypeScript types
  - `orp-js-examples` — Node.js + browser examples
- **Deliverables (Phase 2):** JS SDK on npm, 100+ downloads

**19C: Go SDK & WASM Plugin SDK (6 engineers)**
- Lead: Senior Engineer
- **Crates:**
  - `orp-go-sdk` — Go package, idiomatic concurrency
  - `orp-wasm-plugin-sdk` — WASM bindings for plugin writers
- **Deliverables (Phase 2):** Go SDK on GitHub, WASM SDK docs

**Dependencies:** API (REST endpoint spec), Connectors (plugin system)

**Success Metrics:**
- Phase 2: 3+ SDKs published, 500+ combined downloads
- Example code all tested, works out-of-the-box
- SDK docs coverage 100%

---

### TEAM 20: COMMUNITY/DEVREL (18 engineers)

**Lead:** 1 Staff Engineer (ex-DevRel at large OSS project)
**ICs:** 17 Senior Engineers

**Mission:** Grow the ORP community. Make contributing easy. Build ecosystem.

**Owned Assets:**
- GitHub repo governance (issues, PRs, discussions)
- Tap Registry (community connector marketplace)
- Community Slack/Discord
- Blog, YouTube channel
- Monthly community calls
- Contributor ladder (User → Tap Contributor → Active Contributor → Core Committer → TSC)

**Key Deliverables (Phase 1):**
- GOVERNANCE.md finalized
- First 20 "Good First Issues" tagged
- Contributing guide completed
- Code of Conduct established and enforced
- First 5 external contributors merged

**Key Deliverables (Phase 2):**
- Tap Registry launched (search + download community connectors)
- 50+ community contributors
- 10+ talks at conferences (OSCON, QCon, etc.)
- Academic partnerships: 3+ universities using ORP

**Key Deliverables (Phase 3):**
- Open-source foundation established
- 500+ community contributors
- ORP mentioned in 20+ research papers

**Dependencies:** All teams (enabling contributions)

**Success Metrics:**
- Phase 1: 500+ GitHub stars, 5+ external contributors
- Phase 2: 5000+ GitHub stars, 50+ contributors, 20 community connectors
- Phase 3: 50K+ GitHub stars, 500+ contributors, 100+ community connectors

---

## PART 2: SPRINT PLAN (PHASE 0 + PHASE 1)

### Phase 0: Design Sprint (Weeks 1-6, January 2026)

#### Week 1-2: Foundation & Architecture

**Sprint 1: Weeks 1-2**

**Core Engine Team:**
- Design DuckDB schema: entities, properties, geospatial, temporal
- Design Kuzu schema: entity types, relationship types
- Prototype sync loop (DuckDB → Kuzu every 30s)
- Create GitHub repo, CI/CD pipeline (GitHub Actions)
- Write GOVERNANCE.md, CODE_OF_CONDUCT.md

**Stream Processing Team:**
- Design dedup logic (RocksDB-backed window)
- Design change detection (diff old vs. new facts)
- Design backpressure mechanism
- Write design doc: "Stream Processing Architecture"

**Connectors Team:**
- Design connector trait (lifecycle: init → config → ingest → shutdown)
- Prototype AIS TCP receiver
- Prototype HTTP polling framework

**Testing/QA Team:**
- Set up test harness, synthetic data generators
- Create integration test template
- Establish performance baseline expectations

**Deliverables:**
- GitHub repo public (but not advertised)
- GOVERNANCE.md and CODE_OF_CONDUCT.md reviewed
- 3 architecture decision records (ADRs):
  - Why DuckDB + Kuzu + RocksDB
  - Stream processor design (async/tokio, not Flink)
  - Connector trait design
- CI/CD pipeline green on empty repo
- Performance baselines measured (on reference hardware)

**Go/No-Go Gate:** Can we load DuckDB + Kuzu with 10M entities and run basic queries?

**Demo:** None (internal only)

---

**Week 3-4: Prototyping**

**Sprint 2: Weeks 3-4**

**Core Engine Team:**
- Implement DuckDB embedding (schema creation, basic inserts)
- Implement Kuzu embedding (schema creation, basic queries)
- Implement sync loop (DuckDB → Kuzu every 30s)
- Measure: 10M entities loaded, 1M → 10M sync, query latency

**Stream Processing Team:**
- Implement dedup window (RocksDB-backed)
- Implement change detection (old vs. new entity state)
- Test: 100K events/sec through full pipeline

**Connectors Team:**
- Implement AIS TCP connector (parse NMEA 0183)
- Implement HTTP polling connector
- Test: 10K AIS events into DuckDB

**API Team:**
- Implement basic REST server (Axum)
- Implement /api/ships endpoint (get all ships)
- Implement /api/query endpoint (query by properties)

**Frontend Team:**
- Create React app skeleton (Vite)
- Implement simple map (Deck.gl placeholder)
- Implement entity list view

**Security Team:**
- Design auth architecture (OIDC, ABAC)
- Design Ed25519 signing on data ingestion
- Design audit log schema

**Deliverables:**
- `cargo build` produces binary, ~200MB
- AIS feed → DuckDB → query in <500ms
- HTTP REST endpoints working
- Basic React UI shows entity list
- 50+ unit tests passing

**Go/No-Go Gate:** Can we ingest 100K events/sec through full pipeline? Is binary size <350MB?

**Demo:** Internal: "Here's a working pipeline from AIS → DuckDB → REST API"

---

#### Week 5-6: Integration & Hardening

**Sprint 3: Weeks 5-6**

**All Teams:**
- Integration testing: AIS → dedup → DuckDB → Kuzu sync → REST API → Frontend
- Load testing: 500K events/sec, measure latency and memory
- Binary size audit: eliminate bloat, target <350MB
- Documentation: Write design doc for each team's subsystem
- Security: First security review (code audit, threat model)

**Core Engine Team:**
- Measure Kuzu performance: 3-hop query on 10M entities <1s?
- Optimize sync loop for high-velocity data (1M updates/sync)

**Stream Processing Team:**
- Verify dedup accuracy: zero false positives on real AIS data
- Test backpressure: simulate slow DuckDB writes, verify no loss

**Frontend Team:**
- Map renders 100+ ships, pans/zooms smoothly
- WebSocket real-time updates (to prepare for Phase 1)

**Query Engine Team:**
- Implement basic parser: "ships near X" "ships where speed > Y"
- Test: 200+ queries execute correctly

**Testing/QA Team:**
- 100+ integration tests written
- Performance benchmarks stable
- Nightly regression test suite running

**Deliverables:**
- Single binary, ~250-350MB, all features included
- Load test report: 500K events/sec, latency p50 <100ms, p99 <500ms
- DuckDB + Kuzu performance verified at scale
- 5 architecture decision records (ADRs) published
- 30-page architecture doc (all teams contributing sections)
- First security audit completed, 0 critical issues
- GOVERNANCE.md finalized, TSC established (5 members)

**Reality Check Gate:**
- Ingest 500K events/sec? ✓ (if target is met)
- Query latency <500ms at 1M entities? ✓
- Binary <350MB? ✓
- No data corruption on restart? ✓
- Auth framework ready? ✓

If any gate fails: issue resolved immediately or timeline slips.

**Demo:** To founding sponsors: Full pipeline working, real AIS data flowing, queries returning results.

---

### Phase 1: Maritime MVP (Months 2-10, February–October 2026)

#### Month 2: Core Engine Hardening

**Sprints 4-5 (Weeks 7-10)**

**Core Engine:**
- DuckDB schema finalized (handle all maritime data types)
- Kuzu sync robust (no data loss, atomic transactions)
- Memory profiling: 1M entities <3GB, 10M entities <15GB
- Startup time <5s

**Stream Processing:**
- Dedup accuracy measured on 30K real AIS events/sec (global scale)
- RocksDB state replication across binary restarts
- Backpressure prevents OOM under sustained 500K events/sec

**Connectors:**
- AIS connector: 30K events/sec sustained, <1% packet loss
- ADS-B connector: 1K events/sec, aircraft positions accurate
- HTTP connector: can poll 50 REST endpoints, retry on failure

**Entity Resolution:**
- Structural matching by MMSI: 99.9%+ accuracy
- Ship properties merge correctly when updates arrive from multiple sources

**Testing/QA:**
- 150 integration tests, all passing
- Load test: 10 hours of AIS data (270K events), all ingested correctly
- No memory leaks detected (valgrind clean)

**Deliverables:**
- Stable core binary
- Performance baseline: 500K events/sec, latency <100ms p50
- "What If We Scale 10x?" analysis (technical memo)
- Binary size <350MB confirmed

**Go/No-Go:** Core engine stable, no show-stoppers.

---

#### Month 3: Entity Resolution & Graph

**Sprints 6-7 (Weeks 11-14)**

**Entity Resolution:**
- Structural matching: MMSI (ships), ICAO (aircraft), port ID (ports)
- Master entity creation: first time we see Ship MMSI 211378120 → create entity, subsequent updates merge
- Duplicate detection: two AIS sources report same ship, recognized as same entity

**Graph Engine:**
- Kuzu loaded with 100K ships, ports, weather systems
- Graph sync every 30s, 100K→1M entity updates handled atomically
- Path queries: "ships heading to Rotterdam" <500ms, "ports reachable by fleet" <1s

**Query Engine:**
- ORP-QL v0.1 parser: basic spatial queries ("ships near X within Nkm"), filter queries ("ships where speed > Y")
- Query planner: reorder operations for <500ms latency
- Executor: retrieve results from DuckDB, format as JSON

**Frontend:**
- Entity inspector: click ship, see all properties, relationships, history
- Map shows 1000 ships smoothly

**Documentation:**
- "Architecture" doc (40 pages)
- "Connector Development" guide (30 pages, AIS example)
- ADRs: why structural matching in Phase 1, probabilistic in Phase 2

**Deliverables:**
- ORP-QL v0.1 fully functional
- Graph queries working at scale
- Entity resolution 99.9%+ accurate
- Entity inspector UI fully functional
- 60+ new integration tests

**Go/No-Go:** Entity resolution working, graph queries fast enough.

---

#### Month 4: Query Engine & Advanced Queries

**Sprints 8-9 (Weeks 15-18)**

**Query Engine:**
- Query planner optimizations: predicate pushdown, index usage
- Geospatial queries: "entities within 50km of [lat, lon]" fast
- Temporal queries (initial): "entities updated in last 6 hours"
- Joins: correlate ships with ports, weather with regions

**Connectors:**
- NOAA Weather connector: pull latest observations every 10 minutes
- OpenStreetMap connector: load harbor geometries, route data

**Security:**
- OIDC integration: can log in via external identity provider
- ABAC rules: "user can see Ship entities but not cargo_value property"
- Ed25519 signing: all ingested data signed with connector certificate
- Audit log: every query logged (who, what, when)

**Testing/QA:**
- 200+ integration tests
- Query latency benchmarks: simple <200ms, complex <1s
- ABAC permission checks: <10ms overhead

**Deliverables:**
- Complex queries execute in <1s at 1M entities
- OIDC auth working, ABAC enforced
- Ed25519 signatures verified on all data
- Audit log complete and queryable
- 50-page "Security Architecture" doc

**Reality Check Gate:**
- Entity resolution accuracy >99%? ✓
- Query latency <500ms simple, <3s complex? ✓
- Data quality scores functional? ✓
- Auth + ABAC working? ✓

---

#### Month 5: Console UI & Real-Time Updates

**Sprints 10-11 (Weeks 19-22)**

**Frontend:**
- Console UI complete: map + entity inspector + query bar + alert feed + timeline
- WebSocket real-time updates: ship positions update <100ms end-to-end
- Timeline scrubber: drag to see state at past times (animated map)
- Saved queries: save favorite queries for quick re-run
- Dark mode + accessibility (WCAG 2.1 AA)

**API:**
- REST API fully functional: GET /ships, GET /ships/{id}, POST /query
- OpenAPI spec generated, interactive docs at /api/docs
- Rate limiting: prevent abuse
- Pagination: handle large result sets efficiently

**Stream Processing:**
- Anomaly detection baseline: rule-based (speed spike >threshold, deviation >N degrees)
- Alert firing: when anomaly detected, notification sent to UI
- Monitor agents: simple rules engine for "if X then alert Y"

**Testing/QA:**
- 250+ integration tests
- UI performance tests: map renders 1000 entities smoothly (60fps)
- Load test: 50 concurrent users, latency <200ms

**Deliverables:**
- Full Console UI working
- Real-time updates via WebSocket
- REST API complete
- Anomaly detection (rule-based) working
- 50-page "API Reference" doc
- 30-page "Query Language Guide"

**Demo:** To board/investors: Live maritime dashboard, ships moving on map, querying in natural language (template-based).

---

#### Month 6: Natural Language Queries (Template-Based)

**Sprints 12-13 (Weeks 23-26)**

**AI/ML Team:**
- NL template matching: 100 pre-built query templates
- Example: "ships near Rotterdam" → matched to template, parsed, executed
- Fallback: if no template matches, suggest closest candidates to user
- Coverage: 80% of expected user queries

**Documentation:**
- "Getting Started" guide (20 pages)
- "ORP-QL Tutorial" (40 pages, step-by-step examples)
- 3 example scenarios (maritime, supply chain, climate) with walkthroughs
- FAQ (common questions)

**Community:**
- Contributing guide finalized
- 20 "Good First Issues" created
- First external contributor pull requests (documentation)

**DevOps:**
- Release automation: `cargo release` → signed binary + checksums
- Installer script: `curl -fsSL https://orp.dev/install | sh` works

**Deliverables:**
- Natural language queries for 80% of use cases
- 200+ pages of documentation (searchable)
- First external contributors merged
- Release infrastructure automated
- Binary download from orp.dev/install working

**Go/No-Go:** Ready for public preview?

---

#### Month 7: Monitoring, Alerting, Anomaly Detection

**Sprints 14-15 (Weeks 27-30)**

**Stream Processing:**
- Complex event pattern detection: "ship entered zone then exited zone in <2 hours"
- Alert deduplication: don't fire same alert 100 times/sec
- Alert actions: send to Slack, email, webhook

**Query Engine:**
- Monitor agents: users define alerting rules in simple DSL
- Example: "alert if speed > 25 and vessel_type = fishing_vessel in restricted_area"

**Frontend:**
- Alert feed: real-time notifications
- Alert acknowledgment: dismiss alert after reviewing
- Alert history: see past alerts, patterns

**Testing/QA:**
- Chaos testing: kill DuckDB process, verify recovery
- Anomaly detection accuracy: precision >80%, recall >85%

**Deliverables:**
- Anomaly detection working
- Alert firing and deduplication
- Alert feed in UI
- Chaos tests passing
- "Alerting Guide" (15 pages)

---

#### Month 8: Security Hardening & Pen Testing

**Sprints 16-17 (Weeks 31-34)**

**Security:**
- ABAC rules: complete implementation and testing
- Permission caching: <10ms latency on permission checks
- Audit log verification: cryptographic verification of integrity
- Data erasure: GDPR-compliant key destruction

**DevOps:**
- Kubernetes Helm chart: deploy ORP on K8s
- Health checks: liveness + readiness probes
- Graceful shutdown: in-flight queries complete, then exit

**Testing/QA:**
- External pen test: hire security firm, find + fix issues
- Fuzzing: continuous fuzzing of parser, crypto, query engine
- Regression tests: 300+ tests all passing

**Deliverables:**
- Pen test report: 0 critical, 0 high findings (or all fixed)
- Helm chart working, deploys in <5 minutes
- Audit log proven tamper-evident
- "Security Hardening Guide" (30 pages)
- ABAC playground (test rules without affecting production)

**Go/No-Go:** Ready for security-conscious customers?

---

#### Month 9: Documentation Sprint & Polish

**Sprints 18-19 (Weeks 35-38)**

**Documentation:**
- "Architecture Deep Dive" (60 pages): explain every team's subsystem
- "Troubleshooting Guide" (20 pages): common issues + fixes
- "Performance Tuning" (20 pages): how to optimize for your workload
- "Compliance & Security" (40 pages): SOC 2, GDPR, HIPAA considerations
- Video tutorials: 5 videos, 15 min each (install, first query, custom connector, ABAC setup, monitoring setup)
- External review: 3 external contributors review docs for clarity

**Frontend:**
- UI polish: design refinement, icon improvements, dark mode perfection
- Accessibility audit: WCAG 2.1 AA compliance verified
- Mobile responsiveness: works on tablets + phones

**Testing/QA:**
- Documentation testing: follow every tutorial, verify accuracy
- External testing: hire beta users to try ORP, report issues

**DevOps:**
- Release notes: detailed what's-new for every version
- Migration guide: if updating from older version, what to expect

**Deliverables:**
- 200+ pages of polished documentation
- 5 video tutorials
- External contributor review completed
- Documentation website (docs.orp.dev) live and searchable
- Zero documentation bugs (no broken links, code examples tested)

---

#### Month 10: Public Alpha Launch

**Sprints 20-21 (Weeks 39-42)**

**Community/DevRel:**
- HackerNews launch post: "I built a single-binary Palantir alternative"
- GitHub repo announced publicly: link from HN, Dev.to, etc.
- ProductHunt launch (Week 2)
- Live demo video: 5 minutes, shows ships on map → query → results

**Connectors:**
- 6 built-in connectors tested, documented, ready
- Connector development guide final
- 2 example community connectors (as reference for others)

**QA:**
- Load test: 500+ concurrent users on demo instance
- All known bugs fixed or documented as "future"
- Release candidate binary signed and checksummed

**Marketing/DevRel:**
- Case study draft: Planned first customer (Sarah, shipping company)
- Blog post: "How we built ORP in 10 months"
- Tweet thread: Technical insights + achievements

**Deliverables:**
- Public alpha release: GitHub, HackerNews, ProductHunt
- All code open source (Apache 2.0)
- 500+ downloads in first week (target)
- 20+ GitHub stars, 5+ first issues reported
- Community Slack launched

**Demo:** Public: Full maritime monitoring system, live on GitHub, downloadable, runnable.

**Go/No-Go:** Public alpha successful? Engaged community? Customers interested? Proceed to Phase 2.

---

## PART 3: CRITICAL PATH & DEPENDENCIES

### Dependency DAG (ASCII)

```
Week 1: Foundation
├── Core Engine (DuckDB + Kuzu schema)
├── Stream Processing (design)
└── Connectors (trait design)

Week 3: Prototypes converge
├── Core Engine (embedding + sync) ─→ Graph Engine (Kuzu loaded)
├── Stream Processing (dedup) ─→ Integration Testing
├── Connectors (AIS) ─→ Stream Processing (ingest)
└── Query Engine (parser) ─→ API (REST endpoints)

Week 5: Integration
├── Core Engine ↔ Stream Processing ↔ API ↔ Frontend
├── Query Engine ↔ Security (OIDC, ABAC)
└── Testing (all paths)

Month 2-3: Scale & Entity Resolution
├── Stream Processing (500K events/sec) → Entity Resolution (no loss)
├── Entity Resolution → Graph Engine (master entities synced to Kuzu)
└── Query Engine (advanced queries) → Testing (benchmark)

Month 4-5: Advanced Queries & Console
├── Query Engine (complex queries) → Frontend (render results)
├── Security (auth + signing) → API (enforce permissions)
└── Stream Processing (anomalies) → Frontend (alert feed)

Month 6-9: Polish & Documentation
├── All teams → Documentation (keep in sync)
├── Frontend → Testing (UI performance)
└── DevOps (release automation) → Community (distribution)

Month 10: Launch
└── All subsystems → QA (final regression) → Public release
```

### Critical Path (Longest Chain)

```
1. Core Engine foundation (Weeks 1-2) [BLOCKING]
   ↓
2. Stream Processing ingest (Weeks 3-4) [BLOCKING]
   ↓
3. Entity Resolution structural matching (Weeks 11-14, Month 3)
   ↓
4. Query Engine complex queries (Weeks 15-18, Month 4)
   ↓
5. Console UI (Weeks 19-22, Month 5)
   ↓
6. Public alpha launch (Month 10)

Total: 42 weeks (10 months) from foundation to launch

Parallel streams that don't block critical path:
- AI/ML NL queries (can be Phase 2 feature)
- Mobile app (can be Phase 2 feature)
- Cloud platform (can be Phase 2 feature)
- Advanced security (can be hardened after launch)
```

### Slack (Flexibility)

Teams that have built-in buffer:
- **Documentation** (Month 9 sprint is final polish, not blocking)
- **Mobile** (completely parallel, Phase 2)
- **Cloud** (completely parallel, Phase 2)
- **Simulation** (completely parallel, Phase 2)

Teams with zero slack:
- **Core Engine** (Week 1 blocking)
- **Stream Processing** (Week 3 blocking)
- **Entity Resolution** (Month 3)
- **Query Engine** (Month 4)

---

## PART 4: CODE REVIEW & PR STANDARDS

### Branch Naming Convention

```
feature/entity-resolution-structural    (new feature)
bugfix/dedup-false-positive             (bug fix)
refactor/query-planner-optimizer        (refactoring, no logic change)
docs/getting-started-guide              (documentation)
perf/duckdb-index-optimization          (performance improvement)
security/abac-permission-cache          (security hardening)
test/chaos-network-partition            (test infrastructure)
```

### Commit Message Format

```
<type>(<scope>): <subject>

<body>

<footer>

Example:
--------
feat(stream-processor): implement dedup window with RocksDB

Add RocksDB-backed deduplication window to prevent duplicate facts
entering the system. Window size configurable, default 24 hours.
Dedup state survives binary restart.

Benchmarks:
- 100K events/sec, <10ms latency
- False positive rate <0.01%

Fixes #42
```

**Types:**
- `feat` — new feature
- `fix` — bug fix
- `refactor` — code structure change, no logic change
- `perf` — performance improvement
- `test` — add/update tests
- `docs` — documentation
- `ci` — CI/CD changes
- `chore` — dependencies, tooling

**Scope:** Module/crate name (e.g., `stream-processor`, `query-engine`, `frontend`)

**Subject:** Imperative, present tense, <50 chars, no period

**Body:** Explain why, not what. Problem → solution. Benchmark results.

**Footer:**
- `Fixes #123` if closes issue
- `Relates #123` if related
- Breaking changes: `BREAKING CHANGE: ...`

### Pull Request Template

```markdown
## What

Brief description of changes.

## Why

Problem being solved. Context for reviewers.

## How

Technical approach. Design decisions.

## Testing

How was this tested? Benchmark results? Screenshots?

## Checklist

- [ ] All tests passing (`cargo test`)
- [ ] Clippy clean (`cargo clippy --all-targets`)
- [ ] Code formatted (`cargo fmt`)
- [ ] Benchmark results included (if perf-sensitive)
- [ ] Documentation updated (if API change)
- [ ] Commit messages follow convention
- [ ] No debug prints / TODO comments left

## Reviewers

@core-team/query-engine
@core-team/testing
```

### Review Requirements

**All PRs:**
- Minimum 2 approvals (at least 1 from Core Team)
- All CI checks pass (tests, clippy, fmt, size check)
- No outstanding conversations

**Large PRs (>500 lines):**
- 3 approvals
- 24-hour review window (async-friendly)
- Security team sign-off if touching auth/crypto

**Architectural changes:**
- RFC (Request for Comments) required before coding
- TSC approval
- Design doc in `/docs/rfcs/rfc-nnnn-title.md`

### Merge Strategy

**Strategy:** Squash merge (one commit per PR, clean history)

**Rationale:**
- Feature history is clear (one logical unit per commit)
- Bisecting works (each commit is buildable)
- Reverting is safe (one commit = one feature)

**Exception:** If PR has 10+ commits for pedagogical value (e.g., incrementally building a system), rebase merge is okay (keep history).

### CI/CD Gates

Every PR:
1. `cargo build --release` succeeds on all targets (Linux, macOS, Windows, ARM)
2. `cargo test --release` all tests pass
3. `cargo clippy --all-targets` no warnings
4. `cargo fmt -- --check` code formatted
5. Binary size check: binary doesn't grow >5% from previous release
6. Security audit: `cargo audit` no vulnerabilities
7. Documentation: code examples compile and run
8. Performance: benchmarks don't regress >10%

If any gate fails: PR cannot merge. Author fixes, pushes again. Checks re-run.

---

## PART 5: COMMUNICATION & COORDINATION (490 PEOPLE)

### Meeting Cadence

**Daily:**
- **Team standup** (15 min, 9:30am UTC)
  - Each team: 3-4 people speak, what done yesterday, blockers today
  - Async update in Slack if timezone unfavorable

**Weekly:**
- **Core-team sync** (60 min, Wed 10am UTC)
  - Core Engine, Stream Processing, Query Engine leads
  - Dependencies, blockers, schedule adjustments

- **Cross-team integration** (60 min, Fri 3pm UTC)
  - All team leads (20 people)
  - Demos, blockers, RFC discussions

- **Public Office Hours** (60 min, Thu 2pm UTC)
  - Open to all engineers, community
  - Q&A, architectural discussions, design feedback

**Bi-weekly:**
- **TSC meeting** (90 min, 1st & 3rd Monday)
  - 5 technical steering committee members
  - Major decisions, RFCs, governance

**Monthly:**
- **All-hands** (120 min, 1st Friday)
  - CEO updates, milestones, celebrations
  - Demo stage: 3-4 teams show shipping features
  - Q&A

- **Community call** (60 min, 3rd Thursday)
  - External contributors, users, sponsors
  - Feature announcements, roadmap discussion

### Slack Channel Structure

```
#general                — announcements, off-topic
├── #eng-random         — engineer memes, watercooler
├── #help-wanted        — blocked? ask here, anyone can unblock
├── #security           — security vulnerabilities, only core-team reads

#teams (access by team)
├── #team-core-engine
├── #team-stream-processing
├── #team-connectors
├── #team-entity-resolution
├── #team-query-engine
├── #team-graph-engine
├── #team-ai-ml
├── #team-frontend
├── #team-api
├── #team-security
├── #team-devops
├── #team-documentation
├── #team-testing
├── #team-enterprise
├── #team-cloud
├── #team-simulation
├── #team-mobile
├── #team-edge
├── #team-sdk
└── #team-community

#design (design discussions)
├── #rfc-discussion      — active RFCs being debated
├── #architecture        — major architectural questions
├── #performance         — performance investigations
└── #security-design     — security architecture

#development (active development)
├── #ci-status           — GitHub Actions status
├── #releases            — release notes, tagging
├── #breaking-changes    — announcement of API breaking changes
└── #database-schema     — schema changes, migrations

#external (community, sponsors)
├── #community-contributors
├── #academic-partnerships
└── #sponsor-updates
```

**Rules:**
- Team channels: internal only (private, restricted)
- Design channels: open to all engineers
- Announcements: always #general, can pin
- Decisions: link to ADR or RFC in Slack thread

### RFC (Request for Comments) Process

**Trigger:** Any architectural decision, API change, new team, or major refactoring.

**Process:**
1. Author writes RFC in `/docs/rfcs/rfc-NNNN-title.md` (template):
   ```
   # RFC NNNN: Title

   **Proposed by:** @alice
   **Status:** Draft
   **Discussion:** #rfc-discussion (Slack thread)

   ## Summary
   [1 paragraph]

   ## Motivation
   [Why this? What problem solved?]

   ## Design
   [How? Technical details, alternatives considered]

   ## Tradeoffs
   [What do we gain/lose?]

   ## Implementation Plan
   [Sprints, teams, timeline]
   ```

2. Post in #rfc-discussion, Slack thread for 48 hours (async comment period)

3. TSC reviews Friday, votes:
   - **Approved:** proceed with implementation
   - **Approved with concerns:** proceed, but address concerns in design
   - **Rejected:** requires pivot or more discussion

4. Approved RFCs move to "Active". Rejected move to "Closed".

5. Post-implementation, RFC moves to "Finalized" with link to PR.

**Examples of RFCs needed:**
- Adding Kuzu to architecture (Week 1) ← done
- WASM plugin system (month 6-7, Phase 1→2)
- Horizontal scaling strategy (month 18+, Phase 2→3)

### Architecture Decision Records (ADRs)

Every ADR in `/docs/architecture/adrs/`:

```
# ADR-001: Why DuckDB + Kuzu + RocksDB

**Status:** Accepted
**Context:** Need OLAP for aggregations, graph for relationships, stream state for dedup.
**Decision:** Use DuckDB (columnar), Kuzu (graph), RocksDB (KV store).
**Consequences:** Embeddable, no external services, single binary. But Kuzu adds 40MB binary size and 30s sync latency.
**Alternatives considered:** PostgreSQL+PostGIS (needs separate process), SurrealDB (too immature), Cassandra (overkill).
**Review date:** 2026-06-30 (will revisit if performance targets missed)
```

Full list of ADRs (Phase 1):
- ADR-001: DuckDB + Kuzu + RocksDB
- ADR-002: Async/Tokio for stream processor (not Flink)
- ADR-003: Structural entity matching Phase 1, probabilistic Phase 2
- ADR-004: WASM for Phase 2 plugins (not dynamically-loaded .so)
- ADR-005: ORP-QL language design (Cypher-like vs. SQL)
- ADR-006: OIDC + ABAC for security (not custom auth)
- ADR-007: React + Deck.gl for frontend (not Vue + Mapbox)
- ADR-008: Helm for Kubernetes (not custom deployment)
- ADR-009: Apache 2.0 license (not GPL)
- ADR-010: Single binary paradigm (not microservices)

### Escalation Path

**Problem:** Blocker, design disagreement, or missed deadline.

**Level 1:** Team lead + 1 peer review (sync within 24h)
→ Works 70% of the time

**Level 2:** Core team lead (e.g., Core Engine lead) + product lead (sync 48h)
→ Works 25% of the time

**Level 3:** TSC meeting (1st/3rd Monday, discuss + vote)
→ Works 5% of the time

**Example escalation:**
- Frontend team: "Graph rendering too slow, Kuzu queries not fast enough"
- Level 1: Frontend lead + Testing lead agree to benchmark Kuzu performance
- Benchmark shows: 100-node graph takes 3s to render (p99)
- Level 2: Escalate to Graph Engine lead, performance expectations recalibrated
- Resolution: Graph Engine optimizes Kuzu query batching, frontend adds pagination

### RFC & ADR Timeline (Phase 1)

- **Weeks 1-2:** ADRs for core architecture (DuckDB, Kuzu, async, plugin system)
- **Weeks 3-6:** RFC for ORP-QL language design, finalize before Month 3 query parser work
- **Month 3:** RFC for entity resolution matching strategy (structural vs. fuzzy)
- **Month 4:** RFC for ABAC rule design
- **Month 5:** RFC for anomaly detection framework
- **Month 6-8:** Minor RFCs for connectors, monitoring, observability
- **Month 9-10:** No new RFCs (stabilizing for launch)

---

## PART 6: HIRING PRIORITIES

### Hiring Phases

#### Wave 1: First 20 (Weeks 1-4)

**Role:** Chief Architect (1)
- 20+ yrs systems design, ex-Google/Microsoft/Apple
- Has shipped 2+ major systems
- Can make judgment calls in ambiguous situations

**Role:** Core Engine Lead (1)
- 15+ yrs databases, deep DuckDB/Kuzu knowledge or willing to learn
- Can optimize binary size, memory, query latency

**Role:** Stream Processing Lead (1)
- 15+ yrs high-throughput systems, async Rust
- Tokio expert or equivalent Rust async

**Role:** Product Lead (1)
- 10+ yrs product at data infrastructure / enterprise SaaS
- Can define roadmap, prioritize, talk to customers

**Role:** DevOps Lead (1)
- 10+ yrs CI/CD, Kubernetes, binary distribution
- Owns release process, cross-platform builds

**Role:** Testing Lead (1)
- 10+ yrs QA at scale, load testing, chaos engineering
- Defines quality bar

**Role:** Frontend Lead (1)
- 10+ yrs React + maps libraries (Deck.gl, Mapbox, CesiumJS)
- Can design high-performance UI

**Role:** Security Lead (1)
- 10+ yrs security architecture, cryptography, auth
- Can design OIDC, ABAC, Ed25519 signing

**Role:** Documentation Lead (1)
- 10+ yrs technical writing, ex-Palantir/Google documentation
- Can make ORP accessible

**Role:** Community Lead (1)
- 10+ yrs open-source, contributor relations, governance
- Built communities from 0 → 1000+ members

**Role:** Senior Rust Engineer (4)
- 5+ yrs Rust, systems programming
- Can implement subsystems end-to-end

**Role:** Senior Data Engineer (2)
- 5+ yrs SQL/OLAP/graphs
- Understands storage systems, indexing

**Role:** Senior Frontend Engineer (1)
- 5+ yrs React, D3/Deck.gl/CesiumJS

**Hiring Timeline:** Week 1 (offers), Week 3 (first day), Week 5 (fully onboarded)

**Ramp-up:** Month 1 = deep dives into architecture, by Week 3 contributing to PRs

---

#### Wave 2: Next 30 (Weeks 5-10)

**Composition:**
- 8 more Rust engineers (stream processing, query engine)
- 6 frontend engineers (map, UI, accessibility)
- 5 QA engineers (integration testing, load testing)
- 4 database engineers (DuckDB, Kuzu internals)
- 3 DevOps engineers (CI/CD, distribution, Kubernetes)
- 2 security engineers (testing, hardening)
- 2 documentation engineers (tutorials, API reference)

**Hiring Strategy:** Hire 3-4 per week starting Week 5

**Onboarding:** 2-week sprints where senior engineers pair with new hires

---

#### Wave 3: Next 50 (Weeks 11-20)

**By Month 4, hiring accelerates for Phase 1 crunch:**
- 15 connector engineers (AIS, ADS-B, HTTP, MQTT, weather, custom)
- 10 entity resolution engineers (structural + probabilistic matching)
- 8 API engineers (REST, GraphQL, WebSocket)
- 8 frontend engineers (inspector, alerts, timeline)
- 5 testing engineers (chaos testing, benchmarks)
- 4 security engineers (pen testing, ABAC enforcement)

**Hiring:** 3-5 per week through Month 4

---

#### Wave 4: Final 390 (Months 5-10)

**By Month 5, hiring ramps to full 490:**
- Connectors team (40 engineers total)
- Entity resolution (30)
- Query engine (30)
- Frontend (40)
- API (20)
- Security (20)
- Testing (20)
- DevOps (25)
- Documentation (15)
- Enterprise/Cloud (50)
- Mobile (15)
- Edge (10)
- SDK (20)
- Community (15)
- AI/ML (30)
- Simulation (15)
- Graph engine (25)

**Hiring Plan:** Identify senior engineers (directors/principles) who can each hire + mentor 5-10 engineers

**Compensation:**
- New hires: market rate + options (ORP is pre-revenue startup, but well-funded)
- Ex-FANG engineers: $200K–$400K + equity

**Culture:** Monthly all-hands, weekly office hours, Slack-first async communication for timezones.

---

## PART 7: DEFINITION OF DONE

### Feature

A user-facing feature is "done" when:

1. **Design approved** — RFC or design doc reviewed, TSC sign-off if architectural
2. **Code written** — All paths implemented, edge cases handled
3. **Tests pass** — Unit + integration tests, 100+ test cases
4. **Benchmarked** — Performance measured, no regression, documented
5. **Reviewed** — 2 approvals (1 from core team), all comments addressed
6. **Documented** — User-facing docs written, code examples tested
7. **Integrated** — Works with all dependent systems (tested end-to-end)
8. **Shipped** — Merged to `main`, included in next release

**Time to done:** 2-3 weeks for medium feature, 1 week for small

### Bug Fix

A bug fix is "done" when:

1. **Root cause identified** — Issue traced to source, documented
2. **Fix implemented** — Code change, minimal scope
3. **Regression test added** — Prevents bug from happening again
4. **Tests pass** — All existing tests still pass
5. **Reviewed** — 1 approval (fast-tracked if urgent)
6. **Verified** — Reproducer from issue now passes
7. **Shipped** — Merged, included in hotfix or next release

**Time to done:** <1 day for critical, <1 week for normal

### Connector

A connector is "done" when:

1. **Spec written** — Data source, update frequency, schema mapping
2. **Implemented** — Can ingest data, parse, map to ORP entities
3. **Benchmarked** — Throughput measured (events/sec), latency, error rate
4. **Tested** — Real data source integrated (not just synthetic), error handling
5. **Documented** — Step-by-step setup guide, example config, troubleshooting
6. **Reviewed** — Core team + one user sign-off
7. **Published** — Available in Tap Registry (Phase 2) or as built-in

**Time to done:** 2 weeks for built-in, 1 week for WASM plugin (Phase 2+)

### Documentation Page

A doc page is "done" when:

1. **Outline approved** — Structure reviewed by lead
2. **Written** — First draft complete, examples included
3. **Code tested** — All code examples compile and run
4. **Reviewed** — Technical review (accuracy) + writing review (clarity)
5. **Integrated** — Links from other docs, searchable, in table of contents
6. **Tested** — External person (not author) follows guide, verifies accuracy

**Time to done:** 3-5 days for 10-page doc, 1 day for quick reference

### Architectural Work (e.g., Kuzu Integration)

Done when:

1. **ADR written** — Design decision documented
2. **Prototype built** — Proof of concept, measured performance
3. **Reviewed** — TSC sign-off
4. **Integrated** — Works with upstream systems, no data loss
5. **Tested** — 100+ integration tests, load tested at scale
6. **Documented** — Architecture doc explaining decisions, tradeoffs, measurements
7. **Hardened** — Security review, error handling, edge cases

**Time to done:** 4-6 weeks for major subsystem

---

## PART 8: SPRINT CADENCE & METRICS

### Sprint Structure

**2-week sprints**, planning on Mondays, demos on Fridays (Week 2).

**Sprint template:**
- Monday 9am: Sprint planning (team lead + ICs, 90 min)
  - Review completed work, accept/reject
  - Define sprint goals (3-5 per team)
  - Assign stories to engineers
  - Identify blockers, dependencies, risks

- Daily: 15-min standup (async Slack update if needed)

- Friday 3pm: Sprint review + retrospective (60 min)
  - Each team demos 1-2 shipped features
  - What went well, what didn't
  - Adjustments for next sprint

### Metrics (Per Team)

**Velocity:**
- Actual work completed (story points)
- Trend: should stabilize by Sprint 4
- If declining: identify blocker (process, dependency, scope creep)

**Quality:**
- Bugs found per 1000 lines of code
- Target: <1 critical per sprint
- Tests passing: target 100%

**Performance:**
- For performance-sensitive teams: benchmarks vs. target
- Query Engine: p50 <200ms, p99 <1s (target)
- Stream Processing: 500K events/sec, zero loss (target)
- Frontend: 60fps, map render <1s (target)

**Cycle Time:**
- From "start work" to "merged" → should decrease over time
- Target by Sprint 10: <3 days for small feature

**Release Readiness:**
- Critical issues: 0 required for release
- High issues: <5 allowed
- Technical debt: tracked but deferred

### Release Milestones

**End of Phase 0 (Week 6):** Internal alpha
- Single binary working, core engine stable
- Not public

**End of Month 2 (Sprint 5):** Closed beta (sponsors only)
- All core subsystems working
- Shared with 3-5 early sponsors

**End of Month 5 (Sprint 10):** Limited public beta (HN comment)
- Announce we're building ORP
- 10 beta users invited to try

**End of Month 10 (Sprint 21):** Public alpha launch (HN front page)
- Full release, code open source
- 500+ downloads target

---

## PART 9: POST-PHASE-1 PLANNING (MONTHS 11-22, PHASE 2)

### Phase 2 Milestones (High Level)

- **Month 11-12:** AI/ML integration (llama.cpp + Phi-2 download), WASM plugin system launch
- **Month 13-14:** Probabilistic entity resolution, ORP-QL v0.2 (temporal queries)
- **Month 15-16:** ORP Cloud (SaaS) beta launch, mobile app beta
- **Month 17-18:** Supply chain template, 10+ new connectors
- **Month 19-20:** Federation v0.1 (hub-and-spoke data sharing)
- **Month 21-22:** Windows + ARM support, public Phase 2 release

### Team Expansion (Months 11-22)

- **Existing 100 engineers:** Focus on Phase 2 features
- **Hire 150 new engineers:** Cloud platform (30), mobile (15), connectors (40), AI/ML (20), simulation (15), SDK (20), edge (10), community (15)
- **Total by Month 22:** 250 engineers

---

**END OF BUILD_TEAMS_SPRINTS.md**

This is the definitive playbook for building ORP with 490 engineers.
