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
