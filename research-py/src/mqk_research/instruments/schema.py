from __future__ import annotations

from dataclasses import dataclass
from typing import Literal, Optional


AssetClass = Literal["EQUITY", "OPTIONS", "FUTURES"]


@dataclass(frozen=True)
class Instrument:
    instrument_id: str
    symbol: str
    asset_class: AssetClass


@dataclass(frozen=True)
class EquityInstrument(Instrument):
    asset_class: Literal["EQUITY"] = "EQUITY"


@dataclass(frozen=True)
class OptionInstrument(Instrument):
    """
    Deterministic identifier format (recommended):
      OPTION::<UNDERLYING>::<YYYYMMDD>::<C|P>::<STRIKE>
    Example:
      OPTION::SPY::20260320::C::500.0
    """
    asset_class: Literal["OPTIONS"] = "OPTIONS"
    underlying: Optional[str] = None
    expiry_yyyymmdd: Optional[str] = None
    right: Optional[Literal["C", "P"]] = None
    strike: Optional[float] = None


@dataclass(frozen=True)
class FutureInstrument(Instrument):
    """
    Deterministic identifier format (recommended):
      FUTURE::<ROOT>::<CONTRACT>
    Example:
      FUTURE::ES::ESM2026
    """
    asset_class: Literal["FUTURES"] = "FUTURES"
    root: Optional[str] = None
    contract: Optional[str] = None


def equity_id(symbol: str) -> str:
    return f"EQUITY::{symbol.upper()}"


def option_id(underlying: str, yyyymmdd: str, right: str, strike: float) -> str:
    r = right.upper()
    if r not in ("C", "P"):
        raise ValueError("right must be 'C' or 'P'")
    return f"OPTION::{underlying.upper()}::{yyyymmdd}::{r}::{strike}"


def future_id(root: str, contract: str) -> str:
    return f"FUTURE::{root.upper()}::{contract.upper()}"


def parse_instrument_id(instrument_id: str) -> Instrument:
    """
    Parse deterministic instrument ids. Used for artifact sanity and future multi-asset.
    Phase 1 uses EQUITY only, but this enables Phase 2+ stubs safely.
    """
    if not instrument_id or "::" not in instrument_id:
        raise ValueError(f"Invalid instrument_id: {instrument_id}")

    parts = instrument_id.split("::")
    tag = parts[0].upper()

    if tag == "EQUITY":
        if len(parts) != 2:
            raise ValueError(f"Invalid equity instrument_id: {instrument_id}")
        sym = parts[1].upper()
        return EquityInstrument(instrument_id=equity_id(sym), symbol=sym)

    if tag == "OPTION":
        # OPTION::<UNDERLYING>::<YYYYMMDD>::<C|P>::<STRIKE>
        if len(parts) != 6:
            raise ValueError(f"Invalid option instrument_id: {instrument_id}")
        under = parts[1].upper()
        yyyymmdd = parts[2]
        right = parts[3].upper()  # C/P
        strike = float(parts[4])
        sym = under  # symbol field stores underlying for now
        return OptionInstrument(
            instrument_id=instrument_id,
            symbol=sym,
            underlying=under,
            expiry_yyyymmdd=yyyymmdd,
            right=right,  # type: ignore[arg-type]
            strike=strike,
        )

    if tag == "FUTURE":
        # FUTURE::<ROOT>::<CONTRACT>
        if len(parts) != 3:
            raise ValueError(f"Invalid future instrument_id: {instrument_id}")
        root = parts[1].upper()
        contract = parts[2].upper()
        sym = root
        return FutureInstrument(
            instrument_id=instrument_id,
            symbol=sym,
            root=root,
            contract=contract,
        )

    raise ValueError(f"Unknown instrument tag in instrument_id: {instrument_id}")