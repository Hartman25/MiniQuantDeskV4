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
    _require_md_bars_nonempty(engine)
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