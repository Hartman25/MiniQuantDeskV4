"""
BKT-RESEARCH-MARKET-DATA-AUTHORITY-01 -- tests for the dedicated Alpaca
research historical-data extractor (mqk_research.data.alpaca_historical).

No real network access is used anywhere in this file -- every test injects a
fake `http_get` callable, via the INTERNAL/DIAGNOSTIC entry point
(extract_research_bars_with_provenance_diagnostic) added by
BKT-RESEARCH-MARKET-DATA-AUTHORITY-01-REPAIR-01's official/diagnostic
authority split (Defect 3) -- see test_extractor_split_* below for the
structural proof that the OFFICIAL entry point cannot accept an injected
transport. Covers the mission's REQUIRED TESTS items that pertain to this
module:
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

REPAIR-01 additions (see module docstring in alpaca_historical.py):
  R1.  CA event with ex_date inside the research window but process_date
       after research_end is still discovered (Defect 1)
  R2.  CA discovery does not use an arbitrary short process-date buffer
  R3.  CA discovery pagination that never terminates still fails closed
  R4.  official bars request always sends an explicit asof (Defect 2)
  R5.  official extraction path exposes no injectable transport (Defect 3)
  R6.  a bar exactly at the internal exclusive end_utc boundary is excluded
       from the canonical dataset (provider end-inclusive normalization)
  R7.  a bar before start_utc is excluded
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
ASOF = "2021-02-15"


# ---------------------------------------------------------------------------
# fetch_historical_bars
# ---------------------------------------------------------------------------


def test_fetch_bars_requests_adjustment_all():
    http = FakeHttp()
    http.queue(BARS_URL, 200, _bars_page({"AAA": [_bar("2021-01-04T00:00:00Z")]}))
    df, meta = ah.fetch_historical_bars(
        symbols=["AAA"], start_utc=WINDOW_START, end_utc=WINDOW_END, asof=ASOF, credentials=_creds(), http_get=http
    )
    assert http.calls[0]["params"]["adjustment"] == "all"
    assert meta["resolved_adjustment"] == "all"


def test_fetch_bars_requests_explicit_asof():
    """REQUIRED TEST R4: the official bars request always sends an explicit
    asof -- never relies on Alpaca's implicit current-day default."""
    http = FakeHttp()
    http.queue(BARS_URL, 200, _bars_page({"AAA": [_bar("2021-01-04T00:00:00Z")]}))
    df, meta = ah.fetch_historical_bars(
        symbols=["AAA"], start_utc=WINDOW_START, end_utc=WINDOW_END, asof=ASOF, credentials=_creds(), http_get=http
    )
    assert http.calls[0]["params"]["asof"] == ASOF
    assert meta["resolved_asof"] == ASOF


def test_fetch_bars_missing_asof_rejected():
    http = FakeHttp()
    with pytest.raises(ValueError, match="asof"):
        ah.fetch_historical_bars(
            symbols=["AAA"], start_utc=WINDOW_START, end_utc=WINDOW_END, asof="", credentials=_creds(), http_get=http
        )


def test_fetch_bars_malformed_asof_rejected():
    http = FakeHttp()
    with pytest.raises(ValueError, match="asof"):
        ah.fetch_historical_bars(
            symbols=["AAA"],
            start_utc=WINDOW_START,
            end_utc=WINDOW_END,
            asof="not-a-date",
            credentials=_creds(),
            http_get=http,
        )


def test_fetch_bars_pagination_complete_across_multiple_pages():
    http = FakeHttp()
    http.queue(BARS_URL, 200, _bars_page({"AAA": [_bar("2021-01-04T00:00:00Z")]}, next_token="tok1"))
    http.queue(BARS_URL, 200, _bars_page({"AAA": [_bar("2021-01-05T00:00:00Z")]}))
    df, meta = ah.fetch_historical_bars(
        symbols=["AAA"], start_utc=WINDOW_START, end_utc=WINDOW_END, asof=ASOF, credentials=_creds(), http_get=http
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
        symbols=["AAA"], start_utc=WINDOW_START, end_utc=WINDOW_END, asof=ASOF, credentials=_creds(), http_get=http_a
    )

    http_b = FakeHttp()
    http_b.queue(
        BARS_URL,
        200,
        _bars_page({"AAA": [_bar("2021-01-05T00:00:00Z", c=101.0), _bar("2021-01-04T00:00:00Z", c=100.0)]}),
    )
    df_b, _ = ah.fetch_historical_bars(
        symbols=["AAA"], start_utc=WINDOW_START, end_utc=WINDOW_END, asof=ASOF, credentials=_creds(), http_get=http_b
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
            symbols=["AAA"], start_utc=WINDOW_START, end_utc=WINDOW_END, asof=ASOF, credentials=_creds(), http_get=http
        )


def test_fetch_bars_non_finite_price_fails_closed():
    http = FakeHttp()
    http.queue(BARS_URL, 200, _bars_page({"AAA": [_bar("2021-01-04T00:00:00Z", c=float("nan"))]}))
    with pytest.raises(ah.AlpacaHistoricalExtractionError, match="non-finite"):
        ah.fetch_historical_bars(
            symbols=["AAA"], start_utc=WINDOW_START, end_utc=WINDOW_END, asof=ASOF, credentials=_creds(), http_get=http
        )


def test_fetch_bars_provider_http_error_fails_closed():
    http = FakeHttp()
    http.queue(BARS_URL, 500, {"message": "internal error"})
    with pytest.raises(ah.AlpacaHistoricalExtractionError, match="status=500"):
        ah.fetch_historical_bars(
            symbols=["AAA"], start_utc=WINDOW_START, end_utc=WINDOW_END, asof=ASOF, credentials=_creds(), http_get=http
        )


def test_fetch_bars_malformed_json_fails_closed():
    http = FakeHttp()
    http.queue(BARS_URL, 200, b"{not json")
    with pytest.raises(ah.AlpacaHistoricalExtractionError, match="JSON decode"):
        ah.fetch_historical_bars(
            symbols=["AAA"], start_utc=WINDOW_START, end_utc=WINDOW_END, asof=ASOF, credentials=_creds(), http_get=http
        )


def test_fetch_bars_missing_bars_key_fails_closed():
    http = FakeHttp()
    http.queue(BARS_URL, 200, {"next_page_token": None})
    with pytest.raises(ah.AlpacaHistoricalExtractionError, match="missing required 'bars'"):
        ah.fetch_historical_bars(
            symbols=["AAA"], start_utc=WINDOW_START, end_utc=WINDOW_END, asof=ASOF, credentials=_creds(), http_get=http
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
            symbols=["AAA", "BBB"], start_utc=WINDOW_START, end_utc=WINDOW_END, asof=ASOF, credentials=_creds(), http_get=http
        )


# ---------------------------------------------------------------------------
# REQUIRED TESTS R6/R7 -- provider end-inclusive vs internal [start,end)
# ---------------------------------------------------------------------------


def test_fetch_bars_excludes_bar_exactly_at_end_utc_boundary():
    """Alpaca's documented-inclusive `end` can return a bar timestamped
    exactly at end_utc; this repo's internal contract is the half-open
    [start_utc, end_utc), so that bar must never enter the canonical
    dataset, semantic hash, or economic evaluation."""
    http = FakeHttp()
    http.queue(
        BARS_URL,
        200,
        _bars_page(
            {
                "AAA": [
                    _bar("2021-01-04T00:00:00Z", c=100.0),
                    _bar(WINDOW_END.isoformat(), c=999.0),  # exactly at end_utc
                ]
            }
        ),
    )
    df, meta = ah.fetch_historical_bars(
        symbols=["AAA"], start_utc=WINDOW_START, end_utc=WINDOW_END, asof=ASOF, credentials=_creds(), http_get=http
    )
    assert len(df) == 1
    assert pd.Timestamp(df.iloc[0]["end_ts"]) < WINDOW_END
    assert 999.0 not in df["close"].to_numpy()


def test_fetch_bars_excludes_bar_before_start_utc():
    http = FakeHttp()
    http.queue(
        BARS_URL,
        200,
        _bars_page(
            {
                "AAA": [
                    _bar("2020-12-31T00:00:00Z", c=888.0),  # before start_utc
                    _bar("2021-01-04T00:00:00Z", c=100.0),
                ]
            }
        ),
    )
    df, meta = ah.fetch_historical_bars(
        symbols=["AAA"], start_utc=WINDOW_START, end_utc=WINDOW_END, asof=ASOF, credentials=_creds(), http_get=http
    )
    assert len(df) == 1
    assert 888.0 not in df["close"].to_numpy()


def test_fetch_bars_only_boundary_bar_fails_closed_as_zero_bars():
    http = FakeHttp()
    http.queue(BARS_URL, 200, _bars_page({"AAA": [_bar(WINDOW_END.isoformat(), c=999.0)]}))
    with pytest.raises(ah.AlpacaHistoricalExtractionError, match="none fall inside"):
        ah.fetch_historical_bars(
            symbols=["AAA"], start_utc=WINDOW_START, end_utc=WINDOW_END, asof=ASOF, credentials=_creds(), http_get=http
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
# BKT-RESEARCH-CA-ROLE-AWARE-RESOLUTION-01 -- role-aware resolution (Patch
# B). classify_corporate_action_type's TYPE-level output is unaffected
# (test_classify_corporate_action_type above); these tests cover
# classify_corporate_action_resolution's additional role/identity narrowing.
# ---------------------------------------------------------------------------


def _merger_leg(*, event_id: str, role_symbol: str, other_role_symbol: Optional[str] = None) -> Dict[str, Any]:
    """Build a single stock_merger leg dict as _effective_windows_for_entry
    would produce it, for the given role_symbol=acquirer."""
    raw: Dict[str, Any] = {
        "id": event_id,
        "acquirer_symbol": role_symbol,
        "acquirer_cusip": "cusip-acquirer",
        "process_date": "2021-01-15",
        "effective_date": "2021-01-15",
    }
    if other_role_symbol:
        raw["acquiree_symbol"] = other_role_symbol
        raw["acquiree_cusip"] = "cusip-acquiree"
    legs = ah._effective_windows_for_entry("stock_merger", raw)
    return next(leg for leg in legs if leg["symbol"] == role_symbol)


def test_resolution_acquirer_only_merger_may_pass():
    leg = _merger_leg(event_id="m1", role_symbol="BIG", other_role_symbol="SML")
    assert ah.classify_corporate_action_resolution(leg) == ah.CATEGORY_NO_ADJUSTMENT_REQUIRED_FOR_LEG


def test_resolution_acquiree_leg_still_blocks():
    raw = {
        "id": "m1", "acquirer_symbol": "BIG", "acquirer_cusip": "x",
        "acquiree_symbol": "SML", "acquiree_cusip": "y",
        "process_date": "2021-01-15", "effective_date": "2021-01-15",
    }
    legs = ah._effective_windows_for_entry("stock_merger", raw)
    acquiree_leg = next(leg for leg in legs if leg["symbol"] == "SML")
    assert ah.classify_corporate_action_resolution(acquiree_leg) == ah.CATEGORY_REQUIRES_FAIL_CLOSED_REVIEW


def test_resolution_same_event_mutated_from_acquirer_to_acquiree_must_fail():
    """A leg for the SAME symbol under the SAME event, but with its role
    field swapped from acquirer_symbol to acquiree_symbol, must fail closed
    -- role correctness, not the raw event, drives resolution."""
    raw_as_acquirer = {"id": "m1", "acquirer_symbol": "AAA", "acquirer_cusip": "x", "process_date": "2021-01-15"}
    raw_as_acquiree = {"id": "m1", "acquiree_symbol": "AAA", "acquiree_cusip": "x", "process_date": "2021-01-15"}
    leg_acquirer = ah._effective_windows_for_entry("stock_merger", raw_as_acquirer)[0]
    leg_acquiree = ah._effective_windows_for_entry("stock_merger", raw_as_acquiree)[0]
    assert ah.classify_corporate_action_resolution(leg_acquirer) == ah.CATEGORY_NO_ADJUSTMENT_REQUIRED_FOR_LEG
    assert ah.classify_corporate_action_resolution(leg_acquiree) == ah.CATEGORY_REQUIRES_FAIL_CLOSED_REVIEW


def test_resolution_blank_role_still_blocks():
    leg = {"symbol": "AAA", "action_type": "cash_merger", "effective_start_ts": "2021-01-15", "effective_end_ts": "2021-01-15"}
    assert ah.classify_corporate_action_resolution(leg) == ah.CATEGORY_REQUIRES_FAIL_CLOSED_REVIEW


def test_resolution_acquirer_blocked_by_same_event_terminal_conflict():
    """Adversarial data-integrity case: the SAME symbol appears under BOTH
    acquirer_symbol and acquiree_symbol of the SAME raw event -- the
    acquirer leg must not auto-clear despite matched_role=='acquirer'."""
    raw = {
        "id": "m1", "acquirer_symbol": "AAA", "acquirer_cusip": "x",
        "acquiree_symbol": "AAA", "acquiree_cusip": "y",
        "process_date": "2021-01-15",
    }
    legs = ah._effective_windows_for_entry("stock_merger", raw)
    acquirer_leg = next(leg for leg in legs if leg["matched_role"] == "acquirer")
    siblings = [leg for leg in legs if leg is not acquirer_leg]
    assert ah.classify_corporate_action_resolution(acquirer_leg, sibling_legs=siblings) == ah.CATEGORY_REQUIRES_FAIL_CLOSED_REVIEW


def _name_change_leg(*, old_symbol: str, new_symbol: str, old_cusip: Optional[str], new_cusip: Optional[str], event_id: str = "n1") -> Dict[str, Any]:
    raw = {
        "id": event_id, "old_symbol": old_symbol, "new_symbol": new_symbol,
        "old_cusip": old_cusip, "new_cusip": new_cusip, "process_date": "2021-01-15",
    }
    legs = ah._effective_windows_for_entry("name_change", raw)
    return next(leg for leg in legs if leg["symbol"] == new_symbol)


def test_resolution_name_change_matching_cusips_verified_continuity():
    leg = _name_change_leg(old_symbol="FB", new_symbol="META", old_cusip="30303M102", new_cusip="30303M102")
    assert ah.classify_corporate_action_resolution(leg) == ah.CATEGORY_VERIFIED_SAME_SECURITY_CONTINUITY


def test_resolution_name_change_missing_cusip_still_blocks():
    leg = _name_change_leg(old_symbol="FB", new_symbol="META", old_cusip="30303M102", new_cusip=None)
    assert ah.classify_corporate_action_resolution(leg) == ah.CATEGORY_REQUIRES_FAIL_CLOSED_REVIEW


def test_resolution_name_change_unequal_cusips_still_blocks():
    leg = _name_change_leg(old_symbol="FB", new_symbol="META", old_cusip="30303M102", new_cusip="99999X999")
    assert ah.classify_corporate_action_resolution(leg) == ah.CATEGORY_REQUIRES_FAIL_CLOSED_REVIEW


def test_resolution_unrelated_same_ticker_name_change_not_authorized_by_a_different_resolved_one():
    """Negative control: a genuine FB->META continuity chain resolving
    cleanly must NOT authorize a totally unrelated, unresolved name-change
    event that happens to share the literal ticker META."""
    legit = _name_change_leg(old_symbol="FB", new_symbol="META", old_cusip="30303M102", new_cusip="30303M102", event_id="n1")
    unrelated = _name_change_leg(old_symbol="OLDCO", new_symbol="META", old_cusip="c1", new_cusip="c2", event_id="n2")
    assert ah.classify_corporate_action_resolution(legit) == ah.CATEGORY_VERIFIED_SAME_SECURITY_CONTINUITY
    assert ah.classify_corporate_action_resolution(unrelated) == ah.CATEGORY_REQUIRES_FAIL_CLOSED_REVIEW


@pytest.mark.parametrize(
    "action_type",
    ["unit_split", "stock_dividend", "redemption", "worthless_removal", "rights_distribution", "partial_call", "reorganization"],
)
def test_resolution_unaddressed_types_remain_fail_closed(action_type):
    leg = {
        "symbol": "AAA", "action_type": action_type, "matched_role": "primary",
        "effective_start_ts": "2021-01-15", "effective_end_ts": "2021-01-15",
    }
    assert ah.classify_corporate_action_resolution(leg) == ah.CATEGORY_REQUIRES_FAIL_CLOSED_REVIEW


def test_resolution_unknown_type_still_fails():
    leg = {"symbol": "AAA", "action_type": "not_a_real_type", "matched_role": "primary"}
    with pytest.raises(ValueError, match="Unknown corporate action"):
        ah.classify_corporate_action_resolution(leg)


def test_find_requires_review_events_clears_acquirer_leg_but_not_acquiree():
    entries = ah._effective_windows_for_entry(
        "stock_merger",
        {"id": "m1", "acquirer_symbol": "BIG", "acquirer_cusip": "x", "acquiree_symbol": "SML", "acquiree_cusip": "y", "process_date": "2021-01-15"},
    )
    hits_big = ah.find_requires_review_events(entries, symbol_universe=["BIG"], start_utc=WINDOW_START, end_utc=WINDOW_END)
    hits_sml = ah.find_requires_review_events(entries, symbol_universe=["SML"], start_utc=WINDOW_START, end_utc=WINDOW_END)
    assert hits_big == []
    assert len(hits_sml) == 1


def test_find_requires_review_events_clears_verified_name_change_continuity():
    entries = ah._effective_windows_for_entry(
        "name_change",
        {"id": "n1", "old_symbol": "FB", "new_symbol": "META", "old_cusip": "c1", "new_cusip": "c1", "process_date": "2021-01-15"},
    )
    hits = ah.find_requires_review_events(entries, symbol_universe=["META"], start_utc=WINDOW_START, end_utc=WINDOW_END)
    assert hits == []


def test_extraction_clean_on_acquirer_only_merger():
    """Full-pipeline proof: an acquirer-only merger in range no longer
    raises CorporateActionReviewRequired."""
    http = FakeHttp()
    http.queue(BARS_URL, 200, _bars_page({"BIG": [_bar("2021-01-04T00:00:00Z")]}))
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
                        "acquiree_symbol": "SML",
                        "acquiree_cusip": "y",
                        "process_date": "2021-01-15",
                        "effective_date": "2021-01-15",
                    }
                ]
            }
        ),
    )
    result = ah.extract_research_bars_with_provenance_diagnostic(
        symbols=["BIG"], start_utc=WINDOW_START, end_utc=WINDOW_END, asof=ASOF, credentials=_creds(), http_get=http
    )
    entries = result["corporate_action_entries"]
    assert len(entries) == 1
    assert entries[0]["symbol"] == "BIG"


def test_extraction_clean_on_verified_name_change_continuity():
    http = FakeHttp()
    http.queue(BARS_URL, 200, _bars_page({"META": [_bar("2021-01-04T00:00:00Z")]}))
    http.queue(
        CA_URL,
        200,
        _ca_page(
            {
                "name_changes": [
                    {
                        "id": "n1",
                        "old_symbol": "FB",
                        "new_symbol": "META",
                        "old_cusip": "30303M102",
                        "new_cusip": "30303M102",
                        "process_date": "2021-01-15",
                    }
                ]
            }
        ),
    )
    result = ah.extract_research_bars_with_provenance_diagnostic(
        symbols=["META"], start_utc=WINDOW_START, end_utc=WINDOW_END, asof=ASOF, credentials=_creds(), http_get=http
    )
    entries = result["corporate_action_entries"]
    assert len(entries) == 1
    assert entries[0]["symbol"] == "META"


def _dkng_reviewed_leg(**overrides: Any) -> Dict[str, Any]:
    """The EXACT confirmed live Alpaca leg BKT-RESEARCH-CA-REVIEWED-
    SUCCESSOR-RESOLUTION-01-REPAIR-01's registry record is bound to (see
    ca_reviewed_resolutions.py module header)."""
    leg = {
        "symbol": "DKNG", "action_type": "stock_merger", "matched_role": "acquiree",
        "matched_symbol": "DKNG", "matched_symbol_field": "acquiree_symbol",
        "process_date": "2022-05-05",
        "effective_start_ts": "2022-05-05", "effective_end_ts": "2022-05-05",
        "provider_event_id": "e21ce7ea-649b-456a-a4f4-b025cbdc1fca",
        "matched_cusip": "26142R104", "acquirer_cusip": "26142V105", "acquiree_cusip": "26142R104",
    }
    leg.update(overrides)
    return leg


def test_resolution_dkng_reviewed_event_resolves_via_default_registry():
    """BKT-RESEARCH-CA-REVIEWED-SUCCESSOR-RESOLUTION-01 (REPAIR-01): a
    stock_merger acquiree leg matching the canonical DKNG reviewed record
    exactly resolves via the default registry -- no symbol-specific
    branching in Python, purely a data-driven match."""
    from mqk_research.data.ca_reviewed_resolutions import RESOLUTION_VERIFIED_ONE_FOR_ONE_SUCCESSOR_SECURITY_CONTINUITY

    leg = _dkng_reviewed_leg()
    assert ah.classify_corporate_action_resolution(leg) == RESOLUTION_VERIFIED_ONE_FOR_ONE_SUCCESSOR_SECURITY_CONTINUITY


def test_resolution_reviewed_registry_is_injectable_and_defaults_closed():
    """An empty reviewed_resolutions override (simulating a missing/absent
    reviewed-resolution artifact) must fall back to fail-closed -- required
    test 8, exercised through the actual resolution entry point."""
    leg = _dkng_reviewed_leg()
    assert ah.classify_corporate_action_resolution(leg, reviewed_resolutions=()) == ah.CATEGORY_REQUIRES_FAIL_CLOSED_REVIEW


def test_resolution_dkng_different_provider_event_id_fails_closed():
    """REPAIR-01 E2: a distinct provider event_id on an otherwise-identical
    leg must not inherit the review."""
    leg = _dkng_reviewed_leg(provider_event_id="00000000-0000-0000-0000-000000000000")
    assert ah.classify_corporate_action_resolution(leg) == ah.CATEGORY_REQUIRES_FAIL_CLOSED_REVIEW


def test_resolution_dkng_different_matched_symbol_field_fails_closed():
    leg = _dkng_reviewed_leg(matched_symbol_field="symbol")
    assert ah.classify_corporate_action_resolution(leg) == ah.CATEGORY_REQUIRES_FAIL_CLOSED_REVIEW


def test_resolution_dkng_different_acquiree_cusip_fails_closed():
    leg = _dkng_reviewed_leg(acquiree_cusip="99999999X", matched_cusip="99999999X")
    assert ah.classify_corporate_action_resolution(leg) == ah.CATEGORY_REQUIRES_FAIL_CLOSED_REVIEW


def test_resolution_dkng_different_acquirer_cusip_fails_closed():
    leg = _dkng_reviewed_leg(acquirer_cusip="99999999X")
    assert ah.classify_corporate_action_resolution(leg) == ah.CATEGORY_REQUIRES_FAIL_CLOSED_REVIEW


def test_resolution_reviewed_match_does_not_bypass_covered_or_role_aware_categories():
    """The reviewed registry is only ever consulted as a LAST resort -- a
    type already CATEGORY_COVERED_BY_ADJUSTMENT, or a leg already resolved
    by the automated role-aware rules, never even reaches the registry
    lookup (proven indirectly: an acquirer leg resolves to
    CATEGORY_NO_ADJUSTMENT_REQUIRED_FOR_LEG even when passed an EMPTY
    registry)."""
    leg = _merger_leg(event_id="m1", role_symbol="BIG", other_role_symbol="SML")
    assert ah.classify_corporate_action_resolution(leg, reviewed_resolutions=()) == ah.CATEGORY_NO_ADJUSTMENT_REQUIRED_FOR_LEG


_DKNG_LIVE_STOCK_MERGER_RAW: Dict[str, Any] = {
    "id": "e21ce7ea-649b-456a-a4f4-b025cbdc1fca",
    "acquiree_symbol": "DKNG",
    "acquiree_cusip": "26142R104",
    "acquirer_cusip": "26142V105",
    "process_date": "2022-05-05",
}


def test_extraction_clean_on_dkng_reviewed_stock_merger():
    """Full-pipeline proof: the EXACT confirmed live DKNG stock_merger
    acquiree event no longer blocks extraction via the default (built-in)
    reviewed-resolution registry."""
    http = FakeHttp()
    http.queue(BARS_URL, 200, _bars_page({"DKNG": [_bar("2022-05-06T00:00:00Z")]}))
    http.queue(
        CA_URL,
        200,
        _ca_page({"stock_mergers": [dict(_DKNG_LIVE_STOCK_MERGER_RAW)]}),
    )
    result = ah.extract_research_bars_with_provenance_diagnostic(
        symbols=["DKNG"],
        start_utc=pd.Timestamp("2022-05-01T00:00:00Z"),
        end_utc=pd.Timestamp("2022-06-01T00:00:00Z"),
        asof="2022-06-15",
        credentials=_creds(),
        http_get=http,
    )
    entries = result["corporate_action_entries"]
    assert len(entries) == 1
    assert entries[0]["symbol"] == "DKNG"


def test_extraction_still_fails_closed_on_dkng_event_with_different_process_date():
    """The SAME nominal stock_merger reported for DKNG under a DIFFERENT
    process_date than the exact reviewed record must still fail closed --
    the reviewed exception is bound to the exact event, not the symbol."""
    http = FakeHttp()
    http.queue(BARS_URL, 200, _bars_page({"DKNG": [_bar("2023-01-05T00:00:00Z")]}))
    http.queue(
        CA_URL,
        200,
        _ca_page(
            {"stock_mergers": [dict(_DKNG_LIVE_STOCK_MERGER_RAW, process_date="2023-01-01")]}
        ),
    )
    with pytest.raises(ah.CorporateActionReviewRequired, match="stock_merger"):
        ah.extract_research_bars_with_provenance_diagnostic(
            symbols=["DKNG"],
            start_utc=pd.Timestamp("2023-01-01T00:00:00Z"),
            end_utc=pd.Timestamp("2023-02-01T00:00:00Z"),
            asof="2023-02-15",
            credentials=_creds(),
            http_get=http,
        )


def test_extraction_still_fails_closed_on_unresolved_name_change():
    http = FakeHttp()
    http.queue(BARS_URL, 200, _bars_page({"META": [_bar("2021-01-04T00:00:00Z")]}))
    http.queue(
        CA_URL,
        200,
        _ca_page(
            {
                "name_changes": [
                    {
                        "id": "n1",
                        "old_symbol": "OLDCO",
                        "new_symbol": "META",
                        "old_cusip": "c1",
                        "new_cusip": "c2",
                        "process_date": "2021-01-15",
                    }
                ]
            }
        ),
    )
    with pytest.raises(ah.CorporateActionReviewRequired, match="name_change"):
        ah.extract_research_bars_with_provenance_diagnostic(
            symbols=["META"], start_utc=WINDOW_START, end_utc=WINDOW_END, asof=ASOF, credentials=_creds(), http_get=http
        )


# ---------------------------------------------------------------------------
# extract_research_bars_with_provenance_diagnostic -- full orchestration
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


def _extract(http: FakeHttp, **overrides: Any) -> Dict[str, Any]:
    kwargs: Dict[str, Any] = dict(
        symbols=["AAA"], start_utc=WINDOW_START, end_utc=WINDOW_END, asof=ASOF, credentials=_creds(), http_get=http
    )
    kwargs.update(overrides)
    return ah.extract_research_bars_with_provenance_diagnostic(**kwargs)


def test_extraction_records_exact_symbol_universe_and_range_and_timeframe():
    http = FakeHttp()
    _queue_clean_extraction(http)
    result = _extract(http, timeframe="1Day")
    manifest = result["manifest"]
    assert manifest["symbol_universe"] == ["AAA"]
    assert manifest["timeframe"] == "1Day"
    assert manifest["start_utc"] == WINDOW_START.isoformat()
    assert manifest["end_utc"] == WINDOW_END.isoformat()


def test_extraction_records_provider_identity_and_adjustment_mode():
    http = FakeHttp()
    _queue_clean_extraction(http)
    result = _extract(http)
    manifest = result["manifest"]
    assert manifest["provider_ids_observed"] == ["alpaca"]
    assert manifest["price_adjustment_convention"] == ah.PRICE_CONVENTION_ALPACA_ALL_ADJUSTED
    assert manifest["source_attestation"]["adjustment_mode"] == "all"
    assert manifest["source_attestation"]["asof"] == ASOF


def test_extraction_produces_manifest_that_passes_registered_gates_via_official_path(monkeypatch):
    """REQUIRED TESTS 2/3: exercises the ACTUAL OFFICIAL public wrapper
    (extract_research_bars_with_provenance) -- not the shared neutral core --
    networklessly, by monkeypatching the module's fixed transport
    (_default_http_get) and clock (_utc_now) seams. Its manifest must pass
    the registered gates; see test_diagnostic_authority_cannot_pass_registered_gates
    below for the contrasting diagnostic case."""
    http = FakeHttp()
    _queue_clean_extraction(http)
    monkeypatch.setattr(ah, "_default_http_get", http)
    monkeypatch.setattr(ah, "_utc_now", lambda: pd.Timestamp("2026-08-15T00:00:00Z"))
    result = ah.extract_research_bars_with_provenance(
        symbols=["AAA"],
        start_utc=WINDOW_START,
        end_utc=WINDOW_END,
        asof=ASOF,
        timeframe=ah.DEFAULT_TIMEFRAME,
        feed=ah.DEFAULT_FEED,
        credentials=_creds(),
    )
    manifest = result["manifest"]
    bars = result["bars"]
    assert manifest["source_attestation"]["extractor_id"] == ah.EXTRACTOR_ID
    assert manifest["source_attestation"]["source_authority"] == "official_provider"
    assert manifest["source_attestation"]["retrieval_timestamp_utc"] == "2026-08-15T00:00:00+00:00"
    require_registered_bars_provenance(manifest)
    require_bars_match_manifest(bars, manifest)
    check_corporate_action_integrity(bars, manifest)  # must not raise


def test_diagnostic_authority_cannot_pass_registered_gates():
    """REQUIRED TEST 9: a diagnostic-transport extraction must never
    authorize official registered research, even though it is otherwise a
    perfectly well-formed, internally-consistent manifest."""
    from mqk_research.data.bars_provenance import SourceAttestationUnverifiable

    http = FakeHttp()
    _queue_clean_extraction(http)
    result = _extract(http)
    manifest = result["manifest"]
    bars = result["bars"]
    assert manifest["source_attestation"]["extractor_id"] == ah.DIAGNOSTIC_EXTRACTOR_ID
    assert manifest["source_attestation"]["source_authority"] == "diagnostic_synthetic"
    with pytest.raises(SourceAttestationUnverifiable):
        check_corporate_action_integrity(bars, manifest)


def test_official_entry_point_has_no_injectable_transport_params():
    """REQUIRED TEST 1 (structural): the OFFICIAL public extraction function
    accepts none of: http_get, base_url, extractor_id, source_authority, or
    any caller-selectable CA-discovery-snapshot-clock override (REPAIR-03
    Defect 2 -- neither the old retrieval_timestamp_utc name nor the
    diagnostic path's ca_discovery_cutoff_utc name)."""
    import inspect

    sig = inspect.signature(ah.extract_research_bars_with_provenance)
    forbidden = {
        "http_get",
        "base_url",
        "extractor_id",
        "source_authority",
        "retrieval_timestamp_utc",
        "ca_discovery_cutoff_utc",
    }
    assert forbidden.isdisjoint(sig.parameters)


def test_neutral_extraction_core_has_no_authority_params():
    """REQUIRED TESTS 5/6: the shared neutral extraction core (_neutral_
    extract) cannot accept a caller-selected extractor_id/source_authority --
    no callable path intended as the shared extraction seam takes both an
    injectable transport AND caller-selected official authority."""
    import inspect

    sig = inspect.signature(ah._neutral_extract)
    assert "extractor_id" not in sig.parameters
    assert "source_authority" not in sig.parameters
    assert "http_get" in sig.parameters  # still the injectable-transport seam


def test_official_snapshot_clock_resolved_exactly_once(monkeypatch):
    """REQUIRED TEST 7: the OFFICIAL CA discovery snapshot time is resolved
    exactly once per extraction (_utc_now), never repeatedly inside one
    extraction."""
    http = FakeHttp()
    _queue_clean_extraction(http)
    call_count = {"n": 0}

    def _fake_now() -> pd.Timestamp:
        call_count["n"] += 1
        return pd.Timestamp("2026-08-15T00:00:00Z")

    monkeypatch.setattr(ah, "_default_http_get", http)
    monkeypatch.setattr(ah, "_utc_now", _fake_now)
    ah.extract_research_bars_with_provenance(
        symbols=["AAA"], start_utc=WINDOW_START, end_utc=WINDOW_END, asof=ASOF, credentials=_creds()
    )
    assert call_count["n"] == 1


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
        _extract(http)


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
        _extract(http)


def test_extraction_no_secrets_in_manifest_or_evidence():
    http = FakeHttp()
    _queue_clean_extraction(http)
    creds = ah.AlpacaCredentials(api_key="super-secret-key", api_secret="super-secret-secret")
    result = _extract(http, credentials=creds)
    blob = json.dumps(result["manifest"]) + json.dumps(result["corporate_action_evidence"])
    assert "super-secret-key" not in blob
    assert "super-secret-secret" not in blob
    # Headers sent to the fake transport DID carry credentials (proves the
    # request was actually authenticated) -- but never the artifacts.
    assert any(c["headers"].get("APCA-API-KEY-ID") == "super-secret-key" for c in http.calls)


def test_extraction_altered_bars_fail_before_pl_via_content_binding():
    http = FakeHttp()
    _queue_clean_extraction(http)
    result = _extract(http)
    tampered = result["bars"].copy()
    tampered.loc[0, "close"] = tampered.loc[0, "close"] + 1.0
    from mqk_research.data.bars_provenance import BarsProvenanceContentMismatch

    with pytest.raises(BarsProvenanceContentMismatch):
        require_bars_match_manifest(tampered, result["manifest"])


# ---------------------------------------------------------------------------
# REPAIR-01 Defect 1 -- CA discovery completeness (process_date lag)
# ---------------------------------------------------------------------------


def test_ca_discovery_finds_event_with_ex_date_in_range_but_process_date_after_end():
    """REQUIRED TEST R1: an event whose ex_date falls inside the research
    window but whose process_date lands AFTER research_end (the mission's
    own example: research_end=Jun30, ex_date=Jun28, process_date=Jul02) must
    still be discovered -- proving the pre-repair defect (querying CA
    start/end == the research window, which Alpaca filters by process_date)
    is closed."""
    http = FakeHttp()
    http.queue(BARS_URL, 200, _bars_page({"AAA": [_bar("2021-06-04T00:00:00Z")]}))
    http.queue(
        CA_URL,
        200,
        _ca_page(
            {
                "cash_dividends": [
                    {
                        "id": "d1",
                        "symbol": "AAA",
                        "cusip": "x",
                        "rate": 0.1,
                        "ex_date": "2021-06-28",
                        "process_date": "2021-07-02",
                    }
                ]
            }
        ),
    )
    result = ah.extract_research_bars_with_provenance_diagnostic(
        symbols=["AAA"],
        start_utc=pd.Timestamp("2021-06-01T00:00:00Z"),
        end_utc=pd.Timestamp("2021-06-30T00:00:00Z"),
        asof="2021-07-05",  # must be >= process_date to have discovered it
        credentials=_creds(),
        http_get=http,
    )
    entries = result["corporate_action_entries"]
    assert len(entries) == 1
    assert entries[0]["effective_start_ts"] == "2021-06-28"


def test_ca_discovery_query_uses_full_history_floor_not_a_narrow_buffer():
    """REQUIRED TEST R2: the CA discovery query's process-date lower bound
    is the documented 'no bound available' floor, not a short arbitrary
    padding window (e.g. 30/60/90 days before the research window)."""
    http = FakeHttp()
    _queue_clean_extraction(http)
    _extract(http)
    ca_call = next(c for c in http.calls if c["url"] == CA_URL)
    assert ca_call["params"]["start"] == ah.CA_DISCOVERY_PROCESS_DATE_FLOOR_UTC.date().isoformat()
    assert ca_call["params"]["start"] == "1900-01-01"


def test_ca_discovery_query_end_is_retrieval_cutoff_not_asof():
    """REPAIR-02: the CA discovery process-date ceiling is the resolved CA
    discovery snapshot cutoff (ca_discovery_cutoff_utc), NEVER `asof` -- proves
    REPAIR-01's defective max(asof, research_end) ceiling formula is gone.
    asof is set to a date the ceiling must NOT equal; the CA query's `end`
    must equal the distinct, later retrieval cutoff instead."""
    http = FakeHttp()
    _queue_clean_extraction(http)
    _extract(http, asof="2021-03-01", ca_discovery_cutoff_utc="2022-09-20T00:00:00Z")
    ca_call = next(c for c in http.calls if c["url"] == CA_URL)
    assert ca_call["params"]["end"] == "2022-09-20"
    assert ca_call["params"]["end"] != "2021-03-01"


def test_ca_discovery_cutoff_independent_of_asof():
    """REQUIRED TEST 6: changing bars_asof alone (retrieval cutoff pinned)
    does not silently redefine the CA discovery cutoff."""
    cutoffs = []
    for asof in ("2021-03-01", "2021-06-15", "2021-12-31"):
        http = FakeHttp()
        _queue_clean_extraction(http)
        _extract(http, asof=asof, ca_discovery_cutoff_utc="2022-09-20T00:00:00Z")
        ca_call = next(c for c in http.calls if c["url"] == CA_URL)
        cutoffs.append(ca_call["params"]["end"])
    assert cutoffs == ["2022-09-20"] * 3


def test_ca_discovery_cutoff_change_represented_in_source_contract():
    """REQUIRED TEST 7: a changed CA discovery cutoff is represented in the
    recorded provenance/source contract (corporate_action_query_coverage)."""
    results = {}
    for cutoff in ("2022-01-01T00:00:00Z", "2022-09-20T00:00:00Z"):
        http = FakeHttp()
        _queue_clean_extraction(http)
        results[cutoff] = _extract(http, ca_discovery_cutoff_utc=cutoff)

    coverage_a = results["2022-01-01T00:00:00Z"]["manifest"]["source_attestation"][
        "corporate_action_query_coverage"
    ]
    coverage_b = results["2022-09-20T00:00:00Z"]["manifest"]["source_attestation"][
        "corporate_action_query_coverage"
    ]
    assert coverage_a["ca_discovery_end_utc"] == "2022-01-01T00:00:00+00:00"
    assert coverage_b["ca_discovery_end_utc"] == "2022-09-20T00:00:00+00:00"
    assert coverage_a["ca_discovery_end_utc"] != coverage_b["ca_discovery_end_utc"]
    assert coverage_a["discovery_protocol"] == ah.CA_DISCOVERY_PROTOCOL_V2


def test_mission_scenario_ca_process_date_after_both_research_end_and_bars_asof_still_discovered():
    """RED/GREEN proof for the REPAIR-02 defect, using the mission's exact
    scenario: research_end=2020-06-30, bars_asof=2020-06-30 (== research_end,
    deliberately NOT covering the event), event ex_date=2020-06-28 (inside
    the research interval), event process_date=2020-07-02 (AFTER both
    research_end and bars_asof), retrieval/extraction date=2026-08-15.

    RED (documented, not re-executed): under REPAIR-01's formula, the CA
    discovery ceiling would have been max(asof, research_end) =
    max(2020-06-30, 2020-06-30) = 2020-06-30, which is strictly before the
    event's process_date (2020-07-02) -- Alpaca's /v1/corporate-actions
    filters by process_date, so that query would never have returned this
    event and it would have been silently missed.

    GREEN: REPAIR-02's ceiling is the resolved CA discovery cutoff
    (2026-08-15, the extraction/retrieval instant), which is after the
    event's process_date -- so the event is discovered."""
    research_end = "2020-06-30"
    bars_asof = "2020-06-30"
    event_process_date = "2020-07-02"
    retrieval_cutoff = "2026-08-15T00:00:00Z"

    # RED proof: REPAIR-01's ceiling formula would not have covered the event.
    old_ceiling = max(pd.Timestamp(f"{bars_asof}T00:00:00Z"), pd.Timestamp(f"{research_end}T00:00:00Z"))
    assert old_ceiling < pd.Timestamp(f"{event_process_date}T00:00:00Z")

    http = FakeHttp()
    http.queue(BARS_URL, 200, _bars_page({"AAA": [_bar("2020-06-04T00:00:00Z")]}))
    http.queue(
        CA_URL,
        200,
        _ca_page(
            {
                "cash_dividends": [
                    {
                        "id": "d1",
                        "symbol": "AAA",
                        "cusip": "x",
                        "rate": 0.1,
                        "ex_date": "2020-06-28",
                        "process_date": event_process_date,
                    }
                ]
            }
        ),
    )
    result = ah.extract_research_bars_with_provenance_diagnostic(
        symbols=["AAA"],
        start_utc=pd.Timestamp("2020-06-01T00:00:00Z"),
        end_utc=pd.Timestamp(f"{research_end}T00:00:00Z"),
        asof=bars_asof,
        credentials=_creds(),
        http_get=http,
        ca_discovery_cutoff_utc=retrieval_cutoff,
    )
    entries = result["corporate_action_entries"]
    assert len(entries) == 1
    assert entries[0]["effective_start_ts"] == "2020-06-28"
    # effective/ex-date lies inside the research interval [start_utc, end_utc)
    assert pd.Timestamp("2020-06-01T00:00:00Z") <= pd.Timestamp("2020-06-28T00:00:00Z") < pd.Timestamp(
        f"{research_end}T00:00:00Z"
    )
    # bars_asof remains explicitly sent to /v2/stocks/bars (Defect 2, unaffected)
    bars_call = next(c for c in http.calls if c["url"] == BARS_URL)
    assert bars_call["params"]["asof"] == bars_asof
    # CA discovery ceiling actually used was the retrieval cutoff, not asof/research_end
    ca_call = next(c for c in http.calls if c["url"] == CA_URL)
    assert ca_call["params"]["end"] == "2026-08-15"


def test_same_ca_discovery_snapshot_and_content_is_deterministic():
    """REQUIRED TEST 10: re-running the SAME extraction (same provider
    content, same explicitly-pinned retrieval/CA-discovery cutoff) produces
    an identical attestation identity."""
    pinned_cutoff = "2022-09-20T00:00:00Z"

    http_a = FakeHttp()
    _queue_clean_extraction(http_a)
    result_a = _extract(http_a, ca_discovery_cutoff_utc=pinned_cutoff)

    http_b = FakeHttp()
    _queue_clean_extraction(http_b)
    result_b = _extract(http_b, ca_discovery_cutoff_utc=pinned_cutoff)

    assert (
        result_a["manifest"]["source_attestation"]["attestation_id"]
        == result_b["manifest"]["source_attestation"]["attestation_id"]
    )
    assert (
        result_a["manifest"]["canonical_semantic_bars_hash"]
        == result_b["manifest"]["canonical_semantic_bars_hash"]
    )
    assert (
        result_a["manifest"]["corporate_action_evidence_id"]
        == result_b["manifest"]["corporate_action_evidence_id"]
    )


def test_later_discovery_snapshot_with_additional_ca_changes_evidence_and_identity():
    """REQUIRED TESTS 8/9: a later CA discovery snapshot that turns up an
    ADDITIONAL provider-backfilled corporate action must change the semantic
    corporate-action evidence, and that change must propagate into
    provenance/trial identity (corporate_action_evidence_id,
    source_attestation_id, and provenance_identity_fragment)."""
    from mqk_research.data.bars_provenance import provenance_identity_fragment

    def _queue_extraction_with_entries(http: FakeHttp, entries: List[Dict[str, Any]]) -> None:
        http.queue(BARS_URL, 200, _bars_page({"AAA": [_bar("2021-01-04T00:00:00Z", c=100.0)]}))
        http.queue(CA_URL, 200, _ca_page({"cash_dividends": entries}))

    early_entry = {
        "id": "d1",
        "symbol": "AAA",
        "cusip": "x",
        "rate": 0.1,
        "ex_date": "2021-01-09",
        "process_date": "2021-01-10",
    }
    backfilled_entry = {
        "id": "d2",
        "symbol": "AAA",
        "cusip": "x",
        "rate": 0.2,
        "ex_date": "2021-01-15",
        "process_date": "2021-06-01",  # discovered only by the later snapshot
    }

    http_early = FakeHttp()
    _queue_extraction_with_entries(http_early, [early_entry])
    result_early = _extract(
        http_early,
        start_utc=pd.Timestamp("2021-01-01T00:00:00Z"),
        end_utc=pd.Timestamp("2021-02-01T00:00:00Z"),
        ca_discovery_cutoff_utc="2021-02-01T00:00:00Z",  # before the backfilled process_date
    )

    http_later = FakeHttp()
    _queue_extraction_with_entries(http_later, [early_entry, backfilled_entry])
    result_later = _extract(
        http_later,
        start_utc=pd.Timestamp("2021-01-01T00:00:00Z"),
        end_utc=pd.Timestamp("2021-02-01T00:00:00Z"),
        ca_discovery_cutoff_utc="2021-07-01T00:00:00Z",  # after the backfilled process_date
    )

    assert len(result_early["corporate_action_entries"]) == 1
    assert len(result_later["corporate_action_entries"]) == 2

    manifest_early = result_early["manifest"]
    manifest_later = result_later["manifest"]
    assert manifest_early["corporate_action_evidence_id"] != manifest_later["corporate_action_evidence_id"]
    assert (
        manifest_early["source_attestation"]["attestation_id"]
        != manifest_later["source_attestation"]["attestation_id"]
    )
    assert provenance_identity_fragment(manifest_early) != provenance_identity_fragment(manifest_later)


# ---------------------------------------------------------------------------
# BKT-RESEARCH-MARKET-DATA-AUTHORITY-01-REPAIR-04 -- cross-module regression:
# bars_provenance's mirrored TRUSTED_CA_DISCOVERY_* contract (which cannot
# import this module -- alpaca_historical already imports bars_provenance)
# must equal this extractor's actual query contract.
# ---------------------------------------------------------------------------


def test_trusted_ca_discovery_contract_matches_extractor():
    """The values bars_provenance mirrors as the TRUSTED official CA
    discovery contract must equal this extractor's actual constants -- if
    either side drifts without updating the other, the official gate would
    either wrongly reject real official output or wrongly trust an
    under-scoped one."""
    from mqk_research.data import bars_provenance as bp

    assert bp.TRUSTED_CA_DISCOVERY_PROTOCOL_V2 == ah.CA_DISCOVERY_PROTOCOL_V2
    assert bp.TRUSTED_CA_DISCOVERY_FLOOR_UTC == ah.CA_DISCOVERY_PROCESS_DATE_FLOOR_UTC.isoformat()
    assert bp.TRUSTED_CA_DISCOVERY_TYPES == ah.KNOWN_CORPORATE_ACTION_TYPES


def test_ca_discovery_incomplete_pagination_fails_closed():
    """REQUIRED TEST R3: the broadened CA discovery query still fails closed
    if the provider never terminates pagination -- no silent partial
    'complete enough' result."""
    http = FakeHttp()
    http.queue(BARS_URL, 200, _bars_page({"AAA": [_bar("2021-01-04T00:00:00Z")]}))
    for _ in range(600):
        http.queue(CA_URL, 200, _ca_page({}, next_token="always-more"))
    with pytest.raises(ah.AlpacaHistoricalExtractionError, match="did not terminate"):
        _extract(http)


# ---------------------------------------------------------------------------
# BKT-RESEARCH-CA-ROLE-IDENTITY-EVIDENCE-01 -- role/identity evidence
# preserved on each corporate-action leg (Patch A). Does NOT touch admission
# policy; classify_corporate_action_type / find_requires_review_events are
# unchanged and covered by the existing tests above.
# ---------------------------------------------------------------------------


def test_effective_windows_acquirer_vs_acquiree_role_distinguishable():
    """Negative control 1: the SAME merger event's two legs must carry
    distinguishable role evidence, not an undifferentiated 'symbol'."""
    raw = {
        "id": "m1",
        "acquirer_symbol": "BIG",
        "acquirer_cusip": "cusip-big",
        "acquiree_symbol": "SML",
        "acquiree_cusip": "cusip-sml",
        "process_date": "2021-01-15",
        "effective_date": "2021-01-15",
    }
    legs = ah._effective_windows_for_entry("stock_merger", raw)
    by_symbol = {leg["symbol"]: leg for leg in legs}
    assert by_symbol["BIG"]["matched_role"] == "acquirer"
    assert by_symbol["BIG"]["matched_cusip"] == "cusip-big"
    assert by_symbol["SML"]["matched_role"] == "acquiree"
    assert by_symbol["SML"]["matched_cusip"] == "cusip-sml"
    # Each leg also records the full event's counterparty identity.
    assert by_symbol["BIG"]["acquiree_symbol"] == "SML"
    assert by_symbol["SML"]["acquirer_symbol"] == "BIG"


def test_effective_windows_unit_split_three_roles_distinguishable():
    raw = {
        "id": "u1",
        "old_symbol": "OLD",
        "new_symbol": "NEW",
        "alternate_symbol": "ALT",
        "old_cusip": "cusip-old",
        "new_cusip": "cusip-new",
        "process_date": "2021-03-01",
    }
    legs = ah._effective_windows_for_entry("unit_split", raw)
    roles = {leg["symbol"]: leg["matched_role"] for leg in legs}
    assert roles == {"OLD": "old_symbol", "NEW": "new_symbol", "ALT": "alternate_symbol"}


def test_effective_windows_missing_identity_field_not_fabricated():
    """Negative control 5: a field the provider did not supply must come
    through as None, never a fabricated placeholder."""
    raw = {"id": "n1", "old_symbol": "OLD", "new_symbol": "META", "old_cusip": "cusip-old", "process_date": "2021-04-01"}
    legs = ah._effective_windows_for_entry("name_change", raw)
    new_leg = next(leg for leg in legs if leg["symbol"] == "META")
    assert new_leg["new_cusip"] is None
    assert new_leg["matched_cusip"] is None  # role=new_symbol -> cusip field=new_cusip, unsupplied


def _name_change_ca_page(*, new_symbol: str, new_cusip: str, event_id: str, process_date: str = "2021-05-01") -> Dict[str, Any]:
    return _ca_page(
        {
            "name_changes": [
                {
                    "id": event_id,
                    "old_symbol": "OLDCO",
                    "new_symbol": new_symbol,
                    "old_cusip": "cusip-oldco",
                    "new_cusip": new_cusip,
                    "process_date": process_date,
                }
            ]
        }
    )


def test_ca_ticker_collision_different_cusips_remain_distinguishable():
    """Negative control 2: two unrelated name-change events that share the
    literal ticker (and, here, the same process_date/effective window) but
    have different CUSIPs must not collapse into the same evidence."""
    from mqk_research.data.bars_provenance import build_corporate_action_evidence, corporate_action_evidence_id

    http_a = FakeHttp()
    http_a.queue(CA_URL, 200, _name_change_ca_page(new_symbol="META", new_cusip="cusip-real-meta", event_id="a1"))
    entries_a, _ = ah.fetch_corporate_actions(
        symbols=["META"], start_utc=WINDOW_START, end_utc=WINDOW_END, credentials=_creds(), http_get=http_a
    )

    http_b = FakeHttp()
    http_b.queue(CA_URL, 200, _name_change_ca_page(new_symbol="META", new_cusip="cusip-unrelated-entity", event_id="b1"))
    entries_b, _ = ah.fetch_corporate_actions(
        symbols=["META"], start_utc=WINDOW_START, end_utc=WINDOW_END, credentials=_creds(), http_get=http_b
    )

    ev_a = build_corporate_action_evidence(
        source_provider_id="alpaca", covered_symbol_universe=["META"],
        coverage_start_utc=WINDOW_START.isoformat(), coverage_end_utc=WINDOW_END.isoformat(),
        corporate_action_entries=entries_a,
    )
    ev_b = build_corporate_action_evidence(
        source_provider_id="alpaca", covered_symbol_universe=["META"],
        coverage_start_utc=WINDOW_START.isoformat(), coverage_end_utc=WINDOW_END.isoformat(),
        corporate_action_entries=entries_b,
    )
    assert corporate_action_evidence_id(ev_a) != corporate_action_evidence_id(ev_b)


def test_ca_evidence_identity_invariant_to_provider_page_order():
    """Negative control 3: two ties on (symbol, action_type,
    effective_start_ts, effective_end_ts) -- distinguishable only by CUSIP --
    must hash identically regardless of which order the provider returns
    them in."""
    from mqk_research.data.bars_provenance import build_corporate_action_evidence, corporate_action_evidence_id

    entry_1 = {
        "id": "n1", "old_symbol": "OLDCO1", "new_symbol": "META", "old_cusip": "c1", "new_cusip": "cusip-real-meta",
        "process_date": "2021-05-01",
    }
    entry_2 = {
        "id": "n2", "old_symbol": "OLDCO2", "new_symbol": "META", "old_cusip": "c2", "new_cusip": "cusip-unrelated",
        "process_date": "2021-05-01",
    }

    http_order_1 = FakeHttp()
    http_order_1.queue(CA_URL, 200, _ca_page({"name_changes": [entry_1, entry_2]}))
    entries_order_1, _ = ah.fetch_corporate_actions(
        symbols=["META"], start_utc=WINDOW_START, end_utc=WINDOW_END, credentials=_creds(), http_get=http_order_1
    )

    http_order_2 = FakeHttp()
    http_order_2.queue(CA_URL, 200, _ca_page({"name_changes": [entry_2, entry_1]}))
    entries_order_2, _ = ah.fetch_corporate_actions(
        symbols=["META"], start_utc=WINDOW_START, end_utc=WINDOW_END, credentials=_creds(), http_get=http_order_2
    )

    def _build(entries: List[Dict[str, Any]]) -> Dict[str, Any]:
        return build_corporate_action_evidence(
            source_provider_id="alpaca", covered_symbol_universe=["META"],
            coverage_start_utc=WINDOW_START.isoformat(), coverage_end_utc=WINDOW_END.isoformat(),
            corporate_action_entries=entries,
        )

    assert corporate_action_evidence_id(_build(entries_order_1)) == corporate_action_evidence_id(_build(entries_order_2))


def test_ca_mutation_of_cusip_changes_evidence_identity():
    """Negative control 4: mutating a resolution-relevant CUSIP field must
    change the corporate-action semantic evidence identity."""
    from mqk_research.data.bars_provenance import build_corporate_action_evidence, corporate_action_evidence_id

    http_a = FakeHttp()
    http_a.queue(CA_URL, 200, _name_change_ca_page(new_symbol="XYZ", new_cusip="cusip-1", event_id="e1"))
    entries_a, _ = ah.fetch_corporate_actions(
        symbols=["XYZ"], start_utc=WINDOW_START, end_utc=WINDOW_END, credentials=_creds(), http_get=http_a
    )

    http_b = FakeHttp()
    http_b.queue(CA_URL, 200, _name_change_ca_page(new_symbol="XYZ", new_cusip="cusip-2", event_id="e1"))
    entries_b, _ = ah.fetch_corporate_actions(
        symbols=["XYZ"], start_utc=WINDOW_START, end_utc=WINDOW_END, credentials=_creds(), http_get=http_b
    )

    def _build(entries: List[Dict[str, Any]]) -> Dict[str, Any]:
        return build_corporate_action_evidence(
            source_provider_id="alpaca", covered_symbol_universe=["XYZ"],
            coverage_start_utc=WINDOW_START.isoformat(), coverage_end_utc=WINDOW_END.isoformat(),
            corporate_action_entries=entries,
        )

    assert corporate_action_evidence_id(_build(entries_a)) != corporate_action_evidence_id(_build(entries_b))

    # Same mutation must also change the SAME role field: role/type mutation alone.
    entries_role_mutated = [dict(e) for e in entries_a]
    for e in entries_role_mutated:
        if e["symbol"] == "XYZ":
            e["matched_role"] = "acquirer"  # falsely reassign role
    assert corporate_action_evidence_id(_build(entries_a)) != corporate_action_evidence_id(_build(entries_role_mutated))


def test_extraction_preserves_role_identity_evidence_end_to_end():
    """Full-pipeline proof: a merger event's role/identity evidence survives
    all the way through _neutral_extract into the manifest's
    corporate_action_evidence -- not narrowed away as it was pre-patch."""
    http = FakeHttp()
    http.queue(BARS_URL, 200, _bars_page({"AAA": [_bar("2021-01-04T00:00:00Z")]}))
    http.queue(
        CA_URL,
        200,
        _ca_page(
            {
                "cash_dividends": [
                    {
                        "id": "d1",
                        "symbol": "AAA",
                        "cusip": "cusip-aaa",
                        "rate": 0.1,
                        "process_date": "2021-01-10",
                        "ex_date": "2021-01-09",
                    }
                ]
            }
        ),
    )
    result = _extract(http)
    entries = result["corporate_action_entries"]
    assert len(entries) == 1
    entry = entries[0]
    assert entry["matched_role"] == "primary"
    assert entry["matched_cusip"] == "cusip-aaa"
    assert entry["provider_event_id"] == "d1"
    assert entry["ex_date"] == "2021-01-09"
    # The same evidence must also be reachable from the manifest's own
    # corporate_action_evidence object, not just the raw entries list.
    manifest_entries = result["manifest"]["corporate_action_evidence"]["corporate_action_entries"]
    assert any(e.get("matched_cusip") == "cusip-aaa" for e in manifest_entries)


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
    result = _extract(http)
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


# ---------------------------------------------------------------------------
# BKT-RESEARCH-CA-AUTHORITY-IDENTITY-V2-01 -- extractor identity bump +
# resolution_policy_fingerprint. See test_source_attestation.py for the
# canonical_source_attestation_content-level V1/V2 identity tests.
# ---------------------------------------------------------------------------


def test_extractor_id_bumped_to_v2_never_v1():
    """Required test 4: a fresh extraction can never claim the V1 identity
    -- EXTRACTOR_ID is now v2, and EXTRACTOR_ID_V1_LEGACY (kept only for
    verifying historical artifacts) is a DIFFERENT, distinguishable
    string."""
    assert ah.EXTRACTOR_ID == "mqk_research.data.alpaca_historical.v2"
    assert ah.EXTRACTOR_ID_V1_LEGACY == "mqk_research.data.alpaca_historical.v1"
    assert ah.EXTRACTOR_ID != ah.EXTRACTOR_ID_V1_LEGACY
    assert ah.DIAGNOSTIC_EXTRACTOR_ID == "mqk_research.data.alpaca_historical.diagnostic_v2"


def test_v2_extractor_ids_match_bars_provenance_mirror():
    """bars_provenance._V2_PLUS_EXTRACTOR_IDS must stay in sync with this
    module's actual EXTRACTOR_ID/DIAGNOSTIC_EXTRACTOR_ID -- if either side
    drifts, ca_resolution_policy_id would either wrongly drop out of a real
    V2 attestation's identity or wrongly leak into a non-V2 one."""
    from mqk_research.data import bars_provenance as bp

    assert bp._V2_PLUS_EXTRACTOR_IDS == {ah.EXTRACTOR_ID, ah.DIAGNOSTIC_EXTRACTOR_ID}


def test_official_extraction_mints_v2_attestation_with_policy_fingerprint(monkeypatch):
    http = FakeHttp()
    _queue_clean_extraction(http)
    monkeypatch.setattr(ah, "_default_http_get", http)
    monkeypatch.setattr(ah, "_utc_now", lambda: pd.Timestamp("2026-08-15T00:00:00Z"))
    result = ah.extract_research_bars_with_provenance(
        symbols=["AAA"], start_utc=WINDOW_START, end_utc=WINDOW_END, asof=ASOF, credentials=_creds()
    )
    attestation = result["manifest"]["source_attestation"]
    assert attestation["extractor_id"] == ah.EXTRACTOR_ID
    assert attestation["ca_resolution_policy_id"] == ah.resolution_policy_fingerprint()


def test_resolution_policy_fingerprint_changes_with_reviewed_registry_content():
    """Required test 2: identical policy version/type sets, but a mutated
    (here: emptied) reviewed-resolution registry -> different policy
    fingerprint, proving a changed/removed reviewed-resolution record
    changes official source identity even with no other code change."""
    default_fp = ah.resolution_policy_fingerprint()
    empty_registry_fp = ah.resolution_policy_fingerprint(reviewed_resolutions=())
    assert default_fp != empty_registry_fp

    # Editing a record's content (even leaving the registry the same length)
    # changes its own resolution_id, and therefore the aggregate fingerprint.
    from mqk_research.data.ca_reviewed_resolutions import RESOLUTION_VERIFIED_ONE_FOR_ONE_SUCCESSOR_SECURITY_CONTINUITY, build_reviewed_resolution

    edited_record = build_reviewed_resolution(
        source_provider_id="alpaca", provider_event_id="00000000-0000-0000-0000-000000000000",
        action_type="stock_merger", requested_symbol="DKNG", requested_role="acquiree",
        matched_symbol_field="acquiree_symbol", process_date="2022-05-06",  # one day off from the real record
        resolution=RESOLUTION_VERIFIED_ONE_FOR_ONE_SUCCESSOR_SECURITY_CONTINUITY,
        evidence_summary="edited fixture", primary_source_references=("ref",),
    )
    edited_registry_fp = ah.resolution_policy_fingerprint(reviewed_resolutions=(edited_record,))
    assert edited_registry_fp != default_fp
    assert edited_registry_fp != empty_registry_fp


def test_resolution_policy_fingerprint_deterministic_and_order_independent():
    fp_a = ah.resolution_policy_fingerprint(reviewed_resolutions=ah.REVIEWED_CA_RESOLUTIONS)
    fp_b = ah.resolution_policy_fingerprint(reviewed_resolutions=tuple(reversed(ah.REVIEWED_CA_RESOLUTIONS)))
    assert fp_a == fp_b


# ---------------------------------------------------------------------------
# BKT-RESEARCH-CA-AUTHORITY-IDENTITY-V2-01-REPAIR-01 (F2/F3)
# ---------------------------------------------------------------------------


def test_trusted_v2_ca_resolution_policy_id_matches_bars_provenance_mirror():
    """bars_provenance.TRUSTED_V2_CA_RESOLUTION_POLICY_ID must stay in sync
    with this module's ACTUAL current resolution_policy_fingerprint() -- if
    they drift, either a real official V2 extraction can never authorize
    registered research (mirror stale/behind), or the trust check silently
    stops meaning anything (mirror never enforced)."""
    from mqk_research.data import bars_provenance as bp

    assert bp.TRUSTED_V2_CA_RESOLUTION_POLICY_ID == ah.resolution_policy_fingerprint()


# ---------------------------------------------------------------------------
# BKT-RESEARCH-CA-POLICY-SINGLE-SOURCE-OF-TRUTH-01 -- proves classifier
# BEHAVIOR and resolution_policy_fingerprint IDENTITY share the exact same
# authoritative value: each test mutates ONE field of MERGER_ACQUIRER_RULE/
# NAME_CHANGE_RULE via monkeypatch (a bare module-global reassignment, the
# same seam classify_corporate_action_resolution/_event_group_key actually
# read at call time -- see the module comment above MergerAcquirerRule in
# alpaca_historical.py) and asserts BOTH the classifier's real admission
# decision changes AND the fingerprint changes. A test that only checked the
# fingerprint (Patch F's original mutation tests) could not distinguish a
# genuinely single-sourced value from a hashed-but-unused decoy.
# ---------------------------------------------------------------------------


def test_policy_fingerprint_changes_when_merger_action_types_mutate():
    """Mutating the merger-acquirer rule's covered action_types changes BOTH
    classifier behavior (a stock_merger acquirer leg stops auto-resolving
    once stock_merger is removed from the authoritative set) AND the policy
    fingerprint."""
    import dataclasses
    import mqk_research.data.alpaca_historical as ah_module

    leg = _merger_leg(event_id="action-types-1", role_symbol="BIG", other_role_symbol="SML")
    assert ah.classify_corporate_action_resolution(leg, reviewed_resolutions=()) == ah.CATEGORY_NO_ADJUSTMENT_REQUIRED_FOR_LEG
    default_fp = ah.resolution_policy_fingerprint()

    original = ah_module.MERGER_ACQUIRER_RULE
    try:
        ah_module.MERGER_ACQUIRER_RULE = dataclasses.replace(original, action_types=frozenset({"cash_merger"}))
        assert ah.classify_corporate_action_resolution(leg, reviewed_resolutions=()) == ah.CATEGORY_REQUIRES_FAIL_CLOSED_REVIEW
        mutated_fp = ah.resolution_policy_fingerprint()
    finally:
        ah_module.MERGER_ACQUIRER_RULE = original
    assert mutated_fp != default_fp


def test_merger_acquirer_required_role_mutation_changes_behavior_and_fingerprint():
    """Required negative control 1: mutating MERGER_ACQUIRER_RULE.
    required_role changes classifier behavior (the SAME acquirer leg that
    previously auto-resolved now requires review) AND the fingerprint."""
    import dataclasses
    import mqk_research.data.alpaca_historical as ah_module

    leg = _merger_leg(event_id="req-role-1", role_symbol="BIG", other_role_symbol="SML")
    assert ah.classify_corporate_action_resolution(leg, reviewed_resolutions=()) == ah.CATEGORY_NO_ADJUSTMENT_REQUIRED_FOR_LEG
    default_fp = ah.resolution_policy_fingerprint()

    original = ah_module.MERGER_ACQUIRER_RULE
    try:
        ah_module.MERGER_ACQUIRER_RULE = dataclasses.replace(original, required_role="acquiree")
        assert ah.classify_corporate_action_resolution(leg, reviewed_resolutions=()) == ah.CATEGORY_REQUIRES_FAIL_CLOSED_REVIEW
        mutated_fp = ah.resolution_policy_fingerprint()
    finally:
        ah_module.MERGER_ACQUIRER_RULE = original
    assert mutated_fp != default_fp


def test_merger_terminal_conflict_role_mutation_changes_behavior_and_fingerprint():
    """Required negative control 2: mutating MERGER_ACQUIRER_RULE.
    terminal_conflict_role changes whether the same-event self-consistency
    check fires AND the fingerprint."""
    import dataclasses
    import mqk_research.data.alpaca_historical as ah_module

    raw = {
        "id": "term-role-mut-1", "acquirer_symbol": "AAA", "acquirer_cusip": "x",
        "acquiree_symbol": "AAA", "acquiree_cusip": "y", "process_date": "2021-01-15",
    }
    legs = ah._effective_windows_for_entry("stock_merger", raw)
    acquirer_leg = next(leg for leg in legs if leg["matched_role"] == "acquirer")
    siblings = [leg for leg in legs if leg is not acquirer_leg]
    assert (
        ah.classify_corporate_action_resolution(acquirer_leg, sibling_legs=siblings, reviewed_resolutions=())
        == ah.CATEGORY_REQUIRES_FAIL_CLOSED_REVIEW
    )
    default_fp = ah.resolution_policy_fingerprint()

    original = ah_module.MERGER_ACQUIRER_RULE
    try:
        ah_module.MERGER_ACQUIRER_RULE = dataclasses.replace(original, terminal_conflict_role="no_such_role")
        assert (
            ah.classify_corporate_action_resolution(acquirer_leg, sibling_legs=siblings, reviewed_resolutions=())
            == ah.CATEGORY_NO_ADJUSTMENT_REQUIRED_FOR_LEG
        )
        mutated_fp = ah.resolution_policy_fingerprint()
    finally:
        ah_module.MERGER_ACQUIRER_RULE = original
    assert mutated_fp != default_fp


def test_name_change_allowed_roles_mutation_changes_behavior_and_fingerprint():
    """Required negative control 3: mutating NAME_CHANGE_RULE.allowed_roles
    changes classifier behavior (the new_symbol leg stops auto-resolving
    once 'new_symbol' is removed from the authoritative allowed roles) AND
    the fingerprint."""
    import dataclasses
    import mqk_research.data.alpaca_historical as ah_module

    leg = _name_change_leg(old_symbol="FB", new_symbol="META", old_cusip="30303M102", new_cusip="30303M102")
    assert ah.classify_corporate_action_resolution(leg, reviewed_resolutions=()) == ah.CATEGORY_VERIFIED_SAME_SECURITY_CONTINUITY
    default_fp = ah.resolution_policy_fingerprint()

    original = ah_module.NAME_CHANGE_RULE
    try:
        ah_module.NAME_CHANGE_RULE = dataclasses.replace(original, allowed_roles=frozenset({"old_symbol"}))
        assert ah.classify_corporate_action_resolution(leg, reviewed_resolutions=()) == ah.CATEGORY_REQUIRES_FAIL_CLOSED_REVIEW
        mutated_fp = ah.resolution_policy_fingerprint()
    finally:
        ah_module.NAME_CHANGE_RULE = original
    assert mutated_fp != default_fp


def test_name_change_cusip_equality_requirement_mutation_changes_behavior_and_fingerprint():
    """Required negative control 4: mutating NAME_CHANGE_RULE.
    require_cusip_equality changes classifier behavior (an unequal-CUSIP
    name change that previously required review now auto-resolves) AND the
    fingerprint."""
    import dataclasses
    import mqk_research.data.alpaca_historical as ah_module

    leg = _name_change_leg(old_symbol="FB", new_symbol="META", old_cusip="30303M102", new_cusip="99999X999")
    assert ah.classify_corporate_action_resolution(leg, reviewed_resolutions=()) == ah.CATEGORY_REQUIRES_FAIL_CLOSED_REVIEW
    default_fp = ah.resolution_policy_fingerprint()

    original = ah_module.NAME_CHANGE_RULE
    try:
        ah_module.NAME_CHANGE_RULE = dataclasses.replace(original, require_cusip_equality=False)
        assert ah.classify_corporate_action_resolution(leg, reviewed_resolutions=()) == ah.CATEGORY_VERIFIED_SAME_SECURITY_CONTINUITY
        mutated_fp = ah.resolution_policy_fingerprint()
    finally:
        ah_module.NAME_CHANGE_RULE = original
    assert mutated_fp != default_fp


def test_event_grouping_identity_mutation_changes_grouping_and_fingerprint():
    """Required negative control 5: mutating MERGER_ACQUIRER_RULE.
    same_event_grouping_identity changes which legs find_requires_review_
    events (the real grouping consumer, via _event_group_key) treats as
    siblings of the SAME event AND changes the fingerprint."""
    import dataclasses
    import mqk_research.data.alpaca_historical as ah_module

    leg_evt_a = {"provider_event_id": "evt-A", "action_type": "stock_merger"}
    leg_evt_b = {"provider_event_id": "evt-B", "action_type": "stock_merger"}
    assert ah._event_group_key(leg_evt_a) != ah._event_group_key(leg_evt_b)

    entries = list(
        ah._effective_windows_for_entry(
            "stock_merger",
            {
                "id": "evt-A", "acquirer_symbol": "AAA", "acquirer_cusip": "c1",
                "acquiree_symbol": "SML1", "acquiree_cusip": "c2", "process_date": "2021-01-15",
            },
        )
    ) + list(
        ah._effective_windows_for_entry(
            "stock_merger",
            {
                "id": "evt-B", "acquirer_symbol": "SML2", "acquirer_cusip": "c3",
                "acquiree_symbol": "AAA", "acquiree_cusip": "c4", "process_date": "2021-01-15",
            },
        )
    )
    window_start = pd.Timestamp("2021-01-01T00:00:00Z")
    window_end = pd.Timestamp("2021-02-01T00:00:00Z")
    universe = ["AAA", "SML1", "SML2"]

    default_hits = ah.find_requires_review_events(
        entries, symbol_universe=universe, start_utc=window_start, end_utc=window_end
    )
    # Default grouping (provider_event_id + action_type): evt-A's AAA-
    # acquirer leg is in a DIFFERENT group from evt-B's AAA-acquiree leg --
    # no conflict, AAA-acquirer resolves cleanly, never appears in hits.
    assert not any(h["matched_symbol"] == "AAA" and h["matched_role"] == "acquirer" for h in default_hits)
    default_fp = ah.resolution_policy_fingerprint()

    original = ah_module.MERGER_ACQUIRER_RULE
    try:
        ah_module.MERGER_ACQUIRER_RULE = dataclasses.replace(original, same_event_grouping_identity=("action_type",))
        mutated_hits = ah.find_requires_review_events(
            entries, symbol_universe=universe, start_utc=window_start, end_utc=window_end
        )
        # Mutated grouping (action_type only): both events collapse into ONE
        # group -- evt-A's AAA-acquirer now sees evt-B's AAA-acquiree as a
        # sibling -> terminal conflict fires -> requires review.
        assert any(h["matched_symbol"] == "AAA" and h["matched_role"] == "acquirer" for h in mutated_hits)
        mutated_fp = ah.resolution_policy_fingerprint()
    finally:
        ah_module.MERGER_ACQUIRER_RULE = original
    assert mutated_fp != default_fp


def test_policy_fingerprint_changes_when_covered_by_adjustment_all_mutates():
    """Required test: mutation of the provider-adjusted covered type set
    changes the policy fingerprint."""
    default_fp = ah.resolution_policy_fingerprint()
    import mqk_research.data.alpaca_historical as ah_module
    original = ah_module._COVERED_BY_ADJUSTMENT_ALL
    try:
        ah_module._COVERED_BY_ADJUSTMENT_ALL = frozenset(original | {"stock_dividend"})
        mutated_fp = ah.resolution_policy_fingerprint()
    finally:
        ah_module._COVERED_BY_ADJUSTMENT_ALL = original
    assert mutated_fp != default_fp


def test_stale_reviewed_resolution_id_fails_closed_during_policy_fingerprint_construction():
    """Required test: a reviewed-resolution record with a stale/tampered
    resolution_id in the registry passed to resolution_policy_fingerprint
    must cause the fingerprint construction ITSELF to fail closed -- never
    silently contribute the stale resolution_id to official source identity."""
    from mqk_research.data.ca_reviewed_resolutions import ReviewedResolutionUnverifiable
    import copy

    tampered = copy.deepcopy(ah.REVIEWED_CA_RESOLUTIONS[0])
    tampered["evidence_summary"] = "mutated without re-minting resolution_id"
    with pytest.raises(ReviewedResolutionUnverifiable):
        ah.resolution_policy_fingerprint(reviewed_resolutions=(tampered,))


def test_category_b_event_still_prevents_official_manifest_minting():
    """Required test 7: an unresolved REQUIRES_FAIL_CLOSED_REVIEW event
    still raises before any manifest -- V2-01 only changed source IDENTITY,
    never weakened the fail-closed admission gate itself."""
    http = FakeHttp()
    http.queue(BARS_URL, 200, _bars_page({"AAA": [_bar("2021-01-04T00:00:00Z")]}))
    http.queue(
        CA_URL,
        200,
        _ca_page(
            {
                "unit_splits": [
                    {"id": "u1", "old_symbol": "AAA", "new_symbol": "AAA", "alternate_symbol": "AAA", "process_date": "2021-01-15"}
                ]
            }
        ),
    )
    with pytest.raises(ah.CorporateActionReviewRequired, match="unit_split"):
        _extract(http)
