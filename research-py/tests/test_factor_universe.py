"""
RESEARCH-POINT-IN-TIME-UNIVERSE-01 -- point-in-time universe negative controls.

Proves: future constituents are excluded before their effective date, removed
constituents are excluded after their effective end, membership outside the
declared coverage window fails closed rather than falling back to a current-
list shortcut, canonical membership identity is order-independent but
semantically sensitive, and factor evaluation rejects an out-of-universe
observation.
"""
from __future__ import annotations

import pandas as pd
import pytest

from mqk_research.factors.universe import (
    UniverseCoverageError,
    UniverseMembershipRecord,
    UniverseMembershipViolation,
    UniverseSpec,
    assert_observations_within_universe,
    universe_identity_binding,
)


def _spec(members=None, coverage_start="2020-01-01T00:00:00+00:00", coverage_end="2025-01-01T00:00:00+00:00") -> UniverseSpec:
    return UniverseSpec(
        universe_name="sp500_pit",
        universe_protocol_version="v1",
        coverage_start_utc=coverage_start,
        coverage_end_utc=coverage_end,
        members=members or [],
        source_identity={"provider": "test_fixture"},
    )


def _member(symbol, effective_from, effective_through=None) -> UniverseMembershipRecord:
    return UniverseMembershipRecord(symbol=symbol, effective_from_utc=effective_from, effective_through_utc=effective_through)


# -- future constituent introduced later is excluded earlier --------------

def test_future_constituent_excluded_before_effective_from():
    universe = _spec(members=[_member("NEWCO", "2023-06-01T00:00:00+00:00")])
    assert "NEWCO" not in universe.members_as_of("2022-01-01T00:00:00+00:00")
    assert "NEWCO" in universe.members_as_of("2023-07-01T00:00:00+00:00")


# -- removed constituent is excluded after effective end ------------------

def test_removed_constituent_excluded_after_effective_through():
    universe = _spec(members=[_member("DELISTCO", "2020-01-01T00:00:00+00:00", "2021-01-01T00:00:00+00:00")])
    assert "DELISTCO" in universe.members_as_of("2020-06-01T00:00:00+00:00")
    assert "DELISTCO" not in universe.members_as_of("2021-06-01T00:00:00+00:00")


# -- unknown historical membership fails closed ----------------------------

def test_timestamp_before_coverage_start_fails_closed():
    universe = _spec(members=[_member("AAPL", "2020-01-01T00:00:00+00:00")])
    with pytest.raises(UniverseCoverageError):
        universe.members_as_of("2010-01-01T00:00:00+00:00")


def test_timestamp_at_or_after_coverage_end_fails_closed():
    universe = _spec(members=[_member("AAPL", "2020-01-01T00:00:00+00:00")])
    with pytest.raises(UniverseCoverageError):
        universe.members_as_of("2025-01-01T00:00:00+00:00")  # exclusive end


# -- current-universe-only shortcut rejected for historical evaluation ----

def test_current_snapshot_cannot_answer_pre_coverage_historical_query():
    # A universe captured only from 2024 onward (as if someone only recorded
    # "today's" list) must NOT be usable as a stand-in for 2015 membership.
    current_only = _spec(
        members=[_member("AAPL", "2024-01-01T00:00:00+00:00")],
        coverage_start="2024-01-01T00:00:00+00:00",
        coverage_end="2024-06-01T00:00:00+00:00",
    )
    with pytest.raises(UniverseCoverageError):
        current_only.members_as_of("2015-01-01T00:00:00+00:00")


# -- same canonical membership reordered -> same universe_id ---------------

def test_reordered_members_same_universe_id():
    a = _spec(members=[_member("AAPL", "2020-01-01T00:00:00+00:00"), _member("MSFT", "2020-01-01T00:00:00+00:00")])
    b = _spec(members=[_member("MSFT", "2020-01-01T00:00:00+00:00"), _member("AAPL", "2020-01-01T00:00:00+00:00")])
    assert a.compute_universe_id() == b.compute_universe_id()


# -- semantic membership change -> different universe_id -------------------

def test_added_member_changes_universe_id():
    base_id = _spec(members=[_member("AAPL", "2020-01-01T00:00:00+00:00")]).compute_universe_id()
    changed_id = _spec(
        members=[_member("AAPL", "2020-01-01T00:00:00+00:00"), _member("MSFT", "2020-01-01T00:00:00+00:00")]
    ).compute_universe_id()
    assert base_id != changed_id


def test_effective_date_change_changes_universe_id():
    base_id = _spec(members=[_member("AAPL", "2020-01-01T00:00:00+00:00")]).compute_universe_id()
    changed_id = _spec(members=[_member("AAPL", "2020-02-01T00:00:00+00:00")]).compute_universe_id()
    assert base_id != changed_id


def test_universe_identity_binding_changes_with_universe_id():
    a = _spec(members=[_member("AAPL", "2020-01-01T00:00:00+00:00")])
    b = _spec(members=[_member("MSFT", "2020-01-01T00:00:00+00:00")])
    assert universe_identity_binding(a) != universe_identity_binding(b)


# -- factor evaluation with out-of-universe symbol fails -------------------

def test_out_of_universe_observation_rejected():
    universe = _spec(members=[_member("AAPL", "2020-01-01T00:00:00+00:00")])
    observations = pd.DataFrame(
        [
            {"symbol": "AAPL", "period_ts_utc": "2021-01-01T00:00:00+00:00"},
            {"symbol": "GHOSTCO", "period_ts_utc": "2021-01-01T00:00:00+00:00"},  # never a member
        ]
    )
    with pytest.raises(UniverseMembershipViolation):
        assert_observations_within_universe(observations, universe)


def test_in_universe_observations_pass():
    universe = _spec(members=[_member("AAPL", "2020-01-01T00:00:00+00:00")])
    observations = pd.DataFrame([{"symbol": "AAPL", "period_ts_utc": "2021-01-01T00:00:00+00:00"}])
    assert_observations_within_universe(observations, universe)  # no raise


# -- malformed spec rejected -------------------------------------------

def test_overlapping_membership_windows_rejected():
    universe = _spec(
        members=[
            _member("AAPL", "2020-01-01T00:00:00+00:00", "2021-06-01T00:00:00+00:00"),
            _member("AAPL", "2021-01-01T00:00:00+00:00"),  # overlaps the first window
        ]
    )
    with pytest.raises(ValueError, match="overlapping"):
        universe.validate()


def test_inverted_coverage_window_rejected():
    with pytest.raises(ValueError, match="coverage_start_utc"):
        _spec(coverage_start="2025-01-01T00:00:00+00:00", coverage_end="2020-01-01T00:00:00+00:00").validate()


def test_member_effective_through_before_from_rejected():
    with pytest.raises(ValueError, match="effective_through_utc"):
        _member("AAPL", "2021-01-01T00:00:00+00:00", "2020-01-01T00:00:00+00:00").validate()


def test_naive_timestamp_rejected():
    with pytest.raises(ValueError, match="UTC offset"):
        _member("AAPL", "2020-01-01T00:00:00").validate()  # no offset
