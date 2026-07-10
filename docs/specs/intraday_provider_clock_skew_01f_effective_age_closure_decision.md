# INTRADAY-PROVIDER-CLOCK-SKEW-01F — Effective-Age Repair Closure Decision

`INTRADAY-PROVIDER-CLOCK-SKEW-01F-LIVE-EFFECTIVE-AGE-RECOMPUTE-01`, Phase C.

## 1. Is `INTRADAY-PROVIDER-CLOCK-SKEW-01F-LIVE-EFFECTIVE-AGE-RECOMPUTE-01` closed?

Yes — `CLOSED_LOCAL`. Phase A audited and confirmed the gap. Phase B
repaired the route classifier to derive proof-window fields from effective
(elapsed-time-adjusted) bar age and added regression/proof tests. Phase C
(this doc) confirms the smoke-guard script already consumes the repaired
route correctly and requires no change. No provider/broker/network call was
made in any test; no order was submitted or forced; no daemon gate,
threshold, or config flag was touched.

## 2. What route fields were added

Per-symbol, on `IntradayRefreshSymbolStatus`:

- `evidence_elapsed_secs: Option<i64>` — seconds between the evidence
  file's `produced_at_utc` and the instant the route evaluated the request.
- `effective_latest_completed_bar_age_secs: Option<i64>` — the snapshot
  `latest_completed_bar_age_secs` plus `evidence_elapsed_secs`.

Both are additive; `latest_completed_bar_age_secs` (the evidence snapshot
value) is unchanged and still present for backward compatibility. No field
was removed, renamed, or retyped.

## 3. Does the route now compute effective age from snapshot age plus elapsed time since `produced_at_utc`?

Yes. `effective_bar_age_secs` (`transport_quality.rs`) computes
`evidence_elapsed_secs = max(0, now_utc - produced_at_utc)` and
`effective_latest_completed_bar_age_secs = snapshot_age_secs + evidence_elapsed_secs`,
using one `now_utc = Utc::now()` captured once per request and one parsed
`produced_at_utc` shared across all symbols in that request. Proven by
`IRS-16` (asserts `effective_age == 850 + elapsed` exactly).

## 4. Does proof-window readiness derive from effective age?

Yes. `classify_proof_window_risk` is now called with
`effective_latest_completed_bar_age_secs`, not the snapshot value, so
`freshness_headroom_secs`, `staleness_overage_secs`, `near_expiry`, and
`proof_window_risk` are all effective-age-derived. `aggregate_proof_window_fields`
was additionally repaired so top-level `proof_window_ready` requires every
symbol's `proof_window_risk` to be `"low"` or `"medium"` (not just the
absence of `near_expiry`/`"unknown"`), closing a gap where an
effective-age-only overage could previously have slipped through as ready.
Proven by `IRS-13`/`IRS-18` (still-passing snapshot becomes
`near_expiry`/`proof_window_ready=false` purely from elapsed time).

## 5. Does the smoke guard consume repaired route semantics?

Yes, unchanged. `Start-PaperTradingSmoke.ps1` STEP 14C already prefers the
route's top-level `proof_window_ready` field (line ~1570) and falls back,
only on older daemon builds lacking that field, to a manual per-symbol
`freshness_headroom_secs >= -MinFreshnessHeadroomSeconds` comparison
(line ~1584). Both fields are now effective-age-derived after this repair,
so the script inherits the correct behavior automatically. No script edit
was required or made.

## 6. Was the daemon freshness gate weakened?

No. `DATA-FRESHNESS-READINESS-GATE-01` / `market_data_freshness.rs` /
`MQK_INTRADAY_BAR_MAX_AGE_SECS` were not touched by any phase of this patch.
The repair only makes this diagnostic/status route's classifier stricter
(able to report `proof_window_ready=false` in cases it previously would
have missed) — it cannot let a run through that the prior route logic would
have blocked, only the reverse.

## 7. Was any threshold changed?

No. `NEAR_EXPIRY_THRESHOLD_SECS` (120s), `INTRADAY_EVIDENCE_STALE_SECS`
(86,400s), `MinFreshnessHeadroomSeconds` (script default 120), and
`MQK_INTRADAY_BAR_MAX_AGE_SECS` are all unchanged.

## 8. Were provider/broker/network calls made in tests?

No. All 20 tests in `scenario_intraday_md_refresher_operator_surface_01.rs`
(15 pre-existing + `IRS-16`..`IRS-20` new) write synthetic evidence JSON to
a `tempfile::tempdir()` and call the route in-process via `tower::ServiceExt::oneshot`.
Zero network, DB, or broker calls in any test added or modified by this patch.

## 9. Were any orders attempted?

No. No order/OMS/broker code path was touched or exercised by any phase.

## 10. Exact market-hours retry command

```powershell
cd C:\Users\Zacha\Desktop\MiniQuantDeskV4

powershell -NoProfile -ExecutionPolicy Bypass -File .\scripts\windows\Start-PaperTradingSmoke.ps1 `
  -StartIntradayRefreshLoop `
  -IntradayRefreshIntervalSeconds 300 `
  -RequireIntradayRefresh `
  -MinFreshnessHeadroomSeconds 120 `
  -WatchSeconds 1800
```

## 11. Is `INTRADAY-PROVIDER-CLOCK-SKEW-OPERATOR-GUARD-01-COMBINED` now accepted as repaired/closed?

Yes — `CLOSED_LOCAL / REPAIRED-BY-01F`. The specific gap this repair targets
(proof-window classification ignoring wall-clock time elapsed since evidence
production) is closed, proven by `IRS-16`..`IRS-20` and the updated
`IRS-12`..`IRS-15`. `PAPER-TRADE-LIFECYCLE-PROOF-01` remains `PARTIAL /
DATA-FRESHNESS-BLOCKED` — this patch repairs the operator-facing diagnostic
surface, it does not itself constitute a market-hours proof run.

## Final status

```text
INTRADAY-PROVIDER-CLOCK-SKEW-01F-LIVE-EFFECTIVE-AGE-RECOMPUTE-01: CLOSED_LOCAL
INTRADAY-PROVIDER-CLOCK-SKEW-OPERATOR-GUARD-01-COMBINED: CLOSED_LOCAL / REPAIRED-BY-01F
PAPER-TRADE-LIFECYCLE-PROOF-01: remains PARTIAL / DATA-FRESHNESS-BLOCKED until next market-hours proof
```

## Next patch recommendations

- Retry `PAPER-TRADE-LIFECYCLE-PROOF-01` using the repaired guard during
  market hours (command in §10).
- If repeated data-freshness blocks continue despite `proof_window_ready=true`
  at start: `INTRADAY-DATA-PROVIDER-ALTERNATIVE-SOURCE-AUDIT-01-COMBINED`.
- If a paper order/fill occurs but P&L visibility remains partial:
  `PAPER-PNL-OPERATOR-VISIBILITY-CLOSURE-01-COMBINED`.
