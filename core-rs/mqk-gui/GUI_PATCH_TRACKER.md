# MiniQuantDesk GUI Patch Tracker (Source of Truth)

Owner: Zach
Mode: **Option A** — GUI is a client; daemon is the control-plane (HTTP/SSE).
Rule: **One patch at a time.** Each patch includes exact files + tests.

---

## Status Legend

- TODO
- IN-PROGRESS
- DONE
- BLOCKED (note why)

---

## Hardening Series (GUI/Daemon Operator Console, completed 2026-03)

These patches hardened the GUI to institutional-grade truth enforcement and operator safety.
Completed before the roadmap patches below.

### H-1: Truth-state hard-closure at all screen boundaries
**Status:** DONE
**Files:** `DashboardScreen`, `ExecutionScreen`, `RiskScreen`, `ReconcileScreen`, `PortfolioScreen`,
`SessionScreen`, `RuntimeScreen`, `OpsScreen`, `truthRendering.test.ts`
**What changed:** All 8 critical live-data screens use `if (truthState !== null)` hard-block.
Previously `stale` and `degraded` fell through silently or produced only a soft inline notice.
7 new tests added. 18/18 truth tests pass.

### H-2: Ops contract closure — `/api/v1/ops/action` mounted
**Status:** DONE
**Files:** `OpsScreen.tsx`, `api_types.rs` (`OpsActionRequest`), `routes.rs` (handler + mount),
`scenario_gui_daemon_contract_gate.rs` (5th test)
**What changed:** `/api/v1/ops/action` dispatches arm/disarm/start/stop/halt (200),
change-system-mode (409 CONFLICT), unknown key (400). Mode-change buttons disabled with
operator notice. Contract gate: 5/5 tests pass.

### H-3: Canonical route authority tightening
**Status:** DONE
**Files:** `api.ts` (`invokeOperatorAction`), `OpsScreen.tsx` (`onChangeMode` removed),
`screenRegistry.tsx`
**What changed:** `invokeOperatorAction` no longer falls through to legacy on 400/403/409.
Legacy fallback only on network error or 404. TypeScript zero errors. 18/18 truth tests pass.

### H-4: Contract gate/waiver burn-down
**Status:** DONE
**Files:** `gui_daemon_contract_waivers.md`, `scenario_daemon_routes.rs`
**What changed:** Waivers doc updated with `/api/v1/ops/action` enforced + `/api/v1/ops/change-mode`
intentionally-unmounted. Stale `broker_config_present.is_null()` corrected to `== false`.
Full daemon test suite: all pass.

### H-5: Full screen truth-state closure (11 remaining screens)
**Status:** DONE
**Files:** `AlertsScreen`, `TransportScreen`, `TopologyScreen`, `IncidentsScreen`, `MetricsScreen`,
`MarketDataScreen`, `StrategyScreen`, `ConfigScreen`, `ArtifactsScreen`, `AuditScreen`,
`OperatorTimelineScreen`
**What changed:** All 11 screens changed from partial `if (truthState === "unimplemented" || ...)`
guard to canonical `if (truthState !== null)` hard-block. `stale` and `degraded` now block
everywhere, not just on the 8 original live-data screens. TSC clean. 18/18 truth tests pass.

### H-6: Action catalog derived from daemon truth (superseded by PC-4 below)
**Status:** DONE
**Files:** `api.ts` (`buildActionCatalog`, `resolvedStatus` extraction)
**What changed:** `actionCatalog` was hardcoded `[]`. Now derived via `buildActionCatalog(resolvedStatus,
connected)` which reads `execution_armed`, `kill_switch_active`, `runtime_status` from the
daemon-fetched `SystemStatus`. Correct arm/disarm/start/stop/kill-switch entries surface
automatically from live state. TSC clean.
**Note:** Superseded by PC-4 (daemon-backed catalog). `buildActionCatalog` is fully removed in PC-4.

### H-7: Dead mode-change control paths removed
**Status:** DONE
**Files:** `useOperatorModel.ts` (`requestModeChange` removed), `screenRegistry.tsx`
(`changeMode` removed from `ScreenRenderContext`), `AppShell.tsx` (`handleChangeMode` removed)
**What changed:** `handleChangeMode` → `requestModeChange` → `requestSystemModeTransition` →
`/api/v1/ops/change-mode` (404) chain is fully deleted. Zero grep hits. TSC clean.

### H-8: Legacy fallback authority propagation
**Status:** DONE
**Files:** `api.ts` (lines 715–740)
**What changed:** `portfolioSummary`, `positions`, `openOrders`, `fills` now extract a
`…Canonical` boolean. `usedMockSections.push` fires when the canonical endpoint failed
(not just when the mapped result is null). Legacy-path data with fabricated zeros now
propagates degraded authority through `panelSources` → `panelTruthRenderState` → screen
hard-block. TSC clean.

### H-9: Contract gate burn-down — promoted 2 waivered routes
**Status:** DONE
**Files:** `scenario_gui_daemon_contract_gate.rs` (6th test added),
`gui_daemon_contract_waivers.md` (2 entries promoted)
**What changed:** `/api/v1/system/config-diffs` and `/api/v1/strategy/suppressions` moved
from "Explicitly deferred" to enforced. New test
`gui_contract_recently_promoted_array_surfaces_have_expected_shape` proves 200 + empty
array in test state for both. Contract gate: 6/6 pass.

### PC-1: Final truth-model hardening — operator-console endstate verification
**Status:** DONE
**Files:** `api.ts` (fallback authority audit, `executionOrders`/`executionSummary` propagation)
**What changed:** Verified `portfolioSummary`, `positions`, `openOrders`, `fills` legacy
fallbacks already propagate degraded authority. Added explicit canonical guards for
`executionOrders` and `executionSummary`. TSC clean.

### PC-2: Legacy status fallback truth propagation
**Status:** DONE
**Files:** `api.ts` (status mock-section push when legacy fires)
**What changed:** When canonical `/api/v1/system/status` fails and `/v1/status` is used,
"status" is pushed to `usedMockSections`. Ops panel authority degrades to "placeholder".
Ops truth gate fires when only legacy status resolved.

### PC-3: requestSystemModeTransition fully removed
**Status:** DONE
**Files:** `useOperatorModel.ts`, `screenRegistry.tsx`, `AppShell.tsx`, `api.ts`
**What changed:** Entire mode-change chain (`handleChangeMode` → `requestModeChange` →
`requestSystemModeTransition` → `/api/v1/ops/change-mode`) deleted. Zero grep hits.
Comment documenting removal added to `api.ts`.

### PC-4: Daemon-backed Action Catalog (FINAL CLOSURE)
**Status:** DONE
**Files:**
- `core-rs/crates/mqk-daemon/src/api_types.rs` — `ActionCatalogEntry`, `ActionCatalogResponse`
- `core-rs/crates/mqk-daemon/src/routes.rs` — `ops_catalog` handler, mounted as public GET
- `core-rs/mqk-gui/src/features/system/types.ts` — `OperatorActionDefinition` union pruned to 7 daemon-supported keys; `enabled` + `disabledReason` fields added
- `core-rs/mqk-gui/src/features/system/api.ts` — `buildActionCatalog()` removed; catalog fetched from `GET /api/v1/ops/catalog`; catalog resolution before `dataSource` so failures degrade ops panel authority
- `core-rs/mqk-gui/src/features/system/sourceAuthority.ts` — `/ops/catalog` added to ops panel runtime hints; `actionCatalog` added to ops panel placeholder hints
- `core-rs/mqk-gui/src/features/system/mockData.ts` — `MOCK_ACTION_CATALOG` pruned to 5 daemon-supported entries with `enabled` field
- `core-rs/crates/mqk-daemon/tests/scenario_gui_daemon_contract_gate.rs` — `gui_ops_catalog_endpoint_is_daemon_authoritative` test added (7th test)
- `docs/ci/gui_daemon_contract_waivers.md` — `/api/v1/ops/catalog` added to enforced section
**What changed:** Action Catalog is no longer client-synthesized. Daemon serves `GET /api/v1/ops/catalog`
with state-aware `enabled`/`disabled_reason` per entry. Fantasy action keys removed from type union.
Catalog failure degrades ops panel truth authority and triggers truth gate. Contract gate: 7/7 pass.
TSC clean. 30/30 GUI tests pass.

---

## P0 — Foundation (make GUI a real workstation, keep current controls working)

### GUI-1: Dark theme + tabbed workstation shell (no new deps)
**Status:** DONE (Patch 1)  
**Files:**
- `core-rs/mqk-gui/src/App.tsx`
- `core-rs/mqk-gui/src/App.css`  
**Notes:** Keep existing daemon calls: `/v1/status`, `/v1/stream`, `/v1/run/*`, `/v1/integrity/*`.

### GUI-2: Connection config + environment switching (dev/stage/prod)
**Status:** Done patch 2 
**Goal:** Make daemon base URL configurable without editing source.
**Files (expected):**
- `core-rs/mqk-gui/src/config.ts` (new)
- `core-rs/mqk-gui/src/App.tsx`
- `core-rs/mqk-gui/.env`, `.env.example`  
**Acceptance:** GUI can target laptop/desktop daemon easily; no hard-coded URL.

### GUI-3: Alerts model (typed events + severity) + sticky banners
**Status:** TODO  
**Goal:** Replace “stringy logs” with a typed event stream.  
**Requires daemon changes:** see DAEMON-2 / DAEMON-3.

---

## P1 — Daemon becomes the single control-plane for Trading tab

### DAEMON-1: Trading read APIs (positions/orders/fills/pnl)
**Status:** TODO  
**Goal:** GUI Trading tab shows real state.  
**Files (expected):**
- `core-rs/crates/mqk-daemon/src/routes.rs`
- `core-rs/crates/mqk-daemon/src/*` (handlers)
- `core-rs/crates/mqk-schemas` (response structs)  
**Endpoints (proposed):**
- `GET /v1/trading/positions`
- `GET /v1/trading/orders`
- `GET /v1/trading/fills`
- `GET /v1/trading/pnl` (or summary)
**Acceptance:** Deterministic responses; no hidden I/O beyond the daemon’s existing sources of truth.

### DAEMON-2: Risk summary API (allocator view)
**Status:** TODO  
**Endpoints (proposed):**
- `GET /v1/risk/summary`  
**Must include:** exposure, leverage, limits, violations, halt reasons, integrity state.

### DAEMON-3: SSE topics expansion (structured, filterable)
**Status:** TODO  
**Goal:** SSE stream exposes typed topics beyond `{heartbeat,status,log}`.
**Proposed events:**
- `risk` (violations, limit hits)
- `execution` (orders/fills)
- `backtest_job`
- `research_job`
- `audit` (operator actions)
**Acceptance:** GUI can filter by topic; status events are machine-parseable.

### GUI-4: Trading tab wired to DAEMON-1/2 (tables + summaries)
**Status:** TODO  
**Files (expected):**
- `core-rs/mqk-gui/src/App.tsx` (or split into components)
**Acceptance:** Renders without crashes when endpoints return empty arrays.

---

## P2 — Backtest as first-class daemon jobs (submit, observe, render artifacts)

### DAEMON-4: Backtest job submission + job state machine
**Status:** TODO  
**Goal:** Backtests run as daemon-managed jobs with IDs.
**Endpoints (proposed):**
- `POST /v1/backtest/jobs` (params: strategy, universe, from/to, bar size, seedless)
- `GET /v1/backtest/jobs` (list recent)
- `GET /v1/backtest/jobs/:id` (status/progress)
- `POST /v1/backtest/jobs/:id/cancel`  
**Acceptance:** Deterministic backtest runner; no wall-clock dependency in results.

### DAEMON-5: Backtest artifacts + metrics API
**Status:** TODO  
**Endpoints (proposed):**
- `GET /v1/backtest/jobs/:id/artifacts`
- `GET /v1/backtest/jobs/:id/artifacts/:name`  
**Artifacts (minimum):**
- summary.json (metrics)
- equity_curve.json or csv
- drawdown_curve.json or csv
- trades.csv
- params.json

### GUI-5: Backtest tab wired (form → submit → progress → charts)
**Status:** TODO  
**Notes:** Keep charts minimal initially (simple SVG/canvas) to avoid new deps.

---

## P3 — Research jobs + artifact browser (same pattern as backtest)

### DAEMON-6: Research job submission + artifact store
**Status:** TODO  
**Endpoints (proposed):**
- `POST /v1/research/jobs`
- `GET /v1/research/jobs`
- `GET /v1/research/jobs/:id`
- `GET /v1/research/jobs/:id/artifacts`
- `GET /v1/research/jobs/:id/artifacts/:name`

### GUI-6: Research tab + artifact browser
**Status:** TODO  
**Goal:** Run research jobs and view/download artifacts.

---

## P4 — Mobile-friendly + Remote-ready

### GUI-7: Responsive layout hardening + “compact mode”
**Status:** TODO  
**Goal:** Tablet/phone usability (sidebar collapses, tables stack, big buttons).

### DAEMON-7: AuthN/AuthZ for remote use (do NOT ship open endpoints)
**Status:** TODO  
**Options:** mTLS, bearer tokens, or reverse-proxy auth.  
**Acceptance:** Remote access without exposing control endpoints to the internet.

### GUI-8: Multi-host support (saved profiles)
**Status:** TODO  
**Goal:** Switch between laptop/desktop/remote daemon targets quickly.

---

## P5 — Operator-quality polish

### GUI-9: Command audit log + “who did what” timeline
**Status:** TODO  
**Goal:** Every GUI action emits an auditable event, viewable in GUI.

### GUI-10: Export packs (run + backtest + research bundle)
**Status:** TODO  
**Goal:** One-click export for reviews/debugging (zip of artifacts + logs).

---

## Patch 1 Notes (what you just approved)

- Patch 1 is **GUI-1** only.
- Backtest/Research tabs are placeholders until DAEMON-4+ are implemented.
- Next most important is **GUI-2 (configurable daemon URL)** so you can target multiple machines cleanly.
### REC-01: Reconcile mismatch detail route + fail-closed GUI detail gate
**Status:** DONE
**Files:**
- `core-rs/mqk-gui/src/features/system/truthRendering.ts`
- `core-rs/mqk-gui/src/features/system/truthRendering.test.ts`
- `core-rs/mqk-gui/src/features/system/api.ts`
- `core-rs/crates/mqk-daemon/src/api_types.rs`
- `core-rs/crates/mqk-daemon/src/routes.rs`
- `core-rs/crates/mqk-daemon/src/state.rs`
- `core-rs/crates/mqk-daemon/tests/scenario_gui_daemon_contract_gate.rs`
- `docs/ci/gui_daemon_contract_waivers.md`
**What changed:** Reconcile now requires both summary and mismatch detail truth. The daemon mounts `GET /api/v1/reconcile/mismatches` as a typed truth surface with `truth_state`. GUI only treats the endpoint as present when `truth_state === "active"`; `no_snapshot` and `stale` keep the panel fail-closed so empty mismatch rows cannot masquerade as authoritative zero mismatches.
