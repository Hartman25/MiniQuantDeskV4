"""
DIRECT-RANK-AND-BROAD-UNIVERSE-RESEARCH-01 -- two new, explicitly-versioned
cross-sectional rank direction policies (`cross_sectional_rank_long_only_v1`
/ `cross_sectional_rank_long_short_v1`) built on the frozen, unmodified
causal execution engine (_simulate_fold_execution via _simulate_fold). These
rank the persisted OOS `ml_score` -- never target/fwd_ret, never a raw
feature -- across whatever symbols were actually scored at each EXACT
decision timestamp ("DYNAMIC CROSS-SECTION V1": no fixed fold-wide universe
requirement -- a symbol's rankable membership may differ freely from one
timestamp to the next; only a genuine duplicate row or an undersized
cross-section fails closed).

Covers the mission's Patch A REQUIRED TESTS list (25 items, referenced by
number in each test's docstring). Weight/gross/net mechanics and the
dynamic-membership/no-stale-carry-forward invariant are exercised directly
against the production pending-events builder
(_build_rank_pending_events)/pure selection primitive
(_resolve_rank_direction_for_frame); execution-timing/fold-end-flatten/
gross-cap proofs are exercised end-to-end through the real public pipeline
(run_economic_walkforward), NOT via hand-crafted internal state -- mirroring
test_long_short_economic_policy.py's own approach.
"""
from __future__ import annotations

import json
from pathlib import Path
from typing import Any, Dict, List

import pandas as pd
import pytest

from mqk_research.ml.economic_walkforward import (
    BORROW_MODEL_RESEARCH_ASSUMED_SHORTABLE_UNIVERSE_V1,
    SIGNAL_DIRECTION_POLICY_CROSS_SECTIONAL_RANK_LONG_ONLY_V1,
    SIGNAL_DIRECTION_POLICY_CROSS_SECTIONAL_RANK_LONG_SHORT_V1,
    SIGNAL_SIZING_EQUAL_WEIGHT_RANK_SELECTED_V1,
    AnnualizationSpec,
    CostModelSpec,
    EconomicWalkForwardSpec,
    SignalPolicySpec,
    _build_rank_pending_events,
    _resolve_rank_direction_for_frame,
    economic_protocol_identity,
    run_economic_walkforward,
)
from mqk_research.ml.weight_to_share import WeightToShareSpec

# ---------------------------------------------------------------------------
# Shared fixture helpers (mirrors test_long_short_economic_policy.py's own
# local helpers -- no shared conftest exists in this test package today).
# ---------------------------------------------------------------------------


def _ts(s: str) -> pd.Timestamp:
    return pd.Timestamp(s, tz="UTC")


def _bar_row(symbol: str, end_ts, close: float) -> Dict[str, Any]:
    return {"symbol": symbol, "end_ts": _ts(str(end_ts)).isoformat(), "close": close}


def _oos_row(fold: int, symbol: str, decision_ts, score: float) -> Dict[str, Any]:
    return {
        "fold": fold,
        "symbol": symbol,
        "decision_ts": _ts(str(decision_ts)).isoformat(),
        "label_end_ts": _ts(str(decision_ts)).isoformat(),
        "ml_score": score,
        "target": 1,
    }


def _single_fold(test_start: str, test_end: str, fold: int = 1) -> Dict[str, Any]:
    return {
        "fold": fold,
        "skipped": False,
        "test_start_utc": _ts(test_start).isoformat(),
        "test_end_utc": _ts(test_end).isoformat(),
    }


def _write_fixture(
    run_dir: Path, *, folds: List[Dict[str, Any]], oos_rows: List[Dict[str, Any]], bars_rows: List[Dict[str, Any]]
) -> Path:
    eval_dir = run_dir / "eval"
    eval_dir.mkdir(parents=True, exist_ok=True)
    (eval_dir / "walk_forward_eval.json").write_text(json.dumps({"folds": folds}), encoding="utf-8")
    pd.DataFrame(oos_rows).to_csv(eval_dir / "walk_forward_oos_predictions.csv", index=False)
    bars_path = run_dir / "bars.csv"
    pd.DataFrame(bars_rows).to_csv(bars_path, index=False)
    return bars_path


def _diagnostic_spec(signal_policy: SignalPolicySpec, weight_to_share=None) -> EconomicWalkForwardSpec:
    return EconomicWalkForwardSpec(
        signal_policy=signal_policy,
        cost_model=CostModelSpec(commission_bps_per_side=0.0, slippage_bps_per_side=0.0, diagnostic_zero_cost=True),
        annualization=AnnualizationSpec(),
        weight_to_share=weight_to_share,
    )


def _rank_long_only(rank_side_count: int, max_gross: float = 1.0) -> SignalPolicySpec:
    return SignalPolicySpec(
        direction_policy=SIGNAL_DIRECTION_POLICY_CROSS_SECTIONAL_RANK_LONG_ONLY_V1,
        long_only=True,
        rank_side_count=rank_side_count,
        max_gross_exposure=max_gross,
    ).normalized()


def _rank_long_short(rank_side_count: int, max_gross: float = 1.0, borrow_model=None) -> SignalPolicySpec:
    return SignalPolicySpec(
        direction_policy=SIGNAL_DIRECTION_POLICY_CROSS_SECTIONAL_RANK_LONG_SHORT_V1,
        long_only=False,
        rank_side_count=rank_side_count,
        max_gross_exposure=max_gross,
        borrow_model=borrow_model,
    ).normalized()


def _direct_oos_fold(rows: List[Dict[str, Any]]) -> pd.DataFrame:
    """Constructs an oos_fold-shaped DataFrame directly (bypassing
    load_oos_predictions/run_economic_walkforward's fold-slicing) for
    testing _build_rank_pending_events / _resolve_rank_direction_for_frame
    as pure functions."""
    df = pd.DataFrame(rows)
    df["decision_ts"] = pd.to_datetime(df["decision_ts"], utc=True)
    df["symbol"] = df["symbol"].astype(str)
    df["ml_score"] = df["ml_score"].astype(float)
    return df


# ---------------------------------------------------------------------------
# REQUIRED TESTS 1-4: exact weight/gross/net mechanics
# ---------------------------------------------------------------------------


def test_rank_long_only_top_k_exact_weights() -> None:
    """REQUIRED TEST 1: top-K long-only exact weights (mission worked
    example: A=.9 B=.8 C=.7 D=.6 E=.5 F=.4, K=2, gross=1.0 -> A/B +0.5,
    others 0)."""
    ts = _ts("2021-01-01")
    oos_fold = _direct_oos_fold(
        [
            _oos_row(1, "A", ts, 0.9),
            _oos_row(1, "B", ts, 0.8),
            _oos_row(1, "C", ts, 0.7),
            _oos_row(1, "D", ts, 0.6),
            _oos_row(1, "E", ts, 0.5),
            _oos_row(1, "F", ts, 0.4),
        ]
    )
    symbols = ["A", "B", "C", "D", "E", "F"]
    events = _build_rank_pending_events(oos_fold, symbols, _rank_long_only(2, max_gross=1.0))
    assert [w for (_, w, _) in events["A"]] == [0.5]
    assert [w for (_, w, _) in events["B"]] == [0.5]
    for s in ("C", "D", "E", "F"):
        assert events[s] == []


def test_rank_long_short_top_bottom_k_exact_weights() -> None:
    """REQUIRED TEST 2: top/bottom-K long/short exact weights (mission
    worked example: same scores, K=2 -> A/B +0.25, E/F -0.25, C/D 0)."""
    ts = _ts("2021-01-01")
    oos_fold = _direct_oos_fold(
        [
            _oos_row(1, "A", ts, 0.9),
            _oos_row(1, "B", ts, 0.8),
            _oos_row(1, "C", ts, 0.7),
            _oos_row(1, "D", ts, 0.6),
            _oos_row(1, "E", ts, 0.5),
            _oos_row(1, "F", ts, 0.4),
        ]
    )
    symbols = ["A", "B", "C", "D", "E", "F"]
    events = _build_rank_pending_events(oos_fold, symbols, _rank_long_short(2, max_gross=1.0))
    assert [w for (_, w, _) in events["A"]] == [0.25]
    assert [w for (_, w, _) in events["B"]] == [0.25]
    assert [w for (_, w, _) in events["E"]] == [-0.25]
    assert [w for (_, w, _) in events["F"]] == [-0.25]
    for s in ("C", "D"):
        assert events[s] == []


def test_gross_uses_abs_weights_not_signed_sum() -> None:
    """REQUIRED TEST 3: gross = sum(abs(weight)), not the signed sum."""
    ts = _ts("2021-01-01")
    oos_fold = _direct_oos_fold(
        [_oos_row(1, s, ts, score) for s, score in [("A", 0.9), ("B", 0.8), ("E", 0.5), ("F", 0.4)]]
    )
    events = _build_rank_pending_events(oos_fold, ["A", "B", "E", "F"], _rank_long_short(2, max_gross=1.0))
    weights = [w for sym in ("A", "B", "E", "F") for (_, w, _) in events[sym]]
    assert sum(abs(w) for w in weights) == pytest.approx(1.0)


def test_desired_long_short_net_is_zero() -> None:
    """REQUIRED TEST 4: desired long/short net exposure is exactly 0.0
    (before asynchronous execution effects) -- long-only's net is +gross,
    checked separately."""
    ts = _ts("2021-01-01")
    oos_fold = _direct_oos_fold(
        [_oos_row(1, s, ts, score) for s, score in [("A", 0.9), ("B", 0.8), ("E", 0.5), ("F", 0.4)]]
    )
    events = _build_rank_pending_events(oos_fold, ["A", "B", "E", "F"], _rank_long_short(2, max_gross=1.0))
    weights = [w for sym in ("A", "B", "E", "F") for (_, w, _) in events[sym]]
    assert sum(weights) == pytest.approx(0.0)


# ---------------------------------------------------------------------------
# REQUIRED TESTS 5-7: dynamic per-timestamp cross-section
# ---------------------------------------------------------------------------


def test_rank_only_within_exact_timestamp() -> None:
    """REQUIRED TEST 5: two decision timestamps with different score
    orderings each rank strictly on their own row set."""
    t1, t2 = _ts("2021-01-01"), _ts("2021-01-02")
    oos_fold = _direct_oos_fold(
        [
            _oos_row(1, "A", t1, 0.9), _oos_row(1, "B", t1, 0.5),
            _oos_row(1, "A", t2, 0.2), _oos_row(1, "B", t2, 0.8),
        ]
    )
    events = _build_rank_pending_events(oos_fold, ["A", "B"], _rank_long_only(1, max_gross=1.0))
    a_events = {ts_: w for ts_, w, _ in events["A"]}
    b_events = {ts_: w for ts_, w, _ in events["B"]}
    assert a_events[pd.Timestamp(t1)] == pytest.approx(1.0)  # A selected at t1
    assert b_events[pd.Timestamp(t2)] == pytest.approx(1.0)  # B selected at t2
    assert a_events.get(pd.Timestamp(t2)) == pytest.approx(0.0)  # A demoted at t2


def test_stale_prior_timestamp_score_never_carried_forward() -> None:
    """REQUIRED TESTS 6/7 (mission "DYNAMIC MEMBERSHIP TEST") + R1-A
    (DIRECT-RANK-DYNAMIC-MEMBERSHIP-POSITION-CLOSURE-01, "LONG-ONLY
    DROPOUT"): at T1 the scored set is {F,A,B,C,D,E} (F highest, selected
    top-2 with A); at T2 F is entirely ABSENT and G newly appears with a LOW
    score. The correct T2 selection is {A,B} -- if F's stale T1 score of
    0.95 were incorrectly still considered "in the running" at T2, F (not B)
    would remain selected. A missing T2 row for F is not an engine error
    (dynamic membership is allowed) and F's stale score never re-enters
    T2's ranking input -- but F's ECONOMIC POSITION must not merely be
    excluded from ranking, it must be explicitly FLATTENED: every valid
    decision frame defines the COMPLETE desired direction state, so a
    symbol absent from the current rankable set desires flat (0), exactly
    like a scored-but-unselected symbol. The pre-R1 implementation looped
    only `new_direction.items()` (T2's scored symbols) and so silently
    preserved F's stale +0.5 direction_state forever -- this test's T2
    assertion on F is the regression proof for that fix."""
    t1, t2 = _ts("2021-01-01"), _ts("2021-01-02")
    oos_fold = _direct_oos_fold(
        [
            _oos_row(1, "F", t1, 0.95), _oos_row(1, "A", t1, 0.90), _oos_row(1, "B", t1, 0.85),
            _oos_row(1, "C", t1, 0.70), _oos_row(1, "D", t1, 0.60), _oos_row(1, "E", t1, 0.50),
            _oos_row(1, "A", t2, 0.90), _oos_row(1, "B", t2, 0.85), _oos_row(1, "C", t2, 0.70),
            _oos_row(1, "D", t2, 0.60), _oos_row(1, "E", t2, 0.50), _oos_row(1, "G", t2, 0.30),
        ]
    )
    symbols = ["A", "B", "C", "D", "E", "F", "G"]
    events = _build_rank_pending_events(oos_fold, symbols, _rank_long_only(2, max_gross=1.0))

    # T1: F, A selected (top-2 of F=.95,A=.90,...).
    assert [ts_ for ts_, w, _ in events["A"]] == [pd.Timestamp(t1)]

    # T2: correct ranking of {A,B,C,D,E,G} ONLY selects A,B -- G (0.30) is
    # too low. F is absent from T2's scored rows, but MUST receive an
    # explicit zero (flatten) event at T2 -- absence from the rankable set
    # is not a reason to keep a stale nonzero position.
    assert [(ts_, w) for ts_, w, _ in events["F"]] == [
        (pd.Timestamp(t1), pytest.approx(0.5)),
        (pd.Timestamp(t2), pytest.approx(0.0)),
    ]
    assert [ts_ for ts_, w, _ in events["G"]] == []  # G scored too low, never selected
    b_events = {ts_: w for ts_, w, _ in events["B"]}
    assert b_events[pd.Timestamp(t2)] == pytest.approx(0.5)  # B newly selected at T2
    # A stays selected across T1->T2 (no direction change -> no new event).
    assert [ts_ for ts_, w, _ in events["A"]] == [pd.Timestamp(t1)]


# ---------------------------------------------------------------------------
# R1 (DIRECT-RANK-DYNAMIC-MEMBERSHIP-POSITION-CLOSURE-01): a symbol absent
# from the current rankable cross-section must be explicitly flattened, not
# left silently holding its prior direction_state. R1-A is the fixed
# test_stale_prior_timestamp_score_never_carried_forward above; R1-B..R1-F
# follow.
# ---------------------------------------------------------------------------


def test_r1b_end_to_end_economic_position_after_dropout(tmp_path: Path) -> None:
    """R1-B END-TO-END ECONOMIC POSITION: drives the real economic
    walkforward/execution path (not the pure builder) across a dropout.
    T1 (day0): F=.95,A=.90,B=.85,C=.70,D=.60,E=.50, K=2 -> F,A selected.
    T2 (day2): A=.90,B=.85,C=.70,D=.60,E=.50,G=.30, F ABSENT -> A,B selected.
    Proves actual executed holdings after T2's execution bar correspond to
    exactly {A,B}, not stale {F,A} -- F's own bars continue for the whole
    fold (as they must, to price its flatten trade and because
    close_frame's symbol universe spans every symbol ever scored in the
    fold), but F must never again carry a nonzero executed weight once its
    flatten event has had a chance to execute."""
    days = pd.date_range("2021-01-01", periods=5, freq="D", tz="UTC")
    symbols_all = ["A", "B", "C", "D", "E", "F", "G"]
    bars_rows = [_bar_row(s, d, 100.0) for s in symbols_all for d in days]
    oos_rows = (
        [_oos_row(1, s, days[0], score) for s, score in
         [("F", 0.95), ("A", 0.90), ("B", 0.85), ("C", 0.70), ("D", 0.60), ("E", 0.50)]]
        + [_oos_row(1, s, days[2], score) for s, score in
           [("A", 0.90), ("B", 0.85), ("C", 0.70), ("D", 0.60), ("E", 0.50), ("G", 0.30)]]
    )
    folds = [_single_fold("2021-01-01", "2021-01-06")]
    bars_path = _write_fixture(tmp_path, folds=folds, oos_rows=oos_rows, bars_rows=bars_rows)

    spec = _diagnostic_spec(_rank_long_only(2, max_gross=1.0), weight_to_share=WeightToShareSpec(equity_usd=10_000.0))
    out = json.loads(
        run_economic_walkforward(tmp_path, bars_csv=bars_path, spec=spec).read_text(encoding="utf-8")
    )
    evidence = out["folds"][0]["weight_to_share_evidence"]

    # T2's flatten (F) and buy (B) signals are issued at day2 (idx2) and
    # execute at day3 (idx3) -- one bar later, per the causal same-bar-
    # cannot-execute contract already proven by REQUIRED TESTS 20/21.
    assert evidence["F"][3]["side"] == "sell"
    assert evidence["F"][3]["target_qty"] == 0  # F flattened, not held stale
    assert evidence["B"][3]["side"] == "buy"
    assert evidence["B"][3]["target_qty"] == 50  # B newly holds its exact K-share

    # After T2's execution bar, A remains held (never dropped) and F is
    # exactly flat -- selected names are exactly the current top-K {A,B}.
    assert evidence["A"][3]["target_qty"] == 50
    assert evidence["F"][3]["target_qty"] == 0
    for s in ("C", "D", "E", "G"):
        assert evidence[s][3]["target_qty"] == 0  # never selected

    returns_csv = pd.read_csv(tmp_path / "eval" / "economic_returns.csv")
    assert returns_csv["gross_exposure"].max() <= 1.0 + 1e-6  # max_gross_exposure preserved


def test_r1c_long_short_dropout_flattens_both_sides() -> None:
    """R1-C LONG/SHORT DROPOUT: at T1 A is the sole selected long and F the
    sole selected short (K=1); at T2 both A and F disappear from the
    rankable set while D/E newly take the long/short slots. Required: both
    the dropped long AND the dropped short flatten to 0, and the new
    top/bottom members take their intended side."""
    t1, t2 = _ts("2021-01-01"), _ts("2021-01-02")
    oos_fold = _direct_oos_fold(
        [
            _oos_row(1, "A", t1, 0.95), _oos_row(1, "B", t1, 0.80),
            _oos_row(1, "C", t1, 0.20), _oos_row(1, "F", t1, 0.05),
            _oos_row(1, "B", t2, 0.60), _oos_row(1, "C", t2, 0.40),
            _oos_row(1, "D", t2, 0.90), _oos_row(1, "E", t2, 0.05),
        ]
    )
    symbols = ["A", "B", "C", "D", "E", "F"]
    events = _build_rank_pending_events(oos_fold, symbols, _rank_long_short(1, max_gross=1.0))

    assert [(ts_, pytest.approx(w)) for ts_, w, _ in events["A"]] == [
        (pd.Timestamp(t1), pytest.approx(0.5)), (pd.Timestamp(t2), pytest.approx(0.0)),
    ]
    assert [(ts_, pytest.approx(w)) for ts_, w, _ in events["F"]] == [
        (pd.Timestamp(t1), pytest.approx(-0.5)), (pd.Timestamp(t2), pytest.approx(0.0)),
    ]
    assert [(ts_, w) for ts_, w, _ in events["D"]] == [(pd.Timestamp(t2), pytest.approx(0.5))]
    assert [(ts_, w) for ts_, w, _ in events["E"]] == [(pd.Timestamp(t2), pytest.approx(-0.5))]
    assert events["B"] == []  # never in the top/bottom-1 at either frame
    assert events["C"] == []


def test_r1d_partial_universe_ranks_and_flattens_dropped() -> None:
    """R1-D PARTIAL UNIVERSE: T2's rankable cross-section (4 names) is
    smaller than T1's (6 names) but still >= K=2 -- ranking must succeed on
    the smaller current frame, flatten the two names that dropped out
    (F, E), and select exactly the current top-2 (A, B)."""
    t1, t2 = _ts("2021-01-01"), _ts("2021-01-02")
    oos_fold = _direct_oos_fold(
        [
            _oos_row(1, "F", t1, 0.95), _oos_row(1, "A", t1, 0.90), _oos_row(1, "B", t1, 0.85),
            _oos_row(1, "C", t1, 0.70), _oos_row(1, "D", t1, 0.60), _oos_row(1, "E", t1, 0.50),
            _oos_row(1, "A", t2, 0.90), _oos_row(1, "B", t2, 0.85),
            _oos_row(1, "C", t2, 0.70), _oos_row(1, "D", t2, 0.60),
        ]
    )
    symbols = ["A", "B", "C", "D", "E", "F"]
    events = _build_rank_pending_events(oos_fold, symbols, _rank_long_only(2, max_gross=1.0))

    assert [(ts_, w) for ts_, w, _ in events["F"]] == [
        (pd.Timestamp(t1), pytest.approx(0.5)), (pd.Timestamp(t2), pytest.approx(0.0)),
    ]
    b_events = {ts_: w for ts_, w, _ in events["B"]}
    assert b_events[pd.Timestamp(t2)] == pytest.approx(0.5)
    assert [ts_ for ts_, w, _ in events["A"]] == [pd.Timestamp(t1)]  # unchanged, stays selected
    assert events["E"] == []  # never selected at either frame
    assert events["C"] == [] and events["D"] == []


def test_r1e_insufficient_current_frame_fails_closed() -> None:
    """R1-E INSUFFICIENT CURRENT FRAME: T1 has enough names, but T2's
    cross-section drops below rank_side_count -- must fail closed exactly
    as an ordinary undersized frame would (REQUIRED TEST 9), never silently
    fall back to retaining T1's stale selection as a substitute decision."""
    t1, t2 = _ts("2021-01-01"), _ts("2021-01-02")
    oos_fold = _direct_oos_fold(
        [
            _oos_row(1, "A", t1, 0.90), _oos_row(1, "B", t1, 0.80), _oos_row(1, "C", t1, 0.50),
            _oos_row(1, "A", t2, 0.90),
        ]
    )
    symbols = ["A", "B", "C"]
    with pytest.raises(RuntimeError, match="at least"):
        _build_rank_pending_events(oos_fold, symbols, _rank_long_only(2, max_gross=1.0))


def test_r1f_removed_held_symbol_score_neither_ranks_nor_stays_selected() -> None:
    """R1-F NO STALE SCORE / NO STALE POSITION (mutation-style proof):
    remove an already-held symbol's (F) T2 ml_score entirely. Required,
    both must hold: (1) F's T1 score of 0.95 never participates in T2's
    ranking decision -- structurally guaranteed since
    _resolve_rank_direction_for_frame only ever receives the exact rows
    scored at T2 (proven by G, scored .30 at T2, correctly losing to A/B
    despite F's absent-but-higher stale score); and (2) F's T1 position
    does not remain selected at T2 -- the pre-R1 implementation FAILS this
    exact assertion (it emits no T2 event for F at all, so F's
    direction_state silently stays +0.5 forever)."""
    t1, t2 = _ts("2021-01-01"), _ts("2021-01-02")
    oos_fold = _direct_oos_fold(
        [
            _oos_row(1, "F", t1, 0.95), _oos_row(1, "A", t1, 0.90), _oos_row(1, "B", t1, 0.85),
            _oos_row(1, "C", t1, 0.70),
            # F's T2 row is entirely removed here.
            _oos_row(1, "A", t2, 0.90), _oos_row(1, "B", t2, 0.85),
            _oos_row(1, "C", t2, 0.70), _oos_row(1, "G", t2, 0.30),
        ]
    )
    symbols = ["A", "B", "C", "F", "G"]
    events = _build_rank_pending_events(oos_fold, symbols, _rank_long_only(2, max_gross=1.0))

    # (1) F's stale score cannot outrank G's genuine T2 score of .30 -- G is
    # correctly excluded (top-2 of A=.90,B=.85,C=.70,G=.30 is A,B).
    assert events["G"] == []
    # (2) F's T1 position is explicitly flattened at T2, not silently kept.
    assert [(ts_, w) for ts_, w, _ in events["F"]] == [
        (pd.Timestamp(t1), pytest.approx(0.5)), (pd.Timestamp(t2), pytest.approx(0.0)),
    ]


# ---------------------------------------------------------------------------
# REQUIRED TESTS 8-12: fail-closed negative controls on the pure selector
# ---------------------------------------------------------------------------


def test_duplicate_symbol_at_one_timestamp_fails_closed() -> None:
    """REQUIRED TEST 8."""
    ts = _ts("2021-01-01")
    oos_fold = _direct_oos_fold(
        [_oos_row(1, "A", ts, 0.9), _oos_row(1, "A", ts, 0.8), _oos_row(1, "B", ts, 0.5)]
    )
    with pytest.raises(RuntimeError, match="duplicate"):
        _build_rank_pending_events(oos_fold, ["A", "B"], _rank_long_only(1, max_gross=1.0))


def test_insufficient_names_fails_closed() -> None:
    """REQUIRED TEST 9."""
    with pytest.raises(RuntimeError, match="at least"):
        _resolve_rank_direction_for_frame({"A": 0.9, "B": 0.8}, rank_side_count=3, long_only=True)
    with pytest.raises(RuntimeError, match="at least"):
        _resolve_rank_direction_for_frame(
            {"A": 0.9, "B": 0.8, "C": 0.5}, rank_side_count=2, long_only=False
        )  # needs 2*K=4


def test_long_boundary_tie_fails_closed() -> None:
    """REQUIRED TEST 10: K=2, B and C tie at the K/K+1 boundary."""
    with pytest.raises(RuntimeError, match="boundary tie"):
        _resolve_rank_direction_for_frame(
            {"A": 0.9, "B": 0.8, "C": 0.8, "D": 0.7}, rank_side_count=2, long_only=True
        )


def test_short_boundary_tie_fails_closed() -> None:
    """REQUIRED TEST 11: K=1, C and D tie at the bottom boundary."""
    with pytest.raises(RuntimeError, match="boundary tie"):
        _resolve_rank_direction_for_frame(
            {"A": 0.9, "B": 0.5, "C": 0.3, "D": 0.3}, rank_side_count=1, long_only=False
        )


def test_long_short_overlap_impossible_given_size_gate() -> None:
    """REQUIRED TEST 12: the `n >= 2*K` size gate makes long/short set
    overlap structurally impossible (top-K = indices [0,K), bottom-K =
    indices [n-K,n) are always disjoint index ranges once n>=2K) -- proven
    here across several (n,K) combinations. The selection code additionally
    asserts this explicitly and fails closed if it were ever violated (see
    _resolve_rank_direction_for_frame), but that branch is unreachable
    through this function's own public contract given the gate above."""
    import random

    rng = random.Random(7)
    for n, k in [(4, 2), (6, 3), (10, 2), (11, 5), (20, 4)]:
        scores = {f"S{i}": rng.random() for i in range(n)}
        # Guarantee no accidental ties among random floats at this size.
        assert len(set(scores.values())) == n
        direction = _resolve_rank_direction_for_frame(scores, rank_side_count=k, long_only=False)
        longs = {s for s, d in direction.items() if d == 1}
        shorts = {s for s, d in direction.items() if d == -1}
        assert not (longs & shorts)
        assert len(longs) == k
        assert len(shorts) == k


# ---------------------------------------------------------------------------
# REQUIRED TESTS 13-18: SignalPolicySpec.normalized() fail-closed contract
# ---------------------------------------------------------------------------


def test_rank_side_count_non_positive_rejected() -> None:
    """REQUIRED TEST 13."""
    with pytest.raises(ValueError, match="positive"):
        SignalPolicySpec(
            direction_policy=SIGNAL_DIRECTION_POLICY_CROSS_SECTIONAL_RANK_LONG_ONLY_V1,
            long_only=True, rank_side_count=0,
        ).normalized()
    with pytest.raises(ValueError, match="positive"):
        SignalPolicySpec(
            direction_policy=SIGNAL_DIRECTION_POLICY_CROSS_SECTIONAL_RANK_LONG_ONLY_V1,
            long_only=True, rank_side_count=-1,
        ).normalized()


def test_rank_side_count_valid_integers_accepted() -> None:
    """R2 (DIRECT-RANK-SIDE-COUNT-STRICT-INTEGER-01): plain positive ints,
    and floats that are exactly integral (the only numeric shape JSON
    config can use to represent an int, e.g. `2.0`), are accepted and
    normalize to the same int K."""
    for value in (2, 2.0):
        spec = SignalPolicySpec(
            direction_policy=SIGNAL_DIRECTION_POLICY_CROSS_SECTIONAL_RANK_LONG_ONLY_V1,
            long_only=True, rank_side_count=value,
        ).normalized()
        assert spec.rank_side_count == 2
        assert isinstance(spec.rank_side_count, int)
    spec1 = SignalPolicySpec(
        direction_policy=SIGNAL_DIRECTION_POLICY_CROSS_SECTIONAL_RANK_LONG_ONLY_V1,
        long_only=True, rank_side_count=1,
    ).normalized()
    assert spec1.rank_side_count == 1


def test_rank_side_count_valid_k_identity_unchanged() -> None:
    """R2: an existing valid integer K's registered identity fragment is
    byte-for-byte unchanged by the strict-integer repair -- the semantic
    value (int 2) is what identity reads, not the exact literal type used
    to construct it (2 vs 2.0 are the same K, JSON cannot tell them apart)."""
    spec_int = SignalPolicySpec(
        direction_policy=SIGNAL_DIRECTION_POLICY_CROSS_SECTIONAL_RANK_LONG_ONLY_V1,
        long_only=True, rank_side_count=2,
    ).normalized()
    spec_float = SignalPolicySpec(
        direction_policy=SIGNAL_DIRECTION_POLICY_CROSS_SECTIONAL_RANK_LONG_ONLY_V1,
        long_only=True, rank_side_count=2.0,
    ).normalized()
    identity_int = economic_protocol_identity(_diagnostic_spec(spec_int).normalized())
    identity_float = economic_protocol_identity(_diagnostic_spec(spec_float).normalized())
    assert identity_int == identity_float
    assert identity_int["signal_policy"]["rank_side_count"] == 2


@pytest.mark.parametrize(
    "bad_value",
    [0, -1, 2.5, 1.1, True, False, float("nan"), float("inf"), float("-inf"), "2", None, "2.5"],
    ids=["zero", "neg_one", "two_point_five", "one_point_one", "bool_true", "bool_false",
         "nan", "inf", "neg_inf", "str_two", "none", "str_two_point_five"],
)
def test_rank_side_count_malformed_values_rejected(bad_value) -> None:
    """R2: every malformed candidate is REJECTED outright -- never silently
    canonicalized/truncated into a different, valid K. `bad_value=None` is
    covered by the pre-existing dedicated "requires rank_side_count" check
    (REQUIRED TEST distinct message); every other value is a genuinely
    constructible-but-invalid K rejected by the new strict-integer
    contract."""
    kwargs: Dict[str, Any] = dict(
        direction_policy=SIGNAL_DIRECTION_POLICY_CROSS_SECTIONAL_RANK_LONG_ONLY_V1,
        long_only=True, rank_side_count=bad_value,
    )
    with pytest.raises(ValueError):
        SignalPolicySpec(**kwargs).normalized()


def test_rank_side_count_fractional_cannot_alias_valid_integer_identity() -> None:
    """R2: 2.5 must be REJECTED, not silently truncated to K=2 -- a
    malformed 2.5 candidate can never be constructed at all, so it is
    structurally impossible for it to alias K=2's registered identity."""
    with pytest.raises(ValueError, match="integral"):
        SignalPolicySpec(
            direction_policy=SIGNAL_DIRECTION_POLICY_CROSS_SECTIONAL_RANK_LONG_ONLY_V1,
            long_only=True, rank_side_count=2.5,
        ).normalized()
    # The valid neighbor K=2 remains constructible and unaffected.
    ok = SignalPolicySpec(
        direction_policy=SIGNAL_DIRECTION_POLICY_CROSS_SECTIONAL_RANK_LONG_ONLY_V1,
        long_only=True, rank_side_count=2,
    ).normalized()
    assert ok.rank_side_count == 2


def test_wrong_long_only_flag_rejected() -> None:
    """REQUIRED TEST 14."""
    with pytest.raises(ValueError, match="requires long_only"):
        SignalPolicySpec(
            direction_policy=SIGNAL_DIRECTION_POLICY_CROSS_SECTIONAL_RANK_LONG_ONLY_V1,
            long_only=False, rank_side_count=2,
        ).normalized()
    with pytest.raises(ValueError, match="requires long_only"):
        SignalPolicySpec(
            direction_policy=SIGNAL_DIRECTION_POLICY_CROSS_SECTIONAL_RANK_LONG_SHORT_V1,
            long_only=True, rank_side_count=2,
        ).normalized()


def test_rank_long_only_rejects_borrow_model() -> None:
    """REQUIRED TEST 15."""
    with pytest.raises(ValueError, match="does not accept borrow_model"):
        SignalPolicySpec(
            direction_policy=SIGNAL_DIRECTION_POLICY_CROSS_SECTIONAL_RANK_LONG_ONLY_V1,
            long_only=True, rank_side_count=2,
            borrow_model=BORROW_MODEL_RESEARCH_ASSUMED_SHORTABLE_UNIVERSE_V1,
        ).normalized()


def test_rank_long_short_carries_research_borrow_model() -> None:
    """REQUIRED TEST 16."""
    spec = SignalPolicySpec(
        direction_policy=SIGNAL_DIRECTION_POLICY_CROSS_SECTIONAL_RANK_LONG_SHORT_V1,
        long_only=False, rank_side_count=2,
    ).normalized()
    assert spec.borrow_model == BORROW_MODEL_RESEARCH_ASSUMED_SHORTABLE_UNIVERSE_V1
    with pytest.raises(ValueError, match="unsupported borrow_model"):
        SignalPolicySpec(
            direction_policy=SIGNAL_DIRECTION_POLICY_CROSS_SECTIONAL_RANK_LONG_SHORT_V1,
            long_only=False, rank_side_count=2, borrow_model="not_a_real_borrow_model",
        ).normalized()


def test_rank_policy_rejects_meaningful_short_threshold() -> None:
    """REQUIRED TEST 17."""
    with pytest.raises(ValueError, match="does not accept short_threshold"):
        SignalPolicySpec(
            direction_policy=SIGNAL_DIRECTION_POLICY_CROSS_SECTIONAL_RANK_LONG_ONLY_V1,
            long_only=True, rank_side_count=2, short_threshold=0.3,
        ).normalized()
    with pytest.raises(ValueError, match="does not accept short_threshold"):
        SignalPolicySpec(
            direction_policy=SIGNAL_DIRECTION_POLICY_CROSS_SECTIONAL_RANK_LONG_SHORT_V1,
            long_only=False, rank_side_count=2, short_threshold=0.3,
        ).normalized()


def test_unused_threshold_cannot_manufacture_rank_candidates() -> None:
    """REQUIRED TEST 18: entry_threshold is meaningless for rank policies;
    the only accepted value is the canonical default 0.5 -- any other value
    is REJECTED outright (never silently canonicalized), so it is
    structurally impossible for two rank specs differing only by
    entry_threshold to both succeed and manufacture distinct trial
    identities."""
    ok = SignalPolicySpec(
        direction_policy=SIGNAL_DIRECTION_POLICY_CROSS_SECTIONAL_RANK_LONG_ONLY_V1,
        long_only=True, rank_side_count=2, entry_threshold=0.5,
    ).normalized()
    assert ok.entry_threshold == 0.5
    with pytest.raises(ValueError, match="entry_threshold=0.5"):
        SignalPolicySpec(
            direction_policy=SIGNAL_DIRECTION_POLICY_CROSS_SECTIONAL_RANK_LONG_ONLY_V1,
            long_only=True, rank_side_count=2, entry_threshold=0.6,
        ).normalized()


# ---------------------------------------------------------------------------
# REQUIRED TEST 19: row-permutation invariance
# ---------------------------------------------------------------------------


def test_row_permutation_does_not_change_rank_result() -> None:
    """REQUIRED TEST 19: shuffling the physical CSV/DataFrame row order of
    an otherwise-identical oos_fold must not change the computed pending
    events."""
    ts = _ts("2021-01-01")
    rows = [
        _oos_row(1, "A", ts, 0.9), _oos_row(1, "B", ts, 0.8), _oos_row(1, "C", ts, 0.7),
        _oos_row(1, "D", ts, 0.6), _oos_row(1, "E", ts, 0.5), _oos_row(1, "F", ts, 0.4),
    ]
    symbols = ["A", "B", "C", "D", "E", "F"]
    forward = _build_rank_pending_events(_direct_oos_fold(rows), symbols, _rank_long_short(2, max_gross=1.0))
    shuffled = list(reversed(rows))
    shuffled.insert(2, shuffled.pop(0))
    backward = _build_rank_pending_events(_direct_oos_fold(shuffled), symbols, _rank_long_short(2, max_gross=1.0))
    for s in symbols:
        assert forward[s] == backward[s]


# ---------------------------------------------------------------------------
# REQUIRED TESTS 20-23: causal execution timing / fold-end / gross cap
# (end-to-end through the real, unmodified pipeline)
# ---------------------------------------------------------------------------


def test_signal_cannot_execute_on_its_own_bar_and_executes_next_bar(tmp_path: Path) -> None:
    """REQUIRED TESTS 20/21: a persistently top-ranked symbol's FIRST bar
    (the signal bar itself) cannot execute the new target -- execution
    happens only at that symbol's own NEXT bar, exactly like the legacy
    threshold policy's causal contract."""
    days = pd.date_range("2021-01-01", periods=4, freq="D", tz="UTC")
    bars_rows = (
        [_bar_row("A", d, 100.0) for d in days]
        + [_bar_row("B", d, 100.0) for d in days]
        + [_bar_row("C", d, 100.0) for d in days]
    )
    oos_rows = (
        [_oos_row(1, "A", d, 0.9) for d in days]
        + [_oos_row(1, "B", d, 0.5) for d in days]
        + [_oos_row(1, "C", d, 0.4) for d in days]
    )
    folds = [_single_fold("2021-01-01", "2021-01-05")]
    bars_path = _write_fixture(tmp_path, folds=folds, oos_rows=oos_rows, bars_rows=bars_rows)

    spec = _diagnostic_spec(_rank_long_only(1, max_gross=1.0), weight_to_share=WeightToShareSpec(equity_usd=10_000.0))
    out = json.loads(
        run_economic_walkforward(tmp_path, bars_csv=bars_path, spec=spec).read_text(encoding="utf-8")
    )
    evidence = out["folds"][0]["weight_to_share_evidence"]["A"]
    assert evidence[0]["target_qty"] == 0  # signal row (day1): cannot execute from its own bar
    assert evidence[1]["side"] == "buy" and evidence[1]["target_qty"] == 100  # day2 execution


def test_fold_end_flatten_preserved(tmp_path: Path) -> None:
    """REQUIRED TEST 22."""
    days = pd.date_range("2021-01-01", periods=4, freq="D", tz="UTC")
    bars_rows = [_bar_row("A", d, 100.0) for d in days] + [_bar_row("B", d, 100.0) for d in days]
    oos_rows = [_oos_row(1, "A", d, 0.9) for d in days] + [_oos_row(1, "B", d, 0.4) for d in days]
    folds = [_single_fold("2021-01-01", "2021-01-05")]
    bars_path = _write_fixture(tmp_path, folds=folds, oos_rows=oos_rows, bars_rows=bars_rows)

    spec = _diagnostic_spec(_rank_long_only(1, max_gross=1.0), weight_to_share=WeightToShareSpec(equity_usd=10_000.0))
    out = json.loads(
        run_economic_walkforward(tmp_path, bars_csv=bars_path, spec=spec).read_text(encoding="utf-8")
    )
    evidence = out["folds"][0]["weight_to_share_evidence"]["A"]
    assert evidence[-1]["target_qty"] == 0


def test_actual_gross_never_exceeds_max_gross_exposure(tmp_path: Path) -> None:
    """REQUIRED TEST 23."""
    days = pd.date_range("2021-01-01", periods=4, freq="D", tz="UTC")
    bars_rows = (
        [_bar_row("A", d, 100.0) for d in days]
        + [_bar_row("B", d, 100.0) for d in days]
        + [_bar_row("E", d, 100.0) for d in days]
        + [_bar_row("F", d, 100.0) for d in days]
    )
    oos_rows = (
        [_oos_row(1, "A", d, 0.9) for d in days]
        + [_oos_row(1, "B", d, 0.8) for d in days]
        + [_oos_row(1, "E", d, 0.5) for d in days]
        + [_oos_row(1, "F", d, 0.4) for d in days]
    )
    folds = [_single_fold("2021-01-01", "2021-01-05")]
    bars_path = _write_fixture(tmp_path, folds=folds, oos_rows=oos_rows, bars_rows=bars_rows)

    spec = _diagnostic_spec(_rank_long_short(2, max_gross=1.0))
    run_economic_walkforward(tmp_path, bars_csv=bars_path, spec=spec)
    returns_csv = pd.read_csv(tmp_path / "eval" / "economic_returns.csv")
    assert returns_csv["gross_exposure"].max() <= 1.0 + 1e-6


# ---------------------------------------------------------------------------
# REQUIRED TESTS 24/25: legacy non-regression (spot checks -- the primary
# proof is running test_long_short_economic_policy.py/
# test_economic_walkforward.py unmodified, per mission validation boundary)
# ---------------------------------------------------------------------------


def test_legacy_long_only_v1_identity_and_defaults_unchanged() -> None:
    """REQUIRED TEST 24."""
    spec = SignalPolicySpec(entry_threshold=0.5).normalized()
    assert spec.sizing == "equal_weight_active"
    assert spec.rank_side_count is None
    identity = economic_protocol_identity(_diagnostic_spec(spec).normalized())
    assert set(identity["signal_policy"].keys()) == {
        "entry_threshold", "long_only", "sizing", "max_gross_exposure",
        "fold_end_policy", "capacity_policy",
    }


def test_legacy_long_short_threshold_v1_identity_and_defaults_unchanged() -> None:
    """REQUIRED TEST 25."""
    from mqk_research.ml.economic_walkforward import SIGNAL_DIRECTION_POLICY_LONG_SHORT_THRESHOLD_V1

    spec = SignalPolicySpec(
        entry_threshold=0.7, long_only=False,
        direction_policy=SIGNAL_DIRECTION_POLICY_LONG_SHORT_THRESHOLD_V1,
        short_threshold=0.3,
    ).normalized()
    assert spec.sizing == "equal_weight_active"
    assert spec.rank_side_count is None
    assert spec.borrow_model == BORROW_MODEL_RESEARCH_ASSUMED_SHORTABLE_UNIVERSE_V1
    identity = economic_protocol_identity(_diagnostic_spec(spec).normalized())
    assert identity["signal_policy"]["sizing"] == "equal_weight_active"
    assert "rank_side_count" not in identity["signal_policy"]


def test_rank_sizing_id_is_distinct_from_legacy() -> None:
    """Sanity check backing REQUIRED TESTS 3/24: rank sizing never collides
    with legacy 'equal_weight_active'."""
    assert SIGNAL_SIZING_EQUAL_WEIGHT_RANK_SELECTED_V1 != "equal_weight_active"
    spec = _rank_long_only(2)
    assert spec.sizing == SIGNAL_SIZING_EQUAL_WEIGHT_RANK_SELECTED_V1
