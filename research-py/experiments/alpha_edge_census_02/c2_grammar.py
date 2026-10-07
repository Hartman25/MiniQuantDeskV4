"""Census-02 proposed search grammar: short-only mirrors SH01-SH13 (tier H) and symmetric long/short state pairs LS01-LS04
(tier L). Counts, identities and dependency tags only; nothing here reads prices or results."""

from __future__ import annotations

import hashlib
import sys
from pathlib import Path

HERE = Path(__file__).resolve().parent
sys.path.insert(0, str(HERE))
import c2_protocol as pr  # noqa: E402  (also puts Census-01 + src on sys.path)
import search_space as ss1  # noqa: E402  (Census-01 grammar authority, read-only reuse)

CADENCES = ss1.CADENCES
TRENDS = ("none", "below_sma200")
EXPECTED_H_COUNTS = {"SH01": 8, "SH02": 6, "SH03": 14, "SH04": 12, "SH05": 72, "SH06": 24, "SH07": 108, "SH08": 36,
                     "SH09": 64, "SH10": 12, "SH11": 18, "SH12": 32, "SH13": 24}
EXPECTED_L_COUNTS = {"LS01": 8, "LS02": 6, "LS03": 14, "LS04": 12}
EXPECTED_H_TOTAL, EXPECTED_L_TOTAL = 430, 40
# Short mirror -> the Census-01 family whose long state it mirrors.
MIRROR_OF = {"SH01": "S01", "SH02": "S02", "SH03": "S03", "SH04": "S04", "SH05": "S05", "SH06": "S06", "SH07": "S07",
             "SH08": "S08", "SH09": "S09", "SH10": "S10", "SH11": "S11", "SH12": "S12", "SH13": "S13"}
LS_PAIR = {"LS01": ("S01", "SH01"), "LS02": ("S02", "SH02"), "LS03": ("S03", "SH03"), "LS04": ("S10", "SH10")}
# Exact complements of a Census-01 long state (short state == NOT long state, up to undefined/ties): short-only P&L is the
# Census-01 long P&L minus buy-and-hold, and the LS state pair is their sum, i.e. information already observed.
# Tagged, never silently dropped.
COMPLEMENT_FAMILIES = ("SH01", "SH02", "SH03", "LS01", "LS02", "LS03")
HORIZONS = ss1.CONDITIONAL_HORIZONS


def _g(**kw):
    return kw


def _grid_SH01():
    return [_g(lookback=L, cadence=c) for L in (21, 63, 126, 252) for c in CADENCES]


def _grid_SH02():
    return [_g(sma=m) for m in (20, 50, 100, 150, 200, 250)]


def _grid_SH03():
    return [_g(fast=f, slow=s) for f in (10, 20, 50) for s in (50, 100, 150, 200, 250) if f < s]


def _grid_SH04():
    return [_g(entry=e, exit=x) for e in (20, 50, 100, 150, 200) for x in (10, 20, 50) if x < e]


def _grid_SH05():
    return [_g(period=p, entry_above=e, exit_below=x, trend=t) for p in (2, 3, 5, 7, 10, 14)
            for e in (90, 80, 70) for x in (50, 30) for t in TRENDS]


def _grid_SH06():
    return [_g(lookback=L, entry_z=z, exit_z=0.0, trend=t) for L in (10, 20, 40, 60) for z in (1.5, 2.0, 2.5) for t in TRENDS]


def _grid_SH07():
    return [_g(rise_sessions=d, atr_window=a, mult=m, hold=h, trend=t) for d in (1, 3, 5) for a in (14, 20)
            for m in (1.0, 1.5, 2.0) for h in (1, 3, 5) for t in TRENDS]


def _grid_SH08():
    return [_g(atr_window=a, mult=m, hold=h, trend=t) for a in (14, 20) for m in (1.0, 1.5, 2.0) for h in (1, 3, 5)
            for t in TRENDS]


def _grid_SH09():
    return [_g(short=s, long=l, ratio=r, breakdown=b, exit=x, trend=t) for s in (5, 10) for l in (40, 60)
            for r in (0.25, 0.40) for b in (20, 50) for x in (10, 20) for t in TRENDS]


def _grid_SH10():
    return [_g(low_lookback=L, distance=d, cadence=c) for L in (126, 252) for d in (0.03, 0.05, 0.10) for c in CADENCES]


def _grid_SH11():
    return [_g(up_sessions=n, hold=h, trend=t) for n in (2, 3, 4) for h in (1, 3, 5) for t in TRENDS]


def _grid_SH12():
    return [_g(volume_lookback=L, volume_z=z, price_impulse=i, mode=m, hold=h) for L in (20, 60) for z in (1.5, 2.0)
            for i in (1, 5) for m in ("continuation", "reversal") for h in (1, 3)]


def _grid_SH13():
    return [_g(short_vol=s, long_vol=60, expansion_ratio=r, mode=m, hold=h) for s in (10, 20) for r in (1.5, 2.0)
            for m in ("breakdown", "reversal") for h in (1, 3, 5)]


H_FAMILIES = {
    "SH01": ("SHORT_TIME_SERIES_MOMENTUM", _grid_SH01), "SH02": ("SHORT_PRICE_BELOW_SMA", _grid_SH02),
    "SH03": ("SHORT_BEARISH_SMA_STATE", _grid_SH03), "SH04": ("SHORT_LOWER_CHANNEL_BREAKDOWN", _grid_SH04),
    "SH05": ("SHORT_RSI_OVERBOUGHT_REVERSION", _grid_SH05), "SH06": ("SHORT_POSITIVE_ZSCORE_REVERSION", _grid_SH06),
    "SH07": ("SHORT_ATR_RALLY_REVERSAL", _grid_SH07), "SH08": ("SHORT_UP_GAP_FADE", _grid_SH08),
    "SH09": ("SHORT_VOLATILITY_CONTRACTION_BREAKDOWN", _grid_SH09), "SH10": ("SHORT_LOW_PROXIMITY_MOMENTUM", _grid_SH10),
    "SH11": ("SHORT_CONSECUTIVE_UP_REVERSAL", _grid_SH11), "SH12": ("SHORT_VOLUME_PRICE_SURPRISE", _grid_SH12),
    "SH13": ("SHORT_VOLATILITY_EXPANSION", _grid_SH13),
}
# LS pair grids equal the grid of the long family they pair (the short leg shares the same coordinates).
L_FAMILIES = {"LS01": ("LONG_SHORT_TIME_SERIES_MOMENTUM", _grid_SH01), "LS02": ("LONG_SHORT_SMA_STATE", _grid_SH02),
              "LS03": ("LONG_SHORT_SMA_CROSS_STATE", _grid_SH03),
              "LS04": ("LONG_SHORT_52W_PROXIMITY", lambda: [_g(lookback=L, distance=d, cadence=c) for L in (126, 252)
                                                              for d in (0.03, 0.05, 0.10) for c in CADENCES])}
FAMILY_NAMES = {**{f: n for f, (n, _g) in H_FAMILIES.items()}, **{f: n for f, (n, _g) in L_FAMILIES.items()}}
SIDE = {**{f: "short" for f in H_FAMILIES}, **{f: "long_short" for f in L_FAMILIES}}

CONDITION_PARAM_KEYS = {
    "SH01": ("lookback", "cadence"), "SH02": ("sma",), "SH03": ("fast", "slow"), "SH04": ("entry",),
    "SH05": ("period", "entry_above", "trend"), "SH06": ("lookback", "entry_z", "trend"),
    "SH07": ("rise_sessions", "atr_window", "mult", "trend"), "SH08": ("atr_window", "mult", "trend"),
    "SH09": ("short", "long", "ratio", "breakdown", "trend"), "SH10": ("low_lookback", "distance", "cadence"),
    "SH11": ("up_sessions", "trend"), "SH12": ("volume_lookback", "volume_z", "price_impulse", "mode"),
    "SH13": ("short_vol", "long_vol", "expansion_ratio", "mode")}
EXECUTION_ONLY_KEYS = {"SH04": ("exit",), "SH05": ("exit_below",), "SH06": ("exit_z",), "SH07": ("hold",), "SH08": ("hold",),
                       "SH09": ("exit",), "SH11": ("hold",), "SH12": ("hold",), "SH13": ("hold",)}
# Neutral in-grid values used only to instantiate the Strategy builders when evaluating a condition; the condition series is
# proven independent of them (execution-only parameters change d, never cond or the first-defined bar).
EXECUTION_ONLY_FILL = {"SH04": {"exit": 10}, "SH05": {"exit_below": 50}, "SH06": {"exit_z": 0.0}, "SH07": {"hold": 1},
                       "SH08": {"hold": 1}, "SH09": {"exit": 10}, "SH11": {"hold": 1}, "SH12": {"hold": 1}, "SH13": {"hold": 1}}
EXPECTED_CONDITION_COUNTS = {"SH01": 8, "SH02": 6, "SH03": 14, "SH04": 5, "SH05": 36, "SH06": 24, "SH07": 36, "SH08": 12,
                             "SH09": 32, "SH10": 12, "SH11": 6, "SH12": 16, "SH13": 8}
EXPECTED_CONDITION_TOTAL = 215
# EXCLUDE_COMPLEMENTS_BEFORE_FREEZE removes the SH01-03 conditions (8+6+14); LS01-03 mint no conditional factor of their own.
EXPECTED_CONDITION_COUNTS_EXCLUDED = {f: n for f, n in EXPECTED_CONDITION_COUNTS.items() if f not in COMPLEMENT_FAMILIES}
EXPECTED_CONDITION_TOTAL_EXCLUDED = 187


class GrammarRefusal(RuntimeError):
    pass


def config_id(family: str, params: dict) -> str:
    return hashlib.sha256(pr.canonical({"family": family, "side": SIDE[family], "params": params}).encode()).hexdigest()[:24]


def build_configs(tiers: str = "H+L", complements_excluded: bool = False) -> list[dict]:
    """Deterministic family/grid order. Duplicates, grammar drift and any id collision (also with Census-01 config ids)
    are refused. complements_excluded drops every COMPLEMENT_FAMILIES config (SH01-03 and LS01-03)."""
    if tiers not in ("H", "H+L"):
        raise GrammarRefusal(f"tier selection {tiers!r} is not implemented (H or H+L)")
    fams = {**H_FAMILIES, **L_FAMILIES}
    want_l = tiers == "H+L"
    out, seen = [], set()
    c1_ids = {c["config_id"] for c in ss1.build_configs()}
    for fam, (_name, gen) in fams.items():
        if (fam in L_FAMILIES and not want_l) or (complements_excluded and fam in COMPLEMENT_FAMILIES):
            continue
        for params in gen():
            cid = config_id(fam, params)
            if cid in seen or cid in c1_ids:
                raise GrammarRefusal(f"duplicate or Census-01-colliding coordinate in {fam}: {pr.canonical(params)}")
            seen.add(cid)
            out.append({"config_id": cid, "family": fam, "side": SIDE[fam], "params": params})
    assert_grammar_authority(out, tiers, complements_excluded)
    return out


def assert_grammar_authority(configs: list[dict], tiers: str = "H+L", complements_excluded: bool = False) -> None:
    counts = {f: sum(1 for c in configs if c["family"] == f) for f in {c["family"] for c in configs}}
    want = dict(EXPECTED_H_COUNTS)
    if tiers == "H+L":
        want.update(EXPECTED_L_COUNTS)
    if complements_excluded:
        want = {f: n for f, n in want.items() if f not in COMPLEMENT_FAMILIES}
    if counts != want:
        raise GrammarRefusal(f"per-family counts {counts} differ from the proposed grammar {want}")
    if any(c["side"] != SIDE[c["family"]] for c in configs):
        raise GrammarRefusal("side does not match the family")


def condition_params(family: str, params: dict) -> dict:
    keys, exec_keys = CONDITION_PARAM_KEYS[family], EXECUTION_ONLY_KEYS.get(family, ())
    unknown = sorted(set(params) - set(keys) - set(exec_keys))
    if unknown or sorted(set(params) & set(exec_keys)) != sorted(exec_keys):
        raise GrammarRefusal(f"{family}: parameters {unknown} unclassified or execution-only keys missing")
    return {k: params[k] for k in keys}


def condition_id(family: str, cparams: dict) -> str:
    return hashlib.sha256(pr.canonical({"kind": "conditional_condition", "side": "short", "family": family,
                                        "params": cparams}).encode()).hexdigest()[:24]


def build_conditions(configs: list[dict], complements_excluded: bool = False) -> list[dict]:
    """Distinct short-hypothesis conditions (tier H only; LS legs are covered by their tier-H and Census-01 conditions)."""
    by_id: dict = {}
    for c in configs:
        if c["side"] != "short":
            continue
        cp = condition_params(c["family"], c["params"])
        cid = condition_id(c["family"], cp)
        by_id.setdefault(cid, {"condition_id": cid, "family": c["family"], "params": cp})
    out = list(by_id.values())
    want = EXPECTED_CONDITION_COUNTS_EXCLUDED if complements_excluded else EXPECTED_CONDITION_COUNTS
    counts = {f: sum(1 for c in out if c["family"] == f) for f in want}
    if counts != want or len(out) != sum(want.values()):
        raise GrammarRefusal(f"condition counts {counts} differ from the proposal {want}")
    return out


def trial_identity(config: dict, symbol: str, ids: dict) -> dict:
    """Result-independent identity of one cell. `side` separates mirrors from Census-01 coordinates."""
    return {"schema_version": pr.SCHEMA, "kind": "strategy_edge", "side": config["side"], "family": config["family"],
            "params": config["params"], "scope": symbol, "universe_id": ids["universe_id"],
            "partitions_id": ids["partitions_id"], "protocol_id": ids["protocol_id"]}


def trial_id(identity: dict) -> str:
    return pr.TRIAL_PREFIX + pr.sha256_canonical(identity)[:32]


def tags(config: dict) -> dict:
    """Non-identity dependency tags (never enter trial_id)."""
    fam = config["family"]
    t = {"mirror_of": MIRROR_OF.get(fam), "complement_of_census01": fam in COMPLEMENT_FAMILIES}
    if fam in LS_PAIR:
        t["legs"] = list(LS_PAIR[fam])
    return t


def selected_population(tiers: str = "H+L", complements_excluded: bool = False) -> tuple[list[dict], list[dict]]:
    """The (configs, conditions) population one operator choice of tiers x complement handling would register."""
    configs = build_configs(tiers, complements_excluded)
    return configs, build_conditions(configs, complements_excluded)


def candidate_arithmetic(n_strategy_symbols: int, tiers: str = "H+L", *, complements_excluded: bool = False) -> dict:
    """Exact candidate counts for a given executable-scope size. Strategy trials = configs x scope symbols; conditional
    factors = conditions x horizons, pooled and independent of the symbol count."""
    configs, conditions = selected_population(tiers, complements_excluded)
    comp_registered = sum(1 for c in configs if c["family"] in COMPLEMENT_FAMILIES)
    return {"tiers": tiers, "complements_excluded": complements_excluded, "configs": len(configs),
            "complement_tagged_configs": comp_registered, "executable_scope_symbols": n_strategy_symbols,
            "strategy_trials": len(configs) * n_strategy_symbols, "short_conditions": len(conditions),
            "conditional_factors": len(conditions) * len(HORIZONS),
            "census01_strategy_trials": 38_192, "census01_conditional_factors": 1_095}
