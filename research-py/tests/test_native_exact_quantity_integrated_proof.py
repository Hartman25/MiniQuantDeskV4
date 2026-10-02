"""One quantity/economic contract across the whole native chain.

native exact target (real Rust engine/emitter)
 -> registered trial and attempt
 -> Research economic evidence
 -> Rust Backtest evidence
 -> the identity the Rust promotion verifier reads
all bind the SAME semantics: a +1-share target is +1 share everywhere, on one
capital basis. Skips when the CLI binary with the v2 emitter is not built. No
promotion is created and nothing touches Paper or a broker.
"""

from __future__ import annotations

import csv
import json
import subprocess
import sys
from pathlib import Path

import pytest

sys.path.insert(0, str(Path(__file__).resolve().parent))

from mqk_research.ml.native_signal_registry_integration import build_native_signal_trial_identity

from test_native_signal_registry_integration import (  # noqa: E402  (sibling test module)
    EQUITY, EVAL_START, FP_X, REQUIRED, STRATEGY, SYMBOL, Env, _cli, _cli_has_v2, _spec,
)

GOLDEN = (Path(__file__).resolve().parents[2] / "core-rs" / "crates" / "mqk-promotion" / "tests"
          / "fixtures" / "native_v2_identity_binding.json")


def _kv(text: str) -> dict:
    return dict(line.split("=", 1) for line in text.splitlines() if "=" in line)


def test_python_identity_matches_the_golden_binding_the_rust_verifier_consumes(tmp_path):
    env = Env(tmp_path)
    _, identity = build_native_signal_trial_identity(**{
        k: v for k, v in env.reg_kwargs().items() if k not in ("hypothesis_text", "registry_db")})
    golden = json.loads(GOLDEN.read_text(encoding="utf-8"))
    assert identity["signal_source"] == golden["signal_source"]
    assert identity["economic_protocol"]["signal_policy"] == golden["economic_protocol"]["signal_policy"]
    assert golden["signal_source"]["semantic_fingerprint"] == FP_X and golden["signal_source"]["required_history_bars"] == REQUIRED


@pytest.mark.skipif(not _cli_has_v2(), reason="mqk-cli binary with the v2 emitter not built")
def test_exact_target_quantity_is_one_contract_across_emitter_research_backtest_and_identity(tmp_path):
    env = Env(tmp_path)
    cli = str(_cli())
    info = _kv(subprocess.run([cli, "backtest", "native-fingerprint", "--strategy", STRATEGY, "--symbol", SYMBOL],
                              check=True, capture_output=True, text=True).stdout)
    fp, required = info["semantic_fingerprint"], int(info["required_history_bars"])

    out_dir = tmp_path / "emit"

    def emit():
        subprocess.run([cli, "backtest", "native-signals", "--bars-path", str(env.bt), "--strategy", STRATEGY,
                        "--symbol", SYMBOL, "--timeframe-secs", "86400", "--out-dir", str(out_dir)],
                       check=True, capture_output=True, text=True)

    env.register(semantic_fingerprint=fp, required_history_bars=required)
    econ_path = env.run(register=False, emit_signals=emit, signals_csv=out_dir / "native_signals.csv",
                        signals_meta_json=out_dir / "native_signals_meta.json",
                        expected_semantic_fingerprint=fp, required_history_bars=required)
    econ = json.loads(econ_path.read_text(encoding="utf-8"))
    meta = json.loads((out_dir / "native_signals_meta.json").read_text(encoding="utf-8"))

    # 1. The real engine's emitted targets are absolute whole shares: 0 or +1.
    rows = list(csv.DictReader(open(out_dir / "native_signals.csv", encoding="utf-8")))
    assert {int(r["target_qty_micros"]) for r in rows} == {0, 1_000_000}
    assert meta["quantity_semantics"] == "absolute_target_qty_micros_v1"

    # 2. Research held exactly that quantity, never a re-sized position.
    held = set()
    for fold in econ["folds"]:
        held |= {e["target_qty"] for e in fold["weight_to_share_evidence"][SYMBOL]}
    assert held == {0, 1}
    assert econ["registry"]["execution_fidelity"] == 1.0 and econ["registry"]["max_abs_target_qty_gap"] == 0

    # 3. Backtest evidence on the same bars, same capital basis: the same one share.
    assert meta["initial_cash_micros"] == int(EQUITY * 1_000_000)
    bt_out = tmp_path / "bt"
    subprocess.run([cli, "backtest", "csv", "--bars", str(env.bt), "--strategy", STRATEGY, "--symbol", SYMBOL,
                    "--timeframe-secs", "86400", "--initial-cash-micros", str(meta["initial_cash_micros"]),
                    "--integrity-calendar", "us-equity-regular", "--integrity-stale-threshold-ticks", "259200",
                    "--integrity-gap-tolerance-bars", "3", "--out-dir", str(bt_out)],
                   check=True, capture_output=True, text=True)
    (run_dir,) = [p for p in bt_out.iterdir() if p.is_dir()]
    metrics = json.loads((run_dir / "metrics.json").read_text(encoding="utf-8"))
    assert metrics["starting_equity_micros"] == meta["initial_cash_micros"]
    assert metrics["strategy_sizing"]["target_qty"] == 1
    position = peak = 0
    positions = set()
    for fill in csv.DictReader(open(run_dir / "fills.csv", encoding="utf-8")):
        position += int(fill["qty"]) * (1 if fill["side"] == "BUY" else -1)
        positions.add(position)
        peak = max(peak, position)
    assert positions <= {0, 1}, "the Rust Backtest only ever holds 0 or +1 share"
    assert peak == 1 == max(held), "Backtest and Research hold the same +1 share"

    # 4. The identity registered for this trial carries the contract the promotion verifier requires.
    store = env.store()
    (trial,) = store.list_trials(experiment_id="NATIVE-TEST-EXP")
    identity = json.loads(trial["identity_json"])
    assert identity["signal_source"]["target_semantics"] == "absolute_whole_share_target_v1"
    assert identity["signal_source"]["semantic_fingerprint"] == fp == meta["semantic_fingerprint"]
    assert identity["economic_protocol"]["signal_policy"]["direction_policy"] == "native_exact_target_qty_v1"
    assert identity["economic_protocol"]["weight_to_share"]["equity_usd"] == EQUITY
