"""Edge Registry post-pass. Streams the terminal StrategyEdge chunk files and the authoritative ConditionalEdge factor
evaluations and records ONLY candidates that clear the census floor, each at its highest class
(DISCOVERED_WEAK < MODERATE < STRONG). Every record is NOT_VALIDATED with PROMOTION_AUTHORITY NONE; non-qualifying
candidates stay in the complete search/factor ledgers and are never registry records."""

from __future__ import annotations

import hashlib
import json
import math
from pathlib import Path

import numpy as np

import conditional as cd
import search_space as ss
from census import cell_configs, cell_conditions, chunk_path

LABEL = "DISCOVERED / NOT VALIDATED"
SCHEMA = "alpha_edge_registry_v3"
OUT_SEARCH_LEDGER, OUT_EDGES, OUT_FACTOR_LEDGER = "search_ledger_v3.jsonl", "edge_registry_v3.jsonl", "factor_ledger_v3.jsonl"
OUT_SUMMARY = "edge_registry_summary_v3.json"
WEAK, MODERATE, STRONG = "DISCOVERED_WEAK", "DISCOVERED_MODERATE", "DISCOVERED_STRONG"
BELOW_FLOOR = "POSITIVE_BUT_BELOW_CENSUS_FLOOR"
TOP_N = 50
DSR_MIN, PBO_MAX = 0.5, 0.5

_REQUIRED_STRATEGY_FIELDS = ("window_bars", "round_trips", "net_pnl_usd", "net_alpha_usd", "cost_usd",
                             "benchmark_net_pnl_usd", "benchmark_cost_usd")


class RegistryRefusal(RuntimeError):
    pass


def edge_id(kind: str, identity_id: str) -> str:
    """Result-independent identity: depends only on (kind, trial_id | factor_id)."""
    return ss.sha256_canonical({"kind": kind, "id": identity_id})[:32]


def _is_num(v) -> bool:
    return isinstance(v, (int, float)) and not isinstance(v, bool)


def _finite(v) -> bool:
    return _is_num(v) and math.isfinite(v)


def _all_numbers_finite(obj) -> bool:
    if isinstance(obj, dict):
        return all(_all_numbers_finite(v) for v in obj.values())
    if isinstance(obj, (list, tuple)):
        return all(_all_numbers_finite(v) for v in obj)
    return not _is_num(obj) or math.isfinite(obj)


# ------------------------------------------------------------------------------------------------ qualification

def judge_allows_strong(judge: dict | None) -> bool:
    """STRONG needs an applicable, COMPLETE, authoritative full-population DSR/PBO multiple-testing result."""
    if not judge or judge.get("status") != "COMPLETE" or judge.get("population_complete") is not True:
        return False
    dsr, pbo = judge.get("dsr"), judge.get("pbo")
    return _finite(dsr) and _finite(pbo) and dsr >= DSR_MIN and pbo <= PBO_MAX


def strategy_class(d: str, m: dict | None, judge: dict | None = None) -> str | None:
    """Highest StrategyEdge class of one succeeded, evaluable cell, or None. The cost model and benchmark are part of
    the metrics themselves: absent or non-finite cost/benchmark evidence can never qualify."""
    if d != "EVALUABLE" or not isinstance(m, dict):
        return None
    if not all(_finite(m.get(k)) for k in _REQUIRED_STRATEGY_FIELDS) or not _all_numbers_finite(m):
        return None
    if m["window_bars"] < ss.MIN_EVALUATED_BARS or m["round_trips"] < ss.MIN_CLOSED_ROUND_TRIPS:
        return None
    if not m["net_pnl_usd"] > 0.0:
        return None
    if not m["net_alpha_usd"] > 0.0:
        return WEAK
    return STRONG if judge_allows_strong(judge) else MODERATE


def strategy_positive_below_floor(d: str, m: dict | None, cls: str | None) -> bool:
    """A finite positive net P&L or alpha that failed the floor: reported separately, never a registry entry."""
    if cls is not None or d != "EVALUABLE" or not isinstance(m, dict) or not _all_numbers_finite(m):
        return False
    return (_finite(m.get("net_pnl_usd")) and m["net_pnl_usd"] > 0.0) or \
        (_finite(m.get("net_alpha_usd")) and m["net_alpha_usd"] > 0.0)


def conditional_class(rec: dict, fdr: dict | None) -> str | None:
    """Highest ConditionalEdge class of one authoritative factor evaluation record, or None."""
    if rec.get("status") != "succeeded":
        return None
    ev, p = rec.get("events"), rec.get("pvalue")
    if not isinstance(ev, dict) or ev.get("event_count", 0) < ss.MIN_CONDITIONAL_EVENTS:
        return None
    eff = ev.get("direction_adjusted_effect")
    if not _finite(eff) or not _finite(rec.get("mean_ic")) or not eff > 0.0:
        return None
    pv = p.get("p_value") if isinstance(p, dict) else None
    if not _finite(pv) or pv > ss.CONDITIONAL_P_ALPHA:
        return WEAK
    q = (fdr or {}).get("q_values") if (fdr or {}).get("status") == "complete" else None
    qv = q.get(rec["factor_id"]) if isinstance(q, dict) else None
    if _finite(qv) and qv <= ss.DISCOVERY_FDR_ALPHA:
        return STRONG
    return MODERATE


def conditional_positive_below_floor(rec: dict, cls: str | None) -> bool:
    ev = rec.get("events") if rec.get("status") == "succeeded" else None
    return cls is None and isinstance(ev, dict) and _finite(ev.get("direction_adjusted_effect")) \
        and ev["direction_adjusted_effect"] > 0.0


def authoritative_factor_records(records: list[dict]) -> dict[str, dict]:
    """Highest attempt_index per factor: deterministic and result-independent. A later failed attempt supersedes an
    earlier success exactly as in the repo FDR driver."""
    best: dict[str, dict] = {}
    for r in records:
        cur = best.get(r["factor_id"])
        if cur is None or r["attempt_index"] > cur["attempt_index"]:
            best[r["factor_id"]] = r
    return best


# ------------------------------------------------------------------------------------------------ neighbourhoods

def neighbor_map(configs: list[dict]) -> list[list[int]]:
    """Adjacent-grid neighbours: same family, exactly one numeric param moved to the adjacent distinct value
    (in the sorted values observed among configs that agree on every other param)."""
    groups: dict = {}
    for i, c in enumerate(configs):
        p = c["params"]
        for k, v in p.items():
            if _is_num(v):
                rest = (c["family"], k, ss.canonical({kk: vv for kk, vv in p.items() if kk != k}))
                groups.setdefault(rest, {}).setdefault(v, []).append(i)
    nb: list[set] = [set() for _ in configs]
    for _key, by_val in groups.items():
        vals = sorted(by_val)
        for a, b in zip(vals, vals[1:]):
            for i in by_val[a]:
                for j in by_val[b]:
                    nb[i].add(j)
                    nb[j].add(i)
    return [sorted(x) for x in nb]


def conditional_neighbor_map(conditions: list[dict]) -> list[list[int]]:
    """Adjacency over the semantic condition grid. Fails closed on any execution-only parameter: exit/hold values are
    Strategy execution state and can never make two conditional hypotheses neighbours."""
    for c in conditions:
        if set(c["params"]) - set(ss.CONDITION_PARAM_KEYS[c["family"]]):
            raise RegistryRefusal(f"{c['family']}: execution-only parameter in conditional adjacency")
    return neighbor_map(conditions)


def _share(buckets: dict[str, float]) -> float | None:
    pos = [v for v in buckets.values() if v > 0]
    return (max(pos) / sum(pos)) if pos else None


def _iter_lines(out_dir: Path, cells, chunk_size: int):
    nchunks = (len(cells) + chunk_size - 1) // chunk_size
    for k in range(nchunks):
        path = chunk_path(out_dir, k)
        if not path.exists():
            raise RegistryRefusal(f"missing chunk file {path.name}")
        part = cells[k * chunk_size:(k + 1) * chunk_size]
        n = 0
        with open(path, "r", encoding="utf-8") as f:
            for line, (ci, _cfg, sym, tid) in zip(f, part):
                rec = json.loads(line)
                if rec["t"] != tid or rec["c"] != ci or rec["s"] != sym:
                    raise RegistryRefusal(f"{path.name}: line {n} is not the manifest cell")
                n += 1
                yield rec, _cfg
            if n != len(part) or f.readline():
                raise RegistryRefusal(f"{path.name}: line count differs from manifest chunk")


def require_factor_accounting(conditions: list[dict], ctx: dict, auth: dict, fdr: dict) -> list[str]:
    """Every registered factor has exactly one authoritative record, and the FDR population IS the registered
    population (never a winners subset)."""
    expected = cd.expected_factor_ids(conditions, ctx)
    if set(auth) != set(expected):
        raise RegistryRefusal(f"factor records != registered population: missing={len(set(expected) - set(auth))} "
                              f"extra={len(set(auth) - set(expected))}")
    if sorted(fdr["declared_factor_ids"]) != sorted(expected) or fdr["family"] != cd.FACTOR_FAMILY:
        raise RegistryRefusal("FDR population differs from the registered factor population")
    return expected


# ------------------------------------------------------------------------------------------------ registry build

def build_registry(space: dict, universe: dict, cells, meta: dict, protocol: dict, out_dir: Path, *, ctx: dict,
                   factor_records: list[dict], fdr: dict, judge: dict | None = None,
                   chunk_size: int | None = None) -> dict:
    out_dir = Path(out_dir)
    chunk_size = chunk_size or protocol["chunking"]["cells_per_chunk"]
    configs = cell_configs(cells)
    conditions = cell_conditions(cells)
    symbols = list(universe["symbols"])
    col = {s: i for i, s in enumerate(symbols)}
    ncfg, nsym = len(configs), len(symbols)
    horizons = list(ss.CONDITIONAL_HORIZONS)
    th = protocol["flag_thresholds"]
    cfg_index = {c["config_id"]: i for i, c in enumerate(configs)}
    cond_index = {c["condition_id"]: i for i, c in enumerate(conditions)}

    auth = authoritative_factor_records(factor_records)
    require_factor_accounting(conditions, ctx, auth, fdr)
    fclass = {fid: conditional_class(r, fdr) for fid, r in auth.items()}

    qual_s = np.zeros((ncfg, nsym), bool)
    evaluable = np.zeros((ncfg, nsym), bool)
    for rec, cfg in _iter_lines(out_dir, cells, chunk_size):
        if rec["m"] is None:
            continue
        i, j = cfg_index[cfg["config_id"]], col[rec["s"]]
        evaluable[i, j] = True
        qual_s[i, j] = strategy_class(rec["d"], rec["m"], judge) is not None
    qual_c = {h: np.zeros(len(conditions), bool) for h in horizons}
    for fid, r in auth.items():
        qual_c[r["horizon"]][cond_index[r["condition_id"]]] = fclass[fid] is not None

    nb, nb_c = neighbor_map(configs), conditional_neighbor_map(conditions)

    def neighborhood(vec, ci, nbmap=None):
        ns = (nb if nbmap is None else nbmap)[ci]
        npos = int(sum(vec[n] for n in ns))
        share = (npos / len(ns)) if ns else None
        island = bool(vec[ci] and ns and share < th["parameter_island_neighbor_positive_share"])
        return {"n_neighbors": len(ns), "n_positive_neighbors": npos, "positive_share": share}, island

    def replication(ci):
        nev, npos = int(evaluable[ci].sum()), int(qual_s[ci].sum())
        cls = "SYMBOL_SPECIFIC" if npos <= 1 else ("BROADLY_REPLICATED" if nev and npos / nev >= 0.5 else "CLUSTER_REPLICATED")
        return {"class": cls, "n_qualifying_symbols": npos, "n_evaluable_symbols": nev}

    ident = {"search_space_id": space["search_space_id"], "protocol_id": space["protocol_id"],
             "universe_id": space["universe_id"], "partitions_id": space["partitions_id"]}
    labels = {k: universe[k] for k in ("point_in_time_membership", "survivorship_classification")}
    common = {"schema_version": SCHEMA, **ident, **labels, "partition": "DISCOVERY_2016_2023",
              "VALIDATION_STATUS": "NOT_VALIDATED", "PROMOTION_AUTHORITY": "NONE", "label": LABEL}

    ledger_p, edge_p, fledger_p = out_dir / OUT_SEARCH_LEDGER, out_dir / OUT_EDGES, out_dir / OUT_FACTOR_LEDGER
    classes = (WEAK, MODERATE, STRONG)
    summ = {"cells_total": 0, "dispositions": {}, "by_family": {}, "flag_counts": {},
            "strategy_edges": {c: 0 for c in classes}, "conditional_edges": {c: 0 for c in classes},
            "strategy_positive_below_floor": 0, "conditional_positive_below_floor": 0, "factors_total": 0,
            "factor_statuses": {}, "conditional_by_horizon": {str(h): 0 for h in horizons}, "replication": {},
            "parameter_island_edges": 0}
    top: list = []
    hs = {"l": hashlib.sha256(), "e": hashlib.sha256(), "f": hashlib.sha256()}

    def bump(d, k, n=1):
        d[k] = d.get(k, 0) + n

    def emit(f, key, obj):
        text = json.dumps(obj, sort_keys=True, separators=(",", ":")) + "\n"
        f.write(text)
        hs[key].update(text.encode("utf-8"))

    with open(ledger_p, "w", encoding="utf-8", newline="\n") as lf, open(edge_p, "w", encoding="utf-8", newline="\n") as ef, \
            open(fledger_p, "w", encoding="utf-8", newline="\n") as ff:
        for rec, cfg in _iter_lines(out_dir, cells, chunk_size):
            sym, m, fam = rec["s"], rec["m"], cfg["family"]
            ci, j = cfg_index[cfg["config_id"]], col[sym]
            summ["cells_total"] += 1
            bump(summ["dispositions"], rec["d"])
            fam_s = summ["by_family"].setdefault(fam, {"cells": 0, "evaluable": 0, "strategy_edges": 0,
                                                       "conditional_edges": 0})
            fam_s["cells"] += 1
            cls = strategy_class(rec["d"], m, judge)
            below = strategy_positive_below_floor(rec["d"], m, cls)
            ledger = {"t": rec["t"], "c": rec["c"], "f": fam, "s": sym, "d": rec["d"], "class": cls}
            if m is not None:
                fam_s["evaluable"] += 1
                ledger.update(net_alpha_usd=m["net_alpha_usd"], net_pnl_usd=m["net_pnl_usd"],
                              trade_count=m["trade_count"], round_trips=m["round_trips"], window_bars=m["window_bars"],
                              below_floor=below)
            emit(lf, "l", ledger)
            summ["strategy_positive_below_floor"] += int(below)
            if cls is None:
                continue
            nbh, island = neighborhood(qual_s[:, j], ci)
            repl = replication(ci)
            fl = ["survivorship_caveat"]
            if meta[sym].get("data_short_history"):
                fl.append("data_short_history")
            if meta[sym].get("data_quality_caveat"):
                fl.append("data_quality_caveat")
            if m["trade_count"] < th["tiny_sample_trades"]:
                fl.append("tiny_sample")
            if (m["round_trips_per_year"] or 0.0) > th["high_turnover_round_trips_per_year"]:
                fl.append("high_turnover")
            if m["max_drawdown_frac_of_budget"] > th["large_drawdown_frac_of_budget"]:
                fl.append("large_drawdown")
            if cls != STRONG:
                fl.append("weak_dsr:" + ss.JUDGE_STATUS)
            if repl["class"] == "SYMBOL_SPECIFIC":
                fl.append("single_symbol")
            if (m["year_concentration_share"] or 0.0) > th["single_year_concentration_share"]:
                fl.append("single_year_concentration")
            if (m["regime_concentration_share"] or 0.0) > th["regime_concentration_share"]:
                fl.append("regime_concentration")
            if island:
                fl.append("parameter_island")
            if m["cost_fragile"]:
                fl.append("cost_fragile")
            fl.sort()
            emit(ef, "e", {**common, "edge_id": edge_id("STRATEGY_EDGE", rec["t"]), "kind": "STRATEGY_EDGE",
                           "edge_class": cls, "trial_id": rec["t"], "config_id": cfg["config_id"], "family": fam,
                           "params": cfg["params"], "scope": sym, "metrics": m, "flags": fl, "neighborhood": nbh,
                           "replication": repl, "judge_status": (judge or {}).get("status", ss.JUDGE_STATUS)})
            summ["strategy_edges"][cls] += 1
            fam_s["strategy_edges"] += 1
            bump(summ["replication"], "STRATEGY:" + repl["class"])
            summ["parameter_island_edges"] += int(island)
            for f in fl:
                bump(summ["flag_counts"], "STRATEGY:" + f)
            top.append((m["net_alpha_usd"], rec["t"], sym, fam))
            top.sort(key=lambda x: (-x[0], x[1]))
            del top[TOP_N:]

        for fid in sorted(auth):
            r = auth[fid]
            cond = conditions[cond_index[r["condition_id"]]]
            cls = fclass[fid]
            below = conditional_positive_below_floor(r, cls)
            summ["factors_total"] += 1
            bump(summ["factor_statuses"], r["status"])
            summ["conditional_positive_below_floor"] += int(below)
            ev = r.get("events") or {}
            emit(ff, "f", {"factor_id": fid, "evaluation_id": r["evaluation_id"], "condition_id": r["condition_id"],
                           "family": r["family"], "horizon": r["horizon"], "status": r["status"],
                           "attempt_index": r["attempt_index"], "class": cls, "below_floor": below,
                           "events": ev.get("event_count"), "effect": ev.get("direction_adjusted_effect"),
                           "p_value": (r.get("pvalue") or {}).get("p_value"),
                           "q_value": ((fdr.get("q_values") or {}).get(fid))})
            if cls is None:
                continue
            nbh, island = neighborhood(qual_c[r["horizon"]], cond_index[r["condition_id"]], nb_c)
            fl = ["survivorship_caveat"]
            if ev["symbols_represented"] == 1:
                fl.append("single_symbol")
            if ev["top_symbol_event_share"] > th["single_year_concentration_share"]:
                fl.append("symbol_concentration")
            ys = _share({k: v["effect"] * v["n"] for k, v in ev["per_year"].items()})
            rs = _share({k: v["effect"] * v["n"] for k, v in ev["per_regime"].items()})
            if (ys or 0.0) > th["single_year_concentration_share"]:
                fl.append("single_year_concentration")
            if (rs or 0.0) > th["regime_concentration_share"]:
                fl.append("regime_concentration")
            if island:
                fl.append("parameter_island")
            fl.sort()
            emit(ef, "e", {**common, "edge_id": edge_id("CONDITIONAL_EDGE", fid), "kind": "CONDITIONAL_EDGE",
                           "edge_class": cls, "factor_id": fid, "evaluation_id": r["evaluation_id"],
                           "condition_id": cond["condition_id"], "family": r["family"], "params": cond["params"],
                           "source_config_count": len(cond["source_config_ids"]),
                           "horizon": r["horizon"], "events": ev, "pvalue": r["pvalue"],
                           "q_value": (fdr.get("q_values") or {}).get(fid), "fdr_status": fdr["status"],
                           "flags": fl, "neighborhood": nbh, "executable_pnl": False})
            summ["conditional_edges"][cls] += 1
            summ["conditional_by_horizon"][str(r["horizon"])] += 1
            summ["by_family"][r["family"]]["conditional_edges"] += 1
            bump(summ["replication"], "CONDITIONAL:symbols_represented=" + ("1" if ev["symbols_represented"] == 1 else ">1"))
            summ["parameter_island_edges"] += int(island)
            for f in fl:
                bump(summ["flag_counts"], "CONDITIONAL:" + f)

    if summ["cells_total"] != len(cells):
        raise RegistryRefusal("ledger cell count differs from the frozen manifest")
    summary = {"schema_version": SCHEMA + "_summary", **ident, "label": LABEL, "VALIDATION_STATUS": "NOT_VALIDATED",
               "PROMOTION_AUTHORITY": "NONE", "JUDGE_STATUS": (judge or {}).get("status", ss.JUDGE_STATUS),
               "FDR_STATUS": fdr["status"], "FDR_ALPHA": fdr["alpha"], **summ,
               "strategy_cells_expected": len(cells), "conditional_factors_expected": summ["factors_total"],
               "search_ledger_sha256": hs["l"].hexdigest(), "edge_registry_sha256": hs["e"].hexdigest(),
               "factor_ledger_sha256": hs["f"].hexdigest(),
               "ranking_readonly_NOT_SELECTION": [
                   {"net_alpha_usd": a, "trial_id": t, "scope": s, "family": f} for a, t, s, f in top]}
    (out_dir / OUT_SUMMARY).write_text(
        json.dumps(summary, sort_keys=True, indent=1) + "\n", encoding="utf-8", newline="\n")
    return summary
