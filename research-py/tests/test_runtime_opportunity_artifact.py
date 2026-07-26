"""
RUNTIME-OPPORTUNITY-ALLOCATION-01 Phase B — unit tests for the
runtime-opportunity-set-v1 artifact builder and validator.
"""
from __future__ import annotations

import unittest
from datetime import datetime, timedelta, timezone

from mqk_research.scanner.runtime_opportunity_artifact import (
    MULTI_SYMBOL_HARD_CEILING,
    REASON_CANDIDATE_LIVE_ELIGIBLE,
    REASON_CANDIDATE_NOT_PAPER_ELIGIBLE,
    REASON_DUPLICATE_STRATEGY_ASSIGNMENT,
    REASON_LINEAGE_MISSING,
    REASON_LIVE_APPROVAL_FORBIDDEN,
    REASON_MODE_NOT_PAPER,
    REASON_SCHEMA_INVALID,
    REASON_SCORE_NONCANONICAL,
    REASON_SCORE_NONPOSITIVE,
    REASON_SCORE_SOURCE_INVALID,
    REASON_STALE,
    REASON_SYMBOL_BLANK_OR_DUPLICATE,
    REASON_SYMBOL_STRATEGY_MISMATCH,
    REASON_WATCHLIST_HASH_MISMATCH,
    REASON_WRONG_MARKET_DATE,
    RuntimeOpportunityBuildError,
    build_runtime_opportunity_set,
    candidate_lineage_hash,
    format_canonical_score,
    validate_runtime_opportunity_set,
    watchlist_lineage_hash,
)

MARKET_DATE = "2026-07-26"


def _watchlist(symbols=None, strategy_assignments=None, **overrides) -> dict:
    syms = symbols if symbols is not None else ["AAPL", "MSFT"]
    sa = strategy_assignments if strategy_assignments is not None else {
        s: "intraday_scalper" for s in syms
    }
    w = {
        "schema_version": "watchlist-v2",
        "generated_at_utc": "2026-07-26T12:00:00+00:00",
        "mode": "paper",
        "approved_for_autonomous_paper": True,
        "approved_for_live": False,
        "max_symbols_to_trade": len(syms),
        "max_concurrent_positions": len(syms),
        "symbols": syms,
        "strategy_assignments": sa,
    }
    w.update(overrides)
    return w


def _candidate_record(symbol: str, total_score: float = 0.55, **overrides) -> dict:
    r = {
        "schema_version": "scanner-candidate-v1",
        "scanner_id": "main_scanner",
        "symbol": symbol,
        "generated_at_utc": "2026-07-26T11:55:00+00:00",
        "latest_bar_ts": "2026-07-26T11:55:00+00:00",
        "total_score": total_score,
        "eligible_for_paper": True,
        "eligible_for_live": False,
    }
    r.update(overrides)
    return r


def _build(symbols=None, **kwargs) -> dict:
    syms = symbols if symbols is not None else ["AAPL", "MSFT"]
    watchlist = _watchlist(symbols=syms)
    records = [_candidate_record(s) for s in syms]
    return build_runtime_opportunity_set(
        watchlist=watchlist,
        candidate_records=records,
        market_date=MARKET_DATE,
        timeframe="5m",
        scanner_id="main_scanner",
        scoring_config_hash="abc123",
        generated_at_utc="2026-07-26T12:00:00+00:00",
        **kwargs,
    )


class BuilderTests(unittest.TestCase):
    def test_builds_valid_artifact(self):
        artifact = _build()
        self.assertEqual(artifact["schema_version"], "runtime-opportunity-set-v1")
        self.assertEqual(artifact["mode"], "paper")
        self.assertTrue(artifact["approved_for_autonomous_paper"])
        self.assertFalse(artifact["approved_for_live"])
        self.assertEqual(len(artifact["candidates"]), 2)
        for c in artifact["candidates"]:
            self.assertEqual(c["score_source"], "scanner_total_score")
            self.assertTrue(c["eligible_for_paper"])
            self.assertFalse(c["eligible_for_live"])

    def test_score_is_canonical_and_matches_source(self):
        artifact = _build()
        aapl = next(c for c in artifact["candidates"] if c["symbol"] == "AAPL")
        self.assertEqual(aapl["score"], format_canonical_score(0.55))
        self.assertEqual(aapl["score"], "0.550000")

    def test_rejects_non_v2_watchlist(self):
        watchlist = _watchlist()
        watchlist["schema_version"] = "watchlist-v1"
        with self.assertRaises(RuntimeOpportunityBuildError):
            build_runtime_opportunity_set(
                watchlist=watchlist,
                candidate_records=[_candidate_record("AAPL")],
                market_date=MARKET_DATE,
                timeframe="5m",
                scanner_id="main_scanner",
                scoring_config_hash="abc123",
            )

    def test_rejects_live_approved_watchlist(self):
        watchlist = _watchlist()
        watchlist["approved_for_live"] = True
        with self.assertRaises(RuntimeOpportunityBuildError):
            build_runtime_opportunity_set(
                watchlist=watchlist,
                candidate_records=[_candidate_record("AAPL"), _candidate_record("MSFT")],
                market_date=MARKET_DATE,
                timeframe="5m",
                scanner_id="main_scanner",
                scoring_config_hash="abc123",
            )

    def test_rejects_missing_candidate_record(self):
        watchlist = _watchlist()
        with self.assertRaises(RuntimeOpportunityBuildError):
            build_runtime_opportunity_set(
                watchlist=watchlist,
                candidate_records=[_candidate_record("AAPL")],  # MSFT missing
                market_date=MARKET_DATE,
                timeframe="5m",
                scanner_id="main_scanner",
                scoring_config_hash="abc123",
            )

    def test_rejects_nonpositive_score(self):
        watchlist = _watchlist(symbols=["AAPL"])
        with self.assertRaises(RuntimeOpportunityBuildError):
            build_runtime_opportunity_set(
                watchlist=watchlist,
                candidate_records=[_candidate_record("AAPL", total_score=0.0)],
                market_date=MARKET_DATE,
                timeframe="5m",
                scanner_id="main_scanner",
                scoring_config_hash="abc123",
            )

    def test_rejects_candidate_not_paper_eligible(self):
        watchlist = _watchlist(symbols=["AAPL"])
        with self.assertRaises(RuntimeOpportunityBuildError):
            build_runtime_opportunity_set(
                watchlist=watchlist,
                candidate_records=[_candidate_record("AAPL", eligible_for_paper=False)],
                market_date=MARKET_DATE,
                timeframe="5m",
                scanner_id="main_scanner",
                scoring_config_hash="abc123",
            )

    def test_rejects_symbol_count_above_ceiling(self):
        syms = [f"SYM{i}" for i in range(MULTI_SYMBOL_HARD_CEILING + 1)]
        watchlist = _watchlist(symbols=syms)
        records = [_candidate_record(s) for s in syms]
        with self.assertRaises(RuntimeOpportunityBuildError):
            build_runtime_opportunity_set(
                watchlist=watchlist,
                candidate_records=records,
                market_date=MARKET_DATE,
                timeframe="5m",
                scanner_id="main_scanner",
                scoring_config_hash="abc123",
            )

    def test_lineage_hashes_are_deterministic(self):
        watchlist = _watchlist()
        h1 = watchlist_lineage_hash(watchlist)
        h2 = watchlist_lineage_hash(dict(watchlist))
        self.assertEqual(h1, h2)
        record = _candidate_record("AAPL")
        self.assertEqual(candidate_lineage_hash(record), candidate_lineage_hash(dict(record)))

    def test_build_is_deterministic_for_same_inputs(self):
        a1 = _build()
        a2 = _build()
        self.assertEqual(a1["artifact_id"], a2["artifact_id"])
        self.assertEqual(a1["source_watchlist_hash"], a2["source_watchlist_hash"])


class ValidatorTests(unittest.TestCase):
    def setUp(self):
        self.watchlist = _watchlist()
        self.now = datetime(2026, 7, 26, 12, 5, tzinfo=timezone.utc)

    def _validate(self, artifact):
        return validate_runtime_opportunity_set(
            artifact, watchlist=self.watchlist, now_utc=self.now, market_date=MARKET_DATE
        )

    def test_valid_artifact_passes(self):
        artifact = _build()
        result = self._validate(artifact)
        self.assertTrue(result.valid, result.failure_reasons)
        self.assertEqual(result.candidate_count, 2)

    def test_rejects_wrong_schema(self):
        artifact = _build()
        artifact["schema_version"] = "other-v1"
        result = self._validate(artifact)
        self.assertIn(REASON_SCHEMA_INVALID, result.failure_reasons)

    def test_rejects_non_paper_mode(self):
        artifact = _build()
        artifact["mode"] = "live"
        result = self._validate(artifact)
        self.assertIn(REASON_MODE_NOT_PAPER, result.failure_reasons)

    def test_rejects_live_approval(self):
        artifact = _build()
        artifact["approved_for_live"] = True
        result = self._validate(artifact)
        self.assertIn(REASON_LIVE_APPROVAL_FORBIDDEN, result.failure_reasons)

    def test_rejects_wrong_market_date(self):
        artifact = _build()
        result = validate_runtime_opportunity_set(
            artifact, watchlist=self.watchlist, now_utc=self.now, market_date="2026-01-01"
        )
        self.assertIn(REASON_WRONG_MARKET_DATE, result.failure_reasons)

    def test_rejects_expired_artifact(self):
        artifact = _build()
        stale_now = self.now + timedelta(hours=1)
        result = validate_runtime_opportunity_set(
            artifact, watchlist=self.watchlist, now_utc=stale_now, market_date=MARKET_DATE
        )
        self.assertIn(REASON_STALE, result.failure_reasons)

    def test_rejects_missing_lineage(self):
        artifact = _build()
        del artifact["source_watchlist_hash"]
        result = self._validate(artifact)
        self.assertIn(REASON_LINEAGE_MISSING, result.failure_reasons)

    def test_rejects_watchlist_hash_mismatch(self):
        artifact = _build()
        artifact["source_watchlist_hash"] = "deadbeef"
        result = self._validate(artifact)
        self.assertIn(REASON_WATCHLIST_HASH_MISMATCH, result.failure_reasons)

    def test_rejects_duplicate_symbol(self):
        artifact = _build()
        artifact["candidates"].append(dict(artifact["candidates"][0]))
        result = self._validate(artifact)
        self.assertIn(REASON_SYMBOL_BLANK_OR_DUPLICATE, result.failure_reasons)

    def test_rejects_blank_symbol(self):
        artifact = _build()
        artifact["candidates"][0]["symbol"] = "   "
        result = self._validate(artifact)
        self.assertIn(REASON_SYMBOL_BLANK_OR_DUPLICATE, result.failure_reasons)

    def test_rejects_symbol_strategy_mismatch(self):
        artifact = _build()
        artifact["candidates"][0]["strategy_id"] = "some_other_strategy"
        result = self._validate(artifact)
        self.assertIn(REASON_SYMBOL_STRATEGY_MISMATCH, result.failure_reasons)

    def test_rejects_unknown_symbol(self):
        artifact = _build()
        artifact["candidates"][0]["symbol"] = "ZZZZ"
        result = self._validate(artifact)
        self.assertIn(REASON_SYMBOL_STRATEGY_MISMATCH, result.failure_reasons)

    def test_rejects_noncanonical_score(self):
        artifact = _build()
        artifact["candidates"][0]["score"] = "0.55"  # only 2 fractional digits
        result = self._validate(artifact)
        self.assertIn(REASON_SCORE_NONCANONICAL, result.failure_reasons)

    def test_rejects_scientific_notation_score(self):
        artifact = _build()
        artifact["candidates"][0]["score"] = "5.5e-1"
        result = self._validate(artifact)
        self.assertIn(REASON_SCORE_NONCANONICAL, result.failure_reasons)

    def test_rejects_negative_score(self):
        artifact = _build()
        artifact["candidates"][0]["score"] = "-0.550000"
        result = self._validate(artifact)
        self.assertIn(REASON_SCORE_NONCANONICAL, result.failure_reasons)

    def test_rejects_zero_score(self):
        artifact = _build()
        artifact["candidates"][0]["score"] = "0.000000"
        result = self._validate(artifact)
        self.assertIn(REASON_SCORE_NONPOSITIVE, result.failure_reasons)

    def test_rejects_wrong_score_source(self):
        artifact = _build()
        artifact["candidates"][0]["score_source"] = "made_up"
        result = self._validate(artifact)
        self.assertIn(REASON_SCORE_SOURCE_INVALID, result.failure_reasons)

    def test_rejects_candidate_not_paper_eligible(self):
        artifact = _build()
        artifact["candidates"][0]["eligible_for_paper"] = False
        result = self._validate(artifact)
        self.assertIn(REASON_CANDIDATE_NOT_PAPER_ELIGIBLE, result.failure_reasons)

    def test_rejects_candidate_live_eligible(self):
        artifact = _build()
        artifact["candidates"][0]["eligible_for_live"] = True
        result = self._validate(artifact)
        self.assertIn(REASON_CANDIDATE_LIVE_ELIGIBLE, result.failure_reasons)

    def test_rejects_duplicate_strategy_assignment_same_symbol(self):
        artifact = _build()
        dup = dict(artifact["candidates"][0])
        dup["symbol"] = "AAPL-dup-marker"  # keep symbol check separate from strategy-pair check
        # Force a genuine (symbol, strategy) collision by reusing the same
        # symbol+strategy pair twice without tripping the blank/dup-symbol
        # check first: duplicate the whole entry (symbol identical too).
        artifact["candidates"].append(dict(artifact["candidates"][0]))
        result = self._validate(artifact)
        self.assertIn(REASON_DUPLICATE_STRATEGY_ASSIGNMENT, result.failure_reasons)


if __name__ == "__main__":
    unittest.main()
