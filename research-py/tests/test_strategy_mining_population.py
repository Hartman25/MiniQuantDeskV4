from __future__ import annotations

import dataclasses
import inspect
from pathlib import Path

import pytest

from mqk_research.strategy_mining import grammar as grammar_mod
from mqk_research.strategy_mining import population as population_mod
from mqk_research.strategy_mining.grammar import (
    AssetClassTag,
    Direction,
    GrammarError,
    HypothesisGrammar,
    MechanismFamily,
    OPERATIONAL_ASSET_CLASSES,
)
from mqk_research.strategy_mining.population import (
    PopulationDeclaration,
    deterministic_trial_id,
    generate_population,
    trial_identity_for,
)


def _decl(**over) -> PopulationDeclaration:
    base = dict(
        mechanism_families=(MechanismFamily.TREND_FOLLOWING, MechanismFamily.BREAKOUT),
        directions=(Direction.LONG,),
        asset_classes=(AssetClassTag.EQUITY,),
        parameter_grids={
            MechanismFamily.TREND_FOLLOWING: {"lookback_days": (20, 50)},
            MechanismFamily.BREAKOUT: {"lookback_days": (10,)},
        },
        timeframe="1d",
        data_inputs=("daily_bars",),
        universe_requirement="sp500_point_in_time",
        session_calendar_contract="us_equity_calendar_v1",
        sizing_contract="capital_fraction_v1",
        execution_model_contract="next_bar_open_v1",
        cost_model_contract="flat_bps_v1",
        risk_model_requirement="none",
        point_in_time_universe_requirement="required",
        historical_evidence_partition="research_partition_2026h1",
        max_population_size=2_000,
    )
    base.update(over)
    return PopulationDeclaration(**base)


# ---------------------------------------------------------------------------
# Predeclared cardinality / resource bounds
# ---------------------------------------------------------------------------

def test_declared_cardinality_matches_hand_computed_value():
    decl = _decl()
    # trend_following: 2 lookbacks * 1 direction * 1 asset = 2
    # breakout:        1 lookback  * 1 direction * 1 asset = 1
    assert decl.declared_cardinality() == 3
    manifest = generate_population(decl)
    assert manifest.declared_cardinality == 3
    assert manifest.raw_count == 3


def test_population_over_budget_is_rejected_before_generation():
    decl = _decl(max_population_size=2)
    with pytest.raises(GrammarError, match="exceeds max_population_size"):
        generate_population(decl)


def test_empty_declaration_fails_closed():
    decl = _decl(mechanism_families=())
    with pytest.raises(GrammarError, match="empty"):
        generate_population(decl)


def test_duplicate_values_in_a_parameter_domain_fail_closed():
    decl = _decl(parameter_grids={
        MechanismFamily.TREND_FOLLOWING: {"lookback_days": (20, 20)},
        MechanismFamily.BREAKOUT: {"lookback_days": (10,)},
    })
    with pytest.raises(GrammarError, match="duplicate values"):
        generate_population(decl)


# ---------------------------------------------------------------------------
# Structural no-omission / no-extra-member proof (mutation-forced)
# ---------------------------------------------------------------------------

def test_raw_count_mismatch_against_predeclaration_fails_closed(monkeypatch):
    decl = _decl()

    real = population_mod.PopulationDeclaration._param_combos

    def _drop_one_combo(self, family):
        combos = real(self, family)
        if family == MechanismFamily.TREND_FOLLOWING and len(combos) > 1:
            return combos[:-1]  # silently produce one fewer than declared
        return combos

    monkeypatch.setattr(population_mod.PopulationDeclaration, "_param_combos", _drop_one_combo)
    with pytest.raises(GrammarError, match="generator/declaration mismatch"):
        generate_population(decl)


# ---------------------------------------------------------------------------
# Determinism / identity stability
# ---------------------------------------------------------------------------

def test_identical_declaration_produces_identical_manifest_id():
    m1 = generate_population(_decl())
    m2 = generate_population(_decl())
    assert m1.manifest_id == m2.manifest_id
    assert m1.declaration_id == m2.declaration_id


def test_field_insertion_order_does_not_change_fingerprint():
    g1 = HypothesisGrammar(
        mechanism_family=MechanismFamily.TREND_FOLLOWING, direction=Direction.LONG,
        asset_class=AssetClassTag.EQUITY, parameters={"a": 1, "b": 2},
        timeframe="1d", data_inputs=("x", "y"), universe_requirement="u",
        session_calendar_contract="c", sizing_contract="s", execution_model_contract="e",
        cost_model_contract="co", risk_model_requirement="r",
        point_in_time_universe_requirement="p", historical_evidence_partition="h",
    )
    g2 = HypothesisGrammar(
        mechanism_family=MechanismFamily.TREND_FOLLOWING, direction=Direction.LONG,
        asset_class=AssetClassTag.EQUITY, parameters={"b": 2, "a": 1},  # reversed insertion order
        timeframe="1d", data_inputs=("y", "x"),  # reversed order
        universe_requirement="u", session_calendar_contract="c", sizing_contract="s",
        execution_model_contract="e", cost_model_contract="co", risk_model_requirement="r",
        point_in_time_universe_requirement="p", historical_evidence_partition="h",
    )
    assert g1.semantic_fingerprint() == g2.semantic_fingerprint()


def test_meaningful_parameter_change_changes_fingerprint():
    m1 = generate_population(_decl(parameter_grids={
        MechanismFamily.TREND_FOLLOWING: {"lookback_days": (20,)},
        MechanismFamily.BREAKOUT: {"lookback_days": (10,)},
    }))
    m2 = generate_population(_decl(parameter_grids={
        MechanismFamily.TREND_FOLLOWING: {"lookback_days": (21,)},
        MechanismFamily.BREAKOUT: {"lookback_days": (10,)},
    }))
    assert m1.manifest_id != m2.manifest_id


def test_cost_model_change_changes_fingerprint_and_identity():
    decl_a = _decl(parameter_grids={
        MechanismFamily.TREND_FOLLOWING: {"lookback_days": (20,)},
        MechanismFamily.BREAKOUT: {"lookback_days": (10,)},
    }, cost_model_contract="flat_bps_v1")
    decl_b = dataclasses.replace(decl_a, cost_model_contract="flat_bps_v2")
    m_a = generate_population(decl_a)
    m_b = generate_population(decl_b)
    assert m_a.manifest_id != m_b.manifest_id


# ---------------------------------------------------------------------------
# Deduplication — identical hypotheses never become independent candidates
# ---------------------------------------------------------------------------

def test_duplicate_hypotheses_deduplicate_to_one_item():
    decl = _decl(
        mechanism_families=(MechanismFamily.TREND_FOLLOWING,),
        directions=(Direction.LONG, Direction.LONG),  # caller error: same direction twice
        parameter_grids={MechanismFamily.TREND_FOLLOWING: {"lookback_days": (20,)}},
    )
    manifest = generate_population(decl)
    assert manifest.raw_count == 2  # both instances were actually generated
    assert manifest.duplicate_count == 1
    assert len(manifest.items) == 1  # but only one survives as a candidate


# ---------------------------------------------------------------------------
# Unsupported asset/direction: tagged, never dropped, never coerced to a default
# ---------------------------------------------------------------------------

def test_unsupported_asset_class_is_tagged_not_dropped_and_not_coerced_to_equity():
    decl = _decl(
        mechanism_families=(MechanismFamily.TREND_FOLLOWING,),
        asset_classes=(AssetClassTag.CRYPTO,),
        parameter_grids={MechanismFamily.TREND_FOLLOWING: {"lookback_days": (20,)}},
    )
    manifest = generate_population(decl)
    assert len(manifest.items) == 1
    g, status = manifest.items[0]
    assert g.asset_class == AssetClassTag.CRYPTO  # never silently rewritten to EQUITY
    assert status == "unsupported_asset_class"
    assert manifest.unsupported_count == 1
    assert manifest.compatible_count == 0


def test_unsupported_direction_is_tagged_not_dropped_and_not_coerced():
    decl = _decl(
        mechanism_families=(MechanismFamily.TREND_FOLLOWING,),
        directions=(Direction.UNSUPPORTED,),
        parameter_grids={MechanismFamily.TREND_FOLLOWING: {"lookback_days": (20,)}},
    )
    manifest = generate_population(decl)
    g, status = manifest.items[0]
    assert g.direction == Direction.UNSUPPORTED
    assert status == "unsupported_direction"


def test_operational_asset_classes_is_only_equity_no_premature_enablement():
    assert OPERATIONAL_ASSET_CLASSES == frozenset({AssetClassTag.EQUITY})


# ---------------------------------------------------------------------------
# Result-independence (structural, not just conventional)
# ---------------------------------------------------------------------------

def test_hypothesis_grammar_has_no_result_field():
    field_names = {f.name for f in dataclasses.fields(HypothesisGrammar)}
    for forbidden in ("result", "pnl", "sharpe", "return", "rank", "score", "winner"):
        assert not any(forbidden in name for name in field_names), (
            f"HypothesisGrammar must stay result-independent; found a field matching {forbidden!r}"
        )


def test_population_declaration_has_no_result_field():
    field_names = {f.name for f in dataclasses.fields(PopulationDeclaration)}
    for forbidden in ("result", "pnl", "sharpe", "return", "rank", "score", "winner"):
        assert not any(forbidden in name for name in field_names)


def test_generate_population_signature_takes_no_history_or_result_input():
    params = set(inspect.signature(generate_population).parameters)
    assert params == {"decl"}


# ---------------------------------------------------------------------------
# No registry/broker/network authority embedded in the generator itself
# ---------------------------------------------------------------------------

def test_module_touches_no_registry_broker_or_network():
    """Checks actual import statements only (not prose/docstrings, which
    legitimately describe the existing seam this module stays compatible
    with) — a false-positive fixture would otherwise flag this module's own
    documentation of that relationship."""
    for mod in (grammar_mod, population_mod):
        import_lines = [
            line.strip()
            for line in inspect.getsource(mod).splitlines()
            if line.strip().startswith(("import ", "from "))
        ]
        for forbidden in (
            "exp_distributed.storage", "ResearchResultStore", "subprocess",
            "requests", "httpx", "socket", "promotion", "broker", "daemon",
        ):
            assert not any(forbidden in line for line in import_lines), (
                f"{mod.__name__} unexpectedly imports {forbidden!r}: {import_lines}"
            )


# ---------------------------------------------------------------------------
# Provenance completeness
# ---------------------------------------------------------------------------

def test_manifest_items_carry_complete_provenance_fields():
    manifest = generate_population(_decl())
    required = {
        "mechanism_family", "direction", "asset_class", "parameters", "timeframe",
        "data_inputs", "universe_requirement", "session_calendar_contract",
        "sizing_contract", "execution_model_contract", "cost_model_contract",
        "risk_model_requirement", "point_in_time_universe_requirement",
        "historical_evidence_partition",
    }
    for g, _status in manifest.items:
        fields = g.economic_fields()
        assert required <= set(fields.keys())
        for key in required:
            assert fields[key] not in (None, ""), f"{key} must not be empty/missing"


def test_deterministic_trial_id_is_stable_and_tracks_fingerprint():
    manifest = generate_population(_decl())
    g, _status = manifest.items[0]
    tid_1 = deterministic_trial_id("exp1", g)
    tid_2 = deterministic_trial_id("exp1", g)
    assert tid_1 == tid_2
    assert g.semantic_fingerprint()[:16] in tid_1


# ---------------------------------------------------------------------------
# Compatibility with the REAL existing registration seam (proves authority
# stays with existing machinery; this test, not the module, imports storage).
# ---------------------------------------------------------------------------

def test_trial_identity_round_trips_through_the_real_registry(tmp_path: Path):
    from mqk_research.exp_distributed.storage import ResearchResultStore

    manifest = generate_population(_decl())
    g, status = manifest.items[0]
    assert status == "compatible"

    store = ResearchResultStore(tmp_path / "mining_dry_run_registry.sqlite")
    store.register_hypothesis(hypothesis_id="h1", experiment_id="exp_mining_dry_run_01")

    identity = trial_identity_for(g, experiment_id="exp_mining_dry_run_01", hypothesis_id="h1")
    trial_id = deterministic_trial_id("exp_mining_dry_run_01", g)

    store.register_trial(
        trial_id=trial_id,
        experiment_id="exp_mining_dry_run_01",
        hypothesis_id="h1",
        strategy_id="strategy_mining_dry_run",
        protocol_id="strategy_mining_population_v1",
        identity=identity,
    )

    fetched = store.get_trial(trial_id)
    assert fetched["trial_id"] == trial_id
    assert fetched["hypothesis_id"] == "h1"

    # Idempotent re-registration with the SAME identity succeeds (no-op).
    store.register_trial(
        trial_id=trial_id, experiment_id="exp_mining_dry_run_01", hypothesis_id="h1",
        strategy_id="strategy_mining_dry_run", protocol_id="strategy_mining_population_v1",
        identity=identity,
    )

    # Conflicting identity under the same trial_id fails closed.
    other_g, _ = manifest.items[-1] if len(manifest.items) > 1 else (g, status)
    conflicting_identity = trial_identity_for(other_g, experiment_id="exp_mining_dry_run_01", hypothesis_id="h1")
    if conflicting_identity != identity:
        with pytest.raises(RuntimeError, match="conflicting canonical identity"):
            store.register_trial(
                trial_id=trial_id, experiment_id="exp_mining_dry_run_01", hypothesis_id="h1",
                strategy_id="strategy_mining_dry_run", protocol_id="strategy_mining_population_v1",
                identity=conflicting_identity,
            )
