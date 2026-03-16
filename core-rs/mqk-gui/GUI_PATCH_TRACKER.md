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