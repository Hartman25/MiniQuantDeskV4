"""TV-04: Portfolio allocation / strategy selection.

Proves that multiple strategies cannot all assume ideal capital simultaneously.
Selection is deterministic, bounded by a CapitalBudget, and honest about
what each strategy actually receives versus what it requested.

Pipeline position: TV-01 (promote) → TV-02 (gate) → TV-03 (parity) → TV-04 (select)
"""

from __future__ import annotations

import dataclasses
import json
from pathlib import Path
from typing import List

from mqk_research.contracts import (
    PORTFOLIO_ALLOCATION_CONTRACT_VERSION,
    CapitalBudget,
    PortfolioAllocationManifest,
    StrategyAllocation,
    StrategyCandidate,
)

_SELECTION_METHOD = "gate_then_rank_by_requested_fraction"


def select_strategies(
    candidates: List[StrategyCandidate],
    budget: CapitalBudget,
    *,
    now_utc: str,
) -> PortfolioAllocationManifest:
    """Allocate capital across *candidates* within *budget* constraints.

    ``now_utc`` is required.  Core selection has no hidden wall-clock
    dependency: identical logical inputs always produce identical outputs.

    Algorithm (deterministic):
     1. Validate budget invariants; raise ValueError on bad config.
     2. Detect duplicate artifact_ids; raise ValueError — do not silently dedupe.
     3. Reject gate-failed candidates → rejection_reason="gate_failed".
     4. Reject candidates with invalid requested_fraction →
        rejection_reason="invalid_requested_fraction".
     5. Sort eligible by (-requested_fraction, artifact_id) for stable tiebreaking.
     6. Truncate to max_strategies; overflow →
        rejection_reason="max_strategies_reached".
     7. Cap each eligible fraction at max_single_strategy_fraction.
     8. If sum of capped fractions > 1.0, scale all proportionally so sum == 1.0.
     9. Assign status and compute integer micros first, then derive fractions FROM
        the assigned integer micros (not from the pre-truncation float):
        - "allocated"    — no adjustment applied.
        - "budget_capped"— one or both adjustments applied; adjustment_reasons
          lists every cause: "single_strategy_cap" and/or "budget_contention".
          rejection_reason is None for both allocated and budget_capped.
        allocated_fraction = allocated_capital_micros / budget.total_capital_micros
    10. Sort all allocations by artifact_id for canonical, permutation-stable output.
    11. Compute totals from actual assigned micros:
        total_allocated_fraction = sum(allocated_capital_micros) / total_capital_micros
        unallocated_capital_micros = total_capital_micros - sum(allocated_capital_micros)

    Invariants:
    - total_allocated_fraction <= 1.0 always (integer truncation never overshoots).
    - sum(allocated_capital_micros) + unallocated_capital_micros
      == budget.total_capital_micros exactly.
    - sum(allocated_capital_micros) <= budget.total_capital_micros.
    - allocated_fraction == allocated_capital_micros / budget.total_capital_micros
      for every non-rejected allocation (fraction is derived from actual micros).
    - total_allocated_fraction == sum(non-rejected allocated_capital_micros)
      / budget.total_capital_micros (derived from actual micros, not float intent).
    - All artifact_ids in the candidate set must be unique (enforced by ValueError).
    """
    # --- 1. Validate budget --------------------------------------------------
    if budget.total_capital_micros <= 0:
        raise ValueError(
            f"CapitalBudget.total_capital_micros must be > 0; "
            f"got {budget.total_capital_micros}"
        )
    if budget.max_strategies <= 0:
        raise ValueError(
            f"CapitalBudget.max_strategies must be > 0; "
            f"got {budget.max_strategies}"
        )
    if not (0.0 < budget.max_single_strategy_fraction <= 1.0):
        raise ValueError(
            f"CapitalBudget.max_single_strategy_fraction must be in (0, 1]; "
            f"got {budget.max_single_strategy_fraction}"
        )

    # --- 2. Detect duplicate artifact_ids ------------------------------------
    # Duplicate identities indicate a data integrity problem upstream.
    # Fail closed: do not silently deduplicate or prefer one over the other.
    seen_ids: set[str] = set()
    for c in candidates:
        if c.artifact_id in seen_ids:
            raise ValueError(
                f"Duplicate artifact_id {c.artifact_id!r} in candidate set. "
                "Each artifact_id must appear at most once per selection run."
            )
        seen_ids.add(c.artifact_id)

    allocations: List[StrategyAllocation] = []

    # --- 3 & 4. Filter: gate-failed and invalid fraction ----------------------
    eligible: List[StrategyCandidate] = []
    for c in candidates:
        if not c.gate_passed:
            allocations.append(
                StrategyAllocation(
                    artifact_id=c.artifact_id,
                    status="rejected",
                    allocated_fraction=0.0,
                    allocated_capital_micros=0,
                    rejection_reason="gate_failed",
                    adjustment_reasons=[],
                )
            )
            continue
        if not (0.0 < c.requested_fraction <= 1.0):
            allocations.append(
                StrategyAllocation(
                    artifact_id=c.artifact_id,
                    status="rejected",
                    allocated_fraction=0.0,
                    allocated_capital_micros=0,
                    rejection_reason="invalid_requested_fraction",
                    adjustment_reasons=[],
                )
            )
            continue
        eligible.append(c)

    # --- 5. Sort eligible: highest requested first; artifact_id breaks ties ---
    eligible.sort(key=lambda c: (-c.requested_fraction, c.artifact_id))

    # --- 6. Truncate to max_strategies ----------------------------------------
    accepted = eligible[: budget.max_strategies]
    overflow = eligible[budget.max_strategies :]
    for c in overflow:
        allocations.append(
            StrategyAllocation(
                artifact_id=c.artifact_id,
                status="rejected",
                allocated_fraction=0.0,
                allocated_capital_micros=0,
                rejection_reason="max_strategies_reached",
                adjustment_reasons=[],
            )
        )

    # --- 7. Cap each at max_single_strategy_fraction --------------------------
    capped: List[tuple[StrategyCandidate, float]] = []
    for c in accepted:
        frac = min(c.requested_fraction, budget.max_single_strategy_fraction)
        capped.append((c, frac))

    # --- 8. Proportional scale if total > 1.0 ---------------------------------
    total_desired = sum(f for _, f in capped)
    if total_desired > 1.0:
        scale = 1.0 / total_desired
        capped = [(c, f * scale) for c, f in capped]

    # --- 9. Build allocations with status and multi-cause adjustment truth -----
    for c, final_frac in capped:
        # Single-strategy cap: requested exceeded the per-strategy maximum.
        single_cap_applied = c.requested_fraction > budget.max_single_strategy_fraction
        # Budget contention: sum of capped fractions exceeded 1.0 before scaling.
        contention_applied = total_desired > 1.0

        # Collect all adjustment causes.  Both may fire simultaneously.
        adjustment_reasons: List[str] = []
        if single_cap_applied:
            adjustment_reasons.append("single_strategy_cap")
        if contention_applied:
            adjustment_reasons.append("budget_contention")

        # rejection_reason is reserved exclusively for status == "rejected".
        # Use adjustment_reasons for all budget-adjustment information.
        status = "budget_capped" if adjustment_reasons else "allocated"

        # Integer micros are the source of truth.  Compute micros first via
        # integer truncation (floor), then derive the fraction FROM the micros so
        # that allocated_fraction always equals exactly what was actually assigned.
        micros = int(final_frac * budget.total_capital_micros)
        actual_fraction = micros / budget.total_capital_micros
        allocations.append(
            StrategyAllocation(
                artifact_id=c.artifact_id,
                status=status,
                allocated_fraction=actual_fraction,
                allocated_capital_micros=micros,
                rejection_reason=None,
                adjustment_reasons=adjustment_reasons,
            )
        )

    # --- 10. Canonical output order: sort all allocations by artifact_id ------
    # Guarantees permutation-stable output: the same logical candidate set
    # produces an identical manifest regardless of input ordering.
    allocations.sort(key=lambda a: a.artifact_id)

    # --- 11. Totals derived from actual assigned micros -----------------------
    total_allocated_micros = sum(
        a.allocated_capital_micros for a in allocations if a.status != "rejected"
    )
    # unallocated_capital_micros covers both by-design undeployed capital and any
    # integer-truncation shortfall.
    # Invariant: total_allocated_micros + unallocated_capital_micros
    #            == budget.total_capital_micros exactly.
    unallocated_capital_micros = budget.total_capital_micros - total_allocated_micros

    # total_allocated_fraction is derived from actual assigned micros, not from
    # the sum of pre-truncation float fractions.
    total_allocated_fraction = total_allocated_micros / budget.total_capital_micros
    allocated_count = sum(1 for a in allocations if a.status != "rejected")
    rejected_count = sum(1 for a in allocations if a.status == "rejected")

    return PortfolioAllocationManifest(
        schema_version=PORTFOLIO_ALLOCATION_CONTRACT_VERSION,
        budget=budget,
        allocations=allocations,
        total_allocated_fraction=total_allocated_fraction,
        allocated_count=allocated_count,
        rejected_count=rejected_count,
        unallocated_capital_micros=unallocated_capital_micros,
        selection_method=_SELECTION_METHOD,
        produced_at_utc=now_utc,
    )


def write_portfolio_allocation(
    manifest: PortfolioAllocationManifest,
    output_dir: Path,
) -> Path:
    """Serialize *manifest* to ``portfolio_allocation.json`` in *output_dir*.

    Returns the path to the written file.
    """
    output_dir.mkdir(parents=True, exist_ok=True)
    out_path = output_dir / "portfolio_allocation.json"

    def _serial(obj):  # type: ignore[return]
        if dataclasses.is_dataclass(obj) and not isinstance(obj, type):
            return dataclasses.asdict(obj)
        raise TypeError(f"not serializable: {type(obj)!r}")

    payload = dataclasses.asdict(manifest)
    out_path.write_text(
        json.dumps(payload, indent=2, sort_keys=True, default=_serial),
        encoding="utf-8",
    )
    return out_path
