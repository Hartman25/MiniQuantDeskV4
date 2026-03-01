from __future__ import annotations

import argparse
import json
import os
from datetime import datetime, timezone
from pathlib import Path
from typing import Any, Dict, List, Optional, Tuple

import pandas as pd
import yaml
from sqlalchemy import text

from mqk_research import contracts
from mqk_research.data.adapters.bars_postgres import BarsQuery, history, infer_epoch_unit_strict, EPOCH_MS_THRESHOLD
from mqk_research.features.compute import FeatureConfig, compute_daily_features
from mqk_research.io.hashing import sha256_bytes, sha256_file
from mqk_research.io.manifest import file_record, stable_run_id
from mqk_research.io.pg import PgConfig, make_engine, table_exists
from mqk_research.portfolio.build import build_targets_long_only_equal_weight
from mqk_research.universe.build import build_universe_swing_v1


def _load_dotenv_if_present(dotenv_path: Optional[Path] = None) -> None:
    """
    Minimal dotenv loader:
      - Reads KEY=VALUE lines
      - Ignores comments/blank lines
      - Does NOT overwrite existing environment variables
      - No external dependency (python-dotenv)
    """
    path = dotenv_path or Path(".env")
    if not path.exists() or not path.is_file():
        return

    for raw in path.read_text(encoding="utf-8").splitlines():
        line = raw.strip()
        if not line or line.startswith("#"):
            continue
        if "=" not in line:
            continue
        k, v = line.split("=", 1)
        k = k.strip()
        v = v.strip().strip('"').strip("'")
        if not k:
            continue
        if os.environ.get(k) is None:
            os.environ[k] = v


def _try_reuse_existing_run(run_dir: Path, expected: Dict[str, Any]) -> bool:
    """Return True if an existing run directory matches the expected inputs.

    Patch 2.4: Make runs idempotent and non-destructive.
    If deterministic run_id maps to an existing directory whose manifest matches
    the requested inputs, re-use it and avoid rewriting files.
    """
    manifest_path = run_dir / "manifest.json"
    if not run_dir.exists() or not run_dir.is_dir() or not manifest_path.exists():
        return False
    try:
        raw = json.loads(manifest_path.read_text(encoding="utf-8"))
    except Exception:
        return False

    def _get(d: Dict[str, Any], path: str) -> Any:
        cur: Any = d
        for part in path.split("."):
            if not isinstance(cur, dict) or part not in cur:
                return None
            cur = cur[part]
        return cur

    checks = {
        "schema_version": _get(raw, "schema_version"),
        "policy_sha256": _get(raw, "policy_sha256"),
        "policy_name": _get(raw, "policy_name"),
        "asof_utc": _get(raw, "asof_utc"),
        "params": _get(raw, "params"),
        "md_bars.symbols": _get(raw, "inputs.md_bars.symbols"),
        "md_bars.timeframe": _get(raw, "inputs.md_bars.timeframe"),
        "md_bars.start_utc": _get(raw, "inputs.md_bars.start_utc"),
        "md_bars.end_utc": _get(raw, "inputs.md_bars.end_utc"),
        "asset_class": _get(raw, "inputs.intent.asset_class"),
        "pipeline": _get(raw, "inputs.intent.pipeline"),
        "contract_version": _get(raw, "contract_version"),
    }

    for k, v in expected.items():
        if checks.get(k) != v:
            return False
    return True


def _require_md_bars_nonempty(engine) -> None:
    q = text("select count(*) from md_bars")
    with engine.connect() as cxn:
        n = int(cxn.execute(q).scalar())
        db = cxn.execute(text("select current_database()")).scalar()
        usr = cxn.execute(text("select current_user")).scalar()
    if n <= 0:
        raise RuntimeError(
            "Preflight failed: md_bars is empty.\n"
            f"  database={db} user={usr}\n"
            "Load historical bars into md_bars (ingest pipeline) or point --pg-url to the populated database."
        )


def preflight(engine) -> dict:
    """
    Deterministic DB sanity checks.
    Patch 2.6: enforce epoch unit deterministically for integer md_bars timestamps.
    Returns a dict intended to be JSON-printed.
    """
    with engine.connect() as cxn:
        db = cxn.execute(text("select current_database()")).scalar()
        usr = cxn.execute(text("select current_user")).scalar()

        md_rows = int(cxn.execute(text("select count(*) from md_bars")).scalar())

        tf_rows = cxn.execute(
            text(
                """
                select timeframe, count(*) as n
                from md_bars
                group by timeframe
                order by timeframe asc
                """
            )
        ).fetchall()
        timeframes = [{"timeframe": r[0], "rows": int(r[1])} for r in tf_rows]

        # We assume md_bars uses integer epoch end_ts in this system.
        # If your schema changes, update this intentionally (fail closed).
        cols = cxn.execute(
            text(
                """
                select column_name, data_type
                from information_schema.columns
                where table_schema='public' and table_name='md_bars'
                order by ordinal_position
                """
            )
        ).fetchall()
        col_types = {name: dtype for (name, dtype) in cols}
        if "end_ts" not in col_types:
            raise RuntimeError(
                "Preflight unsafe: md_bars missing expected end_ts column. "
                "This system requires a deterministic bar timestamp column (end_ts)."
            )

        end_ts_type = str(col_types["end_ts"]).lower()
        is_integer_ts = end_ts_type in {"bigint", "integer", "smallint"}

        mm = cxn.execute(text("select min(end_ts), max(end_ts) from md_bars")).fetchone()
        min_end_ts, max_end_ts = mm[0], mm[1]

        unit = None
        interpreted = {"min": None, "max": None}
        counts = {"n_ms": None, "n_s": None, "n_total": None}

        if is_integer_ts:
            # Strict unit enforcement (fails closed on mixed units).
            unit = infer_epoch_unit_strict(engine, "end_ts")

            cnt = cxn.execute(
                text(
                    """
                    select
                      sum(case when end_ts >= :thresh then 1 else 0 end) as n_ms,
                      sum(case when end_ts <  :thresh then 1 else 0 end) as n_s,
                      count(*) as n_total
                    from md_bars
                    where end_ts is not null
                    """
                ),
                {"thresh": int(EPOCH_MS_THRESHOLD)},
            ).fetchone()
            counts = {"n_ms": int(cnt[0] or 0), "n_s": int(cnt[1] or 0), "n_total": int(cnt[2] or 0)}

            # Convert using the detected unit (only one interpretation).
            if min_end_ts is not None:
                min_dt = datetime.fromtimestamp(int(min_end_ts) / 1000.0, tz=timezone.utc) if unit == "ms" else datetime.fromtimestamp(int(min_end_ts), tz=timezone.utc)
                interpreted["min"] = str(min_dt)
            if max_end_ts is not None:
                max_dt = datetime.fromtimestamp(int(max_end_ts) / 1000.0, tz=timezone.utc) if unit == "ms" else datetime.fromtimestamp(int(max_end_ts), tz=timezone.utc)
                interpreted["max"] = str(max_dt)

        else:
            # If end_ts is not integer, we treat it as timestamptz-ish and report directly.
            # This code path stays deterministic.
            unit = "timestamptz"
            interpreted["min"] = None if min_end_ts is None else str(min_end_ts)
            interpreted["max"] = None if max_end_ts is None else str(max_end_ts)

        sym_count = int(cxn.execute(text("select count(distinct symbol) from md_bars")).scalar())
        top_syms = cxn.execute(
            text(
                """
                select symbol
                from (
                  select distinct symbol
                  from md_bars
                ) s
                order by symbol asc
                limit 20
                """
            )
        ).fetchall()
        top_symbols = [r[0] for r in top_syms]

        corp_exists = cxn.execute(text("select to_regclass('public.corporate_events')")).scalar() is not None

    return {
        "database": db,
        "user": usr,
        "md_bars": {
            "rows": md_rows,
            "distinct_symbols": sym_count,
            "top_symbols": top_symbols,
            "timeframes": timeframes,
            "end_ts": {
                "min_raw": min_end_ts,
                "max_raw": max_end_ts,
                "unit": unit,
                "unit_threshold": int(EPOCH_MS_THRESHOLD) if is_integer_ts else None,
                "counts": counts if is_integer_ts else None,
                "as_utc": interpreted,
            },
        },
        "corporate_events_present": corp_exists,
    }


def _parse_utc_ts(s: str, name: str) -> pd.Timestamp:
    ts = pd.Timestamp(s)
    if ts.tz is None:
        raise ValueError(f"{name} must include timezone offset; use UTC like '2026-02-24T00:00:00Z'")
    return ts.tz_convert("UTC")


def _asof_day_bounds(asof_utc: pd.Timestamp) -> Tuple[pd.Timestamp, pd.Timestamp]:
    day = asof_utc.floor("D")
    return day, day + pd.Timedelta(days=1)


def _load_policy(path: Path) -> Dict[str, Any]:
    """
    Policy loader with minimal validation.
    Accepts either:
      - policy_name (preferred)
      - name (legacy)
    """
    if not path.exists():
        raise FileNotFoundError(f"Policy not found: {path}")
    obj = yaml.safe_load(path.read_text(encoding="utf-8"))
    if not isinstance(obj, dict):
        raise ValueError(f"Invalid policy YAML (not a mapping): {path}")

    # Normalize policy_name
    if "policy_name" in obj and isinstance(obj["policy_name"], str) and obj["policy_name"].strip():
        obj.setdefault("name", obj["policy_name"])
    elif "name" in obj and isinstance(obj["name"], str) and obj["name"].strip():
        obj.setdefault("policy_name", obj["name"])
    else:
        raise ValueError(f"Invalid policy YAML (missing policy_name/name): {path}")

    # Normalize asset_class
    asset_class = obj.get("asset_class", "EQUITY")
    if not isinstance(asset_class, str) or not asset_class.strip():
        asset_class = "EQUITY"
    obj["asset_class"] = asset_class.strip().upper()

    # Schema version is informational (string)
    sv = obj.get("schema_version", "1")
    obj["schema_version"] = str(sv)

    return obj


def _write_csv_deterministic(df: pd.DataFrame, path: Path) -> None:
    path.parent.mkdir(parents=True, exist_ok=True)
    df.to_csv(path, index=False, lineterminator="\n")


def finalize_for_csv(df: pd.DataFrame, kind: str) -> pd.DataFrame:
    out = df.copy()

    preferred = {
        "features": ["instrument_id", "symbol", "asset_class", "ts_utc"],
        "universe": [
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
        ],
        "targets": ["instrument_id", "symbol", "asset_class", "side", "weight"],
        "bars": ["symbol", "ts_utc", "open", "high", "low", "close", "volume"],
    }

    pref = preferred.get(kind, [])
    cols = list(out.columns)
    ordered = [c for c in pref if c in cols] + sorted([c for c in cols if c not in pref])
    out = out.loc[:, ordered]

    sort_keys = []
    if "symbol" in out.columns:
        sort_keys.append("symbol")
    if "ts_utc" in out.columns:
        sort_keys.append("ts_utc")
    if kind == "universe" and "rank" in out.columns:
        sort_keys.append("rank")
    if "instrument_id" in out.columns:
        sort_keys.append("instrument_id")

    if sort_keys:
        out = out.sort_values(sort_keys, kind="mergesort", ignore_index=True)

    round_2 = {"adv_usd_20"}
    round_8 = {
        "weight",
        "atr_pct_20",
        "ret_1d",
        "ret_5d",
        "ret_20d",
        "ret_60d",
        "trend_proxy",
        "score",
    }

    for c in out.columns:
        if c in round_2 and pd.api.types.is_numeric_dtype(out[c]):
            out[c] = out[c].round(2)
        elif c in round_8 and pd.api.types.is_numeric_dtype(out[c]):
            out[c] = out[c].round(8)

    return out


def _hash_df_csv_bytes(df: pd.DataFrame) -> str:
    b = df.to_csv(index=False, lineterminator="\n").encode("utf-8")
    return sha256_bytes(b)


def _enforce_data_sufficiency(
    bars_df: pd.DataFrame,
    symbols: list[str],
    asof_day_end_utc: pd.Timestamp,
    lookback_days: int,
    *,
    min_bars_floor: int = 60,
    max_staleness_days: int = 7,
    holiday_buffer_days: int = 8,
) -> None:
    if bars_df is None or bars_df.empty:
        raise RuntimeError("Data gate failed: history returned zero rows.")

    req = [s.strip().upper() for s in symbols if s and s.strip()]
    req = sorted(set(req))
    if not req:
        raise RuntimeError("Data gate failed: empty symbol list.")

    if "symbol" not in bars_df.columns or "ts_utc" not in bars_df.columns:
        raise RuntimeError("Data gate failed: bars_df missing required columns (symbol, ts_utc).")

    df = bars_df.copy()
    df["symbol"] = df["symbol"].astype(str).str.upper()
    df["ts_utc"] = pd.to_datetime(df["ts_utc"], utc=True)

    present = sorted(df["symbol"].unique().tolist())
    missing = sorted([s for s in req if s not in set(present)])
    if missing:
        raise RuntimeError(f"Data gate failed: missing symbols in md_bars for requested window: {missing}")

    counts = bars_df.groupby("symbol")["ts_utc"].count().to_dict()
    min_ts = pd.to_datetime(bars_df["ts_utc"].min(), utc=True)
    max_ts = pd.to_datetime(bars_df["ts_utc"].max(), utc=True)
    expected_bdays = len(pd.bdate_range(start=min_ts.date(), end=max_ts.date()))
    dynamic_buffer = max(int(holiday_buffer_days), int(round(expected_bdays * 0.15)))
    required = max(min_bars_floor, expected_bdays - dynamic_buffer)

    too_few = sorted([s for s, n in counts.items() if int(n) < required])
    if too_few:
        details = {s: int(counts[s]) for s in too_few}
        raise RuntimeError(
            "Data gate failed: insufficient bars.\n"
            f"  required_min_bars={required} (lookback_days={lookback_days})\n"
            f"  counts={details}"
        )

    asof_end = pd.Timestamp(asof_day_end_utc).tz_convert("UTC")
    last_ts = df.groupby("symbol", sort=True)["ts_utc"].max().to_dict()
    stale = sorted(
        [
            s
            for s, ts in last_ts.items()
            if (asof_end - pd.Timestamp(ts).tz_convert("UTC")) > pd.Timedelta(days=max_staleness_days)
        ]
    )
    if stale:
        details = {s: str(pd.Timestamp(last_ts[s]).tz_convert("UTC")) for s in stale}
        raise RuntimeError(
            "Data gate failed: stale last bar vs ASOF.\n"
            f"  max_staleness_days={max_staleness_days} asof_end_utc={asof_end.isoformat()}\n"
            f"  last_bar_utc={details}"
        )


def _earnings_flags_optional(engine, symbols, asof_utc, days_ahead: int = 14) -> pd.DataFrame:
    def _stub() -> pd.DataFrame:
        syms = sorted({s.strip().upper() for s in (symbols or []) if s and s.strip()})
        return pd.DataFrame(
            {"symbol": syms, "earnings_within_14d": [False] * len(syms)},
            columns=["symbol", "earnings_within_14d"],
        )

    syms = sorted({s.strip().upper() for s in (symbols or []) if s and s.strip()})
    if not syms:
        return _stub()

    asof_ts = pd.Timestamp(asof_utc)
    if asof_ts.tz is None:
        raise ValueError("asof_utc must be timezone-aware (UTC)")
    asof_ts = asof_ts.tz_convert("UTC")
    start_ts = asof_ts
    end_ts = asof_ts + pd.Timedelta(days=int(days_ahead) + 1)

    with engine.connect() as cxn:
        reg = cxn.execute(text("select to_regclass('public.corporate_events')")).scalar()
        if reg is None:
            return _stub()

        cols = cxn.execute(
            text(
                """
                select column_name, data_type
                from information_schema.columns
                where table_schema='public' and table_name='corporate_events'
                order by ordinal_position
                """
            )
        ).fetchall()

    col_types = {name: dtype for (name, dtype) in cols}
    colnames = set(col_types.keys())

    symbol_col = "symbol" if "symbol" in colnames else None
    event_type_col = "event_type" if "event_type" in colnames else None

    ts_candidates = [
        "event_ts_utc",
        "ts_utc",
        "event_time_utc",
        "event_time",
        "event_ts",
        "ts",
        "event_date",
        "date",
    ]
    ts_col = next((c for c in ts_candidates if c in colnames), None)

    if symbol_col is None or event_type_col is None or ts_col is None:
        return _stub()

    ts_dtype = (col_types.get(ts_col) or "").lower()
    acceptable_markers = ("timestamp", "date", "time zone", "time")
    if not any(k in ts_dtype for k in acceptable_markers):
        return _stub()

    if "date" in ts_dtype and "timestamp" not in ts_dtype:
        ts_expr = f"({ts_col}::timestamp at time zone 'UTC')"
    else:
        ts_expr = ts_col

    sql = f"""
        select {symbol_col} as symbol, {ts_expr} as ts_utc, {event_type_col} as event_type
        from corporate_events
        where {symbol_col} = any(:symbols)
          and {event_type_col} = 'EARNINGS'
          and {ts_expr} >= :start_utc
          and {ts_expr} < :end_utc
        order by symbol asc, ts_utc asc
    """

    with engine.connect() as cxn:
        try:
            df = pd.read_sql(
                text(sql),
                cxn,
                params={
                    "symbols": syms,
                    "start_utc": start_ts.to_pydatetime(),
                    "end_utc": end_ts.to_pydatetime(),
                },
            )
        except Exception:
            return _stub()

    if df.empty:
        return _stub()

    df["symbol"] = df["symbol"].astype(str).str.upper()
    flagged = set(df["symbol"].unique().tolist())

    out = pd.DataFrame(
        {"symbol": syms, "earnings_within_14d": [s in flagged for s in syms]},
        columns=["symbol", "earnings_within_14d"],
    )
    return out


def _max_available_ts_for_symbols(engine, symbols: List[str], timeframe: str) -> Optional[datetime]:
    """
    Returns the maximum available bar timestamp for the requested symbols/timeframe.
    Deterministic. No system time.
    """
    sql = text(
        """
        select max(end_ts) as max_end_ts
        from md_bars
        where symbol = any(:symbols)
          and timeframe = :timeframe
          and is_complete = true
        """
    )
    with engine.connect() as cxn:
        row = cxn.execute(sql, {"symbols": symbols, "timeframe": timeframe}).fetchone()
    if row is None or row[0] is None:
        return None

    max_raw = int(row[0])
    # Strict unit detection consistent with history/preflight.
    unit = infer_epoch_unit_strict(engine, "end_ts")  # fails closed if mixed
    dt = datetime.fromtimestamp(max_raw / 1000.0, tz=timezone.utc) if unit == "ms" else datetime.fromtimestamp(max_raw, tz=timezone.utc)
    return dt


def _write_manifest_contract(manifest_path: Path, man: contracts.ResearchManifest) -> None:
    contracts.validate_contract_version(man.contract_version)
    if manifest_path.exists():
        raise RuntimeError(f"Refusing to overwrite existing manifest: {manifest_path}")
    manifest_path.write_text(man.to_json(indent=2) + "\n", encoding="utf-8")


def _write_intent_contract(intent_path: Path, intent: contracts.ResearchIntent) -> None:
    contracts.validate_contract_version(intent.contract_version)
    if intent_path.exists():
        raise RuntimeError(f"Refusing to overwrite existing intent: {intent_path}")
    intent_path.write_text(intent.to_json(indent=2) + "\n", encoding="utf-8")


def run_phase1_equity(policy_path: Path, asof_utc: pd.Timestamp, pg_url: str, out_root: Path, symbols_csv: str) -> Path:
    policy = _load_policy(policy_path)
    policy_name = str(policy["policy_name"])
    policy_sha = sha256_file(policy_path)

    engine = make_engine(PgConfig(url=pg_url))
    _require_md_bars_nonempty(engine)

    if "bars" not in policy or not isinstance(policy["bars"], dict):
        raise RuntimeError("Equity Phase 1 requires policy.bars{timeframe,lookback_days}.")

    timeframe = str(policy["bars"]["timeframe"])
    lookback_days = int(policy["bars"]["lookback_days"])

    asof_day_start, asof_day_end = _asof_day_bounds(asof_utc)
    end_utc = asof_day_end
    start_utc = (end_utc - pd.Timedelta(days=lookback_days)).tz_convert("UTC")

    symbols = [s.strip().upper() for s in symbols_csv.split(",") if s.strip()]
    symbols = sorted(set(symbols))
    if not symbols:
        raise ValueError("--symbols must be non-empty (comma-separated)")

    bars_df = history(engine, BarsQuery(symbols=symbols, start_utc=start_utc, end_utc=end_utc, timeframe=timeframe))
    _enforce_data_sufficiency(
        bars_df=bars_df,
        symbols=symbols,
        asof_day_end_utc=end_utc,
        lookback_days=lookback_days,
        min_bars_floor=60,
        max_staleness_days=7,
    )

    feat_cfg = FeatureConfig(atr_window=20, adv_window=20, ret_windows=(1, 5, 20, 60), ma_fast=20, ma_slow=50)
    feats_df = compute_daily_features(bars_df, feat_cfg)

    earnings_df = _earnings_flags_optional(engine, symbols, asof_day_start, days_ahead=14)

    uni_res = build_universe_swing_v1(features=feats_df, policy=policy, earnings_flags=earnings_df)
    universe_df = uni_res.df
    stubbed_earnings = bool(getattr(uni_res, "stubbed_earnings", False))

    targets_df = build_targets_long_only_equal_weight(universe_df, policy)

    params = {"symbols": symbols, "timeframe": timeframe, "lookback_days": lookback_days}
    run_id = stable_run_id(policy_name, asof_utc.isoformat(), params)
    run_dir = out_root / run_id

    expected_manifest = {
        "schema_version": "1",
        "contract_version": contracts.CONTRACT_VERSION,
        "policy_sha256": policy_sha,
        "policy_name": policy_name,
        "asof_utc": asof_utc.isoformat(),
        "params": params,
        "md_bars.symbols": symbols,
        "md_bars.timeframe": timeframe,
        "md_bars.start_utc": start_utc.isoformat(),
        "md_bars.end_utc": end_utc.isoformat(),
        "asset_class": "EQUITY",
        "pipeline": "PHASE1_EQUITY",
    }
    if _try_reuse_existing_run(run_dir, expected_manifest):
        return run_dir
    if (run_dir / "manifest.json").exists():
        raise RuntimeError(f"Run directory already exists but does not match requested inputs: {run_dir}")

    run_dir.mkdir(parents=True, exist_ok=True)

    features_path = run_dir / "features.csv"
    universe_path = run_dir / "universe.csv"
    targets_path = run_dir / "targets.csv"
    manifest_path = run_dir / "manifest.json"

    feats_out = finalize_for_csv(feats_df, "features")
    uni_out = finalize_for_csv(universe_df, "universe")
    tgt_out = finalize_for_csv(targets_df, "targets")

    _write_csv_deterministic(feats_out, features_path)
    _write_csv_deterministic(uni_out, universe_path)
    _write_csv_deterministic(tgt_out, targets_path)

    outputs = {
        "features_csv": file_record(features_path),
        "universe_csv": file_record(universe_path),
        "targets_csv": file_record(targets_path),
    }

    inputs = {
        "pg": {"url_redacted": "<provided via --pg-url>"},
        "intent": {"asset_class": "EQUITY", "pipeline": "PHASE1_EQUITY"},
        "md_bars": {
            "symbols": symbols,
            "start_utc": start_utc.isoformat(),
            "end_utc": end_utc.isoformat(),
            "timeframe": timeframe,
            "bars_rows": int(len(bars_df)),
            "bars_sha256_csv": _hash_df_csv_bytes(finalize_for_csv(bars_df, "bars")),
        },
        "optional": {
            "corporate_events_present": table_exists(engine, "corporate_events"),
            "stubbed_earnings": stubbed_earnings,
        },
    }

    notes = []
    if stubbed_earnings:
        notes.append("STUBBED: earnings exclusion used stub flags (corporate_events missing or unrecognized schema)")

    man = contracts.ResearchManifest(
        schema_version="1",
        contract_version=contracts.CONTRACT_VERSION,
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
    _write_manifest_contract(manifest_path, man)

    return run_dir


def run_phase2_stub(policy_path: Path, asof_utc: pd.Timestamp, out_root: Path, symbols_csv: str) -> Path:
    """
    Phase 2 stub runner:
      - writes an intent artifact + manifest
      - does NOT touch Postgres
      - does NOT compute features/universe/targets
    """
    policy = _load_policy(policy_path)
    policy_name = str(policy["policy_name"])
    policy_sha = sha256_file(policy_path)
    asset_class = str(policy.get("asset_class", "EQUITY")).upper()

    symbols = [s.strip().upper() for s in symbols_csv.split(",") if s.strip()]
    symbols = sorted(set(symbols))
    if not symbols:
        raise ValueError("--symbols must be non-empty (comma-separated)")

    params = {"symbols": symbols, "asset_class": asset_class}
    run_id = stable_run_id(policy_name, asof_utc.isoformat(), params)

    run_dir = out_root / run_id

    expected_manifest = {
        "schema_version": "1",
        "contract_version": contracts.CONTRACT_VERSION,
        "policy_sha256": policy_sha,
        "policy_name": policy_name,
        "asof_utc": asof_utc.isoformat(),
        "params": params,
        "md_bars.symbols": None,
        "md_bars.timeframe": None,
        "md_bars.start_utc": None,
        "md_bars.end_utc": None,
        "asset_class": asset_class,
        "pipeline": "PHASE2_STUB",
    }
    if _try_reuse_existing_run(run_dir, expected_manifest):
        return run_dir
    if (run_dir / "manifest.json").exists():
        raise RuntimeError(f"Run directory already exists but does not match requested inputs: {run_dir}")

    run_dir.mkdir(parents=True, exist_ok=True)

    intent_path = run_dir / "intent.json"
    manifest_path = run_dir / "manifest.json"

    intent = contracts.ResearchIntent(
        schema_version="1",
        contract_version=contracts.CONTRACT_VERSION,
        run_id=run_id,
        asof_utc=asof_utc.isoformat(),
        policy_name=policy_name,
        asset_class=asset_class,
        symbols=symbols,
        pipeline="PHASE2_STUB",
        notes=[
            "PHASE2_STUB: This run intentionally emits only an intent artifact.",
            "No Postgres schema/adapters/pipeline for this asset class yet.",
        ],
    )
    _write_intent_contract(intent_path, intent)

    outputs = {"intent_json": file_record(intent_path)}
    inputs = {"intent": {"asset_class": asset_class, "pipeline": "PHASE2_STUB"}, "optional": {"pg_used": False}}

    man = contracts.ResearchManifest(
        schema_version="1",
        contract_version=contracts.CONTRACT_VERSION,
        run_id=run_id,
        asof_utc=asof_utc.isoformat(),
        policy_name=policy_name,
        policy_path=str(policy_path),
        policy_sha256=policy_sha,
        params=params,
        inputs=inputs,
        outputs=outputs,
        notes=[f"PHASE2_STUB: asset_class={asset_class}. Intent-only artifact emitted."],
    )
    _write_manifest_contract(manifest_path, man)

    return run_dir


def main(argv: Optional[list[str]] = None) -> None:
    p = argparse.ArgumentParser(
        prog="mqk-research",
        description="Deterministic research CLI for MiniQuantDesk V4",
    )
    sub = p.add_subparsers(dest="cmd", required=True)

    sub.add_parser("preflight", help="DB sanity checks (md_bars presence, ranges, timeframes, symbols)")

    common = argparse.ArgumentParser(add_help=False)
    common.add_argument("--policy", required=True, help="Policy YAML path")
    common.add_argument("--asof-utc", required=True, help="ASOF timestamp with timezone (e.g. 2026-02-24T00:00:00Z)")
    common.add_argument("--pg-url", required=False, default=None, help="Postgres URL. If omitted, MQK_PG_URL env var is used.")
    common.add_argument("--out", default="runs", help="Output root directory (default: runs/)")
    common.add_argument("--symbols", required=True, help="Comma-separated symbols")

    sub.add_parser("features", parents=[common], help="Phase 1: compute features (also emits universe/targets for determinism)")
    sub.add_parser("universe", parents=[common], help="Phase 1: build universe (also computes features/targets)")
    sub.add_parser("targets", parents=[common], help="Phase 1: build targets (also computes features/universe)")
    sub.add_parser("run", parents=[common], help="Run based on policy asset_class (EQUITY Phase1, others Phase2 stub)")

    args = p.parse_args(argv)

    _load_dotenv_if_present(Path(".env"))

    pg_url = getattr(args, "pg_url", None) or os.environ.get("MQK_PG_URL")

    if args.cmd == "preflight":
        if not pg_url:
            raise RuntimeError("Missing Postgres connection. Provide --pg-url OR set MQK_PG_URL in the environment.")
        engine = make_engine(PgConfig(url=pg_url))
        print(json.dumps(preflight(engine), indent=2, sort_keys=True))
        return

    policy_path = Path(args.policy)
    asof_utc = _parse_utc_ts(args.asof_utc, "asof_utc")
    out_root = Path(args.out)

    policy = _load_policy(policy_path)
    asset_class = str(policy.get("asset_class", "EQUITY")).upper()

    if asset_class in ("OPTIONS", "FUTURES"):
        run_dir = run_phase2_stub(
            policy_path=policy_path,
            asof_utc=asof_utc,
            out_root=out_root,
            symbols_csv=args.symbols,
        )
        print(str(run_dir))
        return

    if not pg_url:
        raise RuntimeError(
            "Missing Postgres connection.\n"
            "Provide --pg-url OR set MQK_PG_URL in the environment (or .env loaded into the shell)."
        )

    run_dir = run_phase1_equity(
        policy_path=policy_path,
        asof_utc=asof_utc,
        pg_url=pg_url,
        out_root=out_root,
        symbols_csv=args.symbols,
    )
    print(str(run_dir))


if __name__ == "__main__":
    main()