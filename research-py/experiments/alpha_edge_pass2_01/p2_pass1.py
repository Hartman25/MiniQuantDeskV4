"""Read-only binding to the accepted Pass-1 census evidence, plus the discovery-fenced universe loader. Nothing here
writes to a Pass-1 artifact: sqlite files are opened mode=ro and every file is hashed, never touched."""

from __future__ import annotations

import hashlib
import json
import sqlite3
import sys
from contextlib import closing
from pathlib import Path

HERE = Path(__file__).resolve().parent
EXP1 = HERE.parent / "alpha_edge_census_01"
for _p in (str(HERE), str(EXP1), str(HERE.parents[1] / "src")):
    if _p not in sys.path:
        sys.path.insert(0, _p)

import census as ce  # noqa: E402
import conditional as cd  # noqa: E402
import partitions as pt  # noqa: E402
import search_space as ss  # noqa: E402
import signals as sg  # noqa: E402
from mqk_research.exp_distributed.storage import ResearchResultStore  # noqa: E402

RUN_DIR = HERE.parents[1] / "runs" / "alpha_edge_census_01_corrected"
DATA_DIR = RUN_DIR / "data"
STRATEGY_REGISTRY = RUN_DIR / "registry.sqlite"
FACTOR_REGISTRY = RUN_DIR / "registry_conditional_v3.sqlite"
FACTOR_REC_DIR = RUN_DIR / "factor_eval" / "records"

# Run-directory evidence (raw-byte SHA-256; untracked, never rewritten by git).
RUN_FILES = ("search_ledger_v2.jsonl", "search_ledger_v3.jsonl", "factor_ledger_v3.jsonl", "edge_registry_v3.jsonl",
             "edge_registry_summary_v3.json", "factor_fdr_report_v3.json", "registry.sqlite",
             "registry_conditional_v3.sqlite")
# Committed evidence (SHA-256 over LF-normalised bytes: immune to checkout line-ending conversion).
COMMITTED_FILES = ("ALPHA_CENSUS_SEED_UNIVERSE_V2.json", "ALPHA_CENSUS_GRAMMAR_V2.json", "ALPHA_CENSUS_CONDITION_GRAMMAR_V3.json",
                   "ALPHA_CENSUS_UNIVERSE_V2.json", "ALPHA_CENSUS_PARTITIONS_V2.json", "ALPHA_CENSUS_PROTOCOL_V2.json",
                   "ALPHA_CENSUS_SEARCH_SPACE_V2.json", "ALPHA_CENSUS_BARS_MANIFEST_V2.json",
                   "POPULATION_FREEZE_PROOF_V2.json", "FACTOR_FREEZE_PROOF_V3.json", "CAMPAIGN_EVIDENCE_V2.json",
                   "CAMPAIGN_EVIDENCE_V3.json", "CONDITIONAL_V2_DISPOSITION.json")
ACCEPTED_STRATEGY_EDGES = {"DISCOVERED_WEAK": 2851, "DISCOVERED_MODERATE": 789, "DISCOVERED_STRONG": 0}
ACCEPTED_CONDITIONAL_EDGES = {"DISCOVERED_WEAK": 215, "DISCOVERED_MODERATE": 72, "DISCOVERED_STRONG": 135}


class Pass1EvidenceContradiction(RuntimeError):
    """Required accepted evidence is unavailable or contradicts the pushed-verified Pass-1 result: hard stop."""


def sha_raw(p: Path) -> str:
    return ce.sha256_file(Path(p))


def sha_lf(p: Path) -> str:
    return hashlib.sha256(Path(p).read_bytes().replace(b"\r\n", b"\n")).hexdigest()


def load_json(p: Path) -> dict:
    return json.loads(Path(p).read_text(encoding="utf-8"))


def iter_jsonl(p: Path):
    with open(p, "r", encoding="utf-8") as f:
        for line in f:
            yield json.loads(line)


def chunks_root(n_cells: int, chunk_size: int) -> str:
    h = hashlib.sha256()
    for k in range((n_cells + chunk_size - 1) // chunk_size):
        path = ce.chunk_path(RUN_DIR, k)
        if not path.exists():
            raise Pass1EvidenceContradiction(f"missing Pass-1 chunk {path.name}")
        h.update(f"{path.name}:{ce.sha256_file(path)}\n".encode("utf-8"))
    return h.hexdigest()


def _ro(db: Path) -> sqlite3.Connection:
    return sqlite3.connect(f"file:{Path(db).as_posix()}?mode=ro", uri=True)


def attempt_counts() -> dict:
    """Durable Pass-1 attempt counts straight from the accepted registries (read-only)."""
    with closing(_ro(STRATEGY_REGISTRY)) as con:
        s = dict(con.execute("select a.status, count(*) from research_attempts a join research_trials t on "
                             "t.trial_id=a.trial_id where t.experiment_id=? group by a.status", (ss.EXPERIMENT_ID,)).fetchall())
    with closing(_ro(FACTOR_REGISTRY)) as con:
        c = dict(con.execute("select status, count(*) from research_factor_evaluation_attempts where factor_id in "
                             "(select factor_id from research_factors where family=?) group by status",
                             (cd.FACTOR_FAMILY,)).fetchall())
    return {"strategy": dict(sorted(s.items())), "strategy_total": sum(s.values()),
            "conditional_v3": dict(sorted(c.items())), "conditional_v3_total": sum(c.values())}


def pass1_binding(*, cells_count: int = 38_192, chunk_size: int = 500) -> dict:
    """Hash/count every accepted Pass-1 source and prove its internal consistency; any contradiction hard-stops."""
    run = {n: sha_raw(RUN_DIR / n) for n in RUN_FILES}
    committed = {n: sha_lf(EXP1 / n) for n in COMMITTED_FILES}
    root = chunks_root(cells_count, chunk_size)
    ev3, fz = load_json(EXP1 / "CAMPAIGN_EVIDENCE_V3.json"), load_json(EXP1 / "FACTOR_FREEZE_PROOF_V3.json")
    for name, expected in ev3["output_sha256"].items():
        if run[name] != expected:
            raise Pass1EvidenceContradiction(f"{name} differs from CAMPAIGN_EVIDENCE_V3")
    sb = fz["strategy_binding"]
    if sb["strategy_chunks_root_sha256"] != root or sb["strategy_search_ledger_sha256"] != run["search_ledger_v2.jsonl"]:
        raise Pass1EvidenceContradiction("Strategy chunk root / ledger differs from FACTOR_FREEZE_PROOF_V3")
    if ev3["strategy_edges"] != ACCEPTED_STRATEGY_EDGES or ev3["conditional_edges"] != ACCEPTED_CONDITIONAL_EDGES:
        raise Pass1EvidenceContradiction("campaign evidence edge counts differ from the accepted result")
    counts = attempt_counts()
    if counts["strategy_total"] != ev3["strategy_attempts"] or counts["conditional_v3_total"] != ev3["factor_attempts"] \
            or set(counts["strategy"]) != {"succeeded"}:
        raise Pass1EvidenceContradiction("durable attempt counts differ from the campaign evidence")
    return {"run_files_sha256": run, "committed_files_sha256_lf": committed, "strategy_chunks_root_sha256": root,
            "attempt_counts": counts, "bars_manifest_sha256": ev3["bars_manifest_sha256"],
            "factor_freeze_conditional_population_root": fz["conditional_factor_population_root"],
            "strategy_population_root": sb["strategy_population_root"]}


def verify_binding(expected: dict) -> dict:
    """Recompute the binding and require byte/hash/count equality with the predeclared one."""
    now = pass1_binding()
    if now != expected:
        diff = sorted(k for k in expected if expected[k] != now.get(k))
        raise Pass1EvidenceContradiction(f"Pass-1 evidence differs from the predeclared binding: {diff}")
    return now


class ReadOnlyStore(ResearchResultStore):
    """Accepted store API over an immutable Pass-1 registry: opened mode=ro and never initialised, so it cannot write."""

    def __init__(self, db_path: Path) -> None:  # noqa: D107 - deliberately skips _initialize
        self.db_path = Path(db_path)

    def _connect(self) -> sqlite3.Connection:
        con = _ro(self.db_path)
        con.row_factory = sqlite3.Row
        return con


def fence_bars(bars: dict) -> dict:
    """Refuse any row at or after 2024-01-01 (contaminated year, confirmation reserve, final holdout). Returns the proof
    that no forbidden-partition row was read."""
    rows, latest = 0, None
    for sym in sorted(bars):
        ts = bars[sym]["end_ts"]
        pt.require_discovery_only(ts, what=f"pass2 bars {sym}")
        rows += len(ts)
        m = max(ts)
        latest = m if latest is None or m > latest else latest
    return {"symbols": len(bars), "rows_loaded": rows, "max_end_ts": str(latest),
            "rows_in_forbidden_partitions": 0, "discovery_end_exclusive": str(pt.DISCOVERY_END_EXCLUSIVE)}


def load_discovery_universe(uni: dict, bm: dict, data_dir: Path = DATA_DIR):
    """(Universe, fence proof): bars re-verified against the frozen census bars manifest, then discovery-fenced."""
    bars = ce.load_bars(uni, data_dir, bm)
    proof = fence_bars(bars)
    return sg.Universe(bars), proof
