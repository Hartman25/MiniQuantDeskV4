"""M1 native hypothesis batch 01 runner. Every parameter is read from
PREDECLARED_BATCH_01.json; nothing result-dependent is chosen here.

Stages (each once, in order): check | reuse_data | register | trials | judge |
backtest | finalize | review | summary. `register` emits the native signal
streams and registers every hypothesis and all fifteen trials BEFORE any
economic evaluation exists; `trials` then runs the fifteen attempts in the
frozen order and an economic failure never stops the batch.
"""

from __future__ import annotations

import argparse
import json
import re
import subprocess
import sys
from pathlib import Path

import pandas as pd

HERE = Path(__file__).resolve().parent
REPO = HERE.parents[2]
sys.path.insert(0, str(REPO / "research-py" / "src"))

DECL = json.loads((HERE / "PREDECLARED_BATCH_01.json").read_text(encoding="utf-8"))
RUN = HERE / DECL["run_dir"]
REGISTRY = HERE / DECL["experiment"]["registry_db_relative_path"]
EXPERIMENT = DECL["experiment"]["real_experiment_id"]
HYP = {h["strategy_id"]: h for h in DECL["hypotheses"]}
TRIALS = [(t["strategy_id"], t["symbol"]) for t in DECL["universe"]["trials"]]  # frozen order
STRATEGIES = [h["strategy_id"] for h in DECL["hypotheses"]]
CLI = REPO / "core-rs" / "target" / "debug" / "mqk-cli.exe"
BARS = RUN / "data" / "research_bars.csv"
MANIFEST = RUN / "data" / "research_bars_provenance.json"
INDEX = RUN / "trials_index.json"
GAP_TOLERANCE_BARS = 3


def tdir(strategy: str, symbol: str) -> Path:
    return RUN / "trials" / strategy / symbol


def key(strategy: str, symbol: str) -> str:
    return f"{strategy}/{symbol}"


def _economic_spec():
    from mqk_research.ml.economic_walkforward import (
        AnnualizationSpec, CostModelSpec, EconomicWalkForwardSpec, SignalPolicySpec)
    from mqk_research.ml.execution_pricing import ExecutionPricingSpec
    from mqk_research.ml.weight_to_share import WeightToShareSpec
    p = DECL["economic_protocol"]
    return EconomicWalkForwardSpec(
        signal_policy=SignalPolicySpec(**p["signal_policy"]),
        cost_model=CostModelSpec(**p["cost_model"]),
        execution_pricing=ExecutionPricingSpec(**p["execution_pricing"]),
        weight_to_share=WeightToShareSpec(**p["weight_to_share"]),
        annualization=AnnualizationSpec(**p["annualization"]),
    )


def _run_cli(*argv: str) -> str:
    out = subprocess.run([str(CLI), *argv], capture_output=True, text=True)
    if out.returncode != 0:
        raise SystemExit(f"mqk-cli failed ({argv[:2]}): {out.stderr[-800:]}")
    return out.stdout


def _parse(out: str, name: str) -> str:
    m = re.search(rf"^{name}=(.+)$", out, re.M)
    if not m:
        raise SystemExit(f"missing {name} in cli output")
    return m.group(1).strip()


def _load_index() -> dict:
    return json.loads(INDEX.read_text(encoding="utf-8")) if INDEX.exists() else {}


def _save_index(index: dict) -> None:
    INDEX.write_text(json.dumps(index, indent=1, sort_keys=True), encoding="utf-8")


def stage_check(_args) -> None:
    assert len(TRIALS) == DECL["universe"]["max_trials"] == 15
    print(f"batch={DECL['batch_id']} trials={len(TRIALS)} strategies={STRATEGIES} cli_present={CLI.exists()}")


def stage_reuse_data(_args) -> None:
    import shutil
    from mqk_research.ml.util_hash import sha256_file
    src = HERE / DECL["data"]["reuse_verified_data_from"]["run_dir"]
    manifest = json.loads((src / "research_bars_provenance.json").read_text(encoding="utf-8"))
    csv_path = src / "research_bars.csv"
    rows = len(pd.read_csv(csv_path))
    if sha256_file(csv_path) != manifest["artifact_sha256"] or rows != manifest["row_count"]:
        raise SystemExit("fail-closed: source bars do not match their provenance manifest")
    (RUN / "data").mkdir(parents=True, exist_ok=True)
    for name in ("research_bars.csv", "research_bars_provenance.json", "corporate_actions.json",
                 "corporate_actions_provenance.json"):
        shutil.copyfile(src / name, RUN / "data" / name)
    print("reused", rows, "rows", manifest["artifact_sha256"][:12])


def stage_register(_args) -> None:
    """Emit each trial's native signal stream (no economics) and register every hypothesis and trial."""
    from mqk_research.exp_distributed.storage import ResearchResultStore
    from mqk_research.ml.economic_registry_integration import ECONOMIC_PROTOCOL_ID
    from mqk_research.ml.native_signal_registry_integration import (
        build_native_signal_trial_identity, native_holdout_start, research_bars_to_backtest_csv)
    part, manifest = DECL["partition"], json.loads(MANIFEST.read_text(encoding="utf-8"))
    REGISTRY.parent.mkdir(parents=True, exist_ok=True)
    store = ResearchResultStore(REGISTRY)
    index = _load_index()
    for strategy, sym in TRIALS:
        h = HYP[strategy]
        sdir = tdir(strategy, sym)
        sdir.mkdir(parents=True, exist_ok=True)
        hold = native_holdout_start(BARS, sym, part["holdout_months"])
        bt = research_bars_to_backtest_csv(BARS, sym, sdir / "bt_bars.csv", end_exclusive_utc=hold)
        _run_cli("backtest", "native-signals", "--bars-path", str(bt), "--strategy", strategy, "--symbol", sym,
                 "--timeframe-secs", str(h["timeframe_secs"]), "--out-dir", str(sdir / "emit"))
        meta = json.loads((sdir / "emit" / "native_signals_meta.json").read_text(encoding="utf-8"))
        trial_id, identity = build_native_signal_trial_identity(
            experiment_id=EXPERIMENT, hypothesis_id=h["hypothesis_id"], strategy_id=strategy, symbol=sym,
            semantic_fingerprint=meta["semantic_fingerprint"], bars_provenance=manifest,
            evaluation_start_utc=pd.Timestamp(part["evaluation_start_utc"]), test_months=part["test_months"],
            holdout_months=part["holdout_months"], economic_spec=_economic_spec().normalized())
        store.register_hypothesis(hypothesis_id=h["hypothesis_id"], experiment_id=EXPERIMENT,
                                  hypothesis_text=h["economic_rationale"])
        store.register_trial(trial_id=trial_id, experiment_id=EXPERIMENT, hypothesis_id=h["hypothesis_id"],
                             strategy_id=strategy, protocol_id=ECONOMIC_PROTOCOL_ID, identity=identity)
        index[key(strategy, sym)] = {"trial_id": trial_id, "hypothesis_id": h["hypothesis_id"],
                                     "semantic_fingerprint": meta["semantic_fingerprint"]}
        print(key(strategy, sym), trial_id, meta["semantic_fingerprint"][:12])
    _save_index(index)
    registered = store.list_trials(experiment_id=EXPERIMENT)
    if len(registered) != 15:
        raise SystemExit(f"fail-closed: {len(registered)} registered trials, expected 15")
    print("registered_unique_trials", len(registered), "attempts", sum(len(store.list_attempts(t["trial_id"])) for t in registered))


def stage_trials(_args) -> None:
    from mqk_research.exp_distributed.storage import ResearchResultStore
    from mqk_research.ml.native_signal_registry_integration import (
        NativeSignalError, run_registered_native_signal_economic_eval)
    part, manifest = DECL["partition"], json.loads(MANIFEST.read_text(encoding="utf-8"))
    store = ResearchResultStore(REGISTRY)
    if len(store.list_trials(experiment_id=EXPERIMENT)) != 15:
        raise SystemExit("fail-closed: all fifteen trials must be registered before any economic evaluation")
    index = _load_index()
    for strategy, sym in TRIALS:  # frozen order; failures never stop the batch
        h, sdir, rec = HYP[strategy], tdir(strategy, sym), index[key(strategy, sym)]
        try:
            out = run_registered_native_signal_economic_eval(
                sdir / "run", experiment_id=EXPERIMENT, hypothesis_id=h["hypothesis_id"], strategy_id=strategy,
                symbol=sym, bars_csv=BARS, bars_provenance=manifest, backtest_bars_csv=sdir / "bt_bars.csv",
                signals_csv=sdir / "emit" / "native_signals.csv", signals_meta_json=sdir / "emit" / "native_signals_meta.json",
                economic_spec=_economic_spec(), evaluation_start_utc=pd.Timestamp(part["evaluation_start_utc"]),
                test_months=part["test_months"], holdout_months=part["holdout_months"],
                hypothesis_text=h["economic_rationale"], registry_db=REGISTRY,
                expected_timeframe_secs=h["timeframe_secs"], expected_semantic_fingerprint=rec["semantic_fingerprint"])
        except NativeSignalError as exc:  # the failed attempt is already durable
            rec["failed"] = str(exc)
            print(key(strategy, sym), "FAILED attempt kept:", str(exc)[:200])
            continue
        econ = json.loads(out.read_text(encoding="utf-8"))
        if econ["registry"]["trial_id"] != rec["trial_id"]:
            raise SystemExit(f"fail-closed: trial id drift for {key(strategy, sym)}")
        rec.update({"economic_eval_id": econ["ids"]["economic_eval_id"], "economic_path": str(out),
                    "attempt_index": econ["registry"]["attempt_index"],
                    "execution_fidelity": econ["registry"]["execution_fidelity"]})
        print(key(strategy, sym), rec["trial_id"], "attempt", rec["attempt_index"])
    _save_index(index)


def stage_judge(_args) -> None:
    from mqk_research.exp_distributed.hashing import canonical_json, sha256_bytes
    from mqk_research.exp_distributed.storage import ResearchResultStore
    from mqk_research.ml.multiple_testing_judge import build_multiple_testing_judge
    registered = ResearchResultStore(REGISTRY).list_trials(experiment_id=EXPERIMENT)
    if len(registered) != 15:
        raise SystemExit(f"fail-closed: {len(registered)} registered trials, expected 15")
    art = build_multiple_testing_judge(experiment_id=EXPERIMENT, registry_db=REGISTRY)  # whole experiment, hypothesis_id unset
    (RUN / "judge").mkdir(parents=True, exist_ok=True)
    (RUN / "judge" / "judge.json").write_text(json.dumps(art, sort_keys=True, separators=(",", ":")), encoding="utf-8")
    sha = sha256_bytes(canonical_json(art).encode("utf-8"))
    (RUN / "judge" / "judge_sha256.txt").write_text(sha, encoding="utf-8")
    print("judge_status", art["judge_status"], "population", art["registry_population"],
          "included", len(art["included_trial_ids"]), "excluded", art["excluded_trial_ids"], "sha", sha[:12])


def stage_backtest(_args) -> None:
    index = _load_index()
    nb = DECL["native_backtest"]
    for strategy, sym in TRIALS:
        rec = index[key(strategy, sym)]
        if "failed" in rec:
            print(key(strategy, sym), "no succeeded trial; rejected, no backtest evidence")
            continue
        out = _run_cli("backtest", "csv", "--bars", str(tdir(strategy, sym) / "bt_bars.csv"), "--strategy", strategy,
                       "--symbol", sym, "--timeframe-secs", str(nb["timeframe_secs"]),
                       "--initial-cash-micros", str(nb["initial_cash_micros"]),
                       "--integrity-calendar", "us-equity-regular", "--integrity-stale-threshold-ticks", "259200",
                       "--integrity-gap-tolerance-bars", str(GAP_TOLERANCE_BARS),
                       "--out-dir", str(RUN / "backtest" / strategy / sym))
        rec["backtest_run_id"] = _parse(out, "run_id")
        rec["execution_blocked"] = _parse(out, "execution_blocked")
        print(key(strategy, sym), rec["backtest_run_id"], "execution_blocked", rec["execution_blocked"])
    _save_index(index)


def stage_finalize(_args) -> None:
    index = _load_index()
    sha = (RUN / "judge" / "judge_sha256.txt").read_text(encoding="utf-8").strip()
    rb = DECL["robustness"]
    st = rb["p7a_p7b_stress"]
    py = sys.executable
    for strategy, sym in TRIALS:
        rec = index[key(strategy, sym)]
        if "failed" in rec:
            continue
        common = ["--artifact-root", str(RUN / "backtest" / strategy / sym), "--run-id", rec["backtest_run_id"],
                  "--registry-db", str(REGISTRY), "--trial-id", rec["trial_id"]]
        _run_cli("backtest", "finalize-robustness-sensitivity", *common, "--judge-artifact-sha256", sha,
                 "--research-py-root", str(REPO / "research-py"), "--python", py,
                 "--block-counts", ",".join(map(str, rb["block_counts"])),
                 "--dsr-max-sensitivity-range", str(rb["dsr_max_sensitivity_range"]),
                 "--pbo-max-sensitivity-range", str(rb["pbo_max_sensitivity_range"]))
        _run_cli("backtest", "finalize-p7a-p7b-replay-stress", *common, "--economic-eval-id", rec["economic_eval_id"],
                 "--research-py-root", str(REPO / "research-py"), "--python", py,
                 "--stress-out-dir", str(RUN / "stress" / strategy / sym),
                 "--stress-execution-slippage-bps", str(st["stress_execution_slippage_bps"]),
                 "--stress-execution-volatility-mult-bps", str(st["stress_execution_volatility_mult_bps"]),
                 "--stress-max-position-notional-usd", str(st["stress_max_position_notional_usd"]),
                 "--max-drawdown-ceiling", str(st["max_drawdown_ceiling"]))
        _run_cli("backtest", "finalize-genuine-shuffled-placebo", *common, "--economic-eval-id", rec["economic_eval_id"],
                 "--research-py-root", str(REPO / "research-py"), "--python", py,
                 "--placebo-out-dir", str(RUN / "placebo" / strategy / sym))
        print(key(strategy, sym), "finalized")


def stage_review(_args) -> None:
    reg = json.loads((REPO / "config" / "instruments" / "equities.json").read_text(encoding="utf-8"))
    symbols = sorted({s for _, s in TRIALS})
    sub = [i for i in reg if i["symbol"] in symbols]
    assert sorted(i["symbol"] for i in sub) == symbols
    for strategy in STRATEGIES:
        base = RUN / "scan" / strategy
        base.mkdir(parents=True, exist_ok=True)
        reg_path = base / "registry.json"
        reg_path.write_text(json.dumps(sub, indent=1, sort_keys=True), encoding="utf-8")
        root = base / "bars" / "1D"
        root.mkdir(parents=True, exist_ok=True)
        for sym in symbols:
            (root / f"{sym}_1D.csv").write_bytes((tdir(strategy, sym) / "bt_bars.csv").read_bytes())
        out = _run_cli("backtest", "scan-strategies", "--registry", str(reg_path), "--bars-root", str(base / "bars"),
                       "--timeframe", "1D", "--strategy", strategy, "--out-dir", str(base / "scans"))
        scan_dir = _parse(out, "artifacts_dir")
        out = _run_cli("backtest", "review-scan", "--artifact-dir", scan_dir, "--out-dir", str(base / "reviews"))
        print(strategy, out)


def stage_summary(_args) -> None:
    print(INDEX.read_text(encoding="utf-8"))


STAGES = {"check": stage_check, "reuse_data": stage_reuse_data, "register": stage_register, "trials": stage_trials,
          "judge": stage_judge, "backtest": stage_backtest, "finalize": stage_finalize, "review": stage_review,
          "summary": stage_summary}


def main() -> None:
    ap = argparse.ArgumentParser()
    ap.add_argument("stage", choices=sorted(STAGES))
    args = ap.parse_args()
    STAGES[args.stage](args)


if __name__ == "__main__":
    main()
