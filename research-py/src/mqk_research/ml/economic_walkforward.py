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
    DISCRETE_ECONOMICS_PROTOCOL_ID_V1,
    WeightToShareSpec,
    target_qty_to_order_delta,
    weight_to_share_protocol_identity,
    weight_to_target_qty,
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

# RESEARCH-LONG-SHORT-ECONOMIC-POLICY-01: two mutually-exclusive, versioned
# signal DIRECTION policies. `long_only_v1` is the original, still-frozen
# behavior (bit-for-bit unchanged) -- existing artifacts/candidates that
# never set `direction_policy` reproduce identically. `long_short_
# threshold_v1` is new capability, gated behind an explicit, distinct
# identity so a legacy long-only evaluation can never be confused with (or
# silently reinterpreted as) a long/short one. See SignalPolicySpec.
SIGNAL_DIRECTION_POLICY_LONG_ONLY_V1 = "long_only_v1"
SIGNAL_DIRECTION_POLICY_LONG_SHORT_THRESHOLD_V1 = "long_short_threshold_v1"

# DIRECT-SIGNED-RANK-RESEARCH-POLICY-01: two mutually-exclusive, versioned
# CROSS-SECTIONAL RANK direction policies. Unlike the threshold policies
# above (per-symbol, memoryless, independent of any other symbol's score),
# these rank the persisted OOS `ml_score` -- never target/fwd_ret, never a
# raw feature value -- across every symbol scored at the SAME exact decision
# timestamp, and assign the top/bottom `rank_side_count` names long/short.
# See SignalPolicySpec docstring and _build_rank_pending_events for the full
# semantics. `long_only_v1`/`long_short_threshold_v1` remain completely
# frozen and untouched by this addition.
SIGNAL_DIRECTION_POLICY_CROSS_SECTIONAL_RANK_LONG_ONLY_V1 = "cross_sectional_rank_long_only_v1"
SIGNAL_DIRECTION_POLICY_CROSS_SECTIONAL_RANK_LONG_SHORT_V1 = "cross_sectional_rank_long_short_v1"

KNOWN_SIGNAL_DIRECTION_POLICY_IDS = frozenset(
    {
        SIGNAL_DIRECTION_POLICY_LONG_ONLY_V1,
        SIGNAL_DIRECTION_POLICY_LONG_SHORT_THRESHOLD_V1,
        SIGNAL_DIRECTION_POLICY_CROSS_SECTIONAL_RANK_LONG_ONLY_V1,
        SIGNAL_DIRECTION_POLICY_CROSS_SECTIONAL_RANK_LONG_SHORT_V1,
    }
)
RANK_DIRECTION_POLICY_IDS = frozenset(
    {
        SIGNAL_DIRECTION_POLICY_CROSS_SECTIONAL_RANK_LONG_ONLY_V1,
        SIGNAL_DIRECTION_POLICY_CROSS_SECTIONAL_RANK_LONG_SHORT_V1,
    }
)

# The single, fixed (non-configurable) rank policy sizing/tie identity --
# see SignalPolicySpec docstring "SIZING ID" / "TIE POLICY". Distinct from
# legacy `sizing="equal_weight_active"` so a rank trial's identity can never
# collide with a threshold-policy trial's.
SIGNAL_SIZING_EQUAL_WEIGHT_RANK_SELECTED_V1 = "equal_weight_rank_selected_v1"
RANK_TIE_POLICY_ID = "fail_closed_boundary_ties_v1"

# Research/Backtest-only scope declaration (mission Section 5F): this
# economic evaluator has no point-in-time borrow/shortability history for
# its Research universe. `long_short_threshold_v1` explicitly, permanently
# assumes every symbol in the evaluated universe is shortable for the
# purpose of THIS research measurement -- it does NOT prove real broker
# borrow availability, and must never be read as authorizing Paper/Live
# short execution (that remains a completely separate, unimplemented
# routing/risk decision gated on real borrow/shortability data).
BORROW_MODEL_RESEARCH_ASSUMED_SHORTABLE_UNIVERSE_V1 = "research_assumed_shortable_universe_v1"
KNOWN_BORROW_MODEL_IDS = frozenset({BORROW_MODEL_RESEARCH_ASSUMED_SHORTABLE_UNIVERSE_V1})


# ---------------------------------------------------------------------------
# Specs
# ---------------------------------------------------------------------------


@dataclass(frozen=True)
class SignalPolicySpec:
    """Explicit, versioned signal-to-position policy.

    `direction_policy` (RESEARCH-LONG-SHORT-ECONOMIC-POLICY-01) selects
    between two mutually-exclusive, distinctly-identified sub-protocols:

    - `long_only_v1` (the default -- ORIGINAL, FROZEN behavior): threshold
      entry, equal-weight sizing across ACTIVE names, bounded gross
      exposure. `entry_threshold` is the sole activation threshold;
      `long_only` must be True; `short_threshold`/`borrow_model` must be
      unset. Every existing candidate/artifact that never set
      `direction_policy` reproduces bit-for-bit identically under this
      branch (see test_legacy_long_only_reproduces_previous_semantics).

    - `long_short_threshold_v1` (NEW): a per-symbol score maps
      deterministically, every decision frame, to one of three states --
      `score >= entry_threshold` -> LONG, `score <= short_threshold` ->
      SHORT, otherwise -> FLAT (mission Section 5B; NOT a hysteresis/
      sticky-state rule -- every scored frame recomputes the state from
      scratch, exactly like long_only_v1's own active/flat recomputation).
      Requires `long_only=False` and `0 <= short_threshold <
      entry_threshold <= 1`. `borrow_model` defaults to
      BORROW_MODEL_RESEARCH_ASSUMED_SHORTABLE_UNIVERSE_V1 (Research-only
      scope declaration, see that constant's docstring) and is always
      identity-bearing.

      TRUTHFUL SCORE SEMANTICS (mission Section 2 "LONG-SHORT TERMINOLOGY
      DEFECT"): the model's actual training truth is `target = 1 iff
      fwd_ret > a POSITIVE return threshold`, and `ml_score = P(target =
      1)` -- the probability of that BULLISH positive-return class ONLY. A
      score `<= short_threshold` is therefore a LOW probability of the
      bullish class -- NOT a calibrated estimate of how likely the price
      is to fall (a mathematically distinct claim the model was never
      trained to produce, since it never partitions the negative-return
      outcome on its own). `short_threshold` encodes an explicit BEARISH
      STRATEGY HYPOTHESIS (low-bullish-confidence names are used as a
      short candidate), not a statistically-certain forecast of a falling
      price.

    GROSS EXPOSURE (both direction policies): `max_gross_exposure` bounds
    `sum(abs(weight_i))`, NEVER the signed sum -- a +0.5 long and a -0.5
    short together consume gross 1.0, not 0.0 (mission Section 5C). Under
    `long_only_v1` this coincides with the signed sum since weights are
    always >= 0.

    MULTI-SYMBOL EQUAL-WEIGHT SIZING RULE: the DESIRED portfolio target
    (this policy) is recomputed at each decision frame from every symbol's
    latest known LONG/SHORT/FLAT state (see _build_pending_events) — never
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
    closed on an expected asynchronous pattern. RESEARCH-LONG-SHORT-
    ECONOMIC-POLICY-01: _simulate_fold_execution's gross-reducing-vs-
    gross-increasing classification is by GROSS MAGNITUDE delta
    (abs(candidate)-abs(executed)), not raw signed comparison -- a
    long<->short sign flip at equal magnitude consumes no additional
    capacity and executes unconditionally; an exact generalization that is
    bit-for-bit identical to the prior long-only-only comparison whenever
    weights are non-negative.

    DIRECT-SIGNED-RANK-RESEARCH-POLICY-01: `cross_sectional_rank_long_only_v1`
    / `cross_sectional_rank_long_short_v1` select a THIRD, mutually-exclusive
    family -- see module docstring and _build_rank_pending_events. These
    require `rank_side_count` (a positive int, K) and reject/canonicalize
    every field that would be economically meaningless for a rank policy so
    it can never manufacture a spurious distinct trial identity:

    - `entry_threshold` MUST be exactly 0.5 (the field's own canonical
      default) -- rank policies have no probability threshold, so the
      safest implementation rejects any other value outright rather than
      silently canonicalizing it (mission "THRESHOLD FIELD SEMANTICS").
    - `short_threshold` MUST be None (rejected outright, mirrors above).
    - `long_only` MUST match the direction policy exactly
      (`cross_sectional_rank_long_only_v1` -> True,
      `cross_sectional_rank_long_short_v1` -> False) -- fail closed on
      mismatch.
    - `borrow_model` MUST be None for the long-only rank policy (there is
      never a short leg) and defaults to/requires the same Research-only
      BORROW_MODEL_RESEARCH_ASSUMED_SHORTABLE_UNIVERSE_V1 as
      `long_short_threshold_v1` for the long/short rank policy.
    - `sizing` is forced to SIGNAL_SIZING_EQUAL_WEIGHT_RANK_SELECTED_V1
      (the field carries no other meaningful choice under a rank policy --
      see module "SIZING ID").
    """

    entry_threshold: float = 0.5
    long_only: bool = True
    direction_policy: str = SIGNAL_DIRECTION_POLICY_LONG_ONLY_V1
    short_threshold: Optional[float] = None
    borrow_model: Optional[str] = None
    sizing: str = "equal_weight_active"
    max_gross_exposure: float = 1.0
    fold_end_policy: str = "force_flat_last_bar"
    capacity_policy: str = CAPACITY_POLICY_ID
    # DIRECT-SIGNED-RANK-RESEARCH-POLICY-01: the sole new semantic parameter
    # (mission "RANK POLICY PARAMETERS") -- K, the number of names selected
    # per side. Required (and validated positive) for the two rank
    # direction policies; must be None for long_only_v1/
    # long_short_threshold_v1 (fail closed on mismatch either way).
    rank_side_count: Optional[int] = None
    schema_version: str = "economic_signal_policy_v1"

    def normalized(self) -> "SignalPolicySpec":
        if self.direction_policy not in KNOWN_SIGNAL_DIRECTION_POLICY_IDS:
            raise ValueError(f"unsupported signal direction_policy: {self.direction_policy!r}")
        entry_threshold = float(self.entry_threshold)
        if not (0.0 <= entry_threshold <= 1.0):
            raise ValueError("entry_threshold must be within [0,1]")

        short_threshold: Optional[float]
        borrow_model: Optional[str]
        sizing: str
        rank_side_count: Optional[int]
        if self.direction_policy == SIGNAL_DIRECTION_POLICY_LONG_ONLY_V1:
            if not self.long_only:
                raise ValueError(
                    "economic_walk_forward_v1's long_only_v1 direction_policy requires long_only=True"
                )
            if self.short_threshold is not None:
                raise ValueError("long_only_v1 direction_policy does not accept short_threshold")
            if self.borrow_model is not None:
                raise ValueError("long_only_v1 direction_policy does not accept borrow_model")
            if self.rank_side_count is not None:
                raise ValueError("long_only_v1 direction_policy does not accept rank_side_count")
            if self.sizing != "equal_weight_active":
                raise ValueError("economic_walk_forward_v1 supports only sizing='equal_weight_active'")
            short_threshold = None
            borrow_model = None
            sizing = self.sizing
            rank_side_count = None
        elif self.direction_policy == SIGNAL_DIRECTION_POLICY_LONG_SHORT_THRESHOLD_V1:
            if self.long_only:
                raise ValueError(
                    "long_short_threshold_v1 direction_policy requires long_only=False"
                )
            if self.short_threshold is None:
                raise ValueError("long_short_threshold_v1 direction_policy requires short_threshold")
            short_threshold = float(self.short_threshold)
            if not (0.0 <= short_threshold < entry_threshold <= 1.0):
                raise ValueError(
                    "long_short_threshold_v1 requires 0 <= short_threshold < entry_threshold(long) "
                    f"<= 1, got short_threshold={short_threshold!r} entry_threshold={entry_threshold!r}"
                )
            borrow_model = self.borrow_model or BORROW_MODEL_RESEARCH_ASSUMED_SHORTABLE_UNIVERSE_V1
            if borrow_model not in KNOWN_BORROW_MODEL_IDS:
                raise ValueError(f"unsupported borrow_model: {borrow_model!r}")
            if self.rank_side_count is not None:
                raise ValueError("long_short_threshold_v1 direction_policy does not accept rank_side_count")
            if self.sizing != "equal_weight_active":
                raise ValueError("economic_walk_forward_v1 supports only sizing='equal_weight_active'")
            sizing = self.sizing
            rank_side_count = None
        else:
            # DIRECT-SIGNED-RANK-RESEARCH-POLICY-01: the two cross-sectional
            # rank direction policies (see SignalPolicySpec docstring
            # "RANK POLICY PARAMETERS"/"THRESHOLD FIELD SEMANTICS").
            assert self.direction_policy in RANK_DIRECTION_POLICY_IDS
            is_rank_long_short = (
                self.direction_policy == SIGNAL_DIRECTION_POLICY_CROSS_SECTIONAL_RANK_LONG_SHORT_V1
            )
            expected_long_only = not is_rank_long_short
            if self.long_only != expected_long_only:
                raise ValueError(
                    f"{self.direction_policy} direction_policy requires long_only={expected_long_only}"
                )
            if entry_threshold != 0.5:
                raise ValueError(
                    f"{self.direction_policy} direction_policy has no meaningful probability "
                    "threshold and requires entry_threshold=0.5 exactly (rank policies rank "
                    "ml_score cross-sectionally; a threshold cannot be used to manufacture a "
                    "distinct trial identity)"
                )
            if self.short_threshold is not None:
                raise ValueError(f"{self.direction_policy} direction_policy does not accept short_threshold")
            if self.rank_side_count is None:
                raise ValueError(f"{self.direction_policy} direction_policy requires rank_side_count")
            rank_side_count = int(self.rank_side_count)
            if rank_side_count <= 0:
                raise ValueError("rank_side_count must be a positive integer")
            if is_rank_long_short:
                borrow_model = self.borrow_model or BORROW_MODEL_RESEARCH_ASSUMED_SHORTABLE_UNIVERSE_V1
                if borrow_model not in KNOWN_BORROW_MODEL_IDS:
                    raise ValueError(f"unsupported borrow_model: {borrow_model!r}")
            else:
                if self.borrow_model is not None:
                    raise ValueError(
                        f"{self.direction_policy} direction_policy does not accept borrow_model "
                        "(there is no short leg)"
                    )
                borrow_model = None
            short_threshold = None
            sizing = SIGNAL_SIZING_EQUAL_WEIGHT_RANK_SELECTED_V1

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
            entry_threshold=entry_threshold,
            short_threshold=short_threshold,
            borrow_model=borrow_model,
            sizing=sizing,
            rank_side_count=rank_side_count,
            max_gross_exposure=float(self.max_gross_exposure),
        )

    @property
    def is_long_short(self) -> bool:
        return self.direction_policy == SIGNAL_DIRECTION_POLICY_LONG_SHORT_THRESHOLD_V1

    @property
    def is_rank(self) -> bool:
        return self.direction_policy in RANK_DIRECTION_POLICY_IDS

    @property
    def is_rank_long_short(self) -> bool:
        return self.direction_policy == SIGNAL_DIRECTION_POLICY_CROSS_SECTIONAL_RANK_LONG_SHORT_V1


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
            # RESEARCH-LONG-SHORT-ECONOMIC-POLICY-01-REPAIR-01 (mission
            # Section 4A): ADDITIVE ONLY under long_short_threshold_v1 --
            # direction_policy/short_threshold/borrow_model are always
            # identity-bearing THERE (a long/short candidate differing only
            # by short_threshold or borrow_model must never share a
            # trial_id), but are entirely ABSENT from the dict under the
            # legacy long_only_v1 default (both are always None there
            # anyway -- SignalPolicySpec.normalized() enforces it). Absence,
            # not a constant None value, is what keeps every pre-existing
            # long_only_v1 candidate's canonical identity byte-for-byte
            # identical to its pre-long-short-patch trial_id (canonical_json
            # sorts keys, so PRESENCE is what changes the hash, not order --
            # see test_legacy_long_only_identity_exact_golden_equality).
            **(
                {
                    "direction_policy": spec.signal_policy.direction_policy,
                    "short_threshold": spec.signal_policy.short_threshold,
                    "borrow_model": spec.signal_policy.borrow_model,
                }
                if spec.signal_policy.is_long_short
                else {}
            ),
            # DIRECT-SIGNED-RANK-RESEARCH-POLICY-01: ADDITIVE ONLY under a
            # rank direction policy, same absence-not-None-value rationale as
            # the long/short block above -- a legacy long_only_v1/
            # long_short_threshold_v1 candidate's identity is completely
            # unaffected by this addition. `rank_side_count`/`borrow_model`/
            # `tie_policy` are all identity-bearing here per mission
            # ("IDENTITY-BEARING FIELDS"): two rank candidates that differ
            # only by rank_side_count, or a long-only vs long/short rank
            # candidate, or a borrow-model change, must never share a
            # trial_id. `tie_policy` is not a configurable field (V1 has
            # exactly one, fixed, fail-closed tie policy -- see module
            # docstring "TIE POLICY") but is still included explicitly for
            # audit clarity, per mission "if it is represented as a
            # configurable field, it MUST be identity-bearing" -- included
            # here unconditionally as the current fixed constant so a future
            # V2 tie policy would visibly change identity too.
            **(
                {
                    "direction_policy": spec.signal_policy.direction_policy,
                    "rank_side_count": spec.signal_policy.rank_side_count,
                    "borrow_model": spec.signal_policy.borrow_model,
                    "tie_policy": RANK_TIE_POLICY_ID,
                }
                if spec.signal_policy.is_rank
                else {}
            ),
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


def _resolve_signal_direction(score: float, signal_policy: SignalPolicySpec) -> int:
    """RESEARCH-LONG-SHORT-ECONOMIC-POLICY-01: deterministic, memoryless
    per-symbol direction resolution for a single evaluated `score`, per
    `signal_policy.direction_policy` -- returns +1 (long), -1 (short), or 0
    (flat). Under `long_only_v1` this is exactly the original two-state
    active/flat rule (never returns -1). Under `long_short_threshold_v1`
    (mission Section 5B): `score >= entry_threshold` -> long, `score <=
    short_threshold` -> short, otherwise -> flat. `SignalPolicySpec.
    normalized()` already guarantees `0 <= short_threshold <
    entry_threshold <= 1`, so these three ranges are mutually exclusive and
    exhaustive."""
    if signal_policy.direction_policy == SIGNAL_DIRECTION_POLICY_LONG_ONLY_V1:
        return 1 if score >= signal_policy.entry_threshold else 0
    if score >= signal_policy.entry_threshold:
        return 1
    assert signal_policy.short_threshold is not None
    if score <= signal_policy.short_threshold:
        return -1
    return 0


def _build_pending_events(
    oos_fold: pd.DataFrame,
    symbols: List[str],
    signal_policy: SignalPolicySpec,
    *,
    close_frame: Optional[pd.DataFrame] = None,
    wts_spec: Optional[WeightToShareSpec] = None,
) -> Dict[str, List[Tuple[pd.Timestamp, float, Optional[int]]]]:
    """Per-symbol, chronologically-ordered list of
    (signal_ts, desired_weight, signal_time_target_qty) pending-change
    events — the DESIRED side of the desired/pending/executed state machine
    (see module docstring). Does not decide WHEN or WHETHER a symbol
    executes a change; that is a per-frame decision made later in
    _simulate_fold_execution, jointly across symbols, against each symbol's
    own bar timestamps and the shared max_gross_exposure budget.

    A decision frame (one distinct decision_ts, aggregating every symbol
    scored at that instant — physical CSV row order within the frame does
    not matter) only produces new pending events for a symbol whose
    LONG/SHORT/FLAT DIRECTION STATE actually changes at that frame, OR for
    every other symbol when someone else's direction-state change alters
    the equal-weight-active sizing denominator (a rebalance of everyone
    currently non-flat/target). A symbol not scored at a given frame keeps
    its last known direction state — it is never implicitly forced flat
    merely for lacking a decision row at some other symbol's frame.

    RESEARCH-LONG-SHORT-ECONOMIC-POLICY-01: `direction_state[s]` is a
    SIGNED int in {-1, 0, +1} (short/flat/long), resolved fresh every
    scored frame from `signal_policy.direction_policy` (see
    SignalPolicySpec docstring) -- NOT a hysteresis/sticky-state rule.
    Under `long_only_v1`, direction_state is always in {0, 1} (Python bool
    IS an int subtype, so this is the EXACT same representation the
    original active/flat logic used), and `weight_each_magnitude *
    direction_state[s]` reduces to the original `weight_each if
    active_state[s] else 0.0` bit-for-bit. Under `long_short_threshold_v1`,
    the SAME equal-weight-active-count denominator now counts every
    non-flat (long OR short) symbol, and gross exposure is consumed by
    MAGNITUDE regardless of side (mission Section 5C).

    P7B-REPAIR-01 (SIGNAL-TIME SIZING, mission Section 4B): when `wts_spec`
    is engaged, `signal_time_target_qty` is computed HERE, exactly once, at
    the moment the event is created, using THIS symbol's own close AT
    `signal_ts` (`close_frame.at[signal_ts, s]`) — never a later execution
    bar's close/high/low. This value is then carried immutably through
    `_simulate_fold_execution`'s discrete state machine; mutating any bar
    strictly after `signal_ts` cannot change it, because it is never
    recomputed after this point (no future-bar leakage — REQUIRED TESTS 2/3
    in the repair mission). A weight of exactly 0.0 (flatten) always yields
    `target_qty=0` without requiring a price, mirroring
    `weight_to_target_qty`'s own price-optional-when-zero contract. `None`
    (missing/NaN close at signal_ts) for a NONZERO weight fails closed via
    `weight_to_target_qty`'s own fail-closed-on-missing-price behavior."""
    decisions = oos_fold.pivot_table(index="decision_ts", columns="symbol", values="ml_score", aggfunc="last")
    decisions = decisions.reindex(columns=symbols).sort_index(kind="mergesort")

    direction_state: Dict[str, int] = {s: 0 for s in symbols}
    last_issued_weight: Dict[str, float] = {s: 0.0 for s in symbols}
    pending_events: Dict[str, List[Tuple[pd.Timestamp, float, Optional[int]]]] = {s: [] for s in symbols}

    for ts, row in decisions.iterrows():
        changed = False
        for s in symbols:
            score = row[s]
            if pd.notna(score):
                new_direction = _resolve_signal_direction(float(score), signal_policy)
                if new_direction != direction_state[s]:
                    direction_state[s] = new_direction
                    changed = True
        if not changed:
            continue

        active_symbols = [s for s in symbols if direction_state[s] != 0]
        weight_each_magnitude = (
            float(signal_policy.max_gross_exposure) / float(len(active_symbols)) if active_symbols else 0.0
        )
        for s in symbols:
            new_weight = weight_each_magnitude * direction_state[s]
            if abs(new_weight - last_issued_weight[s]) > 1e-12:
                signal_ts = pd.Timestamp(ts)
                target_qty: Optional[int] = None
                if wts_spec is not None:
                    signal_price: Optional[float] = None
                    if close_frame is not None and signal_ts in close_frame.index:
                        raw = close_frame.at[signal_ts, s]
                        signal_price = float(raw) if pd.notna(raw) else None
                    target_qty = weight_to_target_qty(weight=new_weight, price=signal_price, spec=wts_spec)
                pending_events[s].append((signal_ts, new_weight, target_qty))
                last_issued_weight[s] = new_weight

    return pending_events


_RANK_SCORE_TIE_TOL = 1e-9


def _signal_time_target_qty(
    s: str,
    signal_ts: pd.Timestamp,
    new_weight: float,
    close_frame: Optional[pd.DataFrame],
    wts_spec: Optional[WeightToShareSpec],
) -> Optional[int]:
    """Shared signal-time qty/price contract (P7B-REPAIR-01 -- see
    _build_pending_events's own inline use of this exact contract), reused
    verbatim by _build_rank_pending_events: `target_qty` is fixed ONCE, here,
    from THIS symbol's own close AT `signal_ts` -- never recomputed from a
    later execution-time bar. A weight of exactly 0.0 always yields
    target_qty=0 without requiring a price; a missing/NaN close for a
    nonzero weight fails closed via weight_to_target_qty's own contract."""
    if wts_spec is None:
        return None
    signal_price: Optional[float] = None
    if close_frame is not None and signal_ts in close_frame.index:
        raw = close_frame.at[signal_ts, s]
        signal_price = float(raw) if pd.notna(raw) else None
    return weight_to_target_qty(weight=new_weight, price=signal_price, spec=wts_spec)


def _resolve_rank_direction_for_frame(
    scores: Dict[str, float], rank_side_count: int, long_only: bool
) -> Dict[str, int]:
    """DIRECT-RANK-AND-BROAD-UNIVERSE-RESEARCH-01: pure, deterministic
    cross-sectional rank resolution for ONE decision frame's rankable score
    set. `scores` is exactly whatever symbols were actually scored at this
    ONE exact decision timestamp (see _build_rank_pending_events's "DYNAMIC
    CROSS-SECTION V1" docstring) -- membership is free to differ from any
    other timestamp's; this function has no notion of a fold-wide universe
    and never sees a stale/missing entry (the caller only ever passes what
    was truthfully scored at T).

    Fails closed (RuntimeError) if:
      - the cross-section is too small for `rank_side_count` (long-only
        requires `len(scores) >= rank_side_count`; long/short requires
        `len(scores) >= 2*rank_side_count`);
      - the long boundary (Kth-highest score) ties the next score outside
        the long-selected set;
      - the short boundary (Kth-lowest score) ties the next score outside
        the short-selected set (long/short only);
      - the selected long/short sets overlap (defense-in-depth -- the size
        gate above already makes this unreachable through this function's
        own call path, but this is independently asserted here since this
        is the actual selection primitive under direct unit test).

    RANK_TIE_POLICY_ID = fail_closed_boundary_ties_v1 (mission "TIE
    POLICY"). Sort key is `ml_score` only -- `scores`' dict iteration/
    insertion order never participates, so CSV/caller row-order permutation
    can never change the result."""
    n = len(scores)
    if long_only:
        if n < rank_side_count:
            raise RuntimeError(
                "Fail-closed: cross-sectional rank long-only requires at least "
                f"rank_side_count={rank_side_count} rankable symbols at this decision "
                f"timestamp, got {n}"
            )
    else:
        if n < 2 * rank_side_count:
            raise RuntimeError(
                "Fail-closed: cross-sectional rank long/short requires at least "
                f"2*rank_side_count={2 * rank_side_count} rankable symbols at this decision "
                f"timestamp, got {n}"
            )

    ranked = sorted(scores.items(), key=lambda kv: kv[1], reverse=True)
    direction: Dict[str, int] = {s: 0 for s in scores}

    top = ranked[:rank_side_count]
    long_set = {sym for sym, _ in top}
    if rank_side_count < n:
        boundary_score = ranked[rank_side_count - 1][1]
        outside_score = ranked[rank_side_count][1]
        if abs(boundary_score - outside_score) <= _RANK_SCORE_TIE_TOL:
            raise RuntimeError(
                "Fail-closed: cross-sectional rank long boundary tie at "
                f"rank_side_count={rank_side_count} (score={boundary_score!r})"
            )
    for sym in long_set:
        direction[sym] = 1

    if not long_only:
        boundary_idx = n - rank_side_count
        bottom = ranked[boundary_idx:]
        short_set = {sym for sym, _ in bottom}
        if boundary_idx > 0:
            boundary_score = ranked[boundary_idx][1]
            outside_score = ranked[boundary_idx - 1][1]
            if abs(boundary_score - outside_score) <= _RANK_SCORE_TIE_TOL:
                raise RuntimeError(
                    "Fail-closed: cross-sectional rank short boundary tie at "
                    f"rank_side_count={rank_side_count} (score={boundary_score!r})"
                )
        if long_set & short_set:
            raise RuntimeError("Fail-closed: cross-sectional rank long/short selected sets overlap")
        for sym in short_set:
            direction[sym] = -1

    return direction


def _build_rank_pending_events(
    oos_fold: pd.DataFrame,
    symbols: List[str],
    signal_policy: SignalPolicySpec,
    *,
    close_frame: Optional[pd.DataFrame] = None,
    wts_spec: Optional[WeightToShareSpec] = None,
) -> Dict[str, List[Tuple[pd.Timestamp, float, Optional[int]]]]:
    """DIRECT-RANK-AND-BROAD-UNIVERSE-RESEARCH-01 counterpart to
    _build_pending_events for the two cross-sectional rank direction
    policies (see SignalPolicySpec docstring). Deliberately does NOT touch
    or reuse _build_pending_events's own per-symbol threshold/carry-forward
    loop -- only the signal-time qty/price contract
    (_signal_time_target_qty) and the downstream causal execution engine
    (_simulate_fold_execution, via _simulate_fold) are shared, so legacy
    long_only_v1/long_short_threshold_v1 behavior is provably unaffected by
    this addition (zero-diff on _build_pending_events itself).

    DYNAMIC CROSS-SECTION V1 (mission): unlike a fixed-universe design,
    EVERY decision timestamp is resolved INDEPENDENTLY from exactly the OOS
    rows scored at that exact instant -- ranking never reads any other
    timestamp's rows. A symbol's rankable membership is free to differ from
    one timestamp to the next (e.g. a name gaining walk-forward eligibility
    partway through a fold, or a newly-covered symbol appearing) -- a
    missing symbol at some frame is NOT an error and NEVER triggers
    carrying forward a stale prior score or fabricating a value for it; only
    a genuine DUPLICATE (symbol, decision_ts) row, or a cross-section too
    small for the configured `rank_side_count`, fails closed (see
    _resolve_rank_direction_for_frame).

    Weight magnitude here is FIXED by rank_side_count (max_gross_exposure/K
    long-only, max_gross_exposure/(2K) long/short) -- unlike
    _build_pending_events's all-currently-active rebalance-on-any-change
    trigger, it never depends on how many symbols are currently selected
    (that count is fixed by construction whenever selection succeeds), so a
    pending event is only emitted for a symbol whose own top/bottom-K
    membership actually changed at this frame."""
    assert signal_policy.rank_side_count is not None
    rank_side_count = int(signal_policy.rank_side_count)
    long_only = bool(signal_policy.long_only)
    max_gross = float(signal_policy.max_gross_exposure)
    weight_each = max_gross / float(rank_side_count if long_only else 2 * rank_side_count)

    dup_mask = oos_fold.duplicated(subset=["decision_ts", "symbol"], keep=False)
    if dup_mask.any():
        raise RuntimeError(
            "Fail-closed: duplicate (decision_ts,symbol) row in OOS predictions for a "
            "cross-sectional rank decision frame"
        )

    direction_state: Dict[str, int] = {s: 0 for s in symbols}
    pending_events: Dict[str, List[Tuple[pd.Timestamp, float, Optional[int]]]] = {s: [] for s in symbols}

    for ts, group in oos_fold.groupby("decision_ts", sort=True):
        scores = {str(sym): float(score) for sym, score in zip(group["symbol"], group["ml_score"])}
        new_direction = _resolve_rank_direction_for_frame(scores, rank_side_count, long_only)
        signal_ts = pd.Timestamp(ts)
        for s, direction in new_direction.items():
            if s not in pending_events:
                pending_events[s] = []
                direction_state[s] = 0
            if direction_state[s] == direction:
                continue
            direction_state[s] = direction
            new_weight = weight_each * direction
            target_qty = _signal_time_target_qty(s, signal_ts, new_weight, close_frame, wts_spec)
            pending_events[s].append((signal_ts, new_weight, target_qty))

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


def _row_execution_pricing_components_discrete(
    delta_qty: int,
    close: float,
    high: Optional[float],
    low: Optional[float],
    execution_pricing: Optional[ExecutionPricingSpec],
) -> Tuple[float, float, float, float]:
    """P7B-REPAIR-01 discrete-qty analogue of
    _row_execution_pricing_components (mission Section 4E "discrete shares
    must drive economics"): executing `delta_qty` signed shares at THIS
    bar's conservative fill price instead of its close. BUY (delta_qty > 0)
    uses HIGH, SELL (delta_qty < 0) uses LOW — identical side convention.

    Returns (execution_price_cost, commission_notional, turnover_notional,
    fill_price), the first three ALL RAW DOLLAR quantities (P7B-REPAIR-02,
    mission Section 3G: a stateful wealth ledger needs real dollars, not a
    value pre-divided by a FIXED equity_usd denominator -- see
    _discrete_wealth_ledger_returns, which is the only place a discrete
    dollar amount is ever turned into a return fraction, using the RUNNING
    equity level, not this fixed constant). `turnover_notional` is always
    CLOSE-priced (`abs(delta_qty) * close`) -- the intended rebalance size,
    mirroring continuous `turnover[s] = abs(delta_weight)` -- while
    `commission_notional` is FILL-priced under the official model (mirrors
    `commission_notional[s] = turnover[s] * fill_ratio`). Under the
    diagnostic pricing model (or delta_qty == 0), execution_price_cost is
    0.0 and commission_notional == turnover_notional (unrescaled, ratio
    1.0) -- mirrors _row_execution_pricing_components's own
    diagnostic-model identity. `fill_price` is returned so a caller
    (P7B-REPAIR-02's fill-time capacity check, mission Section 3D) can price
    a PROPOSED order's notional at the actual conservative fill BEFORE
    deciding whether to commit it -- this function is pure and has no side
    effects, so calling it ahead of an accept/reject decision is safe even
    if the order ends up rejected and its cost outputs discarded."""
    close_f = float(close)
    if delta_qty == 0:
        return 0.0, 0.0, 0.0, close_f
    turnover_notional = abs(delta_qty) * close_f
    if execution_pricing is None or not execution_pricing.is_official_parity_model:
        return 0.0, turnover_notional, turnover_notional, close_f
    if high is None or low is None or pd.isna(high) or pd.isna(low):
        raise RuntimeError(
            "Fail-closed: official execution-pricing parity model requires a high/low value for "
            "every bar with an executing discrete-share turnover event"
        )
    side = "buy" if delta_qty > 0 else "sell"
    fill = conservative_fill_price(
        high=float(high),
        low=float(low),
        close=close_f,
        side=side,
        slippage_bps=execution_pricing.slippage_bps,
        volatility_mult_bps=execution_pricing.volatility_mult_bps,
    )
    adverse_price_cost = abs(delta_qty) * abs(fill - close_f)
    commission_notional = abs(delta_qty) * fill
    return adverse_price_cost, commission_notional, turnover_notional, fill


def _discrete_wealth_ledger_returns(
    bar_index: pd.Index,
    dollar_gross_pnl: pd.Series,
    dollar_transaction_cost: pd.Series,
    initial_equity_usd: float,
) -> Tuple[pd.Series, pd.Series, pd.Series]:
    """P7B-REPAIR-02 (mission Section 3B/3C, defect A): the official
    discrete-economics stateful wealth ledger. Each row's RETURN FRACTION is
    this row's raw dollar P&L (all symbols, this timestamp) divided by the
    RUNNING equity level as of the END of the prior row -- never a FIXED
    initial-equity denominator, which corrupts the geometric compounding
    _daily_aggregate/_summarize_series already perform on every
    gross_return/net_return series (continuous or discrete alike):

        r_t = dollar_pnl_t / equity_{t-1}
        equity_t = equity_{t-1} * (1 + r_t) = equity_{t-1} + dollar_pnl_t

    Two independent running ledgers are threaded -- `equity_gross` (mark-to-
    market P&L only) and `equity_net` (P&L net of this row's execution
    costs, subtracted as an exact dollar amount -- no multiplicative
    approximation is needed here, unlike economics.compute_net_return_exact,
    because both quantities are already true dollars, not equity-fraction
    approximations) -- mirroring the existing gross_return/net_return
    dual-series convention the continuous weight-space path already uses.

    Example (mission Section 3C, no costs): qty=1000, equity=100_000,
    prices 100 -> 110 -> 121 produce dollar P&L [+10_000, +11_000], hence
    r = [0.10, 0.10] and a compounded total return of 1.10*1.10-1 = 0.21 --
    NOT dollar_pnl/100_000 fixed-denominator fractions [0.10, 0.11], which
    would wrongly geometrically compound to 0.221.

    Returns (gross_return, net_return, transaction_cost) as FRACTIONAL
    pd.Series sharing `bar_index` -- transaction_cost is re-expressed as
    `dollar_cost_t / equity_net_{t-1}` (a cost-as-fraction-of-that-row's-net-
    equity), purely for REPORTING parity with the continuous path's
    fraction-of-weight transaction_cost column; it is not used to derive
    net_return here (net_return is derived directly from exact dollars)."""
    equity_gross = float(initial_equity_usd)
    equity_net = float(initial_equity_usd)
    gross_vals: List[float] = []
    net_vals: List[float] = []
    cost_frac_vals: List[float] = []
    for t in bar_index:
        dollar_pnl = float(dollar_gross_pnl.loc[t])
        dollar_cost = float(dollar_transaction_cost.loc[t])

        if equity_gross <= 0.0:
            raise RuntimeError(
                "Fail-closed: discrete gross wealth ledger equity is <= 0 -- cannot "
                "compute a further return fraction"
            )
        r_gross = dollar_pnl / equity_gross
        gross_vals.append(r_gross)
        equity_gross = equity_gross + dollar_pnl

        if equity_net <= 0.0:
            raise RuntimeError(
                "Fail-closed: discrete net wealth ledger equity is <= 0 -- cannot "
                "compute a further return fraction"
            )
        dollar_net_pnl = dollar_pnl - dollar_cost
        r_net = dollar_net_pnl / equity_net
        net_vals.append(r_net)
        cost_frac_vals.append(dollar_cost / equity_net)
        equity_net = equity_net + dollar_net_pnl

    idx = pd.DatetimeIndex(bar_index)
    return (
        pd.Series(gross_vals, index=idx),
        pd.Series(net_vals, index=idx),
        pd.Series(cost_frac_vals, index=idx),
    )


def _simulate_fold_execution(
    close_frame: pd.DataFrame,
    pending_events: Dict[str, List[Tuple[pd.Timestamp, float, Optional[int]]]],
    max_gross_exposure: float,
    *,
    high_frame: Optional[pd.DataFrame] = None,
    low_frame: Optional[pd.DataFrame] = None,
    execution_pricing: Optional[ExecutionPricingSpec] = None,
    wts_spec: Optional[WeightToShareSpec] = None,
    commission_bps_per_side: float = 0.0,
    one_way_cost_bps: float = 0.0,
) -> Tuple[Dict[str, List[Dict[str, Any]]], Dict[str, float], Dict[str, int]]:
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

    P7B-REPAIR-01 (mission Section 4E, "discrete shares must drive
    economics"): when `wts_spec` is engaged, a PARALLEL discrete signed
    share-quantity state (`current_qty`) is threaded alongside the
    continuous `executed_weight` state, in perfect lockstep with the SAME
    admission/deferral decisions the continuous engine already makes (gross
    exposure ADMISSION stays entirely in weight space -- deliberately
    unchanged/frozen, see mission Section 4F: floor-only rounding can only
    ever reduce a given event's own notional relative to the continuous
    weight target it was sized from, so a capacity budget the continuous
    engine already proved satisfies max_gross_exposure is never exceeded by
    the discrete translation of that SAME event). Each pending event's
    `target_qty` (the 3rd tuple element) was already fixed, once, at SIGNAL
    time (see _build_pending_events) -- this function only ever reads it,
    never recomputes it from an execution-time price. Whenever the
    continuous engine actually executes a candidate (in D or in F), the
    discrete side computes `delta_qty = candidate_qty[s] - current_qty[s]`
    and prices that delta at THIS bar's P7A conservative fill (mirrors
    Rust's `targets_to_order_intents` delta convention exactly). Per-bar
    `qty_gross_contrib[s]` is earned from `current_qty[s]` HELD BEFORE this
    bar's execution (identical convention to the continuous `gross_contrib`)
    -- a target that rounds to qty=0 therefore earns exactly zero economic
    exposure regardless of `executed_weight`, closing the evidence-only gap
    (mission Section 4B/4E, REQUIRED TEST 20).

    Returns (rows_by_symbol, final_weight_by_symbol, final_qty_by_symbol).
    `rows_by_symbol[s]` has one dict per s's own bar, carrying BOTH the
    legacy continuous fields (timestamp/gross_contrib/turnover/
    interval_exposure/executed_weight/execution_price_cost/
    commission_notional, bit-for-bit unchanged when wts_spec is None) AND,
    when `wts_spec` is engaged, the discrete evidence/economics fields
    (target_qty/qty_delta/qty_side/qty_gross_contrib/qty_turnover/
    qty_execution_price_cost/qty_commission_notional).
    """
    symbols = list(close_frame.columns)
    bar_index = close_frame.index

    executed_weight: Dict[str, float] = {s: 0.0 for s in symbols}
    current_qty: Dict[str, int] = {s: 0 for s in symbols}
    pending_idx: Dict[str, int] = {s: 0 for s in symbols}
    prev_close: Dict[str, Optional[float]] = {s: None for s in symbols}
    rows_by_symbol: Dict[str, List[Dict[str, Any]]] = {s: [] for s in symbols}

    # P7B-REPAIR-02 (mission Section 3D): fill-time capacity parity state,
    # mirroring Rust's BacktestEngine -- `last_close` mirrors
    # `self.last_prices` (most recent known mark per symbol, present or
    # not); `equity_net_running` mirrors `compute_equity_micros` computed
    # from cash+positions. Both are updated once per frame (see the
    # `frame_equity_for_cap` assignment below and the frame-end update after
    # the D/F sections), not recomputed from the downstream
    # `_discrete_wealth_ledger_returns` pass -- that pass independently
    # reconstructs the SAME dollar-additive ledger from the returned rows
    # for REPORTING; this one exists only to gate admission DURING
    # simulation, exactly as Rust's own equity/exposure figures are computed
    # fresh at fill time rather than reused from a prior report.
    last_close: Dict[str, Optional[float]] = {s: None for s in symbols}
    equity_net_running = wts_spec.equity_usd if wts_spec is not None else None
    frame_equity_for_cap: Optional[float] = None

    def _current_gross_exposure_dollars() -> float:
        return sum(abs(current_qty[sym]) * (last_close[sym] or 0.0) for sym in symbols)

    def _apply_discrete_execution(s: str, t: Any) -> Tuple[float, float, float, int, Optional[str]]:
        """Executes symbol `s`'s discrete side at frame `t` against whatever
        `candidate_qty[s]` was already resolved for this frame, mutating
        `current_qty[s]` in place ONLY if admitted. Returns
        (qty_execution_price_cost, qty_commission_notional, qty_turnover,
        qty_delta, qty_side); all-zero/None no-op if wts_spec is disengaged,
        `candidate_qty[s]` is None, the resolved delta is zero, OR (P7B-
        REPAIR-02, mission Section 3D/3F) the fill-time allocation-cap check
        rejects a risk-increasing residual -- mirroring Rust's
        `resolve_one_pending_order`: only the risk-INCREASING residual of a
        (possibly reversing) delta is priced against current modeled
        equity/gross-exposure at the ACTUAL conservative fill price, never
        signal-time weight headroom. A rejected order performs no fill, no
        commission, no price drag, and no position mutation -- signal-time
        weight-space admission (frozen, unchanged) may have already deemed
        this event admissible; the discrete evidence/economics are allowed
        to diverge from the continuous baseline here by design (the same
        divergence floor-rounding already produces elsewhere in this
        translation layer)."""
        if wts_spec is None or candidate_qty.get(s) is None:
            return 0.0, 0.0, 0.0, 0, None
        target_qty = candidate_qty[s]
        assert target_qty is not None
        order = target_qty_to_order_delta(current_qty=current_qty[s], target_qty=target_qty)
        delta_qty = 0 if order is None else (order[1] if order[0] == "buy" else -order[1])
        if delta_qty == 0:
            return 0.0, 0.0, 0.0, 0, None
        price_cost, commission, turnover_notional, fill_price = _row_execution_pricing_components_discrete(
            delta_qty,
            close_frame.at[t, s],
            high_frame.at[t, s] if high_frame is not None else None,
            low_frame.at[t, s] if low_frame is not None else None,
            execution_pricing,
        )

        reducing_capacity = max(0, -current_qty[s]) if delta_qty > 0 else max(0, current_qty[s])
        reducing_qty = min(abs(delta_qty), reducing_capacity)
        residual_increasing_qty = abs(delta_qty) - reducing_qty
        if residual_increasing_qty > 0:
            assert frame_equity_for_cap is not None
            exposure = _current_gross_exposure_dollars()
            # P7B-REPAIR-03: prospective-gross parity, mirroring Rust's
            # `resolve_one_pending_order`. The reducing component closes
            # exposure that `_current_gross_exposure_dollars` already counts
            # (at the current mark -- the same convention that function
            # itself uses), so that closed slice must come OUT of the base
            # before the risk-increasing residual is added in, or a
            # reversal's own closed exposure is double-counted against
            # headroom it no longer occupies.
            closing_exposure = reducing_qty * (last_close[s] or 0.0)
            prospective_exposure = exposure - closing_exposure
            proposed_notional = residual_increasing_qty * fill_price
            allowed = frame_equity_for_cap * max_gross_exposure
            if prospective_exposure + proposed_notional > allowed + _GROSS_TOL * max(frame_equity_for_cap, 1.0):
                # Fail-closed admission: reject the WHOLE order, exactly as
                # Rust does -- no partial fill, no state mutation, no cost.
                return 0.0, 0.0, 0.0, 0, None

        current_qty[s] = target_qty
        side = None if delta_qty == 0 else ("buy" if delta_qty > 0 else "sell")
        return price_cost, commission, turnover_notional, delta_qty, side

    for t in bar_index:
        present = [s for s in symbols if pd.notna(close_frame.at[t, s])]
        if not present:
            continue

        gross_contrib: Dict[str, float] = {}
        qty_gross_contrib: Dict[str, float] = {}
        interval_exposure: Dict[str, float] = {}
        candidate: Dict[str, Optional[float]] = {}
        candidate_qty: Dict[str, Optional[int]] = {}
        candidate_ts: Dict[str, Optional[pd.Timestamp]] = {}

        for s in present:
            c = float(close_frame.at[t, s])
            gross_contrib[s] = (
                0.0 if prev_close[s] is None else executed_weight[s] * (c / prev_close[s] - 1.0)
            )
            # P7B-REPAIR-02 (mission Section 3B, defect A): RAW DOLLAR
            # mark-to-market P&L -- never pre-divided by a fixed equity_usd
            # denominator. _discrete_wealth_ledger_returns is the only place
            # a dollar amount becomes a return fraction, using the RUNNING
            # equity level (not this constant).
            qty_gross_contrib[s] = (
                0.0
                if prev_close[s] is None or wts_spec is None
                else current_qty[s] * (c - prev_close[s])
            )
            interval_exposure[s] = abs(executed_weight[s])

            events = pending_events.get(s, [])
            idx = pending_idx[s]
            while idx + 1 < len(events) and events[idx + 1][0] < t:
                idx += 1
            if idx < len(events) and events[idx][0] < t:
                candidate[s] = events[idx][1]
                candidate_qty[s] = events[idx][2]
                candidate_ts[s] = events[idx][0]
            else:
                candidate[s] = None
                candidate_qty[s] = None
                candidate_ts[s] = None
            pending_idx[s] = idx
            prev_close[s] = c
            last_close[s] = c

        # P7B-REPAIR-02 (mission Section 3D): marks are established (above)
        # before any fill-time capacity decision this frame -- mirrors
        # Rust's `resolve_pending_orders_for_batch` marking every symbol's
        # last_prices before any cap check runs. `frame_equity_for_cap` is
        # this frame's PRE-FILL equity: the running equity as of the prior
        # frame's end, plus this frame's own mark-to-market gain on
        # already-held (pre-fill) positions -- exactly what Rust's
        # `compute_equity_micros` reflects immediately after marks update
        # but before any of this frame's fills are applied.
        if wts_spec is not None:
            frame_equity_for_cap = equity_net_running + sum(
                qty_gross_contrib[s] for s in present
            )

        turnover: Dict[str, float] = {s: 0.0 for s in present}
        execution_price_cost: Dict[str, float] = {s: 0.0 for s in present}
        # REPAIR-01: commission_notional is turnover rescaled by the actual
        # fill/close notional ratio (1.0 -- i.e. unrescaled -- under the
        # diagnostic model or when no execution occurs this row); the caller
        # (_simulate_fold) multiplies this by commission_bps_per_side/10000
        # instead of charging commission against close-priced turnover.
        commission_notional: Dict[str, float] = {s: 0.0 for s in present}
        qty_turnover: Dict[str, float] = {s: 0.0 for s in present}
        qty_execution_price_cost: Dict[str, float] = {s: 0.0 for s in present}
        qty_commission_notional: Dict[str, float] = {s: 0.0 for s in present}
        qty_delta: Dict[str, int] = {s: 0 for s in present}
        qty_side: Dict[str, Optional[str]] = {s: None for s in present}

        # D: GROSS-reducing (and no-op) candidates execute unconditionally.
        # RESEARCH-LONG-SHORT-ECONOMIC-POLICY-01: classification is by GROSS
        # MAGNITUDE delta (abs(candidate) - abs(executed)), not raw signed
        # comparison -- a signed candidate can DECREASE magnitude while its
        # raw value INCREASES (e.g. executed=-0.5 -> candidate=0.2 raises
        # the raw value but reduces gross from 0.5 to 0.2), and a long<->
        # short sign flip at equal magnitude (e.g. +0.5 -> -0.5) consumes NO
        # additional capacity and must execute unconditionally too. This is
        # an exact generalization: for long-only weights (always >= 0),
        # abs(candidate)-abs(executed) == candidate-executed, so this
        # produces bit-for-bit identical classification to the pre-existing
        # long-only comparison.
        for s in present:
            if candidate[s] is None:
                continue
            if abs(candidate[s]) <= abs(executed_weight[s]) + _GROSS_TOL:
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
                (
                    qty_execution_price_cost[s],
                    qty_commission_notional[s],
                    qty_turnover[s],
                    qty_delta[s],
                    qty_side[s],
                ) = _apply_discrete_execution(s, t)

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
                candidate_qty[s] = events[idx][2]
                candidate_ts[s] = events[idx][0]
            else:
                candidate[s] = None
                candidate_qty[s] = None
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
            if candidate[s] is not None and abs(candidate[s]) > abs(executed_weight[s]) + _GROSS_TOL
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
                    (
                        qty_execution_price_cost[s],
                        qty_commission_notional[s],
                        qty_turnover[s],
                        qty_delta[s],
                        qty_side[s],
                    ) = _apply_discrete_execution(s, t)
                gross += delta
            # else: defer the entire cohort — pending_idx left untouched, so
            # every member is retried at its own next bar.

        if gross > max_gross_exposure + _GROSS_TOL:
            raise RuntimeError(
                "Fail-closed: reduce_first_defer_increase_batch_v1 allocator "
                "invariant violated — gross exposure exceeded max_gross_exposure "
                "after a frame's reductions/cohort execution"
            )

        # P7B-REPAIR-02: advance the running equity ledger to this frame's
        # end -- this frame's mark-to-market P&L (already captured in
        # frame_equity_for_cap) minus this frame's REALIZED costs, the same
        # official-vs-diagnostic cost-model selection _simulate_fold applies
        # to build the returned transaction_cost series (kept in lockstep so
        # the internal admission ledger and the downstream reported ledger
        # agree on every dollar).
        if wts_spec is not None:
            frame_dollar_cost = 0.0
            for s in present:
                if execution_pricing is not None and execution_pricing.is_official_parity_model:
                    frame_dollar_cost += (
                        qty_commission_notional[s] * (commission_bps_per_side / 10_000.0)
                        + qty_execution_price_cost[s]
                    )
                else:
                    frame_dollar_cost += qty_turnover[s] * (one_way_cost_bps / 10_000.0)
            equity_net_running = frame_equity_for_cap - frame_dollar_cost

        for s in present:
            rows_by_symbol[s].append({
                "timestamp": t,
                "gross_contrib": gross_contrib[s],
                "turnover": turnover[s],
                "interval_exposure": interval_exposure[s],
                "executed_weight": executed_weight[s],
                "execution_price_cost": execution_price_cost[s],
                "commission_notional": commission_notional[s],
                "target_qty": current_qty[s] if wts_spec is not None else None,
                # P7B-REPAIR-02 (mission Section 3D): the SIGNAL-TIME-FIXED
                # candidate quantity for this frame's eligible decision (if
                # any), distinct from `target_qty` above (the RESULTING
                # current position after this row's admit/reject decision).
                # A fill-time capacity rejection leaves `target_qty`
                # unchanged from its prior value while `signal_target_qty`
                # still truthfully reports what the signal asked for --
                # proving admission/rejection is decided at FILL time, never
                # by silently re-sizing the signal-time target.
                "signal_target_qty": candidate_qty.get(s) if wts_spec is not None else None,
                "qty_delta": qty_delta[s],
                "qty_side": qty_side[s],
                "qty_gross_contrib": qty_gross_contrib[s],
                "qty_turnover": qty_turnover[s],
                "qty_execution_price_cost": qty_execution_price_cost[s],
                "qty_commission_notional": qty_commission_notional[s],
            })

    return rows_by_symbol, dict(executed_weight), dict(current_qty)


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
    pending_events: Dict[str, List[Tuple[pd.Timestamp, float, Optional[int]]]],
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
    exception, mirroring Rust's flatten_all.

    P7B-REPAIR-01: gross_return/turnover/execution_price_cost/
    commission_notional (the actual P&L-bearing series) are sourced from the
    DISCRETE qty_* fields _simulate_fold_execution already computed whenever
    `spec.weight_to_share` is engaged -- discrete shares DRIVE the economic
    result, continuous weight-space fields remain wired for
    interval_exposure/gross_exposure/active_positions REPORTING and for the
    (frozen, unchanged) capacity ADMISSION decision only. When
    `spec.weight_to_share` is None, every field/formula below is
    bit-for-bit identical to pre-repair behavior (uses the continuous
    fields exclusively) -- existing diagnostic callers are unaffected."""
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

    # P7B (RESEARCH-WEIGHT-TO-SHARE-PARITY-01-REPAIR-01): normalized once,
    # before _simulate_fold_execution runs, so a validation error (e.g. bad
    # equity_usd) fails closed before any simulation work starts, and so the
    # discrete state machine inside _simulate_fold_execution can thread it
    # directly (signal-time sizing requires wts_spec to be available AT
    # simulation time now, not applied post-hoc).
    wts_spec = spec.weight_to_share.normalized() if spec.weight_to_share is not None else None

    rows_by_symbol, final_weight_by_symbol, final_qty_by_symbol = _simulate_fold_execution(
        close_frame,
        pending_events,
        spec.signal_policy.max_gross_exposure,
        high_frame=high_frame,
        low_frame=low_frame,
        execution_pricing=spec.execution_pricing,
        wts_spec=wts_spec,
        commission_bps_per_side=spec.cost_model.commission_bps_per_side,
        one_way_cost_bps=one_way_cost_bps,
    )

    weight_to_share_events_by_symbol: Dict[str, List[Dict[str, Any]]] = {}

    for s in symbols:
        rows = rows_by_symbol[s]
        final_weight = final_weight_by_symbol[s]
        final_qty = final_qty_by_symbol[s]

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
        #
        # P7B-REPAIR-01: the discrete flatten is gated independently on
        # `final_qty != 0` (NOT on `final_weight`) -- a tiny weight that
        # already rounded to qty=0 must exit with ZERO discrete turnover/
        # cost, even though the continuous weight-space flatten still fires
        # (REQUIRED TEST 20's zero-exposure guarantee extends to fold-end).
        if rows and rows[-1]["timestamp"] == global_last_ts:
            rows[-1] = dict(rows[-1])
            if abs(final_weight) > 1e-12:
                exit_turnover = abs(final_weight)
                rows[-1]["turnover"] = rows[-1]["turnover"] + exit_turnover
                rows[-1]["executed_weight"] = 0.0
                # Close/mark-priced exit (ratio 1.0): added to whatever
                # commission_notional this same row's own ordinary execution
                # (if any) already accrued, never blended into a single
                # rescaled ratio -- each component keeps its own true basis.
                rows[-1]["commission_notional"] = rows[-1]["commission_notional"] + exit_turnover
                final_weight = 0.0
            if wts_spec is not None and final_qty != 0:
                exit_qty_delta = -final_qty
                exit_close = float(close_frame.at[global_last_ts, s])
                exit_turnover_notional = abs(final_qty) * exit_close
                rows[-1]["qty_turnover"] = rows[-1]["qty_turnover"] + exit_turnover_notional
                rows[-1]["qty_commission_notional"] = (
                    rows[-1]["qty_commission_notional"] + exit_turnover_notional
                )
                rows[-1]["qty_delta"] = rows[-1]["qty_delta"] + exit_qty_delta
                rows[-1]["qty_side"] = "buy" if exit_qty_delta > 0 else "sell"
                rows[-1]["target_qty"] = 0
                final_qty = 0
        elif abs(final_weight) > 1e-12 or (wts_spec is not None and final_qty != 0):
            exit_row: Dict[str, Any] = {
                "timestamp": global_last_ts,
                "gross_contrib": 0.0,
                "turnover": abs(final_weight),
                "interval_exposure": abs(final_weight),
                "executed_weight": 0.0,
                "execution_price_cost": 0.0,
                "commission_notional": abs(final_weight),
                "target_qty": 0 if wts_spec is not None else None,
                "signal_target_qty": 0 if wts_spec is not None else None,
                "qty_delta": 0,
                "qty_side": None,
                "qty_gross_contrib": 0.0,
                "qty_turnover": 0.0,
                "qty_execution_price_cost": 0.0,
                "qty_commission_notional": 0.0,
            }
            if wts_spec is not None and final_qty != 0:
                exit_qty_delta = -final_qty
                exit_close = float(close_frame.at[global_last_ts, s])
                exit_turnover_notional = abs(final_qty) * exit_close
                exit_row["qty_turnover"] = exit_turnover_notional
                exit_row["qty_commission_notional"] = exit_turnover_notional
                exit_row["qty_delta"] = exit_qty_delta
                exit_row["qty_side"] = "buy" if exit_qty_delta > 0 else "sell"
                final_qty = 0
            rows.append(exit_row)
            final_weight = 0.0

        if wts_spec is not None:
            # P7B-REPAIR-01: evidence assembled directly from the causal
            # per-row discrete state _simulate_fold_execution/the fold-end
            # flatten above already computed -- NOT a post-hoc re-derivation
            # from each row's own (execution-time) close. `target_qty` was
            # fixed once, at SIGNAL time, in _build_pending_events; nothing
            # here ever recomputes it from a later bar.
            weight_to_share_events_by_symbol[s] = [
                {
                    "timestamp": r["timestamp"],
                    "target_qty": r["target_qty"],
                    "signal_target_qty": r["signal_target_qty"],
                    "side": r["qty_side"],
                    "qty": abs(r["qty_delta"]),
                }
                for r in rows
            ]

        exec_index: List[pd.Timestamp] = []
        exec_values: List[float] = []
        for r in rows:
            t = r["timestamp"]
            if wts_spec is not None:
                gross_by_ts[t] += r["qty_gross_contrib"]
                turnover_by_ts[t] += r["qty_turnover"]
                execution_price_cost_by_ts[t] += r["qty_execution_price_cost"]
                commission_notional_by_ts[t] += r["qty_commission_notional"]
            else:
                gross_by_ts[t] += r["gross_contrib"]
                turnover_by_ts[t] += r["turnover"]
                execution_price_cost_by_ts[t] += r["execution_price_cost"]
                commission_notional_by_ts[t] += r["commission_notional"]
            interval_exposure_by_ts[t] += r["interval_exposure"]
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

    if wts_spec is not None:
        # P7B-REPAIR-02 (defect A): at this point gross_return/transaction_cost
        # are RAW DOLLAR series (see _row_execution_pricing_components_discrete
        # / qty_gross_contrib above) -- economics.compute_net_return_exact
        # assumes FRACTION-of-equity units (as the continuous weight-space
        # path always supplies) and must never be called on dollars directly.
        # The stateful wealth ledger converts dollars to return fractions
        # using the RUNNING equity level, replacing gross_return/
        # transaction_cost with their fractional forms in the same step.
        gross_return, net_return, transaction_cost = _discrete_wealth_ledger_returns(
            bar_index, gross_return, transaction_cost, wts_spec.equity_usd,
        )
    else:
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
        # P7B-REPAIR-01 (mission Section 4G): structural, non-self-asserted
        # proof that discrete shares actually drove THIS fold's economics --
        # distinct from weight_to_share_protocol_id, which only asserts the
        # translation exists. A caller checking for official parity looks
        # for this marker.
        summary["discrete_economics_protocol_id"] = DISCRETE_ECONOMICS_PROTOCOL_ID_V1
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
        wts_spec_for_signal_pricing = spec.weight_to_share.normalized() if spec.weight_to_share is not None else None
        if spec.signal_policy.is_rank:
            pending_events = _build_rank_pending_events(
                oos_fold, symbols, spec.signal_policy,
                close_frame=close_frame, wts_spec=wts_spec_for_signal_pricing,
            )
        else:
            pending_events = _build_pending_events(
                oos_fold, symbols, spec.signal_policy,
                close_frame=close_frame, wts_spec=wts_spec_for_signal_pricing,
            )
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
