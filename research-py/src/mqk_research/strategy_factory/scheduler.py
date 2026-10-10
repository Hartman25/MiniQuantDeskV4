"""Bounded Research control plane: worker loop and pass scheduler over the durable queue.

A pass recovers expired leases, re-evaluates blocked jobs once, then drains eligible work with at most `workers` jobs
running at a time (independent campaigns in parallel, one stage at a time within a campaign). It ends with an explicit
reason: NO_ELIGIBLE_WORK (everything succeeded or is terminal), BLOCKED (work remains but a prerequisite is missing), or
FAILED. It never creates economic-policy decisions, never retries a failed stage by itself, and never touches data it
was not authorized for: the stage authorization is verified by the executor and again by the runner.

Scheduling: run `mqk-factory run --until-idle` from any external scheduler (Windows Task Scheduler / cron). A pass over a
store with no eligible work does nothing and says so.
"""

from __future__ import annotations

import threading
import time
from dataclasses import dataclass, field
from typing import Any, Callable

from mqk_research.strategy_factory.executor import StageExecutor
from mqk_research.strategy_factory.store import FactoryStore

END_NO_WORK, END_BLOCKED, END_FAILED, END_BUDGET = "NO_ELIGIBLE_WORK", "BLOCKED", "FAILED", "JOB_BUDGET_REACHED"


@dataclass
class PassResult:
    ended: str
    jobs_run: int = 0
    interrupted_recovered: int = 0
    by_status: dict[str, int] = field(default_factory=dict)
    campaigns: list[dict[str, Any]] = field(default_factory=list)

    def as_dict(self) -> dict[str, Any]:
        return {"ended": self.ended, "jobs_run": self.jobs_run, "interrupted_recovered": self.interrupted_recovered,
                "by_status": dict(sorted(self.by_status.items())), "campaigns": self.campaigns}


def run_one(store: FactoryStore, executor: StageExecutor, job: dict[str, Any], worker_id: str, lease_seconds: float,
            heartbeat_every: float) -> str:
    campaign = store.get_campaign(job["campaign_id"])
    outcome = executor.execute(
        campaign, job["stage"], job["attempt_count"],
        heartbeat=lambda: store.heartbeat(job["job_id"], job["claim_token"], lease_seconds=lease_seconds),
        heartbeat_every=heartbeat_every)
    store.finish(job["job_id"], job["claim_token"], status=outcome.status, exit_code=outcome.exit_code, reason=outcome.reason,
                 output=outcome.output)
    return outcome.status


def worker_loop(store: FactoryStore, executor: StageExecutor, worker_id: str, *, max_running: int, lease_seconds: float = 900.0,
                heartbeat_every: float = 60.0, budget: list[int] | None = None, tally: dict[str, int] | None = None,
                lock: threading.Lock | None = None, on_job: Callable[[dict[str, Any], str], None] | None = None) -> int:
    ran = 0
    while True:
        if budget is not None:
            with lock or threading.Lock():
                if budget[0] <= 0:
                    return ran
                budget[0] -= 1
        job = store.claim_next(worker_id, max_running=max_running, lease_seconds=lease_seconds)
        if job is None:
            if budget is not None:
                with lock or threading.Lock():
                    budget[0] += 1                 # the reserved slot was not used
            return ran
        status = run_one(store, executor, job, worker_id, lease_seconds, heartbeat_every)
        ran += 1
        if tally is not None:
            with lock or threading.Lock():
                tally[status] = tally.get(status, 0) + 1
        if on_job:
            on_job(job, status)


def run_pass(store: FactoryStore, executor: StageExecutor, *, workers: int = 2, lease_seconds: float = 900.0,
             heartbeat_every: float = 60.0, max_jobs: int | None = None, worker_prefix: str = "w",
             reevaluate_blocked: bool = True) -> PassResult:
    if workers < 1:
        raise ValueError("workers must be >= 1")
    executor.store = store
    result = PassResult(END_NO_WORK)
    result.interrupted_recovered = store.recover_expired()
    if reevaluate_blocked:
        store.requeue_blocked()                      # a prerequisite may have been supplied since the last pass
    budget = [max_jobs] if max_jobs is not None else None
    lock, tally = threading.Lock(), {}
    threads = [threading.Thread(target=worker_loop, name=f"{worker_prefix}{i}", daemon=False, args=(store, executor, f"{worker_prefix}{i}-{id(store) % 9973}"),
                                kwargs=dict(max_running=workers, lease_seconds=lease_seconds, heartbeat_every=heartbeat_every,
                                            budget=budget, tally=tally, lock=lock)) for i in range(workers)]
    for t in threads:
        t.start()
    for t in threads:
        t.join()
    result.by_status = tally
    result.jobs_run = sum(tally.values())
    snap = store.snapshot()
    result.campaigns = snap["campaigns"]
    states = {c["state"] for c in snap["campaigns"]}
    queued = snap["job_counts"].get("queued", 0)
    if budget is not None and budget[0] <= 0 and queued:
        result.ended = END_BUDGET
    elif "FAILED" in states:
        result.ended = END_FAILED
    elif "BLOCKED" in states:
        result.ended = END_BLOCKED
    else:
        result.ended = END_NO_WORK
    return result


def run_until_idle(store: FactoryStore, executor: StageExecutor, *, workers: int = 2, poll_seconds: float = 0.0, max_passes: int = 100,
                   **kw: Any) -> PassResult:
    """Repeat passes until a pass runs nothing. (Blocked jobs are re-evaluated once per pass, so a pass that only
    re-blocks ends the loop: unattended operation never spins on a missing prerequisite.)"""
    total = PassResult(END_NO_WORK)
    for n in range(max_passes):
        r = run_pass(store, executor, workers=workers, reevaluate_blocked=(n == 0), **kw)
        total.jobs_run += r.jobs_run
        total.interrupted_recovered += r.interrupted_recovered
        for k, v in r.by_status.items():
            total.by_status[k] = total.by_status.get(k, 0) + v
        total.ended, total.campaigns = r.ended, r.campaigns
        progressed = r.by_status.get("succeeded", 0) + r.by_status.get("failed", 0)
        if r.jobs_run == 0 or not progressed or r.ended in (END_FAILED, END_BUDGET):
            break
        if poll_seconds:
            time.sleep(poll_seconds)
    return total
