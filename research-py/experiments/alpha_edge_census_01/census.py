"""Census orchestration: provenance-bound data load, cell evaluation, registration gate, resumable executor."""

from __future__ import annotations

import hashlib
import json
import os
import sys
from pathlib import Path

HERE = Path(__file__).resolve().parent
sys.path.insert(0, str(HERE))
sys.path.insert(0, str(HERE.parents[1] / "src"))

import search_space as ss  # noqa: E402
import signals as sg  # noqa: E402
import simulate as sm  # noqa: E402
from data import load_symbol_bars  # noqa: E402
from mqk_research.exp_distributed.storage import ResearchResultStore  # noqa: E402

DATA_PRESENT = "DATA_PRESENT"
NE_SIGNAL = "NON_EVALUABLE_SIGNAL_UNDEFINED_INSUFFICIENT_HISTORY"
FREEZE_PREFIX = f"{ss.EXPERIMENT_ID}:POPULATION_FREEZE:"
ORIGIN = "alpha_census_pass1"


class GateRefusal(RuntimeError):
    pass


def _sha_file(p: Path) -> str:
    return hashlib.sha256(p.read_bytes()).hexdigest()


def build_bars_manifest(universe: dict, data_dir: Path) -> dict:
    """Immutable census-level bars/provenance manifest: one verified row per universe symbol."""
    rows = {}
    for sym in universe["symbols"]:
        sd = Path(data_dir) / sym
        status = json.loads((sd / "status.json").read_text(encoding="utf-8")) if (sd / "status.json").exists() else None
        if status is None or status.get("disposition") != DATA_PRESENT:
            rows[sym] = {"disposition": (status or {}).get("disposition", "DATA_UNAVAILABLE_NOT_ACQUIRED")}
            continue
        bars, prov = load_symbol_bars(sd)
        rows[sym] = {
            "disposition": DATA_PRESENT, "rows": int(len(bars)), "first_end_ts": str(bars["end_ts"].iloc[0]),
            "last_end_ts": str(bars["end_ts"].iloc[-1]), "artifact_sha256": prov["artifact_sha256"],
            "canonical_semantic_bars_hash": prov["canonical_semantic_bars_hash"],
            "canonical_pricing_bars_hash": prov["canonical_pricing_bars_hash"],
            "provenance_file_sha256": _sha_file(sd / "research_bars_provenance.json"),
            "corporate_actions_provenance_sha256": _sha_file(sd / "corporate_actions_provenance.json"),
            "zero_volume_bars": int((bars["volume"] <= 0).sum()),
        }
    from data import REQUEST_CONTRACT
    doc = {"schema_version": "alpha_census_bars_manifest_v2", "request_contract": REQUEST_CONTRACT,
           "universe_id": ss.sha256_canonical(universe)[:32], "symbols": rows}
    doc["manifest_sha256"] = ss.sha256_canonical({k: doc[k] for k in ("request_contract", "universe_id", "symbols")})
    return doc


def load_bars(universe: dict, data_dir: Path, bars_manifest: dict) -> dict:
    """Load every DATA_PRESENT symbol, re-verifying each against the frozen census bars manifest."""
    bars = {}
    for sym in universe["symbols"]:
        row = bars_manifest["symbols"][sym]
        if row["disposition"] != DATA_PRESENT:
            continue
        b, prov = load_symbol_bars(Path(data_dir) / sym)
        if prov["artifact_sha256"] != row["artifact_sha256"] or \
                prov["canonical_semantic_bars_hash"] != row["canonical_semantic_bars_hash"]:
            raise GateRefusal(f"{sym}: bars differ from the frozen census bars manifest")
        bars[sym] = b
    return bars


def symbol_meta(bars_manifest: dict, protocol: dict) -> dict:
    th = protocol["flag_thresholds"]
    out = {}
    for sym, r in bars_manifest["symbols"].items():
        if r["disposition"] != DATA_PRESENT:
            out[sym] = {"disposition": r["disposition"]}
            continue
        out[sym] = {"disposition": DATA_PRESENT, "rows": r["rows"],
                    "data_short_history": r["rows"] < th["data_short_history_rows"],
                    "data_quality_caveat": r["zero_volume_bars"] >= th["data_quality_caveat_zero_volume_bars"]}
    return out


def bench(sd, s: int) -> "sm.SimOut":
    return sd.memo(("bench", s), lambda: sm.simulate(sd.hm, sd.lm, sd.cm, sm.benchmark_d(sd.n, s), s))


def evaluate_cell(U, config: dict, symbol: str, meta: dict) -> dict:
    """StrategyEdge economics of one (config, symbol) cell: pure function of the frozen config and bars. Returns the
    chunk-line body (without trial id / index). Conditional (factor) evidence is a separate registered evaluation."""
    if meta[symbol]["disposition"] != DATA_PRESENT:
        return {"d": "NON_EVALUABLE_" + meta[symbol]["disposition"], "m": None}
    sd = U.sd[symbol]
    sig = U.build(symbol, config["family"], config["params"])
    if sig is None:
        return {"d": NE_SIGNAL, "m": None}
    so = sm.simulate(sd.hm, sd.lm, sd.cm, sig.d, sig.s)
    met = sm.metrics(so, bench(sd, sig.s), sd.years, sd.regime, capital_usd=sm.CAPITAL_USD,
                     budget_usd=sm.BUDGET_USD, window_start=sig.s + 1)
    met["signal_start_bar"] = int(sig.s)
    return {"d": "EVALUABLE", "m": met}


# --------------------------------------------------------------------------------------------- registry / gate

def grammar_configs(space: dict) -> list[dict]:
    """The exact 434-config grammar authority, bound to the frozen search space by grammar_id."""
    g = ss.build_grammar()
    if g["grammar_id"] != space["grammar_id"]:
        raise GateRefusal("search space grammar_id differs from the grammar authority")
    ss.assert_grammar_authority(g["configs"])
    return g["configs"]


def cell_configs(cells) -> list[dict]:
    """Distinct Strategy configs of the cell population in manifest order."""
    seen, out = set(), []
    for _i, c, _s, _t in cells:
        if c["config_id"] not in seen:
            seen.add(c["config_id"])
            out.append(c)
    return out


def cell_conditions(cells) -> list[dict]:
    """Semantic ConditionalEdge conditions projected from the Strategy grammar (execution-only params dropped)."""
    return ss.build_conditions(cell_configs(cells))


def population(universe: dict, space: dict):
    """(configs, symbols, ids, [(config_index, config, symbol, trial_id)]) in manifest order."""
    ids = {k: space[k] for k in ("universe_id", "partitions_id", "protocol_id")}
    configs, symbols = grammar_configs(space), universe["symbols"]
    cells = []
    index = {c["config_id"]: i for i, c in enumerate(configs)}
    for c, s, tid in ss.iter_cells(configs, symbols, ids):
        cells.append((index[c["config_id"]], c, s, tid))
    return configs, symbols, ids, cells


def sorted_root(trial_ids) -> str:
    h = hashlib.sha256()
    for t in sorted(trial_ids):
        h.update(t.encode("ascii") + b"\n")
    return h.hexdigest()


def freeze_record(space: dict, cells) -> dict:
    tids = [c[3] for c in cells]
    if len(set(tids)) != len(tids):
        raise GateRefusal("duplicate cell in expected population")
    return {"search_space_id": space["search_space_id"], "strategy_cell_count": len(tids),
            "manifest_order_root": space["population_root_sha256"], "strategy_population_root": sorted_root(tids)}


def register_population(store: ResearchResultStore, space: dict, cells, *, batch=5000) -> dict:
    """Register every expected StrategyEdge trial before any attempt, then write the Strategy population freeze
    marker. The ConditionalEdge factor population has its own freeze (conditional.py / run_census freeze-factors)."""
    fam_seen = set()
    for _i, c, _s, _t in cells:
        if c["family"] not in fam_seen:
            fam_seen.add(c["family"])
            store.register_hypothesis(hypothesis_id=ss.hypothesis_id(c["family"]), experiment_id=ss.EXPERIMENT_ID,
                                      hypothesis_text=ss.FAMILIES[c["family"]][0])
    ids = {k: space[k] for k in ("universe_id", "partitions_id", "protocol_id")}
    for k in range(0, len(cells), batch):
        rows = [{"trial_id": t, "experiment_id": ss.EXPERIMENT_ID, "hypothesis_id": ss.hypothesis_id(c["family"]),
                 "strategy_id": f"{c['family']}:{c['config_id']}", "protocol_id": ids["protocol_id"],
                 "identity": ss.trial_identity(c, s, ids)} for _i, c, s, t in cells[k:k + batch]]
        store.register_trials_bulk(rows)
    freeze = freeze_record(space, cells)
    store.register_hypothesis(hypothesis_id=FREEZE_PREFIX + freeze["strategy_population_root"],
                              experiment_id=ss.EXPERIMENT_ID,
                              hypothesis_text=json.dumps(freeze, sort_keys=True, separators=(",", ":")))
    return freeze


def require_frozen_population(store: ResearchResultStore, space: dict, cells, *, allow_attempts: bool) -> dict:
    """Refuse unless registered trials == expected StrategyEdge population exactly, the Strategy freeze marker matches
    (its strategy fields; any historical conditional fields of an older marker are not authority), and (before the
    first attempt) all attempts == 0."""
    freeze = freeze_record(space, cells)
    digest = store.trial_attempt_digest(ss.EXPERIMENT_ID)
    expected = {c[3] for c in cells}
    got = set(digest)
    if got != expected:
        raise GateRefusal(f"registered != expected: missing={len(expected - got)} extra={len(got - expected)}")
    marker = FREEZE_PREFIX + freeze["strategy_population_root"]
    with __import__("contextlib").closing(store._connect()) as con:  # noqa: SLF001 - read-only probe
        row = con.execute("select hypothesis_text from research_hypotheses where hypothesis_id=?", (marker,)).fetchone()
    if row is None or {k: json.loads(row[0]).get(k) for k in freeze} != freeze:
        raise GateRefusal("population freeze marker absent or different; attempt before population freeze refused")
    strategy_attempts = sum(d["attempts"] for d in digest.values())
    if not allow_attempts and strategy_attempts:
        raise GateRefusal("attempts already exist; freeze check demands attempts == 0")
    return {"registered_trials": len(got), "strategy_attempts": strategy_attempts,
            "strategy_succeeded": sum(d["succeeded"] for d in digest.values()),
            "strategy_failed": sum(d["failed"] for d in digest.values()),
            "strategy_started": sum(d["started"] for d in digest.values()), **freeze}


def sha256_file(p: Path) -> str:
    h = hashlib.sha256()
    with open(p, "rb") as f:
        for block in iter(lambda: f.read(1 << 20), b""):
            h.update(block)
    return h.hexdigest()


def strategy_binding(store: ResearchResultStore, space: dict, cells, run_dir: Path, *, chunk_size: int,
                     ledger_path: Path, expected_ledger_sha256: str) -> dict:
    """The immutable, accepted StrategyEdge state a ConditionalEdge freeze binds to: the frozen population, every
    attempt terminal and succeeded (no failed/started), the raw result chunks and the accepted search ledger. Any
    drift refuses, so a V3 factor run can never proceed over a changed or incomplete Strategy result."""
    gate = require_frozen_population(store, space, cells, allow_attempts=True)
    n = len(cells)
    if not (gate["strategy_attempts"] == gate["strategy_succeeded"] == n and not gate["strategy_failed"]
            and not gate["strategy_started"]):
        raise GateRefusal("StrategyEdge population is not in the accepted terminal state (one succeeded attempt per trial)")
    h = hashlib.sha256()
    for k in range((n + chunk_size - 1) // chunk_size):
        path = chunk_path(run_dir, k)
        if not path.exists():
            raise GateRefusal(f"missing StrategyEdge chunk file {path.name}")
        h.update(f"{path.name}:{sha256_file(path)}\n".encode("utf-8"))
    ledger = sha256_file(ledger_path)
    if ledger != expected_ledger_sha256:
        raise GateRefusal("StrategyEdge search ledger differs from the accepted ledger hash")
    return {"strategy_population_root": gate["strategy_population_root"], "strategy_cell_count": n,
            "manifest_order_root": gate["manifest_order_root"], "strategy_attempts": gate["strategy_attempts"],
            "strategy_attempts_succeeded": gate["strategy_succeeded"], "strategy_attempts_failed": 0,
            "strategy_chunks_root_sha256": h.hexdigest(), "strategy_search_ledger_sha256": ledger}


def _line_count(p: Path) -> int:
    with open(p, "rb") as f:
        return sum(1 for _ in f)


def chunk_path(out_dir: Path, k: int) -> Path:
    return Path(out_dir) / "chunks" / f"chunk_{k:05d}.jsonl"


def run_chunks(store, U, space, cells, meta, out_dir: Path, *, chunk_size=500, max_chunks=None, log=print) -> dict:
    """Resumable, deterministic-order execution. Chunking never changes identity or economics."""
    require_frozen_population(store, space, cells, allow_attempts=True)
    out_dir = Path(out_dir)
    (out_dir / "chunks").mkdir(parents=True, exist_ok=True)
    digest = store.trial_attempt_digest(ss.EXPERIMENT_ID)
    nchunks = (len(cells) + chunk_size - 1) // chunk_size
    done = ran = 0
    for k in range(nchunks):
        part = cells[k * chunk_size:(k + 1) * chunk_size]
        path = chunk_path(out_dir, k)
        terminal = (path.exists() and _line_count(path) == len(part)
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
            for (ci, cfg, sym, tid), (aid, _idx) in zip(part, started):
                body = evaluate_cell(U, cfg, sym, meta)
                lines.append(json.dumps({"t": tid, "c": ci, "s": sym, **body}, sort_keys=True,
                                        separators=(",", ":")))
                m = body["m"]
                fin.append({"attempt_id": aid, "status": "succeeded", "result_summary": {
                    "disposition": body["d"], "net_alpha_usd": m["net_alpha_usd"] if m else None,
                    "net_pnl_usd": m["net_pnl_usd"] if m else None, "trade_count": m["trade_count"] if m else None}})
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
        log(f"chunk {k + 1}/{nchunks} complete")
    return {"chunks_total": nchunks, "chunks_skipped_terminal": done, "chunks_run": ran}
