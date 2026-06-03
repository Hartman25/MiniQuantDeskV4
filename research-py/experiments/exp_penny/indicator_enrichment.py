"""
EXP-PENNY-01A-INDICATOR-ENRICHMENT — local OHLCV enrichment layer.

Computes technical indicator fields from local per-symbol OHLCV CSV files.
No network calls, no broker calls, no OMS/outbox/inbox writes, no DB access.

Usage:
    from experiments.exp_penny.indicator_enrichment import (
        load_ohlcv_csv,
        compute_symbol_features,
        enrich_universe_rows,
        IndicatorEnrichmentError,
    )
    bars = load_ohlcv_csv("samples/ohlcv/ACME.csv")
    features = compute_symbol_features(bars)
    enriched = enrich_universe_rows(rows, ohlcv_dir="samples/ohlcv")
"""
from __future__ import annotations

import csv
import io
from pathlib import Path
from typing import Any

MIN_BARS_FOR_FEATURES = 220  # MA200 + 20-bar slope lookback

_OHLCV_FLOAT_COLS = frozenset(("open", "high", "low", "close"))
_OHLCV_INT_COLS = frozenset(("volume",))
_OHLCV_REQUIRED_COLS = ("date", "open", "high", "low", "close", "volume")


class IndicatorEnrichmentError(Exception):
    """Raised when OHLCV enrichment cannot be completed."""


def load_ohlcv_csv(path: str | Path) -> list[dict[str, Any]]:
    """
    Load a per-symbol OHLCV CSV file and return a list of bar dicts.

    Expected columns: date, open, high, low, close, volume.
    Raises IndicatorEnrichmentError on missing file, missing columns, or
    invalid numeric values. close and volume must be positive.
    """
    p = Path(path)
    if not p.exists():
        raise IndicatorEnrichmentError(f"OHLCV file not found: {p}")

    try:
        text = p.read_text(encoding="utf-8")
    except OSError as exc:
        raise IndicatorEnrichmentError(f"Cannot read OHLCV file: {exc}") from exc

    lines = [ln for ln in text.splitlines() if ln.strip()]
    if not lines:
        raise IndicatorEnrichmentError(f"OHLCV file is empty: {p}")

    reader = csv.DictReader(io.StringIO("\n".join(lines)))
    if reader.fieldnames is None:
        raise IndicatorEnrichmentError(f"OHLCV file has no header row: {p}")

    headers = [h.strip().lower() for h in reader.fieldnames if h is not None]
    for col in _OHLCV_REQUIRED_COLS:
        if col not in headers:
            raise IndicatorEnrichmentError(
                f"OHLCV file {p.name} is missing required column '{col}'. Found: {headers}"
            )

    bars: list[dict[str, Any]] = []
    for row_num, raw in enumerate(reader, start=2):
        bar: dict[str, Any] = {}
        for k, v in raw.items():
            if k is None:
                continue
            key = k.strip().lower()
            stripped = v.strip() if v is not None else ""

            if key in _OHLCV_FLOAT_COLS:
                try:
                    val = float(stripped)
                except (ValueError, TypeError):
                    raise IndicatorEnrichmentError(
                        f"OHLCV row {row_num}: field '{key}' has invalid numeric value '{stripped}'."
                    )
                if val <= 0:
                    raise IndicatorEnrichmentError(
                        f"OHLCV row {row_num}: field '{key}' must be positive, got {val}."
                    )
                bar[key] = val
            elif key in _OHLCV_INT_COLS:
                try:
                    val = int(stripped)
                except (ValueError, TypeError):
                    raise IndicatorEnrichmentError(
                        f"OHLCV row {row_num}: field '{key}' has invalid integer value '{stripped}'."
                    )
                bar[key] = val
            else:
                bar[key] = stripped

        bars.append(bar)

    if not bars:
        raise IndicatorEnrichmentError(f"OHLCV file has no data rows: {p}")

    return bars


def _mean(values: list[float]) -> float:
    return sum(values) / len(values)


def compute_symbol_features(
    bars: list[dict[str, Any]],
    *,
    consolidation_days: int = 20,
    slope_days: int = 20,
) -> dict[str, Any]:
    """
    Compute technical indicator fields from a list of OHLCV bar dicts.

    Requires at least (200 + slope_days) bars = 220 by default.
    Raises IndicatorEnrichmentError on insufficient data, zero denominators,
    or empty windows. Does not invent values if data is absent.

    Returns dict with keys:
      ma50, ma200, ma50_slope_20d, ma200_slope_20d,
      consolidation_high, consolidation_low, consolidation_range_pct,
      breakout_level, breakout_rvol, adv_20d_shares, adv_20d_usd
    """
    min_required = 200 + slope_days
    if len(bars) < min_required:
        raise IndicatorEnrichmentError(
            f"Insufficient bars: need {min_required}, got {len(bars)}."
        )

    closes = [b["close"] for b in bars]
    highs = [b["high"] for b in bars]
    lows = [b["low"] for b in bars]
    volumes = [b["volume"] for b in bars]

    n = len(bars)

    # Current MA50 and MA200 (use last 50/200 closes)
    ma50 = _mean(closes[n - 50: n])
    ma200 = _mean(closes[n - 200: n])

    # MA50/MA200 at slope_days bars ago
    idx_ago = n - slope_days
    ma50_ago = _mean(closes[idx_ago - 50: idx_ago])
    ma200_ago = _mean(closes[idx_ago - 200: idx_ago])

    if ma50_ago <= 0:
        raise IndicatorEnrichmentError("MA50 baseline (slope_days ago) is zero or negative.")
    if ma200_ago <= 0:
        raise IndicatorEnrichmentError("MA200 baseline (slope_days ago) is zero or negative.")

    ma50_slope = (ma50 - ma50_ago) / ma50_ago
    ma200_slope = (ma200 - ma200_ago) / ma200_ago

    # Consolidation: prior consolidation_days bars, excluding the latest bar
    consol_start = n - consolidation_days - 1
    consol_end = n - 1  # slice is [consol_start:consol_end], excludes bar n-1
    consol_highs = highs[consol_start:consol_end]
    consol_lows = lows[consol_start:consol_end]

    if not consol_highs:
        raise IndicatorEnrichmentError("No bars available for consolidation window.")

    consolidation_high = max(consol_highs)
    consolidation_low = min(consol_lows)
    mid = (consolidation_high + consolidation_low) / 2.0
    if mid <= 0:
        raise IndicatorEnrichmentError("Consolidation midpoint is zero or negative.")
    consolidation_range_pct = (consolidation_high - consolidation_low) / mid * 100.0

    # Volume metrics over same prior window
    prior_vols = [float(v) for v in volumes[consol_start:consol_end]]
    if not prior_vols:
        raise IndicatorEnrichmentError("No bars available for volume window.")
    adv_20d_shares_f = _mean(prior_vols)
    if adv_20d_shares_f <= 0:
        raise IndicatorEnrichmentError("Average daily volume is zero or negative.")

    latest_close = closes[-1]
    if latest_close <= 0:
        raise IndicatorEnrichmentError("Latest close is zero or negative.")

    latest_volume = float(volumes[-1])
    breakout_rvol = latest_volume / adv_20d_shares_f
    adv_20d_usd = adv_20d_shares_f * latest_close

    return {
        "ma50": round(ma50, 6),
        "ma200": round(ma200, 6),
        "ma50_slope_20d": round(ma50_slope, 8),
        "ma200_slope_20d": round(ma200_slope, 8),
        "consolidation_high": round(consolidation_high, 6),
        "consolidation_low": round(consolidation_low, 6),
        "consolidation_range_pct": round(consolidation_range_pct, 4),
        "breakout_level": round(consolidation_high, 6),
        "breakout_rvol": round(breakout_rvol, 6),
        "adv_20d_shares": int(round(adv_20d_shares_f)),
        "adv_20d_usd": round(adv_20d_usd, 2),
    }


def enrich_universe_rows(
    rows: list[dict[str, Any]],
    *,
    ohlcv_dir: str | Path,
) -> list[dict[str, Any]]:
    """
    Enrich universe rows with computed technical fields from per-symbol OHLCV CSVs.

    For each row, looks for <SYMBOL>.csv in ohlcv_dir. Computed fields are merged
    into the row only if not already present (None or absent). Raises
    IndicatorEnrichmentError if the OHLCV file for a symbol is missing or insufficient.
    """
    ohlcv_path = Path(ohlcv_dir)
    enriched: list[dict[str, Any]] = []

    for row in rows:
        symbol = row.get("symbol")
        if not symbol:
            raise IndicatorEnrichmentError("Row is missing 'symbol' field.")

        ohlcv_file = ohlcv_path / f"{symbol}.csv"
        if not ohlcv_file.exists():
            raise IndicatorEnrichmentError(
                f"OHLCV file not found for symbol '{symbol}': expected {ohlcv_file}"
            )

        bars = load_ohlcv_csv(ohlcv_file)
        features = compute_symbol_features(bars)

        merged = dict(row)
        for k, v in features.items():
            if merged.get(k) is None:
                merged[k] = v

        enriched.append(merged)

    return enriched
