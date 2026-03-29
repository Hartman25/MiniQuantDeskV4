# MiniQuantDesk GUI Scaffold Patch 1

This patch replaces the prior single-file GUI shell with a structured operator-terminal scaffold aligned to the canonical GUI spec.

## What is included

- institutional shell layout
- global status bar
- preflight gate
- left command rail
- right-side safety/alerts rail
- bottom event feed rail
- screen registry with dashboard + placeholder screens
- system status/preflight polling model
- daemon URL settings hook using the existing localStorage/env config

## What is intentionally not included yet

- real control POST actions
- execution timeline rendering
- portfolio/risk/reconcile data tables
- WebSocket/SSE live stream wiring
- authentication / operator audit write paths

## Endpoint assumptions

The scaffold prefers these endpoints:

- `GET /api/v1/system/status`
- `GET /api/v1/system/preflight`

It also falls back to the older status endpoint:

- `GET /v1/status`

If the daemon is unavailable, the UI stays read-only and shows disconnected/default posture.

## Files replaced / added

- `src/App.tsx`
- `src/main.tsx`
- `src/styles.css`
- `src/app/AppShell.tsx`
- `src/components/...`
- `src/features/system/...`
- `src/features/screens/screenRegistry.tsx`
- `src/lib/format.ts`

## Verification status

I could not fully verify `npm run build` inside the sandbox because the local Node/React dependencies are not installed in this environment, so TypeScript could not resolve `react` / `react-dom` modules here.

On your machine, run:

```powershell
cd core-rs\mqk-gui
npm install
npm run build
```

If your repo already has `node_modules`, just run `npm run build`.
