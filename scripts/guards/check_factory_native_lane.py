#!/usr/bin/env python3
"""Strategy Factory native-lane proof guard.

Reads the pytest JUnit XML of the strategy-factory CI lane and fails unless the load-bearing native E2E tests
actually EXECUTED and passed: a skipped, missing, failed or errored required test is a failure, not a pass. The only
tolerated skip is E2E-12 on a machine without the local verified real-data run (it never exists on a CI runner).

Usage: check_factory_native_lane.py <junit.xml>
"""

from __future__ import annotations

import sys
import xml.etree.ElementTree as ET

REQUIRED = (
    "test_e2e_01_and_10_unattended_campaign_runs_every_stage_on_the_real_engine",
    "test_e2e_01_evidence_lineage_is_reproducible_from_the_frozen_declaration",
    "test_e2e_02_losing_candidates_are_honestly_rejected_and_nothing_is_promoted",
    "test_e2e_03_unsupported_and_underspecified_ideas_are_dispositioned_never_executed",
    "test_e2e_04_whole_declared_population_is_accounted_including_invalid_and_extreme_members",
    "test_e2e_05_concurrent_campaigns_and_real_worker_processes_never_cross_contaminate",
    "test_e2e_06_killed_mid_trials_recovers_truthfully_without_rewriting_prior_evidence",
    "test_e2e_07_python_rust_parity_native_engine_vs_its_grammar_twin_and_declared_fingerprints",
    "test_e2e_08_full_population_judged_with_preserved_denominator",
    "test_e2e_09_an_unreleased_gate_blocks_every_stage_before_any_data_or_registry_access",
    "test_e2e_09_a_released_gate_without_a_signed_authorization_still_runs_nothing_effectful",
    "test_e2e_09_bars_reaching_the_reserved_holdout_are_refused_before_any_attempt",
    "test_e2e_09_synthetic_data_can_never_be_graded_as_market_evidence",
    "test_e2e_11_scheduled_pass_with_no_eligible_work_does_nothing_then_runs_only_new_work",
    "test_required_native_binary_resolves_the_grammar_engine",
)
OPTIONAL_SKIP = {"test_e2e_12_real_data_readiness_explains_exactly_why_a_real_campaign_cannot_start": "no verified local historical bars"}


def verdict(root: ET.Element) -> list[str]:
    problems: list[str] = []
    outcome: dict[str, str] = {}
    for case in root.iter("testcase"):
        name = case.get("name", "")
        if case.find("failure") is not None or case.find("error") is not None:
            problems.append(f"FAILED: {name}")
            outcome[name] = "failed"
            continue
        skipped = case.find("skipped")
        if skipped is not None:
            msg = skipped.get("message", "")
            if name in OPTIONAL_SKIP and OPTIONAL_SKIP[name] in msg:
                outcome[name] = "optional-skip"
                continue
            problems.append(f"SKIPPED (not allowed): {name}: {msg[:100]}")
            outcome[name] = "skipped"
            continue
        outcome[name] = "passed"
    for name in REQUIRED:
        if outcome.get(name) != "passed":
            problems.append(f"required test did not pass: {name} ({outcome.get(name, 'absent')})")
    return problems


def main(argv: list[str]) -> int:
    if len(argv) != 2:
        print(__doc__)
        return 2
    try:
        root = ET.parse(argv[1]).getroot()
    except (OSError, ET.ParseError) as exc:
        print(f"FACTORY NATIVE LANE: cannot read {argv[1]}: {exc}")
        return 1
    problems = verdict(root)
    for p in problems:
        print(f"FACTORY NATIVE LANE: {p}")
    cases = list(root.iter("testcase"))
    print(f"FACTORY NATIVE LANE: {len(cases)} tests, {len(problems)} problem(s)")
    return 1 if problems else 0


if __name__ == "__main__":
    sys.exit(main(sys.argv))
