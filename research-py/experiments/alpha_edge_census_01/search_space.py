"""Corrected Alpha Edge Census search-space authority: the exact S01-S14 / 434-config StrategyEdge grammar,
result-independent identities and the frozen manifests. Nothing here reads prices or results."""

from __future__ import annotations

import hashlib
import json
import os
import sys
from pathlib import Path

HERE = Path(__file__).resolve().parent
sys.path.insert(0, str(HERE))
sys.path.insert(0, str(HERE.parents[1] / "src"))

from calendar_authority import CONTENT_SHA256 as CALENDAR_SHA256, CONTRACT_ID as CALENDAR_ID  # noqa: E402
from data import ELIGIBLE, EXCLUDED_DISPOSITIONS, MIN_ELIGIBLE_OBSERVATIONS  # noqa: E402
from partitions import PARTITIONS  # noqa: E402

REPO = HERE.parents[2]
SCHEMA = "alpha_census_v2"
EXPERIMENT_ID = "alpha_edge_census_01_corrected"
REJECTED_EXPERIMENT_ID = "alpha_edge_census_01"
TRIAL_PREFIX = "ace2-"
SEED_UNIVERSE_FILE = HERE / "ALPHA_CENSUS_SEED_UNIVERSE_V2.json"
GRAMMAR_FILE = HERE / "ALPHA_CENSUS_GRAMMAR_V2.json"
PARTITIONS_FILE = HERE / "ALPHA_CENSUS_PARTITIONS_V2.json"
PROTOCOL_FILE = HERE / "ALPHA_CENSUS_PROTOCOL_V2.json"
UNIVERSE_FILE = HERE / "ALPHA_CENSUS_UNIVERSE_V2.json"
SEARCH_SPACE_FILE = HERE / "ALPHA_CENSUS_SEARCH_SPACE_V2.json"

CONDITIONAL_HORIZONS = (1, 3, 5, 10, 20)
TRENDS = ("none", "sma200")
CADENCES = ("daily", "month_end")
EXPECTED_FAMILY_IDS = tuple(f"S{i:02d}" for i in range(1, 15))
EXPECTED_FAMILY_COUNTS = {"S01": 8, "S02": 6, "S03": 14, "S04": 12, "S05": 72, "S06": 24, "S07": 108, "S08": 36,
                          "S09": 64, "S10": 12, "S11": 18, "S12": 32, "S13": 24, "S14": 4}
EXPECTED_CONFIG_COUNT = 434
MIN_CLOSED_ROUND_TRIPS = 5
MIN_EVALUATED_BARS = 252
MIN_CONDITIONAL_EVENTS = 30
DISCOVERY_FDR_ALPHA = 0.10
CONDITIONAL_P_ALPHA = 0.10
N_PERMUTATIONS = 200
BASE_SEED = 0
JUDGE_STATUS = "DEFERRED_FULL_POPULATION"
REJECTED_RUN_LABEL = "ALPHA_EDGE_CENSUS_01_REJECTED_EXECUTION_20261005"


class GrammarRefusal(RuntimeError):
    pass


def canonical(obj) -> str:
    return json.dumps(obj, sort_keys=True, separators=(",", ":"), ensure_ascii=False)


def sha256_canonical(obj) -> str:
    return hashlib.sha256(canonical(obj).encode("utf-8")).hexdigest()


def _g(**kw):
    return kw


def _grid_S01():
    return [_g(lookback=L, cadence=c) for L in (21, 63, 126, 252) for c in CADENCES]


def _grid_S02():
    return [_g(sma=m) for m in (20, 50, 100, 150, 200, 250)]


def _grid_S03():
    return [_g(fast=f, slow=s) for f in (10, 20, 50) for s in (50, 100, 150, 200, 250) if f < s]


def _grid_S04():
    return [_g(entry=e, exit=x) for e in (20, 50, 100, 150, 200) for x in (10, 20, 50) if x < e]


def _grid_S05():
    return [_g(period=p, entry_below=e, exit_above=x, trend=t) for p in (2, 3, 5, 7, 10, 14)
            for e in (10, 20, 30) for x in (50, 70) for t in TRENDS]


def _grid_S06():
    return [_g(lookback=L, entry_z=z, exit_z=0.0, trend=t) for L in (10, 20, 40, 60) for z in (-1.5, -2.0, -2.5)
            for t in TRENDS]


def _grid_S07():
    return [_g(decline_sessions=d, atr_window=a, mult=m, hold=h, trend=t) for d in (1, 3, 5) for a in (14, 20)
            for m in (1.0, 1.5, 2.0) for h in (1, 3, 5) for t in TRENDS]


def _grid_S08():
    return [_g(atr_window=a, mult=m, hold=h, trend=t) for a in (14, 20) for m in (1.0, 1.5, 2.0) for h in (1, 3, 5)
            for t in TRENDS]


def _grid_S09():
    return [_g(short=s, long=l, ratio=r, breakout=b, exit=x, trend=t) for s in (5, 10) for l in (40, 60)
            for r in (0.25, 0.40) for b in (20, 50) for x in (10, 20) for t in TRENDS]


def _grid_S10():
    return [_g(high_lookback=L, distance=d, cadence=c) for L in (126, 252) for d in (0.03, 0.05, 0.10) for c in CADENCES]


def _grid_S11():
    return [_g(down_sessions=n, hold=h, trend=t) for n in (2, 3, 4) for h in (1, 3, 5) for t in TRENDS]


def _grid_S12():
    return [_g(volume_lookback=L, volume_z=z, price_impulse=i, mode=m, hold=h) for L in (20, 60) for z in (1.5, 2.0)
            for i in (1, 5) for m in ("continuation", "reversal") for h in (1, 3)]


def _grid_S13():
    return [_g(short_vol=s, long_vol=60, expansion_ratio=r, mode=m, hold=h) for s in (10, 20) for r in (1.5, 2.0)
            for m in ("breakout", "reversal") for h in (1, 3, 5)]


def _grid_S14():
    return [_g(kind="tom", last=a, first=b) for a, b in ((1, 3), (2, 3), (1, 5))] + [_g(kind="season", value="nov_apr")]


FAMILIES = {
    "S01": ("TIME_SERIES_MOMENTUM", "symbol", _grid_S01),
    "S02": ("PRICE_ABOVE_SMA", "symbol", _grid_S02),
    "S03": ("SMA_CROSS", "symbol", _grid_S03),
    "S04": ("DONCHIAN_BREAKOUT", "symbol", _grid_S04),
    "S05": ("RSI_REVERSION", "symbol", _grid_S05),
    "S06": ("ZSCORE_REVERSION", "symbol", _grid_S06),
    "S07": ("ATR_NORMALIZED_REVERSAL", "symbol", _grid_S07),
    "S08": ("OVERNIGHT_GAP_REVERSAL", "symbol", _grid_S08),
    "S09": ("VOLATILITY_CONTRACTION_BREAKOUT", "symbol", _grid_S09),
    "S10": ("HIGH_PROXIMITY", "symbol", _grid_S10),
    "S11": ("CONSECUTIVE_DOWN_REVERSAL", "symbol", _grid_S11),
    "S12": ("VOLUME_PRICE_SURPRISE", "symbol", _grid_S12),
    "S13": ("VOLATILITY_EXPANSION", "symbol", _grid_S13),
    "S14": ("CALENDAR", "symbol", _grid_S14),
}

SIGNAL_SEMANTICS = {
    "S01": "long when log(close_t/close_{t-L})>0; cadence month_end samples the state on the last session of each month and holds",
    "S02": "long while close_t > SMA(close, n)_t",
    "S03": "long while SMA(fast)_t > SMA(slow)_t",
    "S04": "enter when close_t > max(close_{t-entry..t-1}); exit when close_t < min(close_{t-exit..t-1})",
    "S05": "Cutler simple-mean RSI(period); enter RSI<entry_below (and trend filter); exit RSI>exit_above",
    "S06": "z=(close-mean)/std(ddof=1) over lookback; enter z<=entry_z (and trend filter); exit z>=0",
    "S07": "event: close_{t-d}-close_t >= mult*ATR_{t-1} (simple-mean ATR, prior bar) and trend filter; held for `hold` strategy outputs",
    "S08": "event: open_t - close_{t-1} <= -mult*ATR_{t-1}; decision only after bar t completes; held for `hold` strategy outputs",
    "S09": "enter when range(short)/range(long) at t-1 <= ratio and close_t > prior-breakout close channel (and trend filter); exit below prior exit channel",
    "S10": "long while close_t >= (1-distance)*max(close, high_lookback); cadence as S01",
    "S11": "event: `down_sessions` consecutive down closes (and trend filter); held for `hold` strategy outputs",
    "S12": "event: volume z-score vs prior L bars >= volume_z with price impulse sign (continuation up / reversal down); long only",
    "S13": "event: std(short)/std(long) of log returns >= ratio with up-day (breakout) or down-day (reversal); long only",
    "S14": "member(bar t+2) so the held close-to-close return covers a member session: turn-of-month last/first sessions, or Nov-Apr",
}


def config_id(family: str, params: dict) -> str:
    return hashlib.sha256(canonical({"family": family, "params": params}).encode("utf-8")).hexdigest()[:24]


def assert_grammar_authority(configs: list[dict]) -> None:
    """Refuse (before any attempt) unless the family set is exactly S01..S14 and the config count exactly 434."""
    fams = tuple(sorted({c["family"] for c in configs}))
    if fams != EXPECTED_FAMILY_IDS:
        raise GrammarRefusal(f"family set {fams} is not exactly S01..S14")
    if len(configs) != EXPECTED_CONFIG_COUNT:
        raise GrammarRefusal(f"config count {len(configs)} != {EXPECTED_CONFIG_COUNT}")
    counts = {f: sum(1 for c in configs if c["family"] == f) for f in EXPECTED_FAMILY_IDS}
    if counts != EXPECTED_FAMILY_COUNTS:
        raise GrammarRefusal(f"per-family counts {counts} differ from the frozen grammar")
    if any(c["scope"] != "symbol" for c in configs):
        raise GrammarRefusal("every StrategyEdge config must be symbol-scoped")


def build_configs() -> list[dict]:
    """Every parameter coordinate in deterministic family/grid order. Duplicates and any grammar drift are refused."""
    out, seen = [], set()
    for fam, (_name, scope, gen) in FAMILIES.items():
        for params in gen():
            cid = config_id(fam, params)
            if cid in seen:
                raise GrammarRefusal(f"duplicate search coordinate in {fam}: {canonical(params)}")
            seen.add(cid)
            out.append({"config_id": cid, "family": fam, "scope": scope, "params": params})
    assert_grammar_authority(out)
    return out


def trial_identity(config: dict, symbol: str, ids: dict) -> dict:
    """Result-independent identity of one StrategyEdge cell (one config on one symbol)."""
    return {"schema_version": SCHEMA, "kind": "strategy_edge", "family": config["family"],
            "params": config["params"], "scope": symbol, "universe_id": ids["universe_id"],
            "partitions_id": ids["partitions_id"], "protocol_id": ids["protocol_id"]}


def trial_id(identity: dict) -> str:
    return TRIAL_PREFIX + sha256_canonical(identity)[:32]


def hypothesis_id(family: str) -> str:
    return f"{EXPERIMENT_ID}:{family}"


def build_seed_universe() -> dict:
    from mqk_research.universe.snapshot import build_current_enabled_equity_registry_snapshot
    cwd = os.getcwd()
    os.chdir(REPO)
    try:
        snap = build_current_enabled_equity_registry_snapshot(Path("config/instruments/equities.json")).to_json_dict()
    finally:
        os.chdir(cwd)
    if snap["universe_source_kind"] != "current_enabled_equity_registry_snapshot_v1" or snap["point_in_time_membership"] is not False \
            or snap["survivorship_classification"] != "CURRENT_REGISTRY_SNAPSHOT_NOT_POINT_IN_TIME":
        raise RuntimeError("universe snapshot labels differ from the census contract; fail closed")
    return {"schema_version": "alpha_census_seed_universe_v2", "snapshot": snap,
            "symbols": sorted(snap["symbols"]), "symbol_count": snap["symbol_count"],
            "universe_source_kind": snap["universe_source_kind"], "point_in_time_membership": False,
            "survivorship_classification": snap["survivorship_classification"],
            "active_paper_universe_expanded": False}


def validate_dispositions(seed: dict, dispositions: dict) -> None:
    """Every seed symbol carries exactly one ELIGIBLE or typed EXCLUDED_* disposition; nothing else is accepted."""
    if sorted(dispositions) != seed["symbols"]:
        raise RuntimeError("dispositions do not cover exactly the frozen seed symbols")
    allowed = {ELIGIBLE, *EXCLUDED_DISPOSITIONS}
    for sym, rec in dispositions.items():
        if rec.get("disposition") not in allowed:
            raise RuntimeError(f"{sym}: disposition {rec.get('disposition')!r} is not ELIGIBLE or a typed EXCLUDED_*")


def build_universe(seed: dict, dispositions: dict) -> dict:
    validate_dispositions(seed, dispositions)
    eligible = sorted(s for s, r in dispositions.items() if r["disposition"] == ELIGIBLE)
    excluded = {s: dispositions[s] for s in sorted(dispositions) if dispositions[s]["disposition"] != ELIGIBLE}
    return {"schema_version": "alpha_census_universe_v2", "seed_symbol_count": len(seed["symbols"]),
            "seed_universe_sha256": sha256_canonical(seed), "survivorship_classification": seed["survivorship_classification"],
            "point_in_time_membership": False,
            "eligibility_rule": eligibility_rule(),
            "dispositions": {s: dispositions[s] for s in sorted(dispositions)},
            "symbols": eligible, "symbol_count": len(eligible), "excluded": excluded}


def eligibility_rule() -> dict:
    return {"min_valid_completed_1day_observations_strictly_before_2024_01_01": MIN_ELIGIBLE_OBSERVATIONS,
            "requires": "accepted registered SIP/adjustment=all provenance and supported asset/data semantics",
            "data_present_alone_is_not_a_disposition": True, "exclusion_for_performance": False,
            "substitution": False}


def build_partitions() -> dict:
    return dict(PARTITIONS)


def build_grammar() -> dict:
    configs = build_configs()
    per_family = {}
    for c in configs:
        d = per_family.setdefault(c["family"], {"name": FAMILIES[c["family"]][0], "scope": c["scope"], "configs": 0})
        d["configs"] += 1
    doc = {"schema_version": "alpha_census_grammar_v2", "experiment_id": EXPERIMENT_ID, "family_templates": per_family,
           "configs": configs, "config_count": len(configs), "conditional_horizons": list(CONDITIONAL_HORIZONS),
           "conditional_factor_count": len(configs) * len(CONDITIONAL_HORIZONS),
           "signal_semantics": SIGNAL_SEMANTICS,
           "rejected_grammar": {"label": "REJECTED_UNAUTHORIZED_SEARCH_GRAMMAR", "families": "S01..S20",
                                "config_count": 5086, "status": "REJECTED_NOT_AUTHORITATIVE"}}
    doc["grammar_id"] = sha256_canonical({k: doc[k] for k in ("family_templates", "configs", "conditional_horizons")})[:32]
    return doc


def build_protocol() -> dict:
    """Frozen economic/statistical contract. The search grid and universe are excluded so protocol_id is independent."""
    from data import REQUEST_CONTRACT
    return {
        "schema_version": "alpha_census_protocol_v2",
        "data_contract": {**REQUEST_CONTRACT, "no_iex_fallback": True, "symbol_failure_policy": "typed_EXCLUDED_disposition"},
        "calendar_contract": {"contract_id": CALENDAR_ID, "content_sha256": CALENDAR_SHA256},
        "eligibility_rule": eligibility_rule(),
        "indicator_conventions": {"rsi": "mqk_research.indicators.core.rsi (Cutler simple-mean)",
                                  "zscore": "mqk_research.indicators.core.zscore (sample std, ddof=1)",
                                  "atr": "simple rolling mean of true range"},
        "execution_contract": {
            "positions": "long_flat_daily_completed_bars",
            "signal_knowledge": "signal on bar t uses only bars <= t",
            "fill": "first bar strictly after t at rust_conservative_bar_range_v1 (BUY high+slip, SELL low-slip), "
                    "integer-micros, same-bar fill forbidden",
            "pnl_marking": "qty_held_before_bar*(close_t-close_{t-1}); fill cost = qty*adverse(fill vs close)+commission",
            "cost_model": {"commission_bps_per_side": 10.0, "slippage_bps_per_side": 0.0},
            "execution_pricing": {"pricing_model_id": "rust_conservative_bar_range_v1", "slippage_bps": 5,
                                  "volatility_mult_bps": 0, "source": "PREDECLARED_BATCH_03 economic_protocol"},
            "open_position_at_end": "marked_to_last_close_no_liquidation",
            "hold_semantics": "desired while any qualifying event occurred in trailing N completed strategy outputs; no pyramiding",
            "month_end_anchor": "last canonical session of each calendar month",
        },
        "sizing": {"model": "FixedInitialCapitalFractionV1", "initial_capital_usd": 100000, "allocation_bps": 1000,
                   "entry_budget_usd": 10000, "quantity": "floor(budget / completed-signal-bar close)",
                   "plus_one_fallback": False, "compounding": False, "whole_shares": True, "production_default": False},
        "benchmark": {"kind": "capital_matched_passive_buy_hold_same_window_same_cost_model",
                      "window_start": "first bar where the cell signal is defined"},
        "alpha_definition": "net_total_pnl - benchmark_net_total_pnl over the cell evaluation window, finite",
        "regime_definition": "SPY close > SPY SMA200 at the prior completed bar (RISK_ON else RISK_OFF)",
        "strategy_edge_qualification": {
            "common": "frozen registered trial, succeeded authoritative attempt, finite metrics, discovery partition only, "
                      "frozen costs present, valid benchmark present",
            "min_evaluated_bars": MIN_EVALUATED_BARS, "min_closed_round_trips": MIN_CLOSED_ROUND_TRIPS,
            "DISCOVERED_WEAK": "common + >=252 completed evaluated bars + >=5 CLOSED round trips + net strategy P&L > 0",
            "DISCOVERED_MODERATE": "WEAK + finite cost-aware matched-benchmark alpha > 0",
            "DISCOVERED_STRONG": "MODERATE + DSR >= 0.5 + complete authoritative full-population multiple-testing + PBO <= 0.5",
            "judge_status": JUDGE_STATUS, "strong_while_judge_deferred": 0,
            "population_narrowing": "forbidden"},
        "conditional_edge": {
            "authority": "mqk_research.factors registered FactorSpec + durable evaluation attempt opened before computation",
            "horizons": list(CONDITIONAL_HORIZONS),
            "factor_identity": "family/condition type, params, horizon, direction, lookback, normalization, universe identity, "
                               "data provenance, timing convention, information lag; results never alter identity",
            "label": "close_{t+h}/close_t - 1, diagnostic only, never P&L; labels reaching past the discovery fence are dropped",
            "baseline": "same-symbol same-horizon unconditional mean",
            "effect": "conditional mean minus baseline, direction applied prospectively (higher_is_better, fixed ex-ante)",
            "min_events": MIN_CONDITIONAL_EVENTS,
            "DISCOVERED_WEAK": ">=30 valid events + finite direction-adjusted conditional effect > 0 + succeeded authoritative evaluation",
            "DISCOVERED_MODERATE": f"WEAK + deterministic empirical-null two-sided p <= {CONDITIONAL_P_ALPHA}",
            "DISCOVERED_STRONG": f"MODERATE + COMPLETE full registered factor-family BH/FDR + q <= {DISCOVERY_FDR_ALPHA}",
            "null_protocol": {"n_permutations": N_PERMUTATIONS, "base_seed": BASE_SEED,
                              "source": "mqk_research.factors.fdr.compute_empirical_pvalue (accepted repo defaults)"},
            "fdr": {"protocol": "factor_fdr_bh_v1", "alpha": DISCOVERY_FDR_ALPHA,
                    "population": "all registered factors of the corrected family; failed/non-evaluable stay accounted"}},
        "ordering": "families S01..S14 in grid order; symbols sorted; cell identity independent of order",
        "retry_rules": {"infrastructure_retry": "same trial, new attempt, same economics", "outcome_based_retry": "forbidden"},
        "edge_recording_rule": {"strategy_edge": "DISCOVERED_WEAK|MODERATE|STRONG (highest class only)",
                                "conditional_edge": "DISCOVERED_WEAK|MODERATE|STRONG (highest class only)",
                                "VALIDATION_STATUS": "NOT_VALIDATED", "PROMOTION_AUTHORITY": "NONE",
                                "non_qualifying": "stay in the complete search/factor ledger, not registry records"},
        "replication": {"basis": "per config: count of evaluable symbols whose cell is a registry record (>= WEAK)",
                        "SYMBOL_SPECIFIC": "exactly 1 qualifying symbol", "BROADLY_REPLICATED": ">= 50% of evaluable symbols qualifying",
                        "CLUSTER_REPLICATED": "2+ qualifying symbols but < 50%"},
        "parameter_neighborhood": {"axes": "numeric non-bool parameters only", "neighbor": "same family and same symbol, equal on every "
                                   "other parameter, adjacent in that parameter's sorted unique grid values",
                                   "parameter_island": "qualifying cell with >=1 neighbor and qualifying-neighbor share < threshold; never deleted"},
        "edge_identity": {"strategy_edge": "sha256(kind,trial_id)", "conditional_edge": "sha256(kind,factor_id)",
                          "result_independent": True},
        "chunking": {"cells_per_chunk": 500, "identity_independent": True},
        "flag_thresholds": {"tiny_sample_trades": 30, "high_turnover_round_trips_per_year": 50,
                            "large_drawdown_frac_of_budget": 0.25, "single_year_concentration_share": 0.5,
                            "regime_concentration_share": 0.8, "data_short_history_rows": 1500,
                            "data_quality_caveat_zero_volume_bars": 20, "parameter_island_neighbor_positive_share": 0.25,
                            "cost_fragile": "alpha - (strategy_costs - benchmark_costs) <= 0, i.e. doubling costs erases alpha"},
        "metric_definitions": {
            "sharpe": "mean/std(ddof=1) of daily net pnl / 100000 over window bars * sqrt(252); null when std == 0",
            "max_drawdown": "peak-to-trough of cumulative net pnl including the zero start; flagged as a fraction of the 10000 budget",
            "turnover": "traded notional (entries+exits) / 10000",
            "exposure": "fraction of window bars with a position held before the bar",
            "trade_count": "entries with qty > 0 (open run counted)", "round_trips": "closed runs",
            "win_loss": "closed runs only; run pnl = net bars from entry bar through exit bar inclusive",
            "concentration": "max positive bucket / sum of positive buckets of per-bar alpha contribution by year / regime",
            "min_cross_section": 20,
            "dsr": "DEFERRED_FULL_POPULATION: effective-trial correction over the full population is infeasible; per-cell "
                   "sharpe/skew/kurtosis/observations are recorded so research_multiple_testing_judge_v1 can run later"},
    }


def compute_ids(universe: dict, partitions: dict, protocol: dict) -> dict:
    return {"universe_id": sha256_canonical(universe)[:32], "partitions_id": sha256_canonical(partitions)[:32],
            "protocol_id": sha256_canonical(protocol)[:32]}


def build_search_space(grammar: dict, universe: dict, partitions: dict, protocol: dict) -> dict:
    ids = compute_ids(universe, partitions, protocol)
    configs = grammar["configs"]
    assert_grammar_authority(configs)
    symbols = universe["symbols"]
    if not symbols:
        raise GrammarRefusal("empty eligible universe")
    doc = {"schema_version": "alpha_census_search_space_v2", "experiment_id": EXPERIMENT_ID, **ids,
           "grammar_id": grammar["grammar_id"], "config_count": len(configs), "eligible_symbol_count": len(symbols),
           "seed_symbol_count": universe["seed_symbol_count"],
           "strategy_trial_count": len(configs) * len(symbols),
           "conditional_horizons": list(CONDITIONAL_HORIZONS),
           "conditional_factor_count": len(configs) * len(CONDITIONAL_HORIZONS),
           "population_root_sha256": population_root(configs, symbols, ids)}
    doc["search_space_id"] = sha256_canonical({k: doc[k] for k in
                                               ("grammar_id", "universe_id", "partitions_id", "protocol_id")})[:32]
    return doc


def iter_cells(configs: list[dict], symbols: list[str], ids: dict):
    """Deterministic manifest order: config order, then sorted symbol. Yields (config, symbol, trial_id)."""
    for c in configs:
        for s in sorted(symbols):
            yield c, s, trial_id(trial_identity(c, s, ids))


def population_root(configs: list[dict], symbols: list[str], ids: dict) -> str:
    h = hashlib.sha256()
    for _c, _s, tid in iter_cells(configs, symbols, ids):
        h.update(tid.encode("ascii"))
        h.update(b"\n")
    return h.hexdigest()


def _write(path: Path, doc: dict) -> None:
    path.write_text(json.dumps(doc, sort_keys=True, indent=1) + "\n", encoding="utf-8", newline="\n")


def write_authority_manifests() -> dict:
    """Universe-independent manifests, frozen before any economics: seed universe, grammar, partitions, protocol."""
    docs = {SEED_UNIVERSE_FILE: build_seed_universe(), GRAMMAR_FILE: build_grammar(),
            PARTITIONS_FILE: build_partitions(), PROTOCOL_FILE: build_protocol()}
    for path, doc in docs.items():
        _write(path, doc)
    return {p.name: d for p, d in docs.items()}


def write_population_manifests(dispositions: dict) -> dict:
    """Eligible universe + search space, written once every seed symbol has its disposition."""
    seed = json.loads(SEED_UNIVERSE_FILE.read_text(encoding="utf-8"))
    grammar = json.loads(GRAMMAR_FILE.read_text(encoding="utf-8"))
    partitions = json.loads(PARTITIONS_FILE.read_text(encoding="utf-8"))
    protocol = json.loads(PROTOCOL_FILE.read_text(encoding="utf-8"))
    universe = build_universe(seed, dispositions)
    space = build_search_space(grammar, universe, partitions, protocol)
    _write(UNIVERSE_FILE, universe)
    _write(SEARCH_SPACE_FILE, space)
    return space


if __name__ == "__main__":
    docs = write_authority_manifests()
    g = docs[GRAMMAR_FILE.name]
    print({"grammar_id": g["grammar_id"], "config_count": g["config_count"]})
    print({f: d["configs"] for f, d in g["family_templates"].items()})
