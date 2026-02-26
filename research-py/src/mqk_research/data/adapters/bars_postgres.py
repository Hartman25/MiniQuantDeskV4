from __future__ import annotations

from dataclasses import dataclass
from typing import List, Optional, Tuple, Literal

import pandas as pd
from sqlalchemy import text
from sqlalchemy.engine import Engine


@dataclass(frozen=True)
class BarsQuery:
    symbols: List[str]
    start_utc: pd.Timestamp
    end_utc: pd.Timestamp
    timeframe: str  # e.g. "1D"


MICROS_SCALE = 1_000_000.0
EpochUnit = Literal["s", "ms"]


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


def _infer_epoch_unit(engine: Engine, ts_col: str) -> EpochUnit:
    """
    Deterministic inference: sample one non-null value and infer seconds vs ms.
    - > 1e12 => ms (since current epoch seconds ~ 1.7e9)
    """
    q = text(f"select {ts_col} as v from md_bars where {ts_col} is not null limit 1")
    with engine.connect() as cxn:
        row = cxn.execute(q).fetchone()
    if row is None or row[0] is None:
        # If there is no data, the later query will fail anyway; default to seconds.
        return "s"
    v = int(row[0])
    return "ms" if v > 1_000_000_000_000 else "s"


def _to_epoch_bound(ts_utc: pd.Timestamp, unit: EpochUnit) -> int:
    # pandas Timestamp -> epoch integer
    # .timestamp() returns float seconds; convert deterministically.
    sec = int(ts_utc.timestamp())
    if unit == "s":
        return sec
    return sec * 1000


def _epoch_series_to_utc(series: pd.Series, unit: EpochUnit) -> pd.Series:
    # integer epoch -> datetime64[ns, UTC]
    return pd.to_datetime(series.astype("int64"), unit=unit, utc=True)

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

    # min/max end_ts (raw) for the chosen ts column for these symbols and timeframe
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

    return (
        "md_bars returned zero rows.\n"
        f"  symbols={symbols}\n"
        f"  requested_timeframe={timeframe}\n"
        f"  available_timeframes_for_symbols={tf_summary}\n"
        f"  ts_col={ts_col} (raw min/max for requested timeframe: {mm_str})\n"
        f"  is_complete_counts_for_timeframe={complete_str}\n"
        "Next actions:\n"
        "  - If available_timeframes_for_symbols is empty, your symbols are not present.\n"
        "  - If requested_timeframe not present, update policy bars.timeframe to a value shown.\n"
        "  - If max_ts is far earlier than your ASOF, pick an ASOF within your data range.\n"
        "  - If is_complete true count is 0, your ingestor never marked bars complete.\n"
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
        epoch_unit = _infer_epoch_unit(engine, ts_col)
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

    return df