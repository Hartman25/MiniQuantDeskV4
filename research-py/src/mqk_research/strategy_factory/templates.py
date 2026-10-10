"""Closed vocabulary of recognized rule templates, the native-engine semantic cards, and conservative extraction.

A template is a *recognized rule shape* with typed, bounded parameters. Recognition never invents a parameter: a
number is recorded only when the source text states it; everything else is reported as missing. `grammar_v1` marks the
templates the verified Rust grammar engine can execute; every other template is recognized for deduplication and for
the controlled-implementation workflow but is not executable by the Factory.
"""

from __future__ import annotations

import re
from dataclasses import dataclass
from typing import Any, Mapping

from mqk_research.strategy_mining.grammar import MechanismFamily

GRAMMAR_PREFIX = "grammar_v1__"


@dataclass(frozen=True)
class ParamSpec:
    name: str
    lo: int
    hi: int


@dataclass(frozen=True)
class Template:
    template_id: str
    family: MechanismFamily
    params: tuple[ParamSpec, ...]
    grammar_v1: bool = False       # executable by the verified grammar engine
    stateful: bool = False         # path-dependent (entry/exit memory): needs durable-restart semantics

    def spec(self, name: str) -> ParamSpec:
        return next(p for p in self.params if p.name == name)

    def validate(self, params: Mapping[str, int]) -> None:
        if set(params) != {p.name for p in self.params}:
            raise ValueError(f"{self.template_id}: parameters must be exactly {[p.name for p in self.params]}")
        for p in self.params:
            v = params[p.name]
            if isinstance(v, bool) or not isinstance(v, int) or not p.lo <= v <= p.hi:
                raise ValueError(f"{self.template_id}.{p.name}={v!r} outside [{p.lo}, {p.hi}]")
        if self.template_id == "dual_sma_cross" and not params["fast"] < params["slow"]:
            raise ValueError("dual_sma_cross requires fast < slow")


P = ParamSpec
F = MechanismFamily
TEMPLATES: dict[str, Template] = {t.template_id: t for t in (
    Template("sma_trend_gate", F.TREND_FOLLOWING, (P("window", 2, 1000),), grammar_v1=True),
    Template("dual_sma_cross", F.TREND_FOLLOWING, (P("fast", 2, 500), P("slow", 3, 1000)), grammar_v1=True),
    Template("abs_momentum_sessions", F.MOMENTUM, (P("lookback", 2, 1000),), grammar_v1=True),
    Template("near_high_proximity", F.MOMENTUM, (P("window", 20, 1000), P("proximity_bps", 1, 5000),
                                                 P("trend_window", 0, 1000)), grammar_v1=True),
    Template("month_window", F.SEASONAL, (P("start_month", 1, 12), P("end_month", 1, 12)), grammar_v1=True),
    Template("close_channel", F.BREAKOUT, (P("entry_window", 2, 500), P("exit_window", 2, 500)), stateful=True),
    Template("fixed_hold_breakout", F.BREAKOUT, (P("window", 2, 500), P("hold", 1, 60)), stateful=True),
    Template("rsi_reversion", F.PULLBACK_MEAN_REVERSION, (P("rsi_window", 2, 50), P("entry_below", 1, 49),
                                                          P("exit_above", 50, 99), P("trend_window", 0, 1000)), stateful=True),
    Template("zscore_reversion", F.PULLBACK_MEAN_REVERSION, (P("window", 5, 250), P("entry_z_x10", 5, 40),
                                                             P("trend_window", 0, 1000)), stateful=True),
    Template("trend_pullback_hold", F.PULLBACK_MEAN_REVERSION, (P("pullback_window", 1, 60), P("pullback_bps", 1, 5000),
                                                                P("hold", 1, 60), P("trend_window", 0, 1000)), stateful=True),
    Template("monthly_abs_momentum", F.MOMENTUM, (P("lookback_months", 1, 36), P("skip_months", 0, 12))),
    Template("monthly_trend_timing", F.TREND_FOLLOWING, (P("months", 2, 36),)),
    Template("monthly_consensus_momentum", F.MOMENTUM, (P("min_votes", 1, 5),)),
    Template("monthly_near_high", F.MOMENTUM, (P("window", 20, 1000), P("proximity_bps", 1, 5000))),
    Template("turn_of_month", F.SEASONAL, (P("last_k", 0, 5), P("first_m", 0, 10))),
    Template("pre_holiday", F.SEASONAL, (P("sessions", 1, 5),)),
    Template("gap_reversal", F.GAP, (P("atr_mult_x10", 1, 100), P("atr_window", 5, 100), P("hold", 1, 20)), stateful=True),
    Template("atr_drop_reversal", F.REVERSAL, (P("drop_window", 1, 20), P("atr_mult_x10", 1, 100), P("hold", 1, 20),
                                               P("trend_window", 0, 1000)), stateful=True),
    Template("volatility_contraction_breakout", F.VOLATILITY, (P("short_range", 2, 60), P("long_range", 10, 250)),
             stateful=True),
    Template("legacy_engine", F.TREND_FOLLOWING, ()),
)}


@dataclass(frozen=True)
class NativeCard:
    strategy_id: str
    template_id: str
    params: Mapping[str, int]
    direction: str = "long_flat"
    timeframe_secs: int = 86_400
    note: str = ""

    def signature(self) -> tuple:
        return (self.template_id, self.direction, tuple(sorted(self.params.items())))


def _c(sid, tid, direction="long_flat", **params):
    return NativeCard(sid, tid, params, direction)


# One card per registered native identity (REGISTERED_STRATEGY_IDS); constants read from each engine's own meta text.
NATIVE_CARDS: tuple[NativeCard, ...] = (
    _c("swing_momentum", "legacy_engine"), _c("mean_reversion", "legacy_engine"),
    _c("volatility_breakout", "legacy_engine"), _c("intraday_scalper", "legacy_engine"),
    _c("trend_sma50", "sma_trend_gate", window=50),
    _c("dual_sma_50_200_trend", "dual_sma_cross", fast=50, slow=200),
    _c("pullback_mean_reversion_20_2", "zscore_reversion", window=20, entry_z_x10=20, trend_window=0),
    _c("absolute_momentum_252", "abs_momentum_sessions", lookback=252),
    _c("near_high_momentum_252_3pct", "near_high_proximity", window=252, proximity_bps=300, trend_window=50),
    _c("trend_pullback_5d_4pct_hold5", "trend_pullback_hold", pullback_window=5, pullback_bps=400, hold=5, trend_window=200),
    _c("turn_of_month_last1_first3", "turn_of_month", last_k=1, first_m=3),
    _c("halloween_nov_apr", "month_window", start_month=11, end_month=4),
    _c("trading_range_breakout_50d_hold10", "fixed_hold_breakout", window=50, hold=10),
    _c("monthly_multihorizon_abs_momentum_consensus_v1", "monthly_consensus_momentum", min_votes=2),
    _c("trend_filtered_rsi5_reversion_v1", "rsi_reversion", rsi_window=5, entry_below=30, exit_above=70, trend_window=200),
    _c("trend_filtered_extreme_3d_atr_reversal_v1", "atr_drop_reversal", drop_window=3, atr_mult_x10=15, hold=5, trend_window=200),
    _c("close_channel_100_50_trend_v1", "close_channel", entry_window=100, exit_window=50),
    _c("monthly_10month_trend_timing_v1", "monthly_trend_timing", months=10),
    _c("trend_filtered_zscore20_reversion_v1", "zscore_reversion", window=20, entry_z_x10=20, trend_window=200),
    _c("volatility_contraction_breakout_v1", "volatility_contraction_breakout", short_range=10, long_range=60),
    _c("monthly_12_minus_1_abs_momentum_v1", "monthly_abs_momentum", lookback_months=12, skip_months=1),
    _c("delayed_overnight_gap_reversal_v1", "gap_reversal", atr_mult_x10=15, atr_window=20, hold=3),
    _c("monthly_52week_high_proximity_v1", "monthly_near_high", window=252, proximity_bps=500),
    _c("pre_holiday_two_session_long_v1", "pre_holiday", sessions=2),
    _c("intraday_short_scalper", "legacy_engine", direction="short_flat"),
)
CARD_BY_ID = {c.strategy_id: c for c in NATIVE_CARDS}


# ---- grammar names: the full spec is the name, so nothing outside the name is needed to reproduce a strategy ----

def grammar_strategy_name(template_id: str, params: Mapping[str, int]) -> str:
    t = TEMPLATES[template_id]
    if not t.grammar_v1:
        raise ValueError(f"{template_id} is not executable by grammar_v1")
    t.validate(params)
    return GRAMMAR_PREFIX + template_id + "".join(f"__{p.name}_{params[p.name]}" for p in t.params)


def parse_grammar_name(name: str) -> tuple[str, dict[str, int]]:
    if not name.startswith(GRAMMAR_PREFIX):
        raise ValueError("not a grammar_v1 strategy name")
    parts = name[len(GRAMMAR_PREFIX):].split("__")
    t = TEMPLATES.get(parts[0])
    if t is None or not t.grammar_v1:
        raise ValueError(f"unknown or non-executable grammar template {parts[0]!r}")
    params: dict[str, int] = {}
    for tok in parts[1:]:
        key, _, val = tok.rpartition("_")
        if not key or not re.fullmatch(r"\d+", val) or key in params:
            raise ValueError(f"malformed grammar parameter {tok!r}")
        params[key] = int(val)
    t.validate(params)
    if grammar_strategy_name(t.template_id, params) != name:   # canonical spelling only (no padded or reordered forms)
        raise ValueError("non-canonical grammar strategy name")
    return t.template_id, params


# ---- conservative extraction ------------------------------------------------------------------------------------

_NUM = r"(\d{1,4})"
_MONTHS = {"jan": 1, "feb": 2, "mar": 3, "apr": 4, "may": 5, "jun": 6, "jul": 7, "aug": 8, "sep": 9, "oct": 10,
           "nov": 11, "dec": 12}


@dataclass(frozen=True)
class Match:
    template_id: str
    explicit: Mapping[str, int]                 # parameter -> value stated in the source
    basis: Mapping[str, str]                    # parameter -> the exact source span
    missing: tuple[str, ...]                    # declared parameters the source did not state
    aliases: Mapping[str, str] = None           # parameter -> alias that implied it (INFERRED, never EXPLICIT)


def _ma_words(text: str):
    return re.finditer(rf"{_NUM}[- ]?(?:day|d|session|bar)s?[ -]*(?:simple )?(?:sma|moving average|ma)\b", text, re.I)


def extract(text: str) -> list[Match]:
    """All template recognitions in `text` (title + rule + hypothesis, one string). Several matches mean a composite."""
    t = " ".join(text.split())
    out: list[Match] = []

    cross = re.search(rf"{_NUM}\s*/\s*{_NUM}[- ]?(?:day|d)?\s*(?:sma|ma|moving average)?[ -]*(?:cross|crossover)", t, re.I)
    golden = re.search(r"golden cross", t, re.I)
    generic_cross = re.search(r"(?:fast\s*/\s*slow|ma)[ -]*(?:cross|crossover)", t, re.I)
    if cross:
        out.append(Match("dual_sma_cross", {"fast": int(cross.group(1)), "slow": int(cross.group(2))},
                         {"fast": cross.group(0), "slow": cross.group(0)}, ()))
    elif golden:
        out.append(Match("dual_sma_cross", {}, {}, ("fast", "slow"), {"fast": "alias:golden cross=50/200",
                                                                       "slow": "alias:golden cross=50/200"}))
    elif generic_cross:
        out.append(Match("dual_sma_cross", {}, {}, ("fast", "slow")))

    if not (cross or golden or generic_cross) and re.search(
            r"trend (?:gate|filter)|\babove (?:its |the |prior[- ]session )?.{0,20}(?:\bsma\b|\bma\b|moving average)", t, re.I):
        w = next(_ma_words(t), None)
        if not re.search(r"buffer|hysteresis|cooldown|slope", t, re.I):
            out.append(Match("sma_trend_gate", {"window": int(w.group(1))} if w else {},
                             {"window": w.group(0)} if w else {}, () if w else ("window",)))

    mom = re.search(rf"{_NUM}[- ]?(?:day|session)s? (?:absolute |time[- ]series )?momentum", t, re.I)
    if mom:
        out.append(Match("abs_momentum_sessions", {"lookback": int(mom.group(1))}, {"lookback": mom.group(0)}, ()))
    elif re.search(r"absolute (?:etf )?momentum|time[- ]series momentum", t, re.I):
        out.append(Match("abs_momentum_sessions", {}, {}, ("lookback",)))
    m12 = re.search(rf"{_NUM}[- ]?(?:month|m)s? (?:absolute )?momentum", t, re.I)
    if m12:
        out.append(Match("monthly_abs_momentum", {"lookback_months": int(m12.group(1))}, {"lookback_months": m12.group(0)}, ("skip_months",)))

    near = re.search(rf"within (\d{{1,2}}(?:\.\d)?)\s*% of .{{0,30}}high", t, re.I)
    if near and not re.search(r"low-float|earnings", t, re.I):
        bps = round(float(near.group(1)) * 100)
        out.append(Match("near_high_proximity", {"proximity_bps": bps}, {"proximity_bps": near.group(0)}, ("window", "trend_window")))

    mw = re.search(r"\b(jan|feb|mar|apr|may|jun|jul|aug|sep|oct|nov|dec)[a-z]*\s*(?:-|to|through)\s*(jan|feb|mar|apr|may|jun|jul|aug|sep|oct|nov|dec)[a-z]*", t, re.I)
    if mw:
        out.append(Match("month_window", {"start_month": _MONTHS[mw.group(1).lower()], "end_month": _MONTHS[mw.group(2).lower()]},
                         {"start_month": mw.group(0), "end_month": mw.group(0)}, ()))

    rsi = re.search(rf"rsi\(?\s*{_NUM}\s*\)?", t, re.I)
    if rsi:
        lo = re.search(rf"rsi\(?\s*{_NUM}\s*\)?\s*(?:<|below|under)\s*(\d{{1,2}})", t, re.I)
        hi_ = re.search(rf"rsi\(?\s*{_NUM}\s*\)?\s*(?:>|above|over)\s*(\d{{1,2}})", t, re.I)
        explicit, basis = {"rsi_window": int(rsi.group(1))}, {"rsi_window": rsi.group(0)}
        if lo:
            explicit["entry_below"], basis["entry_below"] = int(lo.group(2)), lo.group(0)
        if hi_:
            explicit["exit_above"], basis["exit_above"] = int(hi_.group(2)), hi_.group(0)
        out.append(Match("rsi_reversion", explicit, basis,
                         tuple(n for n in ("entry_below", "exit_above", "trend_window") if n not in explicit)))
    ch = re.search(rf"{_NUM}[- ]?(?:day|session)s? (?:channel|breakout|high breakout)", t, re.I)
    if ch and not rsi and not near:
        out.append(Match("close_channel", {"entry_window": int(ch.group(1))}, {"entry_window": ch.group(0)}, ("exit_window",)))
    if re.search(r"turn[- ]of[- ]the[- ]month|turn of month", t, re.I):
        out.append(Match("turn_of_month", {}, {}, ("last_k", "first_m")))
    if re.search(r"pre[- ]holiday", t, re.I):
        out.append(Match("pre_holiday", {}, {}, ("sessions",)))
    return _fold_trend_filter(out)


_FILTERABLE = ("rsi_reversion", "zscore_reversion", "near_high_proximity", "trend_pullback_hold", "atr_drop_reversal")


def _fold_trend_filter(matches: list[Match]) -> list[Match]:
    """A trend cue next to a filterable rule is that rule's `trend_window`, not a second strategy."""
    host = next((m for m in matches if m.template_id in _FILTERABLE), None)
    gate = next((m for m in matches if m.template_id == "sma_trend_gate"), None)
    if host is None or gate is None:
        return matches
    rest = [m for m in matches if m is not gate]
    if "window" in gate.explicit:
        rest[rest.index(host)] = Match(
            host.template_id, {**host.explicit, "trend_window": gate.explicit["window"]},
            {**host.basis, "trend_window": gate.basis["window"]},
            tuple(n for n in host.missing if n != "trend_window"), host.aliases)
    return rest
