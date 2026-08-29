"""
BKT-RESEARCH-CA-REVIEWED-SUCCESSOR-RESOLUTION-01 -- tests for the narrow,
explicit, content-addressed reviewed corporate-action resolution registry
(mqk_research.data.ca_reviewed_resolutions).

Covers the mission's required tests:
  1. exact canonical DKNG event resolves
  2. changed process date does not resolve
  3. changed action type does not resolve
  4. changed role does not resolve
  5. changed event fingerprint does not resolve
  6. another DKNG merger does not inherit the resolution
  7. another symbol does not inherit it
  8. missing reviewed-resolution artifact returns to fail-closed behavior
"""
from __future__ import annotations

import copy

import pytest

from mqk_research.data import ca_reviewed_resolutions as crr

DKNG_KWARGS = dict(
    source_provider_id="alpaca",
    action_type="reorganization",
    requested_symbol="DKNG",
    requested_role="primary",
    process_date="2022-05-05",
)


def _synthetic_registry():
    record = crr.build_reviewed_resolution(
        resolution=crr.RESOLUTION_VERIFIED_ONE_FOR_ONE_SUCCESSOR_SECURITY_CONTINUITY,
        evidence_summary="test fixture",
        primary_source_references=("test-fixture-reference",),
        **DKNG_KWARGS,
    )
    return (record,)


# ---------------------------------------------------------------------------
# build_reviewed_resolution
# ---------------------------------------------------------------------------


def test_build_reviewed_resolution_is_content_addressed():
    record = crr.build_reviewed_resolution(
        resolution=crr.RESOLUTION_VERIFIED_ONE_FOR_ONE_SUCCESSOR_SECURITY_CONTINUITY,
        evidence_summary="test",
        primary_source_references=("ref-1",),
        **DKNG_KWARGS,
    )
    assert record["event_fingerprint"] == crr.event_fingerprint(**DKNG_KWARGS)
    assert record["resolution_id"]
    # Rebuilding from the exact same inputs is fully deterministic.
    record_2 = crr.build_reviewed_resolution(
        resolution=crr.RESOLUTION_VERIFIED_ONE_FOR_ONE_SUCCESSOR_SECURITY_CONTINUITY,
        evidence_summary="test",
        primary_source_references=("ref-1",),
        **DKNG_KWARGS,
    )
    assert record["resolution_id"] == record_2["resolution_id"]


def test_build_reviewed_resolution_rejects_unknown_resolution_semantics():
    with pytest.raises(ValueError, match="Unknown reviewed resolution"):
        crr.build_reviewed_resolution(
            resolution="ignore_all_dkng_mergers",
            evidence_summary="test",
            primary_source_references=("ref-1",),
            **DKNG_KWARGS,
        )


def test_build_reviewed_resolution_requires_primary_source_reference():
    with pytest.raises(ValueError, match="primary_source_reference"):
        crr.build_reviewed_resolution(
            resolution=crr.RESOLUTION_VERIFIED_ONE_FOR_ONE_SUCCESSOR_SECURITY_CONTINUITY,
            evidence_summary="test",
            primary_source_references=(),
            **DKNG_KWARGS,
        )


# ---------------------------------------------------------------------------
# find_reviewed_resolution -- required tests 1-8
# ---------------------------------------------------------------------------


def test_exact_canonical_dkng_event_resolves():
    """Required test 1."""
    found = crr.find_reviewed_resolution(registry=crr.REVIEWED_CA_RESOLUTIONS, **DKNG_KWARGS)
    assert found is not None
    assert found["resolution"] == crr.RESOLUTION_VERIFIED_ONE_FOR_ONE_SUCCESSOR_SECURITY_CONTINUITY


def test_changed_process_date_does_not_resolve():
    """Required test 2."""
    kwargs = dict(DKNG_KWARGS, process_date="2022-05-06")
    found = crr.find_reviewed_resolution(registry=crr.REVIEWED_CA_RESOLUTIONS, **kwargs)
    assert found is None


def test_changed_action_type_does_not_resolve():
    """Required test 3."""
    kwargs = dict(DKNG_KWARGS, action_type="stock_merger")
    found = crr.find_reviewed_resolution(registry=crr.REVIEWED_CA_RESOLUTIONS, **kwargs)
    assert found is None


def test_changed_role_does_not_resolve():
    """Required test 4."""
    kwargs = dict(DKNG_KWARGS, requested_role="acquirer")
    found = crr.find_reviewed_resolution(registry=crr.REVIEWED_CA_RESOLUTIONS, **kwargs)
    assert found is None


def test_changed_event_fingerprint_does_not_resolve():
    """Required test 5, case A: a record whose declared event_fingerprint
    was overwritten to an arbitrary value no longer matches ANY query
    fingerprint (including the exact canonical DKNG query) -- treated as a
    plain non-match, never found."""
    registry = _synthetic_registry()
    tampered = copy.deepcopy(registry[0])
    tampered["event_fingerprint"] = "0" * 64
    found = crr.find_reviewed_resolution(registry=(tampered,), **DKNG_KWARGS)
    assert found is None


def test_changed_event_fingerprint_case_b_stale_fingerprint_after_field_edit():
    """Required test 5, case B: a bound field (process_date) is hand-edited
    but event_fingerprint is left stale/unchanged -- the stale fingerprint
    still matches the ORIGINAL query, so the record is found by lookup, but
    self-consistency verification (recomputing from the record's own,
    now-mutated fields) must then refuse it rather than trust it."""
    registry = _synthetic_registry()
    stale = copy.deepcopy(registry[0])
    stale["process_date"] = "2022-06-01"  # event_fingerprint left stale
    with pytest.raises(crr.ReviewedResolutionUnverifiable):
        crr.find_reviewed_resolution(registry=(stale,), **DKNG_KWARGS)


def test_another_dkng_event_does_not_inherit_the_resolution():
    """Required test 6: a second, distinct DKNG event (different
    process_date -- a different corporate action entirely) must not match
    the reviewed record authored for the 2022-05-05 reorganization."""
    another_dkng_event = dict(DKNG_KWARGS, process_date="2023-01-01")
    found = crr.find_reviewed_resolution(registry=crr.REVIEWED_CA_RESOLUTIONS, **another_dkng_event)
    assert found is None


def test_another_symbol_does_not_inherit_it():
    """Required test 7."""
    other_symbol = dict(DKNG_KWARGS, requested_symbol="XYZ")
    found = crr.find_reviewed_resolution(registry=crr.REVIEWED_CA_RESOLUTIONS, **other_symbol)
    assert found is None


def test_missing_reviewed_resolution_artifact_returns_to_fail_closed():
    """Required test 8: an empty/absent registry must return None (fail
    closed), never fabricate a match."""
    found = crr.find_reviewed_resolution(registry=(), **DKNG_KWARGS)
    assert found is None


def test_source_provider_mismatch_does_not_resolve():
    kwargs = dict(DKNG_KWARGS, source_provider_id="some_other_provider")
    found = crr.find_reviewed_resolution(registry=crr.REVIEWED_CA_RESOLUTIONS, **kwargs)
    assert found is None
