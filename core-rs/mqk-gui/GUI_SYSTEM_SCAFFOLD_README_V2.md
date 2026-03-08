# MiniQuantDesk GUI System Scaffold V2

This scaffold expands the Patch 1 shell into a broad end-to-end operator terminal baseline.

## What is included

- institutional-style desktop shell for `core-rs/mqk-gui`
- global status bar with live/paper/backtest visibility
- startup preflight gate
- dashboard screen
- execution screen with timeline workbench
- risk screen
- portfolio screen
- reconcile screen
- strategy matrix screen
- logs / audit screen
- operator action catalog screen
- settings / operations screen
- REST read model fetchers with mock fallback when daemon endpoints are absent
- guarded operator action POST helper with simulated fallback receipts

## What is not claimed

This is **not** a finished production GUI.

It is a full scaffolded GUI system that gives you real file structure, real TypeScript/React components, and a sane merge target into the repo. It still needs:

- local `npm install`
- local `npm run build`
- daemon endpoint alignment with actual payload shapes
- tighter UX polish
- real auth / control token behavior if you add that later
- final Tauri packaging validation

## Primary files

- `src/app/AppShell.tsx`
- `src/features/system/types.ts`
- `src/features/system/mockData.ts`
- `src/features/system/api.ts`
- `src/features/system/useOperatorModel.ts`
- `src/features/dashboard/DashboardScreen.tsx`
- `src/features/execution/ExecutionScreen.tsx`
- `src/features/risk/RiskScreen.tsx`
- `src/features/portfolio/PortfolioScreen.tsx`
- `src/features/reconcile/ReconcileScreen.tsx`
- `src/features/strategy/StrategyScreen.tsx`
- `src/features/audit/AuditScreen.tsx`
- `src/features/ops/OpsScreen.tsx`
- `src/features/settings/SettingsScreen.tsx`
- `src/features/screens/screenRegistry.tsx`
- `src/components/common/*`
- `src/styles.css`

## Merge guidance

You asked for full files so you can move them into your main repo later.

Treat this zip as a **staging repo**. Do not blindly overwrite your existing GUI without diffing first.

Recommended order:

1. compare `package.json`
2. compare `src/App.tsx`, `src/main.tsx`, `src/styles.css`
3. merge `src/features/system/*`
4. merge `src/components/*`
5. merge screen modules one at a time
6. run local build
7. patch endpoint payloads to your daemon reality

## Local validation

```powershell
cd core-rs\mqk-gui
npm install
npm run build
```

Then start patching screen-by-screen.

## V3 extension included in this artifact

This updated scaffold now includes institutional monitoring additions:
- OMS state machine visualizer
- execution trace viewer
- execution replay viewer
- dedicated metrics screen
- runtime/execution/fill-quality/reconcile/risk metric strip dashboards
- three-monitor desk layout guidance embedded in the dashboard
- API/type scaffolding for `/api/v1/oms/*`, `/api/v1/execution/trace/*`, `/api/v1/execution/replay/*`, and `/api/v1/metrics/*`
