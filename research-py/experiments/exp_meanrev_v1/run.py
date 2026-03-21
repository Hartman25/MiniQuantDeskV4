from __future__ import annotations

import argparse
import hashlib
import json
import math
import sys
from dataclasses import dataclass
from pathlib import Path
from typing import Any

import pandas as pd
import yaml

EXPERIMENT_DIR = Path(__file__).resolve().parent
RESEARCH_ROOT = EXPERIMENT_DIR.parents[1]
SRC_ROOT = RESEARCH_ROOT / "src"
if str(SRC_ROOT) not in sys.path:
    sys.path.insert(0, str(SRC_ROOT))

from mqk_research.contracts import CONTRACT_VERSION, ResearchManifest  # noqa: E402


_ALLOWED_TOP_LEVEL_KEYS = {"name", "engine", "strategy", "execution", "inputs", "outputs", "notes"}
_ALLOWED_ENGINE_KEYS = {
    "engine_id",
    "canonical",
    "readiness_bearing",
    "operator_visible",
    "capital_authoritative",
    "invocation",
}
_ALLOWED_STRATEGY_KEYS = {"family", "description", "params"}
_ALLOWED_STRATEGY_PARAM_KEYS = {"lookback", "zscore_entry", "zscore_exit"}
_ALLOWED_EXECUTION_KEYS = {"fees"}
_ALLOWED_EXECUTION_FEE_KEYS = {"spread_bps", "slippage_bps"}
_ALLOWED_INPUT_KEYS = {"sample_bars_csv", "asof_utc", "timeframe", "symbol"}
_ALLOWED_OUTPUT_KEYS = {
    "root_dir",
    "write_manifest",
    "write_signal_pack",
    "write_trades",
    "write_equity_curve",
    "write_metrics",
}


@dataclass(frozen=True)
class ExperimentSpec:
    name: str
    engine_id: str
    symbol: str
    asof_utc: str
    timeframe: str
    lookback: int
    zscore_entry: float
    zscore_exit: float
    spread_bps: float
    slippage_bps: float
    sample_bars_csv: str
    output_root_dir: str


@dataclass(frozen=True)
class PipelineResult:
    run_id: str
    run_dir: Path
    bars_path: Path
    config_path: Path
    bars: pd.DataFrame
    signals: pd.DataFrame
    trades: pd.DataFrame
    equity_curve: pd.DataFrame
    metrics: dict[str, Any]
    manifest: ResearchManifest
    resolved_config: dict[str, Any]
    artifact_index: dict[str, Any]


def _stable_json_bytes(obj: Any) -> bytes:
    return json.dumps(obj, sort_keys=True, separators=(",", ":"), ensure_ascii=False).encode("utf-8")


def _json_text(obj: Any) -> str:
    return json.dumps(obj, indent=2, sort_keys=True, ensure_ascii=False) + "\n"


def _sha256_bytes(payload: bytes) -> str:
    return hashlib.sha256(payload).hexdigest()


def _sha256_file(path: Path) -> str:
    return _sha256_bytes(path.read_bytes())


def _load_config(config_path: Path) -> dict[str, Any]:
    with config_path.open("r", encoding="utf-8") as fh:
        data = yaml.safe_load(fh)
    if not isinstance(data, dict):
        raise ValueError(f"Config at {config_path} must load to a mapping.")
    return data


def _require_mapping(parent: dict[str, Any], key: str) -> dict[str, Any]:
    value = parent.get(key)
    if not isinstance(value, dict):
        raise ValueError(f"Expected '{key}' to be a mapping.")
    return value


def _require_exact_keys(mapping: dict[str, Any], *, allowed: set[str], context: str) -> None:
    extra = sorted(set(mapping.keys()) - allowed)
    missing = sorted(allowed - set(mapping.keys()))
    if extra:
        raise ValueError(f"{context} has unsupported keys: {extra}")
    if missing:
        raise ValueError(f"{context} is missing required keys: {missing}")


def _validate_exp_only(config: dict[str, Any]) -> None:
    _require_exact_keys(config, allowed=_ALLOWED_TOP_LEVEL_KEYS, context="config")

    if config.get("name") != "exp_meanrev_v1":
        raise ValueError("This experiment only supports name=exp_meanrev_v1.")

    notes = config.get("notes")
    if not isinstance(notes, list) or not notes or not all(isinstance(note, str) and note.strip() for note in notes):
        raise ValueError("config.notes must be a non-empty list of non-empty strings.")

    engine = _require_mapping(config, "engine")
    _require_exact_keys(engine, allowed=_ALLOWED_ENGINE_KEYS, context="engine")
    required_false = ["canonical", "readiness_bearing", "operator_visible", "capital_authoritative"]
    if engine.get("engine_id") != "EXP":
        raise ValueError("EXP experiment requires engine.engine_id=EXP.")
    for key in required_false:
        if engine.get(key) is not False:
            raise ValueError(f"EXP experiment requires engine.{key}=false.")
    if engine.get("invocation") != "explicit_local_only":
        raise ValueError("EXP experiment must remain explicit_local_only.")

    strategy = _require_mapping(config, "strategy")
    _require_exact_keys(strategy, allowed=_ALLOWED_STRATEGY_KEYS, context="strategy")
    params = _require_mapping(strategy, "params")
    _require_exact_keys(params, allowed=_ALLOWED_STRATEGY_PARAM_KEYS, context="strategy.params")
    if strategy.get("family") != "mean_reversion":
        raise ValueError("This experiment is fixed to family=mean_reversion.")
    if not isinstance(strategy.get("description"), str) or not strategy["description"].strip():
        raise ValueError("strategy.description must be a non-empty string.")
    lookback = int(params["lookback"])
    zscore_entry = float(params["zscore_entry"])
    zscore_exit = float(params["zscore_exit"])
    if lookback < 2:
        raise ValueError("strategy.params.lookback must be >= 2.")
    if zscore_entry <= 0:
        raise ValueError("strategy.params.zscore_entry must be > 0.")
    if zscore_exit < 0:
        raise ValueError("strategy.params.zscore_exit must be >= 0.")
    if zscore_exit >= zscore_entry:
        raise ValueError("strategy.params.zscore_exit must be smaller than zscore_entry.")

    execution = _require_mapping(config, "execution")
    _require_exact_keys(execution, allowed=_ALLOWED_EXECUTION_KEYS, context="execution")
    fees = _require_mapping(execution, "fees")
    _require_exact_keys(fees, allowed=_ALLOWED_EXECUTION_FEE_KEYS, context="execution.fees")
    for field in ("spread_bps", "slippage_bps"):
        value = float(fees[field])
        if value < 0:
            raise ValueError(f"execution.fees.{field} must be >= 0.")

    inputs = _require_mapping(config, "inputs")
    _require_exact_keys(inputs, allowed=_ALLOWED_INPUT_KEYS, context="inputs")
    sample_bars_csv = inputs.get("sample_bars_csv")
    if not isinstance(sample_bars_csv, str) or not sample_bars_csv.endswith(".csv"):
        raise ValueError("inputs.sample_bars_csv must be a CSV filename.")
    sample_bars_path = Path(sample_bars_csv)
    if sample_bars_path.is_absolute() or sample_bars_path.parent != Path("."):
        raise ValueError("inputs.sample_bars_csv must be a local filename in the experiment directory.")
    for field in ("asof_utc", "timeframe", "symbol"):
        value = inputs.get(field)
        if not isinstance(value, str) or not value.strip():
            raise ValueError(f"inputs.{field} must be a non-empty string.")

    outputs = _require_mapping(config, "outputs")
    _require_exact_keys(outputs, allowed=_ALLOWED_OUTPUT_KEYS, context="outputs")
    root_dir = outputs.get("root_dir")
    if not isinstance(root_dir, str) or not root_dir.startswith("runs/EXP/exp_meanrev_v1"):
        raise ValueError("outputs.root_dir must start with 'runs/EXP/exp_meanrev_v1'.")
    for field in (
        "write_manifest",
        "write_signal_pack",
        "write_trades",
        "write_equity_curve",
        "write_metrics",
    ):
        if not isinstance(outputs.get(field), bool):
            raise ValueError(f"outputs.{field} must be a boolean.")


def _to_spec(config: dict[str, Any]) -> ExperimentSpec:
    engine = _require_mapping(config, "engine")
    strategy = _require_mapping(config, "strategy")
    params = _require_mapping(strategy, "params")
    execution = _require_mapping(config, "execution")
    fees = _require_mapping(execution, "fees")
    inputs = _require_mapping(config, "inputs")
    outputs = _require_mapping(config, "outputs")
    return ExperimentSpec(
        name=str(config["name"]),
        engine_id=str(engine["engine_id"]),
        symbol=str(inputs["symbol"]),
        asof_utc=str(inputs["asof_utc"]),
        timeframe=str(inputs["timeframe"]),
        lookback=int(params["lookback"]),
        zscore_entry=float(params["zscore_entry"]),
        zscore_exit=float(params["zscore_exit"]),
        spread_bps=float(fees["spread_bps"]),
        slippage_bps=float(fees["slippage_bps"]),
        sample_bars_csv=str(inputs["sample_bars_csv"]),
        output_root_dir=str(outputs["root_dir"]),
    )


def _normalized_config_payload(spec: ExperimentSpec, *, config_path: Path, bars_path: Path) -> dict[str, Any]:
    return {
        "schema_version": "exp_config_lock_v1",
        "experiment_name": spec.name,
        "engine": {
            "engine_id": spec.engine_id,
            "invocation": "explicit_local_only",
            "canonical": False,
            "readiness_bearing": False,
            "operator_visible": False,
            "capital_authoritative": False,
        },
        "strategy": {
            "family": "mean_reversion",
            "symbol": spec.symbol,
            "timeframe": spec.timeframe,
            "lookback": spec.lookback,
            "zscore_entry": spec.zscore_entry,
            "zscore_exit": spec.zscore_exit,
        },
        "execution": {
            "spread_bps": spec.spread_bps,
            "slippage_bps": spec.slippage_bps,
        },
        "inputs": {
            "asof_utc": spec.asof_utc,
            "sample_bars_csv": bars_path.name,
            "sample_bars_sha256": _sha256_file(bars_path),
        },
        "source_files": {
            "config_path": str(config_path.as_posix()),
            "config_sha256": _sha256_file(config_path),
            "bars_path": str(bars_path.as_posix()),
        },
        "output": {
            "root_dir": spec.output_root_dir,
        },
    }


def _build_run_id(spec: ExperimentSpec, *, config_path: Path | None = None, bars_path: Path | None = None) -> str:
    payload = {
        "name": spec.name,
        "engine_id": spec.engine_id,
        "symbol": spec.symbol,
        "asof_utc": spec.asof_utc,
        "timeframe": spec.timeframe,
        "lookback": spec.lookback,
        "zscore_entry": spec.zscore_entry,
        "zscore_exit": spec.zscore_exit,
        "spread_bps": spec.spread_bps,
        "slippage_bps": spec.slippage_bps,
    }
    if config_path is not None:
        payload["config_sha256"] = _sha256_file(config_path)
    if bars_path is not None:
        payload["sample_bars_sha256"] = _sha256_file(bars_path)
    digest = hashlib.sha256(_stable_json_bytes(payload)).hexdigest()[:12]
    return f"{spec.name}-{digest}"


def _output_root(spec: ExperimentSpec) -> Path:
    return RESEARCH_ROOT / spec.output_root_dir


def _sample_bars_path(spec: ExperimentSpec, config_path: Path) -> Path:
    bars_path = config_path.parent / spec.sample_bars_csv
    if not bars_path.exists():
        raise FileNotFoundError(f"Missing sample bars CSV: {bars_path}")
    return bars_path


def _load_bars(csv_path: Path) -> pd.DataFrame:
    df = pd.read_csv(csv_path)
    required_columns = ["timestamp", "open", "high", "low", "close", "volume"]
    missing = [column for column in required_columns if column not in df.columns]
    if missing:
        raise ValueError(f"Bars CSV missing required columns: {missing}")
    out = df.copy()
    out["timestamp"] = pd.to_datetime(out["timestamp"], utc=True)
    out = out.sort_values("timestamp").reset_index(drop=True)
    if out["timestamp"].duplicated().any():
        raise ValueError("Bars CSV contains duplicate timestamps.")
    if len(out) < 40:
        raise ValueError("Bars CSV must contain at least 40 rows.")
    if (out[["open", "high", "low", "close", "volume"]].isna().any()).any():
        raise ValueError("Bars CSV contains nulls in required numeric columns.")
    if (out["high"] < out[["open", "close", "low"]].max(axis=1)).any():
        raise ValueError("Bars CSV has high below open/close/low.")
    if (out["low"] > out[["open", "close", "high"]].min(axis=1)).any():
        raise ValueError("Bars CSV has low above open/close/high.")
    if (out["volume"] < 0).any():
        raise ValueError("Bars CSV contains negative volume.")
    return out


def _compute_signals(bars: pd.DataFrame, spec: ExperimentSpec) -> pd.DataFrame:
    df = bars.copy()
    df["mean_close"] = df["close"].rolling(spec.lookback, min_periods=spec.lookback).mean()
    df["std_close"] = df["close"].rolling(spec.lookback, min_periods=spec.lookback).std(ddof=1)
    df["zscore"] = (df["close"] - df["mean_close"]) / df["std_close"]

    desired_position: list[int] = []
    current = 0
    for value in df["zscore"]:
        if pd.isna(value):
            desired_position.append(0)
            continue
        if current == 0:
            if value <= -spec.zscore_entry:
                current = 1
            elif value >= spec.zscore_entry:
                current = -1
        elif current == 1 and value >= -spec.zscore_exit:
            current = 0
        elif current == -1 and value <= spec.zscore_exit:
            current = 0
        desired_position.append(current)

    df["desired_position"] = desired_position
    df["signal_side"] = df["desired_position"].map({1: "BUY", -1: "SELL", 0: "FLAT"})
    return df


def _build_trades(signals: pd.DataFrame, spec: ExperimentSpec) -> tuple[pd.DataFrame, pd.DataFrame]:
    df = signals.copy()
    df["position"] = df["desired_position"].astype(int)
    df["position_prev"] = df["position"].shift(1).fillna(0).astype(int)
    df["turnover_units"] = (df["position"] - df["position_prev"]).abs()
    trade_rows: list[dict[str, Any]] = []

    open_trade: dict[str, Any] | None = None
    trade_id = 0
    for row in df.itertuples(index=False):
        timestamp = pd.Timestamp(row.timestamp)
        close_px = float(row.close)
        current_position = int(row.position)
        prev_position = int(row.position_prev)
        zscore_value = None if pd.isna(row.zscore) else float(row.zscore)

        if prev_position == 0 and current_position != 0:
            trade_id += 1
            open_trade = {
                "trade_id": trade_id,
                "entry_timestamp": timestamp.isoformat(),
                "entry_price": close_px,
                "side": "LONG" if current_position > 0 else "SHORT",
                "entry_zscore": zscore_value,
            }
        elif prev_position != 0 and current_position == 0 and open_trade is not None:
            side = str(open_trade["side"])
            signed_return = ((close_px / float(open_trade["entry_price"])) - 1.0) if side == "LONG" else ((float(open_trade["entry_price"]) / close_px) - 1.0)
            total_cost = 2.0 * (spec.spread_bps + spec.slippage_bps) / 10000.0
            pnl_net = signed_return - total_cost
            trade_rows.append(
                {
                    **open_trade,
                    "exit_timestamp": timestamp.isoformat(),
                    "exit_price": close_px,
                    "exit_zscore": zscore_value,
                    "gross_return": signed_return,
                    "net_return": pnl_net,
                    "bars_held": 1,
                }
            )
            open_trade = None

    trade_df = pd.DataFrame(trade_rows)
    return df, trade_df


def _build_equity_curve(position_df: pd.DataFrame, spec: ExperimentSpec) -> pd.DataFrame:
    df = position_df[["timestamp", "close", "position", "turnover_units"]].copy()
    df["bar_return"] = df["close"].pct_change().fillna(0.0)
    df["position_for_bar"] = df["position"].shift(1).fillna(0).astype(int)
    cost_per_turnover = (spec.spread_bps + spec.slippage_bps) / 10000.0
    df["transaction_cost"] = df["turnover_units"] * cost_per_turnover
    df["strategy_return"] = df["position_for_bar"] * df["bar_return"] - df["transaction_cost"]
    df["equity"] = (1.0 + df["strategy_return"]).cumprod()
    running_peak = df["equity"].cummax()
    df["drawdown"] = (df["equity"] / running_peak) - 1.0
    return df


def _compute_metrics(equity_curve: pd.DataFrame, trades: pd.DataFrame) -> dict[str, Any]:
    strategy_returns = equity_curve["strategy_return"]
    bars = max(len(equity_curve), 1)
    bars_per_year = 252 * 78
    ending_equity = float(equity_curve["equity"].iloc[-1])
    total_return = ending_equity - 1.0
    annualized_return = (ending_equity ** (bars_per_year / bars)) - 1.0 if ending_equity > 0 else -1.0
    std = float(strategy_returns.std(ddof=1)) if len(strategy_returns) > 1 else 0.0
    sharpe = 0.0 if math.isclose(std, 0.0) else float(strategy_returns.mean()) / std * math.sqrt(bars_per_year)
    max_drawdown = float(equity_curve["drawdown"].min())

    positive_sum = float(strategy_returns[strategy_returns > 0].sum())
    negative_sum = float(strategy_returns[strategy_returns < 0].sum())
    profit_factor = float("inf") if math.isclose(negative_sum, 0.0) and positive_sum > 0 else (positive_sum / abs(negative_sum) if negative_sum < 0 else 0.0)

    trade_count = int(len(trades))
    win_rate = float((trades["net_return"] > 0).mean()) if trade_count else 0.0
    avg_trade_return = float(trades["net_return"].mean()) if trade_count else 0.0

    return {
        "schema_version": "exp_metrics_v1",
        "trade_count": trade_count,
        "total_return": total_return,
        "annualized_return": annualized_return,
        "max_drawdown": max_drawdown,
        "profit_factor": profit_factor,
        "sharpe": sharpe,
        "win_rate": win_rate,
        "avg_trade_return": avg_trade_return,
        "ending_equity": ending_equity,
    }


def _build_manifest(spec: ExperimentSpec, config_path: Path, bars_path: Path, run_dir: Path) -> ResearchManifest:
    config_sha256 = _sha256_file(config_path)
    return ResearchManifest(
        schema_version="1",
        contract_version=CONTRACT_VERSION,
        run_id=run_dir.name,
        asof_utc=spec.asof_utc,
        policy_name=spec.name,
        policy_path=str(config_path.as_posix()),
        policy_sha256=config_sha256,
        params={
            "engine_id": spec.engine_id,
            "symbol": spec.symbol,
            "timeframe": spec.timeframe,
            "lookback": spec.lookback,
            "zscore_entry": spec.zscore_entry,
            "zscore_exit": spec.zscore_exit,
            "spread_bps": spec.spread_bps,
            "slippage_bps": spec.slippage_bps,
            "invocation_mode": "explicit_local_only",
        },
        inputs={
            "bars_csv": str(bars_path.as_posix()),
            "bars_csv_sha256": _sha256_file(bars_path),
            "symbol": spec.symbol,
            "timeframe": spec.timeframe,
        },
        outputs={
            "root_dir": str(run_dir.as_posix()),
            "manifest_path": str((run_dir / "manifest.json").as_posix()),
            "resolved_config_path": str((run_dir / "resolved_config.json").as_posix()),
            "artifact_index_path": str((run_dir / "artifact_index.json").as_posix()),
            "signal_pack_path": str((run_dir / "signal_pack.csv").as_posix()),
            "trades_path": str((run_dir / "trades.csv").as_posix()),
            "equity_curve_path": str((run_dir / "equity_curve.csv").as_posix()),
            "metrics_path": str((run_dir / "metrics.json").as_posix()),
        },
        notes=[
            "EXP-only sandbox engine.",
            "Non-canonical / non-operator-facing / non-readiness-bearing.",
            "No daemon/runtime/DB/GUI/shared truth wiring is performed by this script.",
            "Artifacts are local research outputs only.",
        ],
    )


def _build_artifact_index(result: PipelineResult) -> dict[str, Any]:
    manifest_path = result.run_dir / "manifest.json"
    resolved_config_path = result.run_dir / "resolved_config.json"
    artifact_paths = {
        "manifest": manifest_path,
        "resolved_config": resolved_config_path,
        "signal_pack": result.run_dir / "signal_pack.csv",
        "trades": result.run_dir / "trades.csv",
        "equity_curve": result.run_dir / "equity_curve.csv",
        "metrics": result.run_dir / "metrics.json",
    }
    return {
        "schema_version": "exp_artifact_index_v1",
        "run_id": result.run_id,
        "engine_id": "EXP",
        "artifact_count": len(artifact_paths),
        "artifacts": {
            name: {
                "path": str(path.as_posix()),
                "sha256": _sha256_file(path),
            }
            for name, path in artifact_paths.items()
        },
    }


def run_pipeline(config_path: Path) -> PipelineResult:
    config_path = config_path.resolve()
    config = _load_config(config_path)
    _validate_exp_only(config)
    spec = _to_spec(config)
    bars_path = _sample_bars_path(spec, config_path)
    run_id = _build_run_id(spec, config_path=config_path, bars_path=bars_path)
    run_dir = _output_root(spec) / run_id
    bars = _load_bars(bars_path)
    signals = _compute_signals(bars, spec)
    positioned, trades = _build_trades(signals, spec)
    equity_curve = _build_equity_curve(positioned, spec)
    metrics = _compute_metrics(equity_curve, trades)
    manifest = _build_manifest(spec, config_path, bars_path, run_dir)
    resolved_config = _normalized_config_payload(spec, config_path=config_path, bars_path=bars_path)
    return PipelineResult(
        run_id=run_id,
        run_dir=run_dir,
        bars_path=bars_path,
        config_path=config_path,
        bars=bars,
        signals=signals,
        trades=trades,
        equity_curve=equity_curve,
        metrics=metrics,
        manifest=manifest,
        resolved_config=resolved_config,
        artifact_index={},
    )


def _write_csv(df: pd.DataFrame, path: Path) -> None:
    path.parent.mkdir(parents=True, exist_ok=True)
    df.to_csv(path, index=False)


def write_artifacts(result: PipelineResult) -> None:
    result.run_dir.mkdir(parents=True, exist_ok=True)
    signal_pack = result.signals[["timestamp", "close", "zscore", "desired_position", "signal_side"]].copy()
    signal_pack["timestamp"] = signal_pack["timestamp"].astype(str)
    trades = result.trades.copy()
    equity = result.equity_curve.copy()
    equity["timestamp"] = equity["timestamp"].astype(str)

    _write_csv(signal_pack, result.run_dir / "signal_pack.csv")
    _write_csv(trades, result.run_dir / "trades.csv")
    _write_csv(equity, result.run_dir / "equity_curve.csv")
    (result.run_dir / "metrics.json").write_text(_json_text(result.metrics), encoding="utf-8")
    (result.run_dir / "manifest.json").write_text(result.manifest.to_json(indent=2) + "\n", encoding="utf-8")
    (result.run_dir / "resolved_config.json").write_text(_json_text(result.resolved_config), encoding="utf-8")

    artifact_index = _build_artifact_index(
        PipelineResult(
            run_id=result.run_id,
            run_dir=result.run_dir,
            bars_path=result.bars_path,
            config_path=result.config_path,
            bars=result.bars,
            signals=result.signals,
            trades=result.trades,
            equity_curve=result.equity_curve,
            metrics=result.metrics,
            manifest=result.manifest,
            resolved_config=result.resolved_config,
            artifact_index={},
        )
    )
    (result.run_dir / "artifact_index.json").write_text(_json_text(artifact_index), encoding="utf-8")


def parse_args(argv: list[str] | None = None) -> argparse.Namespace:
    parser = argparse.ArgumentParser(description="Run the isolated EXP-only exp_meanrev_v1 engine.")
    parser.add_argument(
        "--config",
        type=Path,
        default=EXPERIMENT_DIR / "config.yaml",
        help="Path to the EXP experiment config.",
    )
    parser.add_argument(
        "--allow-exp-local",
        action="store_true",
        help="Required explicit opt-in. Prevents accidental invocation.",
    )
    parser.add_argument(
        "--write-artifacts",
        action="store_true",
        help="Write EXP-only artifacts under research-py/runs/EXP/exp_meanrev_v1/<run_id>/",
    )
    return parser.parse_args(argv)


def main(argv: list[str] | None = None) -> int:
    args = parse_args(argv)
    if not args.allow_exp_local:
        print("Refusing to run EXP engine without --allow-exp-local.")
        return 2

    result = run_pipeline(args.config)
    print("exp_meanrev_v1 isolated engine")
    print(f"engine_id=EXP")
    print(f"run_id={result.run_id}")
    print(f"bars={len(result.bars)}")
    print(f"trades={result.metrics['trade_count']}")
    print(f"ending_equity={result.metrics['ending_equity']:.6f}")
    print(f"config_sha256={result.resolved_config['source_files']['config_sha256']}")
    print(f"bars_sha256={result.resolved_config['inputs']['sample_bars_sha256']}")
    print(f"output_dir={result.run_dir.as_posix()}")

    if not args.write_artifacts:
        print("dry_run=true")
        print(json.dumps(result.metrics, indent=2, sort_keys=True))
        return 0

    write_artifacts(result)
    print("dry_run=false")
    print(f"manifest_written={(result.run_dir / 'manifest.json').as_posix()}")
    print(f"resolved_config_written={(result.run_dir / 'resolved_config.json').as_posix()}")
    print(f"artifact_index_written={(result.run_dir / 'artifact_index.json').as_posix()}")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
