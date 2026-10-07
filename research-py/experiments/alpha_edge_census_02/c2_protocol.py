"""Census-02 result-independent protocol scaffold: structural protocol, operator-decision schema, freeze guard, data
fences and denominator/ledger guards. Nothing here reads prices or results."""

from __future__ import annotations

import hashlib
import json
import subprocess
import sys
from pathlib import Path

HERE = Path(__file__).resolve().parent
C1_DIR = HERE.parent / "alpha_edge_census_01"
for _p in (str(HERE), str(C1_DIR), str(HERE.parents[1] / "src")):
    if _p not in sys.path:
        sys.path.insert(0, _p)

import pandas as pd  # noqa: E402
import partitions as pt  # noqa: E402  (Census-01 fence authority, reused unchanged)
import simulate as _c1sim  # noqa: E402  (Census-01 cost/sizing constants: the single authority the protocol must equal)
import c2_borrow as bw  # noqa: E402

REPO = HERE.parents[2]
EXPERIMENT_ID = "alpha_edge_census_02"
SCHEMA = "alpha_census02_v1"
TRIAL_PREFIX = "ac02-"
STATUS_PROPOSED = "PROPOSED_NOT_FROZEN"
STATUS_FROZEN = "FROZEN_BY_OPERATOR"
PREDECLARATION_FILE = HERE / "CENSUS02_PREDECLARATION.json"
PROPOSAL_FILE = HERE / "CENSUS02_PREDECLARATION_PROPOSAL.json"


class FreezeRefusal(RuntimeError):
    pass


class DenominatorShrink(RuntimeError):
    pass


def canonical(obj) -> str:
    return json.dumps(obj, sort_keys=True, separators=(",", ":"), ensure_ascii=False)


def sha256_canonical(obj) -> str:
    return hashlib.sha256(canonical(obj).encode("utf-8")).hexdigest()


# Side-aware benchmark contract (PROPOSED, not frozen).
BENCHMARK_ROLES = {
    "short": {"cash_zero": "QUALIFICATION_NET", "passive_short_hold": "QUALIFICATION_ALPHA"},
    "long_short": {"cash_zero": "QUALIFICATION_NET", "passive_long_hold": "DIAGNOSTIC_ONLY", "passive_short_hold": "DIAGNOSTIC_ONLY"},
}
BENCHMARK_RULES = ("SIDE_AWARE_SHORT_NET_AND_PASSIVE_SHORT_ALPHA_LONGSHORT_NET_VS_CASH", "NET_POSITIVE_ONLY_ALL_SIDES")

FACTOR_SEMANTICS = {
    "family": "alpha_census02_short_conditional_v1", "protocol_version": "alpha_census02_short_conditional_factor_v1",
    "direction": "lower_is_better", "label": "fwd_close_return_minus_same_symbol_same_horizon_unconditional_mean",
    "raw_effect": "conditional_mean_fwd_ret - same_symbol_same_horizon_unconditional_mean",
    "direction_adjusted_effect": "-raw_effect (positive = favorable evidence for the short hypothesis)",
    "executable_pnl": False, "post_result_direction_flip": "forbidden"}


def cost_truth() -> dict:
    """Machine-readable cost/sizing truth, read from the actual simulator constants (never re-typed)."""
    return {"commission_bps_per_side": _c1sim.COMMISSION_BPS, "fill_slippage_bps_per_side": _c1sim.SLIPPAGE_BPS,
            "volatility_mult_bps": _c1sim.VOL_MULT_BPS, "additional_slippage_bps": 0.0,
            "pricing_model_id": "rust_conservative_bar_range_v1", "annualization_days": _c1sim.ANNUALIZATION,
            "entry_budget_usd": _c1sim.BUDGET_USD, "initial_capital_usd": _c1sim.CAPITAL_USD}


# ------------------------------------------------------------------------------------------ structural protocol
def build_structural_protocol() -> dict:
    """Execution/cost/benchmark semantics that need no new scientific threshold: every numeric value is the accepted
    Census-01 constant or is an explicit operator decision (see DECISIONS)."""
    return {
        "schema_version": SCHEMA, "experiment_id": EXPERIMENT_ID,
        "partitions": dict(pt.PARTITIONS),
        "data_contract": {"window": "[2016-01-01, 2024-01-01)", "contaminated_2024": "never scored",
                          "confirmation": "[2025-01-01, 2026-03-01) never read", "final_holdout": "[2026-03-01, ...) never read"},
        "execution_contract": {
            "positions": "signed_daily_completed_bars: -1 short, 0 flat, +1 long",
            "signal_knowledge": "signal on bar t uses only bars <= t",
            "fill": f"first bar strictly after t; BUY at high+slip, SELL at low-slip (rust_conservative_bar_range_v1, "
                    f"slippage {_c1sim.SLIPPAGE_BPS} bps, integer micros); short entry = SELL, cover = BUY; same-bar fill forbidden",
            "pnl_marking": "signed_qty_held_before_bar*(close_t-close_{t-1}); fill cost = |delta_qty|*adverse(fill vs close)+commission",
            "cost_model": cost_truth(),
            "sizing": f"qty = sign*floor({_c1sim.BUDGET_USD:g} USD / completed-signal-bar close); constant per run; no "
                      "compounding; sign flip = exit leg + entry leg on one fill bar",
            "borrow_cost": f"short holding bars accrue |qty|*prior_close*annual_fee_bps/10000/{_c1sim.ANNUALIZATION}; fee has "
                           "NO default; short rebate ZERO; availability/recall assumptions are disclosed, not asserted",
            "open_position_at_end": "marked_to_last_close_no_liquidation (unchanged from Census-01)",
            "corporate_actions": "adjustment=all total-return series: a short bears dividends through the adjusted series; "
                                 "unsupported corporate actions stay typed EXCLUDED (fail closed)",
            "ssr": "Rule 201 not represented; trigger is derivable from daily bars and is recorded as a per-fill flag only",
            "labels": "fwd_ret/close_{t+h}/close_t-1 is a LABEL: EXECUTABLE_PNL=false, never an input to simulate_signed",
        },
        "benchmark_semantics": {
            "never": "long buy-and-hold as the benchmark of a short-only strategy (double-counts the market); a switching "
                     "long/short strategy has no single-direction benchmark",
            "roles": {side: dict(roles) for side, roles in BENCHMARK_ROLES.items()},
            "passive_holds": "same window, same execution/cost/borrow assumptions as the strategy",
            "rules": list(BENCHMARK_RULES), "qualification_rule": "OPERATOR_DECISION benchmark_rule",
        },
        "conditional_factor_semantics": dict(FACTOR_SEMANTICS),
        "identity": {"trial": "sha256(side,family,params,scope,universe_id,partitions_id,protocol_id); results/attempts never enter",
                     "attempt": "infrastructure retry = new attempt of the SAME trial; outcome-based retry forbidden",
                     "evaluation": "slice/job/window; never mints a trial"},
        "retry_rules": {"infrastructure_retry": "same trial, new attempt, same economics", "outcome_based_retry": "forbidden"},
        "registration_rule": {"winner_only_registration": "forbidden: the complete ledger precedes any registry record",
                              "denominator": "frozen population root; cannot shrink after results"},
        "funnel": ["DISCOVERY", "STATISTICAL_ROBUSTNESS", "ECONOMIC_ROBUSTNESS", "INDEPENDENT_CONFIRMATION",
                   "PORTFOLIO_RISK_SUITABILITY", "PROMOTION", "PAPER"],
        "authority": {"VALIDATION_STATUS": "NOT_VALIDATED", "PROMOTION_AUTHORITY": "NONE"},
    }


# ------------------------------------------------------------------------------------------- operator decisions
def _opt(*values):
    return lambda v: v in values


def _threshold_block(v) -> bool:
    keys = {"trade_count_confidence_band", "year_stability", "regime_concentration", "parameter_neighborhood",
            "portfolio_mdd_worst5day"}
    return isinstance(v, dict) and set(v) == keys and all(isinstance(x, (dict, str)) and x for x in v.values())


def _etf_assumption(v) -> bool:
    if v is None:
        return True
    try:
        bw.validate_etf_assumption(v)
    except bw.BorrowRefusal:
        return False
    return True


DECISIONS = {
    "borrow_policy": (("EQUITY_HYPOTHESIS_ONLY_ETF_EXECUTABLE_FROZEN_ASSUMPTION", "NO_EXECUTABLE_SHORTS_HYPOTHESIS_ONLY"),
                      _opt("EQUITY_HYPOTHESIS_ONLY_ETF_EXECUTABLE_FROZEN_ASSUMPTION", "NO_EXECUTABLE_SHORTS_HYPOTHESIS_ONLY")),
    "etf_borrow_assumption": ("explicit list + annual fee bps (see c2_borrow)", _etf_assumption),
    "grammar_tiers": (("H", "H+L"), _opt("H", "H+L")),   # tier X (cross-sectional) is not implemented: a later amendment
    "complement_handling": (("REGISTER_ALL_TAG_COMPLEMENTS", "EXCLUDE_COMPLEMENTS_BEFORE_FREEZE"),
                            _opt("REGISTER_ALL_TAG_COMPLEMENTS", "EXCLUDE_COMPLEMENTS_BEFORE_FREEZE")),
    "benchmark_rule": (BENCHMARK_RULES, _opt(*BENCHMARK_RULES)),
    "multiple_testing_denominator": (("LOCAL_CENSUS02", "LOCAL_WITH_GLOBAL_DISCLOSURE", "GLOBAL_POOLED_CENSUS01_CENSUS02"),
                                     _opt("LOCAL_CENSUS02", "LOCAL_WITH_GLOBAL_DISCLOSURE", "GLOBAL_POOLED_CENSUS01_CENSUS02")),
    # An equity-only family would need a separately frozen complete instrument-class mapping that does not exist.
    "conditional_scope": (("ALL_SEED_SYMBOLS",), _opt("ALL_SEED_SYMBOLS")),
    "ssr_handling": (("FLAG_ONLY", "DEFER_ENTRY_ON_KNOWN_SSR_DAY"), _opt("FLAG_ONLY", "DEFER_ENTRY_ON_KNOWN_SSR_DAY")),
    "funnel_thresholds": ("five-part block, see c2 doc", _threshold_block),
}


def validate_decisions(decisions) -> dict:
    """Fail closed unless every operator decision is present, no extras, each valid. No default is ever supplied."""
    if not isinstance(decisions, dict):
        raise FreezeRefusal("decisions must be an object")
    missing, extra = sorted(set(DECISIONS) - set(decisions)), sorted(set(decisions) - set(DECISIONS))
    if missing or extra:
        raise FreezeRefusal(f"operator decisions incomplete: missing {missing}, unknown {extra}")
    for k, (_opts, ok) in DECISIONS.items():
        if not ok(decisions[k]):
            raise FreezeRefusal(f"operator decision {k!r} is invalid")
    executable = decisions["borrow_policy"] == "EQUITY_HYPOTHESIS_ONLY_ETF_EXECUTABLE_FROZEN_ASSUMPTION"
    if executable != (decisions["etf_borrow_assumption"] is not None):
        raise FreezeRefusal("etf_borrow_assumption must be present exactly when the policy allows executable ETF shorts")
    return decisions


def frozen_protocol_id(structural: dict, decisions: dict, source_manifest: dict) -> str:
    """Binds the protocol, the operator decisions AND the behavior-source manifest, so a source edit changes every id."""
    validate_decisions(decisions)
    return sha256_canonical({"structural": structural, "decisions": decisions, "behavior_source_manifest": source_manifest})[:32]


# ------------------------------------------------------------------------------- behavior-source manifest
_C1 = "research-py/experiments/alpha_edge_census_01/"
_C2 = "research-py/experiments/alpha_edge_census_02/"
_SRC = "research-py/src/mqk_research/"
# Pre-result sources that can change the candidate grammar, signal values, execution chronology, price/cost arithmetic,
# factor direction/identity or partition fencing. Generated outputs and result values are never bound.
BEHAVIOR_SOURCES = tuple(sorted((
    _C2 + "c2_protocol.py", _C2 + "c2_borrow.py", _C2 + "c2_grammar.py", _C2 + "c2_factors.py", _C2 + "c2_signals.py",
    _C2 + "c2_simulate.py",
    _C1 + "search_space.py", _C1 + "signals.py", _C1 + "simulate.py", _C1 + "partitions.py", _C1 + "calendar_authority.py",
    _SRC + "indicators/core.py", _SRC + "factors/contracts.py", _SRC + "exp_distributed/hashing.py")))
# Frozen pre-result input data a behavior source reads (the seed universe validates the borrow scope).
AUTHORITY_DATA = (_C1 + "ALPHA_CENSUS_SEED_UNIVERSE_V2.json",)
# In-repo modules reachable by import that were inspected and are NOT behavior-bearing for Census-02 values.
REVIEWED_NON_BEHAVIOR = {
    _C1 + "data.py": "imported by search_space for Census-01 eligibility constants and acquisition; no Census-02 value uses it",
    _SRC + "data/alpaca_historical.py": "provider acquisition path reached only via data.py; Census-02 never acquires data",
    _SRC + "data/bars_provenance.py": "acquisition provenance, reached only via alpaca_historical",
    _SRC + "data/ca_reviewed_resolutions.py": "acquisition corporate-action resolutions, reached only via alpaca_historical",
    _SRC + "ml/util_hash.py": "hash helper reached only via ca_reviewed_resolutions",
    _SRC + "universe/snapshot.py": "used only by search_space.build_seed_universe, which Census-02 never calls",
}


def _sha_lf(path: Path) -> str:
    try:
        raw = Path(path).read_bytes()
    except OSError as exc:
        raise FreezeRefusal(f"behavior source {Path(path).name} unreadable ({type(exc).__name__})") from None
    return hashlib.sha256(raw.replace(b"\r\n", b"\n")).hexdigest()


def behavior_source_manifest(repo: Path = REPO) -> dict:
    repo = Path(repo)
    return {"schema_version": "census02_behavior_source_manifest_v1", "hash": "sha256_lf",
            "sources": {rel: _sha_lf(repo / rel) for rel in BEHAVIOR_SOURCES},
            "authority_data": {rel: _sha_lf(repo / rel) for rel in AUTHORITY_DATA}}


# ------------------------------------------------------------------------------------------------- freeze guard
def _git_rc(repo: Path, *args: str) -> int:
    return subprocess.run(["git", *args], cwd=repo, capture_output=True).returncode


def require_committed(paths, repo: Path) -> str:
    repo = Path(repo)
    for p in paths:
        rel = str(Path(p).resolve().relative_to(repo.resolve())).replace("\\", "/")
        if _git_rc(repo, "ls-files", "--error-unmatch", rel) != 0:
            raise FreezeRefusal(f"{rel} is not committed: real Census-02 execution is refused before the freeze commit")
        if _git_rc(repo, "diff", "--quiet", "HEAD", "--", rel) != 0:
            raise FreezeRefusal(f"{rel} differs from HEAD: real Census-02 execution is refused")
    return subprocess.run(["git", "rev-parse", "HEAD"], cwd=repo, capture_output=True, text=True, check=True).stdout.strip()


def require_freeze(repo: Path = REPO, predeclaration: Path = PREDECLARATION_FILE) -> dict:
    """Gate for ANY real-data Census-02 access: a committed, operator-frozen predeclaration whose decisions are complete
    and whose protocol id equals the recomputed one. A proposal file can never satisfy it."""
    predeclaration = Path(predeclaration)
    if not predeclaration.exists():
        raise FreezeRefusal(f"{predeclaration.name} does not exist: real Census-02 execution is refused (policy not frozen)")
    head = require_committed([predeclaration], repo)
    doc = json.loads(predeclaration.read_text(encoding="utf-8"))
    if doc.get("status") != STATUS_FROZEN:
        raise FreezeRefusal(f"predeclaration status {doc.get('status')!r} is not {STATUS_FROZEN}")
    if doc.get("attempts_at_freeze") != 0:
        raise FreezeRefusal("freeze must precede every attempt (attempts_at_freeze != 0)")
    require_committed([Path(repo) / rel for rel in (*BEHAVIOR_SOURCES, *AUTHORITY_DATA)], repo)
    manifest = behavior_source_manifest(repo)
    if doc.get("behavior_source_manifest") != manifest:
        drift = sorted(k for k, v in manifest["sources"].items()
                       if (doc.get("behavior_source_manifest") or {}).get("sources", {}).get(k) != v)
        raise FreezeRefusal(f"behavior-bearing source differs from the frozen manifest: {drift or 'manifest malformed'}")
    structural = build_structural_protocol()
    if doc.get("structural_protocol") != structural:
        raise FreezeRefusal("structural protocol differs from the committed predeclaration (a value changed after the freeze)")
    if doc.get("protocol_id") != frozen_protocol_id(structural, doc.get("decisions"), manifest):
        raise FreezeRefusal("protocol_id does not equal the recomputed id")
    return {"head": head, "predeclaration": doc}


# ---------------------------------------------------------------------------------------------------- data fences
def fence_bars(bars: pd.DataFrame, *, what: str) -> pd.DataFrame:
    """Refuse any bar at/after 2024-01-01 (contaminated year, Confirmation, Final Holdout) and any empty input."""
    pt.require_discovery_only(bars["end_ts"], what=what)
    return bars


def assert_executable_record(rec: dict) -> dict:
    """Only a simulate_signed-derived record under an executable evidence class may carry P&L semantics."""
    if rec.get("executable_pnl") is not True:
        raise bw.BorrowRefusal("record is not executable P&L (label/diagnostic records cannot be scored as P&L)")
    bw.require_executable(rec.get("evidence_class", ""))
    return rec


# ------------------------------------------------------------------------------------------- denominator guards
def assert_complete_ledger(population_ids, ledger_ids) -> None:
    """The ledger must account for exactly the frozen population (every trial, once): the denominator cannot shrink
    after results, and nothing outside the frozen population may appear."""
    pop, led = list(population_ids), list(ledger_ids)
    if len(set(pop)) != len(pop):
        raise DenominatorShrink("frozen population contains duplicate trial ids")
    if len(set(led)) != len(led):
        raise DenominatorShrink("ledger contains a duplicated trial id (a retry must be a new attempt, not a new trial)")
    missing, extra = sorted(set(pop) - set(led)), sorted(set(led) - set(pop))
    if missing or extra:
        raise DenominatorShrink(f"ledger is not the frozen population: {len(missing)} missing, {len(extra)} outside")


def register_edges(registry_records, population_ids, ledger_ids) -> list:
    """Edge registry writes are allowed only after the complete ledger exists (winner-only registration forbidden)."""
    assert_complete_ledger(population_ids, ledger_ids)
    pop = set(population_ids)
    stray = sorted(r["trial_id"] for r in registry_records if r["trial_id"] not in pop)
    if stray:
        raise DenominatorShrink(f"{len(stray)} registry records are outside the frozen population")
    return list(registry_records)
