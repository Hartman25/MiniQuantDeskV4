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

These patches hardened the GUI toward stricter truth enforcement and operator safety.
Historical test-count notes below reflect landing-time proof and may not match the current repo-wide totals.
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
**Files:** `gui_daemon_contract_waivers.md`, `scenario_gui_daemon_contract_gate.rs`
**What changed:** Waivers doc updated with `/api/v1/ops/action` enforced + `/api/v1/ops/change-mode`
intentionally-unmounted. Stale `broker_config_present.is_null()` corrected to `== false`.
Targeted daemon contract verification passed at landing time for this patch; do not read this entry as a claim about the entire current daemon suite.

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
**Status:** SUPERSEDED by REC-03/SS-01 below (truth-state wrappers replaced bare-array contract)
**Files:** `scenario_gui_daemon_contract_gate.rs` (6th test added),
`gui_daemon_contract_waivers.md` (2 entries promoted)
**What changed:** `/api/v1/system/config-diffs` and `/api/v1/strategy/suppressions` moved
from "Explicitly deferred" to enforced. New test
`gui_contract_recently_promoted_array_surfaces_have_expected_shape` proves 200 + empty
array in test state for both. Contract gate: 6/6 pass.

### REC-03: Config / suppressions truth closure — fake-zero to explicit not_wired
**Status:** DONE
**Files:** `api_types.rs` (`ConfigDiffsResponse`, `StrategySuppressionsResponse` added),
`routes.rs` (both handlers return wrapper with `truth_state="not_wired"`),
`api.ts` (IIFEs for both surfaces; ok:false on not_wired),
`ConfigScreen.tsx` (section-level "not wired" notice replaces empty DataTable),
`StrategyScreen.tsx` (section-level "not wired" notice for suppressions panel)
**What changed:** Both surfaces returned unconditional `HTTP 200 + []` — GUI `useArray`
treated these as authoritative zero rows. Now return structured wrapper with
`truth_state="not_wired"`. GUI IIFEs emit `ok:false` → sections render honest
"not yet wired to a durable source" notice instead of empty tables.

### SS-01: Strategy summary truth closure — synthetic daemon row removed
**Status:** DONE
**Files:** `api_types.rs` (`StrategySummaryResponse` added),
`routes.rs` (`strategy_summary` handler rewritten: synthetic `daemon_integrity_gate` row removed),
`api.ts` (IIFE for strategy/summary; ok:false on not_wired → "strategies" in mockSections),
`sourceAuthority.ts` (comment updated to document not_wired authority collapse),
`scenario_daemon_routes.rs` (`api_strategy_summary_declares_not_wired` replaces
`api_strategy_summary_tracks_integrity_gate_truth`; config-diffs + suppressions assertions updated),
`scenario_gui_daemon_contract_gate.rs` (strategy assertions in semantics test updated;
`gui_contract_not_wired_surfaces_declare_truth_state` replaces
`gui_contract_recently_promoted_array_surfaces_have_expected_shape`),
`gui_daemon_contract_waivers.md` (strategy + config-diffs sections updated)
**What changed:** `/api/v1/strategy/summary` returned a synthetic `daemon_integrity_gate`
row reflecting daemon arm state — not a real strategy. GUI rendered it as a real strategy
row. Route now returns `StrategySummaryResponse{truth_state:"not_wired", rows:[]}`. GUI
IIFE emits ok:false → "strategies" in mockSections → panel authority "placeholder" →
`panelTruthRenderState` returns "unimplemented" → StrategyScreen hard-blocks.
Deferred until a real strategy-fleet source is wired.

### D1-R: Determinism cleanup — production control/runtime paths
**Status:** DONE
**Files:** `routes/control.rs`, `routes.rs`, `orchestrator.rs` (test annotation),
`state.rs` (`// allow:` annotations), `mqk-db/src/lib.rs` (`// allow:` annotations)
**What changed:**
- `Uuid::new_v4()` replaced with UUIDv5 in two operator audit-event paths:
  `write_control_operator_audit_event` (emergency run_id + event_id) and
  `write_operator_audit_event` (event_id). Both use deterministic key = `(run_id, event_type, ts_utc_micros)`.
  Wall-clock boundary is isolated to a single `Utc::now()` read per call;
  both the stored `ts_utc` and the derived `event_id` share that same read.
- `orchestrator.rs:2821` `Uuid::new_v4()` annotated `// allow: test-only` — isolated
  inside `#[cfg(test)]` helper, never on a production path.
- `state.rs` three `timestamp_millis()` calls annotated `// allow: ops-metadata`:
  SSE heartbeat ts (genuine real-time boundary) and two format-conversions from
  stored fields (not wall-clock reads).
- `mqk-db/src/lib.rs:1904` `timestamp_millis()` annotated `// allow: ops-metadata` —
  parsing a stored event timestamp, not a wall-clock read.
- `mqk-db/src/lib.rs:172` SQL `now()` in 24h restart-count query annotated
  `-- allow: ops-metadata` — operator display only, not an enforcement path.
- All six P0-1 guard violations resolved; guards now pass clean.
**Still deferred:** All genuine real-time boundaries (initial heartbeat, halt/stop
timestamps, SSE heartbeat, paper broker snapshot synthesis, deadman `now` injection)
remain as real-time calls — these are correctly scoped as live-boundary-only uses
and not semantic determinism violations.

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
TSC clean. Landing-time GUI truth tests passed; current totals should be checked from the live repo, not inferred from this historical entry.

### TI-1: Daemon test isolation / flake cleanup
**Status:** DONE
**Files:** `core-rs/crates/mqk-daemon/src/state.rs`,
`core-rs/crates/mqk-daemon/tests/scenario_daemon_runtime_lifecycle.rs`,
`core-rs/crates/mqk-daemon/tests/scenario_daemon_routes.rs`
**What changed:**
- `AppState::new_with_db_and_operator_auth(pool, operator_auth)` added to `state.rs`.
  Lets DB-backed tests inject auth mode without touching the process environment.
- `scenario_daemon_runtime_lifecycle.rs`: removed `static TEST_OPERATOR_TOKEN_INIT: Once`,
  `fn ensure_test_operator_token()`, and the `std::env::set_var("MQK_OPERATOR_TOKEN", …)`
  call they contained. All three `AppState::new_with_db(pool)` sites replaced with
  `AppState::new_with_db_and_operator_auth(pool, TokenRequired("test-operator-token"))`.
  `MQK_OPERATOR_TOKEN` is never written to the process environment by this file.
- `scenario_daemon_routes.rs`: added `EnvGuard` RAII struct (private `key`/`prior`
  fields; `absent(key)` constructor saves prior value and removes the var; `Drop` impl
  restores prior value or removes the var if it was absent). Updated
  `dev_snapshot_inject_refused_when_env_not_set` to open with
  `let _guard = EnvGuard::absent("MQK_DEV_ALLOW_SNAPSHOT_INJECT");` — the test now
  owns its own precondition and restores prior state on drop rather than relying on
  ambient CI environment or coverage redistribution.
**Defects fixed:**
  1. `MQK_OPERATOR_TOKEN` set via `Once` guard in lifecycle tests leaked into later
     tests in the same process (any test using `AppState::new_with_db()` would see
     the leaked token and silently get `TokenRequired` instead of
     `MissingTokenFailClosed`).
  2. `dev_snapshot_inject_refused_when_env_not_set` relied on the ambient process
     environment rather than explicitly controlling `MQK_DEV_ALLOW_SNAPSHOT_INJECT`.
     A prior incomplete fix removed `remove_var` without adding save/restore, making
     the test *more* environment-dependent. The `EnvGuard` closes this properly.
**Still deferred:** DB-backed lifecycle tests (`#[ignore]`) require `MQK_DATABASE_URL`;
they still use `Utc::now()` inside `broker_snapshot` / `execution_snapshot` fixture
setup — these are legitimate real-time boundaries in test scaffolding, not determinism
violations. No un-guarded env var mutations remain in any test file under the daemon crate.

### RD-01: Durable risk-denial history
**Status:** DONE (corrected — strict durable truth, no overclaiming)
**Files:**
- `core-rs/crates/mqk-db/migrations/0026_risk_denial_events.sql` (NEW)
- `core-rs/crates/mqk-db/src/lib.rs` (`RiskDenialEventRow`, `persist_risk_denial_event`, `load_recent_risk_denial_events`)
- `core-rs/crates/mqk-runtime/Cargo.toml` (`tracing` dep added)
- `core-rs/crates/mqk-runtime/src/orchestrator.rs` (best-effort durable persist before ring-buffer push)
- `core-rs/crates/mqk-daemon/src/routes.rs` (`risk_denials` handler — strict Option A: DB rows only when pool available; ring buffer only under explicit `"active_session_only"` label)
- `core-rs/crates/mqk-daemon/src/api_types.rs` (`RiskDenialRow.strategy_id: Option<String>`; `RiskDenialsResponse` docs for four truth states)
- `core-rs/mqk-gui/src/features/system/api.ts` (`"active_session_only"` + `"durable_history"` added to `RiskDenialsResponse` union)
- `core-rs/mqk-gui/src/features/system/types.ts` (`RiskDenialRow.strategy_id: string | null`)
- `core-rs/mqk-gui/src/features/risk/RiskScreen.tsx` (`row.strategy_id ?? "—"`)
- `core-rs/mqk-gui/src/features/system/sourceAuthority.ts` (`riskDenials.db` now lists `/risk/denials`)
- `core-rs/mqk-gui/src/features/system/mockData.ts` (`MOCK_RISK_DENIALS` uses `strategy_id: null`)
- `core-rs/crates/mqk-daemon/tests/scenario_gui_daemon_contract_gate.rs` (cluster 3 assertions updated; `"active"` → `"active_session_only"` for no-pool tests; `strategy_id: null` asserted)
- `core-rs/crates/mqk-daemon/tests/scenario_daemon_routes.rs` (4 DB-backed tests: persist/reload roundtrip, restart-safe durable_history route, active-pool-DB-only route, empty-table no_snapshot)
**What changed:**
- **Strict durable truth (Option A):** When a DB pool is available, the route returns ONLY rows from `sys_risk_denial_events`. The ring buffer is NOT merged. A denial whose `persist_risk_denial_event` call failed is absent from the durable response (honest — the row is not durable). No overclaiming.
- **`truth_state: "active"` is now fully durable:** pool available + loop running → DB rows only → restart-safe.
- **`truth_state: "active_session_only"` added:** no pool (test environments only; never in production) + loop running → ring buffer only → NOT restart-safe. Explicitly labeled.
- **`truth_state: "durable_history"`:** pool available + loop NOT running + DB has rows → historical durable rows, restart-safe.
- **`truth_state: "no_snapshot"`:** no durable rows and loop not running.
- **`strategy_id` is now `Option<String>` / `null`:** the risk gate path has no strategy attribution. The field is `null` in all real denial rows, never `""`. GUI renders `"—"` when null.
- **`sourceAuthority.ts`** updated: `riskDenials.db` now lists `/risk/denials` (DB-backed when pool available).
- **Mock data** updated: `MOCK_RISK_DENIALS` uses `strategy_id: null`.
- **DB write timing:** `persist_risk_denial_event` is called BEFORE `recent_denials.push_back`. A write failure logs a warning and leaves the row out of DB (and out of the durable route response), but does not abort execution.
**Durable source:** `sys_risk_denial_events` (PostgreSQL table, migration 0026).
**Unavailable fields:** `strategy_id` — always `null`; not available on the risk gate path; not fabricated.
**Verification run:**
- `cargo fmt --all` — PASS
- `cargo fmt --all -- --check` — PASS
- `cargo test -p mqk-daemon --test scenario_daemon_routes -- --test-threads=1` — 57 pass, 4 ignored (DB; require `--include-ignored`)
- `cargo test -p mqk-daemon --test scenario_gui_daemon_contract_gate -- --test-threads=1` — 21/21 PASS
- `cargo clippy -p mqk-daemon --all-targets -- -D warnings` — PASS (zero warnings)
- DB-backed tests (`risk_denial_persist_and_reload_roundtrip`, `risk_denials_route_returns_durable_history_after_restart`, `risk_denials_route_active_with_pool_returns_only_db_rows`, `risk_denials_route_no_snapshot_when_db_empty`) — NOT RUN (`MQK_DATABASE_URL` not available in this session); require `--include-ignored`

### AP-09: External broker operator-truth semantics
**Status:** DONE
**Files:**
- `core-rs/mqk-gui/src/features/system/types.ts` — `SystemStatus` + `SessionStateSummary` + `DEFAULT_STATUS`
- `core-rs/mqk-gui/src/features/system/api.ts` — `mapLegacyStatusToSystemStatus` + `unavailableSessionState`
- `core-rs/mqk-gui/src/features/system/truthRendering.ts` — `EXTERNAL_BROKER_GATED_PANELS`, `hasExternalBrokerContinuityGap`, AP-09 gate inserted before `isMissingPanelTruth`
- `core-rs/mqk-gui/src/features/system/truthRendering.test.ts` — 6 new AP-09 tests
- `core-rs/mqk-gui/src/features/system/mockData.ts` — `MOCK_STATUS` patched with 5 new required fields
- `core-rs/crates/mqk-daemon/tests/scenario_gui_daemon_contract_gate.rs` — status shape check expanded
**What changed:** `SystemStatus` now declares `broker_snapshot_source` (`"synthetic"|"external"`),
`alpaca_ws_continuity` (`"not_applicable"|"cold_start_unproven"|"live"|"gap_detected"`),
`deployment_start_allowed`, `daemon_mode`, `adapter_id`. `truthRendering.ts` gates execution
and reconcile panels on external WS continuity: `cold_start_unproven`/`gap_detected` → `no_snapshot`.
Portfolio is intentionally not gated (REST-independent truth). `DEFAULT_STATUS` and legacy path
fail-closed with `synthetic`/`not_applicable`. 6 new tests; 46/46 pass. Contract gate shape
check expanded to 9 status fields; 20/20 pass.

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
