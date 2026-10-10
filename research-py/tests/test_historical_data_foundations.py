from __future__ import annotations

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
