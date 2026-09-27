"""
D6/A3 (V4-M5-M8-APPROVED-DECISIONS-IMPLEMENTATION-01) -- additive fractional
QtyMicros sizing protocol (`weight_to_target_qty_micros`) parity with the
frozen V1 whole-share protocol (`weight_to_target_qty`).

Focused unit tests only (no economic_walkforward.py integration -- this is
an additive, not-yet-wired sizing seam per the mission's explicit scope:
"Do NOT replace the existing whole-unit Python path").
"""
from __future__ import annotations

from typing import Optional

import pytest

from mqk_research.ml.weight_to_share import (
    QTY_MICROS_SCALE,
    WEIGHT_TO_SHARE_QTY_MICROS_PROTOCOL_ID_V1,
    WeightToShareSpec,
    weight_to_share_qty_micros_protocol_identity,
    weight_to_target_qty,
    weight_to_target_qty_micros,
)


def _truncate_to_whole_toward_zero(qty_micros: int) -> int:
    """Reference truncation used only by these tests: magnitude-floor then
    restore sign -- the same convention V1 itself uses (mirrors
    WEIGHT_TO_SHARE_ROUNDING_POLICY_V1's "floor_toward_zero_magnitude_v1"),
    applied at whole-unit granularity to a QtyMicros value. Deliberately NOT
    plain Python `//` -- that floors toward negative infinity and would
    give the wrong answer for a negative qty_micros that isn't an exact
    multiple of QTY_MICROS_SCALE (e.g. -1_999_999 // 1_000_000 == -2 in
    Python, but the truncate-toward-zero share count is -1).
    """
    if qty_micros == 0:
        return 0
    sign = 1 if qty_micros > 0 else -1
    return sign * (abs(qty_micros) // QTY_MICROS_SCALE)


# --- QSM01: weight == 0.0 always returns 0, no price required ---


def test_qsm01_zero_weight_returns_zero_without_price():
    spec = WeightToShareSpec()
    assert weight_to_target_qty_micros(weight=0.0, price=None, spec=spec) == 0
    assert weight_to_target_qty_micros(weight=0.0, price=-5.0, spec=spec) == 0


# --- QSM02: a nonzero weight with a missing/invalid price fails closed ---


@pytest.mark.parametrize("bad_price", [None, 0.0, -1.0, float("nan"), float("inf")])
def test_qsm02_nonzero_weight_requires_valid_price(bad_price: Optional[float]):
    spec = WeightToShareSpec()
    with pytest.raises(RuntimeError, match=WEIGHT_TO_SHARE_QTY_MICROS_PROTOCOL_ID_V1):
        weight_to_target_qty_micros(weight=0.1, price=bad_price, spec=spec)


# --- QSM03: pure function determinism -- retries/replays never manufacture
# a different result for identical inputs ---


def test_qsm03_deterministic_replay():
    spec = WeightToShareSpec(equity_usd=100_000.0)
    a = weight_to_target_qty_micros(weight=0.037, price=123.45, spec=spec)
    b = weight_to_target_qty_micros(weight=0.037, price=123.45, spec=spec)
    assert a == b


# --- QSM04: parity table -- truncating the exact QtyMicros result down to
# whole shares always reproduces weight_to_target_qty (V1) exactly, for both
# positive and negative weights, with and without caps. ---

_PARITY_CASES = [
    (0.5, 100.0, WeightToShareSpec(equity_usd=100_000.0)),
    (-0.5, 100.0, WeightToShareSpec(equity_usd=100_000.0)),
    (0.037, 123.45, WeightToShareSpec(equity_usd=100_000.0)),
    (-0.037, 123.45, WeightToShareSpec(equity_usd=100_000.0)),
    (1.0, 7.0, WeightToShareSpec(equity_usd=100_000.0)),
    (0.9, 50_000.0, WeightToShareSpec(equity_usd=100_000.0)),  # sub-1-share whole case
    (0.5, 33.0, WeightToShareSpec(equity_usd=100_000.0, max_target_qty=1_000)),
    (-0.5, 33.0, WeightToShareSpec(equity_usd=100_000.0, max_target_qty=1_000)),
    (0.9, 10.0, WeightToShareSpec(equity_usd=100_000.0, max_position_notional_usd=1_000.0)),
    (-0.9, 10.0, WeightToShareSpec(equity_usd=100_000.0, max_position_notional_usd=1_000.0)),
]


@pytest.mark.parametrize("weight,price,spec", _PARITY_CASES)
def test_qsm04_truncated_qty_micros_matches_v1_whole_share_output(
    weight: float, price: float, spec: WeightToShareSpec
):
    qty_micros = weight_to_target_qty_micros(weight=weight, price=price, spec=spec)
    qty_whole_v1 = weight_to_target_qty(weight=weight, price=price, spec=spec)
    assert _truncate_to_whole_toward_zero(qty_micros) == qty_whole_v1


# --- QSM05: the fractional seam preserves information V1 necessarily
# discards -- a case where V1 floors an entire sub-share position to 0 (no
# order at all) while V2 still carries the genuine fractional exposure. ---


def test_qsm05_fractional_seam_preserves_sub_share_exposure_v1_discards():
    spec = WeightToShareSpec(equity_usd=100_000.0)
    weight = 0.0004  # 0.0004 * 100_000 = $40 notional
    price = 50_000.0  # e.g. BTC/USD -- $40 / $50,000 = 0.0008 units
    qty_whole_v1 = weight_to_target_qty(weight=weight, price=price, spec=spec)
    qty_micros = weight_to_target_qty_micros(weight=weight, price=price, spec=spec)
    assert qty_whole_v1 == 0, "V1 must floor a sub-share position to exactly 0 shares"
    assert qty_micros == 800, "V2 must carry the exact 0.0008-unit fractional exposure"


# --- QSM06/07: caps bind at QtyMicros granularity, scaling the whole-share
# cap by QTY_MICROS_SCALE exactly (proven directly, not just via the
# truncation-parity table above). ---


def test_qsm06_max_target_qty_cap_binds_in_micros():
    spec = WeightToShareSpec(equity_usd=1_000_000.0, max_target_qty=5)
    # Uncapped magnitude would be far above 5 shares.
    qty_micros = weight_to_target_qty_micros(weight=1.0, price=1.0, spec=spec)
    assert qty_micros == 5 * QTY_MICROS_SCALE


def test_qsm07_max_position_notional_usd_cap_binds_in_micros():
    spec = WeightToShareSpec(equity_usd=1_000_000.0, max_position_notional_usd=250.0)
    qty_micros = weight_to_target_qty_micros(weight=1.0, price=100.0, spec=spec)
    # $250 notional / $100 price = 2.5 units = 2_500_000 QtyMicros exactly.
    assert qty_micros == 2_500_000


# --- QSM08: protocol identity fragment is distinct from V1's and never
# conflatable with it; spec=None mirrors V1's own diagnostic-state shape. ---


def test_qsm08_protocol_identity_shape():
    assert weight_to_share_qty_micros_protocol_identity(None) == {
        "weight_to_share_qty_micros_protocol_id": None
    }
    frag = weight_to_share_qty_micros_protocol_identity(WeightToShareSpec(equity_usd=50_000.0))
    assert frag["weight_to_share_qty_micros_protocol_id"] == WEIGHT_TO_SHARE_QTY_MICROS_PROTOCOL_ID_V1
    assert frag["qty_micros_scale"] == QTY_MICROS_SCALE
    assert frag["equity_usd"] == 50_000.0


# --- QSM09: overflow/malformed weight refuses (mirrors V1's fail-closed
# contract for a non-finite weight). ---


@pytest.mark.parametrize("bad_weight", [float("nan"), float("inf"), float("-inf")])
def test_qsm09_non_finite_weight_refuses(bad_weight: float):
    spec = WeightToShareSpec()
    with pytest.raises(ValueError):
        weight_to_target_qty_micros(weight=bad_weight, price=100.0, spec=spec)
