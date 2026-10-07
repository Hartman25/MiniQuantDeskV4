"""Census-02 short ConditionalEdge factor authority: every short condition x horizon is a deterministic FactorSpec whose
direction is fixed BEFORE result #1. The conditional effect is a LABEL (diagnostic forward return), never P&L."""

from __future__ import annotations

import sys
from pathlib import Path

HERE = Path(__file__).resolve().parent
sys.path.insert(0, str(HERE))
import c2_grammar as gr  # noqa: E402
import c2_protocol as pr  # noqa: E402
from mqk_research.factors.contracts import (  # noqa: E402
    DIRECTION_LOWER_IS_BETTER, NORMALIZATION_RAW, TIMING_SAME_BAR_CLOSE, FactorSpec)

FACTOR_FAMILY = pr.FACTOR_SEMANTICS["family"]
FACTOR_PROTOCOL_VERSION = pr.FACTOR_SEMANTICS["protocol_version"]
SHORT_FACTOR_DIRECTION = pr.FACTOR_SEMANTICS["direction"]
LABEL = pr.FACTOR_SEMANTICS["label"]
assert SHORT_FACTOR_DIRECTION == DIRECTION_LOWER_IS_BETTER

_REQUIRED_FIELDS = {"SH01": ["close"], "SH02": ["close"], "SH03": ["close"], "SH04": ["close"], "SH05": ["close"],
                    "SH06": ["close"], "SH07": ["close", "high", "low"], "SH08": ["close", "high", "low", "open"],
                    "SH09": ["close", "high", "low"], "SH10": ["close"], "SH11": ["close"],
                    "SH12": ["close", "volume"], "SH13": ["close"]}


class FactorAuthorityRefusal(RuntimeError):
    pass


def condition_lookback(family: str, p: dict) -> int:
    """Index of the first completed bar at which the short CONDITION is defined; execution-only params never enter."""
    trend = 199 if p.get("trend") == "below_sma200" else 0
    table = {"SH01": lambda: p["lookback"], "SH02": lambda: p["sma"] - 1, "SH03": lambda: p["slow"] - 1,
             "SH04": lambda: p["entry"], "SH05": lambda: max(p["period"], trend), "SH06": lambda: max(p["lookback"] - 1, trend),
             "SH07": lambda: max(p["atr_window"] + 1, p["rise_sessions"], trend), "SH08": lambda: max(p["atr_window"] + 1, trend),
             "SH09": lambda: max(p["long"], p["breakdown"], trend), "SH10": lambda: p["low_lookback"] - 1,
             "SH11": lambda: max(p["up_sessions"], trend), "SH12": lambda: max(p["volume_lookback"], p["price_impulse"]),
             "SH13": lambda: max(p["short_vol"], p["long_vol"])}
    if family not in table:
        raise ValueError(f"unsupported short family {family!r}")
    return table[family]()


def factor_spec(condition: dict, horizon: int, ctx: dict, *, direction: str = SHORT_FACTOR_DIRECTION) -> FactorSpec:
    """Result-independent FactorSpec of one short conditional relationship. `direction` exists only so a test can prove a
    flipped direction is a different identity; the registered population is built with the fixed short direction."""
    if horizon not in gr.HORIZONS:
        raise ValueError(f"horizon {horizon} is not one of {gr.HORIZONS}")
    fam, cp = condition["family"], condition["params"]
    if fam not in gr.CONDITION_PARAM_KEYS or set(cp) != set(gr.CONDITION_PARAM_KEYS[fam]):
        raise ValueError(f"{fam}: condition params {sorted(cp)} are not exactly the condition-defining parameters")
    if condition["condition_id"] != gr.condition_id(fam, cp):
        raise ValueError(f"{fam}: condition_id does not match the canonical semantic condition")
    return FactorSpec(
        family=FACTOR_FAMILY, name=f"{fam}:{condition['condition_id']}:h{horizon}", protocol_version=FACTOR_PROTOCOL_VERSION,
        params={"condition_family": fam, "condition_params": cp, "condition_id": condition["condition_id"], "side": "short",
                "scope": "symbol", "condition_encoding": "binary_0_1_at_completed_bar", "label": LABEL,
                "effect": "raw=conditional_mean_minus_baseline; direction_adjusted=-raw; positive=favorable_to_short"},
        required_input_fields=list(_REQUIRED_FIELDS[fam]), lookback_periods=condition_lookback(fam, cp),
        horizon_periods=horizon, normalization=NORMALIZATION_RAW, direction=direction,
        universe_identity=dict(ctx["universe_identity"]), data_provenance_identity=dict(ctx["data_provenance_identity"]),
        timing_convention=TIMING_SAME_BAR_CLOSE, information_lag_periods=0, layout_note="")


def direction_adjusted_effect(raw_effect: float, direction: str = SHORT_FACTOR_DIRECTION) -> float:
    """raw = conditional mean fwd_ret - same-symbol same-horizon unconditional mean. A lower-is-better short factor is
    scored by -raw; a positive value is favorable evidence for the short hypothesis."""
    if direction != SHORT_FACTOR_DIRECTION:
        raise FactorAuthorityRefusal(f"short factors are lower_is_better; {direction!r} is refused (no post-result flip)")
    return -float(raw_effect)


def iter_factor_specs(conditions: list[dict], ctx: dict):
    for cond in conditions:
        for h in gr.HORIZONS:
            yield cond, h, factor_spec(cond, h, ctx)


def expected_factor_ids(conditions: list[dict], ctx: dict) -> list[str]:
    return [spec.compute_factor_id() for _c, _h, spec in iter_factor_specs(conditions, ctx)]


def require_registered_short_factor(spec: FactorSpec, conditions: list[dict], ctx: dict) -> str:
    """A factor evaluated for Census-02 must be exactly a member of the registered short population: a spec whose direction
    (or any other identity field) differs has a different id and is refused."""
    if spec.direction != SHORT_FACTOR_DIRECTION:
        raise FactorAuthorityRefusal("short factor direction must be lower_is_better")
    fid = spec.compute_factor_id()
    if fid not in set(expected_factor_ids(conditions, ctx)):
        raise FactorAuthorityRefusal("factor is not a member of the registered Census-02 short population")
    return fid
