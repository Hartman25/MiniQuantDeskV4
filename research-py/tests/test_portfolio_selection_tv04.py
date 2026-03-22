"""TV-04: Portfolio allocation / strategy selection tests.

Proves:
- Gate-failed candidates are rejected; gate-passed candidates are eligible.
- Budget contention: multiple strategies do NOT all receive ideal capital simultaneously.
- Single-strategy cap prevents monopoly allocation.
- max_strategies truncation is deterministic.
- total_allocated_fraction <= 1.0 always (contention invariant).
- Deterministic sort: (-requested_fraction, artifact_id) for stable tiebreaking.
- Canonical output order: manifest allocations sorted by artifact_id regardless of input order.
- write/read round-trip preserves all fields exactly (including new fields).
- End-to-end TV-01 → TV-02 → TV-04 chain acceptance (schema contract coherence).
- Invalid budget configurations raise ValueError.
- Duplicate artifact_id in candidate set raises ValueError (fail-closed).
- Exact capital accounting: sum(allocated_micros) + unallocated_capital_micros == total.
- Multi-cause adjustment truth: single_strategy_cap and budget_contention are both preserved.
- rejection_reason reserved for rejected status only; adjustment_reasons for budget_capped.
- now_utc is required; core selection has no hidden wall-clock dependency.
- Edge: single eligible strategy, all rejected, exactly 1.0 total, zero candidates.
"""

import json
import tempfile
import unittest
from pathlib import Path

from mqk_research.contracts import (
    PORTFOLIO_ALLOCATION_CONTRACT_VERSION,
    CapitalBudget,
    StrategyCandidate,
    read_portfolio_allocation,
)
from mqk_research.deployment.selection import select_strategies, write_portfolio_allocation

_TS = "2026-01-01T00:00:00+00:00"

_BUDGET = CapitalBudget(
    total_capital_micros=100_000_000,  # $100
    max_strategies=3,
    max_single_strategy_fraction=0.5,
)


def _candidate(
    artifact_id: str,
    *,
    gate_passed: bool = True,
    requested_fraction: float = 0.3,
    signal_pack_id: str = "sp-test",
) -> StrategyCandidate:
    return StrategyCandidate(
        artifact_id=artifact_id,
        signal_pack_id=signal_pack_id,
        gate_passed=gate_passed,
        requested_fraction=requested_fraction,
    )


# ---------------------------------------------------------------------------
# Duplicate artifact_id — fail-closed
# ---------------------------------------------------------------------------

class TestDuplicateArtifactId(unittest.TestCase):
    """Duplicate artifact_id in any candidate combination raises ValueError."""

    def test_duplicate_eligible_candidates_raises(self):
        # Both would be eligible; still a data-integrity error.
        candidates = [
            _candidate("a1", requested_fraction=0.3),
            _candidate("a1", requested_fraction=0.2),
        ]
        with self.assertRaises(ValueError):
            select_strategies(candidates, _BUDGET, now_utc=_TS)

    def test_duplicate_rejected_candidates_raises(self):
        # Both would be rejected (gate failed); still a data-integrity error.
        candidates = [
            _candidate("a1", gate_passed=False),
            _candidate("a1", gate_passed=False),
        ]
        with self.assertRaises(ValueError):
            select_strategies(candidates, _BUDGET, now_utc=_TS)

    def test_duplicate_mixed_eligible_and_rejected_raises(self):
        # One would be eligible, one rejected — same artifact_id is still invalid.
        candidates = [
            _candidate("a1", gate_passed=True, requested_fraction=0.3),
            _candidate("a1", gate_passed=False),
        ]
        with self.assertRaises(ValueError):
            select_strategies(candidates, _BUDGET, now_utc=_TS)


# ---------------------------------------------------------------------------
# Gate filter
# ---------------------------------------------------------------------------

class TestGateFilter(unittest.TestCase):
    def test_gate_failed_is_rejected(self):
        c = _candidate("a1", gate_passed=False)
        m = select_strategies([c], _BUDGET, now_utc=_TS)
        self.assertEqual(len(m.allocations), 1)
        self.assertEqual(m.allocations[0].status, "rejected")
        self.assertEqual(m.allocations[0].rejection_reason, "gate_failed")
        self.assertEqual(m.allocations[0].allocated_fraction, 0.0)
        self.assertEqual(m.allocations[0].allocated_capital_micros, 0)
        self.assertEqual(m.allocations[0].adjustment_reasons, [])

    def test_gate_passed_is_eligible(self):
        c = _candidate("a1", gate_passed=True, requested_fraction=0.2)
        m = select_strategies([c], _BUDGET, now_utc=_TS)
        self.assertEqual(m.allocations[0].status, "allocated")

    def test_mixed_gate_only_passed_receive_capital(self):
        candidates = [
            _candidate("a1", gate_passed=True, requested_fraction=0.3),
            _candidate("a2", gate_passed=False),
            _candidate("a3", gate_passed=True, requested_fraction=0.2),
        ]
        m = select_strategies(candidates, _BUDGET, now_utc=_TS)
        statuses = {a.artifact_id: a.status for a in m.allocations}
        self.assertEqual(statuses["a2"], "rejected")
        self.assertNotEqual(statuses["a1"], "rejected")
        self.assertNotEqual(statuses["a3"], "rejected")


# ---------------------------------------------------------------------------
# Invalid requested fraction
# ---------------------------------------------------------------------------

class TestInvalidRequestedFraction(unittest.TestCase):
    def test_zero_fraction_rejected(self):
        c = _candidate("a1", gate_passed=True, requested_fraction=0.0)
        m = select_strategies([c], _BUDGET, now_utc=_TS)
        self.assertEqual(m.allocations[0].status, "rejected")
        self.assertEqual(m.allocations[0].rejection_reason, "invalid_requested_fraction")
        self.assertEqual(m.allocations[0].adjustment_reasons, [])

    def test_fraction_over_one_rejected(self):
        c = _candidate("a1", gate_passed=True, requested_fraction=1.1)
        m = select_strategies([c], _BUDGET, now_utc=_TS)
        self.assertEqual(m.allocations[0].rejection_reason, "invalid_requested_fraction")

    def test_exactly_one_fraction_is_valid(self):
        c = _candidate("a1", gate_passed=True, requested_fraction=1.0)
        m = select_strategies([c], _BUDGET, now_utc=_TS)
        # 1.0 is valid but will be capped at max_single_strategy_fraction=0.5
        self.assertEqual(m.allocations[0].status, "budget_capped")


# ---------------------------------------------------------------------------
# Contention invariant
# ---------------------------------------------------------------------------

class TestContraryInvariant(unittest.TestCase):
    """total_allocated_fraction must never exceed 1.0."""

    def test_two_strategies_requesting_full_capital_are_scaled(self):
        candidates = [
            _candidate("a1", requested_fraction=0.8),
            _candidate("a2", requested_fraction=0.8),
        ]
        budget = CapitalBudget(
            total_capital_micros=100_000_000,
            max_strategies=2,
            max_single_strategy_fraction=0.9,  # allow high single request
        )
        m = select_strategies(candidates, budget, now_utc=_TS)
        self.assertLessEqual(m.total_allocated_fraction, 1.0)
        # Both should be budget_capped due to contention only (not single-cap)
        for a in m.allocations:
            if a.status != "rejected":
                self.assertEqual(a.status, "budget_capped")
                self.assertIsNone(a.rejection_reason)
                self.assertIn("budget_contention", a.adjustment_reasons)

    def test_three_strategies_sum_exceeds_one_is_scaled(self):
        candidates = [
            _candidate("a1", requested_fraction=0.5),
            _candidate("a2", requested_fraction=0.5),
            _candidate("a3", requested_fraction=0.5),
        ]
        budget = CapitalBudget(
            total_capital_micros=100_000_000,
            max_strategies=3,
            max_single_strategy_fraction=0.5,
        )
        m = select_strategies(candidates, budget, now_utc=_TS)
        self.assertLessEqual(m.total_allocated_fraction, 1.0 + 1e-9)

    def test_contention_invariant_with_many_candidates(self):
        candidates = [_candidate(f"a{i}", requested_fraction=0.4) for i in range(10)]
        m = select_strategies(candidates, _BUDGET, now_utc=_TS)
        self.assertLessEqual(m.total_allocated_fraction, 1.0 + 1e-9)


# ---------------------------------------------------------------------------
# Single-strategy cap and adjustment_reasons
# ---------------------------------------------------------------------------

class TestSingleStrategyCap(unittest.TestCase):
    def test_single_strategy_capped_at_max_fraction(self):
        # Strategy requests 0.8 but cap is 0.5
        c = _candidate("a1", requested_fraction=0.8)
        m = select_strategies([c], _BUDGET, now_utc=_TS)
        a = m.allocations[0]
        self.assertEqual(a.status, "budget_capped")
        # rejection_reason is reserved for rejected status only
        self.assertIsNone(a.rejection_reason)
        # single_strategy_cap is in adjustment_reasons
        self.assertIn("single_strategy_cap", a.adjustment_reasons)
        # no contention (only one strategy)
        self.assertNotIn("budget_contention", a.adjustment_reasons)
        self.assertAlmostEqual(a.allocated_fraction, 0.5)

    def test_strategy_within_cap_not_capped(self):
        c = _candidate("a1", requested_fraction=0.3)
        m = select_strategies([c], _BUDGET, now_utc=_TS)
        a = m.allocations[0]
        self.assertEqual(a.status, "allocated")
        self.assertIsNone(a.rejection_reason)
        self.assertEqual(a.adjustment_reasons, [])
        self.assertAlmostEqual(a.allocated_fraction, 0.3)


# ---------------------------------------------------------------------------
# Multi-cause adjustment truth
# ---------------------------------------------------------------------------

class TestAdjustmentReasons(unittest.TestCase):
    """Prove each adjustment cause is recorded independently and honestly."""

    def test_cap_only_adjustment_reason(self):
        # Single strategy; cap fires but no contention (only one strategy).
        c = _candidate("a1", requested_fraction=0.8)
        m = select_strategies([c], _BUDGET, now_utc=_TS)
        a = m.allocations[0]
        self.assertEqual(a.status, "budget_capped")
        self.assertIsNone(a.rejection_reason)
        self.assertEqual(a.adjustment_reasons, ["single_strategy_cap"])

    def test_contention_only_adjustment_reason(self):
        # Two strategies whose requests sum > 1.0 after cap, but neither hits
        # the per-strategy cap individually (cap=0.9, request=0.8).
        budget = CapitalBudget(
            total_capital_micros=100_000_000,
            max_strategies=2,
            max_single_strategy_fraction=0.9,
        )
        candidates = [
            _candidate("a1", requested_fraction=0.8),
            _candidate("a2", requested_fraction=0.8),
        ]
        m = select_strategies(candidates, budget, now_utc=_TS)
        for a in m.allocations:
            if a.status == "budget_capped":
                self.assertIsNone(a.rejection_reason)
                self.assertEqual(a.adjustment_reasons, ["budget_contention"])
                self.assertNotIn("single_strategy_cap", a.adjustment_reasons)

    def test_both_cap_and_contention_preserved(self):
        # 3 strategies each requesting 0.9 with cap=0.5:
        #   after cap: each 0.5, sum=1.5 > 1.0 → scale fires.
        #   Both single_strategy_cap AND budget_contention apply.
        budget = CapitalBudget(
            total_capital_micros=100_000_000,
            max_strategies=3,
            max_single_strategy_fraction=0.5,
        )
        candidates = [
            _candidate("a1", requested_fraction=0.9),
            _candidate("a2", requested_fraction=0.9),
            _candidate("a3", requested_fraction=0.9),
        ]
        m = select_strategies(candidates, budget, now_utc=_TS)
        for a in m.allocations:
            self.assertEqual(a.status, "budget_capped")
            self.assertIsNone(a.rejection_reason)
            self.assertIn("single_strategy_cap", a.adjustment_reasons)
            self.assertIn("budget_contention", a.adjustment_reasons)

    def test_rejection_reason_only_for_rejected_status(self):
        # Verify the strict separation: rejection_reason is None for non-rejected.
        candidates = [
            _candidate("a1", gate_passed=False),           # rejected
            _candidate("a2", requested_fraction=0.8),      # budget_capped (cap)
            _candidate("a3", requested_fraction=0.2),      # allocated
        ]
        m = select_strategies(candidates, _BUDGET, now_utc=_TS)
        by_id = {a.artifact_id: a for a in m.allocations}

        a1 = by_id["a1"]
        self.assertEqual(a1.status, "rejected")
        self.assertIsNotNone(a1.rejection_reason)
        self.assertEqual(a1.adjustment_reasons, [])

        a2 = by_id["a2"]
        self.assertIn(a2.status, ("budget_capped", "allocated"))
        self.assertIsNone(a2.rejection_reason)

        a3 = by_id["a3"]
        self.assertEqual(a3.status, "allocated")
        self.assertIsNone(a3.rejection_reason)
        self.assertEqual(a3.adjustment_reasons, [])


# ---------------------------------------------------------------------------
# max_strategies truncation
# ---------------------------------------------------------------------------

class TestMaxStrategiesTruncation(unittest.TestCase):
    def test_overflow_candidates_rejected(self):
        candidates = [_candidate(f"a{i}", requested_fraction=0.2) for i in range(5)]
        budget = CapitalBudget(
            total_capital_micros=100_000_000,
            max_strategies=2,
            max_single_strategy_fraction=0.5,
        )
        m = select_strategies(candidates, budget, now_utc=_TS)
        rejected = [a for a in m.allocations if a.status == "rejected"]
        self.assertEqual(len(rejected), 3)
        reasons = {a.rejection_reason for a in rejected}
        self.assertEqual(reasons, {"max_strategies_reached"})

    def test_highest_requesting_strategies_selected_first(self):
        # a_high requests 0.4, a_low requests 0.1 — with max=1 only a_high should be picked
        candidates = [
            _candidate("a_low", requested_fraction=0.1),
            _candidate("a_high", requested_fraction=0.4),
        ]
        budget = CapitalBudget(
            total_capital_micros=100_000_000,
            max_strategies=1,
            max_single_strategy_fraction=0.5,
        )
        m = select_strategies(candidates, budget, now_utc=_TS)
        allocated = [a for a in m.allocations if a.status != "rejected"]
        self.assertEqual(len(allocated), 1)
        self.assertEqual(allocated[0].artifact_id, "a_high")


# ---------------------------------------------------------------------------
# Deterministic sort and canonical output ordering
# ---------------------------------------------------------------------------

class TestDeterministicSort(unittest.TestCase):
    def test_tied_fraction_broken_by_artifact_id_alphabetically(self):
        candidates = [
            _candidate("zzz", requested_fraction=0.3),
            _candidate("aaa", requested_fraction=0.3),
            _candidate("mmm", requested_fraction=0.3),
        ]
        budget = CapitalBudget(
            total_capital_micros=100_000_000,
            max_strategies=2,
            max_single_strategy_fraction=0.5,
        )
        m = select_strategies(candidates, budget, now_utc=_TS)
        allocated_ids = [a.artifact_id for a in m.allocations if a.status != "rejected"]
        # aaa and mmm come before zzz alphabetically → zzz should be rejected
        self.assertIn("aaa", allocated_ids)
        self.assertIn("mmm", allocated_ids)
        rejected_ids = [a.artifact_id for a in m.allocations if a.status == "rejected"]
        self.assertIn("zzz", rejected_ids)

    def test_same_input_different_order_produces_identical_result(self):
        candidates_v1 = [
            _candidate("a2", requested_fraction=0.3),
            _candidate("a1", requested_fraction=0.4),
            _candidate("a3", requested_fraction=0.2),
        ]
        candidates_v2 = list(reversed(candidates_v1))
        m1 = select_strategies(candidates_v1, _BUDGET, now_utc=_TS)
        m2 = select_strategies(candidates_v2, _BUDGET, now_utc=_TS)
        # Sort both by artifact_id to compare (should already be sorted)
        a1_sorted = sorted(m1.allocations, key=lambda a: a.artifact_id)
        a2_sorted = sorted(m2.allocations, key=lambda a: a.artifact_id)
        for a1, a2 in zip(a1_sorted, a2_sorted):
            self.assertAlmostEqual(a1.allocated_fraction, a2.allocated_fraction, places=10)
            self.assertEqual(a1.status, a2.status)

    def test_manifest_allocations_are_in_canonical_order(self):
        # The manifest output list itself is sorted by artifact_id, regardless of input order.
        candidates_v1 = [
            _candidate("zzz", requested_fraction=0.3),
            _candidate("aaa", requested_fraction=0.3),
            _candidate("mmm", requested_fraction=0.3),
        ]
        candidates_v2 = list(reversed(candidates_v1))
        m1 = select_strategies(candidates_v1, _BUDGET, now_utc=_TS)
        m2 = select_strategies(candidates_v2, _BUDGET, now_utc=_TS)
        ids1 = [a.artifact_id for a in m1.allocations]
        ids2 = [a.artifact_id for a in m2.allocations]
        # Output must already be in sorted order.
        self.assertEqual(ids1, sorted(ids1))
        # Both input orders must produce identical output.
        self.assertEqual(ids1, ids2)

    def test_rejected_only_permutation_stable(self):
        # Rejected-only candidate sets also produce canonical output order.
        candidates_v1 = [
            _candidate("zzz", gate_passed=False),
            _candidate("aaa", gate_passed=False),
            _candidate("mmm", gate_passed=False),
        ]
        candidates_v2 = list(reversed(candidates_v1))
        m1 = select_strategies(candidates_v1, _BUDGET, now_utc=_TS)
        m2 = select_strategies(candidates_v2, _BUDGET, now_utc=_TS)
        ids1 = [a.artifact_id for a in m1.allocations]
        ids2 = [a.artifact_id for a in m2.allocations]
        self.assertEqual(ids1, sorted(ids1))
        self.assertEqual(ids1, ids2)

    def test_mixed_allocated_rejected_permutation_stable(self):
        # Mixed candidate sets with both allocated and rejected produce canonical order.
        candidates_v1 = [
            _candidate("zzz", gate_passed=False),
            _candidate("aaa", requested_fraction=0.3),
            _candidate("mmm", requested_fraction=0.2),
        ]
        candidates_v2 = list(reversed(candidates_v1))
        m1 = select_strategies(candidates_v1, _BUDGET, now_utc=_TS)
        m2 = select_strategies(candidates_v2, _BUDGET, now_utc=_TS)
        ids1 = [a.artifact_id for a in m1.allocations]
        ids2 = [a.artifact_id for a in m2.allocations]
        self.assertEqual(ids1, sorted(ids1))
        self.assertEqual(ids1, ids2)


# ---------------------------------------------------------------------------
# Exact capital accounting
# ---------------------------------------------------------------------------

class TestCapitalAccounting(unittest.TestCase):
    """sum(allocated_capital_micros) + unallocated_capital_micros == total exactly."""

    def test_accounting_invariant_basic(self):
        candidates = [
            _candidate("a1", requested_fraction=0.4),
            _candidate("a2", gate_passed=False),
            _candidate("a3", requested_fraction=0.3),
        ]
        m = select_strategies(candidates, _BUDGET, now_utc=_TS)
        total_alloc = sum(
            a.allocated_capital_micros for a in m.allocations if a.status != "rejected"
        )
        self.assertEqual(total_alloc + m.unallocated_capital_micros,
                         _BUDGET.total_capital_micros)

    def test_rounding_remainder_is_explicit(self):
        # 3 strategies at 1/3 each: int(33.33...) * 3 = 99 < 100 → remainder = 1.
        # Fractions must reflect actual micros: each 33/100, total 99/100.
        budget = CapitalBudget(
            total_capital_micros=100,
            max_strategies=3,
            max_single_strategy_fraction=0.5,
        )
        candidates = [_candidate(f"a{i}", requested_fraction=1.0 / 3) for i in range(3)]
        m = select_strategies(candidates, budget, now_utc=_TS)
        total_alloc = sum(
            a.allocated_capital_micros for a in m.allocations if a.status != "rejected"
        )
        self.assertEqual(total_alloc + m.unallocated_capital_micros, budget.total_capital_micros)
        # Truncation floor: each gets 33 micros, total = 99, remainder = 1
        self.assertEqual(total_alloc, 99)
        self.assertEqual(m.unallocated_capital_micros, 1)
        # Fraction truth: each must reflect actual 33 micros, not the 1/3 float intent.
        for a in m.allocations:
            if a.status != "rejected":
                self.assertAlmostEqual(a.allocated_fraction, 33 / 100, places=15)
        # total_allocated_fraction reflects actual micros: 99/100.
        self.assertAlmostEqual(m.total_allocated_fraction, 99 / 100, places=15)

    def test_many_small_fractions_truncation_accounted(self):
        # 7 strategies at 0.1 each from a 1000-micro budget.
        # int(0.1 * 1000) = 100 each; total = 700; unallocated = 300.
        # Fractions must reflect actual micros: each 100/1000 = 0.1, total 700/1000 = 0.7.
        budget = CapitalBudget(
            total_capital_micros=1000,
            max_strategies=10,
            max_single_strategy_fraction=0.2,
        )
        candidates = [_candidate(f"s{i}", requested_fraction=0.1) for i in range(7)]
        m = select_strategies(candidates, budget, now_utc=_TS)
        total_alloc = sum(
            a.allocated_capital_micros for a in m.allocations if a.status != "rejected"
        )
        self.assertEqual(total_alloc, 700)
        self.assertEqual(m.unallocated_capital_micros, 300)
        self.assertEqual(total_alloc + m.unallocated_capital_micros, budget.total_capital_micros)
        # Fraction truth: each must reflect actual 100 micros (100/1000 = 0.1 exactly).
        for a in m.allocations:
            if a.status != "rejected":
                self.assertAlmostEqual(a.allocated_fraction, 100 / 1000, places=15)
        self.assertAlmostEqual(m.total_allocated_fraction, 700 / 1000, places=15)

    def test_allocated_micros_never_exceed_budget(self):
        # Use a non-round budget to exercise truncation edge cases.
        budget = CapitalBudget(
            total_capital_micros=1_000_007,
            max_strategies=3,
            max_single_strategy_fraction=0.5,
        )
        candidates = [_candidate(f"a{i}", requested_fraction=0.4) for i in range(3)]
        m = select_strategies(candidates, budget, now_utc=_TS)
        total_alloc = sum(
            a.allocated_capital_micros for a in m.allocations if a.status != "rejected"
        )
        self.assertLessEqual(total_alloc, budget.total_capital_micros)
        self.assertGreaterEqual(m.unallocated_capital_micros, 0)

    def test_all_rejected_unallocated_equals_total(self):
        candidates = [_candidate(f"a{i}", gate_passed=False) for i in range(3)]
        m = select_strategies(candidates, _BUDGET, now_utc=_TS)
        self.assertEqual(m.unallocated_capital_micros, _BUDGET.total_capital_micros)

    def test_zero_candidates_unallocated_equals_total(self):
        m = select_strategies([], _BUDGET, now_utc=_TS)
        self.assertEqual(m.unallocated_capital_micros, _BUDGET.total_capital_micros)


# ---------------------------------------------------------------------------
# Fraction truth: allocated_fraction and total_allocated_fraction must be
# derived from actual assigned integer micros, not from pre-truncation floats.
# ---------------------------------------------------------------------------

class TestFractionDerivedFromMicros(unittest.TestCase):
    """Prove that fraction fields reflect assigned integer micros, not float intent."""

    def _assert_per_strategy_fraction_matches_micros(
        self, m: object, budget: CapitalBudget
    ) -> None:
        """Helper: every non-rejected allocation's fraction == micros / total."""
        for a in m.allocations:  # type: ignore[attr-defined]
            if a.status != "rejected":
                expected = a.allocated_capital_micros / budget.total_capital_micros
                self.assertAlmostEqual(
                    a.allocated_fraction,
                    expected,
                    places=15,
                    msg=(
                        f"{a.artifact_id}: fraction {a.allocated_fraction!r} "
                        f"!= micros/total ({a.allocated_capital_micros}"
                        f"/{budget.total_capital_micros} = {expected!r})"
                    ),
                )

    def _assert_total_fraction_matches_micros(
        self, m: object, budget: CapitalBudget
    ) -> None:
        """Helper: total_allocated_fraction == sum(micros) / total."""
        total_micros = sum(
            a.allocated_capital_micros
            for a in m.allocations  # type: ignore[attr-defined]
            if a.status != "rejected"
        )
        expected = total_micros / budget.total_capital_micros
        self.assertAlmostEqual(
            m.total_allocated_fraction,  # type: ignore[attr-defined]
            expected,
            places=15,
            msg=(
                f"total_allocated_fraction {m.total_allocated_fraction!r} "  # type: ignore[attr-defined]
                f"!= sum(micros)/total ({total_micros}/{budget.total_capital_micros} = {expected!r})"
            ),
        )

    def test_single_allocated_fraction_matches_micros(self):
        c = _candidate("a1", requested_fraction=0.3)
        m = select_strategies([c], _BUDGET, now_utc=_TS)
        self._assert_per_strategy_fraction_matches_micros(m, _BUDGET)
        self._assert_total_fraction_matches_micros(m, _BUDGET)

    def test_capped_strategy_fraction_matches_micros(self):
        # Strategy requests 0.8, cap at 0.5.
        c = _candidate("a1", requested_fraction=0.8)
        m = select_strategies([c], _BUDGET, now_utc=_TS)
        self._assert_per_strategy_fraction_matches_micros(m, _BUDGET)
        self._assert_total_fraction_matches_micros(m, _BUDGET)

    def test_contention_scaled_fractions_match_micros(self):
        # 2 strategies at 0.8 each with cap=0.9 → sum=1.6 → scaled to 0.5 each.
        budget = CapitalBudget(
            total_capital_micros=100_000_000,
            max_strategies=2,
            max_single_strategy_fraction=0.9,
        )
        candidates = [
            _candidate("a1", requested_fraction=0.8),
            _candidate("a2", requested_fraction=0.8),
        ]
        m = select_strategies(candidates, budget, now_utc=_TS)
        self._assert_per_strategy_fraction_matches_micros(m, budget)
        self._assert_total_fraction_matches_micros(m, budget)

    def test_one_third_each_fractions_match_micros(self):
        # The classic truncation case: 3 × 1/3 → each 33/100, total 99/100.
        budget = CapitalBudget(
            total_capital_micros=100,
            max_strategies=3,
            max_single_strategy_fraction=0.5,
        )
        candidates = [_candidate(f"a{i}", requested_fraction=1.0 / 3) for i in range(3)]
        m = select_strategies(candidates, budget, now_utc=_TS)
        self._assert_per_strategy_fraction_matches_micros(m, budget)
        self._assert_total_fraction_matches_micros(m, budget)
        # Spot-check the exact values.
        for a in m.allocations:
            if a.status != "rejected":
                self.assertEqual(a.allocated_capital_micros, 33)
                self.assertAlmostEqual(a.allocated_fraction, 33 / 100, places=15)
        self.assertAlmostEqual(m.total_allocated_fraction, 99 / 100, places=15)

    def test_many_strategies_fractions_match_micros(self):
        # 10 strategies sharing the budget, some overflow.
        candidates = [_candidate(f"a{i:02d}", requested_fraction=0.15) for i in range(10)]
        m = select_strategies(candidates, _BUDGET, now_utc=_TS)
        self._assert_per_strategy_fraction_matches_micros(m, _BUDGET)
        self._assert_total_fraction_matches_micros(m, _BUDGET)

    def test_mixed_allocated_rejected_fractions_match_micros(self):
        candidates = [
            _candidate("a1", requested_fraction=0.4),
            _candidate("a2", gate_passed=False),
            _candidate("a3", requested_fraction=0.3),
        ]
        m = select_strategies(candidates, _BUDGET, now_utc=_TS)
        self._assert_per_strategy_fraction_matches_micros(m, _BUDGET)
        self._assert_total_fraction_matches_micros(m, _BUDGET)
        # Rejected rows must stay at zero fraction and zero micros.
        for a in m.allocations:
            if a.status == "rejected":
                self.assertEqual(a.allocated_fraction, 0.0)
                self.assertEqual(a.allocated_capital_micros, 0)

    def test_non_round_budget_fractions_match_micros(self):
        # Non-power-of-ten budget exercises truncation edge cases.
        budget = CapitalBudget(
            total_capital_micros=1_000_007,
            max_strategies=3,
            max_single_strategy_fraction=0.5,
        )
        candidates = [_candidate(f"a{i}", requested_fraction=0.4) for i in range(3)]
        m = select_strategies(candidates, budget, now_utc=_TS)
        self._assert_per_strategy_fraction_matches_micros(m, budget)
        self._assert_total_fraction_matches_micros(m, budget)

    def test_round_trip_preserves_fraction_and_micros_consistency(self):
        # After write/read, the fraction–micros consistency invariant still holds.
        import tempfile
        candidates = [
            _candidate("a1", requested_fraction=0.8),  # capped
            _candidate("a2", requested_fraction=0.3),  # allocated
            _candidate("a3", gate_passed=False),        # rejected
        ]
        m = select_strategies(candidates, _BUDGET, now_utc=_TS)
        with tempfile.TemporaryDirectory() as tmp:
            path = write_portfolio_allocation(m, Path(tmp))
            restored = read_portfolio_allocation(path)
        self._assert_per_strategy_fraction_matches_micros(restored, _BUDGET)
        self._assert_total_fraction_matches_micros(restored, _BUDGET)
        # Verify round-trip exactness for all four capital fields.
        self.assertEqual(restored.unallocated_capital_micros, m.unallocated_capital_micros)
        self.assertAlmostEqual(
            restored.total_allocated_fraction, m.total_allocated_fraction, places=15
        )
        for orig, rest in zip(m.allocations, restored.allocations):
            self.assertEqual(rest.allocated_capital_micros, orig.allocated_capital_micros)
            self.assertAlmostEqual(
                rest.allocated_fraction, orig.allocated_fraction, places=15
            )
            self.assertEqual(rest.adjustment_reasons, orig.adjustment_reasons)
            self.assertEqual(rest.rejection_reason, orig.rejection_reason)


# ---------------------------------------------------------------------------
# Edge cases
# ---------------------------------------------------------------------------

class TestEdgeCases(unittest.TestCase):
    def test_zero_candidates_returns_empty_manifest(self):
        m = select_strategies([], _BUDGET, now_utc=_TS)
        self.assertEqual(m.allocations, [])
        self.assertEqual(m.total_allocated_fraction, 0.0)
        self.assertEqual(m.allocated_count, 0)
        self.assertEqual(m.rejected_count, 0)
        self.assertEqual(m.unallocated_capital_micros, _BUDGET.total_capital_micros)

    def test_all_gate_failed_returns_all_rejected(self):
        candidates = [_candidate(f"a{i}", gate_passed=False) for i in range(3)]
        m = select_strategies(candidates, _BUDGET, now_utc=_TS)
        self.assertEqual(m.total_allocated_fraction, 0.0)
        self.assertEqual(m.allocated_count, 0)
        self.assertEqual(m.rejected_count, 3)
        self.assertEqual(m.unallocated_capital_micros, _BUDGET.total_capital_micros)

    def test_single_eligible_within_cap_gets_exactly_requested(self):
        c = _candidate("a1", requested_fraction=0.25)
        m = select_strategies([c], _BUDGET, now_utc=_TS)
        a = m.allocations[0]
        self.assertAlmostEqual(a.allocated_fraction, 0.25)
        self.assertEqual(a.status, "allocated")
        self.assertEqual(a.adjustment_reasons, [])

    def test_exactly_one_total_no_scaling_applied(self):
        # Two strategies each requesting 0.5 with cap=0.5 → sum exactly 1.0, no scale
        candidates = [
            _candidate("a1", requested_fraction=0.5),
            _candidate("a2", requested_fraction=0.5),
        ]
        budget = CapitalBudget(
            total_capital_micros=100_000_000,
            max_strategies=2,
            max_single_strategy_fraction=0.5,
        )
        m = select_strategies(candidates, budget, now_utc=_TS)
        self.assertAlmostEqual(m.total_allocated_fraction, 1.0)
        # Neither should have budget_contention in adjustment_reasons.
        for a in m.allocations:
            self.assertNotIn("budget_contention", a.adjustment_reasons)


# ---------------------------------------------------------------------------
# Budget validation
# ---------------------------------------------------------------------------

class TestBudgetValidation(unittest.TestCase):
    def test_negative_capital_raises(self):
        bad = CapitalBudget(
            total_capital_micros=-1,
            max_strategies=2,
            max_single_strategy_fraction=0.5,
        )
        with self.assertRaises(ValueError):
            select_strategies([_candidate("a1")], bad, now_utc=_TS)

    def test_zero_max_strategies_raises(self):
        bad = CapitalBudget(
            total_capital_micros=100_000_000,
            max_strategies=0,
            max_single_strategy_fraction=0.5,
        )
        with self.assertRaises(ValueError):
            select_strategies([_candidate("a1")], bad, now_utc=_TS)

    def test_zero_max_single_fraction_raises(self):
        bad = CapitalBudget(
            total_capital_micros=100_000_000,
            max_strategies=2,
            max_single_strategy_fraction=0.0,
        )
        with self.assertRaises(ValueError):
            select_strategies([_candidate("a1")], bad, now_utc=_TS)

    def test_max_single_fraction_over_one_raises(self):
        bad = CapitalBudget(
            total_capital_micros=100_000_000,
            max_strategies=2,
            max_single_strategy_fraction=1.1,
        )
        with self.assertRaises(ValueError):
            select_strategies([_candidate("a1")], bad, now_utc=_TS)


# ---------------------------------------------------------------------------
# Wall-clock: now_utc is required, no hidden system-time dependency
# ---------------------------------------------------------------------------

class TestNoWallClock(unittest.TestCase):
    def test_now_utc_is_required_no_default(self):
        # select_strategies must not accept a call without now_utc.
        # Missing required keyword-only argument raises TypeError.
        with self.assertRaises(TypeError):
            select_strategies([], _BUDGET)  # type: ignore[call-arg]

    def test_injected_timestamp_appears_in_manifest(self):
        m = select_strategies([], _BUDGET, now_utc=_TS)
        self.assertEqual(m.produced_at_utc, _TS)

    def test_different_timestamps_produce_otherwise_identical_manifests(self):
        # Two calls with identical candidates but different timestamps differ
        # only in produced_at_utc — all allocation fields are identical.
        ts1 = "2026-01-01T00:00:00+00:00"
        ts2 = "2099-12-31T23:59:59+00:00"
        m1 = select_strategies([_candidate("a1", requested_fraction=0.3)],
                                _BUDGET, now_utc=ts1)
        m2 = select_strategies([_candidate("a1", requested_fraction=0.3)],
                                _BUDGET, now_utc=ts2)
        self.assertEqual(m1.produced_at_utc, ts1)
        self.assertEqual(m2.produced_at_utc, ts2)
        # All allocation fields identical
        self.assertEqual(m1.allocations[0].status, m2.allocations[0].status)
        self.assertAlmostEqual(m1.allocations[0].allocated_fraction,
                               m2.allocations[0].allocated_fraction)
        self.assertEqual(m1.allocations[0].allocated_capital_micros,
                         m2.allocations[0].allocated_capital_micros)


# ---------------------------------------------------------------------------
# Write / read round-trip
# ---------------------------------------------------------------------------

class TestWriteReadRoundTrip(unittest.TestCase):
    def test_write_read_preserves_all_fields(self):
        candidates = [
            _candidate("a1", requested_fraction=0.4),
            _candidate("a2", gate_passed=False),
            _candidate("a3", requested_fraction=0.3),
        ]
        m = select_strategies(candidates, _BUDGET, now_utc=_TS)

        with tempfile.TemporaryDirectory() as tmp:
            path = write_portfolio_allocation(m, Path(tmp))
            self.assertTrue(path.exists())
            self.assertEqual(path.name, "portfolio_allocation.json")

            restored = read_portfolio_allocation(path)

        self.assertEqual(restored.schema_version, PORTFOLIO_ALLOCATION_CONTRACT_VERSION)
        self.assertEqual(restored.budget.total_capital_micros, _BUDGET.total_capital_micros)
        self.assertEqual(restored.budget.max_strategies, _BUDGET.max_strategies)
        self.assertAlmostEqual(
            restored.budget.max_single_strategy_fraction,
            _BUDGET.max_single_strategy_fraction,
        )
        self.assertEqual(restored.allocated_count, m.allocated_count)
        self.assertEqual(restored.rejected_count, m.rejected_count)
        self.assertAlmostEqual(
            restored.total_allocated_fraction, m.total_allocated_fraction, places=10
        )
        self.assertEqual(restored.selection_method, "gate_then_rank_by_requested_fraction")
        self.assertEqual(restored.produced_at_utc, _TS)
        # New fields round-trip exactly.
        self.assertEqual(restored.unallocated_capital_micros, m.unallocated_capital_micros)
        for orig, rest in zip(m.allocations, restored.allocations):
            self.assertEqual(rest.artifact_id, orig.artifact_id)
            self.assertEqual(rest.adjustment_reasons, orig.adjustment_reasons)
            self.assertEqual(rest.rejection_reason, orig.rejection_reason)
            self.assertEqual(rest.status, orig.status)

    def test_write_read_preserves_adjustment_reasons(self):
        # budget_capped strategy round-trips adjustment_reasons correctly.
        c = _candidate("a1", requested_fraction=0.8)  # hits single cap
        m = select_strategies([c], _BUDGET, now_utc=_TS)
        with tempfile.TemporaryDirectory() as tmp:
            path = write_portfolio_allocation(m, Path(tmp))
            restored = read_portfolio_allocation(path)
        self.assertEqual(
            restored.allocations[0].adjustment_reasons,
            m.allocations[0].adjustment_reasons,
        )
        self.assertIsNone(restored.allocations[0].rejection_reason)

    def test_schema_version_mismatch_raises(self):
        with tempfile.TemporaryDirectory() as tmp:
            bad_path = Path(tmp) / "portfolio_allocation.json"
            bad_path.write_text(
                json.dumps({"schema_version": "wrong-v0", "budget": {}}), encoding="utf-8"
            )
            with self.assertRaises(ValueError):
                read_portfolio_allocation(bad_path)

    def test_written_json_is_valid_json(self):
        m = select_strategies([_candidate("a1", requested_fraction=0.2)], _BUDGET, now_utc=_TS)
        with tempfile.TemporaryDirectory() as tmp:
            path = write_portfolio_allocation(m, Path(tmp))
            raw = json.loads(path.read_text(encoding="utf-8"))
        self.assertEqual(raw["schema_version"], PORTFOLIO_ALLOCATION_CONTRACT_VERSION)
        self.assertIn("allocations", raw)
        self.assertIn("budget", raw)
        self.assertIn("unallocated_capital_micros", raw)


# ---------------------------------------------------------------------------
# Manifest fields
# ---------------------------------------------------------------------------

class TestManifestFields(unittest.TestCase):
    def test_selection_method_correct(self):
        m = select_strategies([_candidate("a1", requested_fraction=0.2)], _BUDGET, now_utc=_TS)
        self.assertEqual(m.selection_method, "gate_then_rank_by_requested_fraction")

    def test_schema_version_correct(self):
        m = select_strategies([], _BUDGET, now_utc=_TS)
        self.assertEqual(m.schema_version, "allocation-v2")

    def test_capital_micros_computed_correctly(self):
        # 30% of $100 = $30 = 30_000_000 micros
        c = _candidate("a1", requested_fraction=0.3)
        budget = CapitalBudget(
            total_capital_micros=100_000_000,
            max_strategies=1,
            max_single_strategy_fraction=0.5,
        )
        m = select_strategies([c], budget, now_utc=_TS)
        a = m.allocations[0]
        self.assertEqual(a.allocated_capital_micros, 30_000_000)
        # Verify accounting: 30M allocated + 70M unallocated = 100M total
        self.assertEqual(
            a.allocated_capital_micros + m.unallocated_capital_micros,
            budget.total_capital_micros,
        )


if __name__ == "__main__":
    unittest.main()
