# ORP — Final Integration + LoginPage

Teams wrote ~10K new lines. Make it all compile and work.

## 1. Fix Compilation
Run `cargo check`, fix all errors. New files: graph_engine.rs, updated processor.rs, updated duckdb_engine.rs. Resolve import/type conflicts.

## 2. Create LoginPage
Write `frontend/src/components/LoginPage.tsx`: email+password form, "Login with SSO" button, store JWT in localStorage, error handling. Wire into App.tsx: show LoginPage if no token, main app if authenticated. Add logout to header.

## 3. Install Frontend Test Deps
Run `cd frontend && npm install` to pick up new vitest deps in package.json.

## 4. Verify
- `cargo test` — all must pass
- `cargo clippy` — zero warnings
- `cd frontend && npm run build` — must succeed
- Commit everything, push to origin

## Rules
- Fix don't rewrite. Only touch what's broken.
- Commit message: "feat: final A+ integration — graph engine, tests, OpenAPI, WCAG, auth"
