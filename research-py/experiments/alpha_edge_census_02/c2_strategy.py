"""Census-02 Strategy cell evaluation and approved qualification. Pure functions of (bars, frozen config, frozen decisions):
no result value feeds identity, and nothing here opens a store or reads data."""

from __future__ import annotations

import math
import sys
from pathlib import Path

HERE = Path(__file__).resolve().parent
sys.path.insert(0, str(HERE))
import c2_borrow as bw  # noqa: E402
import c2_grammar as gr  # noqa: E402
import c2_protocol as pr  # noqa: E402
import c2_simulate as sim2  # noqa: E402
import edge_registry as er  # noqa: E402  (Census-01 adjacency definition, reused for the report-only neighbourhood)

MIN_CLOSED_ROUND_TRIPS = 5
BAND_LOW_MAX, BAND_MODERATE_MAX = 14, 29   # 5-14 LOW_SAMPLE, 15-29 MODERATE_SAMPLE, >=30 STRONG_SAMPLE
OUTCOMES = ("QUALIFIED", "NOT_QUALIFIED", "INSUFFICIENT_CLOSED_ROUND_TRIPS", "NOT_EXECUTABLE_HYPOTHESIS_ONLY",
            "NON_EVALUABLE")


def trade_band(closed_round_trips: int) -> str:
    """Confidence classification of the closed round-trip count. Only the typed minimum (<5) is a gate."""
    if closed_round_trips < MIN_CLOSED_ROUND_TRIPS:
        return "INSUFFICIENT"
    if closed_round_trips <= BAND_LOW_MAX:
        return "LOW_SAMPLE"
    return "MODERATE_SAMPLE" if closed_round_trips <= BAND_MODERATE_MAX else "STRONG_SAMPLE"


def _finite(v) -> bool:
    return isinstance(v, (int, float)) and not isinstance(v, bool) and math.isfinite(v)


def strategy_outcome(side: str, rec: dict, rule: str) -> tuple[str, str | None]:
    """(outcome, band). The only veto above evaluability is the typed 5-closed-round-trip minimum; the trade-count band,
    year stability, regime concentration, parameter neighbourhood and drawdown are recorded, never gates."""
    if rec.get("d", "").startswith("HYPOTHESIS_ONLY"):
        return "NOT_EXECUTABLE_HYPOTHESIS_ONLY", None
    if rec.get("d") != "EVALUABLE" or rec.get("executable_pnl") is not True:
        return "NON_EVALUABLE", None
    m = rec["m"]
    cash = m.get("cash_zero") or {}
    needed = [cash.get("net_pnl_usd"), cash.get("round_trips")]
    if side == "short":
        needed.append((m.get("passive_short_hold") or {}).get("net_alpha_usd"))      # a missing benchmark record is not evidence
    if not all(_finite(x) for x in needed):
        return "NON_EVALUABLE", None
    rt = int(cash["round_trips"])
    band = trade_band(rt)
    if band == "INSUFFICIENT":
        return "INSUFFICIENT_CLOSED_ROUND_TRIPS", band
    return ("QUALIFIED" if sim2.qualifies(side, m, rule) else "NOT_QUALIFIED"), band


def evaluate_cell(sd, config: dict, symbol: str, decisions: dict) -> dict:
    """One frozen Strategy cell. Class-A symbols never reach the simulator (hypothesis-only, no P&L)."""
    assumption = decisions.get("etf_borrow_assumption")
    evidence = bw.classify_evidence(symbol, assumption)
    fee = assumption["annual_borrow_fee_bps"] if assumption else None
    rec = sim2.evaluate_short_cell(sd, config, evidence, borrow_fee_bps_annual=fee)
    outcome, band = strategy_outcome(config["side"], rec, decisions["benchmark_rule"])
    cash = (rec.get("m") or {}).get("cash_zero") or {}
    return {**rec, "outcome": outcome, "band": band, "tags": gr.tags(config),
            "report_only": {k: cash.get(k) for k in ("trade_count", "round_trips", "year_concentration_share",
                                                     "regime_concentration_share", "max_drawdown_usd",
                                                     "max_drawdown_frac_of_budget")} if cash else None}


def neighborhood_report(rows: list[dict], configs: list[dict]) -> dict:
    """REPORT_ONLY (never a gate): per cell, the share of same-symbol adjacent-grid neighbours that QUALIFIED."""
    nb = er.neighbor_map(configs)
    by_symbol: dict = {}
    for r in rows:
        by_symbol.setdefault(r["s"], {})[r["c"]] = r["outcome"] == "QUALIFIED"
    out = {}
    for r in rows:
        peers = [by_symbol[r["s"]].get(j) for j in nb[r["c"]]]
        peers = [p for p in peers if p is not None]
        out[r["t"]] = {"neighbors": len(peers), "qualified_neighbor_share": (sum(peers) / len(peers)) if peers else None}
    return out


def edge_records(rows: list[dict]) -> list[dict]:
    """Registry records for QUALIFIED cells only, built from the COMPLETE ledger (the caller proves completeness first)."""
    return [{"edge_id": pr.sha256_canonical({"kind": "strategy_edge", "trial_id": r["t"]})[:32], "trial_id": r["t"],
             "side": r["side"], "class": "DISCOVERED_QUALIFIED", "band": r["band"], "tags": r["tags"],
             "VALIDATION_STATUS": "NOT_VALIDATED", "PROMOTION_AUTHORITY": "NONE"}
            for r in rows if r["outcome"] == "QUALIFIED"]
