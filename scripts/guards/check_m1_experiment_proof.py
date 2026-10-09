#!/usr/bin/env python3
"""CI proof that the M1 KISS experiment tests really executed offline.

Usage: check_m1_experiment_proof.py <pytest-junit.xml> <netguard-summary.json>

Fails when: a required test module is absent from the run (a dropped or renamed test, or a trimmed pytest
command), any test failed/errored/was skipped (a skip would mask a load-bearing proof), the total is below the
floor, the offline guard did not record its own deliberate probes (it did not run), or any unexpected external
network / secret-file attempt (in the test process or in a spawned child) was recorded.
"""

from __future__ import annotations

import json
import sys
import xml.etree.ElementTree as ET
from pathlib import Path

REQUIRED_MODULES = (
    "experiments.m1_native_trend_campaign.test_hermetic_provider_isolation",
    "experiments.m1_native_trend_campaign.test_stage_authorization",
    "experiments.m1_native_trend_campaign.test_subprocess_guard_inheritance",
    "experiments.m1_native_trend_campaign.test_holdout_incident",
    "experiments.m1_native_trend_campaign.test_holdout_guard",
    "experiments.m1_native_trend_campaign.test_kiss_ext032_predeclaration",
    "experiments.m1_native_trend_campaign.test_kiss_ext032_registration_gate",
    "experiments.m1_native_trend_campaign.test_kiss_ext032_fixed_partition",
    "experiments.m1_native_trend_campaign.test_kiss_ext032_near_miss_review",
    "experiments.m1_native_trend_campaign.test_kiss_ext032_search_accounting",
    "experiments.m1_native_trend_campaign.test_batch_runner_chronology",
    "experiments.m1_native_trend_campaign.test_post_discovery_authority",
    "experiments.external_idea_intake.test_external_idea_intake",
    "experiments.external_idea_intake.test_consolidated_readiness_pins",
)
MIN_TESTS_PER_MODULE = 3
MIN_TOTAL_TESTS = 520
MIN_DELIBERATE_PROBES = 5  # the isolation tests' own refused probes: proves the guard was installed and live


def main(argv: list[str]) -> int:
    if len(argv) != 3:
        print("usage: check_m1_experiment_proof.py <junit.xml> <netguard-summary.json>", file=sys.stderr)
        return 2
    problems: list[str] = []
    try:
        root = ET.parse(argv[1]).getroot()
    except (OSError, ET.ParseError) as exc:
        print(f"[M1-EXP-PROOF] junit report unreadable: {exc}", file=sys.stderr)
        return 1
    cases = list(root.iter("testcase"))
    per_module: dict[str, int] = {}
    for case in cases:
        per_module[case.get("classname", "")] = per_module.get(case.get("classname", ""), 0) + 1
        for tag in ("failure", "error", "skipped"):
            if case.find(tag) is not None:
                problems.append(f"{case.get('classname')}::{case.get('name')} reported {tag}")
    for module in REQUIRED_MODULES:
        if per_module.get(module, 0) < MIN_TESTS_PER_MODULE:
            problems.append(f"required module {module} ran {per_module.get(module, 0)} tests (< {MIN_TESTS_PER_MODULE})")
    if len(cases) < MIN_TOTAL_TESTS:
        problems.append(f"only {len(cases)} tests ran (< {MIN_TOTAL_TESTS})")
    try:
        summary = json.loads(Path(argv[2]).read_text(encoding="utf-8"))
    except (OSError, ValueError) as exc:
        problems.append(f"offline-guard summary unreadable: {exc}")
        summary = {}
    if summary.get("unexpected_attempts") != 0:
        problems.append(f"offline guard recorded unexpected attempts: {summary.get('unexpected_attempts')!r}")
    if summary.get("unexpected_child_attempts") != 0:
        problems.append(f"offline guard recorded unexpected attempts by spawned children: {summary.get('unexpected_child_attempts')!r}")
    if summary.get("uninitialized_children") != 0:
        problems.append(f"offline guard: spawned children that never initialized the guard: {summary.get('uninitialized_children')!r}")
    if not isinstance(summary.get("attempted_total"), int) or summary["attempted_total"] < MIN_DELIBERATE_PROBES:
        problems.append(f"offline guard recorded {summary.get('attempted_total')!r} deliberate probes "
                        f"(< {MIN_DELIBERATE_PROBES}): it was not installed or did not run")
    if problems:
        for p in problems[:30]:
            print(f"[M1-EXP-PROOF] {p}", file=sys.stderr)
        return 1
    print(f"[M1-EXP-PROOF] OK: {len(cases)} tests, 0 skipped, {len(REQUIRED_MODULES)} required modules present, "
          f"offline guard refused {summary['attempted_total']} deliberate probes and 0 unexpected attempts.")
    return 0


if __name__ == "__main__":
    sys.exit(main(sys.argv))
