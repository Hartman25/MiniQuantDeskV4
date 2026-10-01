"""Binds the frozen batch-01 predeclaration to the executable Rust engine sources."""

from __future__ import annotations

import json
import re
from pathlib import Path

HERE = Path(__file__).resolve().parent
REPO = HERE.parents[2]
B = json.loads((HERE / "PREDECLARED_BATCH_01.json").read_text(encoding="utf-8"))
ENGINES = REPO / "core-rs/crates/mqk-strategy/src/engines"
FILES = {
    "absolute_momentum_252": "absolute_momentum_252.rs",
    "near_high_momentum_252_3pct": "near_high_momentum_252_3pct.rs",
    "trend_pullback_5d_4pct_hold5": "trend_pullback_5d_4pct_hold5.rs",
}


def src(strategy: str) -> str:
    return (ENGINES / FILES[strategy]).read_text(encoding="utf-8")


def const(text: str, ty: str, name: str) -> int:
    return int(re.search(rf"const {name}: {ty} = ([\d_]+);", text).group(1).replace("_", ""))


def test_strategy_ids_and_timeframe_match_the_sources():
    for h in B["hypotheses"]:
        text = src(h["strategy_id"])
        assert re.search(r'const NAME: &str = "([^"]+)"', text).group(1) == h["strategy_id"]
        assert const(text, "i64", "TIMEFRAME_SECS") == h["timeframe_secs"] == 86400


def test_h1_constants_match():
    text = src("absolute_momentum_252")
    assert const(text, "usize", "MOMENTUM_BARS") == 252
    assert "const REQUIRED_BARS: usize = MOMENTUM_BARS + 1;" in text
    assert next(h for h in B["hypotheses"] if h["strategy_id"] == "absolute_momentum_252")["required_history_bars"] == 253


def test_h2_constants_match():
    text = src("near_high_momentum_252_3pct")
    assert const(text, "usize", "HIGH_LOOKBACK") == 252
    assert const(text, "usize", "MEAN_LOOKBACK") == 50
    assert const(text, "i128", "PROXIMITY_NUM") == 97
    assert next(h for h in B["hypotheses"] if h["strategy_id"] == "near_high_momentum_252_3pct")["required_history_bars"] == 252


def test_h3_constants_match():
    text = src("trend_pullback_5d_4pct_hold5")
    assert const(text, "usize", "TREND_LOOKBACK") == 200
    assert const(text, "usize", "PULLBACK_BARS") == 5
    assert const(text, "i128", "RETAIN_PCT") == 96
    assert const(text, "usize", "HOLD_BARS") == 5
    assert "const REQUIRED_BARS: usize = TREND_LOOKBACK + HOLD_BARS - 1;" in text
    assert 200 + 5 - 1 == next(h for h in B["hypotheses"] if h["strategy_id"] == "trend_pullback_5d_4pct_hold5")["required_history_bars"] == 204


def test_engines_are_stateless_and_long_flat():
    for strategy in FILES:
        non_test = src(strategy).split("#[cfg(test)]")[0]
        struct = re.search(r"pub struct \w+ \{(.*?)\}", non_test, re.S).group(1)
        assert [f.strip() for f in struct.split(",") if f.strip()] == ["symbol: String"], strategy
        assert "initialized" not in non_test and "enum Position" not in non_test
        assert "never short" in non_test.lower()
