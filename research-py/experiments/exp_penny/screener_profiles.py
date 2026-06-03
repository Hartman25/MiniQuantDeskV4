"""
EXP-PENNY-01A — screener export profile mappings.

Defines column alias maps for common screener CSV export formats so that
downloaded files can be normalized into the canonical universe schema before
validation and coercion.

Supported profiles:
  generic      — canonical headers only (no remapping)
  finviz       — Finviz screener export column names
  tradingview  — TradingView screener export column names

This module performs header-name mapping only. It never invents or computes
missing technical values (MA slopes, consolidation range, breakout RVOL, etc.).
If a required canonical field has no alias in the source file the loader will
raise UniverseLoadError naming the missing field so the operator knows exactly
what to add to the export.

No network calls. No broker calls. No OMS/outbox/inbox access. No DB writes.
"""
from __future__ import annotations

# ---------------------------------------------------------------------------
# Alias maps: source_header -> canonical_field_name
#
# Only columns whose exported header differs from the canonical name are
# listed here. All other columns must already use canonical names in the
# source file.
# ---------------------------------------------------------------------------

_PROFILE_ALIASES: dict[str, dict[str, str]] = {
    "generic": {},
    "finviz": {
        "Ticker": "symbol",
        "Price": "price",
        "Volume": "volume",
        "Rel Volume": "breakout_rvol",
        "Average Volume": "adv_20d_shares",
    },
    "tradingview": {
        "Symbol": "symbol",
        "Last": "price",
        "Close": "price",
        "Volume": "volume",
        "Average Volume 20": "adv_20d_shares",
        "Vol 20D Avg": "adv_20d_shares",
        "Relative Volume": "breakout_rvol",
    },
}

SUPPORTED_PROFILES: tuple[str, ...] = tuple(_PROFILE_ALIASES.keys())


def get_profile_aliases(profile: str) -> dict[str, str]:
    """
    Return the alias map for the given profile name.

    Raises KeyError for unknown profiles — callers convert this to
    UniverseLoadError with an informative message.
    """
    if profile not in _PROFILE_ALIASES:
        raise KeyError(profile)
    return _PROFILE_ALIASES[profile]


def apply_profile(headers: list[str], profile: str) -> list[str]:
    """
    Remap a list of CSV header strings using the given profile's alias map.

    Headers that have no alias entry are passed through unchanged so that the
    downstream required-field validator can report exactly which canonical
    field is missing rather than silently dropping columns.

    Raises KeyError if the profile is unsupported.
    """
    aliases = get_profile_aliases(profile)
    return [aliases.get(h, h) for h in headers]
