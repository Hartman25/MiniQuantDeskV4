"""The holdout guard must fail on every category when a holdout-period row appears, and must not pass
vacuously."""

from __future__ import annotations

import json
import sqlite3
import sys
from pathlib import Path

import pandas as pd
import pytest

HERE = Path(__file__).resolve().parent
sys.path.insert(0, str(HERE))
sys.path.insert(0, str(HERE.parents[1] / "src"))

import holdout_guard as hg  # noqa: E402

DECL = {"partition": {"holdout_months": 6}, "universe": {"symbols": ["SPY", "EFA"]}}
START = pd.Timestamp("2026-03-01", tz="UTC")
BEFORE = int(pd.Timestamp("2026-02-27T05:00:00", tz="UTC").timestamp())
AFTER = int(pd.Timestamp("2026-03-02T05:00:00", tz="UTC").timestamp())


def write(path: Path, column: str, values):
    path.parent.mkdir(parents=True, exist_ok=True)
    pd.DataFrame({column: values}).to_csv(path, index=False)


def build(tmp: Path, *, ledger_status="reserved") -> tuple[Path, Path]:
    run = tmp / "run"
    dates = pd.date_range("2025-01-02", "2026-08-31", freq="B", tz="UTC")
    bars = pd.DataFrame([{"symbol": s, "end_ts": d.isoformat(), "close": 1.0} for s in DECL["universe"]["symbols"]
                         for d in dates])
    (run / "data").mkdir(parents=True)
    bars.to_csv(run / "data" / "research_bars.csv", index=False)
    iso = [pd.Timestamp("2026-02-26", tz="UTC").isoformat(), pd.Timestamp("2026-02-27", tz="UTC").isoformat()]
    for name, pattern, column, kind in hg.CATEGORIES:
        path = run / pattern.replace("*", "x")
        write(path, column, [BEFORE - 86400, BEFORE] if kind == "epoch" else iso)
    registry = tmp / "research.sqlite3"
    con = sqlite3.connect(registry)
    con.execute("create table research_holdout_ledger (holdout_id text, status text, consumed_at text, "
                "consumer_identity_json text)")
    con.execute("insert into research_holdout_ledger values ('h', ?, ?, ?)",
                (ledger_status, None if ledger_status == "reserved" else "2026-10-04", None))
    con.commit()
    con.close()
    return run, registry


def test_clean_tree_passes_in_both_phases(tmp_path):
    run, registry = build(tmp_path)
    post = hg.check(DECL, run, registry, "post")
    assert post["holdout_start_utc"] == START.isoformat() and post["ledger_all_reserved"]
    assert all(c["files"] == 1 for c in post["categories"].values()) and len(post["categories"]) == len(hg.CATEGORIES)
    assert hg.check(DECL, run, registry, "pre")["ledger_rows"] == 1


@pytest.mark.parametrize("category", [c[0] for c in hg.CATEGORIES])
def test_a_holdout_row_in_any_category_is_a_breach(tmp_path, category):
    run, registry = build(tmp_path)
    name, pattern, column, kind = next(c for c in hg.CATEGORIES if c[0] == category)
    bad = [BEFORE, AFTER] if kind == "epoch" else ["2026-02-27T00:00:00+00:00", "2026-03-01T05:00:00+00:00"]
    write(run / pattern.replace("*", "x"), column, bad)
    with pytest.raises(hg.HoldoutBreach, match=category):
        hg.check(DECL, run, registry, "post")


def test_a_consumed_ledger_row_is_a_breach(tmp_path):
    run, registry = build(tmp_path, ledger_status="consumed")
    with pytest.raises(hg.HoldoutBreach, match="not RESERVED/UNCONSUMED"):
        hg.check(DECL, run, registry, "pre")


def test_post_phase_never_passes_vacuously(tmp_path):
    run, registry = build(tmp_path)
    (run / "placebo" / "x" / "x" / "eval" / "economic_returns.csv").unlink()
    with pytest.raises(hg.HoldoutBreach, match="placebo: no artifact"):
        hg.check(DECL, run, registry, "post")
    hg.check(DECL, run, registry, "pre")  # before the campaign nothing exists yet and that is fine
    empty = tmp_path / "other"
    run2, _ = build(empty)
    with pytest.raises(hg.HoldoutBreach, match="no holdout ledger row"):
        hg.check(DECL, run2, empty / "missing.sqlite3", "post")


def test_the_guard_report_carries_the_access_incident_and_never_certifies_independence(tmp_path):
    kiss = json.loads((HERE / "PREDECLARED_KISS_EXT032_ETF_01.json").read_text(encoding="utf-8"))
    decl = {"partition": {"holdout_months": 6}, "universe": {"symbols": ["SPY", "EFA"]},
            "data": {"end_utc": "2026-09-01T00:00:00Z"}}
    run, registry = build(tmp_path)
    report = hg.check(decl, run, registry, "post")
    assert report["ledger_all_reserved"] is True
    assert report["access_incident"]["access_incident_status"] == "ACCESS_INCIDENT_PENDING_ADJUDICATION"
    assert report["access_incident"]["independence_certification_blocked"] is True
    assert kiss["holdout"]["access_incident"]["state"] == report["access_incident"]["access_incident_status"]
