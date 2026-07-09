# PAPER-SMOKE-FOLLOWUP-01A — Current Smoke Findings Audit

## 1. Current HEAD

```text
d2857ff2 docs: reconcile market-hours proof sweep
```

## 2. Market-hours proof sweep commits found in `git log`

```text
d2857ff2 docs: reconcile market-hours proof sweep
f7546017 docs: prove v2 equity session source in market hours
21d3d221 docs: close market-hours no-trade proof
6120af47 docs: capture market-hours no-trade proof
a83315e9 docs: prepare market-hours proof sweep
96813548 docs: audit market-hours no-trade proof
```

Closure verdict recorded at
`docs/specs/market_hours_proof_sweep_01e_closure_decision.md`:
`MARKET-HOURS-PROOF-SWEEP-01: CLOSED_LOCAL`, `AUTON-NO-TRADE-02: CLOSED_LOCAL`,
`AUTON-NO-TRADE-01` parent: `CLOSED_LOCAL`, `ASSET-CORE-05K: CLOSED_LOCAL`
(`ASSET-CORE-05` parent unchanged, `PARTIAL / PRODUCTION-CONSUMPTION-OPEN`).
This bundle (`PAPER-SMOKE-FOLLOWUP-01`) does not reopen any of those verdicts.

## 3. The three follow-up findings (grounded against current repo state)

### Finding 1 — stale schema/runbook guard

**CONFIRMED against current committed migrations**, not just the prior
closure doc's claim. `core-rs/crates/mqk-db/migrations/0002_run_lifecycle.sql`
lines 4-19 add `armed_at_utc`, `running_at_utc`, `stopped_at_utc`,
`halted_at_utc`, `last_heartbeat_utc` to `runs`.
`core-rs/crates/mqk-db/migrations/0005_outbox_claim.sql` line 15 adds
`claimed_at_utc` to `oms_outbox`. Both migrations are committed and applied
by every daemon startup (`sqlx migrate run`), so these columns exist in
every current paper DB.

`docs/runbooks/market_hours_proof_sweep_01.md` (lines 62-67 and 76-83)
asserts the opposite: that `runs` "has no per-lifecycle-stage wall-clock
timestamp columns beyond `started_at_utc`" and that `oms_outbox` "has no
separate per-outcome wall-clock timestamp column for each lifecycle stage."
Both claims are false against current HEAD.

`scripts/guards/validate_market_hours_proof_sweep_01.ps1`'s `$ForbiddenColumns`
list (lines 40-49) actively fails the runbook validator if any of those six
real column names appear in the runbook text — i.e. the validator enforces
the stale, incorrect claim rather than catching a real mistake.

### Finding 2 — STEP 9B watchlist-v2 false failure

**CONFIRMED.** `scripts/windows/Start-PaperTradingSmoke.ps1` STEP 9B
(lines 993-1050) calls `GET /api/v1/watchlist/status` and hard-fails
(`exit 1`) unless `schema_version == 'watchlist-v2'` AND `symbols count > 1`
AND `approved_for_autonomous_paper == true`. There is no branch for a valid
single-symbol setup. `core-rs/crates/mqk-daemon/src/watchlist_intake.rs`
confirms the daemon itself supports both `watchlist-v1` (single-symbol) and
`watchlist-v2` (multi-symbol) schemas and reports `status_label()` values
including `"not_configured"` — the daemon already distinguishes these cases
truthfully; the smoke script's STEP 9B collapses all of them to one hard
failure.

This repo's current single-symbol configuration drives `MQK_STRATEGY_SYMBOL`
(default `AAPL`, see `Start-PaperTradingSmoke.ps1` line 651) directly into
the daemon's native single-symbol strategy path — it does not go through
`MQK_PAPER_WATCHLIST_PATH` / watchlist-v2 at all. STEP 9B blocks the
canonical operator startup script for this valid configuration.

### Finding 3 — no continuous intraday refresh loop

**CONFIRMED.** `scripts/windows/Refresh-IntradayMarketData.ps1` already
exists with `-CheckOnly`, `-Once`, and interval-loop modes, and already
writes fail-closed evidence
(`exports/market_data/intraday_refresh_*.json`,
`schema_version=intraday-refresh-v1`) consumed by the daemon's
`GET /api/v1/market-data/intraday-refresh/status` route
(`core-rs/crates/mqk-daemon/src/routes/transport_quality.rs`, handler
`intraday_refresh_status`, lines 553-755). That route reports
`truth_state` (`"active"` / `"no_evidence"` / `"backend_unavailable"` /
`"parse_error"`), `stale_or_missing_evidence`, `all_passed`, and
per-symbol `passed`.

`Start-PaperTradingSmoke.ps1` STEP 5B (lines 627-695) calls
`Prep-PremarketMarketData.ps1` once at startup only — no step in the smoke
script starts or verifies a continuous refresh loop, and no step queries
`/api/v1/market-data/intraday-refresh/status` before or during the
watch/observation window. This is why `DATA-FRESHNESS-READINESS-GATE-01`
correctly went stale (`intraday_bar_stale`) roughly 15 minutes into both
market-hours proof sweep observation windows — the one-shot top-off aged out
and nothing refreshed it.

## 4. Which finding affects core daemon safety

**None.** All three findings are script/runbook/documentation-level. The
daemon's own routes (`watchlist/status`, `intraday-refresh/status`,
`reconcile/status`, the freshness gate) already report truthful state in
every case observed. `DATA-FRESHNESS-READINESS-GATE-01` failing closed on
stale data during the market-hours proof sweep was **correct** behavior, not
a bug — the bug is that no operator workflow kept the data fresh.

## 5. Which finding affects smoke-script truthfulness

Findings 1 and 2. Finding 1: the runbook/guard actively misdescribes live
schema and would reject an accurate correction. Finding 2: STEP 9B produces
a false `MULTI_SYMBOL_SMOKE_BLOCKED_*` failure for a valid, non-multi-symbol
configuration — the canonical startup script cannot currently complete for
this repo's actual single-symbol setup without a code change to the script.

## 6. Which finding affects operator workflow

Finding 3. The operator has no single documented/scripted way to keep
intraday bars fresh for a market-hours smoke session longer than the
freshness-gate window (~15 min default) without manually re-running
`Refresh-IntradayMarketData.ps1` themselves.

## 7. Exact safe patch plan for Phases B-D

- **Phase B**: Rewrite the runbook's DB-schema section to instruct
  schema-discovery-first (`information_schema.columns`) rather than
  asserting which columns exist/don't exist as a fixed list. Replace the
  validator's `$ForbiddenColumns` allowlist-of-absence check with a check
  that the runbook instructs schema discovery and does not claim categorical
  non-existence of any column.
- **Phase C**: Add a single-symbol-valid branch to STEP 9B in
  `Start-PaperTradingSmoke.ps1` that reads `MQK_STRATEGY_SYMBOL` /
  `MQK_WATCHLIST_MODE` (or equivalent existing env surface) and passes when
  watchlist-v2 is genuinely absent/not required, while still hard-failing
  when watchlist-v2 is present-but-invalid or when multi-symbol mode is
  explicitly requested and watchlist-v2 is missing/mismatched.
- **Phase D**: Add an explicit opt-in flag to `Start-PaperTradingSmoke.ps1`
  (default off, so default smoke behavior is unchanged) that, when set,
  polls `/api/v1/market-data/intraday-refresh/status` and fails closed with
  actionable guidance if evidence is stale/missing, plus runbook guidance for
  operators to run `Refresh-IntradayMarketData.ps1` in loop mode alongside a
  market-hours smoke session.

## 8. Explicit non-goals

- No trading behavior change.
- No forced paper orders.
- No live orders.
- No provider calls in tests.
- No gate weakening — the freshness gate's fail-closed behavior in Finding 3
  is correct and must not be loosened.
- No config flag changes (`.env.local` untouched, no strategy threshold
  edits).
- No live routing enabled.
- No generated evidence, smoke logs, exports, or
  `MiniQuantDesk_Master_Patch_Ledger_v2_updated.md` staged.
