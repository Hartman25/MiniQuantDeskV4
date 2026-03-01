# bootstrap_research_py.ps1
# Run from MiniQuantDeskV4 repo root.
# Creates research-py/ and writes all Phase 1 files.

Set-StrictMode -Version Latest
$ErrorActionPreference = "Stop"

function Ensure-Dir([string]$p) {
  New-Item -ItemType Directory -Force -Path $p | Out-Null
}

function Write-TextFile([string]$path, [string]$content) {
  $dir = Split-Path -Parent $path
  if ($dir -and -not (Test-Path $dir)) { Ensure-Dir $dir }
  # Force LF line endings for determinism
  $contentLf = $content -replace "`r`n", "`n"
  [System.IO.File]::WriteAllText($path, $contentLf, (New-Object System.Text.UTF8Encoding($false)))
}

$root = Join-Path (Get-Location) "research-py"

# --- dirs ---
$dirs = @(
  "$root\runs",
  "$root\src\mqk_research",
  "$root\src\mqk_research\io",
  "$root\src\mqk_research\data\adapters",
  "$root\src\mqk_research\instruments",
  "$root\src\mqk_research\features",
  "$root\src\mqk_research\policies",
  "$root\src\mqk_research\universe",
  "$root\src\mqk_research\portfolio"
)
$dirs | ForEach-Object { Ensure-Dir $_ }

# --- files ---
Write-TextFile "$root\pyproject.toml" @'
[project]
name = "mqk-research"
version = "0.1.0"
description = "MiniQuantDesk V4 deterministic research layer (Part A / Phase 1)"
requires-python = ">=3.10"
dependencies = [
  "pandas>=2.0",
  "numpy>=1.24",
  "PyYAML>=6.0",
  "SQLAlchemy>=2.0",
  "psycopg[binary]>=3.1",
]

[project.scripts]
mqk-research = "mqk_research.cli:main"

[tool.setuptools]
package-dir = {"" = "src"}

[tool.setuptools.packages.find]
where = ["src"]
'@

Write-TextFile "$root\src\mqk_research\__init__.py" @'
__all__ = ["__version__"]
__version__ = "0.1.0"
'@

Write-TextFile "$root\src\mqk_research\io\pg.py" @'
from __future__ import annotations

from dataclasses import dataclass

from sqlalchemy import create_engine, text
from sqlalchemy.engine import Engine


@dataclass(frozen=True)
class PgConfig:
    # Explicit, no hidden config. Provide a URL.
    url: str


def make_engine(cfg: PgConfig) -> Engine:
    # SQLAlchemy chosen for:
    # - deterministic synchronous execution
    # - clear explicit SQL + explicit ordering
    # - simple DSN handling via psycopg driver
    return create_engine(cfg.url, future=True, pool_pre_ping=True)


def table_exists(engine: Engine, table_name: str, schema: str = "public") -> bool:
    q = text(
        """
        select 1
        from information_schema.tables
        where table_schema = :schema and table_name = :table
        limit 1
        """
    )
    with engine.connect() as cxn:
        row = cxn.execute(q, {"schema": schema, "table": table_name}).fetchone()
        return row is not None
'@

Write-TextFile "$root\src\mqk_research\io\hashing.py" @'
from __future__ import annotations

import hashlib
from pathlib import Path
from typing import Union

BytesLike = Union[bytes, bytearray, memoryview]


def sha256_bytes(data: BytesLike) -> str:
    h = hashlib.sha256()
    h.update(data)
    return h.hexdigest()


def sha256_file(path: Path) -> str:
    h = hashlib.sha256()
    with path.open("rb") as f:
        for chunk in iter(lambda: f.read(1024 * 1024), b""):
            h.update(chunk)
    return h.hexdigest()
'@

Write-TextFile "$root\src\mqk_research\io\manifest.py" @'
from __future__ import annotations

import json
from dataclasses import asdict, dataclass, field
from pathlib import Path
from typing import Any, Dict, List

from .hashing import sha256_bytes, sha256_file


@dataclass(frozen=True)
class Manifest:
    schema_version: str
    run_id: str
    asof_utc: str
    policy_name: str
    policy_path: str
    policy_sha256: str
    params: Dict[str, Any]
    inputs: Dict[str, Any]
    outputs: Dict[str, Any]
    notes: List[str] = field(default_factory=list)

    def to_json_bytes(self) -> bytes:
        obj = asdict(self)
        return json.dumps(obj, sort_keys=True, separators=(",", ":"), ensure_ascii=False).encode("utf-8")

    def write(self, path: Path) -> None:
        path.parent.mkdir(parents=True, exist_ok=True)
        path.write_bytes(self.to_json_bytes())

    def sha256(self) -> str:
        return sha256_bytes(self.to_json_bytes())


def stable_run_id(policy_name: str, asof_utc: str, params: Dict[str, Any]) -> str:
    blob = json.dumps(
        {"policy_name": policy_name, "asof_utc": asof_utc, "params": params},
        sort_keys=True,
        separators=(",", ":"),
        ensure_ascii=False,
    ).encode("utf-8")
    return sha256_bytes(blob)[:20]


def file_record(path: Path) -> Dict[str, Any]:
    return {
        "path": str(path),
        "sha256": sha256_file(path),
        "bytes": path.stat().st_size,
    }
'@

Write-TextFile "$root\src\mqk_research\data\adapters\bars_postgres.py" @'
from __future__ import annotations

from dataclasses import dataclass
from typing import List

import pandas as pd
from sqlalchemy import text
from sqlalchemy.engine import Engine


@dataclass(frozen=True)
class BarsQuery:
    symbols: List[str]
    start_utc: pd.Timestamp
    end_utc: pd.Timestamp
    timeframe: str  # e.g. "1D"


def _require_utc(ts: pd.Timestamp, name: str) -> None:
    if ts.tz is None or str(ts.tz) != "UTC":
        raise ValueError(f"{name} must be timezone-aware UTC pandas Timestamp")


def history(engine: Engine, q: BarsQuery) -> pd.DataFrame:
    """Deterministic history over md_bars.

    Requirements:
    - explicit symbols list (deduped, sorted)
    - explicit start/end (UTC tz-aware), end > start
    - explicit timeframe enforcement (md_bars.timeframe required)
    - explicit ORDER BY for stable row ordering
    - strict failure if any requested symbol has zero rows

    Assumed md_bars columns (minimum):
      symbol, timeframe, ts_utc (or ts), open, high, low, close, volume
    """
    if not q.symbols:
        raise ValueError("symbols must be non-empty")
    _require_utc(q.start_utc, "start_utc")
    _require_utc(q.end_utc, "end_utc")
    if q.end_utc <= q.start_utc:
        raise ValueError("end_utc must be > start_utc")

    symbols = sorted({s.strip().upper() for s in q.symbols if s.strip()})
    if not symbols:
        raise ValueError("symbols must contain at least one non-empty symbol")

    # Detect timestamp column.
    with engine.connect() as cxn:
        cols = cxn.execute(
            text(
                """
                select column_name
                from information_schema.columns
                where table_schema='public' and table_name='md_bars'
                """
            )
        ).fetchall()
    colset = {c[0] for c in cols}
    if "ts_utc" in colset:
        ts_col = "ts_utc"
    elif "ts" in colset:
        ts_col = "ts"
    elif "bar_ts_utc" in colset:
        ts_col = "bar_ts_utc"
    else:
        raise RuntimeError("md_bars missing a recognizable UTC timestamp column (expected ts_utc or ts)")

    if "timeframe" not in colset:
        raise RuntimeError("md_bars missing 'timeframe' column; cannot enforce explicit timeframe")

    sql = f"""
    select
      symbol,
      {ts_col} as ts_utc,
      open, high, low, close, volume
    from md_bars
    where symbol = any(:symbols)
      and {ts_col} >= :start_utc
      and {ts_col} < :end_utc
      and timeframe = :timeframe
    order by symbol asc, ts_utc asc
    """

    params = {
        "symbols": symbols,
        "start_utc": q.start_utc.to_pydatetime(),
        "end_utc": q.end_utc.to_pydatetime(),
        "timeframe": q.timeframe,
    }

    with engine.connect() as cxn:
        df = pd.read_sql(text(sql), cxn, params=params)

    if df.empty:
        raise RuntimeError("md_bars returned zero rows for requested symbols/time window")

    df["symbol"] = df["symbol"].astype(str).str.upper()
    df["ts_utc"] = pd.to_datetime(df["ts_utc"], utc=True)
    df = df.sort_values(["symbol", "ts_utc"], kind="mergesort").reset_index(drop=True)

    present = set(df["symbol"].unique().tolist())
    missing = [s for s in symbols if s not in present]
    if missing:
        raise RuntimeError(f"Missing data for symbols in window: {missing}")

    return df
'@

Write-TextFile "$root\src\mqk_research\instruments\schema.py" @'
from __future__ import annotations

from dataclasses import dataclass
from enum import Enum
from typing import Optional


class AssetClass(str, Enum):
    EQUITY = "EQUITY"
    OPTION = "OPTION"
    FUTURE = "FUTURE"


@dataclass(frozen=True)
class Instrument:
    instrument_id: str
    symbol: str
    asset_class: AssetClass = AssetClass.EQUITY
    exchange: Optional[str] = None
    currency: str = "USD"
'@

Write-TextFile "$root\src\mqk_research\features\compute.py" @'
from __future__ import annotations

from dataclasses import dataclass
from typing import Tuple

import pandas as pd


@dataclass(frozen=True)
class FeatureConfig:
    atr_window: int = 20
    adv_window: int = 20
    ret_windows: Tuple[int, ...] = (1, 5, 20, 60)
    ma_fast: int = 20
    ma_slow: int = 50


def _true_range(high: pd.Series, low: pd.Series, prev_close: pd.Series) -> pd.Series:
    a = (high - low).abs()
    b = (high - prev_close).abs()
    c = (low - prev_close).abs()
    return pd.concat([a, b, c], axis=1).max(axis=1)


def compute_daily_features(bars: pd.DataFrame, cfg: FeatureConfig) -> pd.DataFrame:
    """Compute reusable daily features (equities, 1D bars).

    Input bars columns:
      symbol, ts_utc, open, high, low, close, volume
    Output rows remain at bar granularity but include feature columns.
    Determinism:
      - explicit sorts
      - fixed rolling windows
      - no randomness
      - no implicit time
    """
    required = {"symbol", "ts_utc", "open", "high", "low", "close", "volume"}
    missing = required - set(bars.columns)
    if missing:
        raise ValueError(f"bars missing required columns: {sorted(missing)}")

    df = bars.copy()
    df["symbol"] = df["symbol"].astype(str).str.upper()
    df["ts_utc"] = pd.to_datetime(df["ts_utc"], utc=True)
    df = df.sort_values(["symbol", "ts_utc"], kind="mergesort").reset_index(drop=True)

    out_parts = []
    for sym, g in df.groupby("symbol", sort=True):
        g = g.sort_values("ts_utc", kind="mergesort").reset_index(drop=True)

        close = g["close"].astype(float)
        high = g["high"].astype(float)
        low = g["low"].astype(float)
        volume = g["volume"].astype(float)

        for w in cfg.ret_windows:
            g[f"ret_{w}d"] = close.pct_change(w)

        prev_close = close.shift(1)
        tr = _true_range(high, low, prev_close)
        atr = tr.rolling(cfg.atr_window, min_periods=cfg.atr_window).mean()
        g[f"atr_pct_{cfg.atr_window}"] = atr / close

        dollar_vol = close * volume
        g[f"adv_usd_{cfg.adv_window}"] = dollar_vol.rolling(cfg.adv_window, min_periods=cfg.adv_window).mean()

        ma_fast = close.rolling(cfg.ma_fast, min_periods=cfg.ma_fast).mean()
        ma_slow = close.rolling(cfg.ma_slow, min_periods=cfg.ma_slow).mean()
        g[f"ma_{cfg.ma_fast}"] = ma_fast
        g[f"ma_{cfg.ma_slow}"] = ma_slow
        g["trend_proxy"] = (ma_fast / ma_slow) - 1.0

        out_parts.append(g)

    out = pd.concat(out_parts, axis=0, ignore_index=True)

    # Keep only rows where core windows exist (prevents silent NaNs).
    core_cols = [
        "ret_1d",
        "ret_5d",
        "ret_20d",
        "ret_60d",
        f"atr_pct_{cfg.atr_window}",
        f"adv_usd_{cfg.adv_window}",
        "trend_proxy",
    ]
    out = out.dropna(subset=core_cols).reset_index(drop=True)

    # Normalize canonical names required downstream in Phase 1.
    out = out.rename(
        columns={
            f"atr_pct_{cfg.atr_window}": "atr_pct_20",
            f"adv_usd_{cfg.adv_window}": "adv_usd_20",
        }
    )

    return out
'@

Write-TextFile "$root\src\mqk_research\policies\swing_v1.yaml" @'
name: swing_v1
asset_class: EQUITY

requires:
  bars: true
  corporate_events: optional  # earnings exclusion stub allowed in Phase 1

bars:
  timeframe: "1D"
  lookback_days: 140

filters:
  min_price: 5.0
  min_adv_usd_20: 2000000.0

rank:
  top_k: 200
  score: "ret_60d + trend_proxy"

portfolio:
  max_positions: 20
  top_n: 10
  long_only: true
'@

Write-TextFile "$root\src\mqk_research\universe\build.py" @'
from __future__ import annotations

from dataclasses import dataclass
from typing import Dict, Optional

import pandas as pd


@dataclass(frozen=True)
class UniverseResult:
    df: pd.DataFrame
    stubbed_earnings: bool


def build_universe_swing_v1(
    features: pd.DataFrame,
    policy: Dict,
    earnings_flags: Optional[pd.DataFrame],
) -> UniverseResult:
    """Build swing_v1 universe (ranked + filtered).

    features must include:
      symbol, ts_utc, close, adv_usd_20, atr_pct_20, ret_60d, trend_proxy

    earnings_flags (optional) schema:
      symbol, earnings_within_14d (bool)
    """
    p_filters = policy["filters"]
    rank_cfg = policy["rank"]

    req = {"symbol", "ts_utc", "close", "adv_usd_20", "atr_pct_20", "ret_60d", "trend_proxy"}
    missing = req - set(features.columns)
    if missing:
        raise ValueError(f"features missing required columns for universe: {sorted(missing)}")

    df = features.copy()
    df["symbol"] = df["symbol"].astype(str).str.upper()
    df["ts_utc"] = pd.to_datetime(df["ts_utc"], utc=True)
    df = df.sort_values(["symbol", "ts_utc"], kind="mergesort")

    # ASOF per symbol = last row in window.
    asof = df.groupby("symbol", sort=True).tail(1).reset_index(drop=True)

    # Earnings exclusion (stub allowed if missing).
    stubbed = False
    if earnings_flags is None:
        asof["earnings_within_14d"] = False
        stubbed = True
    else:
        ef = earnings_flags.copy()
        ef["symbol"] = ef["symbol"].astype(str).str.upper()
        ef = ef[["symbol", "earnings_within_14d"]].drop_duplicates(subset=["symbol"]).reset_index(drop=True)
        asof = asof.merge(ef, on="symbol", how="left")
        asof["earnings_within_14d"] = asof["earnings_within_14d"].fillna(False).astype(bool)

    # Deterministic filters.
    min_price = float(p_filters["min_price"])
    min_adv = float(p_filters["min_adv_usd_20"])

    included = pd.Series(True, index=asof.index)
    included &= asof["close"].astype(float) > min_price
    included &= asof["adv_usd_20"].astype(float) > min_adv
    included &= ~asof["earnings_within_14d"].astype(bool)
    asof["included"] = included

    # Score (Phase 1 fixed formula; policy carries string for audit only).
    asof["score"] = asof["ret_60d"].astype(float) + asof["trend_proxy"].astype(float)

    ranked = asof[asof["included"]].copy()
    ranked = ranked.sort_values(["score", "symbol"], ascending=[False, True], kind="mergesort").reset_index(drop=True)

    top_k = int(rank_cfg["top_k"])
    ranked = ranked.head(top_k).copy()
    ranked["rank"] = range(1, len(ranked) + 1)

    out = ranked[
        [
            "symbol",
            "rank",
            "included",
            "adv_usd_20",
            "atr_pct_20",
            "ret_60d",
            "trend_proxy",
            "earnings_within_14d",
            "score",
        ]
    ].copy()

    # Phase 1: synthetic instrument_id for equities.
    out["instrument_id"] = out["symbol"].map(lambda s: f"EQUITY::{s}")
    out["asset_class"] = policy.get("asset_class", "EQUITY")

    out = out[
        [
            "instrument_id",
            "symbol",
            "asset_class",
            "rank",
            "included",
            "adv_usd_20",
            "atr_pct_20",
            "ret_60d",
            "trend_proxy",
            "earnings_within_14d",
            "score",
        ]
    ].reset_index(drop=True)

    return UniverseResult(df=out, stubbed_earnings=stubbed)
'@

Write-TextFile "$root\src\mqk_research\portfolio\build.py" @'
from __future__ import annotations

from typing import Dict

import pandas as pd


def build_targets_long_only_equal_weight(universe: pd.DataFrame, policy: Dict) -> pd.DataFrame:
    """Portfolio builder: long-only, equal weight top N.

    Input universe expects at least:
      included, rank, instrument_id, symbol, asset_class

    Output targets schema:
      instrument_id, symbol, asset_class, side, weight
    """
    port = policy["portfolio"]
    top_n = int(port["top_n"])
    max_pos = int(port["max_positions"])

    if top_n <= 0:
        raise ValueError("portfolio.top_n must be > 0")
    if max_pos <= 0:
        raise ValueError("portfolio.max_positions must be > 0")
    if top_n > max_pos:
        top_n = max_pos

    df = universe.copy()
    df = df[df["included"] == True].copy()
    df = df.sort_values(["rank", "symbol"], kind="mergesort").head(top_n).reset_index(drop=True)

    if df.empty:
        raise RuntimeError("Universe produced zero included instruments; cannot build targets")

    w = 1.0 / float(len(df))
    out = pd.DataFrame(
        {
            "instrument_id": df["instrument_id"].astype(str),
            "symbol": df["symbol"].astype(str),
            "asset_class": df["asset_class"].astype(str),
            "side": "LONG",
            "weight": w,
        }
    )

    out = out.sort_values(["symbol"], kind="mergesort").reset_index(drop=True)
    return out
'@

Write-TextFile "$root\src\mqk_research\cli.py" @'
from __future__ import annotations

import argparse
from pathlib import Path
from typing import Any, Dict, Optional, Tuple

import pandas as pd
import yaml
from sqlalchemy import text

from mqk_research.data.adapters.bars_postgres import BarsQuery, history
from mqk_research.features.compute import FeatureConfig, compute_daily_features
from mqk_research.io.hashing import sha256_bytes, sha256_file
from mqk_research.io.manifest import Manifest, file_record, stable_run_id
from mqk_research.io.pg import PgConfig, make_engine, table_exists
from mqk_research.portfolio.build import build_targets_long_only_equal_weight
from mqk_research.universe.build import build_universe_swing_v1


def _parse_utc_ts(s: str, name: str) -> pd.Timestamp:
    ts = pd.Timestamp(s)
    if ts.tz is None:
        raise ValueError(f"{name} must include timezone offset; use UTC like '2026-02-24T00:00:00Z'")
    return ts.tz_convert("UTC")


def _asof_day_bounds(asof_utc: pd.Timestamp) -> Tuple[pd.Timestamp, pd.Timestamp]:
    day = asof_utc.floor("D")
    return day, day + pd.Timedelta(days=1)


def _load_policy(path: Path) -> Dict[str, Any]:
    if not path.exists():
        raise FileNotFoundError(f"Policy not found: {path}")
    obj = yaml.safe_load(path.read_text(encoding="utf-8"))
    if not isinstance(obj, dict) or "name" not in obj:
        raise ValueError(f"Invalid policy YAML: {path}")
    return obj


def _write_csv_deterministic(df: pd.DataFrame, path: Path) -> None:
    path.parent.mkdir(parents=True, exist_ok=True)
    df.to_csv(path, index=False, lineterminator="\n")


def _hash_df_csv_bytes(df: pd.DataFrame) -> str:
    b = df.to_csv(index=False, lineterminator="\n").encode("utf-8")
    return sha256_bytes(b)


def _earnings_flags_optional(engine, symbols, asof_day_start, asof_day_end_plus_14d) -> Optional[pd.DataFrame]:
    # Phase 1: optional stub. If corporate_events doesn't exist -> None.
    if not table_exists(engine, "corporate_events"):
        return None

    # Expected minimal schema (from Part A): symbol + some event timestamp + event_type.
    # If table exists but schema doesn't match, we fail closed (no silent behavior).
    q = text(
        """
        select symbol, event_ts_utc as ts_utc, event_type
        from corporate_events
        where symbol = any(:symbols)
          and event_ts_utc >= :start_utc
          and event_ts_utc < :end_utc
          and event_type = 'EARNINGS'
        order by symbol asc, ts_utc asc
        """
    )

    try:
        with engine.connect() as cxn:
            df = pd.read_sql(
                q,
                cxn,
                params={
                    "symbols": symbols,
                    "start_utc": asof_day_start.to_pydatetime(),
                    "end_utc": asof_day_end_plus_14d.to_pydatetime(),
                },
            )
    except Exception as e:
        raise RuntimeError(
            "corporate_events table exists but is not queryable with expected columns "
            "(symbol, event_ts_utc, event_type). Refusing to stub silently."
        ) from e

    df["symbol"] = df["symbol"].astype(str).str.upper()
    df["ts_utc"] = pd.to_datetime(df["ts_utc"], utc=True)

    flags = []
    for sym in symbols:
        has = (df["symbol"] == sym).any()
        flags.append({"symbol": sym, "earnings_within_14d": bool(has)})

    return pd.DataFrame(flags)


def run_phase1(policy_path: Path, asof_utc: pd.Timestamp, pg_url: str, out_root: Path, symbols_csv: str) -> Path:
    policy = _load_policy(policy_path)
    policy_name = str(policy["name"])
    policy_sha = sha256_file(policy_path)

    engine = make_engine(PgConfig(url=pg_url))

    timeframe = str(policy["bars"]["timeframe"])
    lookback_days = int(policy["bars"]["lookback_days"])

    asof_day_start, asof_day_end = _asof_day_bounds(asof_utc)
    end_utc = asof_day_end
    start_utc = (end_utc - pd.Timedelta(days=lookback_days)).tz_convert("UTC")

    symbols = [s.strip().upper() for s in symbols_csv.split(",") if s.strip()]
    symbols = sorted(set(symbols))
    if not symbols:
        raise ValueError("--symbols must be non-empty (comma-separated)")

    # 1) History (strict).
    bars_df = history(engine, BarsQuery(symbols=symbols, start_utc=start_utc, end_utc=end_utc, timeframe=timeframe))

    # 2) Features (strict).
    feat_cfg = FeatureConfig(atr_window=20, adv_window=20, ret_windows=(1, 5, 20, 60), ma_fast=20, ma_slow=50)
    feats_df = compute_daily_features(bars_df, feat_cfg)

    # 3) Optional earnings flags (stub allowed only when table missing).
    earnings_df = _earnings_flags_optional(
        engine,
        symbols,
        asof_day_start,
        asof_day_end + pd.Timedelta(days=14),
    )
    stubbed_earnings = earnings_df is None

    # 4) Universe.
    uni_res = build_universe_swing_v1(features=feats_df, policy=policy, earnings_flags=earnings_df)
    universe_df = uni_res.df
    stubbed_earnings = stubbed_earnings or uni_res.stubbed_earnings

    # 5) Targets.
    targets_df = build_targets_long_only_equal_weight(universe_df, policy)

    # Deterministic run_id and paths.
    params = {"symbols": symbols, "timeframe": timeframe, "lookback_days": lookback_days}
    run_id = stable_run_id(policy_name, asof_utc.isoformat(), params)
    run_dir = out_root / run_id
    run_dir.mkdir(parents=True, exist_ok=True)

    features_path = run_dir / "features.csv"
    universe_path = run_dir / "universe.csv"
    targets_path = run_dir / "targets.csv"
    manifest_path = run_dir / "manifest.json"

    _write_csv_deterministic(feats_df, features_path)
    _write_csv_deterministic(universe_df, universe_path)
    _write_csv_deterministic(targets_df, targets_path)

    outputs = {
        "features_csv": file_record(features_path),
        "universe_csv": file_record(universe_path),
        "targets_csv": file_record(targets_path),
    }

    inputs = {
        "pg": {"url_redacted": "<provided via --pg-url>"},
        "md_bars": {
            "symbols": symbols,
            "start_utc": start_utc.isoformat(),
            "end_utc": end_utc.isoformat(),
            "timeframe": timeframe,
            "bars_rows": int(len(bars_df)),
            "bars_sha256_csv": _hash_df_csv_bytes(bars_df),
        },
        "optional": {
            "corporate_events_present": table_exists(engine, "corporate_events"),
            "stubbed_earnings": stubbed_earnings,
        },
    }

    notes = []
    if stubbed_earnings:
        notes.append("STUBBED: earnings exclusion used stub flags (corporate_events missing)")

    man = Manifest(
        schema_version="1",
        run_id=run_id,
        asof_utc=asof_utc.isoformat(),
        policy_name=policy_name,
        policy_path=str(policy_path),
        policy_sha256=policy_sha,
        params=params,
        inputs=inputs,
        outputs=outputs,
        notes=notes,
    )
    man.write(manifest_path)

    return run_dir


def main(argv: Optional[list[str]] = None) -> None:
    p = argparse.ArgumentParser(prog="mqk-research", description="Deterministic research CLI for MiniQuantDesk V4 (Phase 1)")
    sub = p.add_subparsers(dest="cmd", required=True)

    common = argparse.ArgumentParser(add_help=False)
    common.add_argument("--policy", required=True, help="Policy YAML path (e.g. src/mqk_research/policies/swing_v1.yaml)")
    common.add_argument("--asof-utc", required=True, help="ASOF timestamp with timezone (e.g. 2026-02-24T00:00:00Z)")
    common.add_argument("--pg-url", required=True, help="Postgres URL (e.g. postgresql+psycopg://user:pass@host:5432/db)")
    common.add_argument("--out", default="runs", help="Output root directory (default: runs/)")
    common.add_argument("--symbols", required=True, help="Comma-separated symbols (Phase 1)")

    sub.add_parser("features", parents=[common], help="Phase 1: compute features (also emits universe/targets for determinism)")
    sub.add_parser("universe", parents=[common], help="Phase 1: build universe (also computes features/targets)")
    sub.add_parser("targets", parents=[common], help="Phase 1: build targets (also computes features/universe)")
    sub.add_parser("run", parents=[common], help="Phase 1: all-in-one (features + universe + targets)")

    args = p.parse_args(argv)

    policy_path = Path(args.policy)
    asof_utc = _parse_utc_ts(args.asof_utc, "asof_utc")
    out_root = Path(args.out)

    run_dir = run_phase1(
        policy_path=policy_path,
        asof_utc=asof_utc,
        pg_url=args.pg_url,
        out_root=out_root,
        symbols_csv=args.symbols,
    )

    print(str(run_dir))


if __name__ == "__main__":
    main()
'@

Write-Host "Created research program at: $root"
Write-Host "Next:"
Write-Host "  cd research-py"
Write-Host "  python -m venv .venv"
Write-Host "  .\.venv\Scripts\activate"
Write-Host "  pip install -e ."
Write-Host "  mqk-research --help"