from __future__ import annotations

from pathlib import Path
from typing import Any, Dict, FrozenSet, List, Optional, Sequence

import pandas as pd

from mqk_research.ml.util_hash import sha256_file, sha256_json

# BKT-DATA-PROVENANCE-POINT-IN-TIME-01-REPAIR-01
#
# Durable, content-addressed provenance manifest for OFFICIAL research bars
# data, and the fail-closed corporate-action preflight that must run before
# any economic P&L is computed from those bars.
#
# Distinct from two OTHER, already-existing "provenance" concepts in this
# codebase that this module does not replace:
#   - bars_postgres.py's price_provenance: query-time metadata attached to a
#     DataFrame's `.attrs["price_provenance"]`. Useful in-memory, but NOT
#     durable -- `.attrs` does not survive a `DataFrame.to_csv()` /
#     `pd.read_csv()` round trip (see
#     test_dataframe_attrs_alone_is_not_durable_provenance), which is
#     exactly how bars data actually reaches the registered economic
#     evaluator (economic_registry_integration.
#     run_registered_economic_walkforward_eval takes a bars_csv PATH, not a
#     DataFrame). This module's manifest is the durable, content-addressed
#     record that survives that trip.
#   - economic_walkforward.verify_bars_provenance: an unrelated existing
#     check that bars_csv's file bytes match what shadow_label_meta.json
#     recorded when targets.csv's labels were built. That check answers
#     "is this the SAME bars file the labels came from?" -- it says nothing
#     about the price data's own adjustment convention or corporate-action
#     safety, which is what THIS module answers.
#
# Root Defect 2 (raw prices + corporate actions): the only verified price
# convention anywhere in this system today is raw_unadjusted (see
# bars_postgres.py's BKT-DATA-PROVENANCE-POINT-IN-TIME-01 header). Raw,
# unadjusted closes make a stock split look like a real ~50% loss and omit
# dividend cash flows entirely (core-rs/crates/mqk-backtest/src/
# corporate_actions.rs documents the same fact for the Rust engine). This
# module mirrors that Rust module's conservative two-policy design
# (Allow / ForbidPeriods) rather than building a real adjustment-tables
# subsystem: an explicit ADJUSTED_DATA claim (valid only when actually
# verified) or an explicit FORBID_AFFECTED_PERIODS exclusion (valid only
# with real, content-addressed exclusion evidence). No authoritative
# corporate-action exclusion source exists anywhere in this repository today
# (no DB table, no migration, no fixture) -- so for real RAW_UNADJUSTED
# registered data, this module is DESIGNED to fail closed until that source
# exists. See docs/research/Research_Backtest_V1_Closeout_Audit.md for the
# P8 PARTIAL — CORPORATE_ACTION_SOURCE_REQUIRED status this implies.

SCHEMA_VERSION = "bars_provenance_manifest_v1"

# --- price adjustment convention (mirrors bars_postgres.py's vocabulary) ---
PRICE_CONVENTION_RAW_UNADJUSTED = "raw_unadjusted"
PRICE_CONVENTION_UNVERIFIABLE = "unverifiable"

# No price-adjustment convention other than raw_unadjusted has ever been
# independently verified in this system (see bars_postgres.py's
# BKT-DATA-PROVENANCE-POINT-IN-TIME-01 header and the P8 Wave-1 audit
# findings) -- this set is intentionally empty in production. It exists as
# a named seam so a FUTURE verified adjusted-data provider can be added
# here explicitly (an auditable code change) rather than the
# adjusted_data policy being silently accepted for an unverified source.
_KNOWN_ADJUSTED_CONVENTIONS: FrozenSet[str] = frozenset()

# --- corporate-action policy (mirrors mqk-backtest::CorporateActionPolicy) ---
CA_POLICY_ADJUSTED_DATA = "adjusted_data"
CA_POLICY_FORBID_AFFECTED_PERIODS = "forbid_affected_periods"
CA_POLICY_DIAGNOSTIC_SYNTHETIC_UNPROTECTED = "diagnostic_synthetic_unprotected"

# The only two policies that constitute an honest corporate-action safety
# guarantee for OFFICIAL registered research. The diagnostic policy exists
# solely for synthetic/unit-test fixtures reached through the low-level,
# non-registered economic_walkforward.run_economic_walkforward path -- see
# that function's provenance_manifest=None default, which is the actual
# diagnostic escape hatch; this constant is never treated as satisfying the
# registered contract.
_REAL_CA_POLICIES: FrozenSet[str] = frozenset({CA_POLICY_ADJUSTED_DATA, CA_POLICY_FORBID_AFFECTED_PERIODS})

# --- universe mode ---
# Confirmed by direct code reading (Research_Backtest_V1_Closeout_Audit.md
# P8 findings): this system's universe construction is unambiguously
# "fixed, explicit ex-ante" (the operator supplies --symbols; no code path
# anywhere queries historical index/constituent membership). Point-in-time
# dynamic universe semantics are NOT implemented -- claiming that mode here
# would be false, so it is named but deliberately unsupported.
UNIVERSE_MODE_FIXED_EX_ANTE = "fixed_ex_ante"
UNIVERSE_MODE_POINT_IN_TIME = "point_in_time"
_SUPPORTED_UNIVERSE_MODES: FrozenSet[str] = frozenset({UNIVERSE_MODE_FIXED_EX_ANTE})

_CANONICAL_HASH_COLUMNS = ("symbol", "end_ts", "close")


class BarsProvenanceUnverifiable(RuntimeError):
    """Fail-closed: raised when a bars provenance manifest does not satisfy
    the durable, structural contract required for OFFICIAL registered
    economic evaluation (see require_registered_bars_provenance)."""


class CorporateActionIntegrityError(RuntimeError):
    """Fail-closed: raised BEFORE any economic P&L is computed when the
    manifest's declared corporate-action policy cannot be honestly
    satisfied for the bars actually being evaluated (see
    check_corporate_action_integrity)."""


def canonical_semantic_bars_hash(bars: pd.DataFrame) -> str:
    """Content hash of the bars' SEMANTIC content (symbol, end_ts, close),
    normalized by sorting rows chronologically per-symbol and selecting
    columns in a FIXED order -- invariant to the physical file's row order
    or column order, sensitive to any actual change in symbol/timestamp/
    price content. Distinct from a physical artifact's raw file-bytes hash
    (sha256_file), which two byte-identical-content-but-differently-ordered
    CSVs will NOT share even though this function's output is identical for
    both (see build_bars_provenance_manifest's artifact_sha256 field)."""
    missing = [c for c in _CANONICAL_HASH_COLUMNS if c not in bars.columns]
    if missing:
        raise ValueError(f"canonical_semantic_bars_hash: bars missing required columns: {missing}")
    normalized = bars[list(_CANONICAL_HASH_COLUMNS)].copy()
    normalized["symbol"] = normalized["symbol"].astype(str)
    normalized["end_ts"] = pd.to_datetime(normalized["end_ts"], utc=True).map(lambda t: t.isoformat())
    normalized["close"] = normalized["close"].astype(float)
    normalized = normalized.sort_values(["symbol", "end_ts"], kind="mergesort").reset_index(drop=True)
    rows = [
        {"symbol": r.symbol, "end_ts": r.end_ts, "close": r.close} for r in normalized.itertuples(index=False)
    ]
    return sha256_json(rows)


def build_bars_provenance_manifest(
    *,
    price_provenance: Dict[str, Any],
    corporate_action_policy: str,
    corporate_action_evidence_id: Optional[str] = None,
    forbidden_periods: Sequence[Dict[str, str]] = (),
    timeframe: str,
    start_utc: str,
    end_utc: str,
    symbol_universe: Sequence[str],
    universe_mode: str,
    bars: pd.DataFrame,
    artifact_path: Optional[Path] = None,
) -> Dict[str, Any]:
    """Pure assembly of one durable, versioned bars provenance manifest. No
    IO except (optionally) hashing `artifact_path`'s bytes if supplied.

    `price_provenance` is the dict produced by
    mqk_research.data.adapters.bars_postgres.resolve_price_provenance (or
    the equivalent fields for a non-Postgres source) -- this function does
    not re-derive it, it durably records what was already resolved.
    `forbidden_periods` are plain {"symbol","start_ts","end_ts"} dicts
    (ISO-8601 UTC, inclusive), matching core-rs's ForbidEntry shape.
    """
    symbols_sorted = sorted({str(s).strip().upper() for s in symbol_universe if str(s).strip()})
    forbidden_sorted = sorted(
        (dict(e) for e in forbidden_periods), key=lambda e: (e["symbol"], e["start_ts"], e["end_ts"])
    )
    return {
        "schema_version": SCHEMA_VERSION,
        "provider_ids_observed": sorted(price_provenance.get("provider_ids_observed") or []),
        "resolved_close_column": price_provenance.get("close_column"),
        "price_adjustment_convention": price_provenance.get("price_adjustment_convention"),
        "corporate_action_policy": corporate_action_policy,
        "corporate_action_evidence_id": corporate_action_evidence_id,
        "forbidden_periods": forbidden_sorted,
        "source_query_identity": {
            "provider_metadata_available": price_provenance.get("provider_metadata_available"),
            "convention_basis": price_provenance.get("convention_basis"),
        },
        "timeframe": timeframe,
        "start_utc": start_utc,
        "end_utc": end_utc,
        "symbol_universe": symbols_sorted,
        "symbol_universe_id": sha256_json(symbols_sorted),
        "universe_mode": universe_mode,
        "canonical_semantic_bars_hash": canonical_semantic_bars_hash(bars),
        "row_count": int(len(bars)),
        "artifact_sha256": sha256_file(Path(artifact_path)) if artifact_path is not None else None,
    }


def provenance_identity_fragment(manifest: Dict[str, Any]) -> Dict[str, Any]:
    """The SUBSET of a bars provenance manifest that belongs in trial
    identity -- must change trial_id whenever it changes, and must NOT
    change trial_id when it doesn't. Deliberately excludes artifact_sha256
    and row_count: two semantically-identical bars files that differ only
    in physical row order share the same canonical_semantic_bars_hash (and
    therefore the same identity) even though their raw file bytes, and
    hence artifact_sha256, differ (see canonical_semantic_bars_hash). Also
    excludes source_query_identity's informational-only sub-fields
    (provider_metadata_available / convention_basis are audit context, not
    an independent economic fact once provider_ids_observed and
    price_adjustment_convention are already included)."""
    return {
        "schema_version": manifest["schema_version"],
        "provider_ids_observed": manifest["provider_ids_observed"],
        "resolved_close_column": manifest["resolved_close_column"],
        "price_adjustment_convention": manifest["price_adjustment_convention"],
        "corporate_action_policy": manifest["corporate_action_policy"],
        "corporate_action_evidence_id": manifest["corporate_action_evidence_id"],
        "forbidden_periods": manifest["forbidden_periods"],
        "timeframe": manifest["timeframe"],
        "start_utc": manifest["start_utc"],
        "end_utc": manifest["end_utc"],
        "symbol_universe": manifest["symbol_universe"],
        "universe_mode": manifest["universe_mode"],
        "canonical_semantic_bars_hash": manifest["canonical_semantic_bars_hash"],
    }


def require_registered_bars_provenance(manifest: Dict[str, Any]) -> None:
    """Fail-closed STRUCTURAL gate for the OFFICIAL registered economic
    evaluation entry point (economic_registry_integration.
    run_registered_economic_walkforward_eval). Raises
    BarsProvenanceUnverifiable unless the manifest is well-formed and
    declares: a known/verified price-adjustment convention, a real
    (non-diagnostic) corporate-action policy, and a supported universe
    mode. This is a manifest-SHAPE check; check_corporate_action_integrity
    (below) does the bars-CONTENT check separately -- both must pass before
    any economic P&L is computed."""
    if manifest.get("schema_version") != SCHEMA_VERSION:
        raise BarsProvenanceUnverifiable(
            f"Fail-closed: bars provenance manifest schema_version={manifest.get('schema_version')!r} "
            f"!= {SCHEMA_VERSION!r}"
        )

    convention = manifest.get("price_adjustment_convention")
    known_conventions = {PRICE_CONVENTION_RAW_UNADJUSTED} | _KNOWN_ADJUSTED_CONVENTIONS
    if convention not in known_conventions:
        raise BarsProvenanceUnverifiable(
            f"Fail-closed: price_adjustment_convention={convention!r} is not a verified/known "
            "convention -- refusing official registered economic evaluation on bars data whose "
            "price-adjustment convention cannot be confirmed (e.g. a provider-attribution gap "
            "reporting 'unverifiable')"
        )

    policy = manifest.get("corporate_action_policy")
    if policy not in _REAL_CA_POLICIES:
        raise BarsProvenanceUnverifiable(
            "Fail-closed: official registered economic evaluation requires corporate_action_policy "
            f"in {sorted(_REAL_CA_POLICIES)!r}, got {policy!r}"
        )

    universe_mode = manifest.get("universe_mode")
    if universe_mode not in _SUPPORTED_UNIVERSE_MODES:
        raise BarsProvenanceUnverifiable(
            f"Fail-closed: universe_mode={universe_mode!r} is not a supported universe mode "
            f"({sorted(_SUPPORTED_UNIVERSE_MODES)!r}) -- point-in-time/dynamic universe membership "
            "is not implemented anywhere in this system; claiming it here would be false"
        )

    if not manifest.get("canonical_semantic_bars_hash"):
        raise BarsProvenanceUnverifiable("Fail-closed: bars provenance manifest missing canonical_semantic_bars_hash")
    if not manifest.get("symbol_universe"):
        raise BarsProvenanceUnverifiable("Fail-closed: bars provenance manifest missing symbol_universe")


def check_corporate_action_integrity(bars: pd.DataFrame, manifest: Dict[str, Any]) -> None:
    """Fail-closed preflight. MUST be called BEFORE any economic P&L is
    computed from `bars` (Root Defect 2 / the repair mission's pre-flight
    requirement) -- this is an integrity preflight, not a change to the
    already-accepted future-execution chronology.

    - CA_POLICY_ADJUSTED_DATA is satisfied only when
      price_adjustment_convention is one of the independently-verified
      adjusted conventions this system currently recognizes
      (_KNOWN_ADJUSTED_CONVENTIONS -- empty in production today). Refuses
      to trust an unproven "adjusted" claim.
    - CA_POLICY_FORBID_AFFECTED_PERIODS is satisfied only when a
      corporate_action_evidence_id is present AND every row in `bars`
      falls OUTSIDE every declared forbidden (symbol, period) window.
    - Anything else (including the diagnostic policy, or a missing/unknown
      policy) fails closed.
    """
    policy = manifest.get("corporate_action_policy")
    convention = manifest.get("price_adjustment_convention")

    if policy == CA_POLICY_ADJUSTED_DATA:
        if convention not in _KNOWN_ADJUSTED_CONVENTIONS:
            raise CorporateActionIntegrityError(
                f"Fail-closed: corporate_action_policy={CA_POLICY_ADJUSTED_DATA!r} requires a "
                "verified adjusted price convention, but price_adjustment_convention="
                f"{convention!r} is not one of the independently-verified adjusted conventions "
                "this system currently recognizes -- refusing to trust an unproven 'adjusted' claim"
            )
        return

    if policy == CA_POLICY_FORBID_AFFECTED_PERIODS:
        if not manifest.get("corporate_action_evidence_id"):
            raise CorporateActionIntegrityError(
                "Fail-closed: corporate_action_policy=forbid_affected_periods requires a "
                "corporate_action_evidence_id (content-addressed reference to the exclusion "
                "evidence) -- none was supplied"
            )
        violations = _find_forbidden_period_violations(bars, manifest.get("forbidden_periods") or [])
        if violations:
            raise CorporateActionIntegrityError(
                "Fail-closed: bars contain rows inside a declared corporate-action exclusion "
                f"period -- refusing to score the contaminated interval: {violations}"
            )
        return

    raise CorporateActionIntegrityError(
        f"Fail-closed: corporate_action_policy={policy!r} does not provide a valid, honest "
        f"corporate-action safety guarantee for this bars data (price_adjustment_convention="
        f"{convention!r}) -- refusing to compute economic P&L over data that may contain "
        "unadjusted split/dividend contamination"
    )


def _find_forbidden_period_violations(bars: pd.DataFrame, entries: Sequence[Dict[str, Any]]) -> List[Dict[str, Any]]:
    violations: List[Dict[str, Any]] = []
    if not entries or bars.empty:
        return violations
    symbols = bars["symbol"].astype(str)
    end_ts = pd.to_datetime(bars["end_ts"], utc=True)
    for entry in entries:
        symbol = str(entry["symbol"])
        start = pd.to_datetime(entry["start_ts"], utc=True)
        end = pd.to_datetime(entry["end_ts"], utc=True)
        mask = (symbols == symbol) & (end_ts >= start) & (end_ts <= end)
        if bool(mask.any()):
            violations.append(
                {
                    "symbol": symbol,
                    "start_ts": entry["start_ts"],
                    "end_ts": entry["end_ts"],
                    "violating_row_count": int(mask.sum()),
                }
            )
    return violations
