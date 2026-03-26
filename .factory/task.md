# ORP — Integrate 4 new modules

CRITICAL: DO NOT run cargo, npm, pip, rustc, or ANY build/install commands.
ONLY read and write source files. Nothing else.

## Files already written (just need wiring):
- crates/orp-stream/src/sanctions.rs (839 lines)
- crates/orp-security/src/rbac.rs (731 lines)
- crates/orp-core/src/server/notifications.rs (955 lines)
- crates/orp-core/src/server/users.rs (935 lines)

## What to do (ONLY edit .rs and .toml files):
1. crates/orp-stream/src/lib.rs — uncomment `pub mod sanctions` and its `pub use`
2. crates/orp-stream/Cargo.toml — add `anyhow = "1"` under [dependencies]
3. crates/orp-security/src/lib.rs — add `pub mod rbac;`
4. crates/orp-core/src/server/mod.rs — add `pub mod notifications;` and `pub mod users;`
5. Fix any type errors in the 4 new files by reading the existing codebase types
6. Fix Dashboard.tsx — add null guards: `connector?.stats?.events_per_sec ?? 0`

That's it. Just file edits. DO NOT compile or run anything.
