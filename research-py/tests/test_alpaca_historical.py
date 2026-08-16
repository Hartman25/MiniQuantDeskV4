"""
BKT-RESEARCH-MARKET-DATA-AUTHORITY-01 -- tests for the dedicated Alpaca
research historical-data extractor (mqk_research.data.alpaca_historical).

No real network access is used anywhere in this file -- every test injects a
fake `http_get` callable. Covers the mission's REQUIRED TESTS items that
pertain to this module:
  1.  requests the exact verified historical adjustment mode (all)
  2.  pagination is complete
  3.  provider response row order does not alter canonical semantic identity
  4.  duplicate bars fail closed
  5.  exact symbol universe recorded
  6.  exact request range/timeframe recorded
  7.  provider identity recorded
  8.  adjustment mode recorded
  9.  corporate-action request covers same symbol/range contract
  10. CA pagination complete
  14. altered bars after provenance creation fail before P&L
  17-19. ordinary split/dividend/spin-off covered by adjustment policy
  20. unsupported/complex CA type fails closed
  21. raw pre/post split gap cannot bypass CA safety by removing the bar
  22. provider API error fails closed
  23. incomplete pagination fails closed
  24. malformed response fails closed
  25. non-finite price fails closed
  27. no secrets appear in provenance artifacts
"""
from __future__ import annotations

import json
from pathlib import Path
from typing import Any, Dict, List, Optional, Tuple

import pandas as pd
import pytest

from mqk_research.data import alpaca_historical as ah
from mqk_research.data.bars_provenance import (
    canonical_semantic_bars_hash,
    check_corporate_action_integrity,
    require_bars_match_manifest,
    require_registered_bars_provenance,
)


# ---------------------------------------------------------------------------
# Fake HTTP transport -- records every call, serves queued responses.
# ---------------------------------------------------------------------------


class FakeHttp:
    def __init__(self) -> None:
        self.calls: List[Dict[str, Any]] = []
        self._responses: Dict[str, List[Tuple[int, Any]]] = {}

    def queue(self, endpoint_url: str, status: int, payload: Any) -> "FakeHttp":
        self._responses.setdefault(endpoint_url, []).append((status, payload))
        return self

    def __call__(self, url: str, params: Dict[str, str], headers: Dict[str, str]) -> Tuple[int, bytes]:
        self.calls.append({"url": url, "params": dict(params), "headers": dict(headers)})
        queue = self._responses.get(url)
        if not queue:
            raise AssertionError(f"FakeHttp: no queued response for {url} (call #{len(self.calls)})")
        status, payload = queue.pop(0)
        if isinstance(payload, (bytes, bytearray)):
            body = bytes(payload)
        else:
            body = json.dumps(payload, allow_nan=True).encode("utf-8")
        return status, body


BARS_URL = f"{ah.ALPACA_DATA_BASE_URL}{ah.BARS_PATH}"
CA_URL = f"{ah.ALPACA_DATA_BASE_URL}{ah.CORPORATE_ACTIONS_PATH}"


def _bar(t: str, o: float = 100.0, h: float = 101.0, l: float = 99.0, c: float = 100.5, v: float = 1000.0) -> Dict[str, Any]:
    return {"t": t, "o": o, "h": h, "l": l, "c": c, "v": v}


def _bars_page(bars_by_symbol: Dict[str, List[Dict[str, Any]]], next_token: Optional[str] = None) -> Dict[str, Any]:
    return {"bars": bars_by_symbol, "next_page_token": next_token}


def _ca_page(corporate_actions: Dict[str, List[Dict[str, Any]]], next_token: Optional[str] = None) -> Dict[str, Any]:
    return {"corporate_actions": corporate_actions, "next_page_token": next_token}


def _creds() -> ah.AlpacaCredentials:
    return ah.AlpacaCredentials(api_key="test-key", api_secret="test-secret")


def _queue_empty_ca(http: FakeHttp) -> None:
    http.queue(CA_URL, 200, _ca_page({}))


WINDOW_START = pd.Timestamp("2021-01-01T00:00:00Z")
WINDOW_END = pd.Timestamp("2021-02-01T00:00:00Z")


# ---------------------------------------------------------------------------
# fetch_historical_bars
# ---------------------------------------------------------------------------


def test_fetch_bars_requests_adjustment_all():
    http = FakeHttp()
    http.queue(BARS_URL, 200, _bars_page({"AAA": [_bar("2021-01-04T00:00:00Z")]}))
    df, meta = ah.fetch_historical_bars(
        symbols=["AAA"], start_utc=WINDOW_START, end_utc=WINDOW_END, credentials=_creds(), http_get=http
    )
    assert http.calls[0]["params"]["adjustment"] == "all"
    assert meta["resolved_adjustment"] == "all"


def test_fetch_bars_pagination_complete_across_multiple_pages():
    http = FakeHttp()
    http.queue(BARS_URL, 200, _bars_page({"AAA": [_bar("2021-01-04T00:00:00Z")]}, next_token="tok1"))
    http.queue(BARS_URL, 200, _bars_page({"AAA": [_bar("2021-01-05T00:00:00Z")]}))
    df, meta = ah.fetch_historical_bars(
        symbols=["AAA"], start_utc=WINDOW_START, end_utc=WINDOW_END, credentials=_creds(), http_get=http
    )
    assert len(http.calls) == 2
    assert http.calls[1]["params"]["page_token"] == "tok1"
    assert meta["pagination_complete"] is True
    assert len(df) == 2


def test_fetch_bars_response_row_order_does_not_alter_canonical_hash():
    http_a = FakeHttp()
    http_a.queue(
        BARS_URL,
        200,
        _bars_page({"AAA": [_bar("2021-01-04T00:00:00Z", c=100.0), _bar("2021-01-05T00:00:00Z", c=101.0)]}),
    )
    df_a, _ = ah.fetch_historical_bars(
        symbols=["AAA"], start_utc=WINDOW_START, end_utc=WINDOW_END, credentials=_creds(), http_get=http_a
    )

    http_b = FakeHttp()
    http_b.queue(
        BARS_URL,
        200,
        _bars_page({"AAA": [_bar("2021-01-05T00:00:00Z", c=101.0), _bar("2021-01-04T00:00:00Z", c=100.0)]}),
    )
    df_b, _ = ah.fetch_historical_bars(
        symbols=["AAA"], start_utc=WINDOW_START, end_utc=WINDOW_END, credentials=_creds(), http_get=http_b
    )

    assert canonical_semantic_bars_hash(df_a) == canonical_semantic_bars_hash(df_b)


def test_fetch_bars_duplicate_symbol_end_ts_fails_closed():
    http = FakeHttp()
    http.queue(
        BARS_URL,
        200,
        _bars_page({"AAA": [_bar("2021-01-04T00:00:00Z", c=100.0), _bar("2021-01-04T00:00:00Z", c=100.5)]}),
    )
    with pytest.raises(ah.AlpacaHistoricalExtractionError, match="duplicate"):
        ah.fetch_historical_bars(
            symbols=["AAA"], start_utc=WINDOW_START, end_utc=WINDOW_END, credentials=_creds(), http_get=http
        )


def test_fetch_bars_non_finite_price_fails_closed():
    http = FakeHttp()
    http.queue(BARS_URL, 200, _bars_page({"AAA": [_bar("2021-01-04T00:00:00Z", c=float("nan"))]}))
    with pytest.raises(ah.AlpacaHistoricalExtractionError, match="non-finite"):
        ah.fetch_historical_bars(
            symbols=["AAA"], start_utc=WINDOW_START, end_utc=WINDOW_END, credentials=_creds(), http_get=http
        )


def test_fetch_bars_provider_http_error_fails_closed():
    http = FakeHttp()
    http.queue(BARS_URL, 500, {"message": "internal error"})
    with pytest.raises(ah.AlpacaHistoricalExtractionError, match="status=500"):
        ah.fetch_historical_bars(
            symbols=["AAA"], start_utc=WINDOW_START, end_utc=WINDOW_END, credentials=_creds(), http_get=http
        )


def test_fetch_bars_malformed_json_fails_closed():
    http = FakeHttp()
    http.queue(BARS_URL, 200, b"{not json")
    with pytest.raises(ah.AlpacaHistoricalExtractionError, match="JSON decode"):
        ah.fetch_historical_bars(
            symbols=["AAA"], start_utc=WINDOW_START, end_utc=WINDOW_END, credentials=_creds(), http_get=http
        )


def test_fetch_bars_missing_bars_key_fails_closed():
    http = FakeHttp()
    http.queue(BARS_URL, 200, {"next_page_token": None})
    with pytest.raises(ah.AlpacaHistoricalExtractionError, match="missing required 'bars'"):
        ah.fetch_historical_bars(
            symbols=["AAA"], start_utc=WINDOW_START, end_utc=WINDOW_END, credentials=_creds(), http_get=http
        )


def test_fetch_all_pages_incomplete_pagination_fails_closed():
    http = FakeHttp()
    for _ in range(5):
        http.queue(BARS_URL, 200, _bars_page({"AAA": [_bar("2021-01-04T00:00:00Z")]}, next_token="always-more"))
    with pytest.raises(ah.AlpacaHistoricalExtractionError, match="did not terminate"):
        ah._fetch_all_pages(
            endpoint_url=BARS_URL, base_params={"symbols": "AAA"}, credentials=_creds(), http_get=http, max_pages=3
        )


def test_fetch_bars_missing_symbol_in_response_fails_closed():
    http = FakeHttp()
    http.queue(BARS_URL, 200, _bars_page({"AAA": [_bar("2021-01-04T00:00:00Z")]}))
    with pytest.raises(ah.AlpacaHistoricalExtractionError, match="BBB"):
        ah.fetch_historical_bars(
            symbols=["AAA", "BBB"], start_utc=WINDOW_START, end_utc=WINDOW_END, credentials=_creds(), http_get=http
        )


# ---------------------------------------------------------------------------
# fetch_corporate_actions
# ---------------------------------------------------------------------------


def test_fetch_corporate_actions_covers_same_symbol_range_contract():
    http = FakeHttp()
    http.queue(CA_URL, 200, _ca_page({}))
    entries, meta = ah.fetch_corporate_actions(
        symbols=["AAA", "BBB"], start_utc=WINDOW_START, end_utc=WINDOW_END, credentials=_creds(), http_get=http
    )
    params = http.calls[0]["params"]
    assert params["symbols"] == "AAA,BBB"
    assert params["start"] == "2021-01-01"
    assert params["end"] == "2021-02-01"
    assert meta["symbols_requested"] == ["AAA", "BBB"]


def test_fetch_corporate_actions_pagination_complete():
    http = FakeHttp()
    http.queue(
        CA_URL,
        200,
        _ca_page(
            {"cash_dividends": [{"id": "1", "symbol": "AAA", "cusip": "x", "rate": 0.1, "process_date": "2021-01-10", "ex_date": "2021-01-09"}]},
            next_token="tok1",
        ),
    )
    http.queue(
        CA_URL,
        200,
        _ca_page(
            {"cash_dividends": [{"id": "2", "symbol": "AAA", "cusip": "x", "rate": 0.2, "process_date": "2021-01-20", "ex_date": "2021-01-19"}]}
        ),
    )
    entries, meta = ah.fetch_corporate_actions(
        symbols=["AAA"], start_utc=WINDOW_START, end_utc=WINDOW_END, credentials=_creds(), http_get=http
    )
    assert meta["pagination_complete"] is True
    assert len(entries) == 2


def test_fetch_corporate_actions_missing_key_fails_closed():
    http = FakeHttp()
    http.queue(CA_URL, 200, {"next_page_token": None})
    with pytest.raises(ah.AlpacaHistoricalExtractionError, match="missing required 'corporate_actions'"):
        ah.fetch_corporate_actions(
            symbols=["AAA"], start_utc=WINDOW_START, end_utc=WINDOW_END, credentials=_creds(), http_get=http
        )


def test_fetch_corporate_actions_unmapped_response_bucket_fails_closed():
    """A future Alpaca response bucket this module's vocabulary does not yet
    recognize must fail closed, never be silently skipped."""
    http = FakeHttp()
    http.queue(CA_URL, 200, _ca_page({"some_new_bucket_type": [{"id": "1"}]}))
    with pytest.raises(ah.AlpacaHistoricalExtractionError, match="unmapped bucket"):
        ah.fetch_corporate_actions(
            symbols=["AAA"], start_utc=WINDOW_START, end_utc=WINDOW_END, credentials=_creds(), http_get=http
        )


def test_fetch_corporate_actions_entry_missing_process_date_fails_closed():
    http = FakeHttp()
    http.queue(
        CA_URL,
        200,
        _ca_page({"cash_dividends": [{"id": "1", "symbol": "AAA", "cusip": "x", "rate": 0.1}]}),  # no process_date
    )
    with pytest.raises(ah.AlpacaHistoricalExtractionError, match="process_date"):
        ah.fetch_corporate_actions(
            symbols=["AAA"], start_utc=WINDOW_START, end_utc=WINDOW_END, credentials=_creds(), http_get=http
        )


def test_fetch_corporate_actions_unknown_requested_type_rejected():
    http = FakeHttp()
    with pytest.raises(ValueError, match="Unknown corporate action"):
        ah.fetch_corporate_actions(
            symbols=["AAA"],
            start_utc=WINDOW_START,
            end_utc=WINDOW_END,
            types=["not_a_real_type"],
            credentials=_creds(),
            http_get=http,
        )


def test_fetch_corporate_actions_multi_leg_merger_produces_both_symbols():
    http = FakeHttp()
    http.queue(
        CA_URL,
        200,
        _ca_page(
            {
                "stock_mergers": [
                    {
                        "id": "m1",
                        "acquirer_symbol": "BIG",
                        "acquirer_cusip": "x",
                        "acquirer_rate": 1.0,
                        "acquiree_symbol": "SML",
                        "acquiree_cusip": "y",
                        "acquiree_rate": 2.0,
                        "process_date": "2021-01-15",
                        "effective_date": "2021-01-15",
                    }
                ]
            }
        ),
    )
    entries, _ = ah.fetch_corporate_actions(
        symbols=["BIG", "SML"], start_utc=WINDOW_START, end_utc=WINDOW_END, credentials=_creds(), http_get=http
    )
    symbols = {e["symbol"] for e in entries}
    assert symbols == {"BIG", "SML"}
    assert all(e["action_type"] == "stock_merger" for e in entries)


# ---------------------------------------------------------------------------
# classify_corporate_action_type / find_requires_review_events
# ---------------------------------------------------------------------------


@pytest.mark.parametrize(
    "action_type,expected",
    [
        ("forward_split", ah.CATEGORY_COVERED_BY_ADJUSTMENT),
        ("reverse_split", ah.CATEGORY_COVERED_BY_ADJUSTMENT),
        ("cash_dividend", ah.CATEGORY_COVERED_BY_ADJUSTMENT),
        ("spin_off", ah.CATEGORY_COVERED_BY_ADJUSTMENT),
        ("unit_split", ah.CATEGORY_REQUIRES_FAIL_CLOSED_REVIEW),
        ("stock_dividend", ah.CATEGORY_REQUIRES_FAIL_CLOSED_REVIEW),
        ("cash_merger", ah.CATEGORY_REQUIRES_FAIL_CLOSED_REVIEW),
        ("stock_merger", ah.CATEGORY_REQUIRES_FAIL_CLOSED_REVIEW),
        ("stock_and_cash_merger", ah.CATEGORY_REQUIRES_FAIL_CLOSED_REVIEW),
        ("redemption", ah.CATEGORY_REQUIRES_FAIL_CLOSED_REVIEW),
        ("name_change", ah.CATEGORY_REQUIRES_FAIL_CLOSED_REVIEW),
        ("worthless_removal", ah.CATEGORY_REQUIRES_FAIL_CLOSED_REVIEW),
        ("rights_distribution", ah.CATEGORY_REQUIRES_FAIL_CLOSED_REVIEW),
        ("partial_call", ah.CATEGORY_REQUIRES_FAIL_CLOSED_REVIEW),
        ("reorganization", ah.CATEGORY_REQUIRES_FAIL_CLOSED_REVIEW),
    ],
)
def test_classify_corporate_action_type(action_type, expected):
    assert ah.classify_corporate_action_type(action_type) == expected


def test_find_requires_review_events_ignores_covered_types():
    entries = [
        {"symbol": "AAA", "action_type": "forward_split", "effective_start_ts": "2021-01-15", "effective_end_ts": "2021-01-15"},
        {"symbol": "AAA", "action_type": "cash_dividend", "effective_start_ts": "2021-01-16", "effective_end_ts": "2021-01-16"},
    ]
    hits = ah.find_requires_review_events(
        entries, symbol_universe=["AAA"], start_utc=WINDOW_START, end_utc=WINDOW_END
    )
    assert hits == []


def test_find_requires_review_events_flags_uncovered_type_in_range():
    entries = [
        {"symbol": "AAA", "action_type": "cash_merger", "effective_start_ts": "2021-01-15", "effective_end_ts": "2021-01-15"},
    ]
    hits = ah.find_requires_review_events(
        entries, symbol_universe=["AAA"], start_utc=WINDOW_START, end_utc=WINDOW_END
    )
    assert len(hits) == 1


def test_find_requires_review_events_ignores_outside_universe_or_range():
    entries = [
        {"symbol": "ZZZ", "action_type": "cash_merger", "effective_start_ts": "2021-01-15", "effective_end_ts": "2021-01-15"},
        {"symbol": "AAA", "action_type": "cash_merger", "effective_start_ts": "2019-01-15", "effective_end_ts": "2019-01-15"},
    ]
    hits = ah.find_requires_review_events(
        entries, symbol_universe=["AAA"], start_utc=WINDOW_START, end_utc=WINDOW_END
    )
    assert hits == []


# ---------------------------------------------------------------------------
# extract_research_bars_with_provenance -- full orchestration
# ---------------------------------------------------------------------------


def _queue_clean_extraction(http: FakeHttp, symbol: str = "AAA") -> None:
    http.queue(
        BARS_URL,
        200,
        _bars_page(
            {symbol: [_bar("2021-01-04T00:00:00Z", c=100.0), _bar("2021-01-05T00:00:00Z", c=101.0)]}
        ),
    )
    http.queue(
        CA_URL,
        200,
        _ca_page(
            {
                "cash_dividends": [
                    {
                        "id": "d1",
                        "symbol": symbol,
                        "cusip": "x",
                        "rate": 0.1,
                        "process_date": "2021-01-10",
                        "ex_date": "2021-01-09",
                    }
                ]
            }
        ),
    )


def test_extraction_records_exact_symbol_universe_and_range_and_timeframe():
    http = FakeHttp()
    _queue_clean_extraction(http)
    result = ah.extract_research_bars_with_provenance(
        symbols=["AAA"], start_utc=WINDOW_START, end_utc=WINDOW_END, timeframe="1Day", credentials=_creds(), http_get=http
    )
    manifest = result["manifest"]
    assert manifest["symbol_universe"] == ["AAA"]
    assert manifest["timeframe"] == "1Day"
    assert manifest["start_utc"] == WINDOW_START.isoformat()
    assert manifest["end_utc"] == WINDOW_END.isoformat()


def test_extraction_records_provider_identity_and_adjustment_mode():
    http = FakeHttp()
    _queue_clean_extraction(http)
    result = ah.extract_research_bars_with_provenance(
        symbols=["AAA"], start_utc=WINDOW_START, end_utc=WINDOW_END, credentials=_creds(), http_get=http
    )
    manifest = result["manifest"]
    assert manifest["provider_ids_observed"] == ["alpaca"]
    assert manifest["price_adjustment_convention"] == ah.PRICE_CONVENTION_ALPACA_ALL_ADJUSTED
    assert manifest["source_attestation"]["adjustment_mode"] == "all"


def test_extraction_produces_manifest_that_passes_registered_gates():
    http = FakeHttp()
    _queue_clean_extraction(http)
    result = ah.extract_research_bars_with_provenance(
        symbols=["AAA"], start_utc=WINDOW_START, end_utc=WINDOW_END, credentials=_creds(), http_get=http
    )
    manifest = result["manifest"]
    bars = result["bars"]
    require_registered_bars_provenance(manifest)
    require_bars_match_manifest(bars, manifest)
    check_corporate_action_integrity(bars, manifest)  # must not raise


def test_extraction_fails_closed_on_uncovered_corporate_action_in_range():
    http = FakeHttp()
    http.queue(BARS_URL, 200, _bars_page({"AAA": [_bar("2021-01-04T00:00:00Z")]}))
    http.queue(
        CA_URL,
        200,
        _ca_page(
            {
                "cash_mergers": [
                    {
                        "id": "m1",
                        "acquiree_symbol": "AAA",
                        "acquiree_cusip": "x",
                        "rate": 5.0,
                        "process_date": "2021-01-15",
                        "effective_date": "2021-01-15",
                    }
                ]
            }
        ),
    )
    with pytest.raises(ah.CorporateActionReviewRequired, match="cash_merger"):
        ah.extract_research_bars_with_provenance(
            symbols=["AAA"], start_utc=WINDOW_START, end_utc=WINDOW_END, credentials=_creds(), http_get=http
        )


def test_extraction_fails_closed_even_when_the_action_day_bar_is_absent():
    """A gap in the bars around a REQUIRES_FAIL_CLOSED_REVIEW event's date
    must not let the extraction slip through 'clean' -- the review-required
    check operates on the corporate-action entries directly, independent of
    which bar rows happen to be present."""
    http = FakeHttp()
    # No bar anywhere near 2021-01-15 -- the "action day" is simply missing
    # from the returned bars, as if it had been silently dropped.
    http.queue(
        BARS_URL,
        200,
        _bars_page({"AAA": [_bar("2021-01-04T00:00:00Z"), _bar("2021-01-25T00:00:00Z")]}),
    )
    http.queue(
        CA_URL,
        200,
        _ca_page(
            {
                "worthless_removals": [
                    {"id": "w1", "symbol": "AAA", "cusip": "x", "process_date": "2021-01-15"}
                ]
            }
        ),
    )
    with pytest.raises(ah.CorporateActionReviewRequired):
        ah.extract_research_bars_with_provenance(
            symbols=["AAA"], start_utc=WINDOW_START, end_utc=WINDOW_END, credentials=_creds(), http_get=http
        )


def test_extraction_no_secrets_in_manifest_or_evidence():
    http = FakeHttp()
    _queue_clean_extraction(http)
    creds = ah.AlpacaCredentials(api_key="super-secret-key", api_secret="super-secret-secret")
    result = ah.extract_research_bars_with_provenance(
        symbols=["AAA"], start_utc=WINDOW_START, end_utc=WINDOW_END, credentials=creds, http_get=http
    )
    blob = json.dumps(result["manifest"]) + json.dumps(result["corporate_action_evidence"])
    assert "super-secret-key" not in blob
    assert "super-secret-secret" not in blob
    # Headers sent to the fake transport DID carry credentials (proves the
    # request was actually authenticated) -- but never the artifacts.
    assert any(c["headers"].get("APCA-API-KEY-ID") == "super-secret-key" for c in http.calls)


def test_extraction_altered_bars_fail_before_pl_via_content_binding():
    http = FakeHttp()
    _queue_clean_extraction(http)
    result = ah.extract_research_bars_with_provenance(
        symbols=["AAA"], start_utc=WINDOW_START, end_utc=WINDOW_END, credentials=_creds(), http_get=http
    )
    tampered = result["bars"].copy()
    tampered.loc[0, "close"] = tampered.loc[0, "close"] + 1.0
    from mqk_research.data.bars_provenance import BarsProvenanceContentMismatch

    with pytest.raises(BarsProvenanceContentMismatch):
        require_bars_match_manifest(tampered, result["manifest"])


# ---------------------------------------------------------------------------
# Credentials
# ---------------------------------------------------------------------------


def test_load_alpaca_credentials_fails_closed_when_missing():
    with pytest.raises(ah.AlpacaCredentialsMissing):
        ah.load_alpaca_credentials(env={})


def test_load_alpaca_credentials_reads_repo_convention_env_vars():
    creds = ah.load_alpaca_credentials(env={ah.ENV_ALPACA_KEY: "k", ah.ENV_ALPACA_SECRET: "s"})
    assert creds.api_key == "k"
    assert creds.api_secret == "s"


# ---------------------------------------------------------------------------
# write_research_extraction_artifacts
# ---------------------------------------------------------------------------


def test_write_research_extraction_artifacts_writes_expected_files(tmp_path: Path):
    http = FakeHttp()
    _queue_clean_extraction(http)
    result = ah.extract_research_bars_with_provenance(
        symbols=["AAA"], start_utc=WINDOW_START, end_utc=WINDOW_END, credentials=_creds(), http_get=http
    )
    run_dir = tmp_path / "run1"
    paths = ah.write_research_extraction_artifacts(run_dir, result)

    assert paths["bars_csv"].exists()
    assert paths["bars_provenance_json"].exists()
    assert paths["corporate_actions_json"].exists()
    assert paths["corporate_actions_provenance_json"].exists()

    manifest = json.loads(paths["bars_provenance_json"].read_text(encoding="utf-8"))
    assert manifest["artifact_sha256"] is not None
    assert manifest["row_count"] == 2

    # The written bars csv must satisfy economic_walkforward.load_bars's contract.
    from mqk_research.ml.economic_walkforward import load_bars

    loaded = load_bars(paths["bars_csv"])
    assert list(loaded["symbol"].unique()) == ["AAA"]
