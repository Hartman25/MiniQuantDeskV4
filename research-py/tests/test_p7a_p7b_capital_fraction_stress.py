"""Half-exposure robustness stress for capital-fraction trials.

The stress is a RECOMPUTED target quantity (the Rust resolver re-run at a strictly smaller
`allocation_fraction_bps` on the same immutable capital), replayed through the unchanged exact-target
machinery. It is never a USD cap on the baseline quantity (which the exact-target replay refuses)
and it is an evaluation scenario of the SAME registered trial, never a trial.
"""

from __future__ import annotations

import json
from pathlib import Path

import pandas as pd
import pytest

from mqk_research.ml.native_signal_registry_integration import NATIVE_SIGNAL_STREAM_PROTOCOL_ID
from mqk_research.ml.p7a_p7b_economic_replay_stress_cli import _run_replay_stress
from mqk_research.ml.replay_authority import ReplayAuthorityError
from mqk_research.ml.util_hash import sha256_file

from tests import test_native_signal_registry_integration as base

M = 1_000_000
CAPITAL = int(base.EQUITY * M)
FP_STRESS = "c" * 64
CF = "fixed_initial_capital_fraction_v1"
Q_BASE, Q_STRESS = 80, 40
CAP = 50000.0


def sizing_block(bps: int, q: int) -> dict:
    return {"policy_id": CF, "allocation_fraction_bps": bps, "initial_allocated_capital_micros": CAPITAL,
            "max_target_qty": None, "max_position_notional_usd": CAP,
            "entries": [{"resolved_target_qty_micros": q * M}]}


def expected_sizing(bps: int) -> dict:
    return {k: v for k, v in sizing_block(bps, 1).items() if k != "entries"}


class CfEnv(base.Env):
    """Capital-fraction baseline: registered at 1000 bps, constant Q_BASE while long."""

    def __init__(self, tmp: Path, **kw):
        super().__init__(tmp, qty_one=Q_BASE * M, meta_overrides={"sizing": sizing_block(1000, Q_BASE)}, **kw)

    def spec(self):
        return base._spec(max_position_notional_usd=CAP)

    def run_baseline(self, bps: int = 1000):
        self.meta_overrides = {"sizing": sizing_block(bps, Q_BASE)}
        self.register_cf(bps)
        return self.run_cf(bps)

    def register_cf(self, bps: int) -> str:
        return base.register_native_signal_trial(**self.reg_kwargs(
            economic_spec=self.spec(), capital_sizing=expected_sizing(bps)))

    def run_cf(self, bps: int) -> Path:
        kw = dict(
            experiment_id=base.EXPERIMENT, hypothesis_id="H1", strategy_id=base.STRATEGY, symbol=base.SYMBOL,
            bars_csv=self.bars_csv, bars_provenance=self.manifest, backtest_bars_csv=self.bt,
            emit_signals=self.emit, signals_csv=self.signals, signals_meta_json=self.meta,
            economic_spec=self.spec(), evaluation_start_utc=base.EVAL_START,
            expected_semantic_fingerprint=base.FP_X, required_history_bars=base.REQUIRED, test_months=12,
            holdout_months=6, registry_db=self.db, expected_capital_sizing=expected_sizing(bps))
        return base.run_registered_native_signal_economic_eval(self.tmp / "baseline", **kw)

    def write_stress_stream(self, name: str, *, bps: int = 500, q: int = Q_STRESS, fingerprint: str = FP_STRESS,
                            mutate=None, meta_overrides=None) -> dict:
        spy = pd.read_csv(self.bt)
        base_stream = pd.read_csv(self.signals)
        qty = [q * M if b > 0 else 0 for b in base_stream["target_qty_micros"]]
        if mutate is not None:
            qty = mutate(qty)
        csv = self.tmp / f"{name}_signals.csv"
        pd.DataFrame({"symbol": base.SYMBOL, "decision_ts": spy["end_ts"], "target_qty_micros": qty}).to_csv(
            csv, index=False, lineterminator="\n")
        meta = json.loads(self.meta.read_text(encoding="utf-8"))
        meta.update({"semantic_fingerprint": fingerprint, "native_signals_csv_sha256": sha256_file(csv),
                     "sizing": sizing_block(bps, q), "signal_rows": len(qty)})
        meta.update(meta_overrides or {})
        meta_path = self.tmp / f"{name}_meta.json"
        meta_path.write_text(json.dumps(meta), encoding="utf-8")
        return {"scenario_id": "half_exposure_capital_fraction_500bps_v1", "allocation_fraction_bps": bps,
                "signals_csv": str(csv), "signals_meta": str(meta_path), "expected_semantic_fingerprint": fingerprint}


def stress(env: CfEnv, eval_id: str, trial_id: str, sizing: dict | None, **over):
    kw = dict(registry_db=env.db, trial_id=trial_id, economic_eval_id=eval_id, stress_out_dir=env.tmp / "stress",
              stress_execution_slippage_bps=15, stress_execution_volatility_mult_bps=10,
              stress_max_target_qty=None, stress_max_position_notional_usd=None, max_drawdown_ceiling=0.40,
              stress_sizing=sizing)
    kw.update(over)
    return _run_replay_stress(**kw)


@pytest.fixture()
def fixture(tmp_path):
    env = CfEnv(tmp_path)
    out = env.run_baseline()
    econ = json.loads(out.read_text(encoding="utf-8"))
    return env, econ["registry"]["trial_id"], econ["ids"]["economic_eval_id"], econ


def test_half_exposure_stress_recomputes_a_smaller_quantity_and_replays_it(fixture):
    env, trial_id, eval_id, econ = fixture
    res = stress(env, eval_id, trial_id, env.write_stress_stream("s500"))
    assert res["status"] == "evaluated" and isinstance(res["passed"], bool)
    ss = res["stress_spec"]["stress_sizing"]
    assert (ss["allocation_fraction_bps"], ss["baseline_allocation_fraction_bps"]) == (500, 1000)
    assert ss["nominal_entry_budget_micros"] == 5_000_000_000 and ss["baseline_nominal_entry_budget_micros"] == 10_000_000_000
    assert ss["initial_allocated_capital_micros"] == CAPITAL and ss["is_a_trial"] is False
    assert ss["caps_unchanged_from_baseline"] is True and ss["baseline_caps"]["max_position_notional_usd"] == CAP
    assert ss["stress_semantic_fingerprint"] != ss["baseline_semantic_fingerprint"]
    assert res["stress_spec"]["max_position_notional_usd"] is None  # no cap encodes the half exposure
    # The replayed economics hold the stress quantity, not the baseline one.
    oos = pd.read_csv(env.tmp / "stress" / "stress_oos_predictions.csv")
    assert set(oos["target_qty"]) == {0, Q_STRESS}
    base_oos = pd.read_csv(Path(econ["inputs"]["oos_predictions_csv"]["path"]))
    assert set(base_oos["target_qty"]) == {0, Q_BASE}
    assert list(oos["decision_ts"]) == list(base_oos["decision_ts"])
    assert res["stressed_economic_eval_id"] != res["baseline_economic_eval_id"] == eval_id
    assert res["trial_id"] == trial_id
    # No new trial: the registry still holds exactly the baseline trial.
    assert len(env.store().list_trials(experiment_id=base.EXPERIMENT)) == 1


def test_a_usd_5000_cap_on_the_1000_bps_quantity_is_refused_which_is_why_it_is_not_the_stress(fixture):
    env, trial_id, eval_id, _ = fixture
    with pytest.raises(RuntimeError, match="exceeds max_position_notional_usd=5000"):
        stress(env, eval_id, trial_id, None, stress_max_position_notional_usd=5000.0)


def test_a_cap_alongside_capital_fraction_stress_is_refused_as_conflation(fixture):
    env, trial_id, eval_id, _ = fixture
    sizing = env.write_stress_stream("s500")
    for cap in ({"stress_max_position_notional_usd": 25000.0}, {"stress_max_target_qty": 30}):
        res = stress(env, eval_id, trial_id, sizing, **cap)
        assert res["status"] == "error" and "forbids" in res["reason"]


def test_stress_bps_must_be_strictly_below_the_baseline_and_the_baseline_cannot_be_replaced(fixture):
    env, trial_id, eval_id, _ = fixture
    for bps in (1000, 1001):
        with pytest.raises(ReplayAuthorityError, match="not adverse"):
            stress(env, eval_id, trial_id, env.write_stress_stream(f"s{bps}", bps=bps, q=Q_BASE))
    # A trial registered at 500 bps is a DIFFERENT trial identity from the 1000 bps baseline.
    other = base.build_native_signal_trial_identity(
        experiment_id=base.EXPERIMENT, hypothesis_id="H1", strategy_id=base.STRATEGY, symbol=base.SYMBOL,
        semantic_fingerprint=base.FP_X, required_history_bars=base.REQUIRED, bars_provenance=env.manifest,
        evaluation_start_utc=base.EVAL_START, test_months=12, holdout_months=6, economic_spec=env.spec(),
        capital_sizing=expected_sizing(500))[0]
    assert other != trial_id


def test_501_bps_is_a_different_scenario_identity_but_never_a_new_trial(fixture):
    env, trial_id, eval_id, _ = fixture
    a = stress(env, eval_id, trial_id, env.write_stress_stream("s500", bps=500), stress_out_dir=env.tmp / "st500")
    b = stress(env, eval_id, trial_id, env.write_stress_stream("s501", bps=501), stress_out_dir=env.tmp / "st501")
    sa, sb = a["stress_spec"]["stress_sizing"], b["stress_spec"]["stress_sizing"]
    assert sa != sb and sa["allocation_fraction_bps"] == 500 and sb["allocation_fraction_bps"] == 501
    assert sa["stress_native_signals_meta_sha256"] != sb["stress_native_signals_meta_sha256"]
    assert a["trial_id"] == b["trial_id"] == trial_id
    assert len(env.store().list_trials(experiment_id=base.EXPERIMENT)) == 1
    # A stream whose own sizing block disagrees with the claimed fraction is refused.
    lie = env.write_stress_stream("lie", bps=501)
    lie["allocation_fraction_bps"] = 500
    with pytest.raises(ReplayAuthorityError, match="allocation_fraction_bps"):
        stress(env, eval_id, trial_id, lie)


@pytest.mark.parametrize("name,kwargs,msg", [
    ("baseline_fingerprint", dict(fingerprint=base.FP_X), "wrapper fingerprint|does not equal the expected"),
    ("different_capital", dict(meta_overrides={"initial_cash_micros": 90_000 * M}), "initial_cash_micros"),
    ("greater_than_baseline", dict(q=Q_BASE + 1), "exceeds the baseline"),
    ("unresolved_quantity", dict(q=Q_STRESS, meta_overrides={"sizing": {**sizing_block(500, 7)}}), "no engine-resolved"),
    ("other_bars", dict(meta_overrides={"bars_csv_sha256": "0" * 64}), "not emitted over the bars"),
])
def test_defective_stress_streams_are_refused(fixture, name, kwargs, msg):
    env, trial_id, eval_id, _ = fixture
    with pytest.raises(ReplayAuthorityError, match=msg):
        stress(env, eval_id, trial_id, env.write_stress_stream(name, **kwargs))


def test_a_stress_stream_that_changes_a_decision_is_refused(fixture):
    env, trial_id, eval_id, _ = fixture
    def flip(q):
        out = list(q)
        out[5] = Q_STRESS * M if out[5] == 0 else 0
        return out
    with pytest.raises(ReplayAuthorityError, match="changed a long/flat decision"):
        stress(env, eval_id, trial_id, env.write_stress_stream("flip", mutate=flip))


def test_a_fixed_quantity_baseline_has_no_capital_fraction_stress(tmp_path):
    env = base.Env(tmp_path)
    out = env.run()
    econ = json.loads(out.read_text(encoding="utf-8"))
    cf = CfEnv.__new__(CfEnv)
    cf.__dict__.update(env.__dict__)
    cf.bt, cf.signals = env.bt, env.signals
    sizing = {"scenario_id": "x", "allocation_fraction_bps": 500, "signals_csv": str(env.signals),
              "signals_meta": str(env.meta), "expected_semantic_fingerprint": FP_STRESS}
    with pytest.raises(ReplayAuthorityError, match="not a capital-fraction trial"):
        stress(cf, econ["ids"]["economic_eval_id"], econ["registry"]["trial_id"], sizing)
