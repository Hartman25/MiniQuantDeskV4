# Paper Daily P&L Baseline Capture — 01A Current Truth & Action Design

Patch group: `PAPER-DAILY-PNL-BASELINE-CAPTURE-AND-OPERATOR-CLOSURE-01-COMBINED`
(equivalently referenced as `PAPER-DAILY-PNL-BASELINE-CAPTURE-01-COMBINED`,
the future-patch ID `paper_daily_pnl_baseline_01d_capture_boundary_decision.md`
§4 named), Phase A.

## 1. Current HEAD

`039f5eeb` (`docs: close paper daily pnl baseline`), confirmed via
`git log --oneline -1` at the start of this phase. Working tree clean,
only the pre-authorized untracked files (`smoke_logs/`,
`MiniQuantDesk_Master_Patch_Ledger_v2_updated.md`) present.

## 2. Current daily-P&L status

- Schema (`sys_account_equity_baseline`, migration `0045`) and DB helpers
  (`upsert_account_equity_baseline`, `fetch_account_equity_baseline_for_date`
  in `core-rs/crates/mqk-db/src/account_equity_baseline.rs`) are
  `CLOSED_LOCAL`.
- `GET /api/v1/portfolio/summary`'s read-side (`daily_pnl`,
  `daily_pnl_truth_state`, and four `daily_pnl_baseline_*` provenance
  fields on `PortfolioSummaryResponse`,
  `core-rs/crates/mqk-daemon/src/routes/portfolio.rs`) is `CLOSED_LOCAL`.
- **Capture is open**: confirmed by re-reading
  `docs/specs/paper_daily_pnl_baseline_01d_capture_boundary_decision.md`
  §2 — no CLI command, no HTTP write route, no automatic trigger exists
  anywhere in the repo. `rg "capture-account"` across `mqk-daemon`,
  `mqk-db`, `mqk-cli` returns zero matches prior to this bundle.

## 3. Why capture must be explicit and operator-controlled

Per CLAUDE.md's fail-closed and no-synthetic-truth invariants, a baseline
row is a financial-truth artifact `daily_pnl` depends on. An automatic
trigger (timer, tick-based, or session-boundary-detected) would risk
capturing at an unproven moment (e.g. mid-reconcile, during a partial
broker snapshot, or before the daemon's own session/calendar logic has
converged) with no operator visibility into *why* a particular value was
recorded. An explicit operator-invoked action means every row's
provenance includes a real caller-supplied trading date and reason,
matching `.claude/rules/audit_repo_truth_rules.md`'s deterministic-event
discipline.

## 4. Why the capture path must read the daemon's current `broker_snapshot`

`AppState.broker_snapshot` (`core-rs/crates/mqk-daemon/src/state.rs`) is
the same in-memory `Option<mqk_schemas::BrokerSnapshot>` that
`portfolio_summary`, `portfolio_positions`, `portfolio_open_orders`, and
`portfolio_fills` already read from — it is the daemon's single source of
current account equity/cash. `mqk-cli` has no equivalent broker-snapshot
seam (confirmed: `rg "action_key|OperatorAction" core-rs/crates/mqk-cli`
only matches unrelated diagnostics/run commands), so building capture in
the CLI would mean either constructing a new broker adapter client
(real scope, real network-call risk) or duplicating the daemon's snapshot
state — both violate minimal-scope discipline. The daemon route is the
only place this data already exists.

## 5. Selected mechanism: extend `/api/v1/ops/action`

Confirmed via `core-rs/crates/mqk-daemon/src/routes.rs`:
`/api/v1/ops/action` (`POST`, handler `ops_action` in
`src/routes/control_plane.rs`) is registered on the `operator` sub-router,
which is wrapped in `token_auth_middleware` (`.layer(...)` at the end of
`build_router`) — i.e. it is already authenticated exactly like
`arm-execution`, `flatten-paper-positions`, and `request-mode-change`.//
`ops_action` dispatches on `body.action_key: String` via a plain `match`,
and existing arms (`request-mode-change`, `flatten-paper-positions`)
already demonstrate the precedent for gate-checked DB writes with a
deterministic `Uuid::new_v5(&Uuid::NAMESPACE_DNS, seed)` audit ID
(`request-mode-change`'s `intent_id` at
`control_plane.rs:690`). A dedicated route was **not** selected: the
existing dispatcher already provides authentication, an
`OperatorActionResponse`/`OperatorActionAuditFields` response shape, and
Discord/audit-event plumbing other arms reuse, so extending it is smaller
than adding a new authenticated route from scratch.

New action key: `"capture-account-equity-baseline"`.

## 6. Exact request shape

Extend the existing `OpsActionRequest`
(`core-rs/crates/mqk-daemon/src/api_types.rs`) with one new optional
field — the same pattern `target_mode` (for `request-mode-change`) and
`symbol` (for `flatten-paper-positions`) already establish for
action-specific optional parameters:

```rust
pub struct OpsActionRequest {
    pub action_key: String,
    pub reason: Option<String>,
    pub target_mode: Option<String>,
    pub symbol: Option<String>,
    /// Required for "capture-account-equity-baseline": target trading date
    /// in "YYYY-MM-DD" form. Must be a real NYSE trading day per
    /// `NyseWeekdaysProvider`.
    pub trading_date: Option<String>,
}
```

Example call:

```json
{
  "action_key": "capture-account-equity-baseline",
  "reason": "Capture previous-session-close equity baseline for daily P&L",
  "trading_date": "2026-07-10"
}
```

`reason` is required (non-blank) for this action specifically — stricter
than the dispatcher's general "not required" doc comment, because this
action writes durable financial-truth data, unlike e.g. `arm-execution`.

## 7. Exact response shape

Reuse `OperatorActionResponse` (already the shape every `ops/action` arm
returns) plus one new action-specific optional field, following the exact
precedent `pending_restart_intent` sets for `request-mode-change` (present
only for its own action, `null` otherwise):

```rust
pub struct OperatorActionResponse {
    // ...existing fields unchanged...
    pub pending_restart_intent: Option<PendingRestartIntentSnapshot>,
    /// Present only when `action_key == "capture-account-equity-baseline"`
    /// and `accepted == true`. Null in every other case.
    pub captured_baseline: Option<CapturedAccountEquityBaselineSnapshot>,
}

pub struct CapturedAccountEquityBaselineSnapshot {
    pub trading_date: String,
    pub equity: f64,
    pub cash: f64,
    pub currency: String,
    pub captured_at_utc: String,
    pub captured_by: String,
    pub broker_snapshot_source: String,
    pub audit_event_id: String,
}
```

No new top-level response type is introduced — `blockers`/`warnings`/
`disposition`/`accepted` on `OperatorActionResponse` already cover every
failure case in the required-behavior list below.

## 8. Exact DB helper usage

Calls `mqk_db::upsert_account_equity_baseline` (already implemented,
`core-rs/crates/mqk-db/src/account_equity_baseline.rs`) exactly once per
accepted request, with:

- `trading_date`: parsed from the request.
- `equity_micros` / `cash_micros`: parsed from
  `AppState.broker_snapshot.account.{equity,cash}` via the existing
  `parse_decimal_micros` helper
  (`core-rs/crates/mqk-daemon/src/routes/helpers.rs`), matching the
  precision `resolve_daily_pnl` already assumes.
- `currency`: `AppState.broker_snapshot.account.currency`.
- `captured_at_utc`: `Utc::now()` at the moment of the accepted request
  (matches the `request-mode-change` arm's `let ts_utc = chrono::Utc::now();`
  precedent — an app-level timestamp, not a DB `DEFAULT now()`).
- `captured_by`: the constant `"operator:capture-account-equity-baseline"`
  (deterministic, not sourced from free-text operator input, so the
  audit-ID seed below stays reproducible for a given call's other fields).
- `broker_snapshot_source`: `st.broker_snapshot_source().as_str().to_string()`
  — the same real `BrokerSnapshotTruthSource` value already surfaced by
  `portfolio_positions`/`portfolio_open_orders`/`portfolio_fills`, not a
  hardcoded label.
- `audit_event_id`: see §10.

`fetch_account_equity_baseline_for_date` is not called by the write path
(the upsert result already returns the persisted row) but is reused by
the Phase C read-side proof and the Phase D read-only surface.

## 9. Exact market-calendar validation rule

Reuses the existing `NyseWeekdaysProvider` seam
(`core-rs/crates/mqk-daemon/src/state/market_calendar.rs`), the same
provider `resolve_daily_pnl`'s `most_recent_trading_day_before` already
depends on (`core-rs/crates/mqk-daemon/src/routes/portfolio.rs`). The
handler parses `trading_date` as `NaiveDate` (`"%Y-%m-%d"`, strict, no
fallback formats), then probes
`NyseWeekdaysProvider.session_for(date.and_hms_opt(18, 0, 0)?.and_utc()).is_trading_day`
— the identical 18:00-UTC-probe convention `most_recent_trading_day_before`
uses so the ET calendar date the provider derives matches the requested
date under both EST and EDT. A `false` result fails closed
(`non_trading_day`); this handler does **not** additionally require the
date equal "the required prior trading day" computed by
`resolve_daily_pnl` — the operator may capture for any real trading day
(e.g. today's close, or a specific prior date), and the read-side's own
existing `stale_baseline` / `baseline_unavailable` logic already reports
honestly whichever date ends up captured versus what the read-side needs.

## 10. Exact deterministic `audit_event_id` rule

`Uuid::new_v5(&Uuid::NAMESPACE_DNS, seed.as_bytes())`, mirroring the
`request-mode-change` arm's `intent_id` precedent exactly (same namespace
constant, same versioned-pipe-delimited-seed style). Seed:

```text
mqk.account-equity-baseline.v1|{trading_date}|{equity_micros}|{cash_micros}|{currency}|{captured_by}|{broker_snapshot_source}
```

Using the parsed integer `equity_micros`/`cash_micros` (not the raw
broker-string form) keeps the seed canonical regardless of incidental
string formatting differences (e.g. `"100000.00"` vs `"100000.0"`) the
broker adapter might emit for the same real value. A repeated call with
identical inputs (including a re-capture of the same `trading_date` at a
moment when equity/cash have not changed) produces the same
`audit_event_id`, satisfying `.claude/rules/audit_repo_truth_rules.md`'s
"a re-run of the same logical event must produce the same audit ID" rule.
A capture at a materially different equity/cash produces a different ID,
correctly reflecting that it is a different logical event even though it
upserts the same `trading_date` row (last-write-wins, per the DB helper's
existing doc comment).

## 11. Exact truth / failure states

All failures return `accepted: false` with a `disposition` string and a
`blockers` entry; nothing is silently defaulted:

| Condition | `disposition` | HTTP status |
|---|---|---|
| No DB pool configured | `db_unavailable` | 503 |
| No broker snapshot | `no_broker_snapshot` | 503 |
| Missing/blank `reason` | `missing_reason` | 400 |
| Missing `trading_date` | `missing_trading_date` | 400 |
| `trading_date` not parseable as `YYYY-MM-DD` | `invalid_trading_date` | 400 |
| `trading_date` parses but is not a real NYSE trading day | `non_trading_day` | 403 |
| Account `equity`/`cash` string not parseable as a decimal | `unparseable_account_values` | 503 |
| All gates pass | `applied` | 200, `captured_baseline` populated |

The 503-vs-400-vs-403 split matches the dispatcher's existing convention:
503 for "a required dependency/data source is absent" (`db_unavailable`
pattern from `flatten-paper-positions`), 400 for "the caller's request is
malformed" (`invalid_target_mode` pattern from `request-mode-change`),
403 for "the request is well-formed but a safety gate refuses it"
(`not_paper_mode`/`reconcile_not_ok` pattern from
`flatten-paper-positions`).

## 12. Exact tests

Phase B adds `core-rs/crates/mqk-daemon/tests/scenario_paper_daily_pnl_baseline_capture_01.rs`
covering (numbered `PDBC-01` onward): unauthorized refusal (token-required
mode, no/wrong bearer token), no-DB refusal, no-broker-snapshot refusal,
missing-reason refusal, missing/invalid/non-trading-day `trading_date`
refusals, successful capture writing exactly one row with correct
provenance, idempotent re-capture (same `trading_date` twice still yields
exactly one row), a zero-row-count-change proof for both `oms_outbox` and
`oms_inbox` across a successful capture call (mirroring the existing
`select count(*) from oms_outbox`/`oms_inbox` before/after pattern in
`scenario_broker_position_baseline_adoption_01.rs`), and a deterministic
`audit_event_id` reproducibility check (same inputs twice -> same ID).
Phase C adds the capture -> `portfolio_summary` read-side loop proof to
the same file or a focused sibling file. All DB-backed tests skip
gracefully without `MQK_DATABASE_URL` pointing at the local
`mqk-paper-postgres` (port 5440), matching every prior test file in this
patch lineage. No provider, broker, or network call in any test.

## 13. Non-goals

- No trading, no order submission, no outbox/inbox writes of any kind.
- No provider, broker, or network call in tests or in the capture handler
  itself (the handler only reads `AppState.broker_snapshot`, an in-memory
  value already populated by the daemon's existing broker adapter code
  path — this action never calls a broker or provider directly).
- No auto-capture: this action only ever runs when an authenticated
  operator POSTs it.
- No fabricated baseline: every field written comes from the real
  `broker_snapshot` and the real caller-supplied `trading_date`; there is
  no synthetic or inferred value anywhere in the write path.
- No historical backfill: this action captures exactly the one
  `trading_date` the caller supplies, once, per call. It does not iterate
  over or guess any other date.
- No DB migration: `0045_account_equity_baseline.sql` already provides
  every column this bundle needs; confirmed by re-reading
  `core-rs/crates/mqk-db/src/account_equity_baseline.rs` and the table
  schema in `paper_daily_pnl_baseline_01e_closure_decision.md` §11 — no
  new column or table is required.
