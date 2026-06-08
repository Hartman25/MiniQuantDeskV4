"""
SYMBOL-INPUTS-PRODUCER-01 — Premarket symbol_inputs artifact producer.

Converts in-memory bar history (and optional precomputed liquidity metrics)
into a `symbol-inputs-v1` artifact consumable by
`premarket_revalidation.evaluate_premarket_watchlist`.

This module is an artifact assembler — it does not invent gate logic. It
delegates to the existing MAIN scanner gates:
- `data_quality.evaluate_data_quality`  (data_quality_passed, latest_bar_ts)
- `liquidity.evaluate_liquidity`        (liquidity_passed, spread/slippage/rvol)
- `regime.evaluate_regime`              (regime_label, regime_score)

Hard invariants:
- `approved_for_live` is ALWAYS False at both the artifact and per-symbol level
- No imports from broker adapters, OMS tables, execution orchestrator, or DB
- No network imports (requests, urllib, http.client, aiohttp, psycopg, sqlalchemy)
- No daemon/runtime imports
- No subprocess
- Artifact-only — caller supplies bars; this module does not fetch market data
- Missing/failed inputs produce explicit fail-closed records, never silent omission

Fail-closed cases represented per symbol (never disappear silently):
- no bars                       -> data_quality_passed=False, rejection_reason="no_bars"
- insufficient bars             -> data_quality_passed=False, rejection_reason="insufficient_completed_bars"
- stale latest bar              -> data_quality_passed=False, rejection_reason="latest_bar_stale"
- data quality failure (other)  -> data_quality_passed=False, rejection_reason=<dq reason>
- liquidity metrics absent      -> liquidity_passed=False, rejection_reason="symbol_input_liquidity_metrics_missing"
- liquidity gate failure        -> liquidity_passed=False, rejection_reason=<liquidity reason>
- regime unclassified/low score -> regime_label reflects gate output (e.g. "unclassified"),
                                    rejection_reason=<regime reason>
- missing/invalid latest price  -> latest_close=None, rejection_reason="symbol_input_missing_latest_price"

Notes:
- "Strategy compatibility" of the regime is a premarket-level concern (it needs the
  watchlist's strategy_assignments) and is intentionally NOT decided here. This
  producer reports `regime_label`/`regime_score` honestly; compatibility is judged
  downstream by `evaluate_premarket_watchlist`.
"""
from __future__ import annotations

import json
from dataclasses import dataclass, field
from datetime import datetime, timezone
from pathlib import Path
from typing import Any, Optional

from mqk_research.scanner.data_quality import (
    DataQualityConfig,
    DataQualityResult,
    evaluate_data_quality,
)
from mqk_research.scanner.liquidity import (
    LiquidityConfig,
    LiquidityMetrics,
    LiquidityResult,
    evaluate_liquidity,
)
from mqk_research.scanner.regime import (
    RegimeConfig,
    RegimeResult,
    evaluate_regime,
)

SCHEMA_VERSION = "symbol-inputs-v1"

# ---------------------------------------------------------------------------
# Stable reason constants (producer-local; gate-native reasons are propagated
# verbatim into `rejection_reason` / `reason_tags` where applicable).
# ---------------------------------------------------------------------------

REASON_LIQUIDITY_METRICS_MISSING = "symbol_input_liquidity_metrics_missing"
REASON_MISSING_LATEST_PRICE = "symbol_input_missing_latest_price"

ALL_PRODUCER_REASONS = (
    REASON_LIQUIDITY_METRICS_MISSING,
    REASON_MISSING_LATEST_PRICE,
)


# ---------------------------------------------------------------------------
# Input spec — one per symbol, pure in-memory data
# ---------------------------------------------------------------------------

@dataclass
class SymbolInputSpec:
    """In-memory bundle of inputs needed to build one symbol_inputs record.

    `bars`: ascending-by-time list of bar dicts (ts, open, high, low, close, volume).
    `liquidity_metrics`: precomputed LiquidityMetrics. If None, liquidity is
    explicitly reported as not-evaluated (fail-closed) — this producer does not
    derive ADV/spread/slippage estimates from intraday bars.
    """
    symbol: str
    timeframe: str
    bars: list[dict[str, Any]] = field(default_factory=list)
    liquidity_metrics: Optional[LiquidityMetrics] = None
    suppressed: bool = False


# ---------------------------------------------------------------------------
# Helpers
# ---------------------------------------------------------------------------

def _dedupe(items: list[str]) -> list[str]:
    seen: set[str] = set()
    out: list[str] = []
    for item in items:
        if item not in seen:
            seen.add(item)
            out.append(item)
    return out


def _latest_complete_bar(bars: list[dict[str, Any]]) -> Optional[dict[str, Any]]:
    """Return the latest bar, skipping an explicitly-incomplete trailing bar."""
    if not bars:
        return None
    working = list(bars)
    last = working[-1]
    if isinstance(last, dict) and last.get("is_complete") is False:
        working = working[:-1]
    if not working:
        return None
    return working[-1]


def _extract_latest_close(bars: list[dict[str, Any]]) -> Optional[float]:
    """Best-effort latest close extracted honestly — None on any ambiguity."""
    bar = _latest_complete_bar(bars)
    if bar is None:
        return None
    raw = bar.get("close")
    try:
        value = float(raw)
    except (TypeError, ValueError):
        return None
    if value != value:  # NaN — the only float that compares unequal to itself
        return None
    if value <= 0.0:
        return None
    return value


# ---------------------------------------------------------------------------
# Per-symbol record builder
# ---------------------------------------------------------------------------

def build_symbol_input_record(
    spec: SymbolInputSpec,
    *,
    dq_config: Optional[DataQualityConfig] = None,
    liquidity_config: Optional[LiquidityConfig] = None,
    regime_config: Optional[RegimeConfig] = None,
    now_utc: Optional[datetime] = None,
) -> dict[str, Any]:
    """
    Build a single symbol_inputs record (the value stored at symbols[symbol]).

    Delegates to `evaluate_data_quality`, `evaluate_liquidity`, and
    `evaluate_regime`. Each gate is evaluated independently — one gate's
    failure does not suppress reporting of the others' honest output, except
    that liquidity/regime are only evaluated when bars carry a valid bar
    history (data_quality_passed=True), since both gates assume DQ-clean bars.

    Returns a plain dict matching the symbol-inputs-v1 per-symbol schema.
    `approved_for_live` is intentionally absent from per-symbol records —
    live-eligibility is an artifact-level invariant only (always False).
    """
    now = now_utc or datetime.now(timezone.utc)
    reason_tags: list[str] = []
    rejection_reason: Optional[str] = None

    dq_result: DataQualityResult = evaluate_data_quality(
        spec.symbol, spec.timeframe, spec.bars, dq_config, now
    )
    data_quality_passed = dq_result.passed
    reason_tags.extend(dq_result.reason_tags)
    if not data_quality_passed:
        rejection_reason = dq_result.rejection_reason

    latest_bar_ts = dq_result.latest_bar_ts
    latest_close = _extract_latest_close(spec.bars) if data_quality_passed else None

    # Liquidity — only meaningful once DQ confirms a clean bar history.
    liquidity_passed = False
    liquidity_score: Optional[float] = None
    spread_estimate_bps: Optional[float] = None
    slippage_estimate_bps: Optional[float] = None
    relative_volume: Optional[float] = None

    if data_quality_passed:
        if spec.liquidity_metrics is None:
            reason_tags.append(REASON_LIQUIDITY_METRICS_MISSING)
            if rejection_reason is None:
                rejection_reason = REASON_LIQUIDITY_METRICS_MISSING
        else:
            liq_result: LiquidityResult = evaluate_liquidity(
                spec.symbol, spec.liquidity_metrics, liquidity_config
            )
            liquidity_passed = liq_result.passed
            liquidity_score = liq_result.score
            spread_estimate_bps = liq_result.spread_estimate_bps
            slippage_estimate_bps = liq_result.slippage_estimate_bps
            relative_volume = liq_result.relative_volume
            reason_tags.extend(liq_result.reason_tags)
            if not liquidity_passed and rejection_reason is None:
                rejection_reason = liq_result.rejection_reason

    # Regime — only meaningful once DQ confirms a clean bar history.
    regime_label: Optional[str] = None
    regime_score: Optional[float] = None

    if data_quality_passed:
        regime_result: RegimeResult = evaluate_regime(spec.symbol, spec.bars, regime_config)
        regime_label = regime_result.regime_label
        regime_score = regime_result.regime_score
        reason_tags.extend(regime_result.reason_tags)
        if not regime_result.passed and rejection_reason is None:
            rejection_reason = regime_result.rejection_reason

    # Latest price — must be present and positive once DQ has passed.
    if data_quality_passed and latest_close is None:
        reason_tags.append(REASON_MISSING_LATEST_PRICE)
        if rejection_reason is None:
            rejection_reason = REASON_MISSING_LATEST_PRICE

    return {
        "symbol": spec.symbol,
        "timeframe": spec.timeframe,
        "latest_bar_ts": latest_bar_ts,
        "latest_close": latest_close,
        "data_quality_passed": data_quality_passed,
        "data_quality_score": dq_result.score if data_quality_passed else 0.0,
        "liquidity_passed": liquidity_passed,
        "liquidity_score": liquidity_score,
        "regime_label": regime_label,
        "regime_score": regime_score,
        "spread_estimate_bps": spread_estimate_bps,
        "slippage_estimate_bps": slippage_estimate_bps,
        "relative_volume": relative_volume,
        "suppressed": bool(spec.suppressed),
        "reason_tags": _dedupe(reason_tags),
        "rejection_reason": rejection_reason,
    }


# ---------------------------------------------------------------------------
# Artifact builder
# ---------------------------------------------------------------------------

def build_symbol_inputs(
    specs: list[SymbolInputSpec],
    *,
    trade_date: str,
    source: str,
    generated_at_utc: Optional[str] = None,
    dq_config: Optional[DataQualityConfig] = None,
    liquidity_config: Optional[LiquidityConfig] = None,
    regime_config: Optional[RegimeConfig] = None,
    now_utc: Optional[datetime] = None,
    notes: str = "artifact-only; no broker/daemon/order calls",
) -> dict[str, Any]:
    """
    Build a full symbol-inputs-v1 artifact dict from in-memory per-symbol specs.

    `approved_for_live` is hard-forced to False — not overrideable by caller.
    Does not write anything; pure assembly. Use `write_symbol_inputs_artifact`
    to persist.
    """
    now = now_utc or datetime.now(timezone.utc)
    generated = generated_at_utc or now.isoformat()

    symbols: dict[str, dict[str, Any]] = {}
    for spec in specs:
        symbols[spec.symbol] = build_symbol_input_record(
            spec,
            dq_config=dq_config,
            liquidity_config=liquidity_config,
            regime_config=regime_config,
            now_utc=now,
        )

    return {
        "schema_version": SCHEMA_VERSION,
        "generated_at_utc": generated,
        "trade_date": trade_date,
        "source": source,
        "symbols": symbols,
        "approved_for_live": False,
        "notes": notes,
    }


# ---------------------------------------------------------------------------
# Writer / reader
# ---------------------------------------------------------------------------

def write_symbol_inputs_artifact(artifact: dict[str, Any], path: str) -> Path:
    """Write a symbol_inputs artifact as deterministic JSON.

    Parent directories are created automatically. `approved_for_live` is
    forced False on the persisted copy regardless of the input dict —
    defense in depth against a tampered/forged in-memory artifact.
    """
    out = Path(path)
    out.parent.mkdir(parents=True, exist_ok=True)
    safe = dict(artifact)
    safe["approved_for_live"] = False
    out.write_text(json.dumps(safe, indent=2, sort_keys=False, default=str), encoding="utf-8")
    return out


def load_symbol_inputs_artifact(path: str) -> dict[str, Any]:
    """Load and validate a symbol_inputs artifact from JSON.

    Fail-closed: raises ValueError on missing file, invalid JSON, wrong/missing
    schema_version, or a non-dict `symbols` field. `approved_for_live` is forced
    False on the returned copy regardless of what the file contains — a forged
    `approved_for_live=true` on disk can never propagate into evaluation.
    """
    text = Path(path).read_text(encoding="utf-8")
    data = json.loads(text)
    if not isinstance(data, dict):
        raise ValueError(f"symbol_inputs artifact at {path!r} is not a JSON object")
    if data.get("schema_version") != SCHEMA_VERSION:
        raise ValueError(
            f"symbol_inputs artifact at {path!r} has invalid schema_version "
            f"{data.get('schema_version')!r}; expected {SCHEMA_VERSION!r}"
        )
    if not isinstance(data.get("symbols"), dict):
        raise ValueError(f"symbol_inputs artifact at {path!r} is missing a 'symbols' object")
    data = dict(data)
    data["approved_for_live"] = False
    return data


# ---------------------------------------------------------------------------
# Premarket integration adapter
# ---------------------------------------------------------------------------

def extract_symbol_inputs_map(artifact: dict[str, Any]) -> dict[str, dict[str, Any]]:
    """
    Adapt a symbol-inputs-v1 artifact into the flat `symbol_inputs` dict
    expected by `premarket_revalidation.evaluate_premarket_watchlist`.

    Fail-closed: a malformed or schema-mismatched artifact yields an empty
    map (never a fabricated/partial map), which causes the premarket
    evaluator to report `premarket_symbol_input_missing` for the top symbol —
    the same honest failure as a genuinely absent artifact.

    `approved_for_live` is never read from the artifact and never propagated
    into the returned per-symbol records — premarket revalidation only
    consults the watchlist's own `approved_for_live` field.
    """
    if not isinstance(artifact, dict) or artifact.get("schema_version") != SCHEMA_VERSION:
        return {}
    symbols = artifact.get("symbols")
    if not isinstance(symbols, dict):
        return {}
    out: dict[str, dict[str, Any]] = {}
    for symbol, record in symbols.items():
        if isinstance(symbol, str) and isinstance(record, dict):
            out[symbol] = dict(record)
    return out
