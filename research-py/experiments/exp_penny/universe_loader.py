"""
EXP-PENNY-01A — local universe file ingestion.

Supports .json and .csv universe files. No external API calls, no broker
calls, no DB writes, no OMS/outbox/inbox access.

Usage:
    from experiments.exp_penny.universe_loader import load_universe
    rows = load_universe("sample_universe.json")
    rows = load_universe("sample_universe.csv")
"""
from __future__ import annotations

import csv
import json
from pathlib import Path
from typing import Any


REQUIRED_FIELDS: tuple[str, ...] = (
    "symbol",
    "price",
    "bid",
    "ask",
    "volume",
    "adv_20d_usd",
    "ma200_slope_20d",
    "ma50_slope_20d",
    "consolidation_range_pct",
    "breakout_level",
    "breakout_rvol",
    "gap_flag",
    "halt_flag",
)

FLOAT_FIELDS: frozenset[str] = frozenset(
    (
        "price",
        "bid",
        "ask",
        "adv_20d_usd",
        "rvol",
        "ma200",
        "ma50",
        "ma200_slope_20d",
        "ma50_slope_20d",
        "consolidation_high",
        "consolidation_low",
        "consolidation_range_pct",
        "breakout_level",
        "breakout_rvol",
    )
)

INT_FIELDS: frozenset[str] = frozenset(("volume", "adv_20d_shares"))

BOOL_FIELDS: frozenset[str] = frozenset(("gap_flag", "halt_flag", "news_flag"))

_BOOL_TRUE = frozenset(("true", "1", "yes", "y"))
_BOOL_FALSE = frozenset(("false", "0", "no", "n", ""))


class UniverseLoadError(Exception):
    """Raised when the universe file cannot be loaded or validated."""


def _parse_bool(value: str, row_num: int, field: str) -> bool | None:
    v = value.strip().lower()
    if v in _BOOL_TRUE:
        return True
    if v in _BOOL_FALSE:
        return False
    raise UniverseLoadError(
        f"Row {row_num}: field '{field}' has invalid boolean value '{value}'. "
        f"Expected one of: true/false/1/0/yes/no/y/n or empty."
    )


def _parse_float(value: str, row_num: int, field: str) -> float:
    try:
        return float(value)
    except (ValueError, TypeError):
        raise UniverseLoadError(
            f"Row {row_num}: field '{field}' has invalid numeric value '{value}'."
        )


def _parse_int(value: str, row_num: int, field: str) -> int:
    try:
        return int(value)
    except (ValueError, TypeError):
        raise UniverseLoadError(
            f"Row {row_num}: field '{field}' has invalid integer value '{value}'."
        )


def _coerce_csv_row(raw: dict[str, str], row_num: int) -> dict[str, Any]:
    out: dict[str, Any] = {}

    for field, value in raw.items():
        if field.startswith("_"):
            continue  # skip comment fields

        stripped = value.strip() if value is not None else ""

        if field in FLOAT_FIELDS:
            if stripped == "" or stripped.lower() in ("null", "none"):
                out[field] = None
            else:
                out[field] = _parse_float(stripped, row_num, field)

        elif field in INT_FIELDS:
            if stripped == "" or stripped.lower() in ("null", "none"):
                out[field] = None
            else:
                out[field] = _parse_int(stripped, row_num, field)

        elif field in BOOL_FIELDS:
            if stripped.lower() in ("null", "none"):
                out[field] = None
            else:
                out[field] = _parse_bool(stripped, row_num, field)

        else:
            out[field] = stripped if stripped != "" else None

    return out


def _validate_row(row: dict[str, Any], row_num: int) -> None:
    symbol = row.get("symbol")
    if not symbol or (isinstance(symbol, str) and not symbol.strip()):
        raise UniverseLoadError(f"Row {row_num}: 'symbol' is blank or missing.")
    for field in REQUIRED_FIELDS:
        if field not in row:
            raise UniverseLoadError(
                f"Row {row_num} (symbol={symbol!r}): required field '{field}' is missing."
            )


def _load_json(path: Path) -> list[dict[str, Any]]:
    try:
        text = path.read_text(encoding="utf-8")
    except OSError as exc:
        raise UniverseLoadError(f"Cannot read file: {exc}") from exc

    try:
        raw = json.loads(text)
    except json.JSONDecodeError as exc:
        raise UniverseLoadError(f"File is not valid JSON: {exc}") from exc

    if not isinstance(raw, list):
        raise UniverseLoadError("JSON universe file must be an array at the root level.")

    rows: list[dict[str, Any]] = []
    for i, item in enumerate(raw, start=1):
        if not isinstance(item, dict):
            raise UniverseLoadError(f"Row {i}: expected a JSON object, got {type(item).__name__}.")
        _validate_row(item, i)
        rows.append(item)

    if not rows:
        raise UniverseLoadError("Universe file is empty — no rows found.")

    return rows


def _load_csv(path: Path) -> list[dict[str, Any]]:
    try:
        text = path.read_text(encoding="utf-8")
    except OSError as exc:
        raise UniverseLoadError(f"Cannot read file: {exc}") from exc

    lines = [ln for ln in text.splitlines() if ln.strip()]
    if not lines:
        raise UniverseLoadError("Universe CSV file is empty — no rows found.")

    import io
    reader = csv.DictReader(io.StringIO("\n".join(lines)))

    if reader.fieldnames is None:
        raise UniverseLoadError("Universe CSV file has no header row.")

    headers = [h.strip() for h in reader.fieldnames if h is not None]

    for req in REQUIRED_FIELDS:
        if req not in headers:
            raise UniverseLoadError(
                f"CSV is missing required column '{req}'. "
                f"Found columns: {headers}"
            )

    rows: list[dict[str, Any]] = []
    for i, raw_row in enumerate(reader, start=2):  # row 1 is header
        coerced = _coerce_csv_row(dict(raw_row), i)
        _validate_row(coerced, i)
        rows.append(coerced)

    if not rows:
        raise UniverseLoadError("Universe CSV file is empty — no data rows found.")

    return rows


def load_universe(path: str | Path) -> list[dict[str, Any]]:
    """
    Load a universe file (.json or .csv) and return a list of row dicts.

    Raises UniverseLoadError on any validation failure.
    """
    p = Path(path)

    if not p.exists():
        raise UniverseLoadError(f"Universe file not found: {p}")

    suffix = p.suffix.lower()
    if suffix == ".json":
        return _load_json(p)
    if suffix == ".csv":
        return _load_csv(p)

    raise UniverseLoadError(
        f"Unsupported universe file extension '{suffix}'. Supported: .json, .csv"
    )
