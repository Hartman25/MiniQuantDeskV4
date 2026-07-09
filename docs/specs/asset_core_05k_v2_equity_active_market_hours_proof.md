# ASSET-CORE-05K — v2-Equity-Active Market-Hours Proof

## Status

`ASSET-CORE-05K: CLOSED_LOCAL`

## Scope

Confirms — via a real NYSE regular-session wall-clock observation, not
injected/fixed timestamps — that setting the temporary process-scoped
environment override `MQK_RUNTIME_SESSION_SOURCE=v2_equity_active` drives
the daemon's session-source truth to the v2 candidate and that it matches
legacy behavior for equity trading hours. This is **not** a default
cutover decision and does **not** enable non-equity trading; it is an
isolated, single-window equity-only observation.

## Window

- Ran immediately after the `AUTON-NO-TRADE-02` lane (Phase B/C) was fully
  captured and closed, per this bundle's non-interleaving rule.
- 2026-07-09, real NYSE regular session (confirmed `session_in_window=true`
  throughout, wall-clock ~11:09am–11:53am CDT / 16:09–16:53 UTC).
- Separate daemon process from the `AUTON-NO-TRADE-02` lane: the prior
  daemon (PID 29036, legacy session source) was stopped cleanly
  (`stop-system` → `disarm-execution` → process termination, port
  confirmed free) before this window's daemon (PID 7296) was started with
  the temporary override.
- The override was set via `$env:MQK_RUNTIME_SESSION_SOURCE =
  "v2_equity_active"` in the PowerShell process that launched the daemon
  only — not written to `.env.local`, not exported globally, and not
  present in any committed file.
- Startup followed the same safety sequence as the canonical
  `scripts\windows\Start-PaperTradingSmoke.ps1` (identity check, WS-live
  wait, halt-clear check, broker-baseline adoption, reconcile hard gate,
  arm-if-needed, then wait for the autonomous session controller to start
  a run on its own — no manual `start-system` call), skipping only that
  script's STEP 9B multi-symbol watchlist gate, which is internal to that
  script and unrelated to session-source behavior (same gap already
  documented in the `AUTON-NO-TRADE-02B` summary for the legacy-lane
  daemon).
- Evidence formally captured via the purpose-built, fully read-only
  collector `scripts\windows\Collect-V2EquitySessionActiveProof.ps1
  -IncludeDb`, which calls only GET routes plus read-only
  `docker exec ... psql` queries — see
  `smoke_logs/v2_equity_active_session_proof_20260709_115316.json` /
  `.txt` (untracked, not staged).

## Required evidence (from the runbook)

| Field | Value | Source |
|---|---|---|
| `session_source_mode` | `v2_equity_active` | `system/status.runtime_session_source` |
| `production_cutover_enabled` | `true` (expected — this is exactly what the temporary override activates for this isolated window; not a default/global cutover) | `system/status.runtime_session_source` |
| `runtime_uses_session_v2` | `true` | `system/status.runtime_session_source` |
| `trading_uses_session_v2` | `true` | `system/status.runtime_session_source` |
| `legacy_session_state` | `regular_open` | `system/status.runtime_session_source` |
| `candidate_v2_session_state` | `regular_open` | `system/status.runtime_session_source` |
| `candidate_v2_parity_state` | `matched` | `system/status.runtime_session_source` |
| `active_source_used` | `true` | `system/status.runtime_session_source` |
| `session_window_state` | `in_window` (consistent with real NYSE wall clock at time of observation) | `autonomous/readiness` |
| `live_routing_enabled` | `false` (every poll across the window) | `system/status` |
| `asset_class_scope` | `equity_only` (unchanged) | `system/status` |
| `kill_switch_active` / `risk_halt_active` / `integrity_halt_active` | all `false` | `system/status` |

Legacy and v2-candidate session state matched (`regular_open` /
`regular_open`, `candidate_v2_parity_state=matched`) at every point
observed in this window — the v2 equity session source produces the same
wall-clock session-open determination as the legacy path during this real
NYSE session.

## Non-order-related blocker (does not affect this proof's scope)

The collector's own `safe_to_continue_to_manual_paper_proof` verdict was
`false`/`BLOCKED` (exit code 2), but the single reported blocker was
`intraday refresh all_passed=false stale_or_missing_evidence=true` —
the market-data freshness gate (`DATA-FRESHNESS-READINESS-GATE-01`),
identical in kind to the staleness noted in
`docs/specs/auton_no_trade_02b_market_hours_observation_summary.md`
(no continuous intraday ingest loop was running once the earlier smoke
script exited at its multi-symbol gate). This blocker governs whether it
would be safe to *additionally* attempt a manual paper-order proof in this
window — `ASSET-CORE-05K`'s scope is session-source parity only, which
does not require a fresh order attempt, and every session-source field
required above was confirmed regardless. No order was submitted or forced
in this window (`execution/summary`/`execution/flow` both reachable,
no order-submitting route was ever called).

## Hard boundaries confirmed

- No live routing (`live_routing_enabled=false` throughout).
- No live orders submitted.
- No paper order forced (no order-submitting route called).
- No strategy threshold changed.
- No non-equity trading enabled (`asset_class_scope=equity_only`
  throughout).
- `MQK_RUNTIME_SESSION_SOURCE=v2_equity_active` was temporary,
  process-scoped, and not persisted: not written to `.env.local`, not set
  as a user/system environment variable, and the daemon process that used
  it was the only process carrying it.
- No `.env.local` edit.
- Generated evidence (`smoke_logs/v2_equity_active_session_proof_*.json/.txt`,
  `exports/smoke/daemon_phaseD_*.stdout/.stderr.log`) is untracked and was
  not staged.

## Recommendation

This is a single-window equity-only observation, not a production cutover
decision. `REGISTRY-V2-PRODUCTION-CUTOVER-DECISION-01` remains a distinct,
unstarted item — this proof only confirms the v2 candidate is behaviorally
safe to observe further; it does not recommend flipping the default.
