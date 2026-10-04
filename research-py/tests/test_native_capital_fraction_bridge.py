"""Research side of FixedInitialCapitalFractionV1: trial identity binds the sizing
contract before any result exists, and a stream is accepted only when its sizing
block equals the registered contract exactly. Synthetic fixtures only; the Rust
emitter's own parity with Backtest is proven in
`scenario_native_signal_capital_fraction_01`.
"""

from __future__ import annotations

import copy
import json
import sys
from pathlib import Path

import pytest

HERE = Path(__file__).resolve().parent
sys.path.insert(0, str(HERE))

from test_native_signal_registry_integration import (  # noqa: E402
    EQUITY, EXPERIMENT, FP_X, FP_Y, REQUIRED, STRATEGY, SYMBOL, Env)

from mqk_research.ml.native_signal_registry_integration import (  # noqa: E402
    NativeSignalError, build_native_signal_trial_identity)

CF = "fixed_initial_capital_fraction_v1"
CAPITAL_MICROS = int(EQUITY * 1_000_000)
Q = 50  # shares


def sizing(**over) -> dict:
    base = {"policy_id": CF, "allocation_fraction_bps": 1000, "initial_allocated_capital_micros": CAPITAL_MICROS,
            "max_target_qty": None, "max_position_notional_usd": None}
    base.update(over)
    return base


def stream_sizing(**over) -> dict:
    block = {**sizing(), "entries": [{"resolved_target_qty_micros": Q * 1_000_000}]}
    block.update(over)
    return block


def cf_env(tmp_path, *, sizing_block=None, qty=Q, **kw) -> Env:
    meta = {"sizing": sizing_block if sizing_block is not None else stream_sizing()}
    meta.update(kw.pop("meta_overrides", {}))
    return Env(tmp_path, qty_one=qty * 1_000_000, meta_overrides=meta, **kw)


def run_cf(env: Env, *, expected=None, register=True, **over):
    expected = sizing() if expected is None else expected
    if register:
        env.register(capital_sizing=expected)
    return env.run(register=False, expected_capital_sizing=expected, **over)


def identity_kwargs(env: Env) -> dict:
    return {k: v for k, v in env.reg_kwargs().items() if k not in ("hypothesis_text", "registry_db")}


def test_trial_identity_binds_policy_fraction_capital_and_caps(tmp_path):
    kw = identity_kwargs(Env(tmp_path))

    def tid(cs):
        return build_native_signal_trial_identity(**kw, capital_sizing=cs)[0]

    fixed = build_native_signal_trial_identity(**kw)[0]
    base = tid(sizing())
    assert base == tid(sizing()), "deterministic"
    assert base != fixed, "a capital-fraction trial can never equal the fixed-quantity trial"
    assert base != tid(sizing(allocation_fraction_bps=2000)), "1000 -> 2000 bps"
    assert base != tid(sizing(initial_allocated_capital_micros=200_000 * 1_000_000)), "capital"
    assert base != tid(sizing(max_target_qty=10)), "cap"
    assert base != tid(sizing(max_position_notional_usd=5000)), "notional cap"
    # Historical identity: no capital_sizing argument == the pre-existing builder.
    assert fixed == build_native_signal_trial_identity(**kw, capital_sizing=None)[0]
    with pytest.raises(NativeSignalError, match="exactly the keys"):
        tid({"policy_id": CF})


def test_every_trial_registers_before_any_attempt_and_without_market_results(tmp_path):
    env = Env(tmp_path)
    ids = [env.register(hypothesis=f"H{i}", capital_sizing=sizing(allocation_fraction_bps=1000 + i))
           for i in range(3)]
    store = env.store()
    assert len(set(ids)) == 3
    assert len(store.list_trials(experiment_id=EXPERIMENT)) == 3
    assert [store.list_attempts(t) for t in ids] == [[], [], []], "registration creates no attempt"
    assert env.emit_calls == 0


def test_capital_fraction_stream_is_accepted_and_the_exact_sized_quantity_is_what_is_held(tmp_path):
    env = cf_env(tmp_path)
    econ = json.loads(run_cf(env).read_text(encoding="utf-8"))
    held = set()
    for fold in econ["folds"]:
        held |= {e["target_qty"] for e in fold["weight_to_share_evidence"][SYMBOL]}
    assert held == {0, Q}
    assert econ["registry"]["execution_fidelity"] == 1.0
    assert econ["holdout"] == {"status": "reserved_not_evaluated"}


def test_retry_is_a_new_attempt_not_a_new_trial(tmp_path):
    env = cf_env(tmp_path)
    a = json.loads(run_cf(env, run_name="r1").read_text())["registry"]
    b = json.loads(run_cf(env, run_name="r2", register=False).read_text())["registry"]
    assert a["trial_id"] == b["trial_id"]
    assert (a["attempt_index"], b["attempt_index"]) == (1, 2)
    assert len(env.store().list_trials(experiment_id=EXPERIMENT)) == 1


@pytest.mark.parametrize("name,block,msg", [
    ("fraction", stream_sizing(allocation_fraction_bps=2000), "allocation_fraction_bps"),
    ("capital", stream_sizing(initial_allocated_capital_micros=50_000 * 1_000_000), "initial_allocated_capital_micros"),
    ("policy", stream_sizing(policy_id="fixed_quantity_v1"), "policy_id"),
    ("cap", stream_sizing(max_target_qty=5), "max_target_qty"),
    ("no_entries", stream_sizing(entries="x"), "no entries list"),
    ("non_positive_entry", stream_sizing(entries=[{"resolved_target_qty_micros": 0}]), "non-positive"),
])
def test_stream_sizing_block_must_equal_the_registered_contract(tmp_path, name, block, msg):
    env = cf_env(tmp_path, sizing_block=block)
    with pytest.raises(NativeSignalError, match=msg):
        run_cf(env)
    (trial,) = env.store().list_trials(experiment_id=EXPERIMENT)
    assert [a["status"] for a in env.store().list_attempts(trial["trial_id"])] == ["failed"], name


def test_a_positive_quantity_no_engine_entry_produced_is_refused(tmp_path):
    env = cf_env(tmp_path, qty=Q + 7)  # entries still say Q
    with pytest.raises(NativeSignalError, match="no engine-resolved"):
        run_cf(env)


def test_fixed_quantity_stream_cannot_satisfy_a_capital_fraction_registration(tmp_path):
    env = Env(tmp_path)  # stream has no sizing block
    with pytest.raises(NativeSignalError, match="carries no sizing block"):
        run_cf(env)


def test_capital_fraction_stream_cannot_be_consumed_as_fixed_quantity(tmp_path):
    env = cf_env(tmp_path)
    env.register()
    with pytest.raises(NativeSignalError, match="registered under the fixed-quantity protocol"):
        env.run(register=False)


def test_unregistered_sizing_contract_is_not_a_registered_trial(tmp_path):
    env = cf_env(tmp_path)
    env.register(capital_sizing=sizing())
    with pytest.raises(NativeSignalError, match="is not registered"):
        env.run(register=False, expected_capital_sizing=sizing(allocation_fraction_bps=2000))
    assert env.emit_calls == 0


def test_old_fifty_thousand_dollar_capital_basis_cannot_authorize_the_protocol(tmp_path):
    env = cf_env(tmp_path, meta_overrides={"initial_cash_micros": 50_000 * 1_000_000})
    with pytest.raises(NativeSignalError, match="capital basis"):
        run_cf(env)


def test_the_wrapped_fingerprint_not_the_inner_one_is_required(tmp_path):
    env = cf_env(tmp_path, fingerprint=FP_Y)  # stream bound to a different identity than registered
    with pytest.raises(NativeSignalError, match="expected native fingerprint"):
        run_cf(env)


def test_sizing_meta_is_not_mutated_by_verification(tmp_path):
    env = cf_env(tmp_path)
    before = copy.deepcopy(stream_sizing())
    run_cf(env)
    assert json.loads(env.meta.read_text())["sizing"] == before
    assert REQUIRED == 50 and STRATEGY


def _cli_or_skip() -> Path:
    from test_native_signal_registry_integration import _cli
    cli = _cli()
    if cli is None:
        pytest.skip("mqk-cli binary not built")
    return cli


def _cf_flags(bps: int, capital_micros: int = CAPITAL_MICROS) -> list[str]:
    return ["--sizing-policy", CF, "--allocation-fraction-bps", str(bps), "--initial-cash-micros", str(capital_micros)]


def test_real_rust_capital_fraction_roundtrip(tmp_path):
    import subprocess
    cli = _cli_or_skip()
    env = Env(tmp_path)

    def fingerprint(*flags: str) -> dict:
        out = subprocess.run([str(cli), "backtest", "native-fingerprint", "--strategy", STRATEGY, "--symbol", SYMBOL,
                              *flags], check=True, capture_output=True, text=True).stdout
        return dict(line.split("=", 1) for line in out.splitlines() if "=" in line)

    fixed = fingerprint()
    cf = fingerprint(*_cf_flags(1000))
    assert cf["semantic_fingerprint"] != fixed["semantic_fingerprint"]
    assert cf["inner_semantic_fingerprint"] == fixed["semantic_fingerprint"]
    assert cf["semantic_fingerprint"] != fingerprint(*_cf_flags(2000))["semantic_fingerprint"]
    assert cf["semantic_fingerprint"] != fingerprint(*_cf_flags(1000, 200_000 * 1_000_000))["semantic_fingerprint"]
    required = int(cf["required_history_bars"])

    out_dir = tmp_path / "emit"

    def emit():
        subprocess.run([str(cli), "backtest", "native-signals", "--bars-path", str(env.bt), "--strategy", STRATEGY,
                        "--symbol", SYMBOL, "--timeframe-secs", "86400", "--out-dir", str(out_dir), *_cf_flags(1000)],
                       check=True, capture_output=True, text=True)

    env.register(semantic_fingerprint=cf["semantic_fingerprint"], required_history_bars=required,
                 capital_sizing=sizing())
    out = env.run(register=False, emit_signals=emit, signals_csv=out_dir / "native_signals.csv",
                  signals_meta_json=out_dir / "native_signals_meta.json",
                  expected_semantic_fingerprint=cf["semantic_fingerprint"], required_history_bars=required,
                  expected_capital_sizing=sizing())
    meta = json.loads((out_dir / "native_signals_meta.json").read_text())
    qs = {int(r.split(",")[2]) for r in (out_dir / "native_signals.csv").read_text().splitlines()[1:]}
    resolved = {e["resolved_target_qty_micros"] for e in meta["sizing"]["entries"]}
    assert max(qs) > 1_000_000, "10% of 100k buys more than the legacy +1 share"
    assert qs - {0} <= resolved
    assert meta["sizing"]["allocation_fraction_bps"] == 1000
    assert json.loads(out.read_text())["registry"]["execution_fidelity"] == 1.0


def test_real_rust_capital_fraction_flags_fail_closed(tmp_path):
    import subprocess
    cli = _cli_or_skip()
    env = Env(tmp_path)

    def native_fp(*flags: str) -> subprocess.CompletedProcess:
        return subprocess.run([str(cli), "backtest", "native-fingerprint", "--strategy", STRATEGY, "--symbol", SYMBOL,
                               *flags], capture_output=True, text=True)

    def signals(*flags: str) -> subprocess.CompletedProcess:
        return subprocess.run([str(cli), "backtest", "native-signals", "--bars-path", str(env.bt), "--strategy",
                               STRATEGY, "--symbol", SYMBOL, "--timeframe-secs", "86400", "--out-dir",
                               str(tmp_path / "o"), *flags], capture_output=True, text=True)

    bad_flag_sets = [
        ["--sizing-policy", CF, "--initial-cash-micros", str(CAPITAL_MICROS)],           # no fraction
        ["--sizing-policy", CF, "--allocation-fraction-bps", "1000"],                    # no capital
        ["--sizing-policy", CF, "--allocation-fraction-bps", "0", "--initial-cash-micros", "1"],
        ["--sizing-policy", CF, "--allocation-fraction-bps", "10001", "--initial-cash-micros", "1"],
        ["--sizing-policy", CF, "--allocation-fraction-bps", "1000", "--initial-cash-micros", "0"],
        ["--allocation-fraction-bps", "1000", "--initial-cash-micros", str(CAPITAL_MICROS)],  # fraction w/o policy
        ["--initial-cash-micros", str(CAPITAL_MICROS)],                                       # capital w/o policy
        ["--max-target-qty", "5"],                                                            # cap w/o policy
    ]
    for flags in bad_flag_sets:
        for run in (native_fp, signals):
            res = run(*flags)
            assert res.returncode != 0, (run.__name__, flags)
    assert not (tmp_path / "o" / "native_signals.csv").exists(), "no stream is emitted on a refused declaration"


# ---------------------------------------------------------------------------
# Predeclared stress authority: the exact robustness stress is part of the
# registered trial identity (selected before the first attempt, never from a result).
# ---------------------------------------------------------------------------

SC = "half_exposure_capital_fraction_500bps_v1"


def stress(**over) -> dict:
    base = {"scenario_id": SC, "allocation_fraction_bps": 500}
    base.update(over)
    return base


def test_stress_contract_is_identity_bearing_and_absent_means_historical_identity(tmp_path):
    kw = identity_kwargs(Env(tmp_path))

    def tid(cs=None, sc=None):
        return build_native_signal_trial_identity(**kw, capital_sizing=cs, stress_contract=sc)[0]

    historical = tid(sizing())
    assert historical == build_native_signal_trial_identity(**kw, capital_sizing=sizing())[0]
    with_contract = tid(sizing(), stress())
    assert with_contract == tid(sizing(), stress()), "deterministic"
    assert with_contract != historical
    assert with_contract != tid(sizing(), stress(allocation_fraction_bps=250)), "fraction"
    assert with_contract != tid(sizing(), stress(scenario_id="other_scenario")), "scenario"
    assert with_contract != tid(sizing(allocation_fraction_bps=2000), stress()), "baseline"
    _, identity = build_native_signal_trial_identity(**kw, capital_sizing=sizing(), stress_contract=stress())
    assert identity["signal_source"]["stress_contract"] == stress()
    _, plain = build_native_signal_trial_identity(**kw, capital_sizing=sizing())
    assert "stress_contract" not in plain["signal_source"]


@pytest.mark.parametrize("bad", [
    stress(allocation_fraction_bps=1000),   # not strictly below the 1000 bps baseline
    stress(allocation_fraction_bps=0),
    stress(allocation_fraction_bps=True),
    stress(allocation_fraction_bps=500.0),
    stress(scenario_id=""),
    stress(scenario_id=7),
    {"scenario_id": SC},
    {**stress(), "extra": 1},
])
def test_malformed_stress_contract_is_refused(tmp_path, bad):
    kw = identity_kwargs(Env(tmp_path))
    with pytest.raises(NativeSignalError, match="stress_contract"):
        build_native_signal_trial_identity(**kw, capital_sizing=sizing(), stress_contract=bad)


def test_stress_contract_requires_the_capital_fraction_protocol(tmp_path):
    kw = identity_kwargs(Env(tmp_path))
    with pytest.raises(NativeSignalError, match="stress_contract requires capital_sizing"):
        build_native_signal_trial_identity(**kw, stress_contract=stress())


def test_registered_stress_contract_cannot_be_swapped_after_registration(tmp_path):
    env = cf_env(tmp_path)
    env.register(capital_sizing=sizing(), stress_contract=stress())
    # The registered contract is what an attempt must present: a friendlier stress, or none, is a
    # different (unregistered) trial and is refused before any emission.
    for presented in (stress(allocation_fraction_bps=900), None):
        with pytest.raises(NativeSignalError, match="not registered"):
            env.run(register=False, expected_capital_sizing=sizing(), expected_stress_contract=presented)
    assert env.emit_calls == 0
    out = env.run(register=False, expected_capital_sizing=sizing(), expected_stress_contract=stress())
    assert out.exists()
    registered = json.loads(env.store().get_trial(next(t["trial_id"] for t in env.store().list_trials(experiment_id=EXPERIMENT)))["identity_json"])
    assert registered["signal_source"]["stress_contract"] == stress()
