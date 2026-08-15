"""
RESEARCH-PURGED-HOLDOUT-01 — purged walk-forward + reserved holdout tests.

Two layers:
  * Pure-helper tests (fast, exact-boundary precision) for the purge/embargo/
    holdout mask primitives in mqk_research.ml.eval_walkforward.
  * Integration tests (via run_walkforward_eval against real CSV run dirs)
    for fail-closed conditions, artifact provenance, standardization/holdout
    isolation, row-order and multi-symbol invariance, and deterministic replay.

No subprocess, no broker, no OMS, no network, no DB.
"""
from __future__ import annotations

import json
from pathlib import Path

import numpy as np
import pandas as pd
import pytest

from mqk_research.ml.eval_walkforward import (
    WalkForwardSpec,
    compute_holdout_boundary,
    discovery_usable_mask,
    make_folds,
    run_walkforward_eval,
    train_purge_masks,
)
from mqk_research.ml.schema import generate_feature_schema


# ---------------------------------------------------------------------------
# Fixture helpers
# ---------------------------------------------------------------------------

def _ts(s: str) -> pd.Timestamp:
    return pd.Timestamp(s, tz="UTC")


def _write_run_dir(run_dir: Path, feats: pd.DataFrame, targs: pd.DataFrame) -> None:
    run_dir.mkdir(parents=True, exist_ok=True)
    f = feats.copy()
    f["end_ts"] = f["end_ts"].apply(lambda t: pd.Timestamp(t).isoformat())
    f.to_csv(run_dir / "features.csv", index=False)

    t = targs.copy()
    t["end_ts"] = t["end_ts"].apply(lambda t2: pd.Timestamp(t2).isoformat())
    if "label_end_ts" in t.columns:
        t["label_end_ts"] = t["label_end_ts"].apply(
            lambda x: pd.Timestamp(x).isoformat() if pd.notna(x) else x
        )
    t.to_csv(run_dir / "targets.csv", index=False)

    generate_feature_schema(run_dir, id_columns=["symbol", "end_ts"])


def _build_multi_symbol_dataset(
    symbols=("AAA", "BBB", "CCC"),
    start="2020-01-01",
    periods_days=560,
    horizon_days=3,
    seed=0,
    feature_scale=1.0,
) -> pd.DataFrame:
    rng = np.random.default_rng(seed)
    dates = pd.date_range(start, periods=periods_days, freq="D", tz="UTC")
    rows = []
    for sym in symbols:
        for d in dates:
            f1 = feature_scale * float(rng.normal())
            target = 1 if f1 > 0.0 else 0
            rows.append({
                "symbol": sym,
                "end_ts": d,
                "f1": f1,
                "target": target,
                "label_end_ts": d + pd.Timedelta(days=horizon_days),
            })
    return pd.DataFrame(rows)


def _split_feats_targs(df: pd.DataFrame, *, with_label_end: bool = True):
    feats = df[["symbol", "end_ts", "f1"]].copy()
    cols = ["symbol", "end_ts", "target"] + (["label_end_ts"] if with_label_end else [])
    targs = df[cols].copy()
    return feats, targs


BASE_SPEC_KW = dict(train_years=1, test_months=1, step_months=1, holdout_months=1, min_rows_per_fold=200)


# ---------------------------------------------------------------------------
# Pure-helper tests: purge / embargo / holdout masks (TESTS 1-7)
# ---------------------------------------------------------------------------

def test_baseline_overlap_is_purged():
    """TEST 1 — training row whose label reaches into test must not be effective."""
    test_start = _ts("2021-01-01")
    cutoff = test_start  # embargo = 0
    tr_candidate = pd.Series([True])
    label_end = pd.Series([_ts("2021-01-02")])  # > test_start -> overlap
    overlap, embargo, effective = train_purge_masks(tr_candidate, label_end, test_start, cutoff)
    assert bool(overlap.iloc[0]) is True
    assert bool(embargo.iloc[0]) is False
    assert bool(effective.iloc[0]) is False


def test_exact_test_boundary_is_purged():
    """TEST 2 — label_end_ts == test_start must be purged. label_end_ts is the
    INCLUSIVE last bar consumed by the label, so a label ending exactly at
    test_start has already consumed test-period information; equality is not safe."""
    test_start = _ts("2021-01-01")
    cutoff = test_start  # embargo = 0
    tr_candidate = pd.Series([True])
    label_end = pd.Series([test_start])  # exactly equal -> consumed test_start's bar
    overlap, embargo, effective = train_purge_masks(tr_candidate, label_end, test_start, cutoff)
    assert bool(overlap.iloc[0]) is True
    assert bool(embargo.iloc[0]) is False
    assert bool(effective.iloc[0]) is False


def test_exact_embargo_cutoff_boundary_is_purged():
    """label_end_ts == effective_train_cutoff must be purged (the embargo cutoff
    bar's close has already been consumed by the label at that exact timestamp)."""
    test_start = _ts("2021-01-10")
    cutoff = test_start - pd.Timedelta(days=2)  # embargo = 2 days -> cutoff = 2021-01-08
    tr_candidate = pd.Series([True])
    label_end = pd.Series([cutoff])  # exactly equal to cutoff
    overlap, embargo, effective = train_purge_masks(tr_candidate, label_end, test_start, cutoff)
    assert bool(overlap.iloc[0]) is False
    assert bool(embargo.iloc[0]) is True
    assert bool(effective.iloc[0]) is False


def test_label_end_strictly_before_cutoff_is_allowed():
    """label_end_ts < effective_train_cutoff by the smallest fixture increment
    (1 second) is allowed."""
    test_start = _ts("2021-01-10")
    cutoff = test_start - pd.Timedelta(days=2)  # 2021-01-08
    tr_candidate = pd.Series([True])
    label_end = pd.Series([cutoff - pd.Timedelta(seconds=1)])
    overlap, embargo, effective = train_purge_masks(tr_candidate, label_end, test_start, cutoff)
    assert bool(overlap.iloc[0]) is False
    assert bool(embargo.iloc[0]) is False
    assert bool(effective.iloc[0]) is True


def test_embargo_removes_otherwise_nonoverlapping_row():
    """TEST 3 — label_end_ts <= test_start but inside the embargo window is excluded."""
    test_start = _ts("2021-01-10")
    cutoff = test_start - pd.Timedelta(days=2)  # embargo = 2 days
    tr_candidate = pd.Series([True])
    label_end = pd.Series([_ts("2021-01-09")])  # <= test_start, but > cutoff
    overlap, embargo, effective = train_purge_masks(tr_candidate, label_end, test_start, cutoff)
    assert bool(overlap.iloc[0]) is False
    assert bool(embargo.iloc[0]) is True
    assert bool(effective.iloc[0]) is False


def test_zero_embargo_does_not_invent_gap():
    """TEST 4 — with embargo=0 (cutoff == test_start), a label strictly before
    test_start is unaffected by any embargo-purge artifact; only labels that
    reach test_start's bar or later are excluded."""
    test_start = _ts("2021-01-10")
    cutoff = test_start  # embargo = 0
    tr_candidate = pd.Series([True, True])
    label_end = pd.Series([_ts("2021-01-09"), _ts("2021-01-09T23:59:59")])
    overlap, embargo, effective = train_purge_masks(tr_candidate, label_end, test_start, cutoff)
    assert not overlap.any()
    assert not embargo.any()
    assert effective.tolist() == [True, True]


def test_holdout_never_in_train_or_test_window():
    """TEST 5/6 — rows at/after holdout_start are excluded from discovery use entirely."""
    holdout_start = _ts("2021-06-01")
    feature_ts = pd.Series([_ts("2021-05-31"), _ts("2021-06-01"), _ts("2021-06-02")])
    label_end = pd.Series([_ts("2021-05-31"), _ts("2021-06-01"), _ts("2021-06-02")])
    mask = discovery_usable_mask(feature_ts, label_end, holdout_start)
    assert mask.tolist() == [True, False, False]


def test_pre_holdout_label_overlap_excluded():
    """TEST 7 — feature_ts before holdout but label reaching into holdout is excluded."""
    holdout_start = _ts("2021-06-01")
    feature_ts = pd.Series([_ts("2021-05-30"), _ts("2021-05-30")])
    label_end = pd.Series([_ts("2021-05-31"), _ts("2021-06-02")])  # 2nd reaches into holdout
    mask = discovery_usable_mask(feature_ts, label_end, holdout_start)
    assert mask.tolist() == [True, False]


def test_holdout_label_at_boundary_excluded_from_discovery():
    """Holdout boundary A — feature_ts before holdout but label_end_ts ==
    holdout_start has already consumed the holdout boundary bar's close and
    must be excluded from discovery."""
    holdout_start = _ts("2021-06-01")
    feature_ts = pd.Series([_ts("2021-05-30")])
    label_end = pd.Series([holdout_start])
    mask = discovery_usable_mask(feature_ts, label_end, holdout_start)
    assert mask.tolist() == [False]


def test_holdout_label_strictly_before_boundary_remains_eligible():
    """Holdout boundary B — label_end_ts strictly before holdout_start remains
    eligible for discovery (subject to all other discovery rules)."""
    holdout_start = _ts("2021-06-01")
    feature_ts = pd.Series([_ts("2021-05-30")])
    label_end = pd.Series([holdout_start - pd.Timedelta(seconds=1)])
    mask = discovery_usable_mask(feature_ts, label_end, holdout_start)
    assert mask.tolist() == [True]


def test_holdout_boundary_is_deterministic_and_wallclock_independent():
    t_min = _ts("2020-01-15")
    t_max = _ts("2021-06-20")
    a1, h1, e1 = compute_holdout_boundary(t_min, t_max, 3)
    a2, h2, e2 = compute_holdout_boundary(t_min, t_max, 3)
    assert (a1, h1, e1) == (a2, h2, e2)
    assert a1 == _ts("2020-01-01")
    assert e1 == _ts("2021-07-01")
    assert h1 == _ts("2021-04-01")


def test_make_folds_preserves_rolling_window_semantics():
    t_min = _ts("2020-01-01")
    discovery_end = _ts("2022-01-01")
    spec = WalkForwardSpec(train_years=1, test_months=1, step_months=1).normalized()
    folds = make_folds(t_min=t_min, discovery_end=discovery_end, spec=spec)
    assert len(folds) >= 2
    for i in range(1, len(folds)):
        prev_train_start = folds[i - 1][0]
        cur_train_start = folds[i][0]
        assert cur_train_start == prev_train_start + pd.DateOffset(months=1)
        # test_end must never exceed discovery_end
        assert folds[i][3] <= discovery_end


# ---------------------------------------------------------------------------
# Integration tests: fail-closed conditions (TESTS 12, 13, 14)
# ---------------------------------------------------------------------------

def test_missing_label_end_fails_closed_when_purge_enabled(tmp_path):
    """TEST 12"""
    df = _build_multi_symbol_dataset(symbols=("AAA",), periods_days=400)
    feats, targs = _split_feats_targs(df, with_label_end=False)
    run_dir = tmp_path / "run"
    _write_run_dir(run_dir, feats, targs)

    spec = WalkForwardSpec(**BASE_SPEC_KW)
    with pytest.raises(RuntimeError, match="label_end_ts"):
        run_walkforward_eval(run_dir, spec=spec, steps=5)


def test_invalid_label_interval_fails_closed(tmp_path):
    """TEST 13 — label_end_ts < feature_ts must fail."""
    df = _build_multi_symbol_dataset(symbols=("AAA",), periods_days=400)
    df.loc[df.index[10], "label_end_ts"] = df.loc[df.index[10], "end_ts"] - pd.Timedelta(days=1)
    feats, targs = _split_feats_targs(df, with_label_end=True)
    run_dir = tmp_path / "run"
    _write_run_dir(run_dir, feats, targs)

    spec = WalkForwardSpec(**BASE_SPEC_KW)
    with pytest.raises(RuntimeError, match="label_end_ts"):
        run_walkforward_eval(run_dir, spec=spec, steps=5)


def test_dataset_too_short_for_holdout_fails_closed(tmp_path):
    """TEST 14"""
    df = _build_multi_symbol_dataset(symbols=("AAA",), periods_days=40)
    feats, targs = _split_feats_targs(df, with_label_end=True)
    run_dir = tmp_path / "run"
    _write_run_dir(run_dir, feats, targs)

    spec = WalkForwardSpec(**BASE_SPEC_KW)
    with pytest.raises(RuntimeError, match="[Ff]ail-closed"):
        run_walkforward_eval(run_dir, spec=spec, steps=5)


# ---------------------------------------------------------------------------
# Integration tests: Blocker 2 — holdout isolation must not be disabled by
# purge_enabled=False (--no-purge only disables fold-level overlap/embargo
# purge; global holdout-label isolation is always enforced).
# ---------------------------------------------------------------------------

def _run_with_spec(run_dir: Path, df: pd.DataFrame, spec: WalkForwardSpec, *, steps: int = 5):
    feats, targs = _split_feats_targs(df, with_label_end=True)
    _write_run_dir(run_dir, feats, targs)
    out_path = run_walkforward_eval(run_dir, spec=spec, steps=steps)
    return json.loads(out_path.read_text(encoding="utf-8"))


def test_no_purge_missing_label_end_fails_closed(tmp_path):
    """Blocker 2 #3 — purge_enabled=False does not exempt targets.csv from
    carrying label_end_ts; holdout isolation cannot be established without it."""
    df = _build_multi_symbol_dataset(symbols=("AAA",), periods_days=400)
    feats, targs = _split_feats_targs(df, with_label_end=False)
    run_dir = tmp_path / "run"
    _write_run_dir(run_dir, feats, targs)

    spec = WalkForwardSpec(**{**BASE_SPEC_KW, "purge_enabled": False})
    with pytest.raises(RuntimeError, match="label_end_ts"):
        run_walkforward_eval(run_dir, spec=spec, steps=5)


def test_no_purge_still_purges_label_at_and_past_holdout_boundary(tmp_path):
    """Blocker 2 #1/#2 — purge_enabled=False must still exclude discovery rows
    whose label_end_ts is at (==) or past (>) holdout_start, and the artifact
    must clearly distinguish fold_purge_enabled=False from
    holdout_overlap_protection=enforced."""
    df = _build_multi_symbol_dataset(symbols=("AAA", "BBB"), periods_days=560)
    t_min = pd.to_datetime(df["end_ts"]).min()
    t_max = pd.to_datetime(df["end_ts"]).max()
    holdout_start = compute_holdout_boundary(t_min, t_max, BASE_SPEC_KW["holdout_months"])[1]
    spec = WalkForwardSpec(**{**BASE_SPEC_KW, "purge_enabled": False})

    baseline_out = _run_with_spec(tmp_path / "np_baseline", df, spec)
    baseline_purged = baseline_out["holdout"]["boundary_purged_discovery_rows"]
    assert baseline_out["holdout"]["fold_purge_enabled"] is False
    assert baseline_out["holdout"]["holdout_overlap_protection"] == "enforced"

    at_boundary = pd.concat([df, pd.DataFrame([{
        "symbol": "AAA",
        "end_ts": holdout_start - pd.Timedelta(hours=6),  # off the daily grid, avoids join-key collision
        "f1": 0.0,
        "target": 1,
        "label_end_ts": holdout_start,  # == holdout_start -> must be purged
    }])], ignore_index=True)
    out_at = _run_with_spec(tmp_path / "np_at_boundary", at_boundary, spec)
    assert out_at["holdout"]["boundary_purged_discovery_rows"] == baseline_purged + 1

    past_boundary = pd.concat([df, pd.DataFrame([{
        "symbol": "AAA",
        "end_ts": holdout_start - pd.Timedelta(hours=6),  # off the daily grid, avoids join-key collision
        "f1": 0.0,
        "target": 1,
        "label_end_ts": holdout_start + pd.Timedelta(days=5),  # > holdout_start -> purged
    }])], ignore_index=True)
    out_past = _run_with_spec(tmp_path / "np_past_boundary", past_boundary, spec)
    assert out_past["holdout"]["boundary_purged_discovery_rows"] == baseline_purged + 1


def test_no_purge_holdout_feature_magnitude_cannot_change_discovery_metrics(tmp_path):
    """Blocker 2 #4 — with purge_enabled=False, corrupting holdout-period
    feature values must not change discovery fold metrics."""
    df = _build_multi_symbol_dataset(symbols=("AAA", "BBB"), periods_days=560)
    holdout_start = compute_holdout_boundary(
        pd.to_datetime(df["end_ts"]).min(), pd.to_datetime(df["end_ts"]).max(), BASE_SPEC_KW["holdout_months"]
    )[1]
    spec = WalkForwardSpec(**{**BASE_SPEC_KW, "purge_enabled": False})

    out_baseline = _run_with_spec(tmp_path / "np_feat_baseline", df.copy(), spec)

    corrupted = df.copy()
    mask = pd.to_datetime(corrupted["end_ts"]) >= holdout_start
    corrupted.loc[mask, "f1"] = 1_000_000.0
    out_corrupted = _run_with_spec(tmp_path / "np_feat_corrupted", corrupted, spec)

    assert _discovery_metrics(out_baseline) == _discovery_metrics(out_corrupted)
    assert out_baseline["summary"] == out_corrupted["summary"]


def test_no_purge_holdout_target_flip_cannot_change_discovery_metrics(tmp_path):
    """Blocker 2 #5 — with purge_enabled=False, flipping holdout-period target
    values must not change discovery fold metrics."""
    df = _build_multi_symbol_dataset(symbols=("AAA", "BBB"), periods_days=560)
    holdout_start = compute_holdout_boundary(
        pd.to_datetime(df["end_ts"]).min(), pd.to_datetime(df["end_ts"]).max(), BASE_SPEC_KW["holdout_months"]
    )[1]
    spec = WalkForwardSpec(**{**BASE_SPEC_KW, "purge_enabled": False})

    out_baseline = _run_with_spec(tmp_path / "np_tgt_baseline", df.copy(), spec)

    flipped = df.copy()
    mask = pd.to_datetime(flipped["end_ts"]) >= holdout_start
    flipped.loc[mask, "target"] = 1 - flipped.loc[mask, "target"]
    out_flipped = _run_with_spec(tmp_path / "np_tgt_flipped", flipped, spec)

    assert _discovery_metrics(out_baseline) == _discovery_metrics(out_flipped)
    assert out_baseline["summary"] == out_flipped["summary"]


def test_artifact_distinguishes_fold_purge_from_holdout_protection(tmp_path):
    """Blocker 2 #6 — artifact must show fold_purge_enabled distinctly from
    holdout_overlap_protection, which is always "enforced" regardless of
    fold_purge_enabled."""
    df = _build_multi_symbol_dataset(symbols=("AAA", "BBB"), periods_days=560)

    spec_on = WalkForwardSpec(**BASE_SPEC_KW)
    out_on = _run_with_spec(tmp_path / "purge_on", df.copy(), spec_on)
    assert out_on["holdout"]["fold_purge_enabled"] is True
    assert out_on["holdout"]["holdout_overlap_protection"] == "enforced"

    spec_off = WalkForwardSpec(**{**BASE_SPEC_KW, "purge_enabled": False})
    out_off = _run_with_spec(tmp_path / "purge_off", df.copy(), spec_off)
    assert out_off["holdout"]["fold_purge_enabled"] is False
    assert out_off["holdout"]["holdout_overlap_protection"] == "enforced"


# ---------------------------------------------------------------------------
# Integration tests: controlled small fixture for min-rows-after-purge and
# multi-symbol purge membership (TESTS 11, 16)
# ---------------------------------------------------------------------------

def _controlled_fixture(run_dir: Path, train_specs, test_start: pd.Timestamp, test_end: pd.Timestamp):
    """train_specs: list of (symbol, feature_ts, label_end_ts, target).

    Densely populates the test window (>= floor of 50 rows) plus enough tail
    data past test_end to push the holdout boundary safely beyond test_end,
    so the intended fold is fully inside the discovery region.
    """
    rows = []
    for sym, fts, lend, tgt in train_specs:
        rows.append({"symbol": sym, "end_ts": fts, "f1": 1.0 if tgt else -1.0, "target": tgt, "label_end_ts": lend})

    tail_end = test_end + pd.Timedelta(days=45)
    dense_dates = pd.date_range(test_start, tail_end, freq="12h", tz="UTC")
    for i, d in enumerate(dense_dates):
        tgt = i % 2
        rows.append({
            "symbol": "T",
            "end_ts": d,
            "f1": 1.0 if tgt else -1.0,
            "target": tgt,
            "label_end_ts": d,
        })

    df = pd.DataFrame(rows)
    feats, targs = _split_feats_targs(df, with_label_end=True)
    _write_run_dir(run_dir, feats, targs)


def test_min_rows_after_purge_reason(tmp_path):
    """TEST 11 — candidate rows satisfy threshold, effective rows after purge do not."""
    test_start = _ts("2021-01-01")
    test_end = _ts("2021-03-01")  # test_months=2
    train_specs = [
        ("S", _ts("2020-01-01"), _ts("2020-01-05"), 1),  # survives
        ("S", _ts("2020-01-05"), _ts("2020-01-10"), 0),  # survives
        ("S", _ts("2020-01-10"), _ts("2021-01-05"), 1),  # overlap-purged
        ("S", _ts("2020-01-15"), _ts("2021-01-10"), 0),  # overlap-purged
        ("S", _ts("2020-01-20"), _ts("2021-01-15"), 1),  # overlap-purged
        ("S", _ts("2020-01-25"), _ts("2021-01-20"), 0),  # overlap-purged
    ]
    run_dir = tmp_path / "run"
    _controlled_fixture(run_dir, train_specs, test_start, test_end)

    # step_months=1 so a second, later fold (trained mostly on the dense "T"
    # filler rows) is also generated and satisfies the min-rows gate — the
    # run as a whole must still succeed even though fold 1 (asserted below)
    # is deliberately too small after purge.
    spec = WalkForwardSpec(train_years=1, test_months=2, step_months=1, holdout_months=1, min_rows_per_fold=5)
    out_path = run_walkforward_eval(run_dir, spec=spec, steps=5)
    out = json.loads(out_path.read_text(encoding="utf-8"))

    folds = [f for f in out["folds"] if f["train_start_utc"] == _ts("2020-01-01").isoformat()]
    assert len(folds) == 1
    f = folds[0]
    assert f["candidate_train_rows"] == 6
    assert f["purged_overlap_rows"] == 4
    assert f["effective_train_rows"] == 2
    assert f["skipped"] is True
    assert f["reason"] == "too_few_rows_after_purge"
    assert any(not fold["skipped"] for fold in out["folds"])


def test_multi_symbol_purge_membership_ignores_symbol_identity(tmp_path):
    """TEST 16 — two symbols share identical feature timestamps; purge outcome
    depends only on label_end_ts, not on symbol or row position."""
    test_start = _ts("2021-01-01")
    test_end = _ts("2021-03-01")
    train_specs = []
    shared_dates = [_ts("2020-01-01"), _ts("2020-01-05"), _ts("2020-01-10")]
    for d in shared_dates:
        # symbol A: label stays inside train -> survives
        train_specs.append(("A", d, d + pd.Timedelta(days=1), 1))
        # symbol B: label reaches into test -> purged, same feature_ts as A
        train_specs.append(("B", d, _ts("2021-01-15"), 0))

    run_dir = tmp_path / "run"
    _controlled_fixture(run_dir, train_specs, test_start, test_end)

    spec = WalkForwardSpec(train_years=1, test_months=2, step_months=6, holdout_months=1, min_rows_per_fold=1)
    out_path = run_walkforward_eval(run_dir, spec=spec, steps=5)
    out = json.loads(out_path.read_text(encoding="utf-8"))

    folds = [f for f in out["folds"] if f["train_start_utc"] == _ts("2020-01-01").isoformat()]
    assert len(folds) == 1
    f = folds[0]
    assert f["candidate_train_rows"] == 6
    assert f["purged_overlap_rows"] == 3
    assert f["effective_train_rows"] == 3


# ---------------------------------------------------------------------------
# Integration tests: standardization/holdout isolation (TESTS 8, 9, 10)
# ---------------------------------------------------------------------------

def _run_eval(run_dir: Path, df: pd.DataFrame, *, steps: int = 30):
    feats, targs = _split_feats_targs(df, with_label_end=True)
    _write_run_dir(run_dir, feats, targs)
    spec = WalkForwardSpec(**BASE_SPEC_KW)
    out_path = run_walkforward_eval(run_dir, spec=spec, steps=steps)
    return json.loads(out_path.read_text(encoding="utf-8"))


def _discovery_metrics(out):
    return [
        {"fold": f["fold"], "skipped": f["skipped"], "metrics": f.get("metrics")}
        for f in out["folds"]
    ]


def test_holdout_feature_magnitude_cannot_change_discovery_metrics(tmp_path):
    """TEST 8/10 — giant holdout feature values must not change discovery fold metrics
    (proves standardization/model fit uses effective training rows only)."""
    df = _build_multi_symbol_dataset(symbols=("AAA", "BBB"), periods_days=560)
    holdout_start = compute_holdout_boundary(
        pd.to_datetime(df["end_ts"]).min(), pd.to_datetime(df["end_ts"]).max(), BASE_SPEC_KW["holdout_months"]
    )[1]

    baseline = df.copy()
    out_baseline = _run_eval(tmp_path / "baseline", baseline)

    corrupted = df.copy()
    mask = pd.to_datetime(corrupted["end_ts"]) >= holdout_start
    corrupted.loc[mask, "f1"] = 1_000_000.0
    out_corrupted = _run_eval(tmp_path / "corrupted", corrupted)

    assert _discovery_metrics(out_baseline) == _discovery_metrics(out_corrupted)
    assert out_baseline["summary"] == out_corrupted["summary"]


def test_holdout_target_flip_cannot_change_discovery_metrics(tmp_path):
    """TEST 9"""
    df = _build_multi_symbol_dataset(symbols=("AAA", "BBB"), periods_days=560)
    holdout_start = compute_holdout_boundary(
        pd.to_datetime(df["end_ts"]).min(), pd.to_datetime(df["end_ts"]).max(), BASE_SPEC_KW["holdout_months"]
    )[1]

    baseline = df.copy()
    out_baseline = _run_eval(tmp_path / "baseline", baseline)

    flipped = df.copy()
    mask = pd.to_datetime(flipped["end_ts"]) >= holdout_start
    flipped.loc[mask, "target"] = 1 - flipped.loc[mask, "target"]
    out_flipped = _run_eval(tmp_path / "flipped", flipped)

    assert _discovery_metrics(out_baseline) == _discovery_metrics(out_flipped)
    assert out_baseline["summary"] == out_flipped["summary"]


# ---------------------------------------------------------------------------
# Integration tests: row order / multi-symbol invariance (TESTS 15, 16)
# ---------------------------------------------------------------------------

def test_row_order_invariance(tmp_path):
    """TEST 15 — shuffling input row order must not change split membership or metrics."""
    df = _build_multi_symbol_dataset(symbols=("AAA", "BBB"), periods_days=560)

    ordered_feats, ordered_targs = _split_feats_targs(df, with_label_end=True)
    run_a = tmp_path / "ordered"
    _write_run_dir(run_a, ordered_feats, ordered_targs)

    shuffled = df.sample(frac=1.0, random_state=42).reset_index(drop=True)
    shuffled_feats, shuffled_targs = _split_feats_targs(shuffled, with_label_end=True)
    run_b = tmp_path / "shuffled"
    _write_run_dir(run_b, shuffled_feats, shuffled_targs)

    spec = WalkForwardSpec(**BASE_SPEC_KW)
    out_a = json.loads(run_walkforward_eval(run_a, spec=spec, steps=20).read_text(encoding="utf-8"))
    out_b = json.loads(run_walkforward_eval(run_b, spec=spec, steps=20).read_text(encoding="utf-8"))

    assert _discovery_metrics(out_a) == _discovery_metrics(out_b)
    assert [
        {k: v for k, v in f.items() if k not in ("metrics",)} for f in out_a["folds"]
    ] == [
        {k: v for k, v in f.items() if k not in ("metrics",)} for f in out_b["folds"]
    ]
    assert out_a["summary"] == out_b["summary"]
    assert out_a["holdout"] == out_b["holdout"]


# ---------------------------------------------------------------------------
# Integration tests: holdout not scored, artifact proofs, determinism
# (TESTS 17, 18, 19, 20)
# ---------------------------------------------------------------------------

def test_holdout_not_scored(tmp_path):
    """TEST 17"""
    df = _build_multi_symbol_dataset(symbols=("AAA", "BBB", "CCC"), periods_days=560)
    out = _run_eval(tmp_path / "run", df)

    assert out["holdout"]["status"] == "reserved_not_evaluated"
    blob = json.dumps(out)
    assert "holdout_auc" not in blob
    assert "holdout_logloss" not in blob
    assert "holdout_predictions" not in blob


def test_artifact_proves_purge_boundary(tmp_path):
    """TEST 18"""
    df = _build_multi_symbol_dataset(symbols=("AAA", "BBB"), periods_days=560)
    out = _run_eval(tmp_path / "run", df)

    used = [f for f in out["folds"] if not f["skipped"]]
    assert len(used) >= 1
    for f in used:
        if f["max_used_train_label_end_utc"] is None:
            continue
        assert pd.Timestamp(f["max_used_train_label_end_utc"]) < pd.Timestamp(f["effective_train_cutoff_utc"])


def test_artifact_proves_holdout_isolation(tmp_path):
    """TEST 19"""
    df = _build_multi_symbol_dataset(symbols=("AAA", "BBB"), periods_days=560)
    out = _run_eval(tmp_path / "run", df)

    holdout_start = pd.Timestamp(out["temporal_contract"]["holdout_start_utc"])
    used = [f for f in out["folds"] if not f["skipped"]]
    assert len(used) >= 1
    for f in used:
        if f["max_used_train_label_end_utc"] is not None:
            assert pd.Timestamp(f["max_used_train_label_end_utc"]) < holdout_start


def test_deterministic_replay(tmp_path):
    """TEST 20 — same inputs + same spec twice => byte-identical JSON artifact."""
    df = _build_multi_symbol_dataset(symbols=("AAA", "BBB"), periods_days=560)
    feats, targs = _split_feats_targs(df, with_label_end=True)

    run_a = tmp_path / "run_a"
    _write_run_dir(run_a, feats, targs)
    run_b = tmp_path / "run_b"
    _write_run_dir(run_b, feats, targs)

    spec = WalkForwardSpec(**BASE_SPEC_KW)
    path_a = run_walkforward_eval(run_a, spec=spec, steps=20)
    path_b = run_walkforward_eval(run_b, spec=spec, steps=20)

    bytes_a = path_a.read_bytes()
    bytes_b = path_b.read_bytes()

    # inputs.*.path legitimately differs (different run_dir); strip before comparing.
    obj_a = json.loads(bytes_a)
    obj_b = json.loads(bytes_b)
    obj_a["inputs"] = None
    obj_b["inputs"] = None
    obj_a["ids"] = None
    obj_b["ids"] = None
    assert json.dumps(obj_a, sort_keys=True) == json.dumps(obj_b, sort_keys=True)

    # same run_dir twice must be truly byte-identical, including ids/inputs.
    path_a2 = run_walkforward_eval(run_a, spec=spec, steps=20)
    assert path_a2.read_bytes() == bytes_a


# ---------------------------------------------------------------------------
# WalkForwardSpec.normalized() validation
# ---------------------------------------------------------------------------

@pytest.mark.parametrize("kw", [
    dict(embargo_seconds=-1),
    dict(holdout_months=0),
    dict(holdout_months=-1),
    dict(train_years=0),
    dict(test_months=0),
    dict(step_months=0),
    dict(min_rows_per_fold=0),
])
def test_spec_normalized_rejects_invalid_values(kw):
    with pytest.raises(ValueError):
        WalkForwardSpec(**kw).normalized()


# ---------------------------------------------------------------------------
# Shadow labeler: label_end_ts provenance
# ---------------------------------------------------------------------------

def test_shadow_labeler_emits_truthful_label_end_ts(tmp_path):
    from mqk_research.shadow.label_shadow_intents import label_shadow_intents, LabelSpec

    bars = pd.DataFrame({
        "symbol": ["AAA"] * 10,
        "end_ts": pd.date_range("2021-01-01", periods=10, freq="D", tz="UTC"),
        "close": [100.0 + i for i in range(10)],
    })
    intents = pd.DataFrame({
        "run_id": ["r1"],
        "symbol": ["AAA"],
        "decision_ts": [bars["end_ts"].iloc[2]],
        "intent": ["long"],
        "horizon_bars": [3],
    })

    bars_csv = tmp_path / "bars.csv"
    intents_csv = tmp_path / "shadow_intents.csv"
    out_csv = tmp_path / "targets.csv"
    bars.to_csv(bars_csv, index=False)
    intents.to_csv(intents_csv, index=False)

    label_shadow_intents(shadow_intents_csv=intents_csv, bars_csv=bars_csv, out_targets_csv=out_csv, spec=LabelSpec())

    out = pd.read_csv(out_csv)
    assert "label_end_ts" in out.columns
    expected_label_end = bars["end_ts"].iloc[2 + 3]
    assert pd.Timestamp(out["label_end_ts"].iloc[0]) == expected_label_end
