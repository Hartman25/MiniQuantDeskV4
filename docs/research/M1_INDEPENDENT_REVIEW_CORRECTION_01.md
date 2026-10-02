# M1 independent-review correction 01

Mission `V4-M1-NATIVE-RESEARCH-INDEPENDENT-REVIEW-CORRECTION-01`. Independent review of `7dd31e5d..9b5ede61` returned PARTIAL; this is the one consolidated correction. No hypothesis was run, no threshold or benchmark changed, no holdout consumed, nothing pushed.

## Evidence status of everything produced by the native bridge v1

**HISTORICAL — SUPERSEDED_PROTOCOL — NOT_PROMOTION_AUTHORITY.** This covers the Research economics of the trend_sma50 campaigns 01–03, the dual-SMA campaign, the pullback campaign and Batch 01. The numbers in their records are preserved exactly as produced and are not rewritten. They are an observed result under a bridge whose economics did not match the executable strategy (below), so none of them is promotion evidence, and no old candidate becomes authoritative because code changed later. They were all rejected anyway. A rerun under the corrected protocol needs new trial identities and separate authorization.

## IR-1 — one native quantity contract

- **Authoritative executable contract:** a native `TargetPosition.qty` is an absolute portfolio target; production derives `delta = target - current` (`mqk-daemon/src/decision.rs`, `bar_result_to_decisions`), the opportunity allocator sizes against that same target (`runtime_opportunity_allocation.rs`), the non-scalper engines are fixed one-share signals with no configurable sizing (`engines/mod.rs`), the scanner/Backtest default is `StrategySizingConfig::default_sizing()` = 1 share, and `weight_to_share.py` itself documents that native Rust strategies decide an absolute price-independent quantity with no seam that converts a weight into one. No accepted contract says Paper transforms a native strategy into a predeclared notional, so the decision is not a policy hard stop: the native +1 share IS the executable economic authority.
- **Old mismatch:** the v1 bridge reduced the target to `ml_score` 0/1 and the generic weight-to-share path sized it to the predeclared 100,000 / 50,000 translation (hundreds of shares); fidelity compared only long-vs-flat, so +1 executable vs +hundreds Research scored 1.0. Batch 01 also declared Backtest cash of USD 1,000 against Research equity of USD 100,000, while the scanner used the conservative default of 100,000.
- **Correction:** `native_exact_target_qty_v1` direction policy (sizing `exact_native_target_qty_v1`) carries the exact whole-share `target_qty` through the discrete simulation; `ml_score` is ignored under it; an unfundable target fails closed instead of being resized; fidelity compares exact quantities per evaluated bar (desired native target vs simulated position), so 1/1 passes while 1/250 and 1/2 fail; the emitter's initial cash must equal Research `equity_usd` (one capital basis); signal stream protocol and trial `signal_source.kind` are v2.
- **Identity/versioning:** the exact-target policy, source kind v2, target semantics, required history and equity are identity-bearing, so no v1 trial id can equal a v2 one. Legacy `long_only_v1` identities are byte-for-byte unchanged (absence-not-None). The Rust verifier (`research_registry.rs`) accepts only v2 with the exact-target economics and rejects v1 as superseded (golden identity shared with the Python builder).
- **Proof:** see the findings table; the integrated test binds emitter, Research, Backtest evidence and the registered identity to the same +1 share on the same capital.
- **Not claimed:** nothing here makes a Batch 01 trial promotion-authoritative under the corrected protocol (NO).

## IR-2 — stateful restart

- **Red counterexample:** an entry far older than the Paper window followed by a shallow linear decline keeps the continuous instance LONG while a fresh instance replaying a 256-bar window derives FLAT (`finite_window_restart_cannot_recover_a_long_that_outlives_the_window`, passes today because it documents the defect).
- **Correction (outcome C):** no durable strategy-state seam exists and adding one would expand a frozen contract, so `StrategyMeta.restart_recovery = NotRecoverable` for `pullback_mean_reversion_20_2` and `PluginRegistry::instantiate_verified` — the seam used by Paper bootstrap, promotion fingerprint resolution, dynamic selection and the host pool — refuses it. Backtest/Research (plain `instantiate`) and all historical evidence are unchanged; the engine and its fingerprint are untouched. Proven RED then GREEN in the registry, runtime bootstrap (Failed, not Active) and the daemon fingerprint resolver.

## IR-3 — history provenance

v1 recorded `config.bar_history_len` (50) for a strategy that received 253 bars. v2 records `configured_bar_history_len`, `required_history_bars`, `effective_bar_history_len` (from the single `effective_history_len` rule the engine uses) and `observed_max_window_len` (measured from what the strategy was handed). Tested for 253/252/204/200 and for a requirement below the configured default; the Python loader rejects any inconsistent set.

## IR-4 — chronology

Old: `register` ran the real emitter (a BacktestEngine over market data), then registered. New: fingerprint and history requirement come from `mqk backtest native-fingerprint` with no data; all hypotheses and trials are registered with zero attempts verified; the bridge refuses an unregistered trial; the emitter runs inside the attempt, so a crash is a failed attempt and a retry is the next attempt of the same trial. Guards: behavioral runner tests, an AST check, and three mutations.

## IR-5 — canonical evidence

The review addendum package carries the Batch 01 registry export, judge JSON and hash, the 15 economic JSONs and daily-return CSVs, scanner reviews, Backtest manifests and an artifact map, all copied from the preserved local run (nothing recomputed).

## Governance

OPEN GOVERNANCE QUESTION, unchanged: review alpha compares account return with a fully invested buy-and-hold price return and is not exposure-matched. Not resolved by any accepted contract; no gate or old review state changed.

## Stale text

The "five engine implementations" wording in `engines/mod.rs` no longer carries a count.

## Defect census and second sweep (`7dd31e5d..HEAD`, correction-relevant concerns only)

| Check | Disposition | Evidence |
|---|---|---|
| Exact native quantity preserved | FIXED+PROVEN | Exact-target economics tests (1 and 3 shares held exactly); real-engine integrated proof; fidelity 1/1 passes, 1/250 and 1/2 fail. |
| Research cannot resize +1 into a larger position | FIXED+PROVEN | Negative control: the old policy at the campaign sizing holds >400 shares, the exact policy holds 1; an unfundable target fails the attempt instead of being scaled; mutation `qty*250` killed. |
| Scanner / Backtest / Research economics agree | FIXED+PROVEN | Scanner base is `conservative_defaults()` (100,000 USD, 1-share default sizing); emitter cash must equal Research `equity_usd` (loader check + runner guard); integrated proof shows the same +1 share and starting equity in Research and Backtest. |
| Corrected identity cannot consume v1 evidence | FIXED+PROVEN | Different source kind, policy and equity in the identity; Python loader rejects v1 streams; Rust verifier rejects v1 as superseded and v2 without exact-target economics (golden shared by both languages); daemon gate rejects v1 evidence. |
| Pullback restart truthful or refused | FIXED+PROVEN | Red counterexample; `instantiate_verified` refuses `NotRecoverable` (registry, runtime bootstrap, daemon fingerprint resolver tests). |
| History metadata truthful | FIXED+PROVEN | Engine/emitter share `effective_history_len`; observed window measured; tests for 253/252/204/200 and below-default; mutation killed. |
| Trials precede evaluation; attempts own emissions | FIXED+PROVEN | Runner behavioral and AST guards plus three mutations; bridge refuses an unregistered trial before the emitter runs; emitter crash = failed attempt, retry = next attempt of the same trial. |
| Stale v1 executable paths | FIXED+PROVEN | `run_campaign.py` fails closed with a clear message (guard test); `run_batch.py` refuses the binary-weight predeclaration and a capital-basis mismatch; v1 protocol id exists only as a rejected constant. |
| Failed/historical evidence durable | ALREADY_CORRECT+PROVEN | The Batch 01 registry and run tree are untouched (registry mtime/hash unchanged since the run); nothing was rerun. |
| Holdout untouched | ALREADY_CORRECT+PROVEN | No economic trial was executed in this correction. |
| No threshold change / no alpha-gate rescue | ALREADY_CORRECT+PROVEN | No policy key, review policy or PREDECLARED file appears in the correction diff; old review states untouched. |
| Promotion bypass; Paper activation; Live; secrets | ALREADY_CORRECT+PROVEN | The verifier only became stricter; `MQK_STRATEGY_IDS` and the promotion-policy keys are ABSENT; no `.env`/Live/secret pattern in the diff; no Paper or broker operation. |
| Duplicate authority for the quantity rule | ALREADY_CORRECT+PROVEN | The absolute-target rule lives in `bar_result_to_decisions`; Research only carries it; the window rule lives only in `effective_history_len`. |
| Tests that only compare direction | FIXED+PROVEN | `native_execution_fidelity` (direction-only) replaced; the previous bridge test suite was rewritten around exact quantities and chronology. |
| Commit message of `2d52c2fd` (stray `@` lines) | NOTED | Cosmetic, from the previous controller; amend is forbidden. |

Remaining ordinary deterministic in-scope defects: NONE.

## Batch 01 status after correction

Historical result preserved exactly (BATCH_REJECTED, best DSR 0.214, PBO 0.238, 0/15 `paper_candidate`). Promotion-authoritative under the corrected protocol: NO. Nothing was rerun; a rerun needs a new predeclaration, new trial identities and separate authorization.
