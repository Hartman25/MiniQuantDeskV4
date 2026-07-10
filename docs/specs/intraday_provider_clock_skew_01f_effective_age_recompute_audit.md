# INTRADAY-PROVIDER-CLOCK-SKEW-01F — Effective-Age Recompute Audit

`INTRADAY-PROVIDER-CLOCK-SKEW-01F-AUDIT-EFFECTIVE-AGE-GAP-01`, Phase A.

## 1. Current HEAD

`9c581363` (`docs: close intraday provider freshness guard`), descendant of
`da83f038` (01D), `d2445310` (01B), `c336f79f` (01A). Working tree clean
except the allowed untracked `MiniQuantDesk_Master_Patch_Ledger_v2_updated.md`
and `smoke_logs/`.

## 2. Current status of `INTRADAY-PROVIDER-CLOCK-SKEW-OPERATOR-GUARD-01-COMBINED`

Marked `CLOSED_LOCAL` in the ledger (Phase E, `9c581363`). This audit finds
that closure was premature on one specific axis: the proof-window classifier
it shipped (Phase B / `01B`) does not account for wall-clock time elapsed
between evidence production and route poll. The 2026-07-10 live run this
bundle was built to prevent (STEP 14C preflight pass, then
`DATA-FRESHNESS-READINESS-GATE-01` failure 33s into the run) reproduces the
same class of failure through the *repaired* route, because the repair only
added *headroom classification*, not *effective-age recomputation*. This
patch (`01F`) repairs that specific gap. `01-COMBINED` remains accepted as
closed; this is an additive repair layered on top, not a reopening of 01A–01E.

## 3. Root mechanism

- `Refresh-IntradayMarketData.ps1` writes an evidence JSON file with
  `produced_at_utc` (the moment the script ran) and, per symbol,
  `latest_completed_bar_age_secs` — the bar's age **as measured at
  `produced_at_utc`**, i.e. a snapshot, not a live value.
- The daemon's dispatch-tick freshness gate
  (`DATA-FRESHNESS-READINESS-GATE-01` / `market_data_freshness.rs`, untouched
  by this patch) computes bar age **live**, against `Utc::now()` at the
  moment each tick runs.
- `GET /api/v1/market-data/intraday-refresh/status` (this route) may be
  polled by an operator or by `Start-PaperTradingSmoke.ps1` STEP 14C any
  amount of time after `produced_at_utc` — seconds to minutes later,
  depending on refresh-loop cadence and script timing.
- For the proof-window classifier to answer "is it safe to start a proof run
  **right now**" truthfully, it must reflect bar age **as of the moment the
  route is evaluated** — the live age — matching the live-age semantics the
  dispatch gate itself uses. Classifying on the snapshot value alone answers
  a different, already-stale question: "was it safe when the evidence was
  produced."

## 4. Current implementation finding (grounded in `transport_quality.rs`)

Confirmed by direct inspection, not inference:

- `classify_proof_window_risk` (lines ~391–439) takes
  `latest_completed_bar_age_secs` (the snapshot value), `max_allowed_age_secs`,
  and `passed` as its only inputs. It receives no timestamp and calls no
  clock function. It is a pure function of already-parsed evidence numbers.
- `parse_refresh_symbol` (lines ~456–591) calls
  `classify_proof_window_risk(latest_completed_bar_age_secs, max_allowed_age_secs, passed)`
  directly — the snapshot age, unmodified.
- The route handler `intraday_refresh_status` (lines ~661–883) does parse
  `produced_at_utc` from the evidence file (used only by `is_evidence_stale`,
  a coarse 24 h staleness check via `INTRADAY_EVIDENCE_STALE_SECS = 86_400`)
  but **never passes it, or any `Utc::now()` value, into `parse_refresh_symbol`
  or `classify_proof_window_risk`**.
- **Answer: the classifier uses snapshot age only. It does not account for
  elapsed time since `produced_at_utc` in any way.** `Utc::now()` is called
  exactly once in this route's code path (inside `is_evidence_stale`), for an
  unrelated 24-hour gross-staleness check, not for proof-window risk.
- No existing test proves a snapshot that was fresh at `produced_at_utc`
  becomes stale as route/wall-clock time advances. IRS-12..15 all use
  `recent_ts()` (a fixed ~60s-ago timestamp) as `produced_at_utc`, but their
  assertions only check the classifier's pure-number behavior on the
  snapshot age directly — none asserts an effective-age recompute.
- `Start-PaperTradingSmoke.ps1` STEP 14C (added by `01D`) consumes the
  route's top-level `proof_window_ready` field when present (falling back to
  a manual per-symbol `freshness_headroom_secs` comparison for older daemon
  builds). It depends on both the top-level field and, in the fallback path,
  the per-symbol field — but does no clock math of its own. It is a pure
  consumer of whatever the route reports, so it inherits this gap unchanged;
  no script change is required once the route is repaired.

## 5. Required repair

The route must, for each symbol (and at the top level via
`stale_or_missing_evidence`, unchanged):

1. Parse `produced_at_utc` once, at route level, into a `DateTime<Utc>`.
2. Capture a single `now_utc = Utc::now()` for the whole request so all
   per-symbol computations use one consistent instant (deterministic within
   a request; avoids intra-request clock skew across symbols).
3. Compute `evidence_elapsed_secs = max(0, now_utc - produced_at_utc)`.
4. Compute
   `effective_latest_completed_bar_age_secs = latest_completed_bar_age_secs + evidence_elapsed_secs`.
5. Feed `effective_latest_completed_bar_age_secs` (not the snapshot age) into
   `classify_proof_window_risk`, so `freshness_headroom_secs`,
   `staleness_overage_secs`, `near_expiry`, `proof_window_risk`, and
   `operator_action` are all derived from the effective age.
6. Derive top-level `proof_window_ready` from the effective-age-derived
   per-symbol risk (not solely from the pre-existing `passed`/`near_expiry`
   combination, which can miss an effective-age overage that started from a
   still-technically-passing snapshot).
7. If `produced_at_utc` is missing or unparseable, effective age must be
   `None` — which already routes through `classify_proof_window_risk`'s
   existing `_ => unknown` fail-safe branch, so `proof_window_risk="unknown"`
   and (via the aggregate) `proof_window_ready=false`.
8. If `now_utc < produced_at_utc` (clock skew or malformed future
   timestamp), clamp elapsed at `0` rather than going negative.

## 6. Required additive fields

Added to `IntradayRefreshSymbolStatus` (per symbol):

- `evidence_elapsed_secs: Option<i64>`
- `effective_latest_completed_bar_age_secs: Option<i64>`

`latest_completed_bar_age_secs` is preserved unchanged as the evidence
snapshot value, for backward compatibility with any existing consumer that
reads it directly. No fields are removed or renamed; no existing field's
JSON key or type changes.

## 7. Required tests

Added to `scenario_intraday_md_refresher_operator_surface_01.rs`, continuing
the `IRS-` numbering from `IRS-15`:

1. `IRS-16` — snapshot age 850 / max 900, `produced_at_utc` ~60s ago:
   `effective_latest_completed_bar_age_secs >= 910`,
   `staleness_overage_secs >= 10`, `proof_window_ready=false` even though the
   snapshot itself was within cap.
2. `IRS-17` — snapshot age 300 / max 900, `produced_at_utc` ~60s ago:
   effective age ~360, ample headroom remains, `proof_window_ready=true`.
3. `IRS-18` — snapshot age 780 / max 900, `produced_at_utc` ~60s ago:
   effective age ~840, headroom ~60, `near_expiry=true`,
   `proof_window_risk="high"`, `proof_window_ready=false`.
4. `IRS-19` — snapshot age 913 / max 900, already stale at production time,
   `produced_at_utc` ~60s ago: effective overage grows past the original
   13s; `proof_window_ready` stays `false` (was already `false`
   pre-repair — proves the repair does not regress the already-stale case).
5. `IRS-20` — `produced_at_utc` missing/malformed: `evidence_elapsed_secs`
   and `effective_latest_completed_bar_age_secs` both `None`,
   `proof_window_risk="unknown"`, `proof_window_ready=false`, with an
   actionable `operator_action`.

`IRS-12`..`IRS-15` (the pre-existing `01B` tests) are updated only insofar as
their numeric assertions must honestly reflect that they already use
`recent_ts()` (~60s-ago `produced_at_utc`) as their evidence timestamp, so
after this repair their classifier outputs reflect effective age, not
snapshot age. No test is weakened; assertions become tolerant range checks
(`>=`) where wall-clock elapsed time is inherently variable, instead of exact
equality on a value that depends on real elapsed time.

## 8. Non-goals

- No change to `DATA-FRESHNESS-READINESS-GATE-01` / `market_data_freshness.rs`
  / `MQK_INTRADAY_BAR_MAX_AGE_SECS`. No gate weakening of any kind.
- No strategy code or strategy threshold change.
- No live order submission, no forced paper order, no manual paper order
  submission.
- No provider, broker, or external network calls in any test — all tests
  remain fixture-file-only against a temp directory.
- No DB migration, DB mutation, or paper DB change.
- No `.env.local` edit, no persisted temporary env override.
- No crypto/futures/options/forex/rates trading enablement.
- No config flag change.

## Next phase

`INTRADAY-PROVIDER-CLOCK-SKEW-01F-EFFECTIVE-AGE-ROUTE-REPAIR-01` (Phase B):
implement the pure `effective_bar_age_secs` helper, thread `produced_at_utc`
+ `now_utc` through `parse_refresh_symbol` into `classify_proof_window_risk`,
add the two additive fields, repair `aggregate_proof_window_fields` to key
`proof_window_ready` off effective-age-derived risk, and add IRS-16..20.
