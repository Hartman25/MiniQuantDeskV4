# V4-M2-CONCURRENT-STRATEGY-RUNTIME-FOUNDATIONS-01

Mission record for `feature/m2-concurrent-strategy-runtime-01`. Scope: Contract
rows C11 (multi-strategy / multi-symbol engine) and the C12/C13 edges it
touches. Local proof only; GitHub CI `DISABLED / NOT RUN`; no Paper/Live
activity, no broker call, no holdout access, no push. This document does not
edit the Master Program Plan and Ledger; proposed ledger text is at the end.

## What already existed (reused, not rebuilt)

Runtime identity is `HostPoolKey = (symbol, strategy_id, timeframe_secs)`, one
isolated `StrategyHost` per key (`DynamicSelectionHostPool`, `BTreeMap`-ordered,
duplicate keys refused). Per tick the execution loop evaluates every binding
sequentially in authority order (deterministic by construction, no worker
completion order exists), gathers all decisions, then Bundle 6 (the only
same-symbol arbiter), Bundle 5 (portfolio allocation), cap #6, and the single
strategy submission site (`decision::submit_internal_strategy_decision`, the
only caller is `loop_runner.rs`). Completed bars reach bindings through a
per-binding claim (`sys_autonomous_daily_binding_bar_dispatches`) and a keyed
deposit. This mission added no scheduler, allocator, netting rule or sizing
policy.

## Defect census

| # | Area | Finding | Disposition |
|---|---|---|---|
| 1 | Same-symbol arbitration | Bundle 6 defaults to `off` (and `shadow` passes through). A watchlist-v3 authority with two strategies on one symbol sent both decisions to submission, each sized against the same pre-trade position. Nothing forced Bundle 6 on. | FIX |
| 2 | Replay / double consumption | The host-pool confirm loop re-deposited its trigger on every attempt, before checking for the loop's evaluation row, and never drained on success. Reproduced: the claimed bar was evaluated twice (2 host calls), the second outside any dispatch claim. | FIX |
| 3 | Concurrent claims | Claim primitive is `INSERT .. ON CONFLICT DO NOTHING` + row lock; only sequential tests existed. | ALREADY CORRECT + PROVEN (new 8-way race test) |
| 4 | Identity / routing | Key is the full triple; duplicates refused; v3 shares one timeframe so a symbol cannot carry two timeframes. `per_candidate_timeframe_label` is keyed by symbol only, which is unreachable today (single shared timeframe, one survivor per symbol under Bundle 6). | ALREADY CORRECT (by construction); latent if per-binding timeframes are ever allowed |
| 5 | Stale / out-of-order bars | Wall-clock staleness gate per binding; window is ordered by `end_ts` in SQL. | ALREADY CORRECT + PROVEN (new tests) |
| 6 | Failure isolation | A binding without usable bars is skipped with a durable attributable row; a strategy panic or `on_bar` error halts the whole tick with zero submissions (documented fail-closed policy, M2.9). | ALREADY CORRECT + PROVEN |
| 7 | Restart | Unresolved claims never auto-redispatch; durable capital-fraction hosts recover from DB; in-memory deposits die with the process. | ALREADY CORRECT (existing proofs) |
| 8 | Risk / portfolio authority | One submission call site; Gates 0-7 incl. asset-class instrument context, explicit-size gate for non-equity, promotion, arm, run state. Positions and capital come from one snapshot per tick. | ALREADY CORRECT |
| 9 | Unsupported asset / mode | Refused at the submission choke point and broker capability; v3 is hard-locked to paper + Alpaca. | ALREADY CORRECT |
| 10 | Multi-binding global-critical stop | `DispatchClaimUnresolved` etc. stop the remaining bindings for the tick and degrade the operation (frozen hybrid policy, migration 0071). Unticked bindings keep their previous per-binding row. | OUT OF SCOPE (frozen contract) |
| 11 | GUI / read model | No GUI change. The withheld-symbol reason is exposed through the existing per-symbol target state. | n/a |

## Changes

1. **Arbitration required.** `gather_and_resolve` withholds any symbol proposed
   by more than one strategy when the effective conflict mode is not
   `paper_enforced` (`UnarbitratedRefusal`, reported on the outcome; the loop
   records `unarbitrated_same_symbol_competition` per symbol). The explicit
   multi-strategy start is refused with
   `runtime.start_refused.explicit_multi_strategy_arbitration_not_enforced`
   when any symbol carries more than one authorized strategy and the effective
   mode is not `paper_enforced`. Operator-visible consequence: a v3 artifact
   with same-symbol strategies now needs
   `MQK_STRATEGY_CONFLICT_POLICY_MODE=paper_enforced`.
2. **Exactly-once evaluation of a claimed bar.**
   `deposit_and_confirm_binding_evaluation` checks the evaluation row before
   every (re)deposit and drains the binding's deposit on every exit.
3. **Proofs** (`state/m2_concurrent_runtime_tests.rs`, one DB test in
   `mqk-db`): fan-out of one bar to every authorized binding in authority order
   with a result independent of binding order; single consumption; failure,
   staleness and insertion-order isolation; the guard in every mode.

## Boundaries and limits

- No new economic policy. When two strategies compete and arbitration is not
  enforced, the symbol is withheld; when enforced, Bundle 6 decides (it may
  refuse both). Per-strategy capital carve-outs, cross-strategy netting and
  leverage remain unspecified and are not implemented.
- Evaluation is sequential by design (`docs/design/native_multi_symbol_dispatch.md`);
  no parallel worker pool was introduced. Resource bounds are the existing
  binding ceilings.
- Not proven: any Paper or Live activity, broker round trip, or real-provider
  data. Synthetic bars and hermetic states only.
- Not run: `c3_01` (spawns a Python research subprocess) and the full
  workspace; GitHub CI is disabled.

## Proposed ledger text (for later serial integration)

> C11: same-symbol multi-strategy dispatch now fails closed unless Bundle 6 is
> `paper_enforced` (start refusal + per-tick withholding); a claimed host-pool
> bar is evaluated exactly once. Status stays `IMPLEMENTED` pending independent
> review; no `OPERATIONALLY VERIFIED` claim. Open: per-binding timeframe
> labelling in Bundle 5 if mixed timeframes per symbol are ever authorized.
