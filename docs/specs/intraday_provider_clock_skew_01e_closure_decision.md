# INTRADAY-PROVIDER-CLOCK-SKEW-01E — Closure Decision

`INTRADAY-PROVIDER-CLOCK-SKEW-OPERATOR-GUARD-01-COMBINED`, Phase E.

## 1. Is `INTRADAY-PROVIDER-CLOCK-SKEW-OPERATOR-GUARD-01-COMBINED` closed?

Yes — `CLOSED_LOCAL`. All required diagnostic surfaces exist, are tested,
and are wired: Phase A audit, Phase B route classifier + tests, Phase C
(no-op, folded into Phase B), Phase D smoke-script preflight guard + tests,
Phase E this closure doc. No provider/broker/network call was made by this
bundle; no order was submitted or forced; no daemon gate was weakened.

## 2. What was added

- **`docs/specs/intraday_provider_clock_skew_01a_current_truth_audit.md`** —
  grounded audit of the 2026-07-10 run (913s/900s/2152s), current evidence/
  route/script field inventory, and the safe Phase B–D plan.
- **`transport_quality.rs`/`api_types.rs`** — a pure `classify_proof_window_risk`
  function computing per-symbol `freshness_headroom_secs`,
  `staleness_overage_secs`, `near_expiry`, `proof_window_risk`
  (`low`/`medium`/`high`/`unknown`), and `operator_action` from already-parsed
  evidence fields, wired additively into `IntradayRefreshSymbolStatus`; a new
  `aggregate_proof_window_fields` rolling these into three new
  fail-closed-by-default top-level `IntradayRefreshStatusResponse` fields:
  `proof_window_ready`, `proof_window_risk`, `operator_action`.
- **`scenario_intraday_md_refresher_operator_surface_01.rs`** — IRS-12
  through IRS-15, proving the overage/near-expiry/ample-headroom/missing-
  field cases exactly per the mission's required behavior table.
- **`Start-PaperTradingSmoke.ps1`** — new `-MinFreshnessHeadroomSeconds`
  parameter (default `120`). `STEP 14C` (`-RequireIntradayRefresh`) now also
  fails closed on `proof_window_ready != true` (preferring the new route
  field), with a fallback to a manual per-symbol headroom comparison for
  older daemon builds lacking that field. Default script behavior
  (`-RequireIntradayRefresh` not passed) is unchanged.
- **`validate_intraday_provider_clock_skew_01a_audit.ps1`** and
  **`validate_intraday_provider_clock_skew_01d_smoke_guard.ps1`** — static,
  network/DB/daemon-free validators for the audit doc and the script guard.
- **`roadmap_completion_reconcile_01.md`** §8 — confirms this bundle touches
  no multi-asset roadmap row.

## 3. Does it weaken the freshness gate?

No. `DATA-FRESHNESS-READINESS-GATE-01` / `market_data_freshness.rs` /
`MQK_INTRADAY_BAR_MAX_AGE_SECS` were not touched. The new classifier reads
already-computed evidence fields; it does not change how, or how strictly,
the dispatch-tick gate itself evaluates freshness. The new smoke-script
guard is strictly additive and stricter (it can only fail a preflight that
previously would have passed) — it can never let a run through that the
prior script logic would have blocked.

## 4. Does it change strategy thresholds?

No. No strategy code, threshold, or config flag was touched.

## 5. Does it submit or force paper orders?

No. No order/OMS/broker code path was touched or exercised. All new tests
are fixture-file-only against a temp directory; no daemon, DB, or network
call occurs in any new test.

## 6. Does it add provider calls in tests?

No. Zero network calls in any test added by this bundle.

## 7. Does the status surface now show proof-window risk/headroom?

Yes. `GET /api/v1/market-data/intraday-refresh/status` now returns, per
symbol, `freshness_headroom_secs`, `staleness_overage_secs`, `near_expiry`,
`proof_window_risk`, `operator_action`; and at the top level,
`proof_window_ready`, `proof_window_risk`, `operator_action` — proven by
IRS-12..15.

## 8. Does the smoke script now guard against bars that are fresh but near expiry?

Yes. `STEP 14C` with `-RequireIntradayRefresh` fails closed when
`proof_window_ready != true` (or, on older daemon builds without that
field, when any symbol's `freshness_headroom_secs` is below
`-MinFreshnessHeadroomSeconds`, default 120s) — exactly the condition that
let the 2026-07-10 run pass its preflight and then fail 33 seconds later.

## 9. Exact command the operator should run next during market hours

```powershell
cd C:\Users\Zacha\Desktop\MiniQuantDeskV4

powershell -NoProfile -ExecutionPolicy Bypass -File .\scripts\windows\Start-PaperTradingSmoke.ps1 `
  -StartIntradayRefreshLoop `
  -IntradayRefreshIntervalSeconds 300 `
  -RequireIntradayRefresh `
  -MinFreshnessHeadroomSeconds 120 `
  -WatchSeconds 1800
```

## 10. Exact status route to check before retrying

```powershell
$base = "http://127.0.0.1:8899"
Invoke-RestMethod "$base/api/v1/market-data/intraday-refresh/status" | ConvertTo-Json -Depth 40
```

Proceed only when `truth_state="active"`, `all_passed=true`,
`proof_window_ready=true`, and per-symbol `passed=true` with
`near_expiry=false` and ample `freshness_headroom_secs`.

## 11. Next patch if the next run still blocks

If the run still blocks on `proof_window_ready != true` (or on `all_passed`
directly) repeatedly, despite retrying at different points in the trading
day, that is evidence the provider's publish cadence itself cannot reliably
stay under `MQK_INTRADAY_BAR_MAX_AGE_SECS` for this symbol/timeframe.
Recommended next patch:
`INTRADAY-DATA-PROVIDER-ALTERNATIVE-SOURCE-AUDIT-01-COMBINED`.

## 12. Next patch if a paper order/fill occurs but P&L visibility remains partial

Recommended next patch: `PAPER-PNL-OPERATOR-VISIBILITY-CLOSURE-01-COMBINED`.

## Final status

```text
INTRADAY-PROVIDER-CLOCK-SKEW-OPERATOR-GUARD-01-COMBINED: CLOSED_LOCAL
PAPER-TRADE-LIFECYCLE-PROOF-01: remains PARTIAL / DATA-FRESHNESS-BLOCKED until next market-hours proof
```

## Update: repaired by `INTRADAY-PROVIDER-CLOCK-SKEW-01F`

This closure was found, by a subsequent audit, to have shipped a proof-window
classifier that used the evidence file's **snapshot** bar age only, never
accounting for wall-clock time elapsed between evidence production and route
poll — exactly the mechanism that let the 2026-07-10 run pass STEP 14C
preflight and then fail 33s later. `INTRADAY-PROVIDER-CLOCK-SKEW-01F` (see
`docs/specs/intraday_provider_clock_skew_01f_effective_age_recompute_audit.md`
and `docs/specs/intraday_provider_clock_skew_01f_effective_age_closure_decision.md`)
repaired this additively: the route now recomputes `effective_latest_completed_bar_age_secs`
from snapshot age plus elapsed time and derives all proof-window fields from
that effective age. `01-COMBINED` is now `CLOSED_LOCAL / REPAIRED-BY-01F`.
