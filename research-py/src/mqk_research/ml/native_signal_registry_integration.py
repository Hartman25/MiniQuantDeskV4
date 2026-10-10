"""Registered economic evaluation of a NATIVE strategy's signal stream.

The executable strategy (Rust) stays the single source of truth for decisions:
`mqk backtest native-signals` runs it through the real BacktestEngine and
emits its per-bar ABSOLUTE target quantities. This module registers that
stream as the OOS decision series of a predeclared Research trial and
evaluates it with the unmodified `run_economic_walkforward` -- the same
economic protocol, official execution-pricing/weight-to-share parity and
holdout reservation as every classifier trial. Nothing here re-implements a
trading rule.

Quantity contract (`native_strategy_signal_stream_v2`): a native
`TargetPosition.qty` is an absolute portfolio target and production derives
`delta = target - current` from it. Research therefore executes the EXACT
whole-share target under `direction_policy=native_exact_target_qty_v1`; it
never reduces the target to a direction and re-derives a size from a weight.
The earlier `native_strategy_signal_stream_v1` bridge did exactly that and its
evidence is superseded: it cannot be registered, loaded or promoted here.

Chronology: hypothesis -> trial registration -> attempt -> signal emission ->
economic evaluation. The trial must already be registered when an attempt
begins, and the emitter runs INSIDE the attempt, so an emission failure is a
failed attempt of the same trial, never a new trial. Trial identity is
result-independent; a retry is a new attempt of the SAME trial.

Evaluation is out-of-sample by construction only because the rule has no fitted
parameters: its constants are predeclared (and bound into the native semantic
fingerprint) before any result exists.
"""

from __future__ import annotations

import csv
import json
from dataclasses import dataclass
from pathlib import Path
from typing import Any, Callable, Dict, List, NamedTuple, Optional, Tuple

import numpy as np
import pandas as pd

from mqk_research.data.bars_provenance import (
    provenance_identity_fragment,
    provenance_identity_fragment_canonical_timeframe,
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
    SIGNAL_DIRECTION_POLICY_NATIVE_EXACT_TARGET_QTY_V1,
    EconomicWalkForwardSpec,
    economic_protocol_identity,
    run_economic_walkforward,
)
from mqk_research.ml.eval_walkforward import compute_holdout_boundary
from mqk_research.ml.execution_pricing import to_micros
from mqk_research.ml.holdout_ledger import compute_holdout_id
from mqk_research.ml.util_hash import file_record, sha256_file

NATIVE_SIGNAL_STREAM_PROTOCOL_ID = "native_strategy_signal_stream_v2"
NATIVE_SIGNAL_SOURCE_KIND = "native_strategy_signal_stream_v2"
NATIVE_QUANTITY_SEMANTICS_ID = "absolute_target_qty_micros_v1"
NATIVE_TARGET_SEMANTICS_ID = "absolute_whole_share_target_v1"
# Protocol ids whose economics reinterpreted a native target as a direction
# and re-sized it from a weight. Historical evidence only; never accepted.
SUPERSEDED_NATIVE_PROTOCOL_IDS = frozenset({"native_strategy_signal_stream_v1"})
_MICROS = 1_000_000

# An evaluation only measures the hypothesis if the discrete share positions it
# simulates actually equal the strategy's target quantities. The floor is a
# fixed, outcome-independent implementability check, applied to every native
# trial; it tolerates only causal admission rejections, never a different
# quantity.
NATIVE_EXECUTION_FIDELITY_FLOOR = 0.95

# A fixed partition names the reserved window in the declaration instead of deriving it from the last
# fetched bar, so the development fetch can end exactly at the reserved start without the derived boundary
# (and therefore the historical folds) sliding backwards. Absent = the historical derivation, unchanged.
FIXED_HOLDOUT_BOUNDARY_VERSION = "fixed_holdout_boundary_v1"
_FIXED_HOLDOUT_KEYS = frozenset({"version", "holdout_start_utc", "holdout_end_utc"})

__all__ = [
    "FIXED_HOLDOUT_BOUNDARY_VERSION",
    "validate_fixed_holdout_boundary",
    "NATIVE_EXECUTION_FIDELITY_FLOOR",
    "NATIVE_QUANTITY_SEMANTICS_ID",
    "NATIVE_SIGNAL_SOURCE_KIND",
    "NATIVE_SIGNAL_STREAM_PROTOCOL_ID",
    "NATIVE_TARGET_SEMANTICS_ID",
    "SUPERSEDED_NATIVE_PROTOCOL_IDS",
    "NativeFidelity",
    "NativeSignalError",
    "build_native_signal_trial_identity",
    "native_exact_target_fidelity",
    "native_holdout_start",
    "plan_native_folds",
    "register_native_signal_trial",
    "require_native_exact_target_spec",
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


class NativeFidelity(NamedTuple):
    agreement: float
    evaluated_bars: int
    matched_bars: int
    max_abs_qty_gap: int
    desired_nonzero_bars: int


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


def validate_fixed_holdout_boundary(boundary: Any, holdout_months: int) -> Tuple[pd.Timestamp, pd.Timestamp]:
    """(holdout_start, holdout_end) of a declared fixed partition, or NativeSignalError. The start must be a
    month-aligned UTC midnight, the end exactly `holdout_months` later; nothing is read from market data."""
    if not isinstance(boundary, dict) or set(boundary) != _FIXED_HOLDOUT_KEYS:
        raise NativeSignalError(f"fixed holdout boundary must have exactly the keys {sorted(_FIXED_HOLDOUT_KEYS)}")
    if boundary["version"] != FIXED_HOLDOUT_BOUNDARY_VERSION:
        raise NativeSignalError(f"fixed holdout boundary version must be {FIXED_HOLDOUT_BOUNDARY_VERSION!r}")
    try:
        start, end = pd.Timestamp(boundary["holdout_start_utc"]), pd.Timestamp(boundary["holdout_end_utc"])
    except (TypeError, ValueError) as exc:
        raise NativeSignalError("fixed holdout boundary timestamps are unreadable") from exc
    if start.tzinfo is None or end.tzinfo is None:
        raise NativeSignalError("fixed holdout boundary timestamps must be timezone-aware UTC")
    start, end = start.tz_convert("UTC"), end.tz_convert("UTC")
    if start != pd.Timestamp(year=start.year, month=start.month, day=1, tz="UTC"):
        raise NativeSignalError("fixed holdout start must be a month-aligned UTC midnight")
    if end != start + pd.DateOffset(months=int(holdout_months)):
        raise NativeSignalError("fixed holdout end must equal the start plus holdout_months")
    return start, end


def plan_native_folds(
    *,
    t_min: pd.Timestamp,
    t_max: pd.Timestamp,
    evaluation_start_utc: pd.Timestamp,
    test_months: int,
    holdout_months: int,
    fixed_holdout_boundary: Optional[Dict[str, Any]] = None,
) -> Tuple[List[NativeFold], pd.Timestamp, pd.Timestamp]:
    """Contiguous, non-overlapping test windows from `evaluation_start_utc`
    up to the reserved holdout. Returns (folds, holdout_start, dataset_end).
    Without `fixed_holdout_boundary` the holdout boundary is derived from the observed span, as the
    classifier protocol does. With it, the boundary is the declared one and the observed bars must lie
    wholly before it AND reach its final development month: a missing month never moves the boundary."""
    if int(test_months) <= 0 or int(holdout_months) <= 0:
        raise NativeSignalError("test_months and holdout_months must be > 0")
    if fixed_holdout_boundary is None:
        _anchor, holdout_start, dataset_end = compute_holdout_boundary(t_min, t_max, int(holdout_months))
    else:
        holdout_start, dataset_end = validate_fixed_holdout_boundary(fixed_holdout_boundary, int(holdout_months))
        if t_max >= holdout_start:
            raise NativeSignalError(
                f"bars reach {t_max.isoformat()}, at or after the reserved holdout start {holdout_start.isoformat()}")
        if t_max < holdout_start - pd.DateOffset(months=1):
            raise NativeSignalError(
                "bars do not reach the final development month before the reserved holdout; the boundary does not move")
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


def native_exact_target_fidelity(
    economic_out: Dict[str, Any], symbol: str, signals: pd.DataFrame
) -> NativeFidelity:
    """Exact-quantity agreement between the strategy's absolute target and the
    economic simulator's discrete position.

    For every evaluated bar of every fold (the fold's forced fold-end flatten
    row excluded), the desired quantity is the native `target_qty` of the
    latest decision of that fold strictly before the bar (the causal rule the
    simulator itself applies), and the held quantity is the simulator's
    post-order discrete position. Agreement requires EQUAL quantities: desired
    1 / held 1 matches, desired 1 / held 250 does not. A signal-time target the
    simulator reports for a bar must also equal the desired quantity. Depends
    only on positions, never on any P&L."""
    # Timestamp.value is nanoseconds whatever the frame's datetime resolution is
    # (pandas 3 defaults to microseconds), so never compare it to astype("int64").
    ts_ns = np.array([pd.Timestamp(t).value for t in signals["decision_ts_utc"]], dtype="int64")
    qty = signals["target_qty"].astype("int64").to_numpy()
    order = np.argsort(ts_ns, kind="mergesort")
    sig_ns = ts_ns[order]
    sig_qty = qty[order]

    evaluated = matched = max_gap = desired_nonzero = 0
    for fold in economic_out["folds"]:
        lo = pd.Timestamp(fold["test_start_utc"]).value
        hi = pd.Timestamp(fold["test_end_utc"]).value
        in_fold = (sig_ns >= lo) & (sig_ns < hi)
        f_ns, f_qty = sig_ns[in_fold], sig_qty[in_fold]
        events = fold["weight_to_share_evidence"][symbol]
        for event in events[:-1]:
            t = pd.Timestamp(event["timestamp"]).value
            i = int(np.searchsorted(f_ns, t, side="left")) - 1
            desired = int(f_qty[i]) if i >= 0 else 0
            held = int(event["target_qty"])
            intended = event.get("signal_target_qty")
            ok = held == desired and (intended is None or int(intended) == desired)
            evaluated += 1
            matched += int(ok)
            max_gap = max(max_gap, abs(held - desired))
            desired_nonzero += int(desired != 0)
    if evaluated == 0:
        raise NativeSignalError("no evaluated bars to measure execution fidelity on")
    return NativeFidelity(matched / evaluated, evaluated, matched, max_gap, desired_nonzero)


def native_holdout_start(
    bars_csv: Path, symbol: str, holdout_months: int, fixed_holdout_boundary: Optional[Dict[str, Any]] = None
) -> pd.Timestamp:
    """The reserved holdout start for `symbol`'s bars: derived from the observed span, or the declared
    fixed boundary (refusing bars that reach it)."""
    bars = pd.read_csv(bars_csv)
    ts = pd.to_datetime(bars[bars["symbol"].astype(str) == symbol]["end_ts"], utc=True)
    if ts.empty:
        raise NativeSignalError(f"no bars for symbol {symbol!r} in {bars_csv}")
    if fixed_holdout_boundary is None:
        return compute_holdout_boundary(ts.min(), ts.max(), int(holdout_months))[1]
    start, _end = validate_fixed_holdout_boundary(fixed_holdout_boundary, int(holdout_months))
    if ts.max() >= start:
        raise NativeSignalError(f"{symbol} bars reach {ts.max().isoformat()}, at or after the reserved holdout start")
    return start


def require_native_exact_target_spec(economic_spec: EconomicWalkForwardSpec) -> EconomicWalkForwardSpec:
    """A native absolute target is only evaluated under the exact-target policy
    with the discrete economics engaged. Any other policy would re-size the
    target from a weight and is refused. Returns the normalized spec."""
    spec = economic_spec.normalized()
    if spec.signal_policy.direction_policy != SIGNAL_DIRECTION_POLICY_NATIVE_EXACT_TARGET_QTY_V1:
        raise NativeSignalError(
            "native strategy targets are absolute quantities and require "
            f"direction_policy={SIGNAL_DIRECTION_POLICY_NATIVE_EXACT_TARGET_QTY_V1!r}; got "
            f"{spec.signal_policy.direction_policy!r}, which would re-size the target from a weight"
        )
    if spec.weight_to_share is None:
        raise NativeSignalError("native exact-target evaluation requires the discrete weight_to_share economics")
    return spec


_CAPITAL_SIZING_KEYS = (
    "policy_id", "allocation_fraction_bps", "initial_allocated_capital_micros", "max_target_qty",
    "max_position_notional_usd",
)

# The robustness stress a capital-fraction candidate must be promoted against, fixed in the
# registered trial identity before its first attempt. Every behavior-bearing P7A/P7B stress input
# is bound; Promotion compares the hash-verified stress evidence with this registered value,
# never with a caller-supplied one. The drawdown ceiling is an integer bps so identity and
# comparison are exact (no float equality).
STRESS_CONTRACT_SCHEMA_VERSION = "p7a_p7b_stress_contract_v1"
_STRESS_CONTRACT_KEYS = (
    "schema_version", "scenario_id", "allocation_fraction_bps",
    "stress_execution_slippage_bps", "stress_execution_volatility_mult_bps", "max_drawdown_ceiling_bps",
)


def _validated_stress_contract(
    stress_contract: Dict[str, Any],
    capital_sizing: Optional[Dict[str, Any]],
    baseline_pricing: Any,
) -> Dict[str, Any]:
    if capital_sizing is None:
        raise NativeSignalError("stress_contract requires capital_sizing (the capital-fraction protocol)")
    if not isinstance(stress_contract, dict) or set(stress_contract) != set(_STRESS_CONTRACT_KEYS):
        raise NativeSignalError(f"stress_contract must have exactly the keys {_STRESS_CONTRACT_KEYS}")
    if stress_contract["schema_version"] != STRESS_CONTRACT_SCHEMA_VERSION:
        raise NativeSignalError(f"stress_contract.schema_version must be {STRESS_CONTRACT_SCHEMA_VERSION!r}")
    scenario_id, bps = stress_contract["scenario_id"], stress_contract["allocation_fraction_bps"]
    if not isinstance(scenario_id, str) or not scenario_id.strip():
        raise NativeSignalError("stress_contract.scenario_id must be a non-empty string")
    baseline_bps = capital_sizing["allocation_fraction_bps"]
    if type(bps) is not int or not 1 <= bps < baseline_bps:
        raise NativeSignalError(
            f"stress_contract.allocation_fraction_bps must be an integer in [1, {baseline_bps}) "
            "(strictly below the baseline fraction)"
        )
    slip = stress_contract["stress_execution_slippage_bps"]
    vol = stress_contract["stress_execution_volatility_mult_bps"]
    if type(slip) is not int or type(vol) is not int:
        raise NativeSignalError("stress_contract execution slippage/volatility must be integers")
    base_slip, base_vol = baseline_pricing.slippage_bps, baseline_pricing.volatility_mult_bps
    if slip < base_slip or vol < base_vol or (slip == base_slip and vol == base_vol):
        raise NativeSignalError(
            "stress_contract execution pricing must be at least the baseline "
            f"(slippage {base_slip}, volatility {base_vol}) and strictly worse in one of them"
        )
    ceiling = stress_contract["max_drawdown_ceiling_bps"]
    if type(ceiling) is not int or not 1 <= ceiling <= 10_000:
        raise NativeSignalError("stress_contract.max_drawdown_ceiling_bps must be an integer in [1, 10000]")
    return {k: stress_contract[k] for k in _STRESS_CONTRACT_KEYS}


def _require_declared_sizing(
    meta: Dict[str, Any], expected: Optional[Dict[str, Any]], emitter_cash: int
) -> None:
    """A stream's sizing block must equal the registered capital-fraction contract
    exactly; a stream with a sizing block can never be consumed as fixed-quantity,
    and a fixed-quantity stream can never satisfy a capital-fraction registration."""
    block = meta.get("sizing")
    if expected is None:
        if block is not None:
            raise NativeSignalError(
                "signal stream carries a capital-fraction sizing block but the trial was registered "
                "under the fixed-quantity protocol"
            )
        return
    if not isinstance(block, dict):
        raise NativeSignalError(
            "trial is registered under the capital-fraction protocol but the signal stream carries no sizing block"
        )
    if set(expected) != set(_CAPITAL_SIZING_KEYS):
        raise NativeSignalError(f"expected_capital_sizing must have exactly the keys {_CAPITAL_SIZING_KEYS}")
    for k in _CAPITAL_SIZING_KEYS:
        if block.get(k) != expected[k]:
            raise NativeSignalError(
                f"signal stream sizing.{k} {block.get(k)!r} != the registered {expected[k]!r}"
            )
    if int(expected["initial_allocated_capital_micros"]) != int(emitter_cash):
        raise NativeSignalError("sizing capital differs from the emitter initial_cash_micros")
    if not isinstance(block.get("entries"), list):
        raise NativeSignalError("signal stream sizing block has no entries list")
    for e in block["entries"]:
        if int(e.get("resolved_target_qty_micros", 0)) <= 0:
            raise NativeSignalError("signal stream sizing entry has a non-positive resolved quantity")


def _load_signals(
    signals_csv: Path,
    meta_json: Path,
    *,
    strategy_id: str,
    symbol: str,
    backtest_bars_sha256: str,
    expected_timeframe_secs: int,
    expected_semantic_fingerprint: str,
    expected_required_history_bars: int,
    equity_usd: float,
    expected_capital_sizing: Optional[Dict[str, Any]] = None,
) -> Tuple[pd.DataFrame, Dict[str, Any]]:
    meta = json.loads(Path(meta_json).read_text(encoding="utf-8"))
    protocol = meta.get("protocol_id")
    if protocol in SUPERSEDED_NATIVE_PROTOCOL_IDS:
        raise NativeSignalError(
            f"native signal protocol {protocol!r} is superseded (it reinterpreted the target as a "
            f"direction and re-sized it from a weight) and is not promotion authority; "
            f"expected {NATIVE_SIGNAL_STREAM_PROTOCOL_ID!r}"
        )
    if protocol != NATIVE_SIGNAL_STREAM_PROTOCOL_ID:
        raise NativeSignalError(f"unsupported native signal protocol: {protocol!r}")
    if meta.get("quantity_semantics") != NATIVE_QUANTITY_SEMANTICS_ID:
        raise NativeSignalError(
            f"signal stream quantity_semantics {meta.get('quantity_semantics')!r} != {NATIVE_QUANTITY_SEMANTICS_ID!r}"
        )
    if meta.get("strategy_name") != strategy_id:
        raise NativeSignalError(
            f"signal stream strategy {meta.get('strategy_name')!r} != trial strategy_id {strategy_id!r}"
        )
    if meta.get("symbol") != symbol:
        raise NativeSignalError(f"signal stream symbol {meta.get('symbol')!r} != {symbol!r}")
    fp = str(meta.get("semantic_fingerprint", ""))
    if len(fp) != 64 or any(c not in "0123456789abcdef" for c in fp):
        raise NativeSignalError("signal stream semantic_fingerprint is not 64 lowercase hex")
    if int(meta.get("timeframe_secs", -1)) != int(expected_timeframe_secs):
        raise NativeSignalError(
            f"signal stream timeframe_secs {meta.get('timeframe_secs')!r} != expected {expected_timeframe_secs}"
        )
    if fp != expected_semantic_fingerprint:
        raise NativeSignalError(
            "signal stream semantic_fingerprint does not equal the expected native fingerprint"
        )
    if meta.get("native_signals_csv_sha256") != sha256_file(Path(signals_csv)):
        raise NativeSignalError("signal stream csv does not match its recorded sha256")
    if meta.get("bars_csv_sha256") != backtest_bars_sha256:
        raise NativeSignalError(
            "signals were not emitted over the bars derived from this trial's research bars"
        )

    # History provenance must describe the window the strategy actually received.
    try:
        configured = int(meta["configured_bar_history_len"])
        required = int(meta["required_history_bars"])
        effective = int(meta["effective_bar_history_len"])
        observed = int(meta["observed_max_window_len"])
        emitter_cash = int(meta["initial_cash_micros"])
    except (KeyError, TypeError, ValueError) as exc:
        raise NativeSignalError(f"signal stream meta lacks truthful history/cash provenance: {exc!r}") from exc
    if required != int(expected_required_history_bars):
        raise NativeSignalError(
            f"signal stream required_history_bars {required} != the registered {expected_required_history_bars}"
        )
    if effective != max(configured, required):
        raise NativeSignalError(
            f"signal stream effective_bar_history_len {effective} != max(configured {configured}, required {required})"
        )
    rows = int(meta.get("signal_rows", -1))
    if observed > effective or (rows >= effective and observed != effective):
        raise NativeSignalError(
            f"signal stream observed_max_window_len {observed} contradicts effective_bar_history_len {effective}"
        )
    if emitter_cash != to_micros(equity_usd):
        raise NativeSignalError(
            f"emitter/Backtest initial_cash_micros {emitter_cash} != Research equity_usd {equity_usd} "
            "-- Research and Backtest evidence must share one capital basis"
        )
    _require_declared_sizing(meta, expected_capital_sizing, emitter_cash)

    df = pd.read_csv(signals_csv)
    if list(df.columns) != ["symbol", "decision_ts", "target_qty_micros"]:
        raise NativeSignalError("unexpected native signal csv columns")
    if rows != len(df) or df.empty:
        raise NativeSignalError("signal row count disagrees with meta or is empty")
    if (df["symbol"].astype(str) != symbol).any():
        raise NativeSignalError("signal stream contains a different symbol")
    if df["decision_ts"].duplicated().any():
        raise NativeSignalError("duplicate decision_ts in signal stream")
    q = df["target_qty_micros"].astype("int64")
    if (q < 0).any():
        raise NativeSignalError("native exact-target protocol supports only non-negative targets")
    if (q % _MICROS != 0).any():
        raise NativeSignalError("native exact-target protocol supports only whole-share targets")
    if expected_capital_sizing is not None:
        resolved = {int(e["resolved_target_qty_micros"]) for e in meta["sizing"]["entries"]}
        stray = sorted({int(x) for x in q[q > 0]} - resolved)
        if stray:
            raise NativeSignalError(
                f"signal stream carries positive target quantities {stray} that no engine-resolved "
                "capital-fraction entry produced"
            )
    df = df.sort_values("decision_ts", kind="mergesort").reset_index(drop=True)
    df["decision_ts_utc"] = pd.to_datetime(df["decision_ts"].astype("int64"), unit="s", utc=True)
    df["target_qty"] = (df["target_qty_micros"].astype("int64") // _MICROS).astype("int64")
    # `ml_score` is only the loader's required column; the exact-target policy
    # never reads it.
    df["ml_score"] = (df["target_qty"] > 0).astype(float)
    return df, meta


def build_native_signal_trial_identity(
    *,
    experiment_id: str,
    hypothesis_id: str,
    strategy_id: str,
    symbol: str,
    semantic_fingerprint: str,
    required_history_bars: int,
    bars_provenance: Dict[str, Any],
    evaluation_start_utc: pd.Timestamp,
    test_months: int,
    holdout_months: int,
    economic_spec: EconomicWalkForwardSpec,
    capital_sizing: Optional[Dict[str, Any]] = None,
    stress_contract: Optional[Dict[str, Any]] = None,
    canonical_timeframe_identity: bool = False,
    fixed_holdout_boundary: Optional[Dict[str, Any]] = None,
) -> Tuple[str, Dict[str, Any]]:
    """Result-independent identity: strategy semantics, quantity contract, data
    provenance, partition policy and economic protocol only. The signal source
    kind, the exact-target policy inside the economic identity and the capital
    basis all differ from the superseded v1 bridge, so no v1 trial id can equal
    a v2 one.

    `canonical_timeframe_identity=True` selects the versioned `canonical_semantic_v1`
    bars-provenance fragment (daily aliases `1D`/`1Day` collapse to one identity; unknown
    labels are refused). The default reproduces every historical trial id."""
    spec = require_native_exact_target_spec(economic_spec)
    bars_fragment = (
        provenance_identity_fragment_canonical_timeframe
        if canonical_timeframe_identity
        else provenance_identity_fragment
    )
    identity: Dict[str, Any] = {
        "experiment_id": experiment_id,
        "hypothesis_id": hypothesis_id,
        "strategy_id": strategy_id,
        "protocol_id": ECONOMIC_PROTOCOL_ID,
        "data_identity": {
            "bars_provenance": bars_fragment(bars_provenance),
        },
        "signal_source": {
            "kind": NATIVE_SIGNAL_SOURCE_KIND,
            "symbol": symbol,
            "semantic_fingerprint": semantic_fingerprint,
            "target_semantics": NATIVE_TARGET_SEMANTICS_ID,
            "required_history_bars": int(required_history_bars),
        },
        "evaluation_spec": {
            "signal_source": NATIVE_SIGNAL_SOURCE_KIND,
            "evaluation_start_utc": pd.Timestamp(evaluation_start_utc).isoformat(),
            "test_months": int(test_months),
            "holdout_months": int(holdout_months),
        },
        "economic_protocol": economic_protocol_identity(spec),
    }
    if fixed_holdout_boundary is not None:
        # Absent unless declared, so every historical trial id is unchanged.
        start, end = validate_fixed_holdout_boundary(fixed_holdout_boundary, int(holdout_months))
        identity["evaluation_spec"]["holdout_boundary"] = {
            "version": FIXED_HOLDOUT_BOUNDARY_VERSION, "holdout_start_utc": start.isoformat(),
            "holdout_end_utc": end.isoformat()}
    if capital_sizing is not None:
        # Behavior-bearing and result-independent; absent for fixed-quantity
        # trials, so every historical trial id is unchanged.
        if set(capital_sizing) != set(_CAPITAL_SIZING_KEYS):
            raise NativeSignalError(f"capital_sizing must have exactly the keys {_CAPITAL_SIZING_KEYS}")
        identity["signal_source"]["capital_sizing"] = {k: capital_sizing[k] for k in _CAPITAL_SIZING_KEYS}
    if stress_contract is not None:
        # Absent unless declared, so every historical trial id is unchanged.
        identity["signal_source"]["stress_contract"] = _validated_stress_contract(
            stress_contract, capital_sizing, spec.execution_pricing
        )
    if spec.execution_pricing.is_official_parity_model:
        identity["data_identity"]["bars_pricing_provenance"] = {
            "canonical_pricing_bars_hash": bars_provenance.get("canonical_pricing_bars_hash"),
        }
    return short_hash(identity, length=32), identity


def _require_inputs(**named: Any) -> None:
    for name, value in named.items():
        if not str(value).strip():
            raise ValueError(f"{name} is required")


def register_native_signal_trial(
    *,
    experiment_id: str,
    hypothesis_id: str,
    strategy_id: str,
    symbol: str,
    semantic_fingerprint: str,
    required_history_bars: int,
    bars_provenance: Dict[str, Any],
    economic_spec: EconomicWalkForwardSpec,
    evaluation_start_utc: pd.Timestamp,
    test_months: int = 12,
    holdout_months: int = 6,
    hypothesis_text: Optional[str] = None,
    registry_db: Optional[Path] = None,
    capital_sizing: Optional[Dict[str, Any]] = None,
    stress_contract: Optional[Dict[str, Any]] = None,
    canonical_timeframe_identity: bool = False,
    fixed_holdout_boundary: Optional[Dict[str, Any]] = None,
) -> str:
    """Register the hypothesis and the trial and NOTHING else: no emission, no
    market data, no attempt, no evaluation. The fingerprint comes from the
    native registry (`mqk backtest native-fingerprint`), not from a run."""
    _require_inputs(
        experiment_id=experiment_id, hypothesis_id=hypothesis_id, strategy_id=strategy_id, symbol=symbol,
    )
    if len(semantic_fingerprint) != 64 or any(c not in "0123456789abcdef" for c in semantic_fingerprint):
        raise NativeSignalError("semantic_fingerprint is not 64 lowercase hex")
    require_registered_bars_provenance(bars_provenance)
    spec = require_native_exact_target_spec(economic_spec)
    require_official_execution_pricing_parity(spec)
    require_official_weight_to_share_parity(spec)
    trial_id, identity = build_native_signal_trial_identity(
        experiment_id=experiment_id, hypothesis_id=hypothesis_id, strategy_id=strategy_id, symbol=symbol,
        semantic_fingerprint=semantic_fingerprint, required_history_bars=required_history_bars,
        bars_provenance=bars_provenance, evaluation_start_utc=evaluation_start_utc,
        test_months=test_months, holdout_months=holdout_months, economic_spec=spec,
        capital_sizing=capital_sizing, stress_contract=stress_contract,
        canonical_timeframe_identity=canonical_timeframe_identity,
        fixed_holdout_boundary=fixed_holdout_boundary,
    )
    store = ResearchResultStore(registry_db or default_db_path(default_root()))
    store.register_hypothesis(
        hypothesis_id=hypothesis_id, experiment_id=experiment_id, hypothesis_text=hypothesis_text
    )
    store.register_trial(
        trial_id=trial_id, experiment_id=experiment_id, hypothesis_id=hypothesis_id,
        strategy_id=strategy_id, protocol_id=ECONOMIC_PROTOCOL_ID, identity=identity,
    )
    return trial_id


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
    emit_signals: Callable[[], None],
    signals_csv: Path,
    signals_meta_json: Path,
    economic_spec: EconomicWalkForwardSpec,
    evaluation_start_utc: pd.Timestamp,
    expected_semantic_fingerprint: str,
    required_history_bars: int,
    test_months: int = 12,
    holdout_months: int = 6,
    registry_db: Optional[Path] = None,
    expected_timeframe_secs: int = 86_400,
    expected_capital_sizing: Optional[Dict[str, Any]] = None,
    expected_stress_contract: Optional[Dict[str, Any]] = None,
    canonical_timeframe_identity: bool = False,
    fixed_holdout_boundary: Optional[Dict[str, Any]] = None,
) -> Path:
    """Official registered entry point for a native strategy's signals. The
    trial must ALREADY be registered (`register_native_signal_trial`); order is
    trial -> attempt (BEFORE emission) -> `emit_signals()` -> signal load ->
    economic evaluation -> holdout reservation -> attempt finalized. Any
    failure after the attempt starts -- including a failed emission --
    finalizes it `failed`; the trial is never re-registered or replaced."""
    _require_inputs(
        experiment_id=experiment_id, hypothesis_id=hypothesis_id, strategy_id=strategy_id, symbol=symbol,
    )
    require_registered_bars_provenance(bars_provenance)
    spec = require_native_exact_target_spec(economic_spec)
    require_official_execution_pricing_parity(spec)
    require_official_weight_to_share_parity(spec)
    wts = spec.weight_to_share.normalized()

    run_dir = Path(run_dir)
    bars_csv = Path(bars_csv)
    backtest_bars_csv = Path(backtest_bars_csv)
    for p in (bars_csv, backtest_bars_csv):
        if not p.exists():
            raise FileNotFoundError(f"Missing required native-signal input: {p}")

    bars = pd.read_csv(bars_csv)
    bars = bars[bars["symbol"].astype(str) == symbol]
    bar_ts = pd.to_datetime(bars["end_ts"], utc=True)
    folds, holdout_start, dataset_end = plan_native_folds(
        t_min=bar_ts.min(), t_max=bar_ts.max(),
        evaluation_start_utc=evaluation_start_utc,
        test_months=test_months, holdout_months=holdout_months,
        fixed_holdout_boundary=fixed_holdout_boundary,
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

    trial_id, identity = build_native_signal_trial_identity(
        experiment_id=experiment_id, hypothesis_id=hypothesis_id, strategy_id=strategy_id,
        symbol=symbol, semantic_fingerprint=expected_semantic_fingerprint,
        required_history_bars=required_history_bars, bars_provenance=bars_provenance,
        evaluation_start_utc=evaluation_start_utc, test_months=test_months,
        holdout_months=holdout_months, economic_spec=spec, capital_sizing=expected_capital_sizing,
        stress_contract=expected_stress_contract,
        canonical_timeframe_identity=canonical_timeframe_identity,
        fixed_holdout_boundary=fixed_holdout_boundary,
    )

    store = ResearchResultStore(registry_db or default_db_path(default_root()))
    try:
        registered = store.get_trial(trial_id)
    except KeyError as exc:
        raise NativeSignalError(
            f"trial {trial_id} is not registered: register every trial before any attempt or emission"
        ) from exc
    if registered["strategy_id"] != strategy_id or registered["experiment_id"] != experiment_id:
        raise NativeSignalError(f"registered trial {trial_id} does not match this evaluation's strategy/experiment")

    attempt_id, attempt_index = store.begin_attempt(
        trial_id=trial_id, origin="mqk-native-signal-economic-wf",
        metadata={"native_signal_protocol": NATIVE_SIGNAL_STREAM_PROTOCOL_ID},
    )

    try:
        emit_signals()
        for p in (Path(signals_csv), Path(signals_meta_json)):
            if not p.exists():
                raise NativeSignalError(f"emitter did not produce the required native-signal artifact: {p}")
        signals, meta = _load_signals(
            signals_csv, signals_meta_json, strategy_id=strategy_id, symbol=symbol,
            backtest_bars_sha256=expected_sha, expected_timeframe_secs=expected_timeframe_secs,
            expected_semantic_fingerprint=expected_semantic_fingerprint,
            expected_required_history_bars=required_history_bars, equity_usd=wts.equity_usd,
            expected_capital_sizing=expected_capital_sizing,
        )

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
                    "target_qty": int(r["target_qty"]),
                })
            if in_fold.empty:
                raise NativeSignalError(f"no native signals inside fold {f.fold}")
        oos_path = eval_dir / "walk_forward_oos_predictions.csv"
        with open(oos_path, "w", newline="", encoding="utf-8") as fh:
            w = csv.DictWriter(
                fh, fieldnames=["fold", "symbol", "decision_ts", "ml_score", "target_qty"], lineterminator="\n"
            )
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

        fidelity = native_exact_target_fidelity(economic_out, symbol, signals)
        if fidelity.agreement < NATIVE_EXECUTION_FIDELITY_FLOOR:
            raise NativeSignalError(
                f"execution fidelity {fidelity.agreement:.3f} < floor {NATIVE_EXECUTION_FIDELITY_FLOOR} "
                f"({fidelity.matched_bars}/{fidelity.evaluated_bars} bars held exactly the native target "
                f"quantity; max quantity gap {fidelity.max_abs_qty_gap}): the economic protocol did not "
                "implement the strategy's exact target positions"
            )

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
        "execution_fidelity": round(fidelity.agreement, 6),
        "max_abs_target_qty_gap": int(fidelity.max_abs_qty_gap),
        "effective_bar_history_len": int(meta["effective_bar_history_len"]),
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
