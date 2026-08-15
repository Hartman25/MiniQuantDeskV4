from __future__ import annotations

from dataclasses import dataclass
from typing import Any, Dict, FrozenSet, List, Optional, Tuple, Literal

import pandas as pd
from sqlalchemy import text
from sqlalchemy.engine import Engine


@dataclass(frozen=True)
class BarsQuery:
    symbols: List[str]
    start_utc: pd.Timestamp
    end_utc: pd.Timestamp
    timeframe: str  # e.g. "1D"


# ---------------------------------------------------------------------------
# BKT-DATA-PROVENANCE-POINT-IN-TIME-01
#
# md_bars' close-column resolution (_detect_md_bars_schema below) tries
# several candidate column names, including some that would represent a
# DIFFERENT price-adjustment convention (adj_close/close_adj) than a bare
# "close"/close_micros. Silently picking whichever is present, with no
# record of which one was used, is the audit's A17 finding -- this section
# makes that resolution explicit and auditable instead.
#
# VERIFIED FACTS (read-only investigation, 2026-08; see
# docs/research/Research_Backtest_V1_Closeout_Audit.md Section on P8):
#   - md_bars' actual schema (migrations 0003_backtest_schema.sql +
#     0042_md_bars_provider_metadata.sql) has never had an adj_close/
#     close_adj column -- only close_micros. There is currently no genuine
#     split/dividend-adjusted candidate column in this system's schema for
#     _pick_first_present to have silently preferred.
#   - The only active equity historical-data provider in this codebase
#     (core-rs/crates/mqk-md/src/alpaca_provider.rs) explicitly requests
#     Alpaca's /v2/stocks/bars with adjustment=raw. Per Alpaca's own
#     documentation (docs.alpaca.markets/reference/stockbars, confirmed
#     2026-08): "raw: no adjustments" -- i.e. split/dividend/spin-off
#     adjustments are NOT applied. This is Alpaca's own default and is
#     requested explicitly (not left implicit) by this codebase.
# These two facts are recorded here as an explicit, versioned assertion
# (_VERIFIED_RAW_UNADJUSTED_CLOSE_COLUMNS / _VERIFIED_RAW_UNADJUSTED_PROVIDER_IDS)
# rather than assumed silently -- so a FUTURE change to either side (a new
# adjusted-price ingestion path, or a schema change adding an adjusted
# column) is forced to make an explicit decision here instead of the old
# silent-preference behavior continuing to apply unnoticed.
#
# provider_id is genuinely INCOMPLETE for pre-existing rows in this
# database today (a separate, already-tracked gap --
# MARKET-DATA-PROVIDER-PROVENANCE-01 -- where some ingestion paths wrote
# provider_id='unknown' even for real Alpaca-sourced bars). This module
# does not attempt to resolve that attribution bug; it fails closed
# (PRICE_CONVENTION_UNVERIFIABLE) whenever the observed provider_id set for
# a query window is not exactly the verified set, rather than assuming
# 'unknown' rows are safe.

PRICE_CONVENTION_RAW_UNADJUSTED = "raw_unadjusted"
PRICE_CONVENTION_UNVERIFIABLE = "unverifiable"

_VERIFIED_RAW_UNADJUSTED_CLOSE_COLUMNS: FrozenSet[str] = frozenset({"close_micros"})
_VERIFIED_RAW_UNADJUSTED_PROVIDER_IDS: FrozenSet[str] = frozenset({"alpaca"})

_CONVENTION_BASIS_NOTE = (
    "core-rs/crates/mqk-md/src/alpaca_provider.rs requests Alpaca /v2/stocks/bars "
    "with adjustment=raw (verified against Alpaca's official documentation: "
    "raw = no adjustments applied), and md_bars has never had an adjusted-price "
    "column in its schema. price_adjustment_convention is asserted "
    f"{PRICE_CONVENTION_RAW_UNADJUSTED!r} only when BOTH the resolved close "
    "column and every provider_id observed for the exact query window are "
    "within the independently-verified set; otherwise it is reported "
    f"{PRICE_CONVENTION_UNVERIFIABLE!r} rather than assumed."
)


def classify_price_convention(*, close_col: str, provider_ids_observed: FrozenSet[str]) -> str:
    """Pure decision logic, no IO. Returns PRICE_CONVENTION_RAW_UNADJUSTED
    only when the resolved close column AND every provider_id observed for
    the query window are within the independently-verified raw/unadjusted
    set. An empty, unknown, mixed, or unmapped provider_id set, or any close
    column other than the one verified column, returns
    PRICE_CONVENTION_UNVERIFIABLE -- never silently assumed."""
    if close_col not in _VERIFIED_RAW_UNADJUSTED_CLOSE_COLUMNS:
        return PRICE_CONVENTION_UNVERIFIABLE
    if not provider_ids_observed:
        return PRICE_CONVENTION_UNVERIFIABLE
    if not provider_ids_observed <= _VERIFIED_RAW_UNADJUSTED_PROVIDER_IDS:
        return PRICE_CONVENTION_UNVERIFIABLE
    return PRICE_CONVENTION_RAW_UNADJUSTED


class PriceProvenanceUnverifiable(RuntimeError):
    """Raised by require_verified_price_provenance when a bars provenance
    record cannot be asserted as a known, verified adjustment convention."""


def require_verified_price_provenance(provenance: Dict[str, Any], *, context: Optional[str] = None) -> None:
    """Fail-closed gate for any research/promotion consumer that needs a
    verified price-adjustment convention before treating bars data as
    credible economic evidence. Raises PriceProvenanceUnverifiable unless
    provenance["price_adjustment_convention"] == PRICE_CONVENTION_RAW_UNADJUSTED."""
    if provenance.get("price_adjustment_convention") != PRICE_CONVENTION_RAW_UNADJUSTED:
        where = f" ({context})" if context else ""
        raise PriceProvenanceUnverifiable(
            f"price provenance{where} is not a verified adjustment convention "
            f"(convention={provenance.get('price_adjustment_convention')!r}, "
            f"close_column={provenance.get('close_column')!r}, "
            f"provider_ids_observed={provenance.get('provider_ids_observed')!r}); "
            "refusing to treat this bars data as having a known price-adjustment "
            "convention"
        )


MICROS_SCALE = 1_000_000.0
EpochUnit = Literal["s", "ms"]

# Deterministic boundary:
# - epoch seconds today ~ 1.7e9 (10 digits)
# - epoch millis today ~ 1.7e12 (13 digits)
# We treat >= 2e10 as millis (safe through year ~2604 for seconds).
EPOCH_MS_THRESHOLD = 20_000_000_000


def _require_tz(ts: pd.Timestamp, name: str) -> None:
    if ts.tz is None:
        raise ValueError(f"{name} must be timezone-aware (UTC recommended)")


def _load_md_bars_columns(engine: Engine) -> List[str]:
    q = text(
        """
        select column_name
        from information_schema.columns
        where table_schema='public'
          and table_name='md_bars'
        order by ordinal_position asc
        """
    )
    with engine.connect() as cxn:
        rows = cxn.execute(q).fetchall()
    return [r[0] for r in rows]


def _column_db_type(engine: Engine, col: str) -> str:
    q = text(
        """
        select data_type
        from information_schema.columns
        where table_schema='public'
          and table_name='md_bars'
          and column_name=:col
        """
    )
    with engine.connect() as cxn:
        row = cxn.execute(q, {"col": col}).fetchone()
    if row is None:
        return ""
    return str(row[0]).lower()


def _pick_first_present(colset: set[str], candidates: List[str]) -> Optional[str]:
    for c in candidates:
        if c in colset:
            return c
    return None


def _detect_md_bars_schema(engine: Engine) -> Tuple[str, str, str, str, str, str, bool]:
    """
    Returns:
      (ts_col, open_col, high_col, low_col, close_col, volume_col, has_is_complete)
    """
    cols = _load_md_bars_columns(engine)
    colset = set(cols)

    ts_candidates = [
        "ts_utc",
        "bar_ts_utc",
        "end_ts",
        "end_ts_utc",
        "bar_end_ts",
        "bar_end_ts_utc",
        "ts",
        "bar_ts",
        "timestamp",
        "time",
        "t",
    ]
    ts_col = _pick_first_present(colset, ts_candidates)

    open_candidates = ["open", "o", "open_micros"]
    high_candidates = ["high", "h", "high_micros"]
    low_candidates = ["low", "l", "low_micros"]
    close_candidates = ["close", "c", "close_micros", "adj_close", "close_adj"]
    volume_candidates = ["volume", "v", "vol"]

    open_col = _pick_first_present(colset, open_candidates)
    high_col = _pick_first_present(colset, high_candidates)
    low_col = _pick_first_present(colset, low_candidates)
    close_col = _pick_first_present(colset, close_candidates)
    volume_col = _pick_first_present(colset, volume_candidates)

    missing = []
    if ts_col is None:
        missing.append("timestamp")
    if open_col is None:
        missing.append("open")
    if high_col is None:
        missing.append("high")
    if low_col is None:
        missing.append("low")
    if close_col is None:
        missing.append("close")
    if volume_col is None:
        missing.append("volume")

    if missing:
        raise RuntimeError(
            "md_bars schema detection failed (missing: "
            + ", ".join(missing)
            + "). Available columns: "
            + ", ".join(cols)
        )

    has_is_complete = "is_complete" in colset
    return ts_col, open_col, high_col, low_col, close_col, volume_col, has_is_complete


def _to_price_float(series: pd.Series, source_col: str) -> pd.Series:
    if source_col.endswith("_micros"):
        return series.astype("int64") / MICROS_SCALE
    return series.astype(float)


def infer_epoch_unit_strict(engine: Engine, ts_col: str) -> EpochUnit:
    """
    Deterministic, fail-closed inference for integer epoch timestamps.
    - If values span both sides of EPOCH_MS_THRESHOLD => mixed units => error.
    - Otherwise choose:
        v >= threshold => ms
        v <  threshold => s
    """
    mm_q = text(f"select min({ts_col}) as min_v, max({ts_col}) as max_v from md_bars where {ts_col} is not null")
    cnt_q = text(
        f"""
        select
          sum(case when {ts_col} >= :thresh then 1 else 0 end) as n_ms,
          sum(case when {ts_col} <  :thresh then 1 else 0 end) as n_s,
          count(*) as n_total
        from md_bars
        where {ts_col} is not null
        """
    )
    with engine.connect() as cxn:
        mm = cxn.execute(mm_q).fetchone()
        cnt = cxn.execute(cnt_q, {"thresh": int(EPOCH_MS_THRESHOLD)}).fetchone()

    if mm is None or mm[0] is None or mm[1] is None:
        # No data: downstream history will fail anyway, but keep deterministic default.
        return "s"

    min_v = int(mm[0])
    max_v = int(mm[1])

    n_ms = int(cnt[0] or 0)
    n_s = int(cnt[1] or 0)
    n_total = int(cnt[2] or 0)

    # Mixed => unsafe
    if n_ms > 0 and n_s > 0:
        raise RuntimeError(
            "md_bars timestamp unit is MIXED/AMBIGUOUS (unsafe).\n"
            f"  ts_col={ts_col}\n"
            f"  threshold(EPOCH_MS_THRESHOLD)={EPOCH_MS_THRESHOLD}\n"
            f"  counts: n_ms={n_ms} n_s={n_s} n_total={n_total}\n"
            f"  raw min={min_v} raw max={max_v}\n"
            "Fix your ingestor/storage so md_bars uses a single unit (epoch seconds OR epoch millis) consistently."
        )

    return "ms" if n_ms > 0 else "s"


def _to_epoch_bound(ts_utc: pd.Timestamp, unit: EpochUnit) -> int:
    # pandas Timestamp -> epoch integer
    sec = int(ts_utc.timestamp())
    if unit == "s":
        return sec
    return sec * 1000


def _epoch_series_to_utc(series: pd.Series, unit: EpochUnit) -> pd.Series:
    # integer epoch -> datetime64[ns, UTC]
    return pd.to_datetime(series.astype("int64"), unit=unit, utc=True)


def _query_distinct_provider_ids(
    engine: Engine,
    symbols: List[str],
    timeframe: str,
    ts_col: str,
    start_utc: pd.Timestamp,
    end_utc: pd.Timestamp,
) -> Tuple[bool, List[str]]:
    """Read-only. Returns (provider_metadata_available, provider_ids_observed).
    provider_metadata_available=False means the provider_id column itself is
    absent (e.g. a schema older than migration 0042) -- a genuinely
    different fact from provider_metadata_available=True with an EMPTY
    provider_ids_observed list (column exists, this exact query window
    matched zero rows). Never conflates unavailable, empty, and present."""
    colset = set(_load_md_bars_columns(engine))
    if "provider_id" not in colset:
        return False, []

    ts_type = _column_db_type(engine, ts_col)
    is_integer_ts = ts_type in {"bigint", "integer", "smallint"}
    if is_integer_ts:
        epoch_unit = infer_epoch_unit_strict(engine, ts_col)
        start_bound = _to_epoch_bound(start_utc, epoch_unit)
        end_bound = _to_epoch_bound(end_utc, epoch_unit)
        q = text(
            f"""
            select distinct provider_id
            from md_bars
            where symbol = any(:symbols)
              and timeframe = :timeframe
              and {ts_col} >= :start_bound
              and {ts_col} < :end_bound
            order by provider_id asc
            """
        )
        params: Dict[str, Any] = {
            "symbols": symbols,
            "timeframe": timeframe,
            "start_bound": start_bound,
            "end_bound": end_bound,
        }
    else:
        q = text(
            f"""
            select distinct provider_id
            from md_bars
            where symbol = any(:symbols)
              and timeframe = :timeframe
              and {ts_col} >= :start_utc
              and {ts_col} < :end_utc
            order by provider_id asc
            """
        )
        params = {
            "symbols": symbols,
            "timeframe": timeframe,
            "start_utc": start_utc.to_pydatetime(),
            "end_utc": end_utc.to_pydatetime(),
        }

    with engine.connect() as cxn:
        rows = cxn.execute(q, params).fetchall()
    return True, sorted({str(r[0]) for r in rows if r[0] is not None})


def _build_price_provenance(
    *, close_col: str, provider_metadata_available: bool, provider_ids_observed: List[str]
) -> Dict[str, Any]:
    convention = classify_price_convention(
        close_col=close_col, provider_ids_observed=frozenset(provider_ids_observed)
    )
    return {
        "close_column": close_col,
        "provider_metadata_available": provider_metadata_available,
        "provider_ids_observed": provider_ids_observed,
        "price_adjustment_convention": convention,
        "convention_basis": _CONVENTION_BASIS_NOTE,
    }


def resolve_price_provenance(
    engine: Engine,
    symbols: List[str],
    timeframe: str,
    start_utc: pd.Timestamp,
    end_utc: pd.Timestamp,
) -> Dict[str, Any]:
    """Standalone, read-only entry point: resolve the price-adjustment
    provenance for a given (symbols, timeframe, window) bars query without
    fetching the bars themselves. `history()` computes the equivalent
    result inline (reusing schema/bounds it already resolved) and attaches
    it to the returned DataFrame's `.attrs["price_provenance"]` -- both
    paths share classify_price_convention and _query_distinct_provider_ids
    so they can never disagree."""
    _require_tz(start_utc, "start_utc")
    _require_tz(end_utc, "end_utc")
    start_utc = start_utc.tz_convert("UTC")
    end_utc = end_utc.tz_convert("UTC")
    symbols_norm = sorted({s.strip().upper() for s in symbols if s.strip()})
    if not symbols_norm:
        raise ValueError("symbols must contain at least one non-empty symbol")

    ts_col, _open_col, _high_col, _low_col, close_col, _volume_col, _has_is_complete = _detect_md_bars_schema(engine)
    provider_metadata_available, provider_ids_observed = _query_distinct_provider_ids(
        engine, symbols_norm, timeframe, ts_col, start_utc, end_utc
    )
    return _build_price_provenance(
        close_col=close_col,
        provider_metadata_available=provider_metadata_available,
        provider_ids_observed=provider_ids_observed,
    )


def _diagnose_empty(engine: Engine, symbols: List[str], timeframe: str, ts_col: str, has_is_complete: bool) -> str:
    # Deterministic diagnostics. No sampling randomness, all ORDER BY fixed.
    tf_q = text(
        """
        select timeframe, count(*) as n
        from md_bars
        where symbol = any(:symbols)
        group by timeframe
        order by timeframe asc
        """
    )
    with engine.connect() as cxn:
        tf_rows = cxn.execute(tf_q, {"symbols": symbols}).fetchall()

    tf_summary = ", ".join([f"{r[0]}={r[1]}" for r in tf_rows]) if tf_rows else "<none>"

    mm_q = text(
        f"""
        select min({ts_col}) as min_ts, max({ts_col}) as max_ts, count(*) as n
        from md_bars
        where symbol = any(:symbols)
          and timeframe = :timeframe
        """
    )
    with engine.connect() as cxn:
        mm = cxn.execute(mm_q, {"symbols": symbols, "timeframe": timeframe}).fetchone()

    mm_str = f"min={mm[0]} max={mm[1]} n={mm[2]}" if mm else "<none>"

    complete_str = "n/a"
    if has_is_complete:
        c_q = text(
            """
            select is_complete, count(*) as n
            from md_bars
            where symbol = any(:symbols)
              and timeframe = :timeframe
            group by is_complete
            order by is_complete asc
            """
        )
        with engine.connect() as cxn:
            c_rows = cxn.execute(c_q, {"symbols": symbols, "timeframe": timeframe}).fetchall()
        complete_str = ", ".join([f"{r[0]}={r[1]}" for r in c_rows]) if c_rows else "<none>"

    unit_note = ""
    try:
        ts_type = _column_db_type(engine, ts_col)
        if ts_type in {"bigint", "integer", "smallint"}:
            unit = infer_epoch_unit_strict(engine, ts_col)
            unit_note = f"  inferred_epoch_unit={unit} (threshold={EPOCH_MS_THRESHOLD})\n"
    except Exception as e:
        unit_note = f"  inferred_epoch_unit=<error: {e}>\n"

    return (
        "md_bars returned zero rows.\n"
        f"  symbols={symbols}\n"
        f"  requested_timeframe={timeframe}\n"
        f"  available_timeframes_for_symbols={tf_summary}\n"
        f"  ts_col={ts_col} (raw min/max for requested timeframe: {mm_str})\n"
        f"{unit_note}"
        f"  is_complete_counts_for_timeframe={complete_str}\n"
        "Next actions:\n"
        "  - If available_timeframes_for_symbols is empty, your symbols are not present.\n"
        "  - If requested_timeframe not present, update policy bars.timeframe to a value shown.\n"
        "  - If max_ts is far earlier than your ASOF, pick an ASOF within your data range.\n"
        "  - If is_complete true count is 0, your ingestor never marked bars complete.\n"
        "  - If inferred_epoch_unit errors or reports mixed units, fix md_bars end_ts storage first.\n"
    )


def history(engine: Engine, q: BarsQuery) -> pd.DataFrame:
    if not q.symbols:
        raise ValueError("symbols must be non-empty")

    _require_tz(q.start_utc, "start_utc")
    _require_tz(q.end_utc, "end_utc")
    start_utc = q.start_utc.tz_convert("UTC")
    end_utc = q.end_utc.tz_convert("UTC")
    if end_utc <= start_utc:
        raise ValueError("end_utc must be > start_utc")

    symbols = sorted({s.strip().upper() for s in q.symbols if s.strip()})
    if not symbols:
        raise ValueError("symbols must contain at least one non-empty symbol")

    cols = _load_md_bars_columns(engine)
    colset = set(cols)

    if "timeframe" not in colset:
        raise RuntimeError("md_bars missing 'timeframe' column; cannot enforce explicit timeframe")
    if "symbol" not in colset:
        raise RuntimeError("md_bars missing 'symbol' column")

    ts_col, open_col, high_col, low_col, close_col, volume_col, has_is_complete = _detect_md_bars_schema(engine)

    # Optional quality gate: require complete bars if that flag exists.
    complete_clause = "and is_complete = true" if has_is_complete else ""

    # Decide whether ts_col is epoch integer or timestamptz-like.
    ts_type = _column_db_type(engine, ts_col)
    is_integer_ts = ts_type in {"bigint", "integer", "smallint"}

    if is_integer_ts:
        epoch_unit = infer_epoch_unit_strict(engine, ts_col)
        start_bound = _to_epoch_bound(start_utc, epoch_unit)
        end_bound = _to_epoch_bound(end_utc, epoch_unit)

        sql = f"""
        select
          symbol,
          {ts_col} as ts_raw,
          {open_col} as open_raw,
          {high_col} as high_raw,
          {low_col} as low_raw,
          {close_col} as close_raw,
          {volume_col} as volume
        from md_bars
        where symbol = any(:symbols)
          and {ts_col} >= :start_bound
          and {ts_col} < :end_bound
          and timeframe = :timeframe
          {complete_clause}
        order by symbol asc, {ts_col} asc
        """

        params = {
            "symbols": symbols,
            "start_bound": start_bound,
            "end_bound": end_bound,
            "timeframe": q.timeframe,
        }

        with engine.connect() as cxn:
            df = pd.read_sql(text(sql), cxn, params=params)

        if df.empty:
            raise RuntimeError(_diagnose_empty(engine, symbols, q.timeframe, ts_col, has_is_complete))

        df["symbol"] = df["symbol"].astype(str).str.upper()
        df["ts_utc"] = _epoch_series_to_utc(df["ts_raw"], epoch_unit)
        df = df.drop(columns=["ts_raw"])

    else:
        # Timestamptz-ish path
        sql = f"""
        select
          symbol,
          {ts_col} as ts_utc,
          {open_col} as open_raw,
          {high_col} as high_raw,
          {low_col} as low_raw,
          {close_col} as close_raw,
          {volume_col} as volume
        from md_bars
        where symbol = any(:symbols)
          and {ts_col} >= :start_utc
          and {ts_col} < :end_utc
          and timeframe = :timeframe
          {complete_clause}
        order by symbol asc, {ts_col} asc
        """

        params = {
            "symbols": symbols,
            "start_utc": start_utc.to_pydatetime(),
            "end_utc": end_utc.to_pydatetime(),
            "timeframe": q.timeframe,
        }

        with engine.connect() as cxn:
            df = pd.read_sql(text(sql), cxn, params=params)

        if df.empty:
            raise RuntimeError(_diagnose_empty(engine, symbols, q.timeframe, ts_col, has_is_complete))

        df["symbol"] = df["symbol"].astype(str).str.upper()
        df["ts_utc"] = pd.to_datetime(df["ts_utc"], utc=True)

    # Convert OHLCV deterministically
    df["open"] = _to_price_float(df["open_raw"], open_col)
    df["high"] = _to_price_float(df["high_raw"], high_col)
    df["low"] = _to_price_float(df["low_raw"], low_col)
    df["close"] = _to_price_float(df["close_raw"], close_col)
    df["volume"] = df["volume"].astype(float)
    df = df.drop(columns=["open_raw", "high_raw", "low_raw", "close_raw"])

    # Deterministic ordering
    df = df.sort_values(["symbol", "ts_utc"], kind="mergesort").reset_index(drop=True)

    present = set(df["symbol"].unique().tolist())
    missing_syms = [s for s in symbols if s not in present]
    if missing_syms:
        raise RuntimeError(f"Missing data for symbols in window: {missing_syms}")

    # BKT-DATA-PROVENANCE-POINT-IN-TIME-01: attach an explicit, auditable
    # price-provenance record rather than leaving the close_col resolution
    # (A17) silent. .attrs is metadata on the DataFrame object itself, not a
    # column -- callers that don't know about it see no behavior change.
    provider_metadata_available, provider_ids_observed = _query_distinct_provider_ids(
        engine, symbols, q.timeframe, ts_col, start_utc, end_utc
    )
    df.attrs["price_provenance"] = _build_price_provenance(
        close_col=close_col,
        provider_metadata_available=provider_metadata_available,
        provider_ids_observed=provider_ids_observed,
    )

    return df