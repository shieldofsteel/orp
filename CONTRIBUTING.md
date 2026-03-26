# Contributing to ORP

Welcome. ORP is an open project and we're glad you're here.

This guide covers everything you need to make a great contribution: PR standards, commit format, branch naming, code review expectations, and step-by-step guides for adding connectors and frontend components.

---

## Table of Contents

1. [Before You Start](#before-you-start)
2. [Development Setup](#development-setup)
3. [Branch Naming](#branch-naming)
4. [Commit Format](#commit-format)
5. [Pull Request Standards](#pull-request-standards)
6. [Code Review](#code-review)
7. [How to Add a Connector](#how-to-add-a-connector)
8. [How to Add a Frontend Component](#how-to-add-a-frontend-component)
9. [Testing Requirements](#testing-requirements)
10. [Performance Guidelines](#performance-guidelines)
11. [Security Guidelines](#security-guidelines)

---

## Before You Start

1. **Check for an existing issue.** Search [issues](https://github.com/orproject/orp/issues) before starting work. For non-trivial changes, open an issue first to align on approach.
2. **Read [ARCHITECTURE.md](ARCHITECTURE.md).** Understand the crate structure and data flow before touching the codebase.
3. **For large features:** Open a GitHub Discussion or draft RFC before writing code. See [GOVERNANCE.md](GOVERNANCE.md) for the RFC process.
4. **Security issues:** Do **not** open a public issue. See [docs/SECURITY.md](docs/SECURITY.md#reporting-vulnerabilities) for the responsible disclosure process.

---

## Development Setup

### Requirements

- Rust 1.75+ (`rustup update stable`)
- `cmake`, `pkg-config`, `libssl-dev` (Linux) or Xcode CLI tools (macOS)
- Node.js 20+ (frontend only)

### Clone and Build

```bash
git clone https://github.com/orproject/orp.git
cd orp

# Build (dev)
cargo build

# Build (release — takes a few minutes)
cargo build --release

# Run with the maritime template
./target/release/orp start --template maritime
```

### Run All Checks

These must pass before opening a PR:

```bash
# All 203 tests
cargo test --workspace

# Zero Clippy warnings (errors in CI)
cargo clippy --all-targets -- -D warnings

# Format check
cargo fmt -- --check

# Audit dependencies for known vulnerabilities
cargo audit
```

---

## Branch Naming

Use the format: `<type>/<issue-id>-<short-description>`

| Type | When to Use |
|------|-------------|
| `feat/` | New feature or connector |
| `fix/` | Bug fix |
| `perf/` | Performance improvement |
| `docs/` | Documentation only |
| `refactor/` | Code restructuring, no behavior change |
| `test/` | Adding or fixing tests |
| `chore/` | Dependency bumps, CI changes |

**Examples:**

```
feat/ORP-42-kafka-connector
fix/ORP-87-ais-dedup-regression
docs/ORP-101-websocket-examples
perf/ORP-113-duckdb-batch-insert
```

---

## Commit Format

ORP uses [Conventional Commits](https://www.conventionalcommits.org/). This powers the automated CHANGELOG and semantic versioning.

```
<type>(<scope>): <short summary in imperative mood>

[optional body — wrap at 72 chars]

[optional footer: BREAKING CHANGE: ..., Closes #123]
```

### Types

| Type | When | Example |
|------|------|---------|
| `feat` | New behavior | `feat(connector): add Kafka source connector` |
| `fix` | Bug fix | `fix(stream): prevent dedup hash collision on restart` |
| `perf` | Performance | `perf(storage): increase DuckDB batch size to 5000` |
| `refactor` | Internal change | `refactor(query): extract planner into separate module` |
| `test` | Tests | `test(security): add ABAC policy edge cases` |
| `docs` | Docs | `docs(api): add WebSocket subscription examples` |
| `chore` | Tooling | `chore(deps): bump tokio to 1.37` |
| `ci` | CI/CD | `ci: add arm64 cross-compile job` |

### Scopes

`connector` · `stream` · `storage` · `query` · `security` · `audit` · `api` · `frontend` · `config` · `proto` · `geospatial` · `testbed` · `ci` · `deps`

### Breaking Changes

Add `BREAKING CHANGE:` in the footer:

```
feat(query)!: rename AT TIME syntax to SNAPSHOT AT

BREAKING CHANGE: queries using `AT TIME` must be updated to `SNAPSHOT AT`.
Closes #204
```

---

## Pull Request Standards

### Before Opening

- [ ] `cargo test --workspace` passes
- [ ] `cargo clippy --all-targets -- -D warnings` passes (zero warnings)
- [ ] `cargo fmt -- --check` passes
- [ ] New code has tests (see [Testing Requirements](#testing-requirements))
- [ ] Public API changes are documented in rustdoc
- [ ] Performance-sensitive changes include a benchmark in `orp-testbed`

### PR Description Template

```markdown
## Summary
<!-- One paragraph. What does this PR do and why? -->

## Changes
- 
- 

## Testing
<!-- How was this tested? What test cases were added? -->

## Performance Impact
<!-- If applicable: benchmark results before/after -->

## Breaking Changes
<!-- If yes: what breaks and how to migrate -->

## Related Issues
Closes #
```

### PR Size Guidelines

| Size | Lines Changed | Expectation |
|------|--------------|-------------|
| Small | < 150 | Single logical change, reviewed same day |
| Medium | 150–500 | Focused scope, reviewed within 48 hours |
| Large | 500–1500 | Needs prior issue discussion |
| XL | > 1500 | Split into multiple PRs, discuss in RFC first |

### Review Requirements

- **All PRs:** 1 approving review from a Committer or TSC member
- **Security changes** (`orp-security`, `orp-audit`): 2 reviews, at least 1 from TSC
- **Breaking changes:** TSC review + updated CHANGELOG entry
- **New connectors:** 1 review + integration test included

---

## Code Review

### Reviewer Expectations

- Review within **48 hours** of being assigned (working days)
- Be specific: point to the exact line, explain the concern, suggest an alternative
- Distinguish: 🔴 blocker · 🟡 suggestion · 🟢 nit
- Approve only when you'd be comfortable if it shipped today

### Author Expectations

- Respond to every comment, even if just "acknowledged" or "won't fix (reason)"
- Mark resolved threads as resolved after updating
- Push fixup commits, then squash before merge
- Don't force-push after review starts; use new commits

### What We Look For

1. **Correctness** — does it do what the PR says?
2. **Tests** — are failure modes covered?
3. **Performance** — no accidental O(n²) in hot paths
4. **Security** — no new attack surfaces (see [Security Guidelines](#security-guidelines))
5. **Error handling** — no `unwrap()` in non-test, non-trivial code
6. **Multi-tenancy** — any data access correctly scoped?
7. **Style** — consistent with surrounding code, no cargo-culted patterns

---

## How to Add a Connector

Connectors are located in `crates/orp-connector/src/connectors/`. Each connector is a Rust file that implements the `Connector` trait.

### Step 1: Create the connector file

```bash
touch crates/orp-connector/src/connectors/my_source.rs
```

### Step 2: Implement the `Connector` trait

```rust
// crates/orp-connector/src/connectors/my_source.rs

use async_trait::async_trait;
use tokio::sync::mpsc;
use crate::{Connector, ConnectorError, ConnectorHealth, ConnectorMetrics};
use orp_proto::{OrpEvent, EventPayload};

pub struct MySourceConnector {
    id: String,
    config: MySourceConfig,
}

#[derive(Debug, Clone, serde::Deserialize)]
pub struct MySourceConfig {
    pub host: String,
    pub port: u16,
    // ... your fields
}

impl MySourceConnector {
    pub fn new(id: String, config: MySourceConfig) -> Self {
        Self { id, config }
    }

    fn parse_raw_message(&self, raw: &[u8]) -> Option<OrpEvent> {
        // Parse your protocol → OrpEvent
        // Return None to drop malformed messages (logged as warnings)
        todo!()
    }
}

#[async_trait]
impl Connector for MySourceConnector {
    fn id(&self) -> &str { &self.id }
    fn connector_type(&self) -> &str { "my_source" }

    async fn start(&self, tx: mpsc::Sender<OrpEvent>) -> Result<(), ConnectorError> {
        // Connect to your source, read events, sign them, send to tx
        // This runs in its own Tokio task — panics are caught by the supervisor
        loop {
            // ... receive raw data
            // ... parse to OrpEvent
            // ... sign with Ed25519 (use orp_security::Signer)
            if tx.send(event).await.is_err() {
                break; // receiver dropped — binary is shutting down
            }
        }
        Ok(())
    }

    async fn stop(&self) -> Result<(), ConnectorError> {
        // Signal your connection to close
        Ok(())
    }

    fn health(&self) -> ConnectorHealth {
        ConnectorHealth::Healthy
    }

    fn metrics(&self) -> ConnectorMetrics {
        ConnectorMetrics::default()
    }
}
```

### Step 3: Register the connector type

In `crates/orp-connector/src/lib.rs`, add your type to `ConnectorType` enum and the factory function:

```rust
// In connector_type enum:
MySource,

// In build_connector():
"my_source" => Ok(Box::new(MySourceConnector::new(id, config.try_into()?))),
```

### Step 4: Add config schema

In `crates/orp-config/src/connectors.rs`, add a variant to `ConnectorConfig`:

```rust
#[serde(tag = "type", rename = "my_source")]
MySource {
    host: String,
    port: u16,
    // ...
}
```

### Step 5: Write tests

```rust
#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn test_parse_valid_message() { /* ... */ }

    #[tokio::test]
    async fn test_parse_malformed_returns_none() { /* ... */ }

    #[tokio::test]
    async fn test_connector_sends_events() { /* ... */ }
}
```

### Step 6: Document in the README feature table

Add a row to the Features table in `README.md`:

```markdown
| My Source connector | ✅ Stable | Brief description |
```

### Step 7: Add a template (optional)

If your connector suits a common deployment pattern, add a template YAML in `templates/`:

```yaml
# templates/my-domain.yaml
connectors:
  - id: "my-source-default"
    type: "my_source"
    config:
      host: "example.com"
      port: 1234
```

---

## How to Add a Frontend Component

The frontend lives in `frontend/` and is a React + TypeScript SPA using Deck.gl.

### Stack

- **React 18** with TypeScript strict mode
- **Deck.gl** for map layers
- **Zustand** for UI state (`useAppStore`)
- **TanStack Query** for server state
- **Tailwind CSS** for styling

### Step 1: Create the component

```bash
touch frontend/src/components/MyPanel/index.tsx
touch frontend/src/components/MyPanel/MyPanel.test.tsx
```

```tsx
// frontend/src/components/MyPanel/index.tsx
import React from 'react';

interface MyPanelProps {
  entityId: string;
}

export function MyPanel({ entityId }: MyPanelProps) {
  // Use hooks for data access — never fetch directly in components
  const { data, isLoading } = useEntityDetail(entityId);

  if (isLoading) return <LoadingSpinner />;

  return (
    <div className="orp-panel">
      {/* ... */}
    </div>
  );
}
```

### Step 2: Add a data hook (if needed)

```typescript
// frontend/src/hooks/useEntityDetail.ts
import { useQuery } from '@tanstack/react-query';
import { apiClient } from '../api/client';

export function useEntityDetail(entityId: string) {
  return useQuery({
    queryKey: ['entity', entityId],
    queryFn: () => apiClient.getEntity(entityId),
    staleTime: 5_000,
  });
}
```

### Step 3: Add to the app layout

Import and render your component in the appropriate parent (see `frontend/src/App.tsx`).

### Step 4: Write tests

Use React Testing Library:

```tsx
// frontend/src/components/MyPanel/MyPanel.test.tsx
import { render, screen } from '@testing-library/react';
import { MyPanel } from '.';

describe('MyPanel', () => {
  it('renders entity name when loaded', async () => {
    // ...
  });
});
```

### Step 5: Add a Storybook story (recommended)

```tsx
// frontend/src/components/MyPanel/MyPanel.stories.tsx
import type { Meta, StoryObj } from '@storybook/react';
import { MyPanel } from '.';

const meta: Meta<typeof MyPanel> = { component: MyPanel };
export default meta;

export const Default: StoryObj<typeof MyPanel> = {
  args: { entityId: 'mmsi:123456789' },
};
```

---

## Testing Requirements

| Code Type | Minimum Coverage |
|-----------|-----------------|
| New connector | Parse logic + send path + malformed input |
| Query planner changes | Routing logic for all query shapes affected |
| Security code | All policy paths, including denial cases |
| Storage writes | Transaction success + rollback on failure |
| API handlers | Happy path + 400/401/403/404/500 cases |
| Frontend components | Render + primary user interaction |

Tests live in `#[cfg(test)]` modules in the same file, or `tests/` directories for integration tests. The `orp-testbed` crate contains benchmarks and integration harnesses.

---

## Performance Guidelines

1. **Batch writes.** Never insert one event at a time into DuckDB. Use the batch insert pattern (see `orp-stream`).
2. **Avoid allocations in hot paths.** Profile before fixing. Use `cargo flamegraph` or `perf`.
3. **No blocking in async context.** `tokio::task::spawn_blocking` for CPU-heavy work; never call sync I/O in an async task.
4. **Benchmark significant changes.** Add a criterion benchmark in `orp-testbed` and include before/after numbers in your PR.
5. **Check the performance target table** in README.md. CI will alert if gates are exceeded.

---

## Security Guidelines

1. **Never log secrets.** Tokens, keys, and passwords must never appear in structured logs.
2. **No `unwrap()` in non-test code** unless provably infallible (document why).
3. **Parameterize all queries.** SQL and Cypher queries must use bound parameters, never string interpolation.
4. **ABAC on every data path.** Any function that returns entity data must call the ABAC enforcer. Tests must include denial cases.
5. **Secrets via environment.** Config files use `${env.KEY}` syntax. Never hardcode tokens or passwords.
6. **Responsible disclosure.** Security vulnerabilities go to `security@orp.dev`, not public issues. See [docs/SECURITY.md](docs/SECURITY.md).

---

## Questions?

- **Discord:** `#contributing` channel
- **GitHub Discussions:** for design and architecture questions
- **Issues:** for bugs and feature requests

We review all PRs promptly and appreciate every contribution, no matter the size.
