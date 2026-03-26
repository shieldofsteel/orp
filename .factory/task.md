# ORP — Fix Live UI Bugs

3 real bugs found from browser testing. Fix all.

## Bug 1: Frontend hardcodes localhost:9090
`frontend/src/hooks/useEntities.ts` and `useWebSocket.ts` hardcode API URL to port 9090. Fix: use relative URLs (`/api/v1/...`) so the frontend uses same origin as the server. The Axum server serves both the frontend AND the API.

## Bug 2: Sidebar has fake mock connectors
`frontend/src/components/Sidebar.tsx` has MOCK_CONNECTORS with "degraded" and "error" statuses. Remove ALL mock data. Only show real connectors from the API. If API returns empty, show "No connectors configured".

## Bug 3: Deck.gl crash "Cannot read properties of null (reading 'luma')"
MapView.tsx has a rendering error. The issue is likely entity data format mismatch — coordinates might be null or the layer config has wrong accessors. Fix:
- Add null guards on ALL getPosition/getColor accessors
- Filter entities with valid coordinates before passing to layers
- Ensure MapLibre map renders even with no entities (show the base map)
- Test: the map MUST show a visible map with tiles, even if no entities are visible

## Bug 4: ORP is NOT maritime-only
The frontend says "Maritime Operations" or similar. Fix:
- Header should say "ORP Console — Data Fusion Platform"
- Remove any maritime-specific hardcoding in the UI
- The sidebar, query bar, and entity inspector should be domain-agnostic

## After fixing
- `cd frontend && npm run build`
- Copy dist to where Axum serves it
- Rebuild: `cargo build --release`
- Start: `ORP_DEV_MODE=true ./target/release/orp start --dev`
- Verify: no console errors, map renders, entities visible

cargo test + clippy must pass. Commit + push.
