"""Edge Registry V1 post-pass: streams the terminal chunk files and records every finite positive observation.
Records are DISCOVERED / NOT VALIDATED; nothing here selects, ranks for selection, or grants promotion authority."""

from __future__ import annotations

import hashlib
import json
from pathlib import Path

import numpy as np

import search_space as ss
from census import chunk_path

LABEL = "DISCOVERED / NOT VALIDATED"
SCHEMA = "alpha_edge_registry_v1"
TOP_N = 50


class RegistryRefusal(RuntimeError):
    pass


def edge_id(kind: str, trial_id: str, horizon: int | None = None) -> str:
    """Result-independent identity: depends only on (kind, trial_id[, horizon])."""
    body = {"kind": kind, "trial_id": trial_id}
    if horizon is not None:
        body["horizon"] = horizon
    return ss.sha256_canonical(body)[:32]


def _is_num(v) -> bool:
    return isinstance(v, (int, float)) and not isinstance(v, bool)


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
                if rec["t"] != tid or rec["c"] != ci or rec["s"] != (sym or "UNIVERSE"):
                    raise RegistryRefusal(f"{path.name}: line {n} is not the manifest cell")
                n += 1
                yield rec
            if n != len(part) or f.readline():
                raise RegistryRefusal(f"{path.name}: line count differs from manifest chunk")


def build_registry(space: dict, universe: dict, cells, meta: dict, protocol: dict, out_dir: Path,
                   *, chunk_size: int | None = None) -> dict:
    out_dir = Path(out_dir)
    chunk_size = chunk_size or protocol["chunking"]["cells_per_chunk"]
    configs = space["configs"]
    symbols = list(universe["symbols"])
    col = {s: i for i, s in enumerate(symbols)}
    col["UNIVERSE"] = len(symbols)
    ncfg, ncol = len(configs), len(symbols) + 1
    horizons = list(ss.CONDITIONAL_HORIZONS)
    th = protocol["flag_thresholds"]

    pos_s = np.zeros((ncfg, ncol), bool)
    evaluable = np.zeros((ncfg, ncol), bool)
    pos_c = {h: np.zeros((ncfg, ncol), bool) for h in horizons}
    for rec in _iter_lines(out_dir, cells, chunk_size):
        if rec["m"] is None:
            continue
        j = col[rec["s"]]
        evaluable[rec["c"], j] = True
        pos_s[rec["c"], j] = rec["m"]["net_alpha_usd"] > 0.0 and np.isfinite(rec["m"]["net_alpha_usd"])
        for h in horizons:
            q = rec["q"][str(h)]
            if isinstance(q, dict):
                pos_c[h][rec["c"], j] = True

    nb = neighbor_map(configs)

    def neighborhood(mat, ci, j):
        ns = nb[ci]
        npos = int(sum(mat[n, j] for n in ns))
        share = (npos / len(ns)) if ns else None
        island = bool(mat[ci, j] and ns and share < th["parameter_island_neighbor_positive_share"])
        return {"n_neighbors": len(ns), "n_positive_neighbors": npos, "positive_share": share}, island

    def replication(mat, ci, j):
        if configs[ci]["scope"] == "universe":
            return {"class": "NOT_APPLICABLE_UNIVERSE_SCOPE"}
        nev = int(evaluable[ci, :-1].sum())
        npos = int(mat[ci, :-1].sum())
        if npos <= 1:
            cls = "SYMBOL_SPECIFIC"
        elif nev and npos / nev >= 0.5:
            cls = "BROADLY_REPLICATED"
        else:
            cls = "CLUSTER_REPLICATED"
        return {"class": cls, "n_positive_symbols": npos, "n_evaluable_symbols": nev}

    def sym_flags(symbol, members):
        names = members if symbol == "UNIVERSE" else [symbol]
        fl = []
        if any(meta[n].get("data_short_history") for n in names):
            fl.append("data_short_history")
        if any(meta[n].get("data_quality_caveat") for n in names):
            fl.append("data_quality_caveat")
        return fl

    ident = {"search_space_id": space["search_space_id"], "protocol_id": space["protocol_id"],
             "universe_id": space["universe_id"], "partitions_id": space["partitions_id"]}
    labels = {k: universe[k] for k in ("universe_source_kind", "point_in_time_membership", "survivorship_classification")}
    common = {"schema_version": SCHEMA, **ident, **labels, "partition": "DISCOVERY_PRE_2025",
              "VALIDATION_STATUS": "NOT_VALIDATED", "PROMOTION_AUTHORITY": "NONE", "label": LABEL}

    ledger_p, edge_p = out_dir / "search_ledger_v1.jsonl", out_dir / "edge_registry_v1.jsonl"
    summ = {"cells_total": 0, "dispositions": {}, "strategy_edges": 0, "conditional_edges": 0,
            "by_family": {}, "flag_counts": {}, "replication": {}, "parameter_island_edges": 0,
            "conditional_by_horizon": {str(h): 0 for h in horizons}}
    top: list = []
    lh, eh = hashlib.sha256(), hashlib.sha256()

    def bump(d, k, n=1):
        d[k] = d.get(k, 0) + n

    with open(ledger_p, "w", encoding="utf-8", newline="\n") as lf, open(edge_p, "w", encoding="utf-8", newline="\n") as ef:
        def emit(f, h, obj):
            text = json.dumps(obj, sort_keys=True, separators=(",", ":")) + "\n"
            f.write(text)
            h.update(text.encode("utf-8"))

        for rec in _iter_lines(out_dir, cells, chunk_size):
            ci, sym, m = rec["c"], rec["s"], rec["m"]
            cfg = configs[ci]
            fam = cfg["family"]
            j = col[sym]
            summ["cells_total"] += 1
            bump(summ["dispositions"], rec["d"])
            fam_s = summ["by_family"].setdefault(fam, {"cells": 0, "evaluable": 0, "strategy_edges": 0, "conditional_edges": 0})
            fam_s["cells"] += 1
            ledger = {"t": rec["t"], "c": ci, "f": fam, "s": sym, "d": rec["d"]}
            if m is not None:
                fam_s["evaluable"] += 1
                ledger.update(net_alpha_usd=m["net_alpha_usd"], net_pnl_usd=m["net_pnl_usd"], trade_count=m["trade_count"],
                              q={h: (v if not isinstance(v, dict) else [v["n"], v["effect"]]) for h, v in rec["q"].items()})
            emit(lf, lh, ledger)
            if m is None:
                continue
            members = m.get("member_symbols", [])
            base_fl = sym_flags(sym, members)
            if pos_s[ci, j]:
                nbh, island = neighborhood(pos_s, ci, j)
                repl = replication(pos_s, ci, j)
                fl = list(base_fl) + ["survivorship_caveat"]
                if m["trade_count"] < th["tiny_sample_trades"]:
                    fl.append("tiny_sample")
                if (m["round_trips_per_year"] or 0.0) > th["high_turnover_round_trips_per_year"]:
                    fl.append("high_turnover")
                if m["max_drawdown_frac_of_budget"] > th["large_drawdown_frac_of_budget"]:
                    fl.append("large_drawdown")
                fl.append("weak_dsr:DEFERRED_FULL_POPULATION")
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
                edge = {**common, "edge_id": edge_id("STRATEGY_EDGE", rec["t"]), "kind": "STRATEGY_EDGE",
                        "trial_id": rec["t"], "config_id": cfg["config_id"], "family": fam, "params": cfg["params"],
                        "scope": sym, "metrics": m, "flags": fl, "neighborhood": nbh, "replication": repl}
                emit(ef, eh, edge)
                summ["strategy_edges"] += 1
                fam_s["strategy_edges"] += 1
                bump(summ["replication"], "STRATEGY:" + repl["class"])
                summ["parameter_island_edges"] += int(island)
                for f in fl:
                    bump(summ["flag_counts"], "STRATEGY:" + f)
                top.append((m["net_alpha_usd"], rec["t"], sym, fam))
                top.sort(key=lambda x: (-x[0], x[1]))
                del top[TOP_N:]
            for h in horizons:
                q = rec["q"][str(h)]
                if not isinstance(q, dict):
                    continue
                nbh, island = neighborhood(pos_c[h], ci, j)
                repl = replication(pos_c[h], ci, j)
                fl = list(base_fl) + ["survivorship_caveat"]
                if q["tiny_sample"]:
                    fl.append("tiny_sample")
                if repl["class"] == "SYMBOL_SPECIFIC":
                    fl.append("single_symbol")
                ys = _share({k: v[1] for k, v in q["year"].items()})
                rs = _share({k: v[1] for k, v in q["regime"].items()})
                if (ys or 0.0) > th["single_year_concentration_share"]:
                    fl.append("single_year_concentration")
                if (rs or 0.0) > th["regime_concentration_share"]:
                    fl.append("regime_concentration")
                if island:
                    fl.append("parameter_island")
                fl.sort()
                edge = {**common, "edge_id": edge_id("CONDITIONAL_EDGE", rec["t"], h), "kind": "CONDITIONAL_EDGE",
                        "trial_id": rec["t"], "config_id": cfg["config_id"], "family": fam, "params": cfg["params"],
                        "scope": sym, "horizon": h, "stats": q, "flags": fl, "neighborhood": nbh, "replication": repl,
                        "executable_pnl": False}
                emit(ef, eh, edge)
                summ["conditional_edges"] += 1
                summ["conditional_by_horizon"][str(h)] += 1
                fam_s["conditional_edges"] += 1
                bump(summ["replication"], "CONDITIONAL:" + repl["class"])
                summ["parameter_island_edges"] += int(island)
                for f in fl:
                    bump(summ["flag_counts"], "CONDITIONAL:" + f)

    summary = {"schema_version": SCHEMA + "_summary", **ident, "label": LABEL, "VALIDATION_STATUS": "NOT_VALIDATED",
               "PROMOTION_AUTHORITY": "NONE", "JUDGE_STATUS": "DEFERRED_FULL_POPULATION", **summ,
               "strategy_cells_expected": len(cells),
               "conditional_queries_expected": len(cells) * len(horizons),
               "search_ledger_sha256": lh.hexdigest(), "edge_registry_sha256": eh.hexdigest(),
               "ranking_readonly_NOT_SELECTION": [
                   {"net_alpha_usd": a, "trial_id": t, "scope": s, "family": f} for a, t, s, f in top]}
    if summary["cells_total"] != len(cells):
        raise RegistryRefusal("ledger cell count differs from the frozen manifest")
    (out_dir / "edge_registry_summary_v1.json").write_text(
        json.dumps(summary, sort_keys=True, indent=1) + "\n", encoding="utf-8", newline="\n")
    return summary
