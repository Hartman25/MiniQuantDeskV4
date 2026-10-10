"""Durable Strategy Factory control plane (one SQLite file): imports, ideas, operator decisions, campaigns, job queue.

Ownership rules (enforced here, not by convention):
* Every row is append-only or state-machine guarded; an existing campaign/predeclaration is immutable.
* The queue is claimed with one `BEGIN IMMEDIATE` transaction: eligibility, concurrency limits and the claim are decided
  atomically, so two processes can never both own a job. A claim carries a token that `heartbeat`/`finish` must present.
* A lease that expires is recovered by appending an `interrupted` attempt and re-queueing the SAME job; nothing is
  fabricated and no prior attempt row is ever rewritten.
* Retries append attempts to a job; a job is never duplicated, and a campaign's trials are declared once.
Economic identity (trials, attempts, slices) is NOT stored here: `ResearchResultStore` owns it.
"""

from __future__ import annotations

import json
import sqlite3
import time
from contextlib import contextmanager
from pathlib import Path
from typing import Any, Callable, Iterable, Iterator, Mapping, Sequence

from mqk_research.strategy_factory.contracts import sha

SCHEMA_VERSION = 1
STAGES = ("check", "data", "register", "gate", "holdout_pre", "trials", "judge", "backtest", "finalize", "review",
          "holdout_post", "summary", "report")
JOB_STATES = ("queued", "running", "succeeded", "failed", "blocked")
CAMPAIGN_STATES = ("PREDECLARED", "RUNNING", "BLOCKED", "FAILED", "COMPLETED")


class StoreError(Exception):
    """A refused or inconsistent control-plane operation (fail closed)."""


class ClaimLost(StoreError):
    """The caller no longer owns the claim (lease expired and recovered, or already finished): its result is discarded."""


_SCHEMA = """
create table if not exists factory_meta(k text primary key, v text not null);
create table if not exists catalog_imports(
  ledger_sha256 text primary key, family text not null, profile_id text not null, filename text not null,
  file_sha256 text not null, ledger_json text not null, seq integer not null);
create table if not exists idea_versions(
  record_sha256 text primary key, intake_id text not null, record_json text not null, seq integer not null);
create index if not exists idea_versions_intake on idea_versions(intake_id, seq);
create table if not exists operator_decisions(
  decision_sha256 text primary key, kind text not null, intake_id text not null, payload_json text not null, seq integer not null,
  decision_ref text not null, unique(kind, intake_id, decision_ref));
create table if not exists campaigns(
  campaign_id text primary key, spec_sha256 text not null, spec_json text not null, declaration_sha256 text not null,
  declaration_path text not null, run_dir text not null, evidence_grade text not null, state text not null,
  state_reason text, created_seq integer not null);
create table if not exists campaign_trials(
  campaign_id text not null references campaigns(campaign_id), trial_key text not null, strategy_name text not null,
  symbol text not null, source_intake_id text, relationship text, primary key(campaign_id, trial_key));
create table if not exists jobs(
  job_id text primary key, campaign_id text not null references campaigns(campaign_id), stage text not null,
  stage_order integer not null, status text not null, claim_token text, claimed_by text, lease_expires real,
  attempt_count integer not null default 0, last_reason text, unique(campaign_id, stage));
create table if not exists job_attempts(
  attempt_id text primary key, job_id text not null references jobs(job_id), attempt_no integer not null,
  worker_id text not null, started_at real not null, finished_at real, status text not null, exit_code integer,
  reason text, output_sha256 text, output_tail text, unique(job_id, attempt_no));
create table if not exists ai_normalizations(
  norm_sha256 text primary key, intake_id text not null, status text not null, record_json text not null, seq integer not null);
create table if not exists embeddings(
  text_sha256 text not null, model text not null, vector_json text not null, primary key(text_sha256, model));
create table if not exists events(
  seq integer primary key autoincrement, campaign_id text, kind text not null, payload_json text not null);
"""


class FactoryStore:
    def __init__(self, db_path: Path, *, clock: Callable[[], float] = time.time) -> None:
        self.db_path = Path(db_path)
        self.clock = clock
        self.db_path.parent.mkdir(parents=True, exist_ok=True)
        boot = self._connect()
        try:
            boot.executescript(_SCHEMA)            # idempotent "create if not exists"; runs outside an explicit transaction
        finally:
            boot.close()
        with self._tx() as con:
            row = con.execute("select v from factory_meta where k='schema_version'").fetchone()
            if row is None:
                con.execute("insert into factory_meta(k,v) values('schema_version',?)", (str(SCHEMA_VERSION),))
            elif int(row[0]) != SCHEMA_VERSION:
                raise StoreError(f"store schema {row[0]} != supported {SCHEMA_VERSION}")

    # ------------------------------------------------------------------ plumbing
    def _connect(self) -> sqlite3.Connection:
        con = sqlite3.connect(self.db_path, timeout=60, isolation_level=None)
        con.row_factory = sqlite3.Row
        con.execute("pragma journal_mode=wal")
        con.execute("pragma foreign_keys=on")
        con.execute("pragma busy_timeout=60000")
        return con

    @contextmanager
    def _tx(self) -> Iterator[sqlite3.Connection]:
        con = self._connect()
        try:
            con.execute("begin immediate")
            yield con
            con.execute("commit")
        except BaseException:
            if con.in_transaction:
                con.execute("rollback")
            raise
        finally:
            con.close()

    def _read(self, sql: str, args: Sequence[Any] = ()) -> list[sqlite3.Row]:
        con = self._connect()
        try:
            return con.execute(sql, args).fetchall()
        finally:
            con.close()

    @staticmethod
    def _seq(con: sqlite3.Connection, table: str) -> int:
        return int(con.execute(f"select coalesce(max(seq),0)+1 from {table}").fetchone()[0])

    def event(self, con: sqlite3.Connection, campaign_id: str | None, kind: str, payload: Mapping[str, Any]) -> None:
        con.execute("insert into events(campaign_id,kind,payload_json) values(?,?,?)",
                    (campaign_id, kind, json.dumps(payload, sort_keys=True, separators=(",", ":"))))

    # ------------------------------------------------------------------ imports and ideas
    def record_import(self, ledger: Mapping[str, Any]) -> bool:
        """Idempotent by ledger hash. Returns True when this ledger was new."""
        with self._tx() as con:
            if con.execute("select 1 from catalog_imports where ledger_sha256=?", (ledger["ledger_sha256"],)).fetchone():
                return False
            con.execute("insert into catalog_imports values(?,?,?,?,?,?,?)", (
                ledger["ledger_sha256"], ledger["catalog_family"], ledger["profile_id"], ledger["source"]["filename"],
                ledger["source"]["sha256"], json.dumps(ledger, sort_keys=True, separators=(",", ":")),
                self._seq(con, "catalog_imports")))
            self.event(con, None, "catalog_imported", {"ledger_sha256": ledger["ledger_sha256"], "entries": ledger["counts"]["entries"]})
            return True

    def list_imports(self) -> list[dict[str, Any]]:
        return [json.loads(r["ledger_json"]) for r in self._read("select ledger_json from catalog_imports order by seq")]

    def record_ideas(self, records: Iterable[Mapping[str, Any]]) -> int:
        """Append a version of every idea whose record changed. History is never rewritten."""
        added = 0
        with self._tx() as con:
            for rec in records:
                body = json.dumps(rec, sort_keys=True, separators=(",", ":"))
                rid = sha(rec)
                if con.execute("select 1 from idea_versions where record_sha256=?", (rid,)).fetchone():
                    continue
                con.execute("insert into idea_versions values(?,?,?,?)", (rid, rec["intake_id"], body, self._seq(con, "idea_versions")))
                added += 1
        return added

    def latest_ideas(self) -> dict[str, dict[str, Any]]:
        out: dict[str, dict[str, Any]] = {}
        for r in self._read("select intake_id, record_json from idea_versions order by seq"):
            out[r["intake_id"]] = json.loads(r["record_json"])
        return out

    def idea_history(self, intake_id: str) -> list[dict[str, Any]]:
        return [json.loads(r["record_json"]) for r in
                self._read("select record_json from idea_versions where intake_id=? order by seq", (intake_id,))]

    def record_decision(self, kind: str, payload: Mapping[str, Any]) -> bool:
        if kind not in ("parameter", "field", "novelty"):
            raise StoreError(f"unknown decision kind {kind!r}")
        needed = ("intake_id", "decided_by", "rationale", "decision_ref")
        if any(not payload.get(k) for k in needed):
            raise StoreError(f"a decision needs {needed}")
        dsha = sha({"kind": kind, **payload})
        with self._tx() as con:
            if con.execute("select 1 from operator_decisions where decision_sha256=?", (dsha,)).fetchone():
                return False
            clash = con.execute("select 1 from operator_decisions where kind=? and intake_id=? and decision_ref=?",
                                (kind, payload["intake_id"], payload["decision_ref"])).fetchone()
            if clash:
                raise StoreError(f"decision_ref {payload['decision_ref']!r} already recorded with different content")
            con.execute("insert into operator_decisions values(?,?,?,?,?,?)",
                        (dsha, kind, payload["intake_id"], json.dumps(payload, sort_keys=True, separators=(",", ":")),
                         self._seq(con, "operator_decisions"), payload["decision_ref"]))
        return True

    def decisions(self, kind: str) -> list[dict[str, Any]]:
        return [json.loads(r["payload_json"]) for r in
                self._read("select payload_json from operator_decisions where kind=? order by seq", (kind,))]

    def record_normalization(self, rec: Mapping[str, Any]) -> bool:
        """AI provenance (provider/model/version, prompt and raw response hashes, validation). Append-only and never
        part of any economic identity."""
        nid = sha(rec)
        with self._tx() as con:
            if con.execute("select 1 from ai_normalizations where norm_sha256=?", (nid,)).fetchone():
                return False
            con.execute("insert into ai_normalizations values(?,?,?,?,?)", (nid, rec["intake_id"], rec["status"],
                        json.dumps(rec, sort_keys=True, separators=(",", ":")), self._seq(con, "ai_normalizations")))
        return True

    def normalizations(self, intake_id: str) -> list[dict[str, Any]]:
        return [json.loads(r["record_json"]) for r in
                self._read("select record_json from ai_normalizations where intake_id=? order by seq", (intake_id,))]

    def cached_embedding(self, text_sha256: str, model: str) -> list[float] | None:
        rows = self._read("select vector_json from embeddings where text_sha256=? and model=?", (text_sha256, model))
        return json.loads(rows[0]["vector_json"]) if rows else None

    def cache_embedding(self, text_sha256: str, model: str, vector: Sequence[float]) -> None:
        with self._tx() as con:
            con.execute("insert or ignore into embeddings values(?,?,?)", (text_sha256, model, json.dumps(list(vector))))

    # ------------------------------------------------------------------ campaigns
    def prior_campaign_strategies(self) -> tuple[list[str], list[tuple[str, str]]]:
        """(every predeclared campaign id, (campaign id, strategy name) pairs), oldest first. Names only: no attempt, result or
        verdict is read, so prior-search accounting cannot depend on outcomes."""
        with self._tx() as con:
            ids = [r["campaign_id"] for r in con.execute("select campaign_id from campaigns order by created_seq")]
            pairs = [(r["campaign_id"], r["strategy_name"]) for r in con.execute(
                "select distinct t.campaign_id, t.strategy_name from campaign_trials t join campaigns c using(campaign_id) "
                "order by c.created_seq, t.strategy_name")]
        return ids, pairs

    def find_campaign(self, campaign_id: str) -> dict[str, Any] | None:
        with self._tx() as con:
            row = con.execute("select * from campaigns where campaign_id=?", (campaign_id,)).fetchone()
        return dict(row) if row else None

    def create_campaign(self, *, campaign_id: str, spec: Mapping[str, Any], declaration_sha256: str, declaration_path: str,
                        run_dir: str, evidence_grade: str, trials: Sequence[Mapping[str, Any]], stages: Sequence[str] = STAGES,
                        expected_prior_campaigns: Sequence[str] | None = None) -> bool:
        """Freeze a campaign: its spec, declaration identity and complete trial population. Idempotent only for the
        identical content; any difference for an existing id is refused (a predeclaration is immutable). When
        `expected_prior_campaigns` is given, the campaign is refused if the store's campaign history is not exactly that set:
        its declared prior-search disclosure must describe the history it is actually committed against."""
        if not trials or len({t["trial_key"] for t in trials}) != len(trials):
            raise StoreError("a campaign needs a non-empty population of unique trial keys")
        if any(s not in STAGES for s in stages):
            raise StoreError("unknown stage")
        spec_json = json.dumps(spec, sort_keys=True, separators=(",", ":"))
        spec_sha = sha(spec)
        with self._tx() as con:
            row = con.execute("select spec_sha256, declaration_sha256 from campaigns where campaign_id=?", (campaign_id,)).fetchone()
            if row is not None:
                if row["spec_sha256"] != spec_sha or row["declaration_sha256"] != declaration_sha256:
                    raise StoreError(f"campaign {campaign_id!r} exists with a different predeclaration; it is immutable")
                have = {r["trial_key"] for r in con.execute("select trial_key from campaign_trials where campaign_id=?", (campaign_id,))}
                if have != {t["trial_key"] for t in trials}:
                    raise StoreError("the declared trial population differs from the frozen one")
                return False
            if expected_prior_campaigns is not None:
                have = sorted(r["campaign_id"] for r in con.execute("select campaign_id from campaigns"))
                if have != sorted(expected_prior_campaigns):
                    raise StoreError("the prior-search history changed while this campaign was being compiled; recompile it "
                                     f"(declared against {sorted(expected_prior_campaigns)}, store holds {have})")
            con.execute("insert into campaigns values(?,?,?,?,?,?,?,?,?,?)", (
                campaign_id, spec_sha, spec_json, declaration_sha256, declaration_path, run_dir, evidence_grade,
                "PREDECLARED", None, int(con.execute("select coalesce(max(created_seq),0)+1 from campaigns").fetchone()[0])))
            for t in trials:
                con.execute("insert into campaign_trials values(?,?,?,?,?,?)", (
                    campaign_id, t["trial_key"], t["strategy_name"], t["symbol"], t.get("source_intake_id"), t.get("relationship")))
            for order, stage in enumerate(stages):
                con.execute("insert into jobs(job_id,campaign_id,stage,stage_order,status) values(?,?,?,?, 'queued')",
                            (sha({"c": campaign_id, "s": stage})[:24], campaign_id, stage, order))
            self.event(con, campaign_id, "campaign_predeclared", {"spec_sha256": spec_sha, "declaration_sha256": declaration_sha256,
                                                                   "trials": len(trials), "evidence_grade": evidence_grade})
            return True

    def get_campaign(self, campaign_id: str) -> dict[str, Any]:
        rows = self._read("select * from campaigns where campaign_id=?", (campaign_id,))
        if not rows:
            raise StoreError(f"unknown campaign {campaign_id!r}")
        return dict(rows[0])

    def list_campaigns(self) -> list[dict[str, Any]]:
        return [dict(r) for r in self._read("select * from campaigns order by created_seq")]

    def campaign_trials(self, campaign_id: str) -> list[dict[str, Any]]:
        return [dict(r) for r in self._read("select * from campaign_trials where campaign_id=? order by trial_key", (campaign_id,))]

    def list_jobs(self, campaign_id: str | None = None) -> list[dict[str, Any]]:
        sql, args = ("select * from jobs where campaign_id=? order by stage_order", (campaign_id,)) if campaign_id else \
            ("select j.* from jobs j join campaigns c using(campaign_id) order by c.created_seq, j.stage_order", ())
        return [dict(r) for r in self._read(sql, args)]

    def attempts(self, job_id: str) -> list[dict[str, Any]]:
        return [dict(r) for r in self._read("select * from job_attempts where job_id=? order by attempt_no", (job_id,))]

    def events_for(self, campaign_id: str | None = None) -> list[dict[str, Any]]:
        sql = "select * from events" + (" where campaign_id=?" if campaign_id else "") + " order by seq"
        return [dict(r) | {"payload": json.loads(r["payload_json"])} for r in self._read(sql, (campaign_id,) if campaign_id else ())]

    # ------------------------------------------------------------------ queue
    def recover_expired(self, now: float | None = None) -> int:
        """Re-queue every job whose lease expired: append an `interrupted` attempt, never touch prior rows."""
        now = self.clock() if now is None else now
        n = 0
        with self._tx() as con:
            for j in con.execute("select * from jobs where status='running' and lease_expires < ?", (now,)).fetchall():
                con.execute("update job_attempts set status='interrupted', finished_at=?, reason=? where job_id=? and attempt_no=? and status='running'",
                            (now, "lease expired before a terminal result", j["job_id"], j["attempt_count"]))
                con.execute("update jobs set status='queued', claim_token=null, claimed_by=null, lease_expires=null, last_reason=? where job_id=?",
                            ("interrupted: lease expired", j["job_id"]))
                self.event(con, j["campaign_id"], "job_interrupted", {"stage": j["stage"], "attempt": j["attempt_count"]})
                n += 1
        return n

    def claim_next(self, worker_id: str, *, max_running: int, lease_seconds: float = 900.0, now: float | None = None) -> dict[str, Any] | None:
        """Atomically claim the next eligible job: queued, every earlier stage of its campaign succeeded (which also
        serializes a campaign's stages), and fewer than `max_running` jobs running in total."""
        now = self.clock() if now is None else now
        with self._tx() as con:
            if con.execute("select count(*) from jobs where status='running' and lease_expires >= ?", (now,)).fetchone()[0] >= max_running:
                return None
            cand = con.execute("""
                select j.* from jobs j join campaigns c using(campaign_id)
                where j.status='queued' and c.state in ('PREDECLARED','RUNNING')
                  and not exists (select 1 from jobs p where p.campaign_id=j.campaign_id and p.stage_order<j.stage_order and p.status!='succeeded')
                order by c.created_seq, j.stage_order limit 1""").fetchone()
            if cand is None:
                return None
            n = cand["attempt_count"] + 1
            token = sha({"job": cand["job_id"], "attempt": n, "worker": worker_id, "at": now})[:32]
            con.execute("update jobs set status='running', claim_token=?, claimed_by=?, lease_expires=?, attempt_count=? where job_id=?",
                        (token, worker_id, now + lease_seconds, n, cand["job_id"]))
            con.execute("insert into job_attempts(attempt_id,job_id,attempt_no,worker_id,started_at,status) values(?,?,?,?,?, 'running')",
                        (sha({"j": cand["job_id"], "n": n})[:24], cand["job_id"], n, worker_id, now))
            con.execute("update campaigns set state='RUNNING', state_reason=null where campaign_id=? and state='PREDECLARED'", (cand["campaign_id"],))
            self.event(con, cand["campaign_id"], "job_claimed", {"stage": cand["stage"], "attempt": n, "worker": worker_id})
            return {**dict(cand), "status": "running", "claim_token": token, "attempt_count": n}

    def heartbeat(self, job_id: str, token: str, *, lease_seconds: float = 900.0, now: float | None = None) -> None:
        now = self.clock() if now is None else now
        with self._tx() as con:
            cur = con.execute("update jobs set lease_expires=? where job_id=? and claim_token=? and status='running'",
                              (now + lease_seconds, job_id, token))
            if cur.rowcount != 1:
                raise ClaimLost("heartbeat refused: the claim is no longer held")

    def finish(self, job_id: str, token: str, *, status: str, exit_code: int | None, reason: str | None,
               output: str = "", now: float | None = None) -> None:
        if status not in ("succeeded", "failed", "blocked"):
            raise StoreError(f"invalid terminal status {status!r}")
        now = self.clock() if now is None else now
        with self._tx() as con:
            j = con.execute("select * from jobs where job_id=? and claim_token=? and status='running'", (job_id, token)).fetchone()
            if j is None:
                raise ClaimLost("finish refused: the claim is not held (expired, recovered or already finished)")
            con.execute("update job_attempts set status=?, finished_at=?, exit_code=?, reason=?, output_sha256=?, output_tail=? "
                        "where job_id=? and attempt_no=? and status='running'",
                        (status, now, exit_code, reason, sha(output), output[-4000:], job_id, j["attempt_count"]))
            con.execute("update jobs set status=?, claim_token=null, claimed_by=null, lease_expires=null, last_reason=? where job_id=?",
                        (status, reason, job_id))
            self._derive_campaign_state(con, j["campaign_id"])
            self.event(con, j["campaign_id"], "job_finished", {"stage": j["stage"], "status": status, "attempt": j["attempt_count"],
                                                                "reason": reason})

    @staticmethod
    def _derive_campaign_state(con: sqlite3.Connection, campaign_id: str) -> None:
        st = [r["status"] for r in con.execute("select status from jobs where campaign_id=?", (campaign_id,))]
        reason = None
        if all(s == "succeeded" for s in st):
            state = "COMPLETED"
        elif "failed" in st:
            state, reason = "FAILED", "a stage failed"
        elif "blocked" in st:
            state, reason = "BLOCKED", "a stage is blocked on a missing prerequisite"
        else:
            state = "RUNNING"
        con.execute("update campaigns set state=?, state_reason=? where campaign_id=?", (state, reason, campaign_id))

    def unblock(self, campaign_id: str, stage: str, reason: str) -> None:
        """Operator/scheduler: re-queue ONE blocked job (appends a new attempt on the next claim)."""
        with self._tx() as con:
            cur = con.execute("update jobs set status='queued', last_reason=? where campaign_id=? and stage=? and status='blocked'",
                              (reason, campaign_id, stage))
            if cur.rowcount != 1:
                raise StoreError("only a blocked job can be unblocked")
            con.execute("update campaigns set state='RUNNING', state_reason=null where campaign_id=?", (campaign_id,))
            self.event(con, campaign_id, "job_unblocked", {"stage": stage, "reason": reason})

    def retry_failed(self, campaign_id: str, stage: str, reason: str) -> None:
        """Explicit operator retry of an infrastructure-failed stage. The failed attempt stays; a new one is appended."""
        if not reason.strip():
            raise StoreError("a retry needs a reason")
        with self._tx() as con:
            cur = con.execute("update jobs set status='queued', last_reason=? where campaign_id=? and stage=? and status='failed'",
                              (f"operator retry: {reason}", campaign_id, stage))
            if cur.rowcount != 1:
                raise StoreError("only a failed job can be retried")
            con.execute("update campaigns set state='RUNNING', state_reason=null where campaign_id=?", (campaign_id,))
            self.event(con, campaign_id, "job_retry_requested", {"stage": stage, "reason": reason})

    def requeue_blocked(self) -> int:
        n = 0
        with self._tx() as con:
            for j in con.execute("select * from jobs where status='blocked'").fetchall():
                con.execute("update jobs set status='queued', last_reason=? where job_id=?", ("re-evaluated after block", j["job_id"]))
                con.execute("update campaigns set state='RUNNING', state_reason=null where campaign_id=?", (j["campaign_id"],))
                n += 1
        return n

    def snapshot(self) -> dict[str, Any]:
        """Read-only operator view: UNAVAILABLE/EMPTY/PRESENT is the caller's distinction; this is always PRESENT data."""
        jobs = self.list_jobs()
        by: dict[str, int] = {}
        for j in jobs:
            by[j["status"]] = by.get(j["status"], 0) + 1
        return {"campaigns": [{k: c[k] for k in ("campaign_id", "state", "state_reason", "evidence_grade", "declaration_sha256")}
                              for c in self.list_campaigns()], "job_counts": dict(sorted(by.items())),
                "imports": len(self._read("select 1 from catalog_imports")), "ideas": len(self.latest_ideas())}
