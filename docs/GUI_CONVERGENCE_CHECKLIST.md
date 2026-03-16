# GUI Convergence Checklist

Use this after applying all future GUI patches.

Last verified: 2026-03 (Hardening Series H-1 through H-4 complete)

## Compile and repair
- [ ] `npx tsc --noEmit` — zero TypeScript errors
- [ ] `npm run build` — clean build
- [ ] fix first error only, rebuild, repeat until clean

## Shell validation
- [ ] app loads
- [ ] left rail renders all major screens
- [ ] global status bar renders
- [ ] desk mode toggle persists
- [ ] no full-page vertical scrolling on desktop

## Screen validation
- [ ] dashboard renders
- [ ] execution renders
- [ ] risk renders
- [ ] reconcile renders
- [ ] portfolio renders
- [ ] ops renders
- [ ] runtime renders
- [ ] metrics / transport / topology render
- [ ] alerts / incidents / audit / artifacts / strategy render

## Truth-model validation (H-1 requirement)
- [ ] All 8 critical live-data screens use `if (truthState !== null)` hard-block pattern
- [ ] No screen renders live data under `stale` or `degraded` truth state
- [ ] No inline soft-notice pattern (`{truthState ? <Notice /> : null}`) on live-data screens
- [ ] `panelTruthRenderState` returns null (green) when all panel endpoints resolve
- [ ] `dataSource` exists on `SystemModel`; status bar shows source state
- [ ] `node --experimental-strip-types --test src/features/system/truthRendering.test.ts` — 18/18 pass

## Ops surface validation (H-2 requirement)
- [ ] Mode-change buttons are disabled with panel notice
- [ ] `/api/v1/ops/action` arm-execution → 200 accepted
- [ ] `/api/v1/ops/action` change-system-mode → 409 not_authoritative
- [ ] `/api/v1/ops/change-mode` is not mounted (404)
- [ ] `cargo test -p mqk-daemon --test scenario_gui_daemon_contract_gate` — 5/5 pass

## API authority validation (H-3 requirement)
- [ ] `invokeOperatorAction` does NOT fall through to legacy on 400/403/409 from canonical
- [ ] Legacy fallback only fires on network error or 404
- [ ] `onChangeMode` prop is NOT present on `OpsScreen`

## Daemon contract gate (H-4 requirement)
- [ ] `cargo test -p mqk-daemon` — all pass, zero failures
- [ ] `cargo clippy --workspace -- -D warnings` — zero errors
- [ ] `gui_daemon_contract_waivers.md` reflects current enforced + deferred state

## Daemon agreement validation
- [ ] `/api/v1/system/status` resolves cleanly (canonical preferred)
- [ ] `/v1/status` legacy fallback still resolves and maps correctly
- [ ] `/v1/trading/account` mapping does not break portfolio summary
- [ ] `/v1/trading/positions` mapping does not break positions
- [ ] `/v1/trading/orders` mapping does not break orders tables
- [ ] `/v1/trading/fills` mapping does not break fills tables

## Final hygiene
- [ ] Remove stale backup imports if any remain
- [ ] Commit as one GUI convergence checkpoint
