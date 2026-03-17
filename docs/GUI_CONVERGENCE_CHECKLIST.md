# GUI Convergence Checklist

Use this after applying all future GUI patches.

Last verified: 2026-03-16 (Hardening Series H-1 through H-9, PC-1 through PC-4, REC-01 complete context; daemon-backed Action Catalog and mounted reconcile mismatch detail truth)

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

## Screen validation (all 20 registered screens)
- [ ] dashboard renders
- [ ] execution renders
- [ ] risk renders
- [ ] reconcile renders
- [ ] portfolio renders
- [ ] ops renders
- [ ] runtime renders
- [ ] session renders
- [ ] config renders
- [ ] strategy renders
- [ ] audit renders
- [ ] artifacts renders
- [ ] operatorTimeline renders
- [ ] alerts renders
- [ ] incidents renders
- [ ] metrics renders
- [ ] transport renders
- [ ] topology renders
- [ ] marketData renders
- [ ] settings renders (no truth gate — intentional, static config surface)

## Truth-model validation (H-1 + H-5 requirement)
- [ ] All 19 operator-facing screens use `if (truthState !== null)` hard-block (SettingsScreen intentionally excluded)
- [ ] No screen renders live data under `stale` or `degraded` truth state
- [ ] No inline soft-notice pattern (`{truthState ? <Notice /> : null}`) on live-data screens
- [ ] `panelTruthRenderState` returns null (green) when all panel endpoints resolve
- [ ] `dataSource` exists on `SystemModel`; status bar shows source state
- [ ] `node --experimental-strip-types --test src/features/system/sourceAuthority.test.ts src/features/system/truthRendering.test.ts` — 36/36 pass

## Ops surface validation (H-2 + PC-4 requirement)
- [ ] Mode-change buttons are disabled with panel notice and accurate explanation
- [ ] `/api/v1/ops/action` arm-execution → 200 accepted
- [ ] `/api/v1/ops/action` change-system-mode → 409 not_authoritative
- [ ] `/api/v1/ops/change-mode` is not mounted (404)
- [ ] `/api/v1/ops/catalog` → 200, 5 entries, state-correct enabled/disabled
- [ ] `cargo test -p mqk-daemon --test scenario_gui_daemon_contract_gate` — all pass

## API authority validation (H-3 + PC-1 requirement)
- [ ] `invokeOperatorAction` does NOT fall through to legacy on 400/403/409 from canonical
- [ ] Legacy fallback only fires on network error or 404
- [ ] `requestSystemModeTransition` function is NOT present in api.ts (removed PC-3)
- [ ] `change-system-mode` is NOT in `OperatorActionDefinition.action_key` union (removed PC-4)
- [ ] `onChangeMode` prop is NOT present on `OpsScreen` (removed H-7)

## Action catalog validation (PC-4 requirement — daemon-backed endstate)
- [ ] `actionCatalog` is fetched from `GET /api/v1/ops/catalog`, NOT synthesized client-side
- [ ] `buildActionCatalog` function does NOT exist in `api.ts` (removed in PC-4)
- [ ] Daemon `ops_catalog` handler returns exactly 5 entries with all required fields
- [ ] `change-system-mode` is NOT in the catalog (returns 409 from dispatcher)
- [ ] Catalog failure (unreachable endpoint) pushes "actionCatalog" to `usedMockSections`
- [ ] Catalog resolution happens BEFORE `dataSource` computation so failures reach `dataSource.mockSections`
- [ ] Ops panel "actionCatalog" in placeholder hints → panel degrades if catalog fails
- [ ] `OperatorActionDefinition.action_key` union is pruned to 7 daemon-supported keys only
- [ ] Fantasy keys (`enable-live-routing`, `pause-new-entries`, etc.) are NOT in the union
- [ ] `OperatorActionDefinition` has `enabled: boolean` and optional `disabledReason?: string`
- [ ] When canonical `/api/v1/system/status` fails, "status" is pushed to `usedMockSections`
- [ ] The ops truth gate fires ("unimplemented") when only legacy status resolved

## Fallback authority propagation (PATCH 4 + PC-3 requirement)
- [ ] `portfolioSummary`, `positions`, `openOrders`, `fills` push to `usedMockSections` when legacy fires (PATCH 4)
- [ ] `executionOrders` pushes "executionOrders" to `usedMockSections` when legacy fires (PC-3)
- [ ] `executionSummary` pushes "executionSummary" when canonical probe fails (PC-3)
- [ ] `status` pushes "status" when legacy `/v1/status` fires instead of canonical (PC-1)

## Daemon contract gate (H-4 + H-9 + PC-4 requirement)
- [ ] `cargo test -p mqk-daemon` — all pass, zero failures
- [ ] `cargo test -p mqk-daemon --test scenario_gui_daemon_contract_gate` — all pass
- [ ] `cargo clippy --workspace -- -D warnings` — zero errors
- [ ] `gui_daemon_contract_waivers.md` reflects current enforced + deferred state
- [ ] No mounted+tested routes remain in the waiver list

## Daemon agreement validation
- [ ] `/api/v1/system/status` resolves cleanly (canonical preferred)
- [ ] `/v1/status` legacy fallback still resolves and maps correctly; triggers "status" in mockSections
- [ ] `/v1/trading/account` mapping does not break portfolio summary; propagates degraded authority
- [ ] `/v1/trading/positions` mapping does not break positions; propagates degraded authority
- [ ] `/v1/trading/orders` mapping does not break orders tables; propagates degraded authority
- [ ] `/v1/trading/fills` mapping does not break fills tables; propagates degraded authority

## Final hygiene
- [ ] Remove stale backup imports if any remain
- [ ] Commit as one GUI convergence checkpoint
