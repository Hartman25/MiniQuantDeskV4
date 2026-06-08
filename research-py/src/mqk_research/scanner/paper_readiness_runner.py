"""
PAPER-READINESS-RUNNER-01 — Operational paper-readiness pipeline runner.

Composes the already-proven research-layer artifact chain — symbol-inputs
runner -> risk simulation -> premarket revalidation -> watchlist promotion —
into a single toggle-controlled, fail-closed operational entry point that
produces a promoted paper watchlist artifact and a paper-readiness report.

This module is a pure *composition/orchestration* layer. It does not
re-implement gate logic: every evaluation is delegated to the existing,
independently-proven evaluators (`run_symbol_inputs_producer`,
`evaluate_watchlist_risk`, `evaluate_premarket_watchlist`,
`evaluate_watchlist_promotion`). It only resolves local artifact paths,
sequences the calls, aggregates the honest results, and writes the
conventional output artifacts.

Hard invariants:
- `approved_for_live` is ALWAYS False — at config, result, and artifact level
- `live_locked` is ALWAYS True; `daemon_enforcement_executed` is ALWAYS False;
  `paper_handoff_executed` is ALWAYS False
- `live_handoff_enabled=True` is a hard block — never executes the chain
- A forged `approved_for_live=True` on an input watchlist never propagates;
  the runner reports `paper_readiness_forged_live_approval_forbidden` instead
- `approved_for_autonomous_paper=True` is only ever written when every
  upstream gate (strategy-fit recommendation, risk simulation, premarket
  revalidation, operator review) genuinely passes — derived honestly through
  `evaluate_watchlist_promotion`, never hand-set
- No imports from broker adapters, OMS tables, execution orchestrator, or DB
- No network imports (requests, urllib, http.client, aiohttp, psycopg, sqlalchemy)
- No daemon/runtime imports
- No subprocess
- Local/offline only — every input is a caller-local artifact path; this
  module does not fetch market data, call any broker/API, or contact a daemon
- Missing/malformed/disabled inputs fail closed with explicit, stable reason
  strings — never silent omission, never a fabricated healthy record

Toggle model (all default OFF / fail-closed):
- `live_handoff_enabled`        — hard-forbidden; True blocks unconditionally
- `paper_readiness_enabled`     — must be True to run the chain at all
- `symbol_inputs_enabled`       — must be True to run the symbol-inputs stage
- `watchlist_promotion_enabled` — must be True to run the promotion stage
- `paper_handoff_enabled`       — does NOT wire daemon enforcement; only marks
                                   the report `paper_handoff_requested=True`
                                   ("requested but not executed")
- `operator_review_approved`    — must be True (alongside every other gate) for
                                   `approved_for_autonomous_paper=True`
- `scanner_pipeline_enabled` / `backtest_pipeline_enabled` are surfaced in the
  report for operator context only — this runner consumes their pre-existing
  output artifacts and never invokes either pipeline itself.

Status vocabulary (`PaperReadinessResult.status`):
- "blocked"                  — chain did not complete; nothing (or only a
                               partial set of artifacts) was produced; see
                               `reasons` for the exact cause(s)
- "partial"                  — chain completed but the symbol-inputs stage
                               reported missing/malformed bars for one or more
                               symbols (status == "partial"); promotion still
                               evaluated honestly on what was actually produced
- "ready_for_operator_review"— every gate except operator sign-off passed;
                               `approved_for_autonomous_paper` stays False
                               until the operator explicitly approves
- "ready_for_paper_handoff"  — every gate (including operator review) passed;
                               `approved_for_autonomous_paper=True`,
                               `approved_for_live` stays False; daemon
                               handoff is requested-but-not-executed only if
                               `paper_handoff_enabled=True`

Fail-closed cases represented (never disappear silently):
- live_handoff_enabled=True            -> blocked, "paper_readiness_live_handoff_forbidden"
- mode != "paper"                      -> blocked, "paper_readiness_mode_not_paper"
- paper_readiness_enabled=False        -> blocked, "paper_readiness_disabled"
- no watchlist_path configured         -> blocked, "paper_readiness_watchlist_path_missing"
- watchlist fails to load/parse        -> blocked, "paper_readiness_watchlist_load_failed"
- watchlist has no candidate symbols   -> blocked, "paper_readiness_no_ranked_candidates"
- bars root missing/not a directory    -> blocked, "paper_readiness_bars_root_missing"
- strategy-fit dir missing/not a dir   -> blocked, "paper_readiness_strategy_fit_dir_missing"
- no strategy-fit artifact for top sym -> blocked, "paper_readiness_strategy_fit_missing"
- symbol_inputs_enabled=False          -> blocked, "paper_readiness_symbol_inputs_disabled"
- watchlist_promotion_enabled=False    -> blocked, "paper_readiness_watchlist_promotion_disabled"
- symbol-inputs runner blocked         -> blocked, "paper_readiness_symbol_inputs_blocked"
                                           (+ underlying runner failure reasons)
- symbol-inputs artifact unreadable    -> blocked, "paper_readiness_symbol_inputs_artifact_invalid"
- symbol-inputs runner partial         -> chain continues; "paper_readiness_symbol_inputs_partial"
                                           recorded; status may end as "partial"
- risk simulation fails                -> chain continues (honest aggregation);
                                           "paper_readiness_risk_simulation_failed" recorded
- premarket revalidation fails         -> chain continues (honest aggregation);
                                           "paper_readiness_premarket_revalidation_failed" recorded
- operator review missing              -> final approval blocked; "operator_review_required"
                                           surfaces via the real promotion decision
- forged approved_for_live=True        -> does not block by itself; reported via
                                           "paper_readiness_forged_live_approval_forbidden"
                                           (the real promotion gate independently
                                           fails closed on it as well)
- output artifact cannot be written    -> "paper_readiness_output_write_failed:<artifact>"
                                           recorded; in-memory result still returned

Notes:
- `strategy_fit_dir` is scanned for `*.json` files in sorted order; each valid
  `strategy-fit-v1` artifact is keyed by its `symbol` field (last sorted
  filename wins per symbol — deterministic, no reliance on filesystem mtime).
  This mirrors the existing pattern of resolving caller-local artifacts by
  content rather than inventing a new naming convention.
- The symbol list passed to the symbol-inputs runner is the watchlist's own
  `symbols` list — the same identity the promotion/premarket/risk evaluators
  consume — so every stage evaluates the same candidate set.
"""
from __future__ import annotations

import argparse
import dataclasses
import json
import sys
from dataclasses import dataclass, field
from datetime import datetime
from pathlib import Path
from typing import Any, Optional

from mqk_research.scanner.premarket_revalidation import (
    PremarketRevalidationConfig,
    evaluate_premarket_watchlist,
    write_premarket_revalidation_artifact,
)
from mqk_research.scanner.risk_simulation import (
    RiskSimulationConfig,
    evaluate_watchlist_risk,
)
from mqk_research.scanner.symbol_inputs import (
    extract_symbol_inputs_map,
    load_symbol_inputs_artifact,
)
from mqk_research.scanner.symbol_inputs_runner import (
    SymbolInputsRunnerConfig,
    run_symbol_inputs_producer,
)
from mqk_research.scanner.watchlist_promotion import (
    REASON_OPERATOR_REVIEW_REQUIRED as PROMOTION_REASON_OPERATOR_REVIEW_REQUIRED,
    WatchlistPromotionConfig,
    apply_watchlist_promotion,
    evaluate_watchlist_promotion,
    write_promoted_watchlist,
)

# ---------------------------------------------------------------------------
# Schema / status constants
# ---------------------------------------------------------------------------

SCHEMA_VERSION = "paper-readiness-v1"

STATUS_BLOCKED = "blocked"
STATUS_PARTIAL = "partial"
STATUS_READY_FOR_OPERATOR_REVIEW = "ready_for_operator_review"
STATUS_READY_FOR_PAPER_HANDOFF = "ready_for_paper_handoff"

# ---------------------------------------------------------------------------
# Stable reason constants
# ---------------------------------------------------------------------------

REASON_LIVE_HANDOFF_FORBIDDEN = "paper_readiness_live_handoff_forbidden"
REASON_MODE_NOT_PAPER = "paper_readiness_mode_not_paper"
REASON_PAPER_READINESS_DISABLED = "paper_readiness_disabled"
REASON_SYMBOL_INPUTS_DISABLED = "paper_readiness_symbol_inputs_disabled"
REASON_WATCHLIST_PROMOTION_DISABLED = "paper_readiness_watchlist_promotion_disabled"
REASON_WATCHLIST_PATH_MISSING = "paper_readiness_watchlist_path_missing"
REASON_WATCHLIST_LOAD_FAILED = "paper_readiness_watchlist_load_failed"
REASON_NO_RANKED_CANDIDATES = "paper_readiness_no_ranked_candidates"
REASON_BARS_ROOT_MISSING = "paper_readiness_bars_root_missing"
REASON_STRATEGY_FIT_DIR_MISSING = "paper_readiness_strategy_fit_dir_missing"
REASON_STRATEGY_FIT_MISSING = "paper_readiness_strategy_fit_missing"
REASON_SYMBOL_INPUTS_BLOCKED = "paper_readiness_symbol_inputs_blocked"
REASON_SYMBOL_INPUTS_PARTIAL = "paper_readiness_symbol_inputs_partial"
REASON_SYMBOL_INPUTS_ARTIFACT_INVALID = "paper_readiness_symbol_inputs_artifact_invalid"
REASON_RISK_SIMULATION_FAILED = "paper_readiness_risk_simulation_failed"
REASON_PREMARKET_REVALIDATION_FAILED = "paper_readiness_premarket_revalidation_failed"
REASON_FORGED_LIVE_APPROVAL_FORBIDDEN = "paper_readiness_forged_live_approval_forbidden"
REASON_OUTPUT_WRITE_FAILED = "paper_readiness_output_write_failed"
REASON_CONFIG_LOAD_FAILED = "paper_readiness_config_load_failed"

ALL_READINESS_REASONS = (
    REASON_LIVE_HANDOFF_FORBIDDEN,
    REASON_MODE_NOT_PAPER,
    REASON_PAPER_READINESS_DISABLED,
    REASON_SYMBOL_INPUTS_DISABLED,
    REASON_WATCHLIST_PROMOTION_DISABLED,
    REASON_WATCHLIST_PATH_MISSING,
    REASON_WATCHLIST_LOAD_FAILED,
    REASON_NO_RANKED_CANDIDATES,
    REASON_BARS_ROOT_MISSING,
    REASON_STRATEGY_FIT_DIR_MISSING,
    REASON_STRATEGY_FIT_MISSING,
    REASON_SYMBOL_INPUTS_BLOCKED,
    REASON_SYMBOL_INPUTS_PARTIAL,
    REASON_SYMBOL_INPUTS_ARTIFACT_INVALID,
    REASON_RISK_SIMULATION_FAILED,
    REASON_PREMARKET_REVALIDATION_FAILED,
    REASON_FORGED_LIVE_APPROVAL_FORBIDDEN,
    REASON_OUTPUT_WRITE_FAILED,
    REASON_CONFIG_LOAD_FAILED,
)

_STRATEGY_FIT_SCHEMA = "strategy-fit-v1"

# Config keys that are never accepted from a serialized JSON config — runtime
# objects (datetime / sub-config instances) cannot round-trip through JSON,
# and `approved_for_live` is a hard invariant the loader must never honor.
_CONFIG_RUNTIME_ONLY_FIELDS = (
    "now_utc",
    "generated_at_utc",
    "risk_simulation_config",
    "premarket_config",
    "approved_for_live",
)


# ---------------------------------------------------------------------------
# Config
# ---------------------------------------------------------------------------

@dataclass
class PaperReadinessConfig:
    """
    Configuration for the paper-readiness operational pipeline runner.

    All `*_enabled` toggles default to False (fail-closed). The chain only
    runs when `paper_readiness_enabled=True`, and `live_handoff_enabled=True`
    is an unconditional hard block regardless of any other setting.

    `scanner_pipeline_enabled` / `backtest_pipeline_enabled` are accepted and
    surfaced in the readiness report for operator context only — this runner
    composes pre-existing watchlist / strategy-fit artifacts and never invokes
    either upstream pipeline itself.

    `approved_for_live` is a hard invariant — always forced to False.
    """
    trade_date: str
    scanner_pipeline_enabled: bool = False
    backtest_pipeline_enabled: bool = False
    symbol_inputs_enabled: bool = False
    watchlist_promotion_enabled: bool = False
    paper_readiness_enabled: bool = False
    paper_handoff_enabled: bool = False
    live_handoff_enabled: bool = False
    operator_review_approved: bool = False
    mode: str = "paper"
    bars_root: Optional[str] = None
    watchlist_path: Optional[str] = None
    strategy_fit_dir: Optional[str] = None
    liquidity_root: Optional[str] = None
    timeframe: str = "5m"
    symbol_inputs_output_path: str = "exports/paper_readiness/symbol_inputs.json"
    premarket_output_path: str = "exports/paper_readiness/premarket_revalidation.json"
    promoted_watchlist_output_path: str = "exports/paper_readiness/promoted_watchlist.json"
    readiness_report_output_path: str = "exports/paper_readiness/readiness_report.json"
    risk_simulation_config: Optional[RiskSimulationConfig] = None
    premarket_config: Optional[PremarketRevalidationConfig] = None
    generated_at_utc: Optional[str] = None
    now_utc: Optional[datetime] = None
    approved_for_live: bool = False

    def __post_init__(self) -> None:
        # Hard invariant: live approval is never permitted through config.
        object.__setattr__(self, "approved_for_live", False)


# ---------------------------------------------------------------------------
# Result
# ---------------------------------------------------------------------------

@dataclass
class PaperReadinessResult:
    """
    Structured result of a `run_paper_readiness_pipeline` call.

    `status` is one of `STATUS_BLOCKED` / `STATUS_PARTIAL` /
    `STATUS_READY_FOR_OPERATOR_REVIEW` / `STATUS_READY_FOR_PAPER_HANDOFF`.

    `approved_for_live`, `live_locked`, and `daemon_enforcement_executed` are
    hard invariants — always False / True / False respectively, regardless of
    any upstream input.
    """
    status: str = STATUS_BLOCKED
    reasons: list[str] = field(default_factory=list)
    top_symbol: Optional[str] = None
    symbol_inputs_status: Optional[str] = None
    risk_simulation_passed: Optional[bool] = None
    premarket_revalidation_passed: Optional[bool] = None
    promotion_passed: Optional[bool] = None
    approved_for_autonomous_paper: bool = False
    approved_for_live: bool = False
    live_locked: bool = True
    daemon_enforcement_executed: bool = False
    paper_handoff_requested: bool = False
    paper_handoff_executed: bool = False
    upstream_pipeline_toggles: dict[str, bool] = field(default_factory=dict)
    artifacts_read: dict[str, Optional[str]] = field(default_factory=dict)
    artifacts_written: dict[str, Optional[str]] = field(default_factory=dict)
    notes: str = ""

    def __post_init__(self) -> None:
        # Hard invariants — never overrideable by caller or upstream input.
        object.__setattr__(self, "approved_for_live", False)
        object.__setattr__(self, "live_locked", True)
        object.__setattr__(self, "daemon_enforcement_executed", False)
        object.__setattr__(self, "paper_handoff_executed", False)

    def to_dict(self) -> dict[str, Any]:
        return {
            "schema_version": SCHEMA_VERSION,
            "status": self.status,
            "reasons": list(self.reasons),
            "top_symbol": self.top_symbol,
            "symbol_inputs_status": self.symbol_inputs_status,
            "risk_simulation_passed": self.risk_simulation_passed,
            "premarket_revalidation_passed": self.premarket_revalidation_passed,
            "promotion_passed": self.promotion_passed,
            "approved_for_autonomous_paper": self.approved_for_autonomous_paper,
            "approved_for_live": False,
            "live_locked": True,
            "daemon_enforcement_executed": False,
            "paper_handoff_requested": self.paper_handoff_requested,
            "paper_handoff_executed": False,
            "upstream_pipeline_toggles": dict(self.upstream_pipeline_toggles),
            "artifacts_read": dict(self.artifacts_read),
            "artifacts_written": dict(self.artifacts_written),
            "notes": self.notes,
        }


# ---------------------------------------------------------------------------
# Pure helpers
# ---------------------------------------------------------------------------

def _dedupe_preserve_order(items: list[str]) -> list[str]:
    seen: set[str] = set()
    out: list[str] = []
    for item in items:
        if item not in seen:
            seen.add(item)
            out.append(item)
    return out


def _load_watchlist_artifact(path: str) -> tuple[Optional[dict[str, Any]], bool, Optional[str]]:
    """
    Load a watchlist-shaped JSON artifact as a full dict (not just a symbol list
    — the promotion/risk/premarket evaluators all need the complete artifact).

    Returns `(watchlist, forged_live_approval, error_reason)`:
    - On success: `(watchlist_dict, forged_live_approval, None)`.
    - On failure (missing file, invalid JSON, not an object):
      `(None, False, REASON_WATCHLIST_LOAD_FAILED)`.

    `forged_live_approval` is True iff the artifact carries
    `approved_for_live: true`. This loader does not raise — it fails closed via
    the reason, mirroring `symbol_inputs_runner.load_symbols_from_watchlist`.
    """
    try:
        text = Path(path).read_text(encoding="utf-8")
        data = json.loads(text)
    except (OSError, ValueError):
        return None, False, REASON_WATCHLIST_LOAD_FAILED

    if not isinstance(data, dict):
        return None, False, REASON_WATCHLIST_LOAD_FAILED

    forged_live_approval = data.get("approved_for_live") is True
    return data, forged_live_approval, None


def _load_strategy_fit_artifacts(
    strategy_fit_dir: Optional[str],
) -> tuple[dict[str, dict[str, Any]], Optional[str]]:
    """
    Scan a directory for `strategy-fit-v1` JSON artifacts and build a
    `{symbol: artifact}` map.

    Returns `(fit_map, error_reason)`:
    - missing / not-a-directory `strategy_fit_dir` -> `({}, REASON_STRATEGY_FIT_DIR_MISSING)`
    - directory exists -> `(fit_map, None)`, where `fit_map` may be empty if no
      file in the directory is a valid `strategy-fit-v1` artifact (the caller
      reports the per-symbol absence as `REASON_STRATEGY_FIT_MISSING`; an empty
      directory is not itself a load failure — never fabricate a fit record).

    Deterministic: files are scanned in sorted filename order; for a given
    symbol, the lexicographically-last valid artifact wins. No reliance on
    filesystem mtime. Malformed/non-matching files are skipped, never raised.
    """
    if not strategy_fit_dir:
        return {}, REASON_STRATEGY_FIT_DIR_MISSING
    root = Path(strategy_fit_dir)
    if not root.is_dir():
        return {}, REASON_STRATEGY_FIT_DIR_MISSING

    fit_map: dict[str, dict[str, Any]] = {}
    for candidate in sorted(root.glob("*.json")):
        try:
            data = json.loads(candidate.read_text(encoding="utf-8"))
        except (OSError, ValueError):
            continue
        if not isinstance(data, dict):
            continue
        if data.get("schema_version") != _STRATEGY_FIT_SCHEMA:
            continue
        symbol = data.get("symbol")
        if isinstance(symbol, str) and symbol:
            fit_map[symbol] = data
    return fit_map, None


# ---------------------------------------------------------------------------
# Writer
# ---------------------------------------------------------------------------

def write_paper_readiness_report(result: PaperReadinessResult, path: str) -> Path:
    """Write a paper-readiness report as JSON. Parent dirs created automatically."""
    out = Path(path)
    out.parent.mkdir(parents=True, exist_ok=True)
    out.write_text(json.dumps(result.to_dict(), indent=2, sort_keys=False, default=str), encoding="utf-8")
    return out


# ---------------------------------------------------------------------------
# Runner entry point
# ---------------------------------------------------------------------------

def _blocked(
    config: PaperReadinessConfig,
    reasons: list[str],
    artifacts_read: dict[str, Optional[str]],
    artifacts_written: dict[str, Optional[str]],
    *,
    notes: str,
    top_symbol: Optional[str] = None,
    symbol_inputs_status: Optional[str] = None,
) -> PaperReadinessResult:
    return PaperReadinessResult(
        status=STATUS_BLOCKED,
        reasons=_dedupe_preserve_order(reasons),
        top_symbol=top_symbol,
        symbol_inputs_status=symbol_inputs_status,
        approved_for_autonomous_paper=False,
        paper_handoff_requested=bool(config.paper_handoff_enabled),
        upstream_pipeline_toggles={
            "scanner_pipeline_enabled": config.scanner_pipeline_enabled,
            "backtest_pipeline_enabled": config.backtest_pipeline_enabled,
        },
        artifacts_read=dict(artifacts_read),
        artifacts_written=dict(artifacts_written),
        notes=notes,
    )


def run_paper_readiness_pipeline(config: PaperReadinessConfig) -> PaperReadinessResult:
    """
    Compose the proven research-layer artifact chain into one fail-closed,
    toggle-controlled paper-readiness run.

    Pure-local, deterministic given `now_utc`/`generated_at_utc`. Always
    returns a structured result; never raises on bad/missing/disabled input —
    every failure mode is an explicit reason string and an honest `status`.

    Never calls daemon, broker, DB, or network. Never submits orders or
    injects strategy signals. `approved_for_live` is always False.
    """
    reasons: list[str] = []
    artifacts_read: dict[str, Optional[str]] = {
        "watchlist_path": config.watchlist_path,
        "bars_root": config.bars_root,
        "strategy_fit_dir": config.strategy_fit_dir,
        "liquidity_root": config.liquidity_root,
    }
    artifacts_written: dict[str, Optional[str]] = {
        "symbol_inputs_output_path": None,
        "premarket_output_path": None,
        "promoted_watchlist_output_path": None,
        "readiness_report_output_path": None,
    }
    upstream_toggles = {
        "scanner_pipeline_enabled": config.scanner_pipeline_enabled,
        "backtest_pipeline_enabled": config.backtest_pipeline_enabled,
    }

    # --- Hard live lock — checked first, unconditionally ---------------------
    if config.live_handoff_enabled:
        return _blocked(
            config, [REASON_LIVE_HANDOFF_FORBIDDEN], artifacts_read, artifacts_written,
            notes="blocked: live_handoff_enabled is forbidden — live trading remains hard-disabled",
        )

    # --- Mode lock ------------------------------------------------------------
    if config.mode != "paper":
        return _blocked(
            config, [REASON_MODE_NOT_PAPER], artifacts_read, artifacts_written,
            notes=f"blocked: mode={config.mode!r} is not 'paper'",
        )

    # --- Paper readiness toggle ------------------------------------------------
    if not config.paper_readiness_enabled:
        return _blocked(
            config, [REASON_PAPER_READINESS_DISABLED], artifacts_read, artifacts_written,
            notes="blocked: paper_readiness_enabled is False",
        )

    # --- Resolve watchlist ------------------------------------------------------
    if not config.watchlist_path:
        return _blocked(
            config, [REASON_WATCHLIST_PATH_MISSING], artifacts_read, artifacts_written,
            notes="blocked: no watchlist_path configured",
        )

    watchlist, forged_live, load_error = _load_watchlist_artifact(config.watchlist_path)
    if load_error is not None:
        return _blocked(
            config, [load_error], artifacts_read, artifacts_written,
            notes=f"blocked: {load_error}",
        )
    if forged_live:
        reasons.append(REASON_FORGED_LIVE_APPROVAL_FORBIDDEN)

    symbols: list[str] = [s for s in (watchlist.get("symbols") or []) if isinstance(s, str) and s]
    top_symbol: Optional[str] = symbols[0] if symbols else None

    if not symbols:
        return _blocked(
            config, reasons + [REASON_NO_RANKED_CANDIDATES], artifacts_read, artifacts_written,
            notes="blocked: watchlist has no ranked candidate symbols",
        )

    # --- Validate bars root ------------------------------------------------------
    if not config.bars_root or not Path(config.bars_root).is_dir():
        return _blocked(
            config, reasons + [REASON_BARS_ROOT_MISSING], artifacts_read, artifacts_written,
            notes="blocked: bars_root missing or not a directory", top_symbol=top_symbol,
        )

    # --- Resolve strategy-fit artifacts -------------------------------------------
    fit_artifacts, fit_dir_error = _load_strategy_fit_artifacts(config.strategy_fit_dir)
    if fit_dir_error is not None:
        return _blocked(
            config, reasons + [fit_dir_error], artifacts_read, artifacts_written,
            notes=f"blocked: {fit_dir_error}", top_symbol=top_symbol,
        )
    if top_symbol not in fit_artifacts:
        return _blocked(
            config, reasons + [REASON_STRATEGY_FIT_MISSING], artifacts_read, artifacts_written,
            notes="blocked: no strategy-fit-v1 artifact found for the top-ranked symbol",
            top_symbol=top_symbol,
        )

    # --- Stage toggles for the remaining chain ------------------------------------
    if not config.symbol_inputs_enabled:
        return _blocked(
            config, reasons + [REASON_SYMBOL_INPUTS_DISABLED], artifacts_read, artifacts_written,
            notes="blocked: symbol_inputs_enabled is False", top_symbol=top_symbol,
        )
    if not config.watchlist_promotion_enabled:
        return _blocked(
            config, reasons + [REASON_WATCHLIST_PROMOTION_DISABLED], artifacts_read, artifacts_written,
            notes="blocked: watchlist_promotion_enabled is False", top_symbol=top_symbol,
        )

    # --- Run symbol-inputs producer (real, local-file based) -----------------------
    symbol_inputs_config = SymbolInputsRunnerConfig(
        trade_date=config.trade_date,
        output_path=config.symbol_inputs_output_path,
        bars_root=config.bars_root,
        timeframe=config.timeframe,
        symbols=list(symbols),
        liquidity_root=config.liquidity_root,
        source="paper_readiness_runner",
        generated_at_utc=config.generated_at_utc,
        now_utc=config.now_utc,
    )
    symbol_inputs_result = run_symbol_inputs_producer(symbol_inputs_config)
    symbol_inputs_status = symbol_inputs_result.status

    if symbol_inputs_status == "blocked":
        return _blocked(
            config,
            reasons + [REASON_SYMBOL_INPUTS_BLOCKED] + list(symbol_inputs_result.failure_reasons),
            artifacts_read, artifacts_written,
            notes="blocked: symbol_inputs_runner could not produce an artifact",
            top_symbol=top_symbol,
            symbol_inputs_status=symbol_inputs_status,
        )

    artifacts_written["symbol_inputs_output_path"] = symbol_inputs_result.output_path
    if symbol_inputs_status == "partial":
        reasons.append(REASON_SYMBOL_INPUTS_PARTIAL)

    try:
        symbol_inputs_artifact = load_symbol_inputs_artifact(symbol_inputs_result.output_path)
    except (OSError, ValueError):
        return _blocked(
            config,
            reasons + [REASON_SYMBOL_INPUTS_ARTIFACT_INVALID],
            artifacts_read, artifacts_written,
            notes="blocked: symbol-inputs artifact failed validation on read-back",
            top_symbol=top_symbol,
            symbol_inputs_status=symbol_inputs_status,
        )
    symbol_inputs_map = extract_symbol_inputs_map(symbol_inputs_artifact)

    # --- Risk simulation (real evaluator; chain continues to aggregate honestly) ---
    risk_result = evaluate_watchlist_risk(
        watchlist, fit_artifacts, config.risk_simulation_config or RiskSimulationConfig()
    )
    if not risk_result.passed:
        reasons.append(REASON_RISK_SIMULATION_FAILED)

    # --- Premarket revalidation (real evaluator) ------------------------------------
    premarket_result = evaluate_premarket_watchlist(
        watchlist,
        symbol_inputs_map,
        config.premarket_config or PremarketRevalidationConfig(),
        reference_utc=config.now_utc,
    )
    try:
        premarket_path = write_premarket_revalidation_artifact(premarket_result, config.premarket_output_path)
        artifacts_written["premarket_output_path"] = str(premarket_path)
    except OSError:
        reasons.append(f"{REASON_OUTPUT_WRITE_FAILED}:premarket_output_path")
    if not premarket_result.passed:
        reasons.append(REASON_PREMARKET_REVALIDATION_FAILED)

    # --- Watchlist promotion (real evaluator — the single source of truth for
    # approved_for_autonomous_paper / approved_for_live) ------------------------------
    promotion_config = WatchlistPromotionConfig(
        operator_review_approved=config.operator_review_approved,
        risk_simulation_passed=risk_result.passed,
        premarket_revalidation_required=not premarket_result.passed,
    )
    decision = evaluate_watchlist_promotion(
        watchlist,
        fit_artifacts,
        promotion_config,
        risk_simulation_result=risk_result.to_dict(),
        premarket_revalidation_result=premarket_result.to_dict(),
    )
    promoted = apply_watchlist_promotion(watchlist, decision, promotion_config)
    try:
        promoted_path = write_promoted_watchlist(promoted, config.promoted_watchlist_output_path)
        artifacts_written["promoted_watchlist_output_path"] = str(promoted_path)
    except OSError:
        reasons.append(f"{REASON_OUTPUT_WRITE_FAILED}:promoted_watchlist_output_path")

    reasons = _dedupe_preserve_order(reasons + list(decision.failure_reasons))

    # --- Status classification — derived honestly from the real decision -------------
    only_operator_review_missing = decision.failure_reasons == [PROMOTION_REASON_OPERATOR_REVIEW_REQUIRED]
    if decision.passed:
        status = STATUS_READY_FOR_PAPER_HANDOFF
        notes = "ready_for_paper_handoff: all paper gates passed and operator review approved"
    elif only_operator_review_missing:
        status = STATUS_READY_FOR_OPERATOR_REVIEW
        notes = "ready_for_operator_review: all paper gates passed; awaiting explicit operator sign-off"
    elif symbol_inputs_status == "partial":
        status = STATUS_PARTIAL
        notes = "partial: chain completed but symbol-inputs reported missing/malformed bars; " + decision.notes
    else:
        status = STATUS_BLOCKED
        notes = "blocked: " + decision.notes

    paper_handoff_requested = bool(config.paper_handoff_enabled)
    if paper_handoff_requested:
        notes += " | paper handoff requested but not executed (daemon enforcement not wired)"

    artifacts_written["readiness_report_output_path"] = config.readiness_report_output_path
    result = PaperReadinessResult(
        status=status,
        reasons=reasons,
        top_symbol=top_symbol,
        symbol_inputs_status=symbol_inputs_status,
        risk_simulation_passed=risk_result.passed,
        premarket_revalidation_passed=premarket_result.passed,
        promotion_passed=decision.passed,
        approved_for_autonomous_paper=decision.approved_for_autonomous_paper,
        paper_handoff_requested=paper_handoff_requested,
        upstream_pipeline_toggles=upstream_toggles,
        artifacts_read=artifacts_read,
        artifacts_written=artifacts_written,
        notes=notes,
    )

    try:
        write_paper_readiness_report(result, config.readiness_report_output_path)
    except OSError:
        artifacts_written["readiness_report_output_path"] = None
        result.reasons = _dedupe_preserve_order(list(result.reasons) + [f"{REASON_OUTPUT_WRITE_FAILED}:readiness_report_output_path"])
        result.notes = f"{notes} (readiness report write failed)"

    return result


# ---------------------------------------------------------------------------
# CLI entry point
# ---------------------------------------------------------------------------

def load_config_from_json(path: str) -> PaperReadinessConfig:
    """
    Load a `PaperReadinessConfig` from a JSON file.

    Only declared `PaperReadinessConfig` fields are honored; runtime-only
    fields (`now_utc`, `generated_at_utc`, sub-config instances,
    `approved_for_live`) cannot round-trip through JSON and are ignored —
    `approved_for_live` is forced False by the config's own `__post_init__`
    regardless. Raises `ValueError` on missing/malformed config or a missing
    required `trade_date` field; never raises on the *pipeline* inputs (those
    fail closed through `run_paper_readiness_pipeline`).
    """
    try:
        text = Path(path).read_text(encoding="utf-8")
        data = json.loads(text)
    except OSError as exc:
        raise ValueError(f"{REASON_CONFIG_LOAD_FAILED}: cannot read {path!r}: {exc}") from exc
    except ValueError as exc:
        raise ValueError(f"{REASON_CONFIG_LOAD_FAILED}: invalid JSON in {path!r}: {exc}") from exc

    if not isinstance(data, dict):
        raise ValueError(f"{REASON_CONFIG_LOAD_FAILED}: {path!r} is not a JSON object")

    known_fields = {f.name for f in dataclasses.fields(PaperReadinessConfig)}
    kwargs = {
        k: v for k, v in data.items()
        if k in known_fields and k not in _CONFIG_RUNTIME_ONLY_FIELDS
    }
    if "trade_date" not in kwargs or not isinstance(kwargs["trade_date"], str) or not kwargs["trade_date"]:
        raise ValueError(f"{REASON_CONFIG_LOAD_FAILED}: {path!r} is missing required field 'trade_date'")

    return PaperReadinessConfig(**kwargs)


def _build_arg_parser() -> argparse.ArgumentParser:
    parser = argparse.ArgumentParser(
        prog="python -m mqk_research.scanner.paper_readiness_runner",
        description=(
            "Run the toggle-controlled paper-readiness artifact pipeline from a "
            "local JSON config. Composes existing, independently-proven scanner "
            "modules — no broker, daemon, DB, or network calls; approved_for_live "
            "is always false; daemon watchlist enforcement is never executed."
        ),
    )
    parser.add_argument("--config", dest="config_path", required=True,
                        help="Path to a paper-readiness config JSON file")
    return parser


def main(argv: Optional[list[str]] = None) -> int:
    """
    CLI entry point: `python -m mqk_research.scanner.paper_readiness_runner --config <path>`

    Prints the structured result as JSON to stdout. Exit code is 0 only for
    the two non-dangerous success states (`ready_for_operator_review`,
    `ready_for_paper_handoff`); 1 for `blocked`, `partial`, or a config load
    failure. Never starts a daemon, calls a broker, or reads secrets.
    """
    parser = _build_arg_parser()
    args = parser.parse_args(argv)

    try:
        config = load_config_from_json(args.config_path)
    except ValueError as exc:
        print(json.dumps(
            {
                "schema_version": SCHEMA_VERSION,
                "status": STATUS_BLOCKED,
                "reasons": [REASON_CONFIG_LOAD_FAILED],
                "approved_for_live": False,
                "live_locked": True,
                "daemon_enforcement_executed": False,
                "notes": str(exc),
            },
            indent=2,
        ))
        return 1

    result = run_paper_readiness_pipeline(config)
    print(json.dumps(result.to_dict(), indent=2, sort_keys=False, default=str))
    return 0 if result.status in (STATUS_READY_FOR_OPERATOR_REVIEW, STATUS_READY_FOR_PAPER_HANDOFF) else 1


if __name__ == "__main__":
    sys.exit(main())
