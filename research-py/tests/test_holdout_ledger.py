"""
RESEARCH-HOLDOUT-CONSUMPTION-LEDGER-01 — durable holdout-consumption ledger
tests.

Purely additive tests: no test here evaluates or scores a holdout region's
data. They only prove the LEDGER's own contract (deterministic identity,
reserved-vs-consumed state, atomic one-time consumption, no influence from
result values) and that nothing else in the registry/walk-forward stack
changed meaning as a result of adding it.
"""
from __future__ import annotations

import threading
from concurrent.futures import ThreadPoolExecutor
from pathlib import Path

import pytest

from mqk_research.exp_distributed.storage import ResearchResultStore
from mqk_research.ml.holdout_ledger import (
    compute_holdout_id,
    consume_holdout,
    get_holdout,
    reserve_holdout,
)

REPO_SRC = Path(__file__).resolve().parents[1] / "src" / "mqk_research"


def _dataset_identity(salt: str = "a") -> dict:
    return {
        "features_csv": {"sha256": f"feat-{salt}", "bytes": 100},
        "targets_csv": {"sha256": f"targ-{salt}", "bytes": 100},
        "feature_schema": {"sha256": f"schema-{salt}", "bytes": 10},
    }


def _reserve(registry_db: Path, *, salt="a", start="2024-01-01T00:00:00Z", end="2024-07-01T00:00:00Z", protocol="walk_forward_eval_v2") -> str:
    return reserve_holdout(
        registry_db,
        dataset_identity=_dataset_identity(salt),
        holdout_start_utc=start,
        holdout_end_utc=end,
        protocol_version=protocol,
    )


# ---------------------------------------------------------------------------
# TEST 1 — deterministic holdout_id
# ---------------------------------------------------------------------------


def test_holdout_id_deterministic():
    id_a = compute_holdout_id(
        dataset_identity=_dataset_identity("x"), holdout_start_utc="2024-01-01T00:00:00Z",
        holdout_end_utc="2024-07-01T00:00:00Z", protocol_version="walk_forward_eval_v2",
    )
    id_b = compute_holdout_id(
        dataset_identity=_dataset_identity("x"), holdout_start_utc="2024-01-01T00:00:00Z",
        holdout_end_utc="2024-07-01T00:00:00Z", protocol_version="walk_forward_eval_v2",
    )
    assert id_a == id_b


# ---------------------------------------------------------------------------
# TEST 2 — reservation does not equal consumption
# ---------------------------------------------------------------------------


def test_reservation_is_not_consumption(tmp_path):
    registry_db = tmp_path / "reg.sqlite3"
    holdout_id = _reserve(registry_db)
    record = get_holdout(registry_db, holdout_id)
    assert record["status"] == "reserved"
    assert record["status"] != "consumed"
    assert record["consumed_at"] is None
    assert record["consumer_identity"] is None


# ---------------------------------------------------------------------------
# TEST 3 / TEST 4 / TEST 5 — first consume succeeds, second fails closed,
# original consumer is never overwritten
# ---------------------------------------------------------------------------


def test_first_consume_succeeds_second_fails_closed_consumer_preserved(tmp_path):
    registry_db = tmp_path / "reg.sqlite3"
    holdout_id = _reserve(registry_db)

    consume_holdout(
        registry_db, holdout_id=holdout_id, consumer_identity={"trial_id": "trial-1"},
        consumed_at="2025-01-01T00:00:00Z", result_identity="result-1",
    )
    record = get_holdout(registry_db, holdout_id)
    assert record["status"] == "consumed"
    assert record["consumer_identity"] == {"trial_id": "trial-1"}
    assert record["result_identity"] == "result-1"
    assert record["consumed_at"] == "2025-01-01T00:00:00Z"

    with pytest.raises(RuntimeError):
        consume_holdout(
            registry_db, holdout_id=holdout_id, consumer_identity={"trial_id": "trial-2-imposter"},
            consumed_at="2025-06-01T00:00:00Z", result_identity="result-2-imposter",
        )

    # TEST 5: the original consumer's record is untouched by the failed
    # second attempt.
    record_after = get_holdout(registry_db, holdout_id)
    assert record_after == record


def test_consume_unknown_holdout_fails_closed(tmp_path):
    registry_db = tmp_path / "reg.sqlite3"
    with pytest.raises(KeyError):
        consume_holdout(
            registry_db, holdout_id="never-reserved", consumer_identity={"trial_id": "t"},
            consumed_at="2025-01-01T00:00:00Z",
        )


# ---------------------------------------------------------------------------
# TEST 6 — concurrent consume race -> exactly one winner
# ---------------------------------------------------------------------------


def test_concurrent_consume_race_has_exactly_one_winner(tmp_path):
    registry_db = tmp_path / "reg.sqlite3"
    holdout_id = _reserve(registry_db)

    n_racers = 12
    outcomes = []
    lock = threading.Lock()

    def _race(i: int) -> None:
        try:
            consume_holdout(
                registry_db, holdout_id=holdout_id, consumer_identity={"racer": i},
                consumed_at=f"2025-01-01T00:00:{i:02d}Z", result_identity=f"result-{i}",
            )
            outcome = ("won", i)
        except (RuntimeError, KeyError):
            outcome = ("lost", i)
        with lock:
            outcomes.append(outcome)

    with ThreadPoolExecutor(max_workers=8) as executor:
        futures = [executor.submit(_race, i) for i in range(n_racers)]
        for future in futures:
            future.result()

    winners = [o for o in outcomes if o[0] == "won"]
    losers = [o for o in outcomes if o[0] == "lost"]
    assert len(winners) == 1
    assert len(losers) == n_racers - 1

    record = get_holdout(registry_db, holdout_id)
    assert record["status"] == "consumed"
    assert record["consumer_identity"] == {"racer": winners[0][1]}


# ---------------------------------------------------------------------------
# TEST 7 — result value does not influence holdout_id
# ---------------------------------------------------------------------------


def test_result_value_does_not_influence_holdout_id(tmp_path):
    """compute_holdout_id has no parameter for a result/metric value at all
    -- structurally impossible to leak one in. This proves it end to end:
    the SAME holdout_id, reserved once, is accepted by consume_holdout under
    two independent registries even though each is fed a wildly different
    "result" (consumer_identity/result_identity) at consumption time -- the
    id itself never depended on either."""
    db_a = tmp_path / "a.sqlite3"
    db_b = tmp_path / "b.sqlite3"
    id_a = _reserve(db_a)
    id_b = _reserve(db_b)
    assert id_a == id_b

    consume_holdout(
        db_a, holdout_id=id_a, consumer_identity={"trial_id": "trial-cheap"},
        consumed_at="2025-01-01T00:00:00Z", result_identity="sharpe=0.01",
    )
    consume_holdout(
        db_b, holdout_id=id_b, consumer_identity={"trial_id": "trial-moonshot"},
        consumed_at="2025-01-01T00:00:00Z", result_identity="sharpe=99.0",
    )
    # Both consumptions were accepted against the identical id computed
    # BEFORE either result existed.
    assert get_holdout(db_a, id_a)["status"] == "consumed"
    assert get_holdout(db_b, id_b)["status"] == "consumed"


# ---------------------------------------------------------------------------
# TEST 8 — different dataset identity -> different holdout_id
# ---------------------------------------------------------------------------


def test_different_dataset_identity_changes_holdout_id():
    id_a = compute_holdout_id(
        dataset_identity=_dataset_identity("a"), holdout_start_utc="2024-01-01T00:00:00Z",
        holdout_end_utc="2024-07-01T00:00:00Z", protocol_version="walk_forward_eval_v2",
    )
    id_b = compute_holdout_id(
        dataset_identity=_dataset_identity("b"), holdout_start_utc="2024-01-01T00:00:00Z",
        holdout_end_utc="2024-07-01T00:00:00Z", protocol_version="walk_forward_eval_v2",
    )
    assert id_a != id_b


# ---------------------------------------------------------------------------
# TEST 9 — different holdout boundary -> different holdout_id
# ---------------------------------------------------------------------------


def test_different_boundary_changes_holdout_id():
    base = dict(dataset_identity=_dataset_identity("a"), protocol_version="walk_forward_eval_v2")
    id_start = compute_holdout_id(holdout_start_utc="2024-01-01T00:00:00Z", holdout_end_utc="2024-07-01T00:00:00Z", **base)
    id_diff_start = compute_holdout_id(holdout_start_utc="2024-02-01T00:00:00Z", holdout_end_utc="2024-07-01T00:00:00Z", **base)
    id_diff_end = compute_holdout_id(holdout_start_utc="2024-01-01T00:00:00Z", holdout_end_utc="2024-08-01T00:00:00Z", **base)
    assert len({id_start, id_diff_start, id_diff_end}) == 3


def test_different_protocol_version_changes_holdout_id():
    base = dict(dataset_identity=_dataset_identity("a"), holdout_start_utc="2024-01-01T00:00:00Z", holdout_end_utc="2024-07-01T00:00:00Z")
    id_v1 = compute_holdout_id(protocol_version="walk_forward_eval_v2", **base)
    id_v2 = compute_holdout_id(protocol_version="walk_forward_eval_v3_future", **base)
    assert id_v1 != id_v2


# ---------------------------------------------------------------------------
# TEST 10 — existing research registry semantics remain unchanged
# ---------------------------------------------------------------------------


def test_trial_attempt_registry_unaffected_by_holdout_ledger_in_same_db(tmp_path):
    registry_db = tmp_path / "reg.sqlite3"
    store = ResearchResultStore(registry_db)

    holdout_id = _reserve(registry_db)
    consume_holdout(
        registry_db, holdout_id=holdout_id, consumer_identity={"trial_id": "t"}, consumed_at="2025-01-01T00:00:00Z",
    )

    store.register_hypothesis(hypothesis_id="hyp", experiment_id="exp")
    store.register_trial(
        trial_id="trial-1", experiment_id="exp", hypothesis_id="hyp",
        strategy_id="s", protocol_id="p", identity={"k": "v"},
    )
    attempt_id, attempt_index = store.begin_attempt(trial_id="trial-1")
    assert attempt_index == 1
    store.finalize_attempt(attempt_id, status="succeeded", result_id="r1")

    trials = store.list_trials(experiment_id="exp")
    assert len(trials) == 1
    attempts = store.list_attempts("trial-1")
    assert len(attempts) == 1
    assert attempts[0]["status"] == "succeeded"

    # The holdout ledger row is untouched by unrelated trial/attempt writes.
    record = get_holdout(registry_db, holdout_id)
    assert record["status"] == "consumed"


# ---------------------------------------------------------------------------
# TEST 12 — no code in this patch (or elsewhere in research-py) evaluates
# holdout performance by calling consume_holdout
# ---------------------------------------------------------------------------


def test_consume_holdout_has_no_caller_outside_this_module_and_tests():
    """Structural proof that this patch is purely additive: grep the actual
    source tree (not memory/docs) for any call site of consume_holdout other
    than its own definition and this test file. The purged walk-forward
    evaluator and the economic walk-forward evaluator must not have gained
    the ability to score a holdout as a side effect of this patch."""
    definition_sites = {"holdout_ledger.py", "storage.py"}
    hits = []
    for path in REPO_SRC.rglob("*.py"):
        if path.name in definition_sites:
            continue
        text = path.read_text(encoding="utf-8")
        if "consume_holdout(" in text:
            hits.append(str(path))
    assert hits == [], f"unexpected consume_holdout call site(s) outside holdout_ledger.py/storage.py: {hits}"
