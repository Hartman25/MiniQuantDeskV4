"""Deterministic index of every hypothesis MQD already knows or has searched, keyed by template signature.

Sources (all read-only, none result-dependent): the native engine cards, the frozen historical batch/campaign
declarations (which strategies were already evaluated, and in which batch), the Census-01 long-state grids and the
Census-02 short mirrors, and ideas the Factory itself already admitted. The index never reads a P&L, rank or verdict.
"""

from __future__ import annotations

import json
from dataclasses import dataclass
from pathlib import Path
from typing import Iterable, Mapping

from mqk_research.strategy_factory.templates import NATIVE_CARDS, TEMPLATES

CADENCE_DAILY = "daily"


@dataclass(frozen=True)
class KnownEntry:
    known_id: str
    source: str                      # native_card | census_01 | census_02 | factory_prior
    template_id: str
    params: tuple[tuple[str, int], ...]
    direction: str
    extras: tuple[tuple[str, str], ...] = ()
    tested_in: tuple[str, ...] = ()

    def signature(self) -> tuple:
        return (self.template_id, self.direction, self.params, self.extras)


def _p(**kw: int) -> tuple[tuple[str, int], ...]:
    return tuple(sorted(kw.items()))


def native_entries(tested_in: Mapping[str, Iterable[str]] | None = None) -> list[KnownEntry]:
    tested_in = tested_in or {}
    return [KnownEntry(f"native:{c.strategy_id}", "native_card", c.template_id, _p(**c.params), c.direction,
                       (), tuple(sorted(set(tested_in.get(c.strategy_id, ())))))
            for c in NATIVE_CARDS if c.template_id != "legacy_engine"]


def historical_tested(repo_root: Path) -> dict[str, list[str]]:
    """strategy_id -> batch/campaign ids whose frozen declaration names it as a hypothesis or engine."""
    out: dict[str, list[str]] = {}
    base = repo_root / "research-py" / "experiments" / "m1_native_trend_campaign"
    for path in sorted(base.glob("PREDECLARED_*.json")):
        decl = json.loads(path.read_text(encoding="utf-8"))
        ident = decl.get("batch_id") or decl.get("campaign_id") or path.stem
        ids = [h.get("strategy_id") for h in decl.get("hypotheses", []) if isinstance(h, dict)]
        ids.append((decl.get("native_engine") or {}).get("strategy_id"))
        for sid in filter(None, ids):
            out.setdefault(sid, []).append(ident)
    return {k: sorted(set(v)) for k, v in out.items()}


# Census-01 long-state grids (experiments/alpha_edge_census_01/search_space.py); a test cross-checks every value.
_TREND = {"none": 0, "sma200": 200}
_CADENCES = ("daily", "month_end")


def census_entries() -> list[KnownEntry]:
    out: list[KnownEntry] = []

    def add(src, kid, tid, direction, extras=(), **params):
        out.append(KnownEntry(f"{src}:{kid}", src, tid, _p(**params), direction, tuple(extras)))

    for pre, src, direction in (("S", "census_01", "long_flat"), ("SH", "census_02", "short_flat")):
        for L in (21, 63, 126, 252):
            for c in _CADENCES:
                add(src, f"{pre}01_L{L}_{c}", "abs_momentum_sessions", direction, (("cadence", c),) if c != "daily" else (), lookback=L)
        for m in (20, 50, 100, 150, 200, 250):
            add(src, f"{pre}02_{m}", "sma_trend_gate", direction, window=m)
        for f in (10, 20, 50):
            for s in (50, 100, 150, 200, 250):
                if f < s:
                    add(src, f"{pre}03_{f}_{s}", "dual_sma_cross", direction, fast=f, slow=s)
        for e in (20, 50, 100, 150, 200):
            for x in (10, 20, 50):
                if x < e:
                    add(src, f"{pre}04_{e}_{x}", "close_channel", direction, entry_window=e, exit_window=x)
    for p in (2, 3, 5, 7, 10, 14):
        for e in (10, 20, 30):
            for x in (50, 70):
                for t, tw in _TREND.items():
                    add("census_01", f"S05_{p}_{e}_{x}_{t}", "rsi_reversion", "long_flat", rsi_window=p, entry_below=e,
                        exit_above=x, trend_window=tw)
    for L in (10, 20, 40, 60):
        for z in (15, 20, 25):
            for t, tw in _TREND.items():
                add("census_01", f"S06_{L}_{z}_{t}", "zscore_reversion", "long_flat", window=L, entry_z_x10=z, trend_window=tw)
    for L in (126, 252):
        for d in (300, 500, 1000):
            for c in _CADENCES:
                add("census_01", f"S10_{L}_{d}_{c}", "near_high_proximity", "long_flat",
                    (("cadence", c),) if c != "daily" else (), window=L, proximity_bps=d, trend_window=0)
    for a, b in ((1, 3), (2, 3), (1, 5)):
        add("census_01", f"S14_tom_{a}_{b}", "turn_of_month", "long_flat", last_k=a, first_m=b)
    add("census_01", "S14_nov_apr", "month_window", "long_flat", start_month=11, end_month=4)
    return out


# Templates that describe the same economic mechanism in different parameterizations or cadences.
SEMANTIC_NEIGHBORS = frozenset(frozenset(p) for p in (
    ("abs_momentum_sessions", "monthly_abs_momentum"), ("abs_momentum_sessions", "monthly_consensus_momentum"),
    ("monthly_abs_momentum", "monthly_consensus_momentum"), ("sma_trend_gate", "monthly_trend_timing"),
    ("sma_trend_gate", "dual_sma_cross"), ("close_channel", "fixed_hold_breakout"),
    ("near_high_proximity", "monthly_near_high"), ("zscore_reversion", "rsi_reversion"),
    ("zscore_reversion", "trend_pullback_hold"), ("rsi_reversion", "trend_pullback_hold"),
    ("gap_reversal", "atr_drop_reversal"), ("turn_of_month", "pre_holiday"),
))


def build_index(repo_root: Path, factory_prior: Iterable[KnownEntry] = ()) -> list[KnownEntry]:
    tested = historical_tested(repo_root)
    entries = native_entries(tested) + census_entries() + list(factory_prior)
    ids = [e.known_id for e in entries]
    if len(set(ids)) != len(ids):
        raise ValueError("duplicate known_id in the known index")
    for e in entries:
        TEMPLATES[e.template_id].validate(dict(e.params))
    return sorted(entries, key=lambda e: e.known_id)
