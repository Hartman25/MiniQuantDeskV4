from __future__ import annotations

from pathlib import Path

import pytest

from mqk_research.data import alpaca_historical as ah
from test_alpaca_historical import (
    ASOF,
    BARS_URL,
    WINDOW_END,
    WINDOW_START,
    FakeHttp,
    _bar,
    _bars_page,
    _creds,
)


def fetch(row=None, *, payload=None):
    http = FakeHttp().queue(
        BARS_URL, 200, payload if payload is not None else _bars_page({"AAA": [row]})
    )
    return ah.fetch_historical_bars(
        symbols=["AAA"],
        start_utc=WINDOW_START,
        end_utc=WINDOW_END,
        asof=ASOF,
        credentials=_creds(),
        http_get=http,
    )


@pytest.mark.parametrize(
    "field,value",
    [
        ("v", None),
        ("v", -1),
        ("v", float("inf")),
        ("v", True),
        ("o", 0),
        ("o", -1),
        ("o", 102),
        ("h", 98),
        ("l", 102),
        ("c", 0),
        ("c", float("nan")),
        ("c", None),
        ("t", "2021-01-04T00:00:00"),
        ("t", None),
        ("t", 1609718400),
        ("is_complete", False),
    ],
)
def test_provider_invalid_fields_refused(field, value):
    row = _bar("2021-01-04T05:00:00Z")
    row[field] = value
    with pytest.raises(ah.AlpacaHistoricalExtractionError):
        fetch(row)


def test_provider_missing_volume_is_not_zero():
    row = _bar("2021-01-04T05:00:00Z")
    del row["v"]
    with pytest.raises(ah.AlpacaHistoricalExtractionError, match="volume|missing"):
        fetch(row)


def test_provider_unexpected_symbol_is_not_substituted():
    row = _bar("2021-01-04T05:00:00Z")
    with pytest.raises(ah.AlpacaHistoricalExtractionError, match="symbol"):
        fetch(payload=_bars_page({"AAA": [row], "BBB": [row]}))


@pytest.mark.parametrize(
    "payload",
    [[], {"bars": []}, {"bars": {"AAA": {"t": "bad"}}}, {"bars": {"AAA": [None]}}],
)
def test_provider_malformed_payload_typed_refusal(payload):
    with pytest.raises(ah.AlpacaHistoricalExtractionError):
        fetch(payload=payload)


@pytest.mark.parametrize("token", [False, 4, ["x"], {"token": "x"}])
def test_invalid_pagination_token_refused(token):
    with pytest.raises(ah.AlpacaHistoricalExtractionError, match="pagination"):
        fetch(payload=_bars_page({"AAA": [_bar("2021-01-04T05:00:00Z")]}, token))


def test_zero_volume_and_out_of_order_transport_are_valid():
    bars, _ = fetch(
        payload=_bars_page(
            {
                "AAA": [
                    _bar("2021-01-05T05:00:00Z", v=0),
                    _bar("2021-01-04T05:00:00Z"),
                ]
            }
        )
    )
    assert list(bars["end_ts"]) == [
        "2021-01-04T05:00:00+00:00",
        "2021-01-05T05:00:00+00:00",
    ]
    assert bars.iloc[1]["volume"] == 0


import copy
import pandas as pd

from mqk_research.data import bars_provenance as bp
from mqk_research.data import historical as hist
from mqk_research.ml.economic_walkforward import load_bars
from mqk_research.ml.util_hash import sha256_json
from test_alpaca_historical import CA_URL, _ca_page


def extraction(
    monkeypatch,
    *,
    days=("2024-11-27", "2024-11-29", "2024-12-02"),
    snapshot="2024-12-04T00:00:00Z",
    start="2024-11-27T00:00:00Z",
    end="2024-12-03T00:00:00Z",
    rows=None,
    timeframe="1Day",
    diagnostic=False,
):
    rows = (
        rows
        if rows is not None
        else [
            _bar(pd.Timestamp(d).tz_localize("America/New_York").isoformat())
            for d in days
        ]
    )
    http = (
        FakeHttp()
        .queue(BARS_URL, 200, _bars_page({"AAA": rows}))
        .queue(CA_URL, 200, _ca_page({}))
    )
    kwargs = dict(
        symbols=["AAA"],
        start_utc=pd.Timestamp(start),
        end_utc=pd.Timestamp(end),
        asof="2024-12-04",
        timeframe=timeframe,
        credentials=_creds(),
    )
    if diagnostic:
        return ah.extract_research_bars_with_provenance_diagnostic(
            **kwargs, http_get=http, ca_discovery_cutoff_utc=snapshot
        )
    # Software proof of the real official wrapper, with network and clock replaced.
    monkeypatch.setattr(ah, "_default_http_get", http)
    monkeypatch.setattr(ah, "_utc_now", lambda: pd.Timestamp(snapshot))
    return ah.extract_research_bars_with_provenance(**kwargs)


def qualified_load(tmp_path, result, **requirements):
    path = tmp_path / "bars.csv"
    result["bars"].to_csv(path, index=False)
    return load_bars(
        path,
        provenance_manifest=result["manifest"],
        historical_requirements={
            "decision_time_utc": "2024-12-05T00:00:00Z",
            **requirements,
        },
    )


def test_qualified_real_loader_holiday_early_close_and_boundary(monkeypatch, tmp_path):
    result = extraction(monkeypatch)
    loaded = qualified_load(tmp_path, result)
    report = loaded.attrs["historical_data_qualification"]
    assert report["expected_sessions_per_symbol"] == 3
    assert report["eligibility"] == "QUALIFIED_RETROSPECTIVE_SNAPSHOT"
    assert report["point_in_time_qualified"] is False
    assert report["publication_time"] is None and report["revision_vintage"] is None
    assert len(report["dataset_id"]) == 64
    assert (
        "2024-11-29" in loaded.iloc[1]["end_ts"].isoformat()
    )  # half-day is a full daily observation


@pytest.mark.parametrize(
    "requirement,reason",
    [
        ("require_point_in_time", "vintage unavailable"),
        ("require_pit_universe", "universe membership"),
        ("require_stable_instruments", "delisting"),
    ],
)
def test_unavailable_provider_capabilities_fail_closed(
    monkeypatch, tmp_path, requirement, reason
):
    with pytest.raises(hist.HistoricalDataUnqualified, match=reason):
        qualified_load(tmp_path, extraction(monkeypatch), **{requirement: True})


def test_future_revision_refused_at_earlier_decision(monkeypatch, tmp_path):
    result = extraction(monkeypatch)
    with pytest.raises(hist.HistoricalDataUnqualified, match="future-known"):
        qualified_load(tmp_path, result, decision_time_utc="2024-12-03T23:59:59Z")


def test_same_query_different_provider_revision_changes_identity(monkeypatch):
    first = extraction(monkeypatch)
    rows = [
        _bar(pd.Timestamp(d).tz_localize("America/New_York").isoformat(), v=2000)
        for d in ("2024-11-27", "2024-11-29", "2024-12-02")
    ]
    later = extraction(monkeypatch, rows=rows, snapshot="2024-12-05T00:00:00Z")
    assert (
        first["manifest"]["canonical_semantic_bars_hash"]
        == later["manifest"]["canonical_semantic_bars_hash"]
    )
    assert bp.provenance_identity_fragment(
        first["manifest"]
    ) != bp.provenance_identity_fragment(later["manifest"])


def test_layout_and_snapshot_clock_do_not_manufacture_identity(monkeypatch):
    first = extraction(monkeypatch)
    later = extraction(monkeypatch, snapshot="2024-12-05T00:00:00Z")
    reordered = later["bars"].iloc[::-1][list(reversed(later["bars"].columns))]
    assert (
        hist.full_ohlcv_hash(reordered)
        == first["manifest"]["historical_data_contract"]["full_ohlcv_sha256"]
    )
    assert bp.provenance_identity_fragment(
        first["manifest"]
    ) == bp.provenance_identity_fragment(later["manifest"])


@pytest.mark.parametrize(
    "field",
    ["quality_policy", "normalization_version", "timestamp_meaning", "mapping_asof"],
)
def test_semantic_contract_policy_changes_identity_and_refuses_unknown_policy(
    monkeypatch, field
):
    result = extraction(monkeypatch)
    changed = copy.deepcopy(result["manifest"])
    changed["historical_data_contract"][field] = "unsupported_policy"
    changed["historical_data_contract_id"] = sha256_json(
        changed["historical_data_contract"]
    )
    assert bp.provenance_identity_fragment(changed) != bp.provenance_identity_fragment(
        result["manifest"]
    )
    with pytest.raises(hist.HistoricalDataUnqualified, match="mismatch"):
        bp.require_bars_match_manifest(result["bars"], changed)


@pytest.mark.parametrize(
    "column,value",
    [("open", 100.1), ("volume", 1001), ("volume", -1), ("high", float("inf"))],
)
def test_full_ohlcv_content_gate_cannot_be_bypassed_by_close_only_hash(
    monkeypatch, column, value
):
    result = extraction(monkeypatch)
    changed = result["bars"].copy()
    changed.loc[0, column] = value
    assert (
        bp.canonical_semantic_bars_hash(changed)
        == result["manifest"]["canonical_semantic_bars_hash"]
    )
    with pytest.raises(hist.HistoricalDataUnqualified):
        bp.require_bars_match_manifest(changed, result["manifest"])


def test_stripping_v3_contract_refused(monkeypatch):
    result = extraction(monkeypatch)
    del result["manifest"]["historical_data_contract"]
    del result["manifest"]["historical_data_contract_id"]
    with pytest.raises(hist.HistoricalDataUnqualified, match="missing"):
        bp.require_bars_match_manifest(result["bars"], result["manifest"])


@pytest.mark.parametrize(
    "days",
    [
        ("2024-11-27", "2024-12-02"),  # missing early close
        ("2024-11-27", "2024-11-28", "2024-11-29", "2024-12-02"),  # holiday row
    ],
)
def test_missing_or_extra_expected_session_refused(monkeypatch, tmp_path, days):
    with pytest.raises(hist.HistoricalDataUnqualified, match="session coverage"):
        qualified_load(tmp_path, extraction(monkeypatch, days=days))


def test_wrong_daily_timezone_refused_by_qualified_loader(monkeypatch, tmp_path):
    rows = [_bar(f"{d}T00:00:00Z") for d in ("2024-11-27", "2024-11-29", "2024-12-02")]
    with pytest.raises(hist.HistoricalDataUnqualified, match="midnight"):
        qualified_load(tmp_path, extraction(monkeypatch, rows=rows))


def test_partial_daily_session_refused(monkeypatch, tmp_path):
    result = extraction(
        monkeypatch,
        days=("2024-11-29",),
        start="2024-11-29T00:00:00Z",
        end="2024-11-29T17:00:00Z",
        snapshot="2024-11-29T17:00:00Z",
    )
    with pytest.raises(hist.HistoricalDataUnqualified, match="partial session"):
        qualified_load(tmp_path, result)


def test_unsupported_resolution_refused(monkeypatch, tmp_path):
    with pytest.raises(hist.HistoricalDataUnqualified, match="intraday"):
        qualified_load(tmp_path, extraction(monkeypatch, timeframe="1Hour"))


def test_synthetic_cannot_be_qualified_market_evidence(monkeypatch, tmp_path):
    with pytest.raises(
        bp.SourceAttestationUnverifiable, match="trusted research extractor"
    ):
        qualified_load(tmp_path, extraction(monkeypatch, diagnostic=True))


def test_registered_consumer_refuses_pit_before_registry_or_evaluation(
    monkeypatch, tmp_path
):
    from mqk_research.ml.economic_registry_integration import (
        run_registered_economic_walkforward_eval,
    )

    result = extraction(monkeypatch)
    bars = tmp_path / "bars.csv"
    result["bars"].to_csv(bars, index=False)
    with pytest.raises(hist.HistoricalDataUnqualified, match="vintage unavailable"):
        run_registered_economic_walkforward_eval(
            tmp_path / "run",
            experiment_id="e",
            hypothesis_id="h",
            strategy_id="s",
            bars_csv=bars,
            economic_spec=object(),
            bars_provenance=result["manifest"],
            historical_requirements={
                "decision_time_utc": "2024-12-05T00:00:00Z",
                "require_point_in_time": True,
            },
        )
    assert not (tmp_path / "run").exists()


def test_legacy_loader_rejects_infinite_close(tmp_path):
    path = tmp_path / "bad.csv"
    pd.DataFrame(
        {"symbol": ["AAA"], "end_ts": ["2024-01-02T05:00:00Z"], "close": [float("inf")]}
    ).to_csv(path, index=False)
    with pytest.raises(RuntimeError, match="non-finite"):
        load_bars(path)


@pytest.mark.parametrize(
    "field,value",
    [("price_adjustment_convention", None), ("corporate_action_policy", None)],
)
def test_missing_adjustment_metadata_refused(monkeypatch, tmp_path, field, value):
    result = extraction(monkeypatch)
    result["manifest"][field] = value
    with pytest.raises((bp.BarsProvenanceUnverifiable, hist.HistoricalDataUnqualified)):
        qualified_load(tmp_path, result)


def test_provider_split_adjustment_applied_exactly_once(monkeypatch):
    row = _bar("2024-11-27T05:00:00Z", o=50, h=51, l=49, c=50, v=2000)
    http = (
        FakeHttp()
        .queue(BARS_URL, 200, _bars_page({"AAA": [row]}))
        .queue(
            CA_URL,
            200,
            _ca_page(
                {
                    "forward_splits": [
                        {
                            "symbol": "AAA",
                            "ex_date": "2024-11-27",
                            "process_date": "2024-11-27",
                            "old_rate": 1,
                            "new_rate": 2,
                        }
                    ]
                }
            ),
        )
    )
    monkeypatch.setattr(ah, "_default_http_get", http)
    monkeypatch.setattr(ah, "_utc_now", lambda: pd.Timestamp("2024-12-04T00:00:00Z"))
    result = ah.extract_research_bars_with_provenance(
        symbols=["AAA"],
        start_utc=pd.Timestamp("2024-11-27T00:00:00Z"),
        end_utc=pd.Timestamp("2024-11-28T00:00:00Z"),
        asof="2024-12-04",
        credentials=_creds(),
    )
    assert result["bars"].iloc[0]["close"] == 50
    assert result["bars"].iloc[0]["volume"] == 2000
    assert http.calls[0]["params"]["adjustment"] == "all"
    assert len(result["corporate_action_entries"]) == 1
    bp.check_corporate_action_integrity(result["bars"], result["manifest"])


def test_calendar_dst_uses_local_midnight_not_fixed_offset(monkeypatch, tmp_path):
    result = extraction(
        monkeypatch,
        days=("2024-03-08", "2024-03-11"),
        start="2024-03-08T00:00:00Z",
        end="2024-03-12T00:00:00Z",
    )
    loaded = qualified_load(tmp_path, result)
    assert loaded["end_ts"].dt.hour.tolist() == [5, 4]


def test_exceptional_full_market_closure_is_not_a_missing_session(
    monkeypatch, tmp_path
):
    result = extraction(
        monkeypatch,
        days=("2025-01-08", "2025-01-10"),
        start="2025-01-08T00:00:00Z",
        end="2025-01-11T00:00:00Z",
        snapshot="2025-01-12T00:00:00Z",
    )
    loaded = qualified_load(tmp_path, result, decision_time_utc="2025-01-13T00:00:00Z")
    assert len(loaded) == 2


def test_calendar_out_of_coverage_fails_closed(monkeypatch, tmp_path):
    result = extraction(
        monkeypatch,
        days=("2015-12-30",),
        start="2015-12-30T00:00:00Z",
        end="2015-12-31T00:00:00Z",
    )
    with pytest.raises(hist.HistoricalDataUnqualified, match="coverage"):
        qualified_load(tmp_path, result)


def test_legacy_manifest_does_not_silently_acquire_historical_qualification(tmp_path):
    from test_bars_provenance import _base_manifest, _bars_df

    bars = _bars_df([100, 101])
    manifest = _base_manifest(bars)
    assert "historical_data_contract_id" not in bp.provenance_identity_fragment(
        manifest
    )
    path = tmp_path / "legacy.csv"
    bars.to_csv(path, index=False)
    with pytest.raises(hist.HistoricalDataUnqualified, match="legacy"):
        load_bars(
            path,
            provenance_manifest=manifest,
            historical_requirements={"decision_time_utc": "2024-12-05T00:00:00Z"},
        )


def test_naive_csv_timestamp_cannot_be_hidden_by_legacy_utc_conversion(
    monkeypatch, tmp_path
):
    result = extraction(monkeypatch)
    result["bars"]["end_ts"] = result["bars"]["end_ts"].str.replace(
        "+00:00", "", regex=False
    )
    with pytest.raises(hist.HistoricalDataUnqualified, match="timezone"):
        qualified_load(tmp_path, result)


def test_repeat_publication_idempotent_and_revision_never_overwrites(
    monkeypatch, tmp_path
):
    result = extraction(monkeypatch)
    target = tmp_path / "dataset"
    paths = ah.write_research_extraction_artifacts(target, result)
    before = {
        key: (path.read_bytes(), path.stat().st_mtime_ns) for key, path in paths.items()
    }
    assert ah.write_research_extraction_artifacts(target, result) == paths
    assert before == {
        key: (path.read_bytes(), path.stat().st_mtime_ns) for key, path in paths.items()
    }
    revised = extraction(
        monkeypatch,
        rows=[
            _bar(pd.Timestamp(d).tz_localize("America/New_York").isoformat(), v=2000)
            for d in ("2024-11-27", "2024-11-29", "2024-12-02")
        ],
    )
    with pytest.raises(
        ah.AlpacaHistoricalExtractionError, match="different|revision|immutable"
    ):
        ah.write_research_extraction_artifacts(target, revised)
    assert before == {
        key: (path.read_bytes(), path.stat().st_mtime_ns) for key, path in paths.items()
    }


def test_failed_publish_has_no_visible_dataset_and_retry_recovers(
    monkeypatch, tmp_path
):
    result = extraction(monkeypatch)
    target = tmp_path / "dataset"
    original = Path.write_text

    def crash(path, *args, **kwargs):
        if path.name == "corporate_actions.json":
            raise OSError("injected crash before CA artifact")
        return original(path, *args, **kwargs)

    with monkeypatch.context() as m:
        m.setattr(Path, "write_text", crash)
        with pytest.raises(OSError, match="injected crash"):
            ah.write_research_extraction_artifacts(target, result)
    assert not target.exists()
    paths = ah.write_research_extraction_artifacts(target, result)
    assert all(path.is_file() for path in paths.values())


def test_corrupt_existing_dataset_refuses_reuse(monkeypatch, tmp_path):
    result = extraction(monkeypatch)
    target = tmp_path / "dataset"
    paths = ah.write_research_extraction_artifacts(target, result)
    paths["bars_csv"].write_text(
        paths["bars_csv"].read_text().replace("100.5", "100.6")
    )
    with pytest.raises(
        (
            ah.AlpacaHistoricalExtractionError,
            bp.BarsProvenanceContentMismatch,
            hist.HistoricalDataUnqualified,
        )
    ):
        ah.write_research_extraction_artifacts(target, result)


@pytest.mark.parametrize(
    "artifact",
    [
        "bars_provenance_json",
        "corporate_actions_json",
        "corporate_actions_provenance_json",
        "bars_csv",
    ],
)
def test_corrupt_artifact_set_refused_without_repair(monkeypatch, tmp_path, artifact):
    result = extraction(monkeypatch)
    target = tmp_path / "dataset"
    paths = ah.write_research_extraction_artifacts(target, result)
    paths[artifact].write_bytes(paths[artifact].read_bytes() + b"corrupt")
    corrupt = paths[artifact].read_bytes()
    with pytest.raises(
        ah.AlpacaHistoricalExtractionError, match="checksum|invalid|integrity"
    ):
        ah.load_research_extraction_artifacts(target)
    with pytest.raises(ah.AlpacaHistoricalExtractionError):
        ah.write_research_extraction_artifacts(target, result)
    assert paths[artifact].read_bytes() == corrupt


def test_equivalent_layout_daily_alias_and_retrieval_preserve_publication(
    monkeypatch, tmp_path
):
    first = extraction(monkeypatch)
    later = extraction(monkeypatch, timeframe="1D", snapshot="2024-12-05T00:00:00Z")
    later["bars"] = later["bars"].iloc[::-1][list(reversed(later["bars"].columns))]
    assert bp.provenance_identity_fragment(
        first["manifest"]
    ) == bp.provenance_identity_fragment(later["manifest"])
    target = tmp_path / "dataset"
    paths = ah.write_research_extraction_artifacts(target, first)
    before = {k: p.read_bytes() for k, p in paths.items()}
    ah.write_research_extraction_artifacts(target, later)
    assert before == {k: p.read_bytes() for k, p in paths.items()}
    loaded = ah.load_research_extraction_artifacts(
        target, historical_requirements={"decision_time_utc": "2024-12-06T00:00:00Z"}
    )
    assert loaded["qualification"]["point_in_time_qualified"] is False
    assert loaded["qualification"]["expected_sessions_per_symbol"] == 3


def test_rename_failure_never_publishes_partial_and_retry_recovers(
    monkeypatch, tmp_path
):
    result = extraction(monkeypatch)
    target = tmp_path / "dataset"

    def crash(*args):
        raise OSError("injected rename crash")

    with monkeypatch.context() as m:
        m.setattr(ah.os, "rename", crash)
        with pytest.raises(OSError, match="rename crash"):
            ah.write_research_extraction_artifacts(target, result)
    assert not target.exists()
    assert (tmp_path / ".dataset.staging").is_dir()
    ah.write_research_extraction_artifacts(target, result)
    assert ah.load_research_extraction_artifacts(target)["manifest"]["row_count"] == 3


def test_real_process_crash_releases_lock_and_retry_recovers(monkeypatch, tmp_path):
    import json
    import subprocess
    import sys

    result = extraction(monkeypatch)
    payload = {**result, "bars": result["bars"].to_dict("records")}
    source = tmp_path / "offline_result.json"
    source.write_text(json.dumps(payload), encoding="utf-8")
    target = tmp_path / "dataset"
    script = r"""
import json, os, sys
from pathlib import Path
import pandas as pd
from mqk_research.data.alpaca_historical import write_research_extraction_artifacts
result = json.loads(Path(sys.argv[1]).read_text(encoding="utf-8"))
result["bars"] = pd.DataFrame(result["bars"])
original = Path.write_text
def crash(path, *args, **kwargs):
    value = original(path, *args, **kwargs)
    if path.name == "corporate_actions_provenance.json":
        os._exit(17)
    return value
Path.write_text = crash
write_research_extraction_artifacts(Path(sys.argv[2]), result)
"""
    completed = subprocess.run(
        [sys.executable, "-c", script, str(source), str(target)],
        capture_output=True,
        timeout=30,
    )
    assert completed.returncode == 17, completed.stderr.decode()
    assert not target.exists()
    assert (tmp_path / ".dataset.staging").is_dir()
    ah.write_research_extraction_artifacts(target, result)
    assert ah.load_research_extraction_artifacts(target)["manifest"]["row_count"] == 3


def test_publication_lock_contention_refuses_and_retries(monkeypatch, tmp_path):
    result = extraction(monkeypatch)
    target = tmp_path / "dataset"
    with ah._extraction_publish_lock(target):
        with pytest.raises(ah.AlpacaHistoricalExtractionError, match="busy"):
            ah.write_research_extraction_artifacts(target, result)
    assert not target.exists()
    assert ah.write_research_extraction_artifacts(target, result)["bars_csv"].exists()


def test_float_roundtrip_preserves_exact_content_identity(monkeypatch, tmp_path):
    rows = [
        _bar(
            "2024-11-27T05:00:00Z",
            o=100.12345678901235,
            h=101.12345678901235,
            l=99.12345678901235,
            c=100.12345678901235,
            v=123.12345678901235,
        )
    ]
    result = extraction(
        monkeypatch, rows=rows, start="2024-11-27T00:00:00Z", end="2024-11-28T00:00:00Z"
    )
    paths = ah.write_research_extraction_artifacts(tmp_path / "dataset", result)
    loaded = load_bars(
        paths["bars_csv"],
        provenance_manifest=result["manifest"],
        historical_requirements={"decision_time_utc": "2024-12-05T00:00:00Z"},
    )
    assert hist.full_ohlcv_hash(loaded) == hist.full_ohlcv_hash(result["bars"])


def test_cli_revision_identity_selects_new_destination(monkeypatch, tmp_path):
    from mqk_research.cli import run_alpaca_research_extraction

    first = extraction(monkeypatch)
    later = extraction(
        monkeypatch,
        rows=[
            _bar(pd.Timestamp(d).tz_localize("America/New_York").isoformat(), v=2000)
            for d in ("2024-11-27", "2024-11-29", "2024-12-02")
        ],
    )

    def run(result):
        monkeypatch.setattr(
            ah, "extract_research_bars_with_provenance", lambda **kwargs: result
        )
        return run_alpaca_research_extraction(
            symbols_csv="AAA",
            start_utc=pd.Timestamp("2024-11-27T00:00:00Z"),
            end_utc=pd.Timestamp("2024-12-03T00:00:00Z"),
            timeframe="1Day",
            asof="2024-12-04",
            out_root=tmp_path,
        )

    a, repeated, b = run(first), run(first), run(later)
    assert a == repeated and a != b
    assert (
        ah.load_research_extraction_artifacts(a)["bars"]["volume"].tolist()
        == [1000] * 3
    )
    assert (
        ah.load_research_extraction_artifacts(b)["bars"]["volume"].tolist()
        == [2000] * 3
    )
