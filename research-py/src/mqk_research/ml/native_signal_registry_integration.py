"""Registered economic evaluation of a NATIVE strategy's signal stream.

The executable strategy (Rust) stays the single source of truth for decisions:
`mqk backtest native-signals` runs it through the real BacktestEngine and
emits its per-bar targets. This module registers that stream as the OOS
decision series of a predeclared Research trial and evaluates it with the
unmodified `run_economic_walkforward` -- the same economic protocol, official
execution-pricing/weight-to-share parity and holdout reservation as every
classifier trial. Nothing here re-implements a trading rule.

Evaluation is out-of-sample by construction only because the rule has no fitted
parameters: its constants are predeclared (and bound into the native semantic
fingerprint) before any result exists. Trial identity is result-independent;
a retry is a new attempt of the SAME trial.
"""

from __future__ import annotations

import csv
import json
from dataclasses import dataclass
from pathlib import Path
from typing import Any, Dict, List, Optional, Tuple

import pandas as pd

from mqk_research.data.bars_provenance import (
    provenance_identity_fragment,
    require_registered_bars_provenance,
)
from mqk_research.exp_distributed.hashing import short_hash
from mqk_research.exp_distributed.runner import default_db_path, default_root
from mqk_research.exp_distributed.storage import REGISTRY_SCHEMA_VERSION, ResearchResultStore
from mqk_research.ml.economic_registry_integration import (
    ECONOMIC_PROTOCOL_ID,
    require_official_execution_pricing_parity,
    require_official_weight_to_share_parity,
)
from mqk_research.ml.economic_walkforward import (
    EconomicWalkForwardSpec,
    economic_protocol_identity,
    run_economic_walkforward,
)
from mqk_research.ml.eval_walkforward import compute_holdout_boundary
from mqk_research.ml.holdout_ledger import compute_holdout_id
from mqk_research.ml.util_hash import file_record, sha256_file

NATIVE_SIGNAL_STREAM_PROTOCOL_ID = "native_strategy_signal_stream_v1"
NATIVE_SIGNAL_SOURCE_KIND = "native_strategy_signal_stream_v1"
_MICROS = 1_000_000

__all__ = [
    "NATIVE_SIGNAL_STREAM_PROTOCOL_ID",
    "NativeSignalError",
    "build_native_signal_trial_identity",
    "native_holdout_start",
    "plan_native_folds",
    "research_bars_to_backtest_csv",
    "run_registered_native_signal_economic_eval",
]


class NativeSignalError(RuntimeError):
    """Fail-closed refusal to register or evaluate a native signal stream."""


@dataclass(frozen=True)
class NativeFold:
    fold: int
    test_start: pd.Timestamp
    test_end: pd.Timestamp


def research_bars_to_backtest_csv(
    bars_csv: Path,
    symbol: str,
    out_csv: Path,
    *,
    end_exclusive_utc: Optional[pd.Timestamp] = None,
) -> Path:
    """Deterministic conversion of the provenance-bound research bars for
    `symbol` into the Rust backtest loader format: epoch-second `end_ts` and
    integer micros with `micros = round(price * 1e6)`. Identical on every run.
    `end_exclusive_utc` drops every bar at or after that instant -- the native
    bridge always passes the reserved holdout start so no holdout-period bar
    ever reaches the Rust emitter, backtest or scanner."""
    bars = pd.read_csv(bars_csv)
    bars = bars[bars["symbol"].astype(str) == symbol].copy()
    if bars.empty:
        raise NativeSignalError(f"no bars for symbol {symbol!r} in {bars_csv}")
    bars["end_ts"] = pd.to_datetime(bars["end_ts"], utc=True)
    if end_exclusive_utc is not None:
        bars = bars[bars["end_ts"] < pd.Timestamp(end_exclusive_utc)]
        if bars.empty:
            raise NativeSignalError("no bars remain before the exclusive end")
    bars = bars.sort_values("end_ts", kind="mergesort")
    if bars["end_ts"].duplicated().any():
        raise NativeSignalError("duplicate end_ts rows for symbol")
    out = pd.DataFrame(
        {
            "symbol": symbol,
            "end_ts": ((bars["end_ts"] - pd.Timestamp("1970-01-01", tz="UTC")) // pd.Timedelta(seconds=1)).astype("int64"),
            "open_micros": (bars["open"].astype(float) * _MICROS).round().astype("int64"),
            "high_micros": (bars["high"].astype(float) * _MICROS).round().astype("int64"),
            "low_micros": (bars["low"].astype(float) * _MICROS).round().astype("int64"),
            "close_micros": (bars["close"].astype(float) * _MICROS).round().astype("int64"),
            "volume": bars["volume"].astype(float).round().astype("int64"),
        }
    )
    out.to_csv(out_csv, index=False, lineterminator="\n")
    return Path(out_csv)


def plan_native_folds(
    *,
    t_min: pd.Timestamp,
    t_max: pd.Timestamp,
    evaluation_start_utc: pd.Timestamp,
    test_months: int,
    holdout_months: int,
) -> Tuple[List[NativeFold], pd.Timestamp, pd.Timestamp]:
    """Contiguous, non-overlapping test windows from `evaluation_start_utc`
    up to the reserved holdout. Returns (folds, holdout_start, dataset_end).
    The holdout boundary is the one the classifier protocol uses."""
    if int(test_months) <= 0 or int(holdout_months) <= 0:
        raise NativeSignalError("test_months and holdout_months must be > 0")
    _anchor, holdout_start, dataset_end = compute_holdout_boundary(t_min, t_max, int(holdout_months))
    start = pd.Timestamp(evaluation_start_utc)
    if start.tzinfo is None:
        start = start.tz_localize("UTC")
    folds: List[NativeFold] = []
    cursor = start
    while True:
        end = cursor + pd.DateOffset(months=int(test_months))
        if end > holdout_start:
            break
        folds.append(NativeFold(fold=len(folds) + 1, test_start=cursor, test_end=end))
        cursor = end
    if not folds:
        raise NativeSignalError("no evaluation fold fits before the reserved holdout")
    return folds, holdout_start, dataset_end


def native_holdout_start(bars_csv: Path, symbol: str, holdout_months: int) -> pd.Timestamp:
    """The reserved holdout start the bridge derives for `symbol`'s bars."""
    bars = pd.read_csv(bars_csv)
    ts = pd.to_datetime(bars[bars["symbol"].astype(str) == symbol]["end_ts"], utc=True)
    if ts.empty:
        raise NativeSignalError(f"no bars for symbol {symbol!r} in {bars_csv}")
    return compute_holdout_boundary(ts.min(), ts.max(), int(holdout_months))[1]


def _load_signals(
    signals_csv: Path,
    meta_json: Path,
    *,
    strategy_id: str,
    symbol: str,
    backtest_bars_sha256: str,
) -> Tuple[pd.DataFrame, Dict[str, Any]]:
    meta = json.loads(Path(meta_json).read_text(encoding="utf-8"))
    if meta.get("protocol_id") != NATIVE_SIGNAL_STREAM_PROTOCOL_ID:
        raise NativeSignalError(f"unsupported native signal protocol: {meta.get('protocol_id')!r}")
    if meta.get("strategy_name") != strategy_id:
        raise NativeSignalError(
            f"signal stream strategy {meta.get('strategy_name')!r} != trial strategy_id {strategy_id!r}"
        )
    if meta.get("symbol") != symbol:
        raise NativeSignalError(f"signal stream symbol {meta.get('symbol')!r} != {symbol!r}")
    fp = str(meta.get("semantic_fingerprint", ""))
    if len(fp) != 64 or any(c not in "0123456789abcdef" for c in fp):
        raise NativeSignalError("signal stream semantic_fingerprint is not 64 lowercase hex")
    if meta.get("native_signals_csv_sha256") != sha256_file(Path(signals_csv)):
        raise NativeSignalError("signal stream csv does not match its recorded sha256")
    if meta.get("bars_csv_sha256") != backtest_bars_sha256:
        raise NativeSignalError(
            "signals were not emitted over the bars derived from this trial's research bars"
        )
    df = pd.read_csv(signals_csv)
    if list(df.columns) != ["symbol", "decision_ts", "target_qty_micros"]:
        raise NativeSignalError("unexpected native signal csv columns")
    if int(meta.get("signal_rows", -1)) != len(df) or df.empty:
        raise NativeSignalError("signal row count disagrees with meta or is empty")
    if (df["symbol"].astype(str) != symbol).any():
        raise NativeSignalError("signal stream contains a different symbol")
    if df["decision_ts"].duplicated().any():
        raise NativeSignalError("duplicate decision_ts in signal stream")
    q = df["target_qty_micros"].astype("int64")
    if ((q != 0) & (q != _MICROS)).any():
        raise NativeSignalError("protocol v1 research bridge supports only flat/+1 share targets")
    df = df.sort_values("decision_ts", kind="mergesort").reset_index(drop=True)
    df["decision_ts_utc"] = pd.to_datetime(df["decision_ts"].astype("int64"), unit="s", utc=True)
    df["ml_score"] = (df["target_qty_micros"].astype("int64") > 0).astype(float)
    return df, meta


def build_native_signal_trial_identity(
    *,
    experiment_id: str,
    hypothesis_id: str,
    strategy_id: str,
    symbol: str,
    semantic_fingerprint: str,
    bars_provenance: Dict[str, Any],
    evaluation_start_utc: pd.Timestamp,
    test_months: int,
    holdout_months: int,
    economic_spec: EconomicWalkForwardSpec,
) -> Tuple[str, Dict[str, Any]]:
    """Result-independent identity: strategy semantics, data provenance,
    partition policy and economic protocol only. Mirrors the keys the judge's
    comparison scope reads for classifier trials."""
    spec = economic_spec.normalized()
    identity: Dict[str, Any] = {
        "experiment_id": experiment_id,
        "hypothesis_id": hypothesis_id,
        "strategy_id": strategy_id,
        "protocol_id": ECONOMIC_PROTOCOL_ID,
        "data_identity": {
            "bars_provenance": provenance_identity_fragment(bars_provenance),
        },
        "signal_source": {
            "kind": NATIVE_SIGNAL_SOURCE_KIND,
            "symbol": symbol,
            "semantic_fingerprint": semantic_fingerprint,
        },
        "evaluation_spec": {
            "signal_source": NATIVE_SIGNAL_SOURCE_KIND,
            "evaluation_start_utc": pd.Timestamp(evaluation_start_utc).isoformat(),
            "test_months": int(test_months),
            "holdout_months": int(holdout_months),
        },
        "economic_protocol": economic_protocol_identity(spec),
    }
    if spec.execution_pricing.is_official_parity_model:
        identity["data_identity"]["bars_pricing_provenance"] = {
            "canonical_pricing_bars_hash": bars_provenance.get("canonical_pricing_bars_hash"),
        }
    return short_hash(identity, length=32), identity


def run_registered_native_signal_economic_eval(
    run_dir: Path,
    *,
    experiment_id: str,
    hypothesis_id: str,
    strategy_id: str,
    symbol: str,
    bars_csv: Path,
    bars_provenance: Dict[str, Any],
    backtest_bars_csv: Path,
    signals_csv: Path,
    signals_meta_json: Path,
    economic_spec: EconomicWalkForwardSpec,
    evaluation_start_utc: pd.Timestamp,
    test_months: int = 12,
    holdout_months: int = 6,
    hypothesis_text: Optional[str] = None,
    registry_db: Optional[Path] = None,
) -> Path:
    """Official registered entry point for a native strategy's signals.
    Order mirrors the classifier path: identity -> trial -> attempt (BEFORE
    evaluation) -> economic evaluation -> holdout reservation -> attempt
    finalized. Any failure after the attempt starts finalizes it `failed`."""
    for name, value in (("experiment_id", experiment_id), ("hypothesis_id", hypothesis_id),
                        ("strategy_id", strategy_id), ("symbol", symbol)):
        if not str(value).strip():
            raise ValueError(f"{name} is required")
    require_registered_bars_provenance(bars_provenance)
    spec = economic_spec.normalized()
    require_official_execution_pricing_parity(spec)
    require_official_weight_to_share_parity(spec)

    run_dir = Path(run_dir)
    bars_csv = Path(bars_csv)
    backtest_bars_csv = Path(backtest_bars_csv)
    for p in (bars_csv, backtest_bars_csv, Path(signals_csv), Path(signals_meta_json)):
        if not p.exists():
            raise FileNotFoundError(f"Missing required native-signal input: {p}")

    bars = pd.read_csv(bars_csv)
    bars = bars[bars["symbol"].astype(str) == symbol]
    bar_ts = pd.to_datetime(bars["end_ts"], utc=True)
    folds, holdout_start, dataset_end = plan_native_folds(
        t_min=bar_ts.min(), t_max=bar_ts.max(),
        evaluation_start_utc=evaluation_start_utc,
        test_months=test_months, holdout_months=holdout_months,
    )

    # The backtest CSV must be exactly the deterministic conversion of the
    # provenance-bound research bars, truncated at the reserved holdout start
    # -- never an independently supplied file, never containing a holdout bar.
    check_csv = run_dir / "backtest_bars_check.csv"
    run_dir.mkdir(parents=True, exist_ok=True)
    research_bars_to_backtest_csv(bars_csv, symbol, check_csv, end_exclusive_utc=holdout_start)
    expected_sha = sha256_file(check_csv)
    if sha256_file(backtest_bars_csv) != expected_sha:
        raise NativeSignalError(
            "backtest bars csv is not the holdout-truncated conversion of the research bars"
        )

    signals, meta = _load_signals(
        signals_csv, signals_meta_json, strategy_id=strategy_id, symbol=symbol,
        backtest_bars_sha256=expected_sha,
    )

    trial_id, identity = build_native_signal_trial_identity(
        experiment_id=experiment_id, hypothesis_id=hypothesis_id, strategy_id=strategy_id,
        symbol=symbol, semantic_fingerprint=meta["semantic_fingerprint"],
        bars_provenance=bars_provenance, evaluation_start_utc=evaluation_start_utc,
        test_months=test_months, holdout_months=holdout_months, economic_spec=spec,
    )

    store = ResearchResultStore(registry_db or default_db_path(default_root()))
    store.register_hypothesis(
        hypothesis_id=hypothesis_id, experiment_id=experiment_id, hypothesis_text=hypothesis_text
    )
    store.register_trial(
        trial_id=trial_id, experiment_id=experiment_id, hypothesis_id=hypothesis_id,
        strategy_id=strategy_id, protocol_id=ECONOMIC_PROTOCOL_ID, identity=identity,
    )
    attempt_id, attempt_index = store.begin_attempt(
        trial_id=trial_id, origin="mqk-native-signal-economic-wf",
        metadata={"native_signals_csv_sha256": meta["native_signals_csv_sha256"]},
    )

    try:
        eval_dir = run_dir / "eval"
        eval_dir.mkdir(parents=True, exist_ok=True)

        rows: List[Dict[str, Any]] = []
        for f in folds:
            in_fold = signals[
                (signals["decision_ts_utc"] >= f.test_start) & (signals["decision_ts_utc"] < f.test_end)
            ]
            for _, r in in_fold.iterrows():
                rows.append({
                    "fold": f.fold, "symbol": symbol,
                    "decision_ts": r["decision_ts_utc"].isoformat(), "ml_score": float(r["ml_score"]),
                })
            if in_fold.empty:
                raise NativeSignalError(f"no native signals inside fold {f.fold}")
        oos_path = eval_dir / "walk_forward_oos_predictions.csv"
        with open(oos_path, "w", newline="", encoding="utf-8") as fh:
            w = csv.DictWriter(fh, fieldnames=["fold", "symbol", "decision_ts", "ml_score"], lineterminator="\n")
            w.writeheader()
            w.writerows(rows)

        wf_out = {
            "schema_version": "walk_forward_eval_v2",
            "signal_source": NATIVE_SIGNAL_SOURCE_KIND,
            "spec": {"test_months": int(test_months), "holdout_months": int(holdout_months),
                     "evaluation_start_utc": pd.Timestamp(evaluation_start_utc).isoformat()},
            "holdout": {
                "status": "reserved_not_evaluated",
                "start_utc": holdout_start.isoformat(),
                "end_utc": dataset_end.isoformat(),
            },
            "inputs": {
                "native_signals_csv": file_record(Path(signals_csv)),
                "native_signals_meta": file_record(Path(signals_meta_json)),
            },
            "outputs": {"oos_predictions_csv": file_record(oos_path)},
            "folds": [
                {"fold": f.fold, "test_start_utc": f.test_start.isoformat(),
                 "test_end_utc": f.test_end.isoformat(), "skipped": False}
                for f in folds
            ],
        }
        wf_path = eval_dir / "walk_forward_eval.json"
        wf_path.write_text(json.dumps(wf_out, sort_keys=True, separators=(",", ":")), encoding="utf-8")

        economic_out_path = run_economic_walkforward(
            run_dir, bars_csv=bars_csv, spec=spec, walk_forward_eval_path=wf_path,
            oos_predictions_path=oos_path, provenance_manifest=bars_provenance,
        )
        economic_out = json.loads(economic_out_path.read_text(encoding="utf-8"))

        holdout_id = compute_holdout_id(
            dataset_identity=identity["data_identity"],
            holdout_start_utc=wf_out["holdout"]["start_utc"],
            holdout_end_utc=wf_out["holdout"]["end_utc"],
            protocol_version=ECONOMIC_PROTOCOL_ID,
        )
        store.reserve_holdout(
            holdout_id=holdout_id, dataset_identity=identity["data_identity"],
            holdout_start_utc=wf_out["holdout"]["start_utc"],
            holdout_end_utc=wf_out["holdout"]["end_utc"], protocol_version=ECONOMIC_PROTOCOL_ID,
        )
    except Exception as exc:
        store.finalize_attempt(attempt_id, status="failed", failure_reason=f"{type(exc).__name__}: {exc}")
        raise

    economic_eval_id = economic_out["ids"]["economic_eval_id"]
    economic_out["registry"] = {
        "schema_version": REGISTRY_SCHEMA_VERSION, "experiment_id": experiment_id,
        "hypothesis_id": hypothesis_id, "strategy_id": strategy_id, "trial_id": trial_id,
        "attempt_id": attempt_id, "attempt_index": attempt_index, "status": "succeeded",
        "holdout_id": holdout_id,
        "signal_source": NATIVE_SIGNAL_SOURCE_KIND,
        "semantic_fingerprint": meta["semantic_fingerprint"],
    }
    economic_out_path.write_text(json.dumps(economic_out, sort_keys=True, separators=(",", ":")), encoding="utf-8")
    store.finalize_attempt(
        attempt_id, status="succeeded", result_id=economic_eval_id,
        artifact_paths={
            "walk_forward_eval": str(wf_path), "economic_walk_forward": str(economic_out_path),
            "native_signals": str(signals_csv), "native_signals_meta": str(signals_meta_json),
        },
        result_summary=economic_out.get("aggregate"),
    )
    return economic_out_path
