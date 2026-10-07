"""Census-02 CURRENT partition-consumption truth (C3C). The Confirmation window is consumed globally and never read by
Census-02; the Final Holdout stays reserved; Census-01's historical partition object is never copied as current truth; no
Confirmation RESULT value can enter any Census-02 identity. Synthetic data only."""

from __future__ import annotations

import builtins
import copy
import io
import json
import sys
from pathlib import Path

import pandas as pd
import pytest

sys.path.insert(0, str(Path(__file__).resolve().parent))
import c2_testing as ct  # noqa: E402  (puts the Census-02 modules on sys.path)

import c2_policy as pol  # noqa: E402
import c2_population as pop  # noqa: E402
import c2_protocol as pr  # noqa: E402
import c2_signals as sg2  # noqa: E402
import partitions as pt1  # noqa: E402

REPO = pr.REPO
FIXTURE = Path(__file__).resolve().parent / "fixtures" / "census02_superseded_freeze_ab686f1.json"
EXPECTED_TRUTH = {
    "schema_version": "census02_partition_truth_v1",
    "discovery": {"start_inclusive": "2016-01-01", "end_exclusive": "2024-01-01", "role": "CENSUS02_DISCOVERY_REUSE"},
    "contaminated_2024": {"start_inclusive": "2024-01-01", "end_exclusive": "2025-01-01", "status": "CONTAMINATED_BY_REJECTED_RUN",
                          "role": "CENSUS02_NEVER_READ"},
    "confirmation_window": {"start_inclusive": "2025-01-01", "end_exclusive": "2026-03-01",
                            "status": "CONSUMED_BY_ALPHA_EDGE_CONFIRMATION_01", "role": "CENSUS02_NEVER_READ"},
    "final_holdout": {"start_inclusive": "2026-03-01", "status": "RESERVED_UNCONSUMED", "role": "FINAL_HOLDOUT"},
    "census02_access": {"discovery": "READ_AND_SCORED_STRICTLY_BEFORE_2024-01-01", "contaminated_2024": "NEVER_READ",
                        "confirmation_window": "NEVER_READ", "final_holdout": "NEVER_READ"},
    "semantics": "the Confirmation window is consumed globally and never read by Census-02; the Final Holdout stays reserved",
}


# ================================================================================================ current truth
def test_current_partition_truth_is_exact():
    assert pr.partition_truth() == EXPECTED_TRUTH            # literal: any drift of status / role / window / access fails
    cw = pr.partition_truth()["confirmation_window"]
    assert (cw["start_inclusive"], cw["end_exclusive"]) == ("2025-01-01", "2026-03-01")
    assert cw["status"] == "CONSUMED_BY_ALPHA_EDGE_CONFIRMATION_01" and cw["role"] == "CENSUS02_NEVER_READ"
    assert pr.partition_truth()["final_holdout"]["status"] == "RESERVED_UNCONSUMED"          # NOT consumed
    assert pr.partition_truth()["census02_access"]["confirmation_window"] == "NEVER_READ"
    assert pr.partition_truth()["census02_access"]["final_holdout"] == "NEVER_READ"


def test_the_structural_protocol_carries_current_truth_never_the_historical_object():
    s = pr.build_structural_protocol()
    assert s["partitions"] == EXPECTED_TRUTH
    text = json.dumps(s)
    for stale in ("remaining_confirmation_reserve", "LATER_INDEPENDENT_CONFIRMATION", "NEVER_AN_UNREAD_RESERVE",
                  "REMAINING_CONFIRMATION_RESERVE"):
        assert stale not in text, f"stale historical partition metadata copied into the frozen protocol: {stale}"
    assert "consumed by alpha_edge_confirmation_01; never read by Census-02" in s["data_contract"]["confirmation"]
    # the consumption fact is real repository truth (existence only: no outcome file is opened)
    assert (REPO / "research-py" / "experiments" / "alpha_edge_confirmation_01").is_dir()


def test_historical_census01_authority_is_unchanged_and_deliberately_not_current():
    """Census-01's partitions.py / frozen JSON are accepted historical authority: pinned byte-for-byte (LF-normalised)."""
    base = REPO / "research-py" / "experiments" / "alpha_edge_census_01"
    assert pr._sha_lf(base / "partitions.py") == "211e4a13aaf16e7b5327cc3a8a2f9e2b8a13d6091185230eee3eaa935dd86b43"
    assert pr._sha_lf(base / "ALPHA_CENSUS_PARTITIONS_V2.json") == "9549618bd8fae8a57716e50bcd3f22be1c8b67791645389933772c3d379b9c4d"
    assert pt1.PARTITIONS["remaining_confirmation_reserve"]["status"] == "RESERVED_UNCONSUMED"   # historically true when frozen
    assert pr.partition_truth() != pt1.PARTITIONS and pop.partitions_id() != pr.sha256_canonical(pt1.PARTITIONS)[:32]


def test_truth_boundaries_must_equal_the_reused_hard_fence(monkeypatch):
    pr.partition_truth()
    for attr in ("DISCOVERY_END_EXCLUSIVE", "RESERVE_END_EXCLUSIVE", "FINAL_HOLDOUT_START", "RESERVE_START", "CONTAMINATED_END_EXCLUSIVE"):
        with monkeypatch.context() as m:
            m.setattr(pr.pt, attr, pd.Timestamp("2030-01-01", tz="UTC"))
            with pytest.raises(pr.PartitionTruthRefusal):
                pr.partition_truth()


@pytest.mark.parametrize("ts,name", [("2016-01-04", "discovery"), ("2023-12-29", "discovery"), ("2023-12-31T23:59:59", "discovery"),
                                     ("2024-01-01", "contaminated_2024"), ("2024-12-31T23:59:59", "contaminated_2024"),
                                     ("2025-01-01", "confirmation_window"), ("2026-02-27", "confirmation_window"),
                                     ("2026-02-28T23:59:59", "confirmation_window"), ("2026-03-01", "final_holdout"),
                                     ("2030-01-01", "final_holdout")])
def test_classify_partition_table(ts, name):
    assert pr.classify_partition(ts) == name


# ============================================================================================ fences (Census-02 reads)
def _bars(*days):
    return pd.DataFrame({"end_ts": [pd.Timestamp(d, tz="UTC") + pd.Timedelta(hours=5) for d in days], "open": 1.0, "high": 1.1,
                         "low": 0.9, "close": 1.0, "volume": 1.0, "symbol": "T"})


@pytest.mark.parametrize("day,name,status", [
    ("2024-01-02", "contaminated_2024", "CONTAMINATED_BY_REJECTED_RUN"),
    ("2024-12-30", "contaminated_2024", "CONTAMINATED_BY_REJECTED_RUN"),
    ("2025-01-02", "confirmation_window", "CONSUMED_BY_ALPHA_EDGE_CONFIRMATION_01"),
    ("2025-09-15", "confirmation_window", "CONSUMED_BY_ALPHA_EDGE_CONFIRMATION_01"),
    ("2026-02-27", "confirmation_window", "CONSUMED_BY_ALPHA_EDGE_CONFIRMATION_01"),
    ("2026-03-02", "final_holdout", "RESERVED_UNCONSUMED"),
    ("2026-10-01", "final_holdout", "RESERVED_UNCONSUMED")])
def test_every_row_at_or_after_2024_is_refused_with_the_true_census02_partition(day, name, status):
    with pytest.raises(pt1.PartitionBreach) as e:
        pr.fence_bars(_bars("2023-12-28", day), what="t")
    assert name in str(e.value) and status in str(e.value) and "Census-02 never reads it" in str(e.value)
    with pytest.raises(pt1.PartitionBreach):                                            # the bar loader fences first
        sg2.build_symbol_data("T", _bars(day))


def test_confirmation_input_is_refused_and_discovery_is_accepted():
    for day in ("2025-01-01", "2025-06-30", "2026-02-27"):
        with pytest.raises(pt1.PartitionBreach, match="confirmation_window"):
            pr.fence_bars(_bars("2023-12-27", day), what="confirmation")
    with pytest.raises(pt1.PartitionBreach, match="final_holdout"):
        pr.fence_bars(_bars("2026-03-01"), what="holdout")
    assert len(pr.fence_bars(_bars("2023-12-27", "2023-12-29"), what="discovery")) == 2
    with pytest.raises(pt1.PartitionBreach):
        pr.fence_bars(_bars(), what="empty")                                            # a guard that checked nothing proves nothing


# ===================================================================== no Confirmation RESULT value in any identity
FORBIDDEN_OUTCOME_TOKENS = ("CONFIRMED_STRONG", "CONFIRMED_DIRECTIONAL_ONLY", "NOT_CONFIRMED", "DIRECTIONAL_ONLY",
                            "confirmation_attempt", "confirmation_result", "confirmation_outcome", "confirmation_metrics",
                            "n_confirmed", "n_directional")


def test_no_confirmation_outcome_participates_in_protocol_factor_or_trial_identity():
    s = pr.build_structural_protocol()
    blob = json.dumps(s, sort_keys=True) + json.dumps(pol.policy_document(), sort_keys=True) + json.dumps(
        pop.population_authority(pol.approved_decisions(), "p" * 32), sort_keys=True)
    for token in FORBIDDEN_OUTCOME_TOKENS:
        assert token not in blob, f"Confirmation outcome token {token!r} reached a Census-02 identity input"
    assert set(s["partitions"]) == set(EXPECTED_TRUTH)                                 # closed key set: nothing can be injected
    for k in ("confirmation_window", "final_holdout", "contaminated_2024", "discovery"):
        assert set(s["partitions"][k]) <= {"start_inclusive", "end_exclusive", "status", "role"}
    # the manifest binds no Confirmation experiment file at all
    for rel in (*pr.BEHAVIOR_SOURCES, *pr.AUTHORITY_DATA):
        assert "alpha_edge_confirmation_01" not in rel and "alpha_edge_pass2_01" not in rel


def test_building_every_identity_opens_no_confirmation_file(monkeypatch):
    opened: list[str] = []
    real = builtins.open

    def spy(file, *a, **k):
        opened.append(str(file))
        return real(file, *a, **k)

    monkeypatch.setattr(builtins, "open", spy)
    monkeypatch.setattr(io, "open", spy)
    d = pol.approved_decisions()
    s = pr.build_structural_protocol()
    pid = pr.frozen_protocol_id(s, d, pr.behavior_source_manifest(), {"python": "x", "numpy": "x", "pandas": "x"})
    pop.population_authority(d, pid)
    pop.partitions_id()
    assert opened, "non-vacuous: the spy must have observed the manifest / seed reads"
    assert not [p for p in opened if "confirmation" in p.lower()], "a Confirmation artifact was opened while building identities"


def test_the_partition_identity_changes_only_with_the_consumption_state_never_with_results(monkeypatch):
    base = pop.partitions_id()
    assert base == pr.sha256_canonical(EXPECTED_TRUTH)[:32]
    tweaked = copy.deepcopy(pr.PARTITION_TRUTH)
    tweaked["confirmation_window"]["status"] = "RESERVED_UNCONSUMED"
    monkeypatch.setattr(pr, "PARTITION_TRUTH", tweaked)
    assert pop.partitions_id() != base                                                  # the STATE is identity-relevant, deliberately


def test_counts_and_complements_are_unchanged_by_the_correction():
    d = pol.approved_decisions()
    a = pop.population_authority(d, "p" * 32)
    s, f = a["strategy_population"], a["factor_population"]
    assert (s["config_count"], s["class_c_symbol_count"], s["trial_count"]) == (470, 20, 9400)
    assert (f["condition_count"], len(f["horizons"]), f["factor_count"]) == (215, 5, 1075)
    assert (s["complement_tagged_configs"], s["complement_tagged_trials"]) == (56, 1120)
    assert f["direction"] == "lower_is_better" and f["scope"] == "ALL_SEED_SYMBOLS" and f["scope_symbol_count"] == 88
    assert pol.approved_decisions() == pol.APPROVED_DECISIONS and d["etf_borrow_assumption"]["annual_borrow_fee_bps"] == 100.0


# ===================================================================================== the old freeze is stale
def _stale():
    return json.loads(FIXTURE.read_text(encoding="utf-8"))


def test_the_superseded_freeze_carries_the_stale_status_and_no_attempt_or_result():
    old = _stale()
    assert old["behavior_head"] == "675549e219b4bb78adc298fde56dd39bae655e45"
    assert old["attempts_at_freeze"] == 0 and old["results_present_at_freeze"] is False
    assert old["structural_protocol"]["partitions"]["remaining_confirmation_reserve"]["status"] == "RESERVED_UNCONSUMED"   # the defect
    assert old["strategy_population"]["trial_count"] == 9400 and old["factor_population"]["factor_count"] == 1075
    assert old["decisions"] == pol.approved_decisions()                                # the approved policy is NOT what was wrong


def test_the_old_freeze_cannot_pass_require_freeze_after_the_correction():
    old = _stale()
    cur = pr.behavior_source_manifest()
    assert old["behavior_source_manifest"]["sources"][pr.BEHAVIOR_SOURCES[0]] is not None
    drift = sorted(k for k, v in cur["sources"].items() if old["behavior_source_manifest"]["sources"].get(k) != v)
    assert any(k.endswith("c2_protocol.py") for k in drift) and any(k.endswith("c2_population.py") for k in drift)
    assert old["structural_protocol"] != pr.build_structural_protocol()
    assert old["protocol_id"] != pr.frozen_protocol_id(pr.build_structural_protocol(), old["decisions"], cur, old["environment_identity"])
    with pytest.raises(pr.FreezeRefusal):                                                # the real guard, the real repo
        pr.require_freeze(REPO, FIXTURE)


def test_the_old_freeze_is_refused_for_the_right_reason_in_a_controlled_repo(tmp_path):
    repo, _good = ct.make_frozen_repo(tmp_path)
    stale = repo / "STALE.json"
    stale.write_text(FIXTURE.read_text(encoding="utf-8"), encoding="utf-8")
    ct.git(repo, "add", "-A")
    ct.git(repo, "commit", "-qm", "stale")
    with pytest.raises(pr.FreezeRefusal) as e:
        pr.require_freeze(repo, stale)
    assert "differs from the frozen manifest" in str(e.value) or "environment" in str(e.value)
    if "environment" not in str(e.value):
        assert "c2_protocol.py" in str(e.value) and "c2_population.py" in str(e.value)


def test_a_self_consistent_forged_stale_partition_freeze_is_refused(tmp_path):
    """Every neighbouring check satisfied (manifest, head, decisions, populations recomputed): ONLY the structural-protocol /
    protocol-id equality can refuse a freeze that still labels the consumed window RESERVED_UNCONSUMED."""
    repo, f = ct.make_frozen_repo(tmp_path)
    good = json.loads(f.read_text(encoding="utf-8"))
    assert pr.require_freeze(repo, f)                                                    # positive control
    stale_structural = copy.deepcopy(good["structural_protocol"])
    stale_structural["partitions"] = copy.deepcopy(pt1.PARTITIONS)
    pid = pr.frozen_protocol_id(stale_structural, good["decisions"], good["behavior_source_manifest"], good["environment_identity"])
    forged = {**good, "structural_protocol": stale_structural, "protocol_id": pid,
              **pop.population_authority(good["decisions"], pid)}
    f.write_text(json.dumps(forged, sort_keys=True), encoding="utf-8")
    ct.git(repo, "add", "-A")
    ct.git(repo, "commit", "-qm", "forged stale partition freeze")
    with pytest.raises(pr.FreezeRefusal, match="structural protocol differs"):
        pr.require_freeze(repo, f)
