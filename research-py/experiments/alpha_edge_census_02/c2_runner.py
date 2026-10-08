"""Census-02 campaign runner. ORDER IS THE CONTRACT: (1) require_freeze (committed predeclaration, behavior sources, numerical
runtime, approved policy, population authority); (2) the single guarded loader; (3) discovery-fenced universe; (4) the COMPLETE
Strategy and factor populations are registered and parity-checked; (5) only then any evaluation. Nothing reads data, opens a
store or opens an attempt before step 1."""

from __future__ import annotations

import contextlib
import json
import os
import sys
from pathlib import Path

HERE = Path(__file__).resolve().parent
sys.path.insert(0, str(HERE))
import c2_factor_eval as fe  # noqa: E402
import c2_grammar as gr  # noqa: E402
import c2_population as pop  # noqa: E402
import c2_protocol as pr  # noqa: E402
import c2_strategy as st  # noqa: E402
from mqk_research.exp_distributed.storage import ResearchResultStore  # noqa: E402

DEFAULT_RUN_DIR = HERE.parents[1] / "runs" / "alpha_edge_census_02"
CHUNK_SIZE = 500
ORIGIN = "alpha_census02_strategy"
FREEZE_PREFIX = f"{pr.EXPERIMENT_ID}:POPULATION_FREEZE:"


class CampaignRefusal(RuntimeError):
    pass


class CampaignIncomplete(RuntimeError):
    pass


# --------------------------------------------------------------------------------------- Strategy population registration
def hypothesis_id(family: str) -> str:
    return f"{pr.EXPERIMENT_ID}:{family}"


def register_strategy_population(store: ResearchResultStore, cells, protocol_id: str, ids: dict, frozen: dict, *, batch=5000) -> dict:
    """Register EVERY frozen Strategy trial (all 9,400) before any attempt, then the population freeze marker."""
    seen = set()
    for _i, c, _s, _t in cells:
        if c["family"] not in seen:
            seen.add(c["family"])
            store.register_hypothesis(hypothesis_id=hypothesis_id(c["family"]), experiment_id=pr.EXPERIMENT_ID,
                                      hypothesis_text=gr.FAMILY_NAMES[c["family"]])
    for k in range(0, len(cells), batch):
        store.register_trials_bulk([
            {"trial_id": t, "experiment_id": pr.EXPERIMENT_ID, "hypothesis_id": hypothesis_id(c["family"]),
             "strategy_id": f"{c['family']}:{c['config_id']}", "protocol_id": protocol_id,
             "identity": gr.trial_identity(c, s, ids)} for _i, c, s, t in cells[k:k + batch]])
    marker = {"trial_count": len(cells), "population_root": frozen["population_root"], "protocol_id": protocol_id}
    store.register_hypothesis(hypothesis_id=FREEZE_PREFIX + frozen["population_root"], experiment_id=pr.EXPERIMENT_ID,
                              hypothesis_text=json.dumps(marker, sort_keys=True, separators=(",", ":")))
    return marker


def require_registered_strategy_population(store: ResearchResultStore, cells, frozen: dict, protocol_id: str, *,
                                           allow_attempts: bool) -> dict:
    """Registered trials == frozen population exactly, freeze marker present, and (before attempt #1) zero attempts."""
    expected = {c[3] for c in cells}
    if len(expected) != frozen["trial_count"] or pop.sorted_root(expected) != frozen["population_root"]:
        raise CampaignRefusal("expanded Strategy population differs from the frozen population authority")
    digest = store.trial_attempt_digest(pr.EXPERIMENT_ID)
    if set(digest) != expected:
        raise CampaignRefusal(f"registered != frozen: missing={len(expected - set(digest))} extra={len(set(digest) - expected)}")
    marker = {"trial_count": frozen["trial_count"], "population_root": frozen["population_root"], "protocol_id": protocol_id}
    with contextlib.closing(store._connect()) as con:  # noqa: SLF001 - read-only probe
        row = con.execute("select hypothesis_text from research_hypotheses where hypothesis_id=?",
                          (FREEZE_PREFIX + frozen["population_root"],)).fetchone()
    if row is None or json.loads(row[0]) != marker:
        raise CampaignRefusal("population freeze marker absent or different; attempt before complete registration refused")
    attempts = sum(d["attempts"] for d in digest.values())
    if attempts and not allow_attempts:
        raise CampaignRefusal("attempts already exist; the pre-evaluation check demands zero")
    return {"registered": len(digest), "attempts": attempts}


# ------------------------------------------------------------------------------------------------ Strategy chunk execution
def chunk_path(out_dir: Path, k: int) -> Path:
    return Path(out_dir) / "chunks" / f"chunk_{k:05d}.jsonl"


def evaluate_cell_row(U, cell, decisions: dict, dispositions: dict) -> dict:
    """The ledger row of one frozen cell. An ineligible Class-C symbol is a typed row, never a dropped trial."""
    ci, cfg, sym, tid = cell
    disp = (dispositions.get(sym) or {}).get("disposition", "EXCLUDED_DATA_UNAVAILABLE")
    if disp != "ELIGIBLE" or sym not in U.sd:
        body = {"d": "NON_EVALUABLE_" + disp, "outcome": "NON_EVALUABLE", "band": None, "m": None, "executable_pnl": False,
                "evidence_class": None, "tags": gr.tags(cfg), "ssr": None, "report_only": None}
    else:
        body = st.evaluate_cell(U.sd[sym], cfg, sym, decisions)
    return {"t": tid, "c": ci, "s": sym, "side": cfg["side"], "family": cfg["family"], **body}


def run_strategy_chunks(store, U, cells, decisions: dict, dispositions: dict, out_dir: Path, frozen: dict, protocol_id: str, *,
                        max_chunks=None, log=print) -> dict:
    """Resumable deterministic-order execution. A retry is a NEW ATTEMPT of the same trial; chunking never changes identity."""
    require_registered_strategy_population(store, cells, frozen, protocol_id, allow_attempts=True)
    out_dir = Path(out_dir)
    (out_dir / "chunks").mkdir(parents=True, exist_ok=True)
    digest = store.trial_attempt_digest(pr.EXPERIMENT_ID)
    nchunks, done, ran = (len(cells) + CHUNK_SIZE - 1) // CHUNK_SIZE, 0, 0
    for k in range(nchunks):
        part = cells[k * CHUNK_SIZE:(k + 1) * CHUNK_SIZE]
        path = chunk_path(out_dir, k)
        terminal = (path.exists() and sum(1 for _ in open(path, "rb")) == len(part)
                    and all(digest[c[3]]["succeeded"] >= 1 and not digest[c[3]]["started"] for c in part))
        if terminal:
            done += 1
            continue
        if max_chunks is not None and ran >= max_chunks:
            break
        stale = [{"attempt_id": f"{c[3]}:att{digest[c[3]]['attempts']:04d}", "status": "failed",
                  "failure_reason": "infrastructure_interrupted"} for c in part if digest[c[3]]["started"]]
        if stale:
            store.finalize_attempts_bulk(stale)
        started = store.begin_attempts_bulk([c[3] for c in part], origin=ORIGIN, metadata={"chunk_index": k})
        lines, fin = [], []
        try:
            for cell, (aid, _idx) in zip(part, started):
                row = evaluate_cell_row(U, cell, decisions, dispositions)
                lines.append(json.dumps(row, sort_keys=True, separators=(",", ":")))
                m = (row["m"] or {}).get("cash_zero") or {}
                fin.append({"attempt_id": aid, "status": "succeeded", "result_summary": {
                    "disposition": row["d"], "outcome": row["outcome"], "net_pnl_usd": m.get("net_pnl_usd"),
                    "round_trips": m.get("round_trips")}})
        except Exception as exc:  # noqa: BLE001 - recorded then re-raised: a defect is a hard stop, never retried by outcome
            store.finalize_attempts_bulk([{"attempt_id": a, "status": "failed",
                                           "failure_reason": f"{type(exc).__name__}: {str(exc)[:300]}"} for a, _ in started])
            raise
        tmp = path.with_suffix(".tmp")
        tmp.write_text("\n".join(lines) + "\n", encoding="utf-8", newline="\n")
        os.replace(tmp, path)
        store.finalize_attempts_bulk(fin)
        for c in part:
            d = digest[c[3]]
            d.update(attempts=d["attempts"] + 1, started=0, succeeded=d["succeeded"] + 1)
        ran += 1
        log(f"strategy chunk {k + 1}/{nchunks} complete")
    return {"chunks_total": nchunks, "chunks_skipped_terminal": done, "chunks_run": ran}


def load_ledger(out_dir: Path, cells) -> list[dict]:
    rows = []
    for k in range((len(cells) + CHUNK_SIZE - 1) // CHUNK_SIZE):
        path = chunk_path(out_dir, k)
        if not path.exists():
            raise CampaignIncomplete(f"missing Strategy chunk {path.name}")
        rows += [json.loads(line) for line in path.read_text(encoding="utf-8").splitlines() if line]
    return rows


# ---------------------------------------------------------------------------------------------------------- campaign
def build_outputs(run_dir: Path, cells, items, factor_db: Path, rec_dir: Path, decisions: dict, doc: dict) -> dict:
    """Only after BOTH populations are fully settled: complete-ledger proof, then (and only then) registry records."""
    rows = load_ledger(run_dir, cells)
    population_ids = [c[3] for c in cells]
    pr.assert_complete_ledger(population_ids, [r["t"] for r in rows])
    configs = gr.build_configs(decisions["grammar_tiers"], decisions["complement_handling"] == "EXCLUDE_COMPLEMENTS_BEFORE_FREEZE")
    edges = pr.register_edges(st.edge_records(rows), population_ids, [r["t"] for r in rows])
    records = fe.load_factor_records(factor_db, rec_dir, items)
    fdr = fe.family_fdr_report(factor_db, records)
    cedges = fe.conditional_edge_records(items, records, fdr)
    counts = {o: sum(1 for r in rows if r["outcome"] == o) for o in st.OUTCOMES}
    tagged_trials = sum(1 for r in rows if r["tags"]["complement_of_census01"])
    tagged_factors = sum(1 for c, _h, _s in items if c["family"] in gr.COMPLEMENT_FAMILIES)
    # Tagged complements stay registered and counted; they are NOT independent information when effective trial counts are estimated.
    dependency = {"strategy_trials_registered": len(rows), "strategy_trials_complement_tagged": tagged_trials,
                  "effective_independent_strategy_trials_estimate": len(rows) - tagged_trials,
                  "factors_registered": len(items), "factors_complement_related": tagged_factors,
                  "effective_independent_factors_estimate": len(items) - tagged_factors}
    out = {"strategy_edges.json": edges, "conditional_edges.json": cedges, "factor_fdr_report.json": fdr,
           "strategy_neighborhood_report_only.json": st.neighborhood_report(rows, configs),
           "campaign_disclosure.json": {
               "protocol_id": doc["protocol_id"], "strategy_trials": len(rows), "strategy_outcomes": counts,
               "conditional_factors": len(records), "fdr_status": fdr.get("status"), "global_disclosure": doc["global_disclosure"],
               "complement_dependency": dependency,
               "VALIDATION_STATUS": "NOT_VALIDATED", "PROMOTION_AUTHORITY": "NONE", "dsr_pbo": "DEFERRED_FULL_POPULATION"}}
    for name, obj in out.items():
        (run_dir / name).write_text(json.dumps(obj, sort_keys=True, indent=1) + "\n", encoding="utf-8", newline="\n")
    return {"strategy_edges": len(edges), "conditional_edges": len(cedges), "fdr_status": fdr.get("status"), "outcomes": counts}


def run_campaign(*, repo: Path = pr.REPO, predeclaration: Path = pr.PREDECLARATION_FILE, run_dir: Path = DEFAULT_RUN_DIR,
                 loader=None, max_strategy_chunks=None, max_factors=None, log=print, trace: list | None = None) -> dict:
    """The canonical entrance. The freeze gate is the first load-bearing operation; the loader is the only data entrance."""
    trace = trace if trace is not None else []
    frozen = pr.require_freeze(repo, predeclaration)
    trace.append("require_freeze")
    doc = frozen["predeclaration"]
    decisions, protocol_id = doc["decisions"], doc["protocol_id"]
    if loader is None:
        import c2_data  # lazy: importing it never touches data, but nothing needs it before the gate
        def loader(data_dir, contract):
            return c2_data.load_discovery_universe(data_dir, contract, protocol_id)
    run_dir = Path(run_dir)
    loaded = loader(run_dir / "data", doc["data_request_contract"])
    trace.append("loader")
    U = fe.Universe2(loaded.bars)                                        # discovery fence runs before any array is built
    trace.append("fence")
    ctx = fe.population_context(loaded.universe, loaded.bars_manifest, protocol_id)
    cells = pop.strategy_cells(decisions, protocol_id)
    ids = pop.strategy_ids(decisions, protocol_id)
    run_dir.mkdir(parents=True, exist_ok=True)
    store = ResearchResultStore(run_dir / "registry_strategy.sqlite")
    factor_db = run_dir / "registry_factor.sqlite"
    sfrozen, ffrozen = doc["strategy_population"], doc["factor_population"]
    items = fe.materialize_specs(decisions, ctx, ffrozen)
    register_strategy_population(store, cells, protocol_id, ids, sfrozen)
    fe.register_factor_population(factor_db, items)
    require_registered_strategy_population(store, cells, sfrozen, protocol_id, allow_attempts=True)
    fe.require_registered_factor_population(factor_db, items, allow_attempts=True)
    trace.append("registered_complete_populations")
    dispositions = loaded.universe["dispositions"]
    strat = run_strategy_chunks(store, U, cells, decisions, dispositions, run_dir, sfrozen, protocol_id,
                                max_chunks=max_strategy_chunks, log=log)
    trace.append("strategy_evaluation")
    fact = fe.run_factors(factor_db, run_dir / "factor_eval", run_dir / "factor_records", U, items, max_factors=max_factors, log=log)
    trace.append("factor_evaluation")
    result = {"strategy": strat, "factors": fact, "protocol_id": protocol_id}
    settled = (strat["chunks_run"] + strat["chunks_skipped_terminal"] == strat["chunks_total"]
               and fact["processed"] == fact["factors_total"])
    if settled:
        result["outputs"] = build_outputs(run_dir, cells, items, factor_db, run_dir / "factor_records", decisions, doc)
        trace.append("outputs")
    return result
