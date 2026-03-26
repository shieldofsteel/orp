# ORP — Military-Grade Integration Build

5 teams wrote ~12,600 lines. Make it compile and work.

## 1. Fix Compilation
Run `cargo check`. New files: analytics.rs, threat.rs, layers.rs, generic_api.rs, syslog.rs, database.rs. Resolve all import/type conflicts. Update lib.rs exports for orp-stream and orp-connector. Update mod.rs for adapters.

## 2. Fix Frontend
New components: MapControls.tsx, SearchPanel.tsx, QueryConsole.tsx, Dashboard.tsx, EntityCard.tsx. Updated: MapView.tsx, EntityInspector.tsx, App.tsx. Run `cd frontend && npm run build` — fix any TS errors.

## 3. Wire Layers API
Add layer endpoints (GET/POST/PUT/DELETE /api/v1/layers) to http.rs router and handlers.rs.

## 4. Wire Analytics
Connect analytics engine (CPA, anomaly detection, threat scoring) to the stream processor pipeline. When entities update, run analytics checks.

## 5. Verify
- cargo test — all must pass
- cargo clippy — zero warnings
- frontend builds clean
- Server starts without errors

Commit + push. Message: "feat: military-grade COP — analytics, threat, layers, universal connectors, map controls, dashboard"
