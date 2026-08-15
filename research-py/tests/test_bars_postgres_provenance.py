"""
BKT-DATA-PROVENANCE-POINT-IN-TIME-01 — bars_postgres price-provenance tests.

Default coverage is pure unit tests for classify_price_convention /
require_verified_price_provenance (no DB dependency, no IO). The optional
real-DB proof (resolve_price_provenance / history() against this box's
actual paper Postgres) is skipped unless MQK_RUN_DB_PROOF_TEST=1 and
MQK_PAPER_DB_URL are explicitly provided by the operator, matching the
existing convention in test_scanner_market_data_export_db_proof.py.
"""
from __future__ import annotations

import os
import unittest

import pandas as pd
import pytest

from mqk_research.data.adapters.bars_postgres import (
    PRICE_CONVENTION_RAW_UNADJUSTED,
    PRICE_CONVENTION_UNVERIFIABLE,
    BarsQuery,
    PriceProvenanceUnverifiable,
    classify_price_convention,
    history,
    require_verified_price_provenance,
    resolve_price_provenance,
)


# ---------------------------------------------------------------------------
# classify_price_convention — pure, no DB
# ---------------------------------------------------------------------------


def test_verified_close_column_and_provider_yields_raw_unadjusted():
    result = classify_price_convention(close_col="close_micros", provider_ids_observed=frozenset({"alpaca"}))
    assert result == PRICE_CONVENTION_RAW_UNADJUSTED


def test_unknown_provider_is_unverifiable():
    """This is the actual, real-world state of this box's paper DB today for
    the one genuine equity symbol it holds (AAPL): provider_id='unknown' due
    to a separate, already-tracked attribution bug
    (MARKET-DATA-PROVIDER-PROVENANCE-01). This module must fail closed
    rather than assume 'unknown' rows share Alpaca's raw convention."""
    result = classify_price_convention(close_col="close_micros", provider_ids_observed=frozenset({"unknown"}))
    assert result == PRICE_CONVENTION_UNVERIFIABLE


def test_mixed_provider_ids_is_unverifiable():
    result = classify_price_convention(
        close_col="close_micros", provider_ids_observed=frozenset({"alpaca", "unknown"})
    )
    assert result == PRICE_CONVENTION_UNVERIFIABLE


def test_unmapped_provider_is_unverifiable():
    result = classify_price_convention(close_col="close_micros", provider_ids_observed=frozenset({"twelvedata"}))
    assert result == PRICE_CONVENTION_UNVERIFIABLE


def test_empty_provider_set_is_unverifiable():
    result = classify_price_convention(close_col="close_micros", provider_ids_observed=frozenset())
    assert result == PRICE_CONVENTION_UNVERIFIABLE


def test_non_close_micros_column_is_unverifiable_even_with_verified_provider():
    """A future schema change that introduces adj_close (a genuinely
    different price convention) must not silently inherit today's
    raw_unadjusted verdict just because the provider happens to be alpaca --
    the close column itself is part of what's verified."""
    result = classify_price_convention(close_col="adj_close", provider_ids_observed=frozenset({"alpaca"}))
    assert result == PRICE_CONVENTION_UNVERIFIABLE


# ---------------------------------------------------------------------------
# require_verified_price_provenance — fail-closed gate
# ---------------------------------------------------------------------------


def test_require_verified_raises_for_unverifiable_convention():
    provenance = {
        "close_column": "close_micros",
        "provider_metadata_available": True,
        "provider_ids_observed": ["unknown"],
        "price_adjustment_convention": PRICE_CONVENTION_UNVERIFIABLE,
    }
    with pytest.raises(PriceProvenanceUnverifiable):
        require_verified_price_provenance(provenance, context="research dataset build")


def test_require_verified_does_not_raise_for_raw_unadjusted():
    provenance = {
        "close_column": "close_micros",
        "provider_metadata_available": True,
        "provider_ids_observed": ["alpaca"],
        "price_adjustment_convention": PRICE_CONVENTION_RAW_UNADJUSTED,
    }
    require_verified_price_provenance(provenance)  # must not raise


def test_require_verified_negative_control_missing_field_fails_closed():
    """A malformed/incomplete provenance dict (missing the convention field
    entirely) must fail closed, not be silently treated as verified."""
    with pytest.raises(PriceProvenanceUnverifiable):
        require_verified_price_provenance({})


# ---------------------------------------------------------------------------
# Optional real-DB proof (opt-in only)
# ---------------------------------------------------------------------------


class RealDbProvenanceProofTests(unittest.TestCase):
    @unittest.skipUnless(
        os.environ.get("MQK_RUN_DB_PROOF_TEST") == "1" and os.environ.get("MQK_PAPER_DB_URL"),
        "requires MQK_RUN_DB_PROOF_TEST=1 and MQK_PAPER_DB_URL",
    )
    def test_resolve_price_provenance_against_real_paper_db(self):
        from mqk_research.io.pg import PgConfig, make_engine

        engine = make_engine(PgConfig(url=os.environ["MQK_PAPER_DB_URL"]))
        provenance = resolve_price_provenance(
            engine,
            symbols=["AAPL"],
            timeframe="5Min",
            start_utc=pd.Timestamp("2026-03-01T00:00:00Z"),
            end_utc=pd.Timestamp("2026-08-01T00:00:00Z"),
        )
        self.assertEqual(provenance["close_column"], "close_micros")
        self.assertTrue(provenance["provider_metadata_available"])
        # Documents the real, currently-observed state discovered during the
        # P8 investigation (see bars_postgres.py's module comment): this
        # box's AAPL rows are tagged provider_id='unknown', so the gate must
        # report unverifiable, not silently assume raw_unadjusted.
        self.assertIn("unknown", provenance["provider_ids_observed"])
        self.assertEqual(provenance["price_adjustment_convention"], PRICE_CONVENTION_UNVERIFIABLE)

    @unittest.skipUnless(
        os.environ.get("MQK_RUN_DB_PROOF_TEST") == "1" and os.environ.get("MQK_PAPER_DB_URL"),
        "requires MQK_RUN_DB_PROOF_TEST=1 and MQK_PAPER_DB_URL",
    )
    def test_history_attaches_price_provenance_attrs(self):
        from mqk_research.io.pg import PgConfig, make_engine

        engine = make_engine(PgConfig(url=os.environ["MQK_PAPER_DB_URL"]))
        df = history(
            engine,
            BarsQuery(
                symbols=["AAPL"],
                start_utc=pd.Timestamp("2026-03-01T00:00:00Z"),
                end_utc=pd.Timestamp("2026-08-01T00:00:00Z"),
                timeframe="5Min",
            ),
        )
        self.assertIn("price_provenance", df.attrs)
        self.assertEqual(df.attrs["price_provenance"]["close_column"], "close_micros")
