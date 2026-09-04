"""
RESEARCH-POINT-IN-TIME-UNIVERSE-01 -- proves `pit_universe` wiring into
`run_registered_factor_diagnostics` (the RESEARCH-FACTOR-IC-IR-QUANTILE-
BENCH-01 registered runner), not the `UniverseSpec` primitive itself
(already covered by test_factor_universe.py).

This is the first genuine production caller of
`UniverseSpec.members_as_of` / `assert_observations_within_universe` /
`universe_identity_binding` outside their own tests.

`factor_spec.universe_identity` must already equal the resolved (mode-
tagged) evaluation universe identity -- the runner fails closed on any
mismatch rather than silently reconciling them (see runner.py's module
docstring). Every fixture here therefore constructs the universe identity
FIRST and threads it into both the FactorSpec and the runner call.

PIT vs fixed-ex-ante mode is semantic, execution-affecting behavior (only
PIT proves row-by-row membership), so the resolved mode is baked directly
into the bound universe_identity and is never accepted verbatim from a
caller-supplied value (neither in universe_identity's content nor in
metadata["universe_mode"]) -- several tests here specifically prove that
authority can never be spoofed or made to collide across modes.
"""
from __future__ import annotations

import pandas as pd
import pytest

from mqk_research.factors.contracts import (
    DIRECTION_HIGHER_IS_BETTER,
    EVAL_STATUS_FAILED,
    EVAL_STATUS_SUCCEEDED,
    FactorSpec,
    NORMALIZATION_CROSS_SECTIONAL_RANK,
    TIMING_NEXT_BAR_TRADABLE,
)
from mqk_research.factors.diagnostics import EvaluationWindowViolation
from mqk_research.factors.registry import list_factor_evaluation_attempts
from mqk_research.factors.runner import (
    UNIVERSE_MODE_FIXED_EX_ANTE,
    UNIVERSE_MODE_POINT_IN_TIME,
    run_registered_factor_diagnostics,
)
from mqk_research.factors.universe import UniverseMembershipRecord, UniverseSpec, universe_identity_binding

N_SYMBOLS = 6
N_PERIODS = 8
_FAR_FUTURE_LABEL_END = "2099-01-01T00:00:00+00:00"
_COVERAGE_START = "2024-01-01T00:00:00+00:00"
_COVERAGE_END = "2024-02-01T00:00:00+00:00"


def _periods():
    return [f"2024-01-{d:02d}T00:00:00+00:00" for d in range(1, N_PERIODS + 1)]


def _symbols():
    return [f"SYM{i}" for i in range(N_SYMBOLS)]


def _with_causal_columns(df: pd.DataFrame) -> pd.DataFrame:
    df = df.copy()
    df["information_cutoff_ts_utc"] = df["period_ts_utc"]
    df["label_end_ts_utc"] = _FAR_FUTURE_LABEL_END
    return df


def _monotonic_dataset() -> pd.DataFrame:
    rows = []
    for period in _periods():
        for i, sym in enumerate(_symbols()):
            rows.append(
                {"symbol": sym, "period_ts_utc": period, "factor_value": float(i), "label_fwd_ret": float(i)}
            )
    return _with_causal_columns(pd.DataFrame(rows))


def _full_coverage_universe(*, name: str = "test_pit_universe") -> UniverseSpec:
    members = [
        UniverseMembershipRecord(symbol=sym, effective_from_utc=_COVERAGE_START, effective_through_utc=None)
        for sym in _symbols()
    ]
    return UniverseSpec(
        universe_name=name,
        universe_protocol_version="test_pit_v1",
        coverage_start_utc=_COVERAGE_START,
        coverage_end_utc=_COVERAGE_END,
        members=members,
    )


def _spec(**overrides) -> FactorSpec:
    kwargs = dict(
        family="momentum",
        name="12m_1m_lag",
        protocol_version="v1",
        params={"lookback_days": 252},
        required_input_fields=["close"],
        lookback_periods=252,
        horizon_periods=21,
        normalization=NORMALIZATION_CROSS_SECTIONAL_RANK,
        direction=DIRECTION_HIGHER_IS_BETTER,
        universe_identity={},
        data_provenance_identity={"provider": "alpaca"},
        timing_convention=TIMING_NEXT_BAR_TRADABLE,
        information_lag_periods=1,
    )
    kwargs.update(overrides)
    return FactorSpec(**kwargs)


def _run(
    registry_db,
    out_dir,
    *,
    observations,
    universe_identity=None,
    pit_universe=None,
    spec=None,
    evaluation_window_start_utc="2024-01-01T00:00:00+00:00",
    evaluation_window_end_utc=_COVERAGE_END,
    **overrides,
):
    kwargs = dict(
        factor_spec=spec or _spec(),
        observations=observations,
        universe_identity=universe_identity,
        pit_universe=pit_universe,
        evaluation_window_start_utc=evaluation_window_start_utc,
        evaluation_window_end_utc=evaluation_window_end_utc,
        label_protocol_version="fwd_ret_label_v1",
        origin="test_pit_universe_wiring",
    )
    kwargs.update(overrides)
    return run_registered_factor_diagnostics(registry_db, out_dir, **kwargs)


def test_pit_universe_evaluation_succeeds_and_binds_universe_identity(tmp_path):
    registry_db = tmp_path / "registry.sqlite3"
    universe = _full_coverage_universe()
    resolved_identity = {**universe_identity_binding(universe), "universe_mode": UNIVERSE_MODE_POINT_IN_TIME}
    spec = _spec(universe_identity=resolved_identity)

    result = _run(registry_db, tmp_path / "out", observations=_monotonic_dataset(), pit_universe=universe, spec=spec)

    assert result.status == EVAL_STATUS_SUCCEEDED
    attempts = list_factor_evaluation_attempts(registry_db, result.factor_id)
    assert len(attempts) == 1
    assert attempts[0]["metadata"]["universe_mode"] == UNIVERSE_MODE_POINT_IN_TIME
    assert attempts[0]["evaluation_identity"]["universe_identity"] == resolved_identity


def test_fixed_ex_ante_mode_is_recorded_and_never_mislabeled_pit(tmp_path):
    registry_db = tmp_path / "registry.sqlite3"
    declared_identity = {"universe_id": "fixed_declared_v1"}
    resolved_identity = {**declared_identity, "universe_mode": UNIVERSE_MODE_FIXED_EX_ANTE}

    result = _run(
        registry_db,
        tmp_path / "out",
        observations=_monotonic_dataset(),
        universe_identity=declared_identity,
        spec=_spec(universe_identity=resolved_identity),
    )

    assert result.status == EVAL_STATUS_SUCCEEDED
    attempts = list_factor_evaluation_attempts(registry_db, result.factor_id)
    assert attempts[0]["metadata"]["universe_mode"] == UNIVERSE_MODE_FIXED_EX_ANTE


def test_fixed_mode_cannot_spoof_metadata_as_point_in_time(tmp_path):
    """A caller-supplied metadata["universe_mode"] must never be trusted --
    it is always overwritten from the actual resolved mode, so a fixed-
    ex-ante run (unproven membership chronology) can never be durably
    mislabeled as a proven point-in-time evaluation."""
    registry_db = tmp_path / "registry.sqlite3"
    declared_identity = {"universe_id": "fixed_declared_v1"}
    resolved_identity = {**declared_identity, "universe_mode": UNIVERSE_MODE_FIXED_EX_ANTE}

    result = _run(
        registry_db,
        tmp_path / "out",
        observations=_monotonic_dataset(),
        universe_identity=declared_identity,
        spec=_spec(universe_identity=resolved_identity),
        metadata={"universe_mode": UNIVERSE_MODE_POINT_IN_TIME},
    )

    attempts = list_factor_evaluation_attempts(registry_db, result.factor_id)
    assert attempts[0]["metadata"]["universe_mode"] == UNIVERSE_MODE_FIXED_EX_ANTE


def test_same_universe_content_under_fixed_vs_pit_never_collides(tmp_path):
    """Identical underlying universe_id/universe_protocol_version, one
    evaluated as a proven `pit_universe` and one declared as a bare fixed
    `universe_identity` -- these execute genuinely different membership
    rules and must never produce the same factor_id/evaluation_id merely
    because the caller passed matching universe content."""
    registry_db = tmp_path / "registry.sqlite3"
    universe = _full_coverage_universe()
    shared_binding = universe_identity_binding(universe)

    pit_identity = {**shared_binding, "universe_mode": UNIVERSE_MODE_POINT_IN_TIME}
    fixed_identity = {**shared_binding, "universe_mode": UNIVERSE_MODE_FIXED_EX_ANTE}
    assert pit_identity != fixed_identity  # sanity: mode is the only difference

    pit_result = _run(
        registry_db,
        tmp_path / "a",
        observations=_monotonic_dataset(),
        pit_universe=universe,
        spec=_spec(universe_identity=pit_identity),
    )
    fixed_result = _run(
        registry_db,
        tmp_path / "b",
        observations=_monotonic_dataset(),
        universe_identity=shared_binding,
        spec=_spec(universe_identity=fixed_identity),
    )

    assert pit_result.factor_id != fixed_result.factor_id
    assert pit_result.evaluation_id != fixed_result.evaluation_id


def test_neither_universe_identity_nor_pit_universe_rejected(tmp_path):
    registry_db = tmp_path / "registry.sqlite3"
    with pytest.raises(ValueError):
        _run(registry_db, tmp_path / "out", observations=_monotonic_dataset())


def test_both_universe_identity_and_pit_universe_rejected(tmp_path):
    registry_db = tmp_path / "registry.sqlite3"
    with pytest.raises(ValueError):
        _run(
            registry_db,
            tmp_path / "out",
            observations=_monotonic_dataset(),
            universe_identity={"universe_id": "x"},
            pit_universe=_full_coverage_universe(),
        )


def test_factor_spec_universe_identity_mismatch_fails_closed(tmp_path):
    """factor_spec.universe_identity must already equal the resolved
    (mode-tagged) evaluation universe identity -- the runner never
    silently reconciles a mismatch."""
    registry_db = tmp_path / "registry.sqlite3"
    universe = _full_coverage_universe()
    mismatched_spec = _spec(universe_identity={"universe_id": "not_this_universe"})

    with pytest.raises(ValueError):
        _run(
            registry_db, tmp_path / "out", observations=_monotonic_dataset(), pit_universe=universe, spec=mismatched_spec
        )


def test_evaluation_window_exceeding_universe_coverage_fails_closed(tmp_path):
    """A declared evaluation window that extends past the universe's
    declared PIT coverage is not proven PIT authority, regardless of which
    rows the caller happened to supply."""
    registry_db = tmp_path / "registry.sqlite3"
    universe = _full_coverage_universe()
    resolved_identity = {**universe_identity_binding(universe), "universe_mode": UNIVERSE_MODE_POINT_IN_TIME}
    spec = _spec(universe_identity=resolved_identity)

    with pytest.raises(ValueError):
        _run(
            registry_db,
            tmp_path / "out",
            observations=_monotonic_dataset(),
            pit_universe=universe,
            spec=spec,
            evaluation_window_end_utc="2024-06-01T00:00:00+00:00",  # past coverage_end 2024-02-01
        )

    attempts = list_factor_evaluation_attempts(registry_db, spec.compute_factor_id())
    assert len(attempts) == 1
    assert attempts[0]["status"] == EVAL_STATUS_FAILED
    assert "coverage" in attempts[0]["failure_reason"]


def test_evaluation_window_starting_before_universe_coverage_fails_closed(tmp_path):
    registry_db = tmp_path / "registry.sqlite3"
    universe = _full_coverage_universe()
    resolved_identity = {**universe_identity_binding(universe), "universe_mode": UNIVERSE_MODE_POINT_IN_TIME}
    spec = _spec(universe_identity=resolved_identity)

    with pytest.raises(ValueError):
        _run(
            registry_db,
            tmp_path / "out",
            observations=_monotonic_dataset(),
            pit_universe=universe,
            spec=spec,
            evaluation_window_start_utc="2023-12-01T00:00:00+00:00",  # before coverage_start
        )

    attempts = list_factor_evaluation_attempts(registry_db, spec.compute_factor_id())
    assert len(attempts) == 1
    assert attempts[0]["status"] == EVAL_STATUS_FAILED


def test_observation_outside_declared_evaluation_window_fails_closed_even_inside_universe_coverage(tmp_path):
    """P1's window-binding check applies to a PIT run too -- proven
    universe membership coverage does NOT substitute for proving each
    observation belongs to the DECLARED evaluation window slice."""
    registry_db = tmp_path / "registry.sqlite3"
    universe = _full_coverage_universe()
    resolved_identity = {**universe_identity_binding(universe), "universe_mode": UNIVERSE_MODE_POINT_IN_TIME}
    spec = _spec(universe_identity=resolved_identity)

    with pytest.raises(EvaluationWindowViolation):
        _run(
            registry_db,
            tmp_path / "out",
            # dataset periods run 2024-01-01..2024-01-08, all inside universe
            # coverage [2024-01-01, 2024-02-01) -- but the DECLARED window
            # below ends 2024-01-05, narrower than both.
            observations=_monotonic_dataset(),
            pit_universe=universe,
            spec=spec,
            evaluation_window_end_utc="2024-01-05T00:00:00+00:00",
        )


def test_future_constituent_observation_fails_closed_no_survivorship_shortcut(tmp_path):
    registry_db = tmp_path / "registry.sqlite3"
    # SYM0 only becomes a member on 2024-01-05 -- observations include it
    # from 2024-01-01, i.e. before its real effective_from_utc.
    members = [
        UniverseMembershipRecord(symbol="SYM0", effective_from_utc="2024-01-05T00:00:00+00:00")
    ] + [
        UniverseMembershipRecord(symbol=sym, effective_from_utc=_COVERAGE_START)
        for sym in _symbols()
        if sym != "SYM0"
    ]
    universe = UniverseSpec(
        universe_name="partial_coverage_universe",
        universe_protocol_version="test_pit_v1",
        coverage_start_utc=_COVERAGE_START,
        coverage_end_utc=_COVERAGE_END,
        members=members,
    )
    resolved_identity = {**universe_identity_binding(universe), "universe_mode": UNIVERSE_MODE_POINT_IN_TIME}
    spec = _spec(universe_identity=resolved_identity)

    with pytest.raises(Exception):
        _run(registry_db, tmp_path / "out", observations=_monotonic_dataset(), pit_universe=universe, spec=spec)

    attempts = list_factor_evaluation_attempts(registry_db, spec.compute_factor_id())
    assert len(attempts) == 1
    assert attempts[0]["status"] == EVAL_STATUS_FAILED
    assert "not a member" in attempts[0]["failure_reason"]


def test_removed_constituent_observation_after_effective_through_fails_closed(tmp_path):
    registry_db = tmp_path / "registry.sqlite3"
    # SYM0's membership ends 2024-01-04 -- observations still reference it
    # through 2024-01-08, i.e. after its real effective_through_utc.
    members = [
        UniverseMembershipRecord(
            symbol="SYM0", effective_from_utc=_COVERAGE_START, effective_through_utc="2024-01-04T00:00:00+00:00"
        )
    ] + [
        UniverseMembershipRecord(symbol=sym, effective_from_utc=_COVERAGE_START)
        for sym in _symbols()
        if sym != "SYM0"
    ]
    universe = UniverseSpec(
        universe_name="removed_constituent_universe",
        universe_protocol_version="test_pit_v1",
        coverage_start_utc=_COVERAGE_START,
        coverage_end_utc=_COVERAGE_END,
        members=members,
    )
    resolved_identity = {**universe_identity_binding(universe), "universe_mode": UNIVERSE_MODE_POINT_IN_TIME}
    spec = _spec(universe_identity=resolved_identity)

    with pytest.raises(Exception):
        _run(registry_db, tmp_path / "out", observations=_monotonic_dataset(), pit_universe=universe, spec=spec)

    attempts = list_factor_evaluation_attempts(registry_db, spec.compute_factor_id())
    assert len(attempts) == 1
    assert attempts[0]["status"] == EVAL_STATUS_FAILED
    assert "not a member" in attempts[0]["failure_reason"]


def test_universe_membership_change_changes_both_factor_and_evaluation_identity(tmp_path):
    """A genuine membership CONTENT change (removing a member) mints a
    different universe_id -- and since factor_spec.universe_identity must
    track the resolved evaluation universe identity under the conservative
    contract, that change is visible at BOTH factor_id and evaluation_id,
    never silently absorbed."""
    registry_db = tmp_path / "registry.sqlite3"
    full_universe = _full_coverage_universe(name="universe_v1")
    narrower_members = [m for m in full_universe.members if m.symbol != "SYM5"]
    narrower_universe = UniverseSpec(
        universe_name=full_universe.universe_name,
        universe_protocol_version=full_universe.universe_protocol_version,
        coverage_start_utc=full_universe.coverage_start_utc,
        coverage_end_utc=full_universe.coverage_end_utc,
        members=narrower_members,
    )
    assert full_universe.compute_universe_id() != narrower_universe.compute_universe_id()

    full_identity = {**universe_identity_binding(full_universe), "universe_mode": UNIVERSE_MODE_POINT_IN_TIME}
    narrower_identity = {**universe_identity_binding(narrower_universe), "universe_mode": UNIVERSE_MODE_POINT_IN_TIME}
    full_spec = _spec(universe_identity=full_identity)
    narrower_spec = _spec(universe_identity=narrower_identity)

    full_obs = _monotonic_dataset()
    narrower_obs = full_obs[full_obs["symbol"] != "SYM5"]

    first = _run(registry_db, tmp_path / "a", observations=full_obs, pit_universe=full_universe, spec=full_spec)
    second = _run(
        registry_db, tmp_path / "b", observations=narrower_obs, pit_universe=narrower_universe, spec=narrower_spec
    )

    assert first.factor_id != second.factor_id
    assert first.evaluation_id != second.evaluation_id


def test_member_layout_order_only_change_never_changes_identity(tmp_path):
    """Reordering a UniverseSpec's `members` list (no content change) must
    never mint a new universe_id, factor_id, or evaluation_id --
    `UniverseSpec.identity_payload()` already sorts members canonically."""
    registry_db = tmp_path / "registry.sqlite3"
    universe_a = _full_coverage_universe(name="order_test")
    universe_b = UniverseSpec(
        universe_name=universe_a.universe_name,
        universe_protocol_version=universe_a.universe_protocol_version,
        coverage_start_utc=universe_a.coverage_start_utc,
        coverage_end_utc=universe_a.coverage_end_utc,
        members=list(reversed(universe_a.members)),
    )
    assert universe_a.compute_universe_id() == universe_b.compute_universe_id()

    resolved_identity = {**universe_identity_binding(universe_a), "universe_mode": UNIVERSE_MODE_POINT_IN_TIME}
    spec = _spec(universe_identity=resolved_identity)

    first = _run(registry_db, tmp_path / "a", observations=_monotonic_dataset(), pit_universe=universe_a, spec=spec)
    second = _run(registry_db, tmp_path / "b", observations=_monotonic_dataset(), pit_universe=universe_b, spec=spec)

    assert first.factor_id == second.factor_id
    assert first.evaluation_id == second.evaluation_id
