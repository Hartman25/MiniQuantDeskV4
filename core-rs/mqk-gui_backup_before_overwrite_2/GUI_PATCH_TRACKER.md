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