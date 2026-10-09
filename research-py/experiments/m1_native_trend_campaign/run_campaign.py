"""M1 native trend campaign runner. Every parameter is read from
PREDECLARED_CAMPAIGN.json; nothing result-dependent is chosen here.

Stages (run in order, each once): check | fetch | trials | judge | backtest |
finalize | review | summary. `fetch` is the only stage that contacts a
network and requires --execute. Failed trials are registered and kept; there
is no stage that registers only winners.
"""

from __future__ import annotations

import argparse
import json
import os
import re
import subprocess
import sys
from pathlib import Path

import pandas as pd

HERE = Path(__file__).resolve().parent
REPO = HERE.parents[2]
sys.path.insert(0, str(REPO / "research-py" / "src"))

CAMPAIGN_FILE = os.environ.get("M1_CAMPAIGN_FILE", "PREDECLARED_CAMPAIGN_PULLBACK_01.json")
DECL = json.loads((HERE / CAMPAIGN_FILE).read_text(encoding="utf-8"))
sys.path.insert(0, str(HERE))
import stage_authorization  # noqa: E402

if not stage_authorization.is_frozen_historical(DECL):  # this runner has no stage gate: it may only serve pinned history
    raise SystemExit(f"fail-closed: {CAMPAIGN_FILE} is not a frozen historical declaration; this closed runner executes the "
                     "native binary without stage authorization and refuses any other declaration")
RUN = HERE / DECL.get("run_dir", "runs/run_01")
REGISTRY = HERE / DECL["experiment"].get("registry_db_relative_path", str(Path(DECL.get("run_dir", "runs/run_01")) / "registry" / "research.sqlite3"))
EXPERIMENT = DECL["experiment"]["real_experiment_id"]
HYPOTHESIS = DECL["hypothesis"]["hypothesis_id"]
STRATEGY = DECL["native_engine"]["strategy_id"]
SYMBOLS = DECL["universe"]["symbols"]
CLI = REPO / "core-rs" / "target" / "debug" / ("mqk-cli.exe" if os.name == "nt" else "mqk-cli")
BARS = RUN / "data" / "research_bars.csv"
MANIFEST = RUN / "data" / "research_bars_provenance.json"
INDEX = RUN / "trials_index.json"
GAP_TOLERANCE_BARS = 3  # infrastructure only: NYSE holidays are missing weekdays, not data gaps


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


def _load_alpaca_env() -> None:
    """Load only the two research-data credential keys from .env.local (values are never printed)."""
    want = {"ALPACA_API_KEY_PAPER", "ALPACA_API_SECRET_PAPER"}
    if all(os.environ.get(k) for k in want):
        return
    for line in (REPO / ".env.local").read_text(encoding="utf-8").splitlines():
        k, _, v = line.partition("=")
        if k.strip() in want and v.strip():
            os.environ[k.strip()] = v.strip().strip('"').strip("'")
    missing = [k for k in sorted(want) if not os.environ.get(k)]
    if missing:
        raise SystemExit(f"credentials unavailable: {missing}")


def stage_check(_args) -> None:
    assert len(SYMBOLS) == DECL["universe"]["max_trials"] == 5
    print(f"campaign={DECL['campaign_id']} symbols={SYMBOLS} strategy={STRATEGY} cli_present={CLI.exists()}")


def stage_reuse_data(_args) -> None:
    """Byte-identical reuse of a previously fetched, provenance-verified bars set."""
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


def stage_seed_registry(_args) -> None:
    """Byte-identical copy of the prior campaign's registry so its trial history stays in the judge population."""
    import shutil
    from mqk_research.ml.util_hash import sha256_file
    src = HERE / DECL["experiment"]["seed_registry_copy_from"]
    if REGISTRY.exists():
        raise SystemExit("fail-closed: registry already exists; the seed copy runs once")
    REGISTRY.parent.mkdir(parents=True, exist_ok=True)
    shutil.copyfile(src, REGISTRY)
    if sha256_file(src) != sha256_file(REGISTRY):
        raise SystemExit("fail-closed: seed registry copy does not match its source")
    print("seeded registry from", src.name, sha256_file(REGISTRY)[:12])


def stage_fetch(args) -> None:
    if not args.execute:
        raise SystemExit("fetch contacts the data provider; pass --execute")
    from mqk_research.data.alpaca_historical import (
        CorporateActionReviewRequired, extract_research_bars_with_provenance, write_research_extraction_artifacts)
    _load_alpaca_env()
    d = DECL["data"]
    windows = [d["end_utc"], d["fallback_if_corporate_action_gate_refuses"]["end_utc"]]
    result = None
    for end in windows:
        try:
            result = extract_research_bars_with_provenance(
                symbols=SYMBOLS, start_utc=pd.Timestamp(d["start_utc"]), end_utc=pd.Timestamp(end),
                asof=d["asof"], timeframe=d["timeframe"], feed=d["feed"])
            print(f"fetched end_utc={end}")
            break
        except CorporateActionReviewRequired as exc:
            print(f"corporate-action gate refused end_utc={end}: {type(exc).__name__}")
    if result is None:
        raise SystemExit("provider capability unavailable for every predeclared window")
    paths = write_research_extraction_artifacts(RUN / "data", result)
    print("rows", len(result["bars"]), {k: v.name for k, v in paths.items()})


def _run_cli(*argv: str) -> str:
    out = subprocess.run([str(CLI), *argv], capture_output=True, text=True)
    if out.returncode != 0:
        raise SystemExit(f"mqk-cli failed ({argv[:2]}): {out.stderr[-800:]}")
    return out.stdout


def stage_trials(_args) -> None:
    from mqk_research.ml.native_signal_registry_integration import (
        NativeSignalError, native_holdout_start, research_bars_to_backtest_csv, run_registered_native_signal_economic_eval)
    part, manifest = DECL["partition"], json.loads(MANIFEST.read_text(encoding="utf-8"))
    index = {}
    for sym in SYMBOLS:  # every symbol, in fixed order; no result-dependent skipping
        sdir = RUN / "trials" / sym
        sdir.mkdir(parents=True, exist_ok=True)
        hold = native_holdout_start(BARS, sym, part["holdout_months"])
        bt = research_bars_to_backtest_csv(BARS, sym, sdir / "bt_bars.csv", end_exclusive_utc=hold)
        _run_cli("backtest", "native-signals", "--bars-path", str(bt), "--strategy", STRATEGY, "--symbol", sym,
                 "--timeframe-secs", str(DECL["native_engine"]["timeframe_secs"]), "--out-dir", str(sdir / "emit"))
        try:
            out = run_registered_native_signal_economic_eval(
                sdir / "run", experiment_id=EXPERIMENT, hypothesis_id=HYPOTHESIS, strategy_id=STRATEGY, symbol=sym,
                bars_csv=BARS, bars_provenance=manifest, backtest_bars_csv=bt,
                signals_csv=sdir / "emit" / "native_signals.csv", signals_meta_json=sdir / "emit" / "native_signals_meta.json",
                economic_spec=_economic_spec(), evaluation_start_utc=pd.Timestamp(part["evaluation_start_utc"]),
                test_months=part["test_months"], holdout_months=part["holdout_months"],
                hypothesis_text=DECL["hypothesis"]["economic_rationale"], registry_db=REGISTRY,
                expected_timeframe_secs=DECL["native_engine"]["timeframe_secs"])
        except NativeSignalError as exc:  # the failed attempt is already durable; register every symbol
            index[sym] = {"failed": str(exc)}
            print(sym, "FAILED attempt kept:", str(exc)[:160])
            continue
        econ = json.loads(out.read_text(encoding="utf-8"))
        index[sym] = {"trial_id": econ["registry"]["trial_id"], "economic_eval_id": econ["ids"]["economic_eval_id"],
                      "economic_path": str(out), "attempt_index": econ["registry"]["attempt_index"],
                      "net_total_return": econ["aggregate"].get("net_total_return")}
        print(sym, index[sym]["trial_id"], "attempt", index[sym]["attempt_index"])
    INDEX.write_text(json.dumps(index, indent=1, sort_keys=True), encoding="utf-8")


def stage_judge(_args) -> None:
    from mqk_research.exp_distributed.hashing import canonical_json, sha256_bytes
    from mqk_research.ml.multiple_testing_judge import build_multiple_testing_judge
    from mqk_research.exp_distributed.storage import ResearchResultStore
    trials = ResearchResultStore(REGISTRY).list_trials(experiment_id=EXPERIMENT)
    seed = DECL["experiment"].get("seed_registry_copy_from")
    prior = len(ResearchResultStore(HERE / seed).list_trials(experiment_id=EXPERIMENT)) if seed else 0
    if len(trials) != prior + len(SYMBOLS):
        raise SystemExit(f"fail-closed: {len(trials)} registered trials, expected {prior + len(SYMBOLS)}")
    art = build_multiple_testing_judge(experiment_id=EXPERIMENT, registry_db=REGISTRY)
    (RUN / "judge").mkdir(parents=True, exist_ok=True)
    path = RUN / "judge" / "judge.json"
    path.write_text(json.dumps(art, sort_keys=True, separators=(",", ":")), encoding="utf-8")
    sha = sha256_bytes(canonical_json(art).encode("utf-8"))
    (RUN / "judge" / "judge_sha256.txt").write_text(sha, encoding="utf-8")
    print("judge_status", art["judge_status"], "included", len(art["included_trial_ids"]), "excluded", art["excluded_trial_ids"], "sha", sha[:12])


def _parse(out: str, key: str) -> str:
    m = re.search(rf"^{key}=(.+)$", out, re.M)
    if not m:
        raise SystemExit(f"missing {key} in cli output")
    return m.group(1).strip()


def stage_backtest(_args) -> None:
    index = json.loads(INDEX.read_text(encoding="utf-8"))
    nb = DECL["native_backtest"]
    for sym in SYMBOLS:
        if "failed" in index[sym]:
            print(sym, "no succeeded trial; rejected, no backtest evidence")
            continue
        out = _run_cli("backtest", "csv", "--bars", str(RUN / "trials" / sym / "bt_bars.csv"), "--strategy", STRATEGY,
                       "--symbol", sym, "--timeframe-secs", str(nb["timeframe_secs"]),
                       "--initial-cash-micros", str(nb["initial_cash_micros"]),
                       "--integrity-calendar", "us-equity-regular", "--integrity-stale-threshold-ticks", "259200",
                       "--integrity-gap-tolerance-bars", str(GAP_TOLERANCE_BARS),
                       "--out-dir", str(RUN / "backtest" / sym))
        index[sym]["backtest_run_id"] = _parse(out, "run_id")
        index[sym]["execution_blocked"] = _parse(out, "execution_blocked")
        print(sym, index[sym]["backtest_run_id"], "execution_blocked", index[sym]["execution_blocked"])
    INDEX.write_text(json.dumps(index, indent=1, sort_keys=True), encoding="utf-8")


def stage_finalize(_args) -> None:
    index = json.loads(INDEX.read_text(encoding="utf-8"))
    sha = (RUN / "judge" / "judge_sha256.txt").read_text(encoding="utf-8").strip()
    rb = DECL["robustness"]
    st = rb["p7a_p7b_stress"]
    py = sys.executable
    for sym in SYMBOLS:
        rec = index[sym]
        if "failed" in rec:
            continue
        common = ["--artifact-root", str(RUN / "backtest" / sym), "--run-id", rec["backtest_run_id"],
                  "--registry-db", str(REGISTRY), "--trial-id", rec["trial_id"]]
        _run_cli("backtest", "finalize-robustness-sensitivity", *common, "--judge-artifact-sha256", sha,
                 "--research-py-root", str(REPO / "research-py"), "--python", py,
                 "--block-counts", ",".join(map(str, rb["block_counts"])),
                 "--dsr-max-sensitivity-range", str(rb["dsr_max_sensitivity_range"]),
                 "--pbo-max-sensitivity-range", str(rb["pbo_max_sensitivity_range"]))
        _run_cli("backtest", "finalize-p7a-p7b-replay-stress", *common, "--economic-eval-id", rec["economic_eval_id"],
                 "--research-py-root", str(REPO / "research-py"), "--python", py,
                 "--stress-out-dir", str(RUN / "stress" / sym),
                 "--stress-execution-slippage-bps", str(st["stress_execution_slippage_bps"]),
                 "--stress-execution-volatility-mult-bps", str(st["stress_execution_volatility_mult_bps"]),
                 "--stress-max-position-notional-usd", str(st["stress_max_position_notional_usd"]),
                 "--max-drawdown-ceiling", str(st["max_drawdown_ceiling"]))
        _run_cli("backtest", "finalize-genuine-shuffled-placebo", *common, "--economic-eval-id", rec["economic_eval_id"],
                 "--research-py-root", str(REPO / "research-py"), "--python", py,
                 "--placebo-out-dir", str(RUN / "placebo" / sym))
        print(sym, "finalized")


def stage_review(_args) -> None:
    reg = json.loads((REPO / "config" / "instruments" / "equities.json").read_text(encoding="utf-8"))
    sub = [i for i in reg if i["symbol"] in SYMBOLS]
    assert sorted(i["symbol"] for i in sub) == sorted(SYMBOLS)
    (RUN / "scan").mkdir(parents=True, exist_ok=True)
    reg_path = RUN / "scan" / "registry.json"
    reg_path.write_text(json.dumps(sub, indent=1, sort_keys=True), encoding="utf-8")
    root = RUN / "scan" / "bars" / "1D"
    root.mkdir(parents=True, exist_ok=True)
    for sym in SYMBOLS:
        (root / f"{sym}_1D.csv").write_bytes((RUN / "trials" / sym / "bt_bars.csv").read_bytes())
    out = _run_cli("backtest", "scan-strategies", "--registry", str(reg_path), "--bars-root", str(RUN / "scan" / "bars"),
                   "--timeframe", "1D", "--strategy", STRATEGY, "--out-dir", str(RUN / "scan" / "scans"))
    scan_dir = _parse(out, "artifacts_dir")
    out = _run_cli("backtest", "review-scan", "--artifact-dir", scan_dir, "--out-dir", str(RUN / "scan" / "reviews"))
    print(out)


def stage_summary(_args) -> None:
    print(INDEX.read_text(encoding="utf-8"))


STAGES = {"check": stage_check, "fetch": stage_fetch, "reuse_data": stage_reuse_data, "seed_registry": stage_seed_registry, "trials": stage_trials, "judge": stage_judge,
          "backtest": stage_backtest, "finalize": stage_finalize, "review": stage_review, "summary": stage_summary}


def main() -> None:
    # This runner drove native bridge v1 (the native +1-share target turned into a binary weight and
    # re-sized) for the closed campaigns. Their evidence is HISTORICAL / SUPERSEDED_PROTOCOL /
    # NOT_PROMOTION_AUTHORITY, and the v1 bridge entry point no longer exists.
    raise SystemExit(
        "fail-closed: run_campaign.py drove the superseded native bridge v1; its evidence is historical and "
        "not promotion authority. Use run_batch.py with a new predeclaration under native_exact_target_qty_v1 "
        "and separate authorization."
    )
    ap = argparse.ArgumentParser()
    ap.add_argument("stage", choices=sorted(STAGES))
    ap.add_argument("--execute", action="store_true")
    args = ap.parse_args()
    STAGES[args.stage](args)


if __name__ == "__main__":
    main()
