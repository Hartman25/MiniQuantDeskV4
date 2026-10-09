"""The prospective campaign's data path cannot request or admit a reserved date, and the fixed partition
reproduces the accepted ten folds without fabricated rows. Synthetic bars and a stub extractor only:
no provider, no registry write beyond a temp directory, no economics."""

from __future__ import annotations

import argparse
import copy
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
import holdout_incident as hi  # noqa: E402
import stage_auth_testkit  # noqa: E402
import mqk_research.data.alpaca_historical as ah  # noqa: E402
import mqk_research.ml.native_signal_registry_integration as bridge  # noqa: E402
from test_hermetic_provider_isolation import _load_runner  # noqa: E402
from test_kiss_ext032_registration_gate import STRATEGY, fingerprints, manifest, rb  # noqa: E402

SYMBOLS = ["SPY", "QQQ", "IWM", "DIA"]
BOUNDARY = rb.DECL["partition"]["holdout_boundary"]
START = pd.Timestamp("2026-03-01", tz="UTC")
EXPECTED_FOLD_STARTS = [f"{y}-03-01" for y in range(2016, 2026)]
# trial id of SPY under the pre-change bridge for the fixture below (computed from the unmodified source)
HISTORICAL_GOLDEN_TRIAL_ID = "6656affe066b94f4d17399ead8530057"


def bars(first="2016-01-04", last="2026-02-27", symbols=SYMBOLS, extra=None) -> pd.DataFrame:
    dates = pd.bdate_range(first, last, tz="UTC")
    rows = [{"symbol": s, "end_ts": d.isoformat(), "open": 1.0, "high": 1.0, "low": 1.0, "close": 1.0, "volume": 1}
            for s in symbols for d in dates]
    rows += list(extra or [])
    return pd.DataFrame(rows)


def plan(df: pd.DataFrame, boundary=BOUNDARY):
    ts = pd.to_datetime(df["end_ts"], utc=True)
    return bridge.plan_native_folds(t_min=ts.min(), t_max=ts.max(), evaluation_start_utc=pd.Timestamp("2016-03-01", tz="UTC"),
                                    test_months=12, holdout_months=6, fixed_holdout_boundary=boundary)


# ------------------------------------------------------------------ chronology contract

def test_the_fixed_partition_reproduces_the_ten_accepted_folds_and_the_reserved_window_exactly():
    folds, hold_start, dataset_end = plan(bars())
    assert [f.test_start.strftime("%Y-%m-%d") for f in folds] == EXPECTED_FOLD_STARTS
    assert folds[-1].test_end == START == hold_start and len(folds) == rb.DECL["partition"]["expected_folds"] == 10
    assert dataset_end == pd.Timestamp("2026-09-01", tz="UTC")
    # identical to what the historical derivation yields from the (never fetched) bars through 2026-08-31
    h_folds, h_start, h_end = plan(bars(last="2026-08-31"), boundary=None)
    assert [(f.test_start, f.test_end) for f in folds] == [(f.test_start, f.test_end) for f in h_folds]
    assert (hold_start, dataset_end) == (h_start, h_end)


def test_no_fabricated_dataset_end_observation_exists_in_the_bars_or_the_cut():
    df = bars()
    ts = pd.to_datetime(df["end_ts"], utc=True)
    _, hold_start, dataset_end = plan(df)
    assert ts.max() < hold_start < dataset_end and not (ts >= dataset_end).any() and not (ts >= hold_start).any()


def test_a_missing_final_month_does_not_move_the_boundary_it_fails_closed():
    short = bars(last="2026-01-30")  # February absent
    with pytest.raises(bridge.NativeSignalError, match="boundary does not move"):
        plan(short)
    # the defect being closed: the historical derivation silently slides the reserved window back a month
    _, slid, _ = plan(short, boundary=None)
    assert slid == pd.Timestamp("2025-08-01", tz="UTC") != START


@pytest.mark.parametrize("stamp,ok", [("2026-02-27T00:00:00+00:00", True), ("2026-02-28T23:59:59+00:00", True),
                                      ("2026-03-01T00:00:00+00:00", False), ("2026-03-02T00:00:00+00:00", False)])
def test_the_boundary_is_exclusive_the_instant_itself_is_reserved(stamp, ok):
    df = bars(extra=[{"symbol": "SPY", "end_ts": stamp, "open": 1.0, "high": 1.0, "low": 1.0, "close": 1.0, "volume": 1}])
    if ok:
        plan(df)
    else:
        with pytest.raises(bridge.NativeSignalError, match="at or after the reserved holdout start"):
            plan(df)


@pytest.mark.parametrize("label,boundary", [
    ("wrong keys", {"version": "fixed_holdout_boundary_v1", "holdout_start_utc": "2026-03-01T00:00:00Z"}),
    ("wrong version", {**BOUNDARY, "version": "fixed_holdout_boundary_v2"}),
    ("naive timestamp", {**BOUNDARY, "holdout_start_utc": "2026-03-01T00:00:00"}),
    ("not month aligned", {**BOUNDARY, "holdout_start_utc": "2026-03-02T00:00:00Z"}),
    ("end not start+6 months", {**BOUNDARY, "holdout_end_utc": "2026-08-01T00:00:00Z"}),
    ("unreadable", {**BOUNDARY, "holdout_end_utc": "not-a-date"}),
    ("not a mapping", "2026-03-01"),
])
def test_a_malformed_fixed_boundary_is_refused(label, boundary):
    with pytest.raises(bridge.NativeSignalError):
        bridge.validate_fixed_holdout_boundary(boundary, 6)


def test_native_holdout_start_and_the_backtest_cut_use_the_declared_boundary(tmp_path):
    src = tmp_path / "bars.csv"
    bars().to_csv(src, index=False)
    for sym in SYMBOLS:
        hold = bridge.native_holdout_start(src, sym, 6, BOUNDARY)
        assert hold == START
        out = bridge.research_bars_to_backtest_csv(src, sym, tmp_path / f"{sym}.csv", end_exclusive_utc=hold)
        assert pd.to_datetime(pd.read_csv(out)["end_ts"].astype("int64"), unit="s", utc=True).max() < hold
    reserved = bars(extra=[{"symbol": "SPY", "end_ts": "2026-03-02T00:00:00+00:00", "open": 1.0, "high": 1.0, "low": 1.0,
                            "close": 1.0, "volume": 1}])
    bad = tmp_path / "bad.csv"
    reserved.to_csv(bad, index=False)
    with pytest.raises(bridge.NativeSignalError, match="at or after the reserved holdout start"):
        bridge.native_holdout_start(bad, "SPY", 6, BOUNDARY)


# ------------------------------------------------------------------ identity compatibility

def _identity(man, boundary=None, symbol="SPY"):
    return bridge.build_native_signal_trial_identity(
        experiment_id=rb.EXPERIMENT, hypothesis_id=rb.HYP[STRATEGY]["hypothesis_id"], strategy_id=STRATEGY, symbol=symbol,
        semantic_fingerprint="01" * 32, required_history_bars=2, bars_provenance=man,
        evaluation_start_utc=pd.Timestamp(rb.DECL["partition"]["evaluation_start_utc"]), test_months=12, holdout_months=6,
        economic_spec=rb._economic_spec(), capital_sizing=rb.research_capital_sizing(rb.DECL),
        stress_contract=rb.research_stress_contract(rb.DECL), canonical_timeframe_identity=True,
        fixed_holdout_boundary=boundary)


def test_historical_trial_identity_is_byte_identical_and_the_fixed_boundary_is_identity_bearing(tmp_path):
    man = manifest(tmp_path)
    trial_id, identity = _identity(man)
    assert trial_id == HISTORICAL_GOLDEN_TRIAL_ID, "a campaign without a fixed boundary must keep its trial ids"
    assert "holdout_boundary" not in identity["evaluation_spec"]
    fixed_id, fixed_identity = _identity(man, BOUNDARY)
    assert fixed_id != trial_id and fixed_identity["evaluation_spec"]["holdout_boundary"]["holdout_start_utc"].startswith("2026-03-01")
    moved = {**BOUNDARY, "holdout_start_utc": "2026-02-01T00:00:00Z", "holdout_end_utc": "2026-08-01T00:00:00Z"}
    assert _identity(man, moved)[0] not in (trial_id, fixed_id)


def test_the_declared_boundary_flows_into_every_expected_trial_identity(tmp_path):
    man = manifest(tmp_path)
    expected = rb.expected_trial_ids(fingerprints(), man)
    assert {e[3]["evaluation_spec"]["holdout_boundary"]["holdout_end_utc"][:10] for e in expected} == {"2026-09-01"}
    saved = copy.deepcopy(rb.DECL)
    try:
        del rb.DECL["partition"]["holdout_boundary"]
        legacy = rb.expected_trial_ids(fingerprints(), man)
    finally:
        rb.DECL.clear()
        rb.DECL.update(saved)
    assert {e[2] for e in expected}.isdisjoint({e[2] for e in legacy})


# ------------------------------------------------------------------ the declaration and the request path

def test_the_declaration_requests_exactly_the_development_window():
    d = rb.DECL
    assert (d["data"]["start_utc"], d["data"]["end_utc"]) == ("2016-01-01T00:00:00Z", "2026-03-01T00:00:00Z")
    assert d["data"]["end_utc"] == BOUNDARY["holdout_start_utc"]
    rb.require_development_window(d)
    assert hi.reserved_window(d) == (START, pd.Timestamp("2026-09-01", tz="UTC"))


@pytest.mark.parametrize("label,mutate", [
    ("old Batch-03 window", lambda d: d["data"].update(end_utc="2026-09-01T00:00:00Z")),
    ("one day into the reserved window", lambda d: d["data"].update(end_utc="2026-03-02T00:00:00Z")),
    ("naive end", lambda d: d["data"].update(end_utc="2026-03-01T00:00:00")),
    ("end before start", lambda d: d["data"].update(end_utc="2015-01-01T00:00:00Z")),
    ("boundary stripped from a graded declaration", lambda d: d["partition"].pop("holdout_boundary")),
])
def test_a_request_that_could_reach_a_reserved_date_is_refused_before_any_effect(label, mutate):
    d = copy.deepcopy(rb.DECL)
    mutate(d)
    with pytest.raises(SystemExit, match="fail-closed"):
        rb.require_development_window(d)


def test_historical_declarations_are_untouched_by_the_development_window_rule():
    for name in ("PREDECLARED_BATCH_03.json", "PREDECLARED_BATCH_02.json", "PREDECLARED_CAMPAIGN_03.json"):
        decl = json.loads((HERE / name).read_text(encoding="utf-8"))
        rb.require_development_window(decl)
        assert rb.fixed_holdout_boundary(decl) is None


@pytest.fixture
def fetch_world(tmp_path, monkeypatch):
    """The real stage_fetch (authorized, hermetic credentials loader stubbed) with a stub extractor that records
    the exact request it receives and returns a caller-chosen frame."""
    decl = copy.deepcopy(json.loads((HERE / "PREDECLARED_KISS_EXT032_ETF_01.json").read_text(encoding="utf-8")))
    decl["execution_gate"] = {"status": "MUTATED_OPEN", "executable": True, "blocker": None}
    decl["run_dir"] = str(tmp_path / "run")
    decl["experiment"]["registry_db_relative_path"] = str(tmp_path / "run" / "registry" / "research.sqlite3")
    path = tmp_path / "decl.json"
    path.write_text(json.dumps(decl), encoding="utf-8")
    runner = _load_runner(path)
    stage_auth_testkit.grant_runner_stages(monkeypatch, runner)
    monkeypatch.setattr(runner, "_load_alpaca_env", lambda: None)
    calls: list[dict] = []
    state = {"frame": bars()}

    def fake_extract(**kw):
        calls.append(kw)
        return {"bars": state["frame"], "manifest": {}, "corporate_action_evidence": {}, "corporate_action_entries": []}

    monkeypatch.setattr(ah, "extract_research_bars_with_provenance", fake_extract)
    written: list = []
    monkeypatch.setattr(ah, "write_research_extraction_artifacts", lambda dest, result: written.append(dest) or {})
    return runner, calls, state, written, tmp_path


def test_stage_fetch_requests_exactly_the_development_window_and_no_reserved_timestamp(fetch_world):
    runner, calls, _, written, _ = fetch_world
    runner.stage_fetch(argparse.Namespace(execute=True))
    (req,) = calls
    assert (req["start_utc"], req["end_utc"]) == (pd.Timestamp("2016-01-01T00:00:00Z"), START)
    assert req["end_utc"] <= START and written, "the stub run reached the artifact write only after verification"
    assert all(pd.Timestamp(v) <= START for k, v in req.items() if k.endswith("_utc"))  # end is the exclusive bound


def test_a_returned_reserved_row_is_refused_before_anything_is_written(fetch_world):
    runner, calls, state, written, tmp_path = fetch_world
    state["frame"] = bars(extra=[{"symbol": "SPY", "end_ts": "2026-03-02T00:00:00+00:00", "open": 1.0, "high": 1.0,
                                  "low": 1.0, "close": 1.0, "volume": 1}])
    with pytest.raises(SystemExit, match="at or after the reserved holdout start"):
        runner.stage_fetch(argparse.Namespace(execute=True))
    assert written == [] and not (tmp_path / "run" / "data").exists()


def test_symbols_with_different_coverage_fail_closed(fetch_world):
    runner, _, state, written, _ = fetch_world
    ragged = bars()
    ragged = ragged[~((ragged["symbol"] == "DIA") & (pd.to_datetime(ragged["end_ts"], utc=True) >= pd.Timestamp("2026-02-20", tz="UTC")))]
    state["frame"] = ragged
    with pytest.raises(SystemExit, match="different spans"):
        runner.stage_fetch(argparse.Namespace(execute=True))
    state["frame"] = bars(first="2016-01-05")  # late start for all passes the span test; one symbol late does not
    late = bars()
    late = late[~((late["symbol"] == "IWM") & (pd.to_datetime(late["end_ts"], utc=True) < pd.Timestamp("2016-02-01", tz="UTC")))]
    state["frame"] = late
    with pytest.raises(SystemExit, match="different spans"):
        runner.stage_fetch(argparse.Namespace(execute=True))
    assert written == []


def test_a_row_before_the_declared_start_is_refused(fetch_world):
    runner, _, state, written, _ = fetch_world
    state["frame"] = bars(first="2015-12-01")
    with pytest.raises(SystemExit, match="before the declared development start"):
        runner.stage_fetch(argparse.Namespace(execute=True))
    assert written == []


def test_the_old_window_is_refused_before_credentials_or_the_extractor(fetch_world):
    runner, calls, _, written, tmp_path = fetch_world
    runner.DECL["data"]["end_utc"] = "2026-09-01T00:00:00Z"
    runner._load_alpaca_env = lambda: pytest.fail("credentials were loaded for a window that reaches the reserved dates")
    with pytest.raises(SystemExit, match="reserved"):
        runner.stage_fetch(argparse.Namespace(execute=True))
    assert calls == [] and written == []


# ------------------------------------------------------------------ guard and downstream artifacts

def _guard_world(tmp: Path, rows_extra=None):
    run = tmp / "run"
    (run / "data").mkdir(parents=True)
    bars(extra=rows_extra).to_csv(run / "data" / "research_bars.csv", index=False)
    before = int(pd.Timestamp("2026-02-27T05:00:00", tz="UTC").timestamp())
    iso = [pd.Timestamp("2026-02-26", tz="UTC").isoformat(), pd.Timestamp("2026-02-27", tz="UTC").isoformat()]
    for name, pattern, column, kind in hg.CATEGORIES:
        path = run / pattern.replace("*", "x")
        path.parent.mkdir(parents=True, exist_ok=True)
        pd.DataFrame({column: [before - 86400, before] if kind == "epoch" else iso}).to_csv(path, index=False)
    registry = tmp / "research.sqlite3"
    con = sqlite3.connect(registry)
    con.execute("create table research_holdout_ledger (holdout_id text, status text, consumed_at text, consumer_identity_json text)")
    con.execute("insert into research_holdout_ledger values ('h', 'reserved', NULL, NULL)")
    con.commit()
    con.close()
    return run, registry


def test_the_guard_uses_the_declared_boundary_and_checks_the_fetched_bars_too(tmp_path):
    decl = {"partition": rb.DECL["partition"], "universe": {"symbols": SYMBOLS}, "data": rb.DECL["data"]}
    run, registry = _guard_world(tmp_path / "ok")
    report = hg.check(decl, run, registry, "post")
    assert report["holdout_start_utc"] == START.isoformat() and "research_bars_fetch" in report["categories"]
    assert report["categories"]["research_bars_fetch"]["files"] == 1
    run, registry = _guard_world(tmp_path / "bad", rows_extra=[{"symbol": "SPY", "end_ts": "2026-03-02T00:00:00+00:00", "open": 1.0,
                                                               "high": 1.0, "low": 1.0, "close": 1.0, "volume": 1}])
    with pytest.raises(hg.HoldoutBreach, match="at or after the reserved holdout start"):
        hg.check(decl, run, registry, "post")


def test_the_guard_for_a_historical_declaration_keeps_its_categories_and_derivation(tmp_path):
    decl = {"partition": {"holdout_months": 6}, "universe": {"symbols": SYMBOLS}}
    run, registry = _guard_world(tmp_path)
    (run / "data" / "research_bars.csv").write_text(bars(last="2026-08-31").to_csv(index=False), encoding="utf-8")
    report = hg.check(decl, run, registry, "post")
    assert "research_bars_fetch" not in report["categories"] and report["holdout_start_utc"] == START.isoformat()
