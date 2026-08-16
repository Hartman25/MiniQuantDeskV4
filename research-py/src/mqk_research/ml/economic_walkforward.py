from __future__ import annotations

import json
from dataclasses import dataclass, field, replace
from pathlib import Path
from typing import Any, Dict, List, Optional, Tuple

import numpy as np
import pandas as pd

from mqk_research.data.bars_provenance import (
    check_corporate_action_integrity,
    require_bars_match_manifest,
    require_bars_pricing_provenance,
)
from mqk_research.ml import economics
from mqk_research.ml.execution_pricing import ExecutionPricingSpec, conservative_fill_price
from mqk_research.ml.util_hash import file_record, sha256_json
from mqk_research.ml.weight_to_share import (
    WeightToShareSpec,
    translate_symbol_weight_series_to_share_events,
    weight_to_share_protocol_identity,
)

# RESEARCH-ECONOMIC-WALKFORWARD-01 (+ REPAIR-01)
#
# Extends the purged, registered walk-forward research system
# (mqk_research.ml.eval_walkforward) with OUT-OF-SAMPLE ECONOMIC evaluation:
# a deterministic, causal, cost-aware return series built ONLY from the
# persisted test-fold OOS prediction stream (walk_forward_oos_predictions.csv)
# and real market bars — never from the classification label `fwd_ret`.
#
# CRITICAL: `fwd_ret` in targets.csv begins at the signal bar's own close and
# is NOT executable P&L under the future-execution contract established by
# BKT-FUTURE-EXECUTION-01. This module never reads targets.csv or fwd_ret.
#
# Protocol identifier: PROTOCOL_ID (below). Distinct from, and materially
# different in economic behavior from, the classification-only
# `walk_forward_eval_v2` protocol.
#
# REPAIR-01 causal model (replaces a prior global-row `shift(2)` shortcut
# that (a) attributed execution cost to the same row as the earned return
# instead of the execution row, and (b) shifted by a fixed ROW COUNT rather
# than each symbol's own next bar, so a sibling symbol's missing/extra bar
# could silently move another symbol's execution timestamp). Per symbol:
#
#     desired weight    — computed at a decision frame (see
#                          _build_pending_events), long-only /
#                          equal-weight-active / threshold, exactly as before.
#     pending change     — a desired weight not yet executed for THIS symbol,
#                          keyed by the decision frame's timestamp
#                          ("signal_ts").
#     executed weight    — the position actually held, updated only at THIS
#                          symbol's own first bar with timestamp > signal_ts
#                          (never inferred from another symbol's bar, never
#                          from a fixed row offset).
#
# At each of a symbol's own bars: (1) that row's gross return is earned from
# the weight already executed BEFORE this row (i.e. established at an
# earlier bar) — so a signal can never earn a return over the interval
# ending at its own execution bar; (2) only THEN is any pending change
# executed, charging turnover/cost at THIS bar.
#
# REPAIR-02 (capacity_policy=CAPACITY_POLICY_ID) replaces REPAIR-01's fully
# independent per-symbol execution with a joint TIMESTAMP-FRAME allocator
# (see _simulate_fold_execution) that shares a single portfolio-level
# max_gross_exposure budget across every symbol. REPAIR-01 let independently
# lagged symbols' executions transiently push `sum(abs(executed_weight))`
# above max_gross_exposure during an ordinary asynchronous rebalance (e.g. a
# newly-active symbol's own bar arriving before the symbol it is replacing
# has had a bar to exit on) and failed closed on that — which is correct for
# a genuine implementation bug, but wrong for expected asynchronous
# behavior. REPAIR-02 instead, at each distinct timestamp T present in the
# fold's bars: (a) realizes every present symbol's return from its
# already-executed weight (unchanged from REPAIR-01); (b) executes every
# GROSS-REDUCING (or no-op) pending change unconditionally; (c) groups
# GROSS-INCREASING pending changes into cohorts by their shared decision
# timestamp and executes each cohort, oldest first, ONLY IF the entire
# cohort fits the gross headroom remaining after (b) and any earlier cohort
# this same frame — deferring the WHOLE cohort (never a partial fill, never
# an alphabetical or CSV-row-order subset) otherwise. A deferred pending
# change is retried at that symbol's own next bar; if a strictly newer
# decision for that symbol becomes eligible before it ever executes, the
# newer decision supersedes it (the stale value is never separately
# executed) — see _simulate_fold_execution's docstring for the full
# chronology contract. Held positions are never rescaled to make room; only
# deferral is used. After every frame, `sum(abs(executed_weight))` is
# asserted internally to never exceed max_gross_exposure — under this
# design an exception there indicates a genuine implementation invariant
# failure, not expected asynchronous behavior.
#
# REPAIR-03 fixes a defect in REPAIR-02's cohort-membership determination:
# _simulate_fold_execution previously formed a gross-increasing cohort only
# from symbols with an ACTUAL bar at the current frame (`present`), then
# grouped those present candidates by signal_ts. This could split a same-
# decision atomic cohort whenever its members' execution bars arrived
# asynchronously — e.g. two symbols signalled together at T0 where only one
# of them has a bar at T1 could let that one symbol execute alone, violating
# the atomic-cohort policy. REPAIR-03 instead determines full cohort
# membership from every symbol's causally-effective unresolved pending
# increase as of T — including symbols absent from the current frame,
# inspected read-only (never mutating their pending pointer, never
# fabricating an execution for them) — and defers the ENTIRE cohort if any
# member lacks an actual bar at T. Causal supersession is preserved: an
# absent member's stale pending increase, once superseded by a strictly
# newer decision with signal_ts < T, no longer counts as that member's
# effective pending increase and so can no longer hold an otherwise-eligible
# cohort hostage.

PROTOCOL_ID = "economic_walk_forward_v1"
CAPACITY_POLICY_ID = "reduce_first_defer_increase_batch_v1"


# ---------------------------------------------------------------------------
# Specs
# ---------------------------------------------------------------------------


@dataclass(frozen=True)
class SignalPolicySpec:
    """Explicit, versioned signal-to-position policy. v1 is deliberately
    narrow: long-only, threshold entry, equal-weight sizing across active
    names, bounded gross exposure. See module docstring for why shorts are
    out of scope (the current target is a binary bullish-forward-return
    classifier; a short policy would be an uninvented, un-registered
    strategy).

    MULTI-SYMBOL EQUAL-WEIGHT SIZING RULE: the DESIRED portfolio target
    (this policy) is recomputed at each decision frame from every symbol's
    latest known active/flat state (see _build_pending_events) — never
    rescaled retroactively just because a sibling symbol lacked a bar. Each
    symbol's resulting target change then executes only at THAT symbol's
    own next bar — never inferred from a sibling's bar and never a fixed
    row-count offset. A DIRECT CONSEQUENCE of symbols executing a shared
    rebalance at independently lagged times is that a naive independent
    per-symbol executor could transiently push `sum(abs(executed_weight))`
    above `max_gross_exposure` mid-rebalance (e.g. a newly-active symbol's
    own bar arrives before a symbol whose target shrank to make room for it
    has reached its own next bar). REPAIR-02's `capacity_policy` (see
    _simulate_fold_execution) resolves this the ordinary-asynchronous way —
    by DEFERRING the gross-increasing change until real capacity exists —
    rather than by rescaling other symbols' still-held positions (that
    would itself violate causality — see module docstring) or by failing
    closed on an expected asynchronous pattern."""

    entry_threshold: float = 0.5
    long_only: bool = True
    sizing: str = "equal_weight_active"
    max_gross_exposure: float = 1.0
    fold_end_policy: str = "force_flat_last_bar"
    capacity_policy: str = CAPACITY_POLICY_ID
    schema_version: str = "economic_signal_policy_v1"

    def normalized(self) -> "SignalPolicySpec":
        if not self.long_only:
            raise ValueError("economic_walk_forward_v1 supports only long_only=True")
        if self.sizing != "equal_weight_active":
            raise ValueError("economic_walk_forward_v1 supports only sizing='equal_weight_active'")
        if not (0.0 <= float(self.entry_threshold) <= 1.0):
            raise ValueError("entry_threshold must be within [0,1]")
        if float(self.max_gross_exposure) <= 0.0:
            raise ValueError("max_gross_exposure must be > 0")
        if self.fold_end_policy != "force_flat_last_bar":
            raise ValueError("economic_walk_forward_v1 supports only fold_end_policy='force_flat_last_bar'")
        if self.capacity_policy != CAPACITY_POLICY_ID:
            raise ValueError(
                f"economic_walk_forward_v1 supports only capacity_policy={CAPACITY_POLICY_ID!r}"
            )
        return replace(
            self,
            entry_threshold=float(self.entry_threshold),
            max_gross_exposure=float(self.max_gross_exposure),
        )


@dataclass(frozen=True)
class CostModelSpec:
    """Base commission/slippage cost model. No authoritative base commission
    profile exists in repo config (only `execution.base_slippage_bps: 5` in
    config/defaults/base.yaml, which is a Rust-side parity constant, not a
    research-registered default) — so REGISTERED economic evaluation requires
    explicit, non-zero total cost unless `diagnostic_zero_cost=True` is set
    on purpose. This is not the 2x/3x stress gauntlet (a later patch); it is
    the base cost semantics that gauntlet will scale."""

    commission_bps_per_side: float
    slippage_bps_per_side: float
    diagnostic_zero_cost: bool = False
    schema_version: str = "economic_cost_model_v1"

    def normalized(self) -> "CostModelSpec":
        commission = float(self.commission_bps_per_side)
        slippage = float(self.slippage_bps_per_side)
        if commission < 0.0:
            raise ValueError("commission_bps_per_side must be >= 0")
        if slippage < 0.0:
            raise ValueError("slippage_bps_per_side must be >= 0")
        if commission + slippage <= 0.0 and not self.diagnostic_zero_cost:
            raise RuntimeError(
                "Fail-closed: registered economic evaluation requires positive "
                "commission_bps_per_side + slippage_bps_per_side unless "
                "diagnostic_zero_cost=True is explicitly set"
            )
        return replace(self, commission_bps_per_side=commission, slippage_bps_per_side=slippage)

    @property
    def one_way_cost_bps(self) -> float:
        return float(self.commission_bps_per_side) + float(self.slippage_bps_per_side)


@dataclass(frozen=True)
class AnnualizationSpec:
    annualization_days: int = 252
    risk_free_rate_annual: float = 0.0
    schema_version: str = "economic_annualization_v1"

    def normalized(self) -> "AnnualizationSpec":
        if int(self.annualization_days) <= 0:
            raise ValueError("annualization_days must be > 0")
        return replace(
            self,
            annualization_days=int(self.annualization_days),
            risk_free_rate_annual=float(self.risk_free_rate_annual),
        )


@dataclass(frozen=True)
class EconomicWalkForwardSpec:
    signal_policy: SignalPolicySpec
    cost_model: CostModelSpec
    annualization: AnnualizationSpec
    # P7A (RESEARCH-EXECUTION-PRICING-PARITY-01): defaults to the diagnostic/
    # legacy close-only pricing model, so existing callers that don't pass
    # this explicitly reproduce pre-P7A behavior bit-for-bit -- see
    # ExecutionPricingSpec / mqk_research.ml.execution_pricing.
    execution_pricing: ExecutionPricingSpec = field(default_factory=ExecutionPricingSpec)
    # P7B (RESEARCH-WEIGHT-TO-SHARE-PARITY-01): optional weight->share
    # translation evidence. Defaults to None (diagnostic/legacy continuous-
    # weight-only behavior, bit-for-bit unchanged) so existing callers that
    # don't pass this are completely unaffected -- see weight_to_share
    # module docstring.
    weight_to_share: Optional[WeightToShareSpec] = None
    protocol_id: str = PROTOCOL_ID

    def normalized(self) -> "EconomicWalkForwardSpec":
        if self.protocol_id != PROTOCOL_ID:
            raise ValueError(f"unsupported economic protocol_id: {self.protocol_id!r}")
        execution_pricing = self.execution_pricing.normalized()
        cost_model = self.cost_model.normalized()
        if execution_pricing.is_official_parity_model and cost_model.slippage_bps_per_side != 0.0:
            raise RuntimeError(
                "Fail-closed: execution_pricing uses the official rust_conservative_bar_range_v1 "
                "parity model, which already models slippage via directional bar-range pricing -- "
                "cost_model.slippage_bps_per_side must be 0 to avoid double-charging slippage "
                "(commission_bps_per_side is unaffected and remains the separate commission charge)"
            )
        weight_to_share = self.weight_to_share.normalized() if self.weight_to_share is not None else None
        return EconomicWalkForwardSpec(
            signal_policy=self.signal_policy.normalized(),
            cost_model=cost_model,
            annualization=self.annualization.normalized(),
            execution_pricing=execution_pricing,
            weight_to_share=weight_to_share,
            protocol_id=self.protocol_id,
        )


def economic_protocol_identity(spec: EconomicWalkForwardSpec) -> Dict[str, Any]:
    """Canonical, result-independent identity fragment for the economic
    protocol/signal/cost/annualization choices — consumed by
    economic_registry_integration.build_economic_trial_identity."""
    return {
        "protocol_id": spec.protocol_id,
        "signal_policy": {
            "entry_threshold": spec.signal_policy.entry_threshold,
            "long_only": spec.signal_policy.long_only,
            "sizing": spec.signal_policy.sizing,
            "max_gross_exposure": spec.signal_policy.max_gross_exposure,
            "fold_end_policy": spec.signal_policy.fold_end_policy,
            "capacity_policy": spec.signal_policy.capacity_policy,
        },
        "cost_model": {
            "commission_bps_per_side": spec.cost_model.commission_bps_per_side,
            "slippage_bps_per_side": spec.cost_model.slippage_bps_per_side,
            "diagnostic_zero_cost": spec.cost_model.diagnostic_zero_cost,
        },
        # P7A: always identity-bearing -- which pricing model produced a
        # trial's economics is always a real, distinguishing fact, even
        # when it's the diagnostic default (see execution_pricing module
        # docstring). slippage_bps/volatility_mult_bps only economically
        # matter under the official model, but are included unconditionally
        # since they're cheap, small, and their presence alone never
        # collides two otherwise-identical diagnostic trials (both report
        # the same defaults).
        "execution_pricing": {
            "pricing_model_id": spec.execution_pricing.pricing_model_id,
            "slippage_bps": spec.execution_pricing.slippage_bps,
            "volatility_mult_bps": spec.execution_pricing.volatility_mult_bps,
        },
        # P7B (RESEARCH-WEIGHT-TO-SHARE-PARITY-01): always identity-bearing,
        # same rationale as execution_pricing above -- see weight_to_share
        # module docstring. `{"weight_to_share_protocol_id": None}` when
        # spec.weight_to_share is the diagnostic/legacy None default.
        "weight_to_share": weight_to_share_protocol_identity(spec.weight_to_share),
        "annualization": {
            "annualization_days": spec.annualization.annualization_days,
            "risk_free_rate_annual": spec.annualization.risk_free_rate_annual,
        },
    }


# ---------------------------------------------------------------------------
# Bars loading + provenance
# ---------------------------------------------------------------------------


def load_bars(bars_csv: Path, *, require_pricing_columns: bool = False) -> pd.DataFrame:
    """`require_pricing_columns=True` (P7A) additionally requires and
    validates `high`/`low` -- callers pass this whenever
    `spec.execution_pricing.is_official_parity_model`, since the official
    rust_conservative_bar_range_v1 model consumes them and must fail closed
    rather than silently fall back to close-only pricing (see
    Research_Backtest_V1_Closeout_Audit.md P7A mission, "no fallback from
    missing high/low to close for official evaluation")."""
    bars_csv = Path(bars_csv)
    if not bars_csv.exists():
        raise FileNotFoundError(f"Fail-closed: missing economic bars file: {bars_csv}")
    bars = pd.read_csv(bars_csv)
    required = ["symbol", "end_ts", "close"]
    if require_pricing_columns:
        required = required + ["high", "low"]
    missing = [c for c in required if c not in bars.columns]
    if missing:
        raise RuntimeError(f"Fail-closed: bars csv missing required columns: {missing}")

    bars = bars.copy()
    bars["symbol"] = bars["symbol"].astype(str)
    bars["end_ts"] = pd.to_datetime(bars["end_ts"], utc=True, errors="coerce")
    if bars["end_ts"].isna().any():
        raise RuntimeError("Fail-closed: bars end_ts contains missing/unparsable values")

    close = pd.to_numeric(bars["close"], errors="coerce")
    if close.isna().any():
        raise RuntimeError("Fail-closed: bars close contains missing/non-numeric values")
    if (close <= 0.0).any():
        raise RuntimeError("Fail-closed: bars close must be strictly positive")
    bars["close"] = close.astype(float)

    if require_pricing_columns:
        high = pd.to_numeric(bars["high"], errors="coerce")
        low = pd.to_numeric(bars["low"], errors="coerce")
        if high.isna().any() or not np.isfinite(high.to_numpy(dtype=float)).all():
            raise RuntimeError("Fail-closed: bars high contains missing/non-finite values")
        if low.isna().any() or not np.isfinite(low.to_numpy(dtype=float)).all():
            raise RuntimeError("Fail-closed: bars low contains missing/non-finite values")
        bars["high"] = high.astype(float)
        bars["low"] = low.astype(float)
        if (bars["high"] < bars["low"]).any():
            raise RuntimeError("Fail-closed: bars contain high < low (impossible bar shape)")
        if ((bars["close"] < bars["low"]) | (bars["close"] > bars["high"])).any():
            raise RuntimeError(
                "Fail-closed: bars contain close outside [low, high] (impossible bar shape)"
            )

    dup_mask = bars.duplicated(subset=["symbol", "end_ts"], keep=False)
    if dup_mask.any():
        raise RuntimeError("Fail-closed: duplicate (symbol,end_ts) rows in economic bars csv")

    return bars.sort_values(["symbol", "end_ts"], kind="mergesort").reset_index(drop=True)


def verify_bars_provenance(run_dir: Path, bars_csv: Path) -> Dict[str, Any]:
    """Content-identity record for `bars_csv`. If `run_dir/shadow_label_meta.json`
    exists and records a `bars_csv` provenance hash, the caller-supplied
    `bars_csv` must match it byte-for-byte — fail closed on any mismatch
    rather than silently evaluating on a different bars file than the one
    that produced targets.csv's labels."""
    run_dir = Path(run_dir)
    bars_record = file_record(Path(bars_csv))
    if bars_record["sha256"] is None:
        raise FileNotFoundError(f"Fail-closed: missing economic bars file: {bars_csv}")

    meta_path = run_dir / "shadow_label_meta.json"
    if meta_path.exists():
        meta = json.loads(meta_path.read_text(encoding="utf-8"))
        recorded = (meta.get("inputs") or {}).get("bars_csv")
        if recorded is not None and recorded.get("sha256") is not None:
            if recorded["sha256"] != bars_record["sha256"]:
                raise RuntimeError(
                    "Fail-closed: economic bars_csv content does not match the "
                    "sha256 recorded in shadow_label_meta.json (the bars file "
                    "that produced targets.csv's labels) — refusing to "
                    "economically simulate on a different bars file"
                )
    return bars_record


# ---------------------------------------------------------------------------
# OOS prediction loading
# ---------------------------------------------------------------------------


def load_oos_predictions(oos_predictions_csv: Path) -> pd.DataFrame:
    oos_predictions_csv = Path(oos_predictions_csv)
    if not oos_predictions_csv.exists():
        raise FileNotFoundError(f"Fail-closed: missing OOS predictions file: {oos_predictions_csv}")
    df = pd.read_csv(oos_predictions_csv)
    required = ["fold", "symbol", "decision_ts", "ml_score"]
    missing = [c for c in required if c not in df.columns]
    if missing:
        raise RuntimeError(f"Fail-closed: OOS predictions csv missing required columns: {missing}")

    df = df.copy()
    df["fold"] = df["fold"].astype(int)
    df["symbol"] = df["symbol"].astype(str)
    df["decision_ts"] = pd.to_datetime(df["decision_ts"], utc=True, errors="coerce")
    if df["decision_ts"].isna().any():
        raise RuntimeError("Fail-closed: OOS predictions decision_ts contains missing/unparsable values")

    score = pd.to_numeric(df["ml_score"], errors="coerce")
    if score.isna().any() or not np.isfinite(score.to_numpy(dtype=float)).all():
        raise RuntimeError("Fail-closed: OOS predictions ml_score contains missing/non-finite values")
    if ((score < 0.0) | (score > 1.0)).any():
        raise RuntimeError("Fail-closed: OOS predictions ml_score outside expected [0,1] range")
    df["ml_score"] = score.astype(float)

    dup_mask = df.duplicated(subset=["fold", "symbol", "decision_ts"], keep=False)
    if dup_mask.any():
        raise RuntimeError("Fail-closed: duplicate (fold,symbol,decision_ts) in OOS predictions csv")

    return df


# ---------------------------------------------------------------------------
# Fold boundaries
# ---------------------------------------------------------------------------


def _parse_used_folds(wf_eval: Dict[str, Any]) -> List[Dict[str, Any]]:
    used = [f for f in wf_eval["folds"] if not f.get("skipped")]
    if not used:
        raise RuntimeError("Fail-closed: no usable walk-forward folds for economic evaluation")
    parsed = [
        {
            "fold": int(f["fold"]),
            "test_start": pd.Timestamp(f["test_start_utc"]),
            "test_end": pd.Timestamp(f["test_end_utc"]),
        }
        for f in used
    ]
    parsed.sort(key=lambda x: x["fold"])
    for i in range(len(parsed)):
        for j in range(i + 1, len(parsed)):
            a, b = parsed[i], parsed[j]
            if a["test_start"] < b["test_end"] and b["test_start"] < a["test_end"]:
                raise RuntimeError(
                    "Fail-closed: overlapping test-fold windows "
                    f"(fold {a['fold']} and fold {b['fold']}) are not supported by "
                    f"{PROTOCOL_ID}'s stitched aggregate — no deterministic ownership "
                    "rule is implemented in this patch (step_months < test_months)"
                )
    return parsed


# ---------------------------------------------------------------------------
# Per-fold economic simulation
# ---------------------------------------------------------------------------


def _build_fold_close_frame(
    bars: pd.DataFrame, symbols: List[str], test_start: pd.Timestamp, test_end: pd.Timestamp
) -> pd.DataFrame:
    subset = bars[
        bars["symbol"].isin(symbols) & (bars["end_ts"] >= test_start) & (bars["end_ts"] < test_end)
    ]
    if subset.empty:
        raise RuntimeError(
            "Fail-closed: no economic bars available inside the fold's test window "
            f"[{test_start.isoformat()}, {test_end.isoformat()}) for symbols {symbols}"
        )
    pivot = subset.pivot_table(index="end_ts", columns="symbol", values="close", aggfunc="last")
    pivot = pivot.sort_index(kind="mergesort").sort_index(axis=1)
    pivot = pivot.reindex(columns=symbols)
    return pivot


def _build_fold_high_low_frames(
    bars: pd.DataFrame, symbols: List[str], test_start: pd.Timestamp, test_end: pd.Timestamp
) -> Tuple[pd.DataFrame, pd.DataFrame]:
    """P7A companion to _build_fold_close_frame: high/low pivoted the same
    way, over the same fold window/symbol set, so `.at[t, s]` lookups stay
    aligned with `close_frame`'s. Only called when
    spec.execution_pricing.is_official_parity_model -- diagnostic-model
    folds never touch high/low."""
    subset = bars[
        bars["symbol"].isin(symbols) & (bars["end_ts"] >= test_start) & (bars["end_ts"] < test_end)
    ]
    if subset.empty:
        raise RuntimeError(
            "Fail-closed: no economic bars available inside the fold's test window "
            f"[{test_start.isoformat()}, {test_end.isoformat()}) for symbols {symbols}"
        )
    high_pivot = subset.pivot_table(index="end_ts", columns="symbol", values="high", aggfunc="last")
    low_pivot = subset.pivot_table(index="end_ts", columns="symbol", values="low", aggfunc="last")
    high_pivot = high_pivot.sort_index(kind="mergesort").sort_index(axis=1).reindex(columns=symbols)
    low_pivot = low_pivot.sort_index(kind="mergesort").sort_index(axis=1).reindex(columns=symbols)
    return high_pivot, low_pivot


def _build_pending_events(
    oos_fold: pd.DataFrame, symbols: List[str], signal_policy: SignalPolicySpec
) -> Dict[str, List[Tuple[pd.Timestamp, float]]]:
    """Per-symbol, chronologically-ordered list of (signal_ts, desired_weight)
    pending-change events — the DESIRED side of the desired/pending/executed
    state machine (see module docstring). Does not decide WHEN or WHETHER a
    symbol executes a change; that is a per-frame decision made later in
    _simulate_fold_execution, jointly across symbols, against each symbol's
    own bar timestamps and the shared max_gross_exposure budget.

    A decision frame (one distinct decision_ts, aggregating every symbol
    scored at that instant — physical CSV row order within the frame does
    not matter) only produces new pending events for a symbol whose
    long/flat ACTIVE STATE actually changes at that frame, OR for every
    other symbol when someone else's active-state change alters the
    equal-weight-active sizing denominator (a rebalance of everyone
    currently active/target). A symbol not scored at a given frame keeps its
    last known active state — it is never implicitly forced flat merely for
    lacking a decision row at some other symbol's frame."""
    decisions = oos_fold.pivot_table(index="decision_ts", columns="symbol", values="ml_score", aggfunc="last")
    decisions = decisions.reindex(columns=symbols).sort_index(kind="mergesort")

    active_state: Dict[str, bool] = {s: False for s in symbols}
    last_issued_weight: Dict[str, float] = {s: 0.0 for s in symbols}
    pending_events: Dict[str, List[Tuple[pd.Timestamp, float]]] = {s: [] for s in symbols}

    for ts, row in decisions.iterrows():
        changed = False
        for s in symbols:
            score = row[s]
            if pd.notna(score):
                new_active = bool(float(score) >= signal_policy.entry_threshold)
                if new_active != active_state[s]:
                    active_state[s] = new_active
                    changed = True
        if not changed:
            continue

        active_symbols = [s for s in symbols if active_state[s]]
        weight_each = (
            float(signal_policy.max_gross_exposure) / float(len(active_symbols)) if active_symbols else 0.0
        )
        for s in symbols:
            new_weight = weight_each if active_state[s] else 0.0
            if abs(new_weight - last_issued_weight[s]) > 1e-12:
                pending_events[s].append((pd.Timestamp(ts), new_weight))
                last_issued_weight[s] = new_weight

    return pending_events


_GROSS_TOL = 1e-9


def _row_execution_pricing_components(
    delta_weight: float,
    close: float,
    high: Optional[float],
    low: Optional[float],
    execution_pricing: Optional[ExecutionPricingSpec],
) -> Tuple[float, float]:
    """P7A + REPAIR-01: the two auditable pricing components of executing
    `delta_weight` worth of gross exposure at THIS bar's conservative fill
    price instead of its close. BUY (delta_weight > 0) uses HIGH, SELL
    (delta_weight < 0, including ordinary decreases) uses LOW -- mirrors
    Rust's conservative_fill_price side convention exactly.

    Returns (adverse_price_cost, fill_to_close_notional_ratio):
      - adverse_price_cost: `abs(delta_weight) * abs(fill - close) / close`
        (unchanged from P7A) -- the return-series drag from filling away
        from close.
      - fill_to_close_notional_ratio: `fill / close` -- REPAIR-01. Mirrors
        Rust's `commission.compute_fee(qty, fill_price)`, which charges
        commission against ACTUAL FILL notional, not close notional. At the
        continuous-weight level used here (no discrete share quantities --
        those remain P7B), this ratio is how a caller rescales
        close-priced turnover into fill-priced notional for the commission
        calculation: `commission_notional = turnover * ratio`.

    Both are always (0.0, 1.0) under the diagnostic pricing model
    (execution_pricing is None or not the official model) or for a no-op
    delta_weight -- existing callers/tests that never pass execution_pricing
    are numerically unaffected. Only applies to ORDINARY strategy-driven
    executions (this function's only two call sites, both inside
    _simulate_fold_execution) -- the fold-end force_flat_last_bar exception
    in _simulate_fold is a separate code path that never calls this,
    matching Rust's flatten_all exception (close/mark pricing for both the
    adverse-price cost AND the commission basis -- no conservative HIGH/LOW
    fill anywhere in a forced exit)."""
    if execution_pricing is None or not execution_pricing.is_official_parity_model:
        return 0.0, 1.0
    if delta_weight == 0.0:
        return 0.0, 1.0
    if high is None or low is None or pd.isna(high) or pd.isna(low):
        raise RuntimeError(
            "Fail-closed: official execution-pricing parity model requires a high/low value for "
            "every bar with an executing turnover event"
        )
    side = "buy" if delta_weight > 0 else "sell"
    fill = conservative_fill_price(
        high=float(high),
        low=float(low),
        close=float(close),
        side=side,
        slippage_bps=execution_pricing.slippage_bps,
        volatility_mult_bps=execution_pricing.volatility_mult_bps,
    )
    close_f = float(close)
    adverse_price_cost = abs(delta_weight) * abs(fill - close_f) / close_f
    fill_to_close_ratio = fill / close_f
    return adverse_price_cost, fill_to_close_ratio


def _simulate_fold_execution(
    close_frame: pd.DataFrame,
    pending_events: Dict[str, List[Tuple[pd.Timestamp, float]]],
    max_gross_exposure: float,
    *,
    high_frame: Optional[pd.DataFrame] = None,
    low_frame: Optional[pd.DataFrame] = None,
    execution_pricing: Optional[ExecutionPricingSpec] = None,
) -> Tuple[Dict[str, List[Dict[str, Any]]], Dict[str, float]]:
    """Joint, timestamp-frame causal execution/allocation state machine
    (capacity_policy=CAPACITY_POLICY_ID) sharing one portfolio-level gross-
    exposure budget across every symbol in this fold.

    At each distinct timestamp T present in `close_frame` (a "frame"), for
    every symbol WITH AN ACTUAL BAR AT T (never fabricated for an absent
    symbol — see module docstring):

      1. Earn this row's gross return from the weight already EXECUTED
         before T (i.e. established at an earlier bar of THIS symbol) —
         identical to the REPAIR-01 per-symbol contract.
      2. Resolve this symbol's next unresolved pending change, if any, into
         a CANDIDATE target: walk forward through pending events whose
         signal_ts < T, but stop at (do not consume) the LAST such event.
         Any strictly older eligible events encountered along the way are
         superseded without ever executing — this is what makes a stale
         DEFERRED target incapable of executing once the strategy changes
         its mind (module docstring PENDING SUPERSESSION), while an event
         that is ALREADY the sole eligible candidate at some earlier frame
         still gets a chance to execute at that frame before a same-T-or-
         later decision can supersede it (a decision generated AT exactly T
         has signal_ts == T, which is never < T, so it can never supersede
         or execute at its own frame).
      3. GROSS-REDUCING (or no-op) candidates execute unconditionally, in
         full, before any increase this frame: turnover/cost charged at T,
         `executed` updated, the event consumed.
      4. GROSS-INCREASING candidates are grouped into cohorts by their
         candidate's own signal_ts (same decision instant across symbols =
         same cohort; cohorts are atomic). REPAIR-03: cohort MEMBERSHIP is
         determined from every symbol's causally-effective unresolved
         pending event as of T — including a symbol ABSENT from this frame
         — never from `present` symbols alone (see module docstring
         REPAIR-03 addendum). An absent symbol's effective candidate is
         resolved the identical read-only way (walking past superseded
         events, same signal_ts < T rule) but its `pending_idx` is never
         persisted from that inspection — only an actual execution ever
         consumes a pending event. If ANY member of a cohort lacks an
         actual bar at T, the ENTIRE cohort is deferred, exactly as if it
         hadn't fit headroom: a sibling's missing execution bar can never
         let the rest of a same-instant increase cohort execute ahead of
         it. Cohorts (now known to have every member present) are attempted
         oldest signal_ts first against the gross headroom remaining after
         this frame's reductions and any earlier cohort this same frame
         that fit. A cohort that does not fit in full is deferred IN FULL —
         no partial fill, no alphabetical or CSV-row-order subset — and its
         members' pending pointers are left untouched so each is retried at
         that symbol's own next bar. A held position that is not itself
         part of an executing change is NEVER rescaled to create headroom.

    After every frame, `sum(abs(executed[s]) for s in ALL symbols)` is
    asserted <= max_gross_exposure + tolerance; a violation here indicates
    an implementation invariant failure, not expected asynchronous
    behavior (the design guarantees ordinary asynchronous rebalances always
    resolve by deferral).

    Returns (rows_by_symbol, final_weight_by_symbol). `rows_by_symbol[s]`
    has one dict per s's own bar, same shape as the pre-REPAIR-02 per-symbol
    rows: timestamp/gross_contrib/turnover/interval_exposure/executed_weight.
    """
    symbols = list(close_frame.columns)
    bar_index = close_frame.index

    executed_weight: Dict[str, float] = {s: 0.0 for s in symbols}
    pending_idx: Dict[str, int] = {s: 0 for s in symbols}
    prev_close: Dict[str, Optional[float]] = {s: None for s in symbols}
    rows_by_symbol: Dict[str, List[Dict[str, Any]]] = {s: [] for s in symbols}

    for t in bar_index:
        present = [s for s in symbols if pd.notna(close_frame.at[t, s])]
        if not present:
            continue

        gross_contrib: Dict[str, float] = {}
        interval_exposure: Dict[str, float] = {}
        candidate: Dict[str, Optional[float]] = {}
        candidate_ts: Dict[str, Optional[pd.Timestamp]] = {}

        for s in present:
            c = float(close_frame.at[t, s])
            gross_contrib[s] = (
                0.0 if prev_close[s] is None else executed_weight[s] * (c / prev_close[s] - 1.0)
            )
            interval_exposure[s] = abs(executed_weight[s])

            events = pending_events.get(s, [])
            idx = pending_idx[s]
            while idx + 1 < len(events) and events[idx + 1][0] < t:
                idx += 1
            if idx < len(events) and events[idx][0] < t:
                candidate[s] = events[idx][1]
                candidate_ts[s] = events[idx][0]
            else:
                candidate[s] = None
                candidate_ts[s] = None
            pending_idx[s] = idx
            prev_close[s] = c

        turnover: Dict[str, float] = {s: 0.0 for s in present}
        execution_price_cost: Dict[str, float] = {s: 0.0 for s in present}
        # REPAIR-01: commission_notional is turnover rescaled by the actual
        # fill/close notional ratio (1.0 -- i.e. unrescaled -- under the
        # diagnostic model or when no execution occurs this row); the caller
        # (_simulate_fold) multiplies this by commission_bps_per_side/10000
        # instead of charging commission against close-priced turnover.
        commission_notional: Dict[str, float] = {s: 0.0 for s in present}

        # D: gross-reducing (and no-op) candidates execute unconditionally.
        for s in present:
            if candidate[s] is None:
                continue
            if candidate[s] <= executed_weight[s] + _GROSS_TOL:
                old_weight = executed_weight[s]
                delta = candidate[s] - old_weight
                turnover[s] = abs(delta)
                price_cost, fill_ratio = _row_execution_pricing_components(
                    delta,
                    close_frame.at[t, s],
                    high_frame.at[t, s] if high_frame is not None else None,
                    low_frame.at[t, s] if low_frame is not None else None,
                    execution_pricing,
                )
                execution_price_cost[s] = price_cost
                commission_notional[s] = turnover[s] * fill_ratio
                executed_weight[s] = candidate[s]
                pending_idx[s] += 1

        # E: actual gross after this frame's reductions.
        gross = sum(abs(w) for w in executed_weight.values())

        # F0: causally-effective candidate for ABSENT symbols, resolved the
        # identical read-only way as the `present` branch above (same
        # signal_ts < T rule, same walk-past-superseded-events semantics)
        # so a missing execution bar can correctly block (or, once
        # superseded, correctly stop blocking) a shared increase cohort —
        # WITHOUT ever fabricating an execution or mutating that symbol's
        # pending_idx from mere inspection (only an actual execution ever
        # consumes a pending event).
        absent = [s for s in symbols if s not in present]
        for s in absent:
            events = pending_events.get(s, [])
            idx = pending_idx[s]
            while idx + 1 < len(events) and events[idx + 1][0] < t:
                idx += 1
            if idx < len(events) and events[idx][0] < t:
                candidate[s] = events[idx][1]
                candidate_ts[s] = events[idx][0]
            else:
                candidate[s] = None
                candidate_ts[s] = None
            # NOTE: pending_idx[s] intentionally NOT persisted here.

        # F: gross-increasing candidates across ALL symbols (present or
        # absent), grouped into atomic same-signal_ts cohorts, attempted
        # oldest signal_ts first against remaining headroom. A cohort with
        # any absent member is deferred in full — a sibling's missing
        # execution bar can never let the rest of a same-instant cohort
        # execute ahead of it.
        increase_symbols = [
            s for s in symbols
            if candidate[s] is not None and candidate[s] > executed_weight[s] + _GROSS_TOL
        ]
        cohorts: Dict[pd.Timestamp, List[str]] = {}
        for s in increase_symbols:
            cohorts.setdefault(candidate_ts[s], []).append(s)

        for ts_key in sorted(cohorts.keys()):
            members = cohorts[ts_key]
            if any(s not in present for s in members):
                continue  # an absent member blocks the whole atomic cohort
            delta = sum(abs(candidate[s]) - abs(executed_weight[s]) for s in members)
            if gross + delta <= max_gross_exposure + _GROSS_TOL:
                for s in members:
                    old_weight = executed_weight[s]
                    member_delta = candidate[s] - old_weight
                    turnover[s] = abs(member_delta)
                    price_cost, fill_ratio = _row_execution_pricing_components(
                        member_delta,
                        close_frame.at[t, s],
                        high_frame.at[t, s] if high_frame is not None else None,
                        low_frame.at[t, s] if low_frame is not None else None,
                        execution_pricing,
                    )
                    execution_price_cost[s] = price_cost
                    commission_notional[s] = turnover[s] * fill_ratio
                    executed_weight[s] = candidate[s]
                    pending_idx[s] += 1
                gross += delta
            # else: defer the entire cohort — pending_idx left untouched, so
            # every member is retried at its own next bar.

        if gross > max_gross_exposure + _GROSS_TOL:
            raise RuntimeError(
                "Fail-closed: reduce_first_defer_increase_batch_v1 allocator "
                "invariant violated — gross exposure exceeded max_gross_exposure "
                "after a frame's reductions/cohort execution"
            )

        for s in present:
            rows_by_symbol[s].append({
                "timestamp": t,
                "gross_contrib": gross_contrib[s],
                "turnover": turnover[s],
                "interval_exposure": interval_exposure[s],
                "executed_weight": executed_weight[s],
                "execution_price_cost": execution_price_cost[s],
                "commission_notional": commission_notional[s],
            })

    return rows_by_symbol, dict(executed_weight)


def _daily_aggregate(rows: pd.DataFrame) -> pd.DataFrame:
    columns = [
        "date", "gross_daily_return", "net_daily_return", "turnover",
        "transaction_cost", "interval_exposure", "gross_exposure", "active_positions", "row_count",
    ]
    if rows.empty:
        return pd.DataFrame(columns=columns)
    ts = pd.to_datetime(rows["timestamp"], utc=True)
    dates = ts.dt.strftime("%Y-%m-%d")
    out_rows = []
    for date_str, group in rows.assign(_date=dates).groupby("_date", sort=True):
        out_rows.append({
            "date": date_str,
            "gross_daily_return": float((1.0 + group["gross_return"]).prod() - 1.0),
            "net_daily_return": float((1.0 + group["net_return"]).prod() - 1.0),
            "turnover": float(group["turnover"].sum()),
            "transaction_cost": float(group["transaction_cost"].sum()),
            "interval_exposure": float(group["interval_exposure"].mean()),
            "gross_exposure": float(group["gross_exposure"].mean()),
            "active_positions": int(group["active_positions"].max()),
            "row_count": int(len(group)),
        })
    return pd.DataFrame(out_rows, columns=columns)


def _summarize_series(
    daily: pd.DataFrame, annualization: AnnualizationSpec
) -> Dict[str, Any]:
    trading_days = int(len(daily))
    if trading_days == 0:
        return {
            "gross_total_return": 0.0, "net_total_return": 0.0,
            "annualized_net_return": 0.0, "annualized_net_volatility": 0.0,
            "net_sharpe": None, "max_drawdown": 0.0,
            "active_days": 0, "positive_day_rate": 0.0, "negative_day_rate": 0.0,
            "cost_drag": 0.0, "active_interval_expectancy": None,
            "trading_days": 0,
        }
    gross_total = economics.compute_compounded_total_return(daily["gross_daily_return"])
    net_total = economics.compute_compounded_total_return(daily["net_daily_return"])
    active_mask = daily["active_positions"] > 0
    return {
        "gross_total_return": gross_total,
        "net_total_return": net_total,
        "annualized_net_return": economics.compute_annualized_return(
            net_total, trading_days, annualization.annualization_days
        ),
        "annualized_net_volatility": economics.compute_annualized_volatility(
            daily["net_daily_return"], annualization.annualization_days
        ),
        "net_sharpe": economics.compute_sharpe(
            daily["net_daily_return"],
            risk_free_rate_annual=annualization.risk_free_rate_annual,
            annualization_days=annualization.annualization_days,
        ),
        "max_drawdown": economics.compute_max_drawdown(daily["net_daily_return"]),
        "active_days": int(active_mask.sum()),
        "positive_day_rate": float((daily["net_daily_return"] > 0).mean()),
        "negative_day_rate": float((daily["net_daily_return"] < 0).mean()),
        "cost_drag": economics.compute_cost_drag(gross_total, net_total),
        "active_interval_expectancy": (
            float(daily.loc[active_mask, "net_daily_return"].mean()) if active_mask.any() else None
        ),
        "trading_days": trading_days,
    }


def _simulate_fold(
    fold_no: int,
    boundaries: Dict[str, Any],
    close_frame: pd.DataFrame,
    pending_events: Dict[str, List[Tuple[pd.Timestamp, float]]],
    spec: EconomicWalkForwardSpec,
    *,
    high_frame: Optional[pd.DataFrame] = None,
    low_frame: Optional[pd.DataFrame] = None,
) -> Tuple[pd.DataFrame, Dict[str, Any]]:
    """Run the joint, gross-cap-aware causal state machine
    (_simulate_fold_execution) for every symbol in this fold and merge the
    results onto the fold's shared timestamp timeline. `close_frame` carries
    NaN wherever a symbol lacks a bar at a given timestamp — it is NEVER
    forward-filled here, because a forward-filled close would fabricate a
    same-price "bar" for the missing
    symbol and let a sibling symbol's timestamp silently stand in for this
    symbol's own next bar (BLOCKER 2).

    P7A: `high_frame`/`low_frame` (only passed when
    spec.execution_pricing.is_official_parity_model) drive an ADDITIONAL
    adverse-price cost component on top of transaction_cost, and (REPAIR-01)
    rescale the commission basis from close-priced to fill-priced turnover
    -- see _row_execution_pricing_components / _simulate_fold_execution. The
    fold-end force_flat_last_bar exit below deliberately never adds the
    adverse-price component and always uses an unrescaled (ratio 1.0)
    commission basis (see its own comment) -- it is the forced-flatten
    exception, mirroring Rust's flatten_all."""
    bar_index = close_frame.index
    symbols = list(close_frame.columns)
    global_last_ts = bar_index[-1]
    one_way_cost_bps = spec.cost_model.one_way_cost_bps

    gross_by_ts: Dict[pd.Timestamp, float] = {t: 0.0 for t in bar_index}
    turnover_by_ts: Dict[pd.Timestamp, float] = {t: 0.0 for t in bar_index}
    interval_exposure_by_ts: Dict[pd.Timestamp, float] = {t: 0.0 for t in bar_index}
    execution_price_cost_by_ts: Dict[pd.Timestamp, float] = {t: 0.0 for t in bar_index}
    # REPAIR-01: turnover rescaled by each execution's own fill/close
    # notional ratio (see _row_execution_pricing_components), summed per
    # timestamp -- the basis commission_bps_per_side is charged against
    # under the official pricing model (RESEARCH-EXECUTION-PRICING-PARITY-
    # 01-REPAIR-01), instead of close-priced turnover.
    commission_notional_by_ts: Dict[pd.Timestamp, float] = {t: 0.0 for t in bar_index}
    executed_weight_series: Dict[str, pd.Series] = {}

    rows_by_symbol, final_weight_by_symbol = _simulate_fold_execution(
        close_frame,
        pending_events,
        spec.signal_policy.max_gross_exposure,
        high_frame=high_frame,
        low_frame=low_frame,
        execution_pricing=spec.execution_pricing,
    )

    # P7B (RESEARCH-WEIGHT-TO-SHARE-PARITY-01): normalized once, outside the
    # loop, so a validation error (e.g. bad equity_usd) fails closed before
    # any per-symbol translation work starts.
    wts_spec = spec.weight_to_share.normalized() if spec.weight_to_share is not None else None
    weight_to_share_events_by_symbol: Dict[str, List[Dict[str, Any]]] = {}

    for s in symbols:
        rows = rows_by_symbol[s]
        final_weight = final_weight_by_symbol[s]

        # force_flat_last_bar (the only supported fold_end_policy — enforced
        # by SignalPolicySpec.normalized): liquidate any still-held position
        # at the FOLD's own last timestamp. If this symbol's own last bar is
        # already that timestamp, fold the exit turnover into that row so it
        # doesn't fabricate a second, priceless row. If this symbol's own
        # last bar is earlier (it had no bar at the fold's final timestamp),
        # append a priceless exit row: turnover/cost only, no gross return —
        # closing a position never earns a return over an interval with no
        # observed price move, and no bar beyond the fold's window is read.
        # P7A: this exit is the forced-flatten EXCEPTION (mirrors Rust's
        # flatten_all) — it never adds execution_price_cost, ordinary/
        # strategy-driven turnover already carries whatever
        # _simulate_fold_execution computed for it.
        if rows and rows[-1]["timestamp"] == global_last_ts:
            if abs(final_weight) > 1e-12:
                exit_turnover = abs(final_weight)
                rows[-1] = dict(rows[-1])
                rows[-1]["turnover"] = rows[-1]["turnover"] + exit_turnover
                rows[-1]["executed_weight"] = 0.0
                # Close/mark-priced exit (ratio 1.0): added to whatever
                # commission_notional this same row's own ordinary execution
                # (if any) already accrued, never blended into a single
                # rescaled ratio -- each component keeps its own true basis.
                rows[-1]["commission_notional"] = rows[-1]["commission_notional"] + exit_turnover
                final_weight = 0.0
        elif abs(final_weight) > 1e-12:
            rows.append({
                "timestamp": global_last_ts,
                "gross_contrib": 0.0,
                "turnover": abs(final_weight),
                "interval_exposure": abs(final_weight),
                "executed_weight": 0.0,
                "execution_price_cost": 0.0,
                "commission_notional": abs(final_weight),
            })
            final_weight = 0.0

        # P7B: translated AFTER the forced-flatten mutation above, so the
        # fold-end flatten (if any) is correctly seen as executed_weight=0.0
        # here too -- a full flatten needs no price (see weight_to_target_qty
        # docstring), so the priceless forced-exit row never fails closed on
        # a missing close. Uses each row's own close (the SAME close already
        # used for that row's gross_return/turnover above) as the sizing
        # price -- never a later row's price (no future-bar leakage).
        if wts_spec is not None:
            price_rows: List[Dict[str, Any]] = []
            for r in rows:
                close_val = close_frame.at[r["timestamp"], s]
                price_rows.append({
                    "timestamp": r["timestamp"],
                    "close": float(close_val) if pd.notna(close_val) else None,
                    "executed_weight": r["executed_weight"],
                })
            weight_to_share_events_by_symbol[s] = translate_symbol_weight_series_to_share_events(
                price_rows, wts_spec
            )

        exec_index: List[pd.Timestamp] = []
        exec_values: List[float] = []
        for r in rows:
            t = r["timestamp"]
            gross_by_ts[t] += r["gross_contrib"]
            turnover_by_ts[t] += r["turnover"]
            interval_exposure_by_ts[t] += r["interval_exposure"]
            execution_price_cost_by_ts[t] += r["execution_price_cost"]
            commission_notional_by_ts[t] += r["commission_notional"]
            exec_index.append(t)
            exec_values.append(r["executed_weight"])
        executed_weight_series[s] = pd.Series(exec_values, index=pd.DatetimeIndex(exec_index))

    # Post-trade holdings, forward-filled per symbol onto the full fold
    # timeline for exposure/active-position REPORTING only — a symbol
    # lacking a bar this row still holds whatever it last executed; this
    # never feeds gross_return/turnover, which are only ever written on a
    # symbol's own event rows above.
    exposure_frame = pd.DataFrame(index=bar_index)
    for s in symbols:
        exposure_frame[s] = executed_weight_series[s].reindex(bar_index).ffill().fillna(0.0)

    gross_return = pd.Series([gross_by_ts[t] for t in bar_index], index=bar_index)
    turnover = pd.Series([turnover_by_ts[t] for t in bar_index], index=bar_index)
    interval_exposure = pd.Series([interval_exposure_by_ts[t] for t in bar_index], index=bar_index)
    execution_price_cost = pd.Series(
        [execution_price_cost_by_ts[t] for t in bar_index], index=bar_index
    )
    # P7A + REPAIR-01 (RESEARCH-EXECUTION-PRICING-PARITY-01-REPAIR-01):
    # commission and the directional adverse-price cost are two distinct,
    # additive components — see EconomicWalkForwardSpec.normalized()'s
    # double-charging guard, which requires cost_model.slippage_bps_per_side
    # == 0 whenever execution_price_cost can be nonzero. Under the OFFICIAL
    # rust_conservative_bar_range_v1 model, commission is charged against
    # actual conservative fill-price notional (commission_notional, already
    # rescaled per-row by fill/close — see _row_execution_pricing_components)
    # rather than close-priced turnover, mirroring Rust's
    # `commission.compute_fee(qty, fill_price)` where `fill_price` is the
    # same conservative fill used for the return-series drag. The forced
    # fold-end flatten exit's commission_notional is always its own
    # close/mark-priced turnover (ratio 1.0), matching Rust's flatten_all.
    # Under the diagnostic model, commission_notional == turnover for every
    # row (ratio is always 1.0 there), so the ORIGINAL single-expression
    # formula is used unchanged to guarantee bit-for-bit parity of the
    # diagnostic transaction_cost series with pre-REPAIR-01 behavior
    # (turnover * (a/10000) + turnover * (b/10000)
    # is not guaranteed bit-identical to turnover * ((a+b)/10000) in
    # floating point).
    if spec.execution_pricing.is_official_parity_model:
        commission_notional = pd.Series(
            [commission_notional_by_ts[t] for t in bar_index], index=bar_index
        )
        commission_cost = commission_notional * (spec.cost_model.commission_bps_per_side / 10_000.0)
        transaction_cost = commission_cost + execution_price_cost
    else:
        commission_cost = economics.compute_transaction_cost(turnover, one_way_cost_bps)
        transaction_cost = commission_cost + execution_price_cost
    net_return = economics.compute_net_return_exact(gross_return, transaction_cost)

    gross_exposure = exposure_frame.abs().sum(axis=1)
    active_positions = (exposure_frame.abs() > 1e-12).sum(axis=1)

    economics.validate_return_series(gross_return)
    economics.validate_return_series(net_return)

    max_allowed = spec.signal_policy.max_gross_exposure + 1e-9
    if (gross_exposure > max_allowed).any():
        raise RuntimeError("Fail-closed: gross exposure exceeded configured max_gross_exposure")

    fold_df = pd.DataFrame({
        "fold": fold_no,
        "timestamp": [pd.Timestamp(t).isoformat() for t in bar_index],
        "gross_return": gross_return.to_numpy(dtype=float),
        "turnover": turnover.to_numpy(dtype=float),
        "transaction_cost": transaction_cost.to_numpy(dtype=float),
        "execution_price_cost": execution_price_cost.to_numpy(dtype=float),
        "commission_cost": commission_cost.to_numpy(dtype=float),
        "net_return": net_return.to_numpy(dtype=float),
        "interval_exposure": interval_exposure.to_numpy(dtype=float),
        "gross_exposure": gross_exposure.to_numpy(dtype=float),
        "active_positions": active_positions.to_numpy(dtype=int),
    })

    daily = _daily_aggregate(fold_df)
    summary = _summarize_series(daily, spec.annualization)
    summary.update({
        "fold": fold_no,
        "test_start_utc": boundaries["test_start"].isoformat(),
        "test_end_utc": boundaries["test_end"].isoformat(),
        "bars_used": int(len(close_frame)),
        "symbols": list(close_frame.columns),
        "total_turnover": float(fold_df["turnover"].sum()),
        "average_gross_exposure": float(fold_df["gross_exposure"].mean()) if len(fold_df) else 0.0,
    })
    if wts_spec is not None:
        # JSON-safe: pd.Timestamp -> isoformat string (this fold summary is
        # embedded verbatim into economic_walk_forward.json's "folds" array).
        summary["weight_to_share_evidence"] = {
            s: [
                {**event, "timestamp": pd.Timestamp(event["timestamp"]).isoformat()}
                for event in events
            ]
            for s, events in weight_to_share_events_by_symbol.items()
        }
    return fold_df, summary


# ---------------------------------------------------------------------------
# Top-level entry point
# ---------------------------------------------------------------------------


def run_economic_walkforward(
    run_dir: Path,
    *,
    bars_csv: Path,
    spec: EconomicWalkForwardSpec,
    walk_forward_eval_path: Optional[Path] = None,
    oos_predictions_path: Optional[Path] = None,
    provenance_manifest: Optional[Dict[str, Any]] = None,
) -> Path:
    """`provenance_manifest` (mqk_research.data.bars_provenance) is
    OPTIONAL here (BKT-DATA-PROVENANCE-POINT-IN-TIME-01-REPAIR-01): this is
    the low-level, non-registered entry point, and existing direct/
    synthetic callers construct bars_csv fixtures with no durable
    provenance record at all. When omitted (None), no corporate-action
    preflight runs -- this is the explicit diagnostic path, distinct from
    economic_registry_integration.run_registered_economic_walkforward_eval,
    which ALWAYS requires and verifies a real manifest before calling this
    function. When supplied, the manifest is first CONTENT-BOUND to the
    actually-loaded bars (Defect 1 / P8 REPAIR-02 -- see
    mqk_research.data.bars_provenance.require_bars_match_manifest, which
    catches a stale/wrong manifest paired with different bars data), and
    only then is the manifest's corporate-action policy checked against
    that bars content BEFORE any fold is simulated (see
    check_corporate_action_integrity) -- an integrity preflight, not a
    change to the existing future-execution chronology below."""
    run_dir = Path(run_dir)
    spec = spec.normalized()
    bars_csv = Path(bars_csv)

    eval_dir = run_dir / "eval"
    wf_path = Path(walk_forward_eval_path) if walk_forward_eval_path else eval_dir / "walk_forward_eval.json"
    oos_path = Path(oos_predictions_path) if oos_predictions_path else eval_dir / "walk_forward_oos_predictions.csv"
    if not wf_path.exists():
        raise FileNotFoundError(f"Fail-closed: missing walk-forward eval artifact: {wf_path}")

    wf_eval = json.loads(wf_path.read_text(encoding="utf-8"))
    used_folds = _parse_used_folds(wf_eval)

    use_official_pricing = spec.execution_pricing.is_official_parity_model
    bars_record = verify_bars_provenance(run_dir, bars_csv)
    bars = load_bars(bars_csv, require_pricing_columns=use_official_pricing)
    if provenance_manifest is not None:
        require_bars_match_manifest(bars, provenance_manifest)
        check_corporate_action_integrity(bars, provenance_manifest)
        if use_official_pricing:
            require_bars_pricing_provenance(bars, provenance_manifest)
    oos = load_oos_predictions(oos_path)

    fold_frames: List[pd.DataFrame] = []
    fold_summaries: List[Dict[str, Any]] = []
    for boundaries in used_folds:
        fold_no = boundaries["fold"]
        oos_fold = oos[oos["fold"] == fold_no]
        if oos_fold.empty:
            raise RuntimeError(f"Fail-closed: no OOS predictions found for used fold {fold_no}")
        symbols = sorted(oos_fold["symbol"].unique())
        close_frame = _build_fold_close_frame(bars, symbols, boundaries["test_start"], boundaries["test_end"])
        high_frame = low_frame = None
        if use_official_pricing:
            high_frame, low_frame = _build_fold_high_low_frames(
                bars, symbols, boundaries["test_start"], boundaries["test_end"]
            )
        pending_events = _build_pending_events(oos_fold, symbols, spec.signal_policy)
        fold_df, fold_summary = _simulate_fold(
            fold_no, boundaries, close_frame, pending_events, spec,
            high_frame=high_frame, low_frame=low_frame,
        )
        fold_frames.append(fold_df)
        fold_summaries.append(fold_summary)

    stitched = pd.concat(fold_frames, ignore_index=True)
    stitched = stitched.sort_values(["fold", "timestamp"], kind="mergesort").reset_index(drop=True)

    daily_all = _daily_aggregate(stitched)
    aggregate = _summarize_series(daily_all, spec.annualization)
    fold_net_totals = [fs["net_total_return"] for fs in fold_summaries]
    aggregate.update({
        "stitched_oos_rows": int(len(stitched)),
        "total_turnover": float(stitched["turnover"].sum()) if len(stitched) else 0.0,
        "folds_used": len(fold_summaries),
        "median_fold_net_total_return": float(np.median(fold_net_totals)),
        "profitable_fold_count": int(sum(1 for v in fold_net_totals if v > 0.0)),
        "worst_fold_net_total_return": float(min(fold_net_totals)),
    })

    eval_dir.mkdir(parents=True, exist_ok=True)
    economic_returns_path = eval_dir / "economic_returns.csv"
    stitched.to_csv(economic_returns_path, index=False)
    economic_daily_path = eval_dir / "economic_daily_returns.csv"
    daily_all.to_csv(economic_daily_path, index=False)

    identity = economic_protocol_identity(spec)
    out: Dict[str, Any] = {
        "schema_version": "economic_walk_forward_v1",
        "protocol": {"protocol_id": spec.protocol_id},
        "signal_policy": identity["signal_policy"],
        "cost_model": identity["cost_model"],
        "execution_pricing": identity["execution_pricing"],
        "weight_to_share": identity["weight_to_share"],
        "annualization": identity["annualization"],
        "inputs": {
            "bars_csv": bars_record,
            "oos_predictions_csv": file_record(oos_path),
            "walk_forward_eval": file_record(wf_path),
        },
        "outputs": {
            "economic_returns_csv": file_record(economic_returns_path),
            "economic_daily_returns_csv": file_record(economic_daily_path),
        },
        "holdout": {"status": "reserved_not_evaluated"},
        "folds": fold_summaries,
        "aggregate": aggregate,
        "bars_provenance": provenance_manifest,
    }
    out["ids"] = {"economic_eval_id": sha256_json(out)}

    out_path = eval_dir / "economic_walk_forward.json"
    out_path.write_text(json.dumps(out, sort_keys=True, separators=(",", ":")), encoding="utf-8")
    return out_path
