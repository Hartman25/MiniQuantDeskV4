"""
RUNTIME-OPPORTUNITY-ALLOCATION-01 Phase B — runtime-opportunity-set-v1 artifact.

A narrow, immutable artifact that carries the *only* source-proven score the
Bundle 5 daemon-side allocator is permitted to consume. Phase A audit
(docs/specs/runtime_opportunity_allocation_01a_current_truth_and_contract.md,
Q1/Q2) established that `scanner-candidate-v1.total_score` is dropped before
reaching the daemon's `LoadedWatchlistArtifact` — there is no score, and no
candidate-identity lineage, on the path the daemon currently trusts. This
module closes that gap without touching the existing watchlist/promotion
artifacts or gates.

Hard invariants:
- schema_version is always "runtime-opportunity-set-v1"
- mode is always "paper"; approved_for_live is always False
- approved_for_autonomous_paper is only True when every candidate binds to an
  approved watchlist-v2 symbol/strategy assignment
- score is a canonical decimal string (see CANONICAL_SCORE_RE) derived only
  from scanner-candidate-v1.total_score (score_source="scanner_total_score")
  — never synthesized from list order, symbol name, or target quantity
- No broker/OMS/execution/DB imports; no network imports; no orders placed
- Read-only from the daemon's perspective — this module only builds/validates
  the artifact; it does not consume it
"""
from __future__ import annotations

import hashlib
import json
import re
from dataclasses import dataclass, field
from datetime import datetime, timedelta, timezone
from pathlib import Path
from typing import Any, Optional

SCHEMA_VERSION = "runtime-opportunity-set-v1"

# Mirrors core-rs/crates/mqk-daemon/src/watchlist_intake.rs::MULTI_SYMBOL_HARD_CEILING
# and research-py/src/mqk_research/scanner/watchlist_promotion.py::MULTI_SYMBOL_HARD_CEILING.
MULTI_SYMBOL_HARD_CEILING = 5

SCORE_SOURCE_SCANNER_TOTAL_SCORE = "scanner_total_score"

# Canonical exact score representation: a non-negative decimal string with
# exactly 6 fractional digits, no sign, no exponent, no leading zeros beyond
# a single "0". Matches `f"{x:.6f}"` for any x in [0, 1e9). Score must also be
# strictly positive (see validation) — "0.000000" is syntactically canonical
# but semantically nonpositive and rejected separately.
CANONICAL_SCORE_RE = re.compile(r"^(0\.\d{6}|[1-9]\d*\.\d{6})$")

DEFAULT_TTL_SECONDS = 900  # 15 minutes

# Reason constants — stable strings, closed vocabulary.
REASON_SCHEMA_INVALID = "runtime_opportunity_schema_invalid"
REASON_MODE_NOT_PAPER = "runtime_opportunity_mode_not_paper"
REASON_LIVE_APPROVAL_FORBIDDEN = "runtime_opportunity_live_approval_forbidden"
REASON_STALE = "runtime_opportunity_stale"
REASON_WRONG_MARKET_DATE = "runtime_opportunity_wrong_market_date"
REASON_LINEAGE_MISSING = "runtime_opportunity_lineage_missing"
REASON_SYMBOL_BLANK_OR_DUPLICATE = "runtime_opportunity_symbol_blank_or_duplicate"
REASON_DUPLICATE_STRATEGY_ASSIGNMENT = "runtime_opportunity_duplicate_strategy_assignment"
REASON_SYMBOL_STRATEGY_MISMATCH = "runtime_opportunity_symbol_strategy_mismatch"
REASON_SCORE_MISSING = "runtime_opportunity_score_missing"
REASON_SCORE_NONCANONICAL = "runtime_opportunity_score_noncanonical"
REASON_SCORE_NONPOSITIVE = "runtime_opportunity_score_nonpositive"
REASON_SCORE_SOURCE_INVALID = "runtime_opportunity_score_source_invalid"
REASON_SYMBOL_COUNT_ABOVE_CEILING = "runtime_opportunity_symbol_count_above_ceiling"
REASON_CANDIDATE_NOT_PAPER_ELIGIBLE = "runtime_opportunity_candidate_not_paper_eligible"
REASON_CANDIDATE_LIVE_ELIGIBLE = "runtime_opportunity_candidate_live_eligible"
REASON_WATCHLIST_HASH_MISMATCH = "runtime_opportunity_watchlist_hash_mismatch"


def _utcnow_iso() -> str:
    return datetime.now(timezone.utc).isoformat()


def format_canonical_score(value: float) -> str:
    """Format a float as the canonical 6-fractional-digit decimal string."""
    return f"{float(value):.6f}"


def _stable_hash(payload: Any) -> str:
    """Deterministic SHA-256 hex digest of a JSON-canonicalized payload."""
    blob = json.dumps(payload, sort_keys=True, default=str, separators=(",", ":"))
    return hashlib.sha256(blob.encode("utf-8")).hexdigest()


def _canonical_watchlist_lineage_string(watchlist: dict[str, Any]) -> str:
    """
    Build a canonical, delimiter-based (not JSON) string over the
    identity-relevant watchlist fields.

    Deliberately NOT JSON: JSON map-key ordering and number formatting are
    library/feature-flag dependent (e.g. serde_json's `preserve_order`
    feature), which would make a byte-for-byte hash impossible to safely
    replicate on the Rust daemon side. This format is unambiguous in any
    language: fixed field order, sorted map entries, comma/colon delimiters.
    """
    schema_version = str(watchlist.get("schema_version") or "")
    generated_at_utc = str(watchlist.get("generated_at_utc") or "")
    max_symbols_to_trade = watchlist.get("max_symbols_to_trade")
    max_concurrent_positions = watchlist.get("max_concurrent_positions")
    symbols = [str(s) for s in (watchlist.get("symbols") or [])]
    strategy_assignments = dict(watchlist.get("strategy_assignments") or {})
    sa_str = ",".join(f"{k}:{v}" for k, v in sorted(strategy_assignments.items()))
    symbols_str = ",".join(symbols)
    msts = "" if max_symbols_to_trade is None else str(int(max_symbols_to_trade))
    mcp = "" if max_concurrent_positions is None else str(int(max_concurrent_positions))
    return (
        f"schema_version={schema_version}|"
        f"generated_at_utc={generated_at_utc}|"
        f"max_symbols_to_trade={msts}|"
        f"max_concurrent_positions={mcp}|"
        f"symbols={symbols_str}|"
        f"strategy_assignments={sa_str}"
    )


def watchlist_lineage_hash(watchlist: dict[str, Any]) -> str:
    """
    Deterministic hash of a watchlist-v2 artifact's identity-relevant content.

    Uses a canonical delimited string (see `_canonical_watchlist_lineage_string`)
    rather than JSON so the daemon's Rust-side validator can recompute this
    exact hash from the raw watchlist JSON.
    """
    blob = _canonical_watchlist_lineage_string(watchlist)
    return hashlib.sha256(blob.encode("utf-8")).hexdigest()


def candidate_lineage_hash(candidate_record: dict[str, Any]) -> str:
    """Deterministic hash of one scanner-candidate-v1 record's identity content."""
    return _stable_hash(
        {
            "schema_version": candidate_record.get("schema_version"),
            "scanner_id": candidate_record.get("scanner_id"),
            "symbol": candidate_record.get("symbol"),
            "generated_at_utc": candidate_record.get("generated_at_utc"),
            "latest_bar_ts": candidate_record.get("latest_bar_ts"),
            "total_score": candidate_record.get("total_score"),
        }
    )


# ---------------------------------------------------------------------------
# Builder
# ---------------------------------------------------------------------------

@dataclass
class RuntimeOpportunityBuildError(ValueError):
    """Raised when the builder cannot produce a valid artifact from its inputs."""
    reasons: list[str] = field(default_factory=list)

    def __str__(self) -> str:  # pragma: no cover - trivial
        return "; ".join(self.reasons) or "runtime opportunity build failed"


def build_runtime_opportunity_set(
    *,
    watchlist: dict[str, Any],
    candidate_records: list[dict[str, Any]],
    market_date: str,
    timeframe: str,
    scanner_id: str,
    scoring_config_hash: str,
    code_version: str = "runtime_opportunity_artifact_v1",
    ttl_seconds: int = DEFAULT_TTL_SECONDS,
    generated_at_utc: Optional[str] = None,
) -> dict[str, Any]:
    """
    Build a runtime-opportunity-set-v1 artifact from an approved watchlist-v2
    artifact and the scanner-candidate-v1 records for its assigned symbols.

    Raises RuntimeOpportunityBuildError (never emits a partial/best-effort
    artifact) if:
    - watchlist is not schema_version=="watchlist-v2"
    - watchlist.approved_for_autonomous_paper is not True
    - watchlist.approved_for_live is True
    - watchlist.symbols is empty or exceeds MULTI_SYMBOL_HARD_CEILING
    - any watchlist symbol has no matching candidate_records entry, or the
      matching record's symbol/schema does not agree, or its
      eligible_for_paper is not True, or eligible_for_live is not False
    - any two watchlist symbols would collide on strategy_id assignment
      inconsistently with the watchlist's own strategy_assignments (defense
      in depth; the watchlist promotion gate already enforces one strategy
      per symbol)

    approved_for_live is always False in the output. approved_for_autonomous_paper
    is always True in the output (the builder only runs after the caller has
    already confirmed watchlist approval and candidate presence — see raises
    above for every path that instead raises rather than emitting a
    downgraded artifact).
    """
    reasons: list[str] = []

    if watchlist.get("schema_version") != "watchlist-v2":
        reasons.append("source watchlist must be schema_version=='watchlist-v2'")
    if watchlist.get("mode") != "paper":
        reasons.append("source watchlist mode must be 'paper'")
    if watchlist.get("approved_for_live") is True:
        reasons.append("source watchlist has approved_for_live=true; refusing to build")
    if watchlist.get("approved_for_autonomous_paper") is not True:
        reasons.append("source watchlist is not approved_for_autonomous_paper")

    symbols: list[str] = list(watchlist.get("symbols") or [])
    if not symbols:
        reasons.append("source watchlist has no approved symbols")
    if len(symbols) > MULTI_SYMBOL_HARD_CEILING:
        reasons.append(
            f"source watchlist symbol count {len(symbols)} exceeds "
            f"MULTI_SYMBOL_HARD_CEILING={MULTI_SYMBOL_HARD_CEILING}"
        )

    strategy_assignments: dict[str, str] = dict(watchlist.get("strategy_assignments") or {})

    records_by_symbol: dict[str, dict[str, Any]] = {}
    for record in candidate_records:
        sym = record.get("symbol")
        if sym:
            records_by_symbol[sym] = record

    if reasons:
        raise RuntimeOpportunityBuildError(reasons=reasons)

    candidates: list[dict[str, Any]] = []
    for sym in symbols:
        strategy_id = strategy_assignments.get(sym)
        if not strategy_id:
            reasons.append(f"watchlist symbol '{sym}' has no strategy_assignments entry")
            continue
        record = records_by_symbol.get(sym)
        if record is None:
            reasons.append(f"no scanner-candidate-v1 record found for approved symbol '{sym}'")
            continue
        if record.get("schema_version") != "scanner-candidate-v1":
            reasons.append(f"candidate record for '{sym}' has wrong schema_version")
            continue
        if record.get("eligible_for_paper") is not True:
            reasons.append(f"candidate record for '{sym}' is not eligible_for_paper")
            continue
        if record.get("eligible_for_live") is not False:
            reasons.append(f"candidate record for '{sym}' has eligible_for_live != False")
            continue
        total_score = record.get("total_score")
        if not isinstance(total_score, (int, float)) or total_score <= 0:
            reasons.append(f"candidate record for '{sym}' has missing/nonpositive total_score")
            continue

        candidates.append(
            {
                "symbol": sym,
                "strategy_id": strategy_id,
                "score": format_canonical_score(total_score),
                "score_source": SCORE_SOURCE_SCANNER_TOTAL_SCORE,
                "candidate_artifact_id": candidate_lineage_hash(record),
                "candidate_generated_at_utc": record.get("generated_at_utc"),
                "eligible_for_paper": True,
                "eligible_for_live": False,
            }
        )

    if reasons:
        raise RuntimeOpportunityBuildError(reasons=reasons)

    gen_at = generated_at_utc or _utcnow_iso()
    gen_dt = datetime.fromisoformat(gen_at)
    expires_at = (gen_dt + timedelta(seconds=ttl_seconds)).isoformat()

    source_watchlist_hash = watchlist_lineage_hash(watchlist)
    source_candidate_artifact_hashes = sorted({c["candidate_artifact_id"] for c in candidates})

    artifact_id = _stable_hash(
        {
            "market_date": market_date,
            "source_watchlist_hash": source_watchlist_hash,
            "source_candidate_artifact_hashes": source_candidate_artifact_hashes,
        }
    )

    return {
        "schema_version": SCHEMA_VERSION,
        "artifact_id": artifact_id,
        "generated_at_utc": gen_at,
        "expires_at_utc": expires_at,
        "market_date": market_date,
        "mode": "paper",
        "approved_for_autonomous_paper": True,
        "approved_for_live": False,
        "source_watchlist_hash": source_watchlist_hash,
        "source_candidate_artifact_hashes": source_candidate_artifact_hashes,
        "scanner_lineage": {
            "scanner_id": scanner_id,
            "code_version": code_version,
            "scoring_config_hash": scoring_config_hash,
        },
        "timeframe": timeframe,
        "max_symbols_to_trade": watchlist.get("max_symbols_to_trade"),
        "max_concurrent_positions": watchlist.get("max_concurrent_positions"),
        "candidates": candidates,
    }


def write_runtime_opportunity_set(artifact: dict[str, Any], path: str) -> Path:
    """Write the artifact as JSON. Parent dirs created automatically."""
    out = Path(path)
    out.parent.mkdir(parents=True, exist_ok=True)
    out.write_text(json.dumps(artifact, indent=2, default=str), encoding="utf-8")
    return out


# ---------------------------------------------------------------------------
# Validator
# ---------------------------------------------------------------------------

@dataclass
class RuntimeOpportunityValidation:
    valid: bool
    failure_reasons: list[str] = field(default_factory=list)
    candidate_count: int = 0

    def to_dict(self) -> dict[str, Any]:
        return {
            "valid": self.valid,
            "failure_reasons": list(self.failure_reasons),
            "candidate_count": self.candidate_count,
        }


def validate_runtime_opportunity_set(
    artifact: dict[str, Any],
    *,
    watchlist: dict[str, Any],
    now_utc: datetime,
    market_date: str,
) -> RuntimeOpportunityValidation:
    """
    Validate a runtime-opportunity-set-v1 artifact against the approved
    watchlist-v2 artifact it claims to be bound to, plus freshness/market-date.

    Pure function: no filesystem, network, or DB access. Every rejection
    reason in the task's required list is covered. Multiple failures
    accumulate rather than short-circuiting on the first one, except where a
    missing top-level field makes further checks meaningless (e.g. a
    non-list `candidates`).
    """
    reasons: list[str] = []

    if artifact.get("schema_version") != SCHEMA_VERSION:
        reasons.append(REASON_SCHEMA_INVALID)
    if artifact.get("mode") != "paper":
        reasons.append(REASON_MODE_NOT_PAPER)
    if artifact.get("approved_for_live") is not False:
        reasons.append(REASON_LIVE_APPROVAL_FORBIDDEN)

    if artifact.get("market_date") != market_date:
        reasons.append(REASON_WRONG_MARKET_DATE)

    expires_at_raw = artifact.get("expires_at_utc")
    try:
        expires_at = datetime.fromisoformat(str(expires_at_raw))
        if expires_at.tzinfo is None:
            expires_at = expires_at.replace(tzinfo=timezone.utc)
        if now_utc > expires_at:
            reasons.append(REASON_STALE)
    except (TypeError, ValueError):
        reasons.append(REASON_STALE)

    source_watchlist_hash = artifact.get("source_watchlist_hash")
    scanner_lineage = artifact.get("scanner_lineage")
    if (
        not source_watchlist_hash
        or not isinstance(scanner_lineage, dict)
        or not scanner_lineage.get("scanner_id")
        or not scanner_lineage.get("code_version")
        or not artifact.get("source_candidate_artifact_hashes")
    ):
        reasons.append(REASON_LINEAGE_MISSING)
    elif source_watchlist_hash != watchlist_lineage_hash(watchlist):
        reasons.append(REASON_WATCHLIST_HASH_MISMATCH)

    watchlist_assignments: dict[str, str] = dict(watchlist.get("strategy_assignments") or {})
    watchlist_symbols = set(watchlist.get("symbols") or [])

    candidates = artifact.get("candidates")
    if not isinstance(candidates, list):
        reasons.append(REASON_SCORE_MISSING)
        return RuntimeOpportunityValidation(valid=False, failure_reasons=reasons, candidate_count=0)

    if len(candidates) > MULTI_SYMBOL_HARD_CEILING:
        reasons.append(REASON_SYMBOL_COUNT_ABOVE_CEILING)

    seen_symbols: set[str] = set()
    seen_symbol_strategy_pairs: set[tuple[str, str]] = set()
    for c in candidates:
        if not isinstance(c, dict):
            reasons.append(REASON_SYMBOL_BLANK_OR_DUPLICATE)
            continue
        sym = c.get("symbol")
        strategy_id = c.get("strategy_id")

        if not isinstance(sym, str) or not sym.strip() or sym in seen_symbols:
            reasons.append(REASON_SYMBOL_BLANK_OR_DUPLICATE)
        else:
            seen_symbols.add(sym)

        if isinstance(sym, str) and isinstance(strategy_id, str):
            pair = (sym, strategy_id)
            if pair in seen_symbol_strategy_pairs:
                reasons.append(REASON_DUPLICATE_STRATEGY_ASSIGNMENT)
            seen_symbol_strategy_pairs.add(pair)

        if isinstance(sym, str) and (
            sym not in watchlist_symbols or watchlist_assignments.get(sym) != strategy_id
        ):
            reasons.append(REASON_SYMBOL_STRATEGY_MISMATCH)

        score = c.get("score")
        if score is None or not isinstance(score, str):
            reasons.append(REASON_SCORE_MISSING)
        elif not CANONICAL_SCORE_RE.match(score):
            reasons.append(REASON_SCORE_NONCANONICAL)
        elif float(score) <= 0.0:
            reasons.append(REASON_SCORE_NONPOSITIVE)

        if c.get("score_source") != SCORE_SOURCE_SCANNER_TOTAL_SCORE:
            reasons.append(REASON_SCORE_SOURCE_INVALID)

        if c.get("eligible_for_paper") is not True:
            reasons.append(REASON_CANDIDATE_NOT_PAPER_ELIGIBLE)
        if c.get("eligible_for_live") is not False:
            reasons.append(REASON_CANDIDATE_LIVE_ELIGIBLE)

    # Deduplicate reasons while preserving first-seen order for readability.
    deduped: list[str] = []
    seen_reasons: set[str] = set()
    for r in reasons:
        if r not in seen_reasons:
            seen_reasons.add(r)
            deduped.append(r)

    return RuntimeOpportunityValidation(
        valid=len(deduped) == 0,
        failure_reasons=deduped,
        candidate_count=len(candidates),
    )
