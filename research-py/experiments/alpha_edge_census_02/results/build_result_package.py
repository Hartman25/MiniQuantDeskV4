"""Census-02 Discovery result packager. Read-only on the settled run directory; copies the exact output artifacts and derives
settlement statistics, universe dispositions, the bars/provenance manifest and a SHA-256 run manifest. No raw vendor bars are
copied. Result values are never altered. STRICTLY OFFLINE: it reads only settled local artifacts, never acquires data, reads no
credential, opens no network endpoint, and proves the run directory byte-identical before and after.
Usage: python build_result_package.py --run-dir <runs/alpha_edge_census_02> --out <dir> --result-run-head <Result #1 head>"""

from __future__ import annotations

import argparse
import hashlib
import json
import platform
import shutil
import subprocess
import sys
import tempfile
from collections import Counter, defaultdict
from pathlib import Path

HERE = Path(__file__).resolve().parent
sys.path.insert(0, str(HERE.parent))
import c2_borrow as bw  # noqa: E402
import c2_data  # noqa: E402  (import performs no I/O; only its pure contract check and LoadedUniverse type are used)
import c2_factor_eval as fe  # noqa: E402
import c2_population as pop  # noqa: E402
import c2_protocol as pr  # noqa: E402
import c2_runner as rn  # noqa: E402
import census as ce  # noqa: E402  (Census-01 read-only manifest / bar loading)
import data as dt  # noqa: E402  (Census-01 read-only eligibility / bar validation; its acquisition functions are never called)
import search_space as ss1  # noqa: E402
from mqk_research.exp_distributed.storage import ResearchResultStore  # noqa: E402

OUTPUTS = ("campaign_disclosure.json", "strategy_edges.json", "conditional_edges.json", "factor_fdr_report.json",
           "strategy_neighborhood_report_only.json")
STRATEGY_LEDGER, FACTOR_LEDGER = "strategy_trial_ledger.jsonl", "factor_evidence_ledger.jsonl"
# Result #1 package settlement (a package-integrity check on THIS preserved run, not an economic threshold).
RESULT01_COUNTS = {"strategy_trials": 9400, "factors": 1075, "factor_status": {"succeeded": 1065, "not_evaluable": 10},
                   "dispositions": {"ELIGIBLE": 88}}
# The commit Result #1 actually executed at (== origin/main when the run started). Historical provenance: never inferred from the
# current HEAD and never overridable.
RESULT01_RUN_HEAD = "778099c6e96d9395b6cfb5e0eee27f2b8186a00e"


class PackageRefusal(RuntimeError):
    pass


def require_result01_run_head(value) -> str:
    """The recorded result-run head of THIS package is the fixed Result #1 execution head; anything else is refused."""
    if value != RESULT01_RUN_HEAD:
        raise PackageRefusal(f"result-run head {value!r} is not the Result #1 execution head {RESULT01_RUN_HEAD}")
    return value


def run_dir_content_digest(root: Path) -> str:
    """SHA-256 over (relative path, file SHA-256) for every file under `root`: a content-strong read-only proof."""
    h = hashlib.sha256()
    for p in sorted(Path(root).rglob("*")):
        if p.is_file():
            h.update(f"{p.relative_to(root).as_posix()}\0{sha_file(p)}\n".encode("utf-8"))
    return h.hexdigest()


def load_settled_universe_offline(data_dir: Path, request_contract: dict, protocol_id: str) -> "c2_data.LoadedUniverse":
    """Rebuild the eligible universe, bars/provenance manifest and verified bars from SETTLED local artifacts only. A missing,
    malformed or contract-mismatched artifact is a PackageRefusal; nothing is acquired, repaired, written or read from credentials.
    Reuses only read-only Census-01 validation (classify_eligibility / build_bars_manifest / load_bars); never an acquire_* entrance."""
    pr.assert_freeze_gate_passed(protocol_id)
    c2_data.require_request_contract(request_contract)
    data_dir = Path(data_dir)
    seed = json.loads(bw.SEED_UNIVERSE_FILE.read_text(encoding="utf-8"))
    symbols = sorted(seed["symbols"])
    statuses = {}
    for sym in symbols:
        sdir = data_dir / sym
        status_path = sdir / "status.json"
        if not status_path.is_file():
            raise PackageRefusal(f"{sym}: settled status.json is absent; packaging never acquires data")
        try:
            st = json.loads(status_path.read_text(encoding="utf-8"))
        except ValueError:
            raise PackageRefusal(f"{sym}: settled status.json is malformed") from None
        if not isinstance(st, dict) or st.get("request_contract") != dt.REQUEST_CONTRACT:
            raise PackageRefusal(f"{sym}: settled status.json was not acquired under the frozen request contract")
        if st.get("disposition") == "DATA_PRESENT":
            need = {"research_bars.csv", "research_bars_provenance.json", "corporate_actions_provenance.json",
                    *(st.get("artifact_names") or [])}
            missing = sorted(n for n in need if not (sdir / n).is_file())
            if missing:
                raise PackageRefusal(f"{sym}: settled artifact(s) absent: {missing}; packaging never acquires data")
        statuses[sym] = st
    dispositions = {s: dt.classify_eligibility(s, statuses[s], data_dir / s) for s in symbols}
    universe = ss1.build_universe(seed, dispositions)
    manifest = ce.build_bars_manifest(universe, data_dir)
    return c2_data.LoadedUniverse(universe, ce.load_bars(universe, data_dir, manifest), manifest)


def strategy_ledger_bytes(chunk_paths: list[Path], population_ids: list[str]) -> bytes:
    """The settled chunk ledgers, concatenated byte-for-byte in canonical chunk order. Refuses unless the trial ids, in order,
    are exactly the frozen population (count, duplicates, missing, extra and order are all checked)."""
    raw = b"".join(Path(p).read_bytes() for p in chunk_paths)
    lines = raw.split(b"\n")
    if lines[-1] != b"" or any(not ln for ln in lines[:-1]):
        raise PackageRefusal("Strategy chunk ledgers are not newline-terminated non-empty JSONL lines")
    ids = [json.loads(ln)["t"] for ln in lines[:-1]]
    if len(ids) != len(population_ids):
        raise PackageRefusal(f"Strategy ledger has {len(ids)} rows, frozen population has {len(population_ids)}")
    if len(set(ids)) != len(ids):
        raise PackageRefusal("Strategy ledger contains a duplicated trial id")
    if set(ids) != set(population_ids):
        raise PackageRefusal(f"Strategy ledger ids differ from the frozen population: missing={len(set(population_ids) - set(ids))} "
                             f"extra={len(set(ids) - set(population_ids))}")
    if ids != list(population_ids):
        raise PackageRefusal("Strategy ledger is not in frozen population order")
    return raw


def factor_ledger_bytes(records: list[dict], expected_ids: list[str], expected_status: dict | None) -> bytes:
    """One canonical-JSON line per settled factor record, in frozen population order (the registry-bound records are copied, never
    re-evaluated). Refuses on count / duplicate / missing / extra / order, and on a status mix other than `expected_status`."""
    ids = [r["factor_id"] for r in records]
    if len(ids) != len(expected_ids):
        raise PackageRefusal(f"factor ledger has {len(ids)} records, frozen population has {len(expected_ids)}")
    if len(set(ids)) != len(ids):
        raise PackageRefusal("factor ledger contains a duplicated factor id")
    if set(ids) != set(expected_ids):
        raise PackageRefusal(f"factor ledger ids differ from the frozen population: missing={len(set(expected_ids) - set(ids))} "
                             f"extra={len(set(ids) - set(expected_ids))}")
    if ids != list(expected_ids):
        raise PackageRefusal("factor ledger is not in frozen population order")
    status = dict(Counter(r["status"] for r in records))
    if expected_status is not None and status != expected_status:
        raise PackageRefusal(f"factor statuses {status} differ from the expected settlement {expected_status}")
    return b"".join(json.dumps(r, sort_keys=True, separators=(",", ":")).encode("utf-8") + b"\n" for r in records)


def sha_file(p: Path) -> str:
    return hashlib.sha256(Path(p).read_bytes()).hexdigest()


def dump(path: Path, obj) -> None:
    Path(path).write_text(json.dumps(obj, sort_keys=True, indent=1) + "\n", encoding="utf-8", newline="\n")


def git(*a: str) -> str:
    return subprocess.run(["git", *a], cwd=pr.REPO, capture_output=True, text=True, check=True).stdout.strip()


def strategy_summary(rows: list[dict]) -> dict:
    def by(key):
        out = defaultdict(Counter)
        for r in rows:
            out[r[key]][r["outcome"]] += 1
        return {k: dict(sorted(v.items())) for k, v in sorted(out.items())}
    bands = Counter((r["band"] or "NONE", r["outcome"]) for r in rows)
    qual = [r for r in rows if r["outcome"] == "QUALIFIED"]
    return {"trials": len(rows), "outcomes": dict(sorted(Counter(r["outcome"] for r in rows).items())),
            "dispositions": dict(sorted(Counter(r["d"] for r in rows).items())),
            "by_side": by("side"), "by_family": by("family"), "by_symbol": by("s"),
            "trade_count_bands_all": dict(sorted(Counter(r["band"] or "NONE" for r in rows).items())),
            "trade_count_bands_qualified": dict(sorted(Counter(r["band"] for r in qual).items())),
            "band_by_outcome": {f"{b}|{o}": n for (b, o), n in sorted(bands.items())},
            "complement_tagged_trials": sum(1 for r in rows if r["tags"]["complement_of_census01"]),
            "complement_tagged_qualified": sum(1 for r in qual if r["tags"]["complement_of_census01"]),
            "qualified_executable_pnl_true": sum(1 for r in qual if r["executable_pnl"] is True),
            "executable_pnl_true_all": sum(1 for r in rows if r["executable_pnl"] is True)}


def factor_summary(items, records, fdr, cedges, factor_db: Path) -> dict:
    meta = {s.compute_factor_id(): (c["family"], h) for c, h, s in items}
    ev = [r for r in records if r["status"] == "succeeded"]
    pv = {r["factor_id"]: r["pvalue"]["p_value"] for r in records if r.get("pvalue")}
    q = fdr["q_values"]
    cls = Counter(e["class"] for e in cedges)
    def by(idx, ids):
        return dict(sorted(Counter(meta[i][idx] for i in ids).items()))
    edge_ids = [e["factor_id"] for e in cedges]
    store = ResearchResultStore(factor_db)
    atts = [a for _c, _h, s in items for a in store.list_factor_evaluation_attempts(s.compute_factor_id())]
    return {"registered": len(items), "records": len(records), "evaluable": len(ev), "non_evaluable": len(records) - len(ev),
            "non_evaluable_failure_reasons": dict(sorted(Counter(a.get("failure_reason") for a in atts if a["status"] == "not_evaluable").items())),
            "non_evaluable_by_family_horizon": dict(sorted(Counter(f"{meta[a['factor_id']][0]}|h{meta[a['factor_id']][1]}" for a in atts if a["status"] == "not_evaluable").items())),
            "non_evaluable_factor_ids": sorted(fdr["excluded_factor_ids_with_reasons"]),
            "fdr_status": fdr["status"], "fdr_declared_population": fdr["declared_population_count"],
            "fdr_tested_hypotheses": fdr["hypothesis_count"], "fdr_alpha": fdr["alpha"],
            "classes_highest": dict(sorted(cls.items())), "min_raw_p": min(pv.values()), "min_bh_q": min(q.values()),
            "bh_q_le_alpha_count_any_direction": len(fdr["rejected_factor_ids"]),
            "bh_q_le_alpha_and_favorable_direction_count": cls.get("DISCOVERED_STRONG", 0),
            "classes_by_family": {k: dict(sorted(Counter(e["class"] for e in cedges if meta[e["factor_id"]][0] == k).items()))
                                  for k in sorted({m[0] for m in meta.values()})},
            "edges_by_family": by(0, edge_ids), "edges_by_horizon": {str(k): v for k, v in by(1, edge_ids).items()},
            "factors_by_family": by(0, list(meta)), "factors_by_horizon": {str(k): v for k, v in by(1, list(meta)).items()},
            "complement_related_factors": sum(1 for c, _h, _s in items if c["family"] in pop.gr.COMPLEMENT_FAMILIES),
            "attempts_total": len(atts), "attempts_by_status": dict(sorted(Counter(a["status"] for a in atts).items())),
            "attempts_per_factor_max": max(Counter(a["factor_id"] for a in atts).values())}


def parse_args(argv=None):
    ap = argparse.ArgumentParser(description=__doc__)
    ap.add_argument("--run-dir", required=True, type=Path)
    ap.add_argument("--out", required=True, type=Path)
    ap.add_argument("--result-run-head", required=True, help="must equal the fixed Result #1 execution head")
    return ap.parse_args(argv)


def main(argv=None) -> None:
    a = parse_args(argv)
    run, out = a.run_dir.resolve(), a.out.resolve()
    require_result01_run_head(a.result_run_head)
    manifest_path = out / "RUN_MANIFEST.json"
    if manifest_path.is_file() and json.loads(manifest_path.read_text(encoding="utf-8")).get("result_run_head") != RESULT01_RUN_HEAD:
        raise PackageRefusal("the existing RUN_MANIFEST records a different result-run head; historical provenance is immutable")
    for needed in ("registry_strategy.sqlite", "registry_factor.sqlite"):
        if not (run / needed).is_file():
            raise PackageRefusal(f"settled {needed} is absent; packaging never creates a registry")
    before = run_dir_content_digest(run)
    with tempfile.TemporaryDirectory() as td:   # the registry stores run DDL on open: they get private copies, never the run dir
        regs = {n: Path(shutil.copyfile(run / n, Path(td) / n)) for n in ("registry_strategy.sqlite", "registry_factor.sqlite")}
        package(run, out, regs)
    if run_dir_content_digest(run) != before:
        raise PackageRefusal("the settled run directory changed during packaging (read-only invariant violated)")
    print(json.dumps({"out": str(out), "run_dir_unchanged": True}))


def package(run: Path, out: Path, regs: dict) -> None:
    frozen = pr.require_freeze()
    doc = frozen["predeclaration"]
    protocol_id, decisions = doc["protocol_id"], doc["decisions"]
    loaded = load_settled_universe_offline(run / "data", doc["data_request_contract"], protocol_id)
    got = dict(Counter(d["disposition"] for d in loaded.universe["dispositions"].values()))
    if got != RESULT01_COUNTS["dispositions"]:
        raise PackageRefusal(f"universe dispositions {got} differ from the settled Result #1 universe")
    cells = pop.strategy_cells(decisions, protocol_id)
    rows = rn.load_ledger(run, cells)
    pr.assert_complete_ledger([c[3] for c in cells], [r["t"] for r in rows])
    ctx = fe.population_context(loaded.universe, loaded.bars_manifest, protocol_id)
    items = fe.materialize_specs(decisions, ctx, doc["factor_population"])
    records = fe.load_factor_records(regs["registry_factor.sqlite"], run / "factor_records", items)
    fdr = json.loads((run / "factor_fdr_report.json").read_text(encoding="utf-8"))
    cedges = json.loads((run / "conditional_edges.json").read_text(encoding="utf-8"))
    out.mkdir(parents=True, exist_ok=True)
    for name in OUTPUTS:
        shutil.copyfile(run / name, out / name)
    chunk_paths = [rn.chunk_path(run, k) for k in range((len(cells) + rn.CHUNK_SIZE - 1) // rn.CHUNK_SIZE)]
    if len(cells) != RESULT01_COUNTS["strategy_trials"] or len(items) != RESULT01_COUNTS["factors"]:
        raise PackageRefusal("frozen populations differ from the Result #1 counts")
    s_bytes = strategy_ledger_bytes(chunk_paths, [c[3] for c in cells])
    f_bytes = factor_ledger_bytes(records, [s.compute_factor_id() for _c, _h, s in items], RESULT01_COUNTS["factor_status"])
    (out / STRATEGY_LEDGER).write_bytes(s_bytes)
    (out / FACTOR_LEDGER).write_bytes(f_bytes)
    # Written bytes must equal the settled run authority: chunk bytes, and the registry-bound factor records re-read from disk.
    if (out / STRATEGY_LEDGER).read_bytes() != b"".join(p.read_bytes() for p in chunk_paths):
        raise PackageRefusal("preserved Strategy ledger differs from the settled chunk ledgers")
    on_disk = [json.loads(ln) for ln in (out / FACTOR_LEDGER).read_bytes().splitlines()]
    if on_disk != records or on_disk != [fe.cd1.read_factor_record(run / "factor_records", r["factor_id"]) for r in records]:
        raise PackageRefusal("preserved factor ledger differs from the settled factor records")
    store = ResearchResultStore(regs["registry_strategy.sqlite"])
    digest = store.trial_attempt_digest(pr.EXPERIMENT_ID)
    ledger_ids = sorted(r["t"] for r in rows)
    attempts = {"registered_trials": len(digest), "attempts": sum(d["attempts"] for d in digest.values()),
                "succeeded": sum(d["succeeded"] for d in digest.values()), "failed": sum(d["failed"] for d in digest.values()),
                "started_unfinalized": sum(d["started"] for d in digest.values()), "blocked": sum(d["blocked"] for d in digest.values()),
                "max_attempts_per_trial": max(d["attempts"] for d in digest.values()),
                "interrupted_or_retried_trials": sum(1 for d in digest.values() if d["attempts"] != 1),
                "registered_equals_frozen_population": sorted(digest) == sorted(c[3] for c in cells),
                "ledger_trial_ids_sha256": hashlib.sha256("\n".join(ledger_ids).encode()).hexdigest(),
                "trial_attempt_digest_sha256": pr.sha256_canonical(digest)}
    dump(out / "strategy_campaign_summary.json", {"attempts": attempts, **strategy_summary(rows)})
    dump(out / "factor_campaign_summary.json", factor_summary(items, records, fdr, cedges, regs["registry_factor.sqlite"]))
    dump(out / "universe_dispositions.json", {"universe_id": loaded.bars_manifest["universe_id"],
         "seed_symbols": len(loaded.universe["dispositions"]), "eligible": len(loaded.universe["symbols"]),
         "disposition_counts": dict(sorted(Counter(d["disposition"] for d in loaded.universe["dispositions"].values()).items())),
         "dispositions": loaded.universe["dispositions"]})
    mf = loaded.bars_manifest
    dump(out / "bars_provenance_manifest.json", mf)
    last = max(r["last_end_ts"] for r in mf["symbols"].values() if "last_end_ts" in r)
    first = min(r["first_end_ts"] for r in mf["symbols"].values() if "first_end_ts" in r)
    chunks = {p.name: sha_file(p) for p in sorted((run / "chunks").glob("chunk_*.jsonl"))}
    produced = {n: sha_file(out / n) for n in sorted(p.name for p in [*out.glob("*.json"), *out.glob("*.jsonl")])
                if n != "RUN_MANIFEST.json"}
    dump(out / "RUN_MANIFEST.json", {
        "schema_version": "census02_result_package_v1", "status": "DISCOVERY_ONLY", "VALIDATION_STATUS": "NOT_VALIDATED",
        "PROMOTION_AUTHORITY": "NONE", "protocol_id": protocol_id, "behavior_head": doc["behavior_head"],
        "result_run_head": RESULT01_RUN_HEAD, "environment_identity": doc["environment_identity"],
        "runtime_python_platform": platform.platform(), "data_request_contract": doc["data_request_contract"],
        "discovery_window": "[2016-01-01, 2024-01-01)", "bars_first_end_ts": first, "bars_last_end_ts": last,
        "bars_manifest_sha256": mf["manifest_sha256"], "universe_id": mf["universe_id"],
        "strategy_population_root": doc["strategy_population"]["population_root"],
        "factor_coordinate_root": doc["factor_population"]["coordinate_root"],
        "reads_2024_confirmation_holdout": {"bars_with_end_ts_ge_2024_01_01": 0 if last < "2024-01-01" else "VIOLATION",
                                            "fence": "c2_data.load_discovery_universe + fence_bars; request end exclusive 2024-01-01"},
        "attempt_registry": attempts, "strategy_ledger_chunk_sha256": chunks,
        "registry_sqlite_not_committed": {"registry_strategy.sqlite": sha_file(run / "registry_strategy.sqlite"),
                                          "registry_factor.sqlite": sha_file(run / "registry_factor.sqlite")},
        "evidence_ledgers": {
            STRATEGY_LEDGER: {"records": len(cells), "order": "frozen population order (chunk_00000..chunk_00018, line order)",
                              "source": "byte concatenation of the 19 settled chunk ledgers; not recomputed"},
            FACTOR_LEDGER: {"records": len(items), "order": "frozen factor population order",
                            "source": "registry-bound settled factor records (canonical JSON, one per line); not re-evaluated",
                            "status_counts": dict(Counter(r["status"] for r in records))},
            "note": "compact committed copies of the settled evidence; they are NOT the registry SQLite files"},
        "raw_vendor_data": "NOT COMMITTED; per-symbol artifact hashes are in bars_provenance_manifest.json",
        "preserved_artifact_sha256": produced})


if __name__ == "__main__":
    main()
