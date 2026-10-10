"""M1 native hypothesis batch runner. Every parameter is read from the declaration
named by MQK_M1_BATCH_DECLARATION (default PREDECLARED_BATCH_01.json, which this runner
refuses); nothing result-dependent is chosen here.

Stages (each once, in order): check | reuse_data | register | gate | trials | judge |
backtest | finalize | review | summary.

Chronology (hypothesis -> trial registration -> attempt -> evaluation):
`register` resolves every strategy's semantic identity WITHOUT market data
(`mqk backtest native-fingerprint`) and registers the hypotheses and ALL trials;
it never runs an emitter or Backtest. `trials` then runs the attempts in the
frozen order; the native signal emitter is invoked INSIDE each attempt, so an
emission failure is a failed attempt of the same trial. An economic failure
never stops the batch.

Batch 01 itself was run by an earlier version of this runner (emitter before
registration, binary-weight economics). Its evidence is historical and
superseded; `check` refuses to run any predeclaration that is not under the
exact-target protocol.
"""

from __future__ import annotations

import argparse
import json
import os
import re
import subprocess
import sys
from pathlib import Path

import pandas as pd

HERE = Path(__file__).resolve().parent
REPO = HERE.parents[2]
sys.path.insert(0, str(REPO / "research-py" / "src"))
sys.path.insert(0, str(HERE))
import stage_authorization  # noqa: E402

DECL_FILE = os.environ.get("MQK_M1_BATCH_DECLARATION", "PREDECLARED_BATCH_01.json")
DECL = json.loads((HERE / DECL_FILE).read_text(encoding="utf-8"))
RUN = HERE / DECL["run_dir"]
REGISTRY = HERE / DECL["experiment"]["registry_db_relative_path"]
EXPERIMENT = DECL["experiment"]["real_experiment_id"]
HYP = {h["strategy_id"]: h for h in DECL["hypotheses"]}
TRIALS = [(t["strategy_id"], t["symbol"]) for t in DECL["universe"]["trials"]]  # frozen order
STRATEGIES = [h["strategy_id"] for h in DECL["hypotheses"]]
CLI = Path(os.environ.get("MQK_M1_CLI") or REPO / "core-rs" / "target" / "debug" / "mqk-cli.exe")
BARS = RUN / "data" / "research_bars.csv"
MANIFEST = RUN / "data" / "research_bars_provenance.json"
INDEX = RUN / "trials_index.json"
GAP_TOLERANCE_BARS = 3
# The canonical Backtest integrity configuration. `backtest csv` and the
# Benchmark V2 `scan-strategies` MUST receive the identical values so the
# scanner/review candidate and the canonical Backtest evidence share one
# BacktestConfig identity (IR-BV2-01); Promotion refuses a mismatch.
INTEGRITY_ARGS = ["--integrity-calendar", "us-equity-regular", "--integrity-stale-threshold-ticks", "259200",
                  "--integrity-gap-tolerance-bars", str(GAP_TOLERANCE_BARS)]
EXACT_TARGET_DIRECTION_POLICY = "native_exact_target_qty_v1"
SIZING_POLICY_CF = "fixed_initial_capital_fraction_v1"
BENCHMARK_V2 = "capital_matched_exact_target_buy_hold_v1"
BENCHMARK_CF = "capital_fraction_matched_passive_buy_hold_v1"
CAPITAL_BASIS = "native_backtest.initial_cash_micros"


_ACTIVE = {"stage": None, "auth": None}


def _resume(args) -> bool:
    """`--resume` opts a stage into crash-safe convergence (skip what is already terminal). Without it every stage keeps
    its historical single-pass behaviour exactly."""
    return bool(getattr(args, "resume", False))


def _require_active_stage() -> None:
    """Effectful helpers (`_run_cli`, `_save_index`) run only inside an authorized stage of a non-historical
    declaration, so importing this module and calling a helper directly cannot bypass the stage gate."""
    if _ACTIVE["stage"] is None and not stage_authorization.is_frozen_historical(DECL):
        raise stage_authorization.AuthorizationError(
            "fail-closed: effectful helper called outside an authorized runner stage")


def staged(name: str):
    """Authorize `name` for this exact declaration BEFORE the stage does anything (credentials, HTTP,
    directories, registry, subprocess), whether it is reached from the CLI dispatcher or called directly."""
    def wrap(fn):
        def run(args):
            outer = (_ACTIVE["stage"], _ACTIVE["auth"])
            _ACTIVE["auth"] = stage_authorization.require_stage(DECL, name)
            _ACTIVE["stage"] = name
            try:
                return fn(args)
            finally:
                _ACTIVE["stage"], _ACTIVE["auth"] = outer  # a nested stage never ends or inherits its caller's authority
        run.__name__, run.__doc__, run.__wrapped__ = fn.__name__, fn.__doc__, fn
        return run
    return wrap


def tdir(strategy: str, symbol: str) -> Path:
    return RUN / "trials" / strategy / symbol


def key(strategy: str, symbol: str) -> str:
    return f"{strategy}/{symbol}"


def _economic_spec(decl: dict | None = None):
    from mqk_research.ml.economic_walkforward import (
        AnnualizationSpec, CostModelSpec, EconomicWalkForwardSpec, SignalPolicySpec)
    from mqk_research.ml.execution_pricing import ExecutionPricingSpec
    from mqk_research.ml.weight_to_share import WeightToShareSpec
    p = (decl or DECL)["economic_protocol"]
    return EconomicWalkForwardSpec(
        signal_policy=SignalPolicySpec(**p["signal_policy"]),
        cost_model=CostModelSpec(**p["cost_model"]),
        execution_pricing=ExecutionPricingSpec(**p["execution_pricing"]),
        weight_to_share=WeightToShareSpec(**p["weight_to_share"]),
        annualization=AnnualizationSpec(**p["annualization"]),
    )


def _run_cli(*argv: str) -> str:
    _require_active_stage()
    exe = CLI
    if not stage_authorization.is_frozen_historical(DECL):
        # _ACTIVE is mutable state: the signed authorization is re-verified here, at the executable boundary, and
        # only the exact binary it pins may run.
        exe = stage_authorization.authorize_native_execution(DECL, _ACTIVE["stage"], _ACTIVE["auth"], CLI)
    out = subprocess.run([str(exe), *argv], capture_output=True, text=True)
    if out.returncode != 0:
        raise SystemExit(f"mqk-cli failed ({argv[:2]}): {out.stderr[-800:]}")
    return out.stdout


def _parse(out: str, name: str) -> str:
    m = re.search(rf"^{name}=(.+)$", out, re.M)
    if not m:
        raise SystemExit(f"missing {name} in cli output")
    return m.group(1).strip()


def _load_index() -> dict:
    return json.loads(INDEX.read_text(encoding="utf-8")) if INDEX.exists() else {}


def _save_index(index: dict) -> None:
    _require_active_stage()
    if not stage_authorization.is_frozen_historical(DECL):
        stage_authorization.reverify_active_stage(DECL, _ACTIVE["stage"], _ACTIVE["auth"])
    INDEX.parent.mkdir(parents=True, exist_ok=True)
    tmp = INDEX.with_suffix(".json.tmp")  # atomic replace: a crash never leaves a torn index
    tmp.write_text(json.dumps(index, indent=1, sort_keys=True), encoding="utf-8")
    os.replace(tmp, INDEX)


def _require_exact_target_protocol() -> None:
    policy = DECL["economic_protocol"]["signal_policy"].get("direction_policy")
    if policy != EXACT_TARGET_DIRECTION_POLICY:
        raise SystemExit(
            f"fail-closed: this predeclaration uses direction_policy={policy!r} (the superseded binary-weight "
            f"bridge); its evidence is historical and not promotion authority. A rerun needs a new "
            f"predeclaration under {EXACT_TARGET_DIRECTION_POLICY!r} and separate authorization"
        )
    equity_micros = round(float(DECL["economic_protocol"]["weight_to_share"]["equity_usd"]) * 1_000_000)
    if int(DECL["native_backtest"]["initial_cash_micros"]) != equity_micros:
        raise SystemExit(
            "fail-closed: native_backtest.initial_cash_micros must equal the Research equity_usd "
            f"({equity_micros} micros): Research and Backtest evidence share one capital basis"
        )


_DESCRIPTIVE_SIZING_FIELDS = {"cap_note", "quantity_rule", "forbidden"}


def sizing_args(decl: dict, allocation_fraction_bps: int | None = None) -> list[str]:
    """CLI sizing flags from the declaration's optional `capital_sizing` block.
    `allocation_fraction_bps` overrides the declared fraction ONLY for the validated
    robustness stress scenario (never for a trial).

    Absent block = the historical fixed-quantity protocol (no flags). A present
    block must name the capital-fraction policy, an explicit integer fraction in
    1..=10000 bps, the immutable initial capital basis, and the matching
    capital-fraction benchmark; anything else is refused. Caps are optional
    positive integers.
    """
    block = decl.get("capital_sizing")
    if block is None:
        return []
    if not isinstance(block, dict):
        raise SystemExit("fail-closed: capital_sizing must be an object")
    unknown = set(block) - {"policy_id", "allocation_fraction_bps", "capital_basis", "max_target_qty",
                            "max_position_notional_usd", "initial_capital_micros",
                            "nominal_entry_budget_micros"} - _DESCRIPTIVE_SIZING_FIELDS
    if unknown:
        raise SystemExit(f"fail-closed: capital_sizing has unknown fields {sorted(unknown)}")
    if block.get("policy_id") != SIZING_POLICY_CF:
        raise SystemExit(f"fail-closed: capital_sizing.policy_id must be {SIZING_POLICY_CF!r}")
    bps = block.get("allocation_fraction_bps")
    if type(bps) is not int or not 1 <= bps <= 10_000:
        raise SystemExit("fail-closed: capital_sizing.allocation_fraction_bps must be an explicit integer 1..=10000")
    if block.get("capital_basis") != CAPITAL_BASIS:
        raise SystemExit(f"fail-closed: capital_sizing.capital_basis must be {CAPITAL_BASIS!r}")
    capital = int(decl["native_backtest"]["initial_cash_micros"])
    if "initial_capital_micros" in block and block["initial_capital_micros"] != capital:
        raise SystemExit("fail-closed: capital_sizing.initial_capital_micros != native_backtest.initial_cash_micros")
    if "nominal_entry_budget_micros" in block and block["nominal_entry_budget_micros"] != capital * bps // 10_000:
        raise SystemExit("fail-closed: capital_sizing.nominal_entry_budget_micros != floor(capital * bps / 10000)")
    if decl["scanner_review"].get("benchmark_policy") != BENCHMARK_CF:
        raise SystemExit(f"fail-closed: capital_sizing requires scanner_review.benchmark_policy {BENCHMARK_CF!r}")
    if allocation_fraction_bps is not None:
        if type(allocation_fraction_bps) is not int or not 1 <= allocation_fraction_bps <= 10_000:
            raise SystemExit("fail-closed: the stress allocation_fraction_bps must be an explicit integer 1..=10000")
        bps = allocation_fraction_bps
    args = ["--sizing-policy", SIZING_POLICY_CF, "--allocation-fraction-bps", str(bps)]
    for field, flag in (("max_target_qty", "--max-target-qty"),
                        ("max_position_notional_usd", "--max-position-notional-usd")):
        if field in block:
            if type(block[field]) is not int or block[field] <= 0:
                raise SystemExit(f"fail-closed: capital_sizing.{field} must be a positive integer")
            args += [flag, str(block[field])]
    return args


def research_capital_sizing(decl: dict) -> dict | None:
    """The Research-side capital-fraction contract (identity + stream verification), derived
    from the same validated block and the same capital basis `backtest csv` receives. None =
    historical fixed-quantity protocol."""
    if sizing_args(decl) == []:
        return None
    block = decl["capital_sizing"]
    return {"policy_id": SIZING_POLICY_CF, "allocation_fraction_bps": block["allocation_fraction_bps"],
            "initial_allocated_capital_micros": int(decl["native_backtest"]["initial_cash_micros"]),
            "max_target_qty": block.get("max_target_qty"),
            "max_position_notional_usd": block.get("max_position_notional_usd")}


def research_stress_contract(decl: dict) -> dict | None:
    """The robustness stress to bind into every registered trial identity, or None.

    Only a declaration that opts in with a literal `robustness.p7a_p7b_stress.register_stress_contract:
    true` registers it (Promotion later requires it for a capital-fraction candidate). A closed
    historical campaign without the flag keeps its recorded trial ids."""
    st = decl["robustness"]["p7a_p7b_stress"]
    flag = st.get("register_stress_contract")
    if flag is None:
        return None
    if flag is not True:
        raise SystemExit("fail-closed: register_stress_contract must be the literal true when present")
    plan = stress_plan(decl)
    if plan["mode"] != "capital_fraction":
        raise SystemExit("fail-closed: register_stress_contract requires a capital-fraction stress declaration")
    from mqk_research.ml.native_signal_registry_integration import STRESS_CONTRACT_SCHEMA_VERSION

    st = decl["robustness"]["p7a_p7b_stress"]
    ceiling = st["max_drawdown_ceiling"]
    if type(ceiling) not in (int, float):
        raise SystemExit("fail-closed: max_drawdown_ceiling must be a number")
    ceiling_bps = ceiling * 10_000
    if abs(ceiling_bps - round(ceiling_bps)) > 1e-9:
        raise SystemExit("fail-closed: max_drawdown_ceiling must be a number that is an exact whole number of bps")
    return {
        "schema_version": STRESS_CONTRACT_SCHEMA_VERSION,
        "scenario_id": plan["scenario_id"],
        "allocation_fraction_bps": plan["allocation_fraction_bps"],
        "stress_execution_slippage_bps": st["stress_execution_slippage_bps"],
        "stress_execution_volatility_mult_bps": st["stress_execution_volatility_mult_bps"],
        "max_drawdown_ceiling_bps": round(ceiling_bps),
    }


def canonical_timeframe_identity(decl: dict) -> bool:
    """True only when the declaration opts in to the versioned `canonical_semantic_v1` timeframe
    identity (1D == 1Day). Absent = the historical raw-label identity, so closed campaigns keep their ids."""
    value = decl["data"].get("timeframe_identity")
    if value is None:
        return False
    if value != "canonical_semantic_v1":
        raise SystemExit("fail-closed: data.timeframe_identity must be 'canonical_semantic_v1' when present")
    return True


def native_bridge_args(decl: dict, allocation_fraction_bps: int | None = None) -> list[str]:
    """CLI flags for `native-fingerprint` / `native-signals`: the sizing flags plus the explicit
    initial capital (the capital-fraction bridge has no default capital)."""
    args = sizing_args(decl, allocation_fraction_bps)
    if not args:
        return []
    return [*args, "--initial-cash-micros", str(int(decl["native_backtest"]["initial_cash_micros"]))]


STRESS_FORBIDDEN_CAP_FIELDS = ("stress_max_position_notional_usd", "stress_max_target_qty")
STRESS_SIZING_KEYS = {"scenario_id", "policy_id", "allocation_fraction_bps", "initial_capital_micros",
                      "nominal_entry_budget_micros", "quantity_rule", "is_a_trial", "identity"}


def stress_plan(decl: dict) -> dict:
    """The validated robustness stress contract.

    Fixed-quantity declarations keep the historical P7B cap stress. A capital-fraction declaration
    must express its half-exposure stress as a RECOMPUTED quantity at a strictly smaller
    `allocation_fraction_bps` on the same immutable capital: a USD cap on the baseline quantity is
    refused by the exact-target replay, and a non-binding historical cap is not a half-exposure test.
    """
    st = decl["robustness"]["p7a_p7b_stress"]
    sizing = st.get("stress_sizing")
    block = decl.get("capital_sizing")
    if block is None:
        if sizing is not None:
            raise SystemExit("fail-closed: stress_sizing requires a capital_sizing declaration")
        return {"mode": "cap"}
    sizing_args(decl)  # the baseline block must itself be valid
    if not isinstance(sizing, dict):
        raise SystemExit("fail-closed: a capital-fraction declaration requires robustness.p7a_p7b_stress.stress_sizing")
    present = [f for f in STRESS_FORBIDDEN_CAP_FIELDS if f in st]
    if present:
        raise SystemExit(f"fail-closed: a capital-fraction stress must not carry {present}: the half exposure is a "
                         "recomputed quantity, not a USD cap on the baseline quantity")
    if set(sizing) != STRESS_SIZING_KEYS:
        raise SystemExit(f"fail-closed: stress_sizing must have exactly the keys {sorted(STRESS_SIZING_KEYS)}")
    bps, base_bps = sizing["allocation_fraction_bps"], block["allocation_fraction_bps"]
    if type(bps) is not int or not 1 <= bps < base_bps:
        raise SystemExit(f"fail-closed: stress allocation_fraction_bps must be an integer strictly below the "
                         f"baseline {base_bps}")
    capital = int(decl["native_backtest"]["initial_cash_micros"])
    if sizing["policy_id"] != SIZING_POLICY_CF or sizing["initial_capital_micros"] != capital:
        raise SystemExit("fail-closed: the stress must use the capital-fraction policy on the same immutable capital")
    if sizing["nominal_entry_budget_micros"] != capital * bps // 10_000:
        raise SystemExit("fail-closed: stress_sizing.nominal_entry_budget_micros != floor(capital * bps / 10000)")
    if sizing["is_a_trial"] is not False or not sizing["scenario_id"]:
        raise SystemExit("fail-closed: a stress scenario is never a trial and must be named")
    return {"mode": "capital_fraction", "scenario_id": sizing["scenario_id"], "allocation_fraction_bps": bps}


def _require_frozen_trial_structure() -> None:
    trials = DECL["universe"]["trials"]
    if [t["order"] for t in trials] != list(range(1, len(trials) + 1)):
        raise SystemExit("fail-closed: trial order fields must be exactly 1..N in declaration order")
    if len(set(TRIALS)) != len(TRIALS):
        raise SystemExit("fail-closed: duplicate (strategy, symbol) trial in the declaration")
    symbols = set(DECL["universe"]["symbols"])
    if any(strategy not in HYP or sym not in symbols for strategy, sym in TRIALS):
        raise SystemExit("fail-closed: a declared trial names an undeclared strategy or symbol")
    if len(TRIALS) != DECL["universe"]["max_trials"]:
        raise SystemExit("fail-closed: the declared trial count differs from max_trials")


@staged("check")
def stage_check(_args) -> None:
    _require_exact_target_protocol()
    require_development_window(DECL)
    sizing_args(DECL)
    stress_plan(DECL)
    _require_frozen_trial_structure()
    assert len(TRIALS) == DECL["universe"]["max_trials"]
    cli_check = "NOT_PRESENT"
    if CLI.exists():  # a stale binary that lacks a declared engine or sizing flag fails here, before any work
        # Executing the native binary is not read-only: a graded declaration runs it only under an
        # authorization that pins the exact binary; otherwise the declaration-only validation above stands.
        auth = None if stage_authorization.is_frozen_historical(DECL) else stage_authorization.optional_authorization(
            DECL, stage_authorization.NATIVE_IDENTITY_RESOLUTION)
        if stage_authorization.is_frozen_historical(DECL) or auth is not None:
            _ACTIVE["auth"] = auth
            for strategy, sym in TRIALS:
                _fingerprint, required = _resolve_native_identity(strategy, sym)
                if required != HYP[strategy]["required_history_bars"]:
                    raise SystemExit(f"fail-closed: {strategy} requires {required} bars in the CLI but "
                                     f"{HYP[strategy]['required_history_bars']} in the predeclaration")
            cli_check = "PERFORMED"
        else:
            cli_check = "SKIPPED_NOT_AUTHORIZED"
    print(f"batch={DECL['batch_id']} trials={len(TRIALS)} strategies={STRATEGIES} cli_present={CLI.exists()} "
          f"cli_identity_check={cli_check}")


@staged("reuse_data")
def stage_reuse_data(_args) -> None:
    import shutil
    from mqk_research.ml.util_hash import sha256_file
    src = HERE / DECL["data"]["reuse_verified_data_from"]["run_dir"]
    manifest = json.loads((src / "research_bars_provenance.json").read_text(encoding="utf-8"))
    csv_path = src / "research_bars.csv"
    rows = len(pd.read_csv(csv_path))
    if sha256_file(csv_path) != manifest["artifact_sha256"] or rows != manifest["row_count"]:
        raise SystemExit("fail-closed: source bars do not match their provenance manifest")
    pin = DECL["data"]["reuse_verified_data_from"]
    for declared, actual in (("expected_artifact_sha256", manifest["artifact_sha256"]),
                             ("expected_row_count", manifest["row_count"]),
                             ("expected_canonical_semantic_bars_hash", manifest["canonical_semantic_bars_hash"]),
                             ("expected_source_attestation_id", manifest["source_attestation_id"])):
        if declared in pin and pin[declared] != actual:
            raise SystemExit(f"fail-closed: reused data {declared} differs from the predeclared data identity")
    (RUN / "data").mkdir(parents=True, exist_ok=True)
    for name in ("research_bars.csv", "research_bars_provenance.json", "corporate_actions.json",
                 "corporate_actions_provenance.json"):
        shutil.copyfile(src / name, RUN / "data" / name)
    print("reused", rows, "rows", manifest["artifact_sha256"][:12])


def _load_alpaca_env() -> None:
    """Load only the two research-data credential keys from .env.local (values are never printed)."""
    from mqk_research.data.alpaca_historical import require_provider_access_allowed
    require_provider_access_allowed()  # before ANY read of the environment or .env.local
    want = {"ALPACA_API_KEY_PAPER", "ALPACA_API_SECRET_PAPER"}
    if all(os.environ.get(k) for k in want):
        return
    env_file = REPO / ".env.local"
    if env_file.exists():
        for line in env_file.read_text(encoding="utf-8").splitlines():
            k, _, v = line.partition("=")
            if k.strip() in want and v.strip():
                os.environ[k.strip()] = v.strip().strip('"').strip("'")
    missing = [k for k in sorted(want) if not os.environ.get(k)]
    if missing:
        raise SystemExit(f"fail-closed: credentials unavailable: {missing}")


def fixed_holdout_boundary(decl: dict) -> dict | None:
    """The declared fixed partition (`partition.holdout_boundary`), validated, or None for the historical
    derivation from the fetched span. A malformed boundary is refused, never ignored."""
    boundary = decl["partition"].get("holdout_boundary")
    if boundary is None:
        return None
    from mqk_research.ml.native_signal_registry_integration import NativeSignalError, validate_fixed_holdout_boundary
    try:
        validate_fixed_holdout_boundary(boundary, decl["partition"]["holdout_months"])
    except NativeSignalError as exc:
        raise SystemExit(f"fail-closed: {exc}") from exc
    return boundary


def require_development_window(decl: dict) -> None:
    """A graded declaration (one carrying `evidence_grade`) must name a fixed reserved boundary, and any
    declared boundary bounds the provider request: the development fetch can never ask for a reserved date.
    Ungraded historical declarations keep their derived boundary and their recorded fetch window."""
    boundary = fixed_holdout_boundary(decl)
    if boundary is None:
        if "evidence_grade" in decl:
            raise SystemExit("fail-closed: a graded declaration must declare partition.holdout_boundary "
                             "(the provider request may not be allowed to reach a derived reserved window)")
        return
    start, end = pd.Timestamp(decl["data"]["start_utc"]), pd.Timestamp(decl["data"]["end_utc"])
    reserved = pd.Timestamp(boundary["holdout_start_utc"])
    if start.tzinfo is None or end.tzinfo is None or not start < end <= reserved:
        raise SystemExit(f"fail-closed: data.end_utc {end} must be a UTC instant at or before the reserved holdout "
                         f"start {reserved}; the provider request may not include a reserved date")


def verify_fetched_bars(decl: dict, bars: pd.DataFrame) -> dict[str, int]:
    """Every declared symbol present exactly (no extras, no substitution) with enough history to cover the
    widest declared strategy requirement. Returns per-symbol row counts. Under a fixed partition every
    returned row must lie in [data.start_utc, holdout_start) and every symbol must cover the same span."""
    symbols = list(decl["universe"]["symbols"])
    present = sorted(bars["symbol"].unique())
    if present != sorted(symbols):
        raise SystemExit(f"fail-closed: fetched symbols {present} differ from the declared universe {sorted(symbols)}")
    boundary = fixed_holdout_boundary(decl)
    if boundary is not None:
        ts = pd.to_datetime(bars["end_ts"], utc=True)
        reserved, begin = pd.Timestamp(boundary["holdout_start_utc"]), pd.Timestamp(decl["data"]["start_utc"])
        if (ts >= reserved).any():
            raise SystemExit(f"fail-closed: the provider returned {int((ts >= reserved).sum())} row(s) at or after the "
                             f"reserved holdout start {reserved}; nothing is admitted")
        if (ts < begin).any():
            raise SystemExit("fail-closed: the provider returned rows before the declared development start")
        spans = {s: (ts[bars["symbol"] == s].min(), ts[bars["symbol"] == s].max()) for s in symbols}
        if len(set(spans.values())) != 1:
            raise SystemExit(f"fail-closed: symbols cover different spans {sorted((s, str(v)) for s, v in spans.items())}")
    need = max(h["required_history_bars"] for h in decl["hypotheses"])
    counts = {s: int((bars["symbol"] == s).sum()) for s in symbols}
    short = {s: n for s, n in counts.items() if n < need}
    if short:
        raise SystemExit(f"fail-closed: insufficient adjusted history (need >= {need} bars): {short}")
    return counts


@staged("fetch")
def stage_fetch(args) -> None:
    if not args.execute:
        raise SystemExit("fetch contacts the data provider; pass --execute")
    require_development_window(DECL)  # before credentials, HTTP or any directory
    dest = RUN / "data"
    if (dest / "research_bars.csv").exists():
        raise SystemExit("fail-closed: bars already fetched for this run; refusing to overwrite")
    from mqk_research.data.alpaca_historical import (
        extract_research_bars_with_provenance, write_research_extraction_artifacts)
    _load_alpaca_env()
    d = DECL["data"]
    result = extract_research_bars_with_provenance(
        symbols=list(DECL["universe"]["symbols"]), start_utc=pd.Timestamp(d["start_utc"]),
        end_utc=pd.Timestamp(d["end_utc"]), asof=d["asof"], timeframe=d["timeframe"], feed=d["feed"])
    counts = verify_fetched_bars(DECL, result["bars"])
    boundary = fixed_holdout_boundary(DECL)
    if boundary is not None:  # the attested range, not only the rows, must stay inside the development window
        try:
            attested_end = pd.Timestamp(result["manifest"]["end_utc"])
        except (KeyError, TypeError, ValueError):
            raise SystemExit("fail-closed: the provenance manifest carries no readable end_utc") from None
        if attested_end.tzinfo is None or attested_end > pd.Timestamp(boundary["holdout_start_utc"]):
            raise SystemExit(f"fail-closed: the provenance manifest attests a range ending {attested_end}, past the "
                             f"reserved holdout start {boundary['holdout_start_utc']}")
    paths = write_research_extraction_artifacts(dest, result)
    print("rows", counts, {k: v.name for k, v in paths.items()})


def _resolve_native_identity(strategy: str, sym: str) -> tuple[str, int]:
    """(semantic_fingerprint, required_history_bars) from the native registry; no market data."""
    info = _run_cli("backtest", "native-fingerprint", "--strategy", strategy, "--symbol", sym,
                    *native_bridge_args(DECL))
    return _parse(info, "semantic_fingerprint"), int(_parse(info, "required_history_bars"))


def expected_trial_ids(fingerprints: dict, manifest: dict, decl: dict | None = None) -> list[tuple[str, str, str, dict]]:
    """The predeclared (strategy, symbol, trial_id, identity) for every slot, derived only from the
    declaration, the resolved native fingerprints and the data provenance -- never from a result.
    `decl` defaults to the runner's declaration; a reviewer passes the declaration it is reviewing."""
    from mqk_research.ml.native_signal_registry_integration import build_native_signal_trial_identity
    if decl is None:  # the runner's own (possibly test-narrowed) module state
        decl, experiment, hyp, slots = DECL, EXPERIMENT, HYP, TRIALS
    else:
        experiment = decl["experiment"]["real_experiment_id"]
        hyp = {h["strategy_id"]: h for h in decl["hypotheses"]}
        slots = [(t["strategy_id"], t["symbol"]) for t in decl["universe"]["trials"]]
    part = decl["partition"]
    out = []
    for strategy, sym in slots:
        h, (fingerprint, required) = hyp[strategy], fingerprints[(strategy, sym)]
        trial_id, identity = build_native_signal_trial_identity(
            experiment_id=experiment, hypothesis_id=h["hypothesis_id"], strategy_id=strategy, symbol=sym,
            semantic_fingerprint=fingerprint, required_history_bars=required, bars_provenance=manifest,
            evaluation_start_utc=pd.Timestamp(part["evaluation_start_utc"]), test_months=part["test_months"],
            holdout_months=part["holdout_months"], economic_spec=_economic_spec(decl),
            capital_sizing=research_capital_sizing(decl), stress_contract=research_stress_contract(decl),
            canonical_timeframe_identity=canonical_timeframe_identity(decl),
            fixed_holdout_boundary=fixed_holdout_boundary(decl))
        out.append((strategy, sym, trial_id, identity))
    return out


def registration_gate(store, experiment_id: str, expected: list, *, require_zero_attempts: bool) -> dict:
    """The registry must hold EXACTLY the predeclared trial slots -- no fewer, no more, no duplicate,
    no foreign symbol/strategy/fingerprint/sizing/provenance (any of those changes the trial id) -- and,
    before the first attempt, zero attempts."""
    expected_ids = [e[2] for e in expected]
    if len(set(expected_ids)) != len(expected_ids):
        raise SystemExit("fail-closed: the predeclared contract maps two slots to one trial identity")
    registered = store.list_trials(experiment_id=experiment_id)
    registered_ids = sorted(t["trial_id"] for t in registered)
    if len(registered) != len(expected):
        raise SystemExit(f"fail-closed: {len(registered)} registered trials, the contract requires {len(expected)}")
    if registered_ids != sorted(expected_ids):
        missing = sorted(set(expected_ids) - set(registered_ids))
        unexpected = sorted(set(registered_ids) - set(expected_ids))
        raise SystemExit(f"fail-closed: registered trial identities differ from the predeclared contract "
                         f"(missing {len(missing)}, unexpected {len(unexpected)})")
    by_id = {t["trial_id"]: t for t in registered}
    for strategy, sym, trial_id, identity in expected:
        row = by_id[trial_id]
        canonical = json.dumps(identity, sort_keys=True, separators=(",", ":"))
        if row["strategy_id"] != strategy or row["identity_json"] != canonical:
            raise SystemExit(f"fail-closed: registered trial {trial_id} does not carry the predeclared identity "
                             f"of {strategy}/{sym}")
    attempts = sum(len(store.list_attempts(i)) for i in registered_ids)
    if require_zero_attempts and attempts != 0:
        raise SystemExit(f"fail-closed: {attempts} attempts exist; the registration gate requires zero before the first")
    return {"registered": len(registered), "attempts": attempts}


def _run_registration_gate(*, require_zero_attempts: bool) -> dict:
    from mqk_research.exp_distributed.storage import ResearchResultStore
    if not REGISTRY.exists():  # the store constructor creates directories and a database: verification must not
        raise SystemExit("fail-closed: the batch registry does not exist; nothing to verify")
    manifest = json.loads(MANIFEST.read_text(encoding="utf-8"))
    fingerprints = {(strategy, sym): _resolve_native_identity(strategy, sym) for strategy, sym in TRIALS}
    return registration_gate(ResearchResultStore(REGISTRY), EXPERIMENT,
                             expected_trial_ids(fingerprints, manifest),
                             require_zero_attempts=require_zero_attempts)


@staged("gate")
def stage_gate(_args) -> None:
    """Pre-run gate: every predeclared trial is registered, nothing else is, and no attempt exists."""
    _require_exact_target_protocol()
    result = _run_registration_gate(require_zero_attempts=True)
    print("registration_gate_passed registered", result["registered"], "attempts", result["attempts"])


@staged("register")
def stage_register(_args) -> None:
    """Register every hypothesis and trial. Resolves the native fingerprint from the
    strategy registry only -- no emitter, no Backtest, no market data."""
    from mqk_research.exp_distributed.storage import ResearchResultStore
    from mqk_research.ml.native_signal_registry_integration import register_native_signal_trial
    _require_exact_target_protocol()
    require_development_window(DECL)
    part, manifest = DECL["partition"], json.loads(MANIFEST.read_text(encoding="utf-8"))
    REGISTRY.parent.mkdir(parents=True, exist_ok=True)
    store = ResearchResultStore(REGISTRY)
    if store.list_trials(experiment_id=EXPERIMENT) and not _resume(_args):
        raise SystemExit("fail-closed: the batch registry already holds trials; registration runs once")
    # Under --resume registration converges: trial ids are deterministic, register_trial is idempotent for identical
    # content, and the exact-set / zero-attempt checks below refuse any foreign trial or any attempt.
    index = {}
    for strategy, sym in TRIALS:
        h = HYP[strategy]
        info = _run_cli("backtest", "native-fingerprint", "--strategy", strategy, "--symbol", sym,
                        *native_bridge_args(DECL))
        fingerprint = _parse(info, "semantic_fingerprint")
        required = int(_parse(info, "required_history_bars"))
        if int(_parse(info, "timeframe_secs")) != h["timeframe_secs"] or required != h["required_history_bars"]:
            raise SystemExit(f"fail-closed: {strategy} disagrees with its predeclared timeframe/history requirement")
        trial_id = register_native_signal_trial(
            experiment_id=EXPERIMENT, hypothesis_id=h["hypothesis_id"], strategy_id=strategy, symbol=sym,
            semantic_fingerprint=fingerprint, required_history_bars=required, bars_provenance=manifest,
            economic_spec=_economic_spec(), evaluation_start_utc=pd.Timestamp(part["evaluation_start_utc"]),
            test_months=part["test_months"], holdout_months=part["holdout_months"],
            hypothesis_text=h["economic_rationale"], registry_db=REGISTRY,
            capital_sizing=research_capital_sizing(DECL), stress_contract=research_stress_contract(DECL),
            canonical_timeframe_identity=canonical_timeframe_identity(DECL),
            fixed_holdout_boundary=fixed_holdout_boundary(DECL))
        index[key(strategy, sym)] = {"trial_id": trial_id, "hypothesis_id": h["hypothesis_id"],
                                     "semantic_fingerprint": fingerprint, "required_history_bars": required}
        print(key(strategy, sym), trial_id, fingerprint[:12])
    _save_index(index)
    registered = store.list_trials(experiment_id=EXPERIMENT)
    attempts = sum(len(store.list_attempts(t["trial_id"])) for t in registered)
    if len(registered) != len(TRIALS) or attempts != 0:
        raise SystemExit(f"fail-closed: {len(registered)} registered trials / {attempts} attempts after registration")
    print("registered_unique_trials", len(registered), "attempts", attempts)


INTERRUPTED_REASON = "interrupted_before_terminal_result"


def _reconcile_trial(store, rec: dict) -> bool:
    """Resume semantics for one trial. True = a terminal result is already durable (do NOT evaluate again). A genuine
    failed attempt is terminal (no outcome-dependent retry); a succeeded attempt missing from the index is recovered
    from the registry; a 'started' attempt orphaned by a crash is finalized `failed` with an explicit interruption
    reason (nothing is fabricated) and the trial is then evaluated again as a new attempt of the SAME trial."""
    if "economic_eval_id" in rec or "failed" in rec:
        return True
    attempts = store.list_attempts(rec["trial_id"])
    done = [a for a in attempts if a["status"] == "succeeded"]
    if done:
        a = done[-1]
        paths = json.loads(a["artifact_paths_json"]) if a.get("artifact_paths_json") else {}
        econ_path = paths.get("economic_walk_forward")
        if not econ_path or not Path(econ_path).exists():
            raise SystemExit(f"fail-closed: {rec['trial_id']} has a succeeded attempt whose economic artifact is missing; reconcile manually")
        econ = json.loads(Path(econ_path).read_text(encoding="utf-8"))
        rec.update({"economic_eval_id": a["result_id"], "economic_path": econ_path, "attempt_index": a["attempt_index"],
                    "execution_fidelity": econ["registry"]["execution_fidelity"]})
        return True
    for a in attempts:
        if a["status"] == "started":
            store.finalize_attempt(a["attempt_id"], status="failed", failure_reason=INTERRUPTED_REASON)
    genuine = [a for a in store.list_attempts(rec["trial_id"]) if a["status"] == "failed" and a.get("failure_reason") != INTERRUPTED_REASON]
    if genuine:
        rec["failed"] = genuine[-1].get("failure_reason") or "failed attempt"
        return True
    return False


@staged("trials")
def stage_trials(_args) -> None:
    from mqk_research.exp_distributed.storage import ResearchResultStore
    from mqk_research.ml.native_signal_registry_integration import (
        NativeSignalError, native_holdout_start, research_bars_to_backtest_csv,
        run_registered_native_signal_economic_eval)
    _require_exact_target_protocol()
    require_development_window(DECL)
    part, manifest = DECL["partition"], json.loads(MANIFEST.read_text(encoding="utf-8"))
    store = ResearchResultStore(REGISTRY)
    if len(store.list_trials(experiment_id=EXPERIMENT)) != len(TRIALS):
        raise SystemExit("fail-closed: every predeclared trial must be registered before any attempt or emission")
    _run_registration_gate(require_zero_attempts=False)  # exactly the predeclared identities, retries allowed
    index = _load_index()
    for strategy, sym in TRIALS:  # frozen order; failures never stop the batch
        h, sdir, rec = HYP[strategy], tdir(strategy, sym), index[key(strategy, sym)]
        if _resume(_args) and _reconcile_trial(store, rec):
            continue
        sdir.mkdir(parents=True, exist_ok=True)
        hold = native_holdout_start(BARS, sym, part["holdout_months"], fixed_holdout_boundary(DECL))
        bt = research_bars_to_backtest_csv(BARS, sym, sdir / "bt_bars.csv", end_exclusive_utc=hold)

        def emit(bt=bt, strategy=strategy, sym=sym, sdir=sdir, h=h):
            _run_cli("backtest", "native-signals", "--bars-path", str(bt), "--strategy", strategy, "--symbol", sym,
                     "--timeframe-secs", str(h["timeframe_secs"]), "--out-dir", str(sdir / "emit"),
                     *native_bridge_args(DECL))

        try:
            out = run_registered_native_signal_economic_eval(
                sdir / "run", experiment_id=EXPERIMENT, hypothesis_id=h["hypothesis_id"], strategy_id=strategy,
                symbol=sym, bars_csv=BARS, bars_provenance=manifest, backtest_bars_csv=bt, emit_signals=emit,
                signals_csv=sdir / "emit" / "native_signals.csv", signals_meta_json=sdir / "emit" / "native_signals_meta.json",
                economic_spec=_economic_spec(), evaluation_start_utc=pd.Timestamp(part["evaluation_start_utc"]),
                test_months=part["test_months"], holdout_months=part["holdout_months"], registry_db=REGISTRY,
                expected_timeframe_secs=h["timeframe_secs"], expected_semantic_fingerprint=rec["semantic_fingerprint"],
                required_history_bars=rec["required_history_bars"],
                expected_capital_sizing=research_capital_sizing(DECL),
                expected_stress_contract=research_stress_contract(DECL),
                canonical_timeframe_identity=canonical_timeframe_identity(DECL),
                fixed_holdout_boundary=fixed_holdout_boundary(DECL))
        except NativeSignalError as exc:  # the failed attempt is already durable
            rec["failed"] = str(exc)
            print(key(strategy, sym), "FAILED attempt kept:", str(exc)[:200])
            if _resume(_args):
                _save_index(index)
            continue
        econ = json.loads(out.read_text(encoding="utf-8"))
        if econ["registry"]["trial_id"] != rec["trial_id"]:
            raise SystemExit(f"fail-closed: trial id drift for {key(strategy, sym)}")
        rec.update({"economic_eval_id": econ["ids"]["economic_eval_id"], "economic_path": str(out),
                    "attempt_index": econ["registry"]["attempt_index"],
                    "execution_fidelity": econ["registry"]["execution_fidelity"]})
        print(key(strategy, sym), rec["trial_id"], "attempt", rec["attempt_index"])
        if _resume(_args):
            _save_index(index)
    _save_index(index)


@staged("judge")
def stage_judge(_args) -> None:
    if _resume(_args) and (RUN / "judge" / "judge_sha256.txt").exists() and (RUN / "judge" / "judge.json").exists():
        print("judge already complete (sha file written last); not recomputed")
        return
    from mqk_research.exp_distributed.hashing import canonical_json, sha256_bytes
    from mqk_research.exp_distributed.storage import ResearchResultStore
    from mqk_research.ml.multiple_testing_judge import build_multiple_testing_judge
    registered = ResearchResultStore(REGISTRY).list_trials(experiment_id=EXPERIMENT)
    if len(registered) != len(TRIALS):
        raise SystemExit(f"fail-closed: {len(registered)} registered trials, expected {len(TRIALS)}")
    art = build_multiple_testing_judge(experiment_id=EXPERIMENT, registry_db=REGISTRY)  # whole experiment, hypothesis_id unset
    (RUN / "judge").mkdir(parents=True, exist_ok=True)
    (RUN / "judge" / "judge.json").write_text(json.dumps(art, sort_keys=True, separators=(",", ":")), encoding="utf-8")
    sha = sha256_bytes(canonical_json(art).encode("utf-8"))
    (RUN / "judge" / "judge_sha256.txt").write_text(sha, encoding="utf-8")
    print("judge_status", art["judge_status"], "population", art["registry_population"],
          "included", len(art["included_trial_ids"]), "excluded", art["excluded_trial_ids"], "sha", sha[:12])


@staged("backtest")
def stage_backtest(_args) -> None:
    index = _load_index()
    nb = DECL["native_backtest"]
    for strategy, sym in TRIALS:
        rec = index[key(strategy, sym)]
        if "failed" in rec:
            print(key(strategy, sym), "no succeeded trial; rejected, no backtest evidence")
            continue
        if _resume(_args) and rec.get("backtest_run_id"):
            continue
        out = _run_cli("backtest", "csv", "--bars", str(tdir(strategy, sym) / "bt_bars.csv"), "--strategy", strategy,
                       "--symbol", sym, "--timeframe-secs", str(nb["timeframe_secs"]),
                       "--initial-cash-micros", str(nb["initial_cash_micros"]),
                       *INTEGRITY_ARGS, *sizing_args(DECL),
                       "--out-dir", str(RUN / "backtest" / strategy / sym))
        rec["backtest_run_id"] = _parse(out, "run_id")
        rec["execution_blocked"] = _parse(out, "execution_blocked")
        print(key(strategy, sym), rec["backtest_run_id"], "execution_blocked", rec["execution_blocked"])
        if _resume(_args):
            _save_index(index)
    _save_index(index)


def _capital_fraction_stress_args(strategy: str, sym: str, plan: dict) -> list[str]:
    """Re-run the SAME native decisions through the Rust capital-fraction resolver at the stress
    fraction (an evaluation scenario of the registered trial, not a trial) and return the finalize
    flags that bind that stream. The baseline caps are carried unchanged by `sizing_args`."""
    bps = plan["allocation_fraction_bps"]
    emit_dir = RUN / "stress" / strategy / sym / "emit"
    info = _run_cli("backtest", "native-fingerprint", "--strategy", strategy, "--symbol", sym,
                    *native_bridge_args(DECL, bps))
    fingerprint = _parse(info, "semantic_fingerprint")
    _run_cli("backtest", "native-signals", "--bars-path", str(tdir(strategy, sym) / "bt_bars.csv"),
             "--strategy", strategy, "--symbol", sym, "--timeframe-secs", str(HYP[strategy]["timeframe_secs"]),
             "--out-dir", str(emit_dir), *native_bridge_args(DECL, bps))
    return ["--stress-allocation-fraction-bps", str(bps), "--stress-sizing-scenario-id", plan["scenario_id"],
            "--stress-sizing-signals-csv", str(emit_dir / "native_signals.csv"),
            "--stress-sizing-signals-meta", str(emit_dir / "native_signals_meta.json"),
            "--stress-sizing-expected-semantic-fingerprint", fingerprint]


@staged("finalize")
def stage_finalize(_args) -> None:
    index = _load_index()
    sha = (RUN / "judge" / "judge_sha256.txt").read_text(encoding="utf-8").strip()
    rb = DECL["robustness"]
    st = rb["p7a_p7b_stress"]
    plan = stress_plan(DECL)
    py = sys.executable
    for strategy, sym in TRIALS:
        rec = index[key(strategy, sym)]
        if "failed" in rec or (_resume(_args) and rec.get("finalized")):
            continue
        common = ["--artifact-root", str(RUN / "backtest" / strategy / sym), "--run-id", rec["backtest_run_id"],
                  "--registry-db", str(REGISTRY), "--trial-id", rec["trial_id"]]
        _run_cli("backtest", "finalize-robustness-sensitivity", *common, "--judge-artifact-sha256", sha,
                 "--research-py-root", str(REPO / "research-py"), "--python", py,
                 "--block-counts", ",".join(map(str, rb["block_counts"])),
                 "--dsr-max-sensitivity-range", str(rb["dsr_max_sensitivity_range"]),
                 "--pbo-max-sensitivity-range", str(rb["pbo_max_sensitivity_range"]))
        if plan["mode"] == "capital_fraction":
            stress_exposure_args = _capital_fraction_stress_args(strategy, sym, plan)
        else:
            stress_exposure_args = ["--stress-max-position-notional-usd", str(st["stress_max_position_notional_usd"])]
        _run_cli("backtest", "finalize-p7a-p7b-replay-stress", *common, "--economic-eval-id", rec["economic_eval_id"],
                 "--research-py-root", str(REPO / "research-py"), "--python", py,
                 "--stress-out-dir", str(RUN / "stress" / strategy / sym),
                 "--stress-execution-slippage-bps", str(st["stress_execution_slippage_bps"]),
                 "--stress-execution-volatility-mult-bps", str(st["stress_execution_volatility_mult_bps"]),
                 *stress_exposure_args,
                 "--max-drawdown-ceiling", str(st["max_drawdown_ceiling"]))
        _run_cli("backtest", "finalize-genuine-shuffled-placebo", *common, "--economic-eval-id", rec["economic_eval_id"],
                 "--research-py-root", str(REPO / "research-py"), "--python", py,
                 "--placebo-out-dir", str(RUN / "placebo" / strategy / sym))
        print(key(strategy, sym), "finalized")
        if _resume(_args):
            rec["finalized"] = True
            _save_index(index)


SCAN_REGISTRY_SUPPLEMENT = {
    "XBI": {"instrument_id": "equity:US:XBI", "symbol": "XBI", "asset_class": "equity", "provider": "twelvedata",
            "provider_symbol": "XBI", "venue": "NYSEARCA", "currency": "USD", "enabled": True, "timeframes": ["1D"],
            "notes": "per-run scan identity row; not in config/instruments/equities.json"},
}


@staged("review")
def stage_review(_args) -> None:
    reg = json.loads((REPO / "config" / "instruments" / "equities.json").read_text(encoding="utf-8"))
    symbols = sorted({s for _, s in TRIALS})
    sub = [i for i in reg if i["symbol"] in symbols]
    # XBI is absent from the production instrument registry; the scanner needs an identity row only,
    # so a per-run row in the seeded-universe schema is supplied. Any other absent symbol is refused.
    for sym in symbols:
        if sym not in {i["symbol"] for i in sub} and sym in SCAN_REGISTRY_SUPPLEMENT:
            sub.append(dict(SCAN_REGISTRY_SUPPLEMENT[sym]))
    sub.sort(key=lambda i: i["symbol"])
    assert sorted(i["symbol"] for i in sub) == symbols
    for strategy in STRATEGIES:
        base = RUN / "scan" / strategy
        if _resume(_args) and any((base / "reviews").rglob("review_decisions.json")):
            continue
        base.mkdir(parents=True, exist_ok=True)
        reg_path = base / "registry.json"
        reg_path.write_text(json.dumps(sub, indent=1, sort_keys=True), encoding="utf-8")
        root = base / "bars" / "1D"
        root.mkdir(parents=True, exist_ok=True)
        for sym in symbols:
            (root / f"{sym}_1D.csv").write_bytes((tdir(strategy, sym) / "bt_bars.csv").read_bytes())
        bench = DECL["scanner_review"].get("benchmark_policy")
        bench_args = ["--benchmark-policy", bench] if bench else []
        # Config flags are accepted only under the V2 policy.
        scan_cfg_args = ([*INTEGRITY_ARGS, "--initial-cash-micros", str(DECL["native_backtest"]["initial_cash_micros"])]
                         + sizing_args(DECL) if bench else [])
        out = _run_cli("backtest", "scan-strategies", "--registry", str(reg_path), "--bars-root", str(base / "bars"),
                       "--timeframe", "1D", "--strategy", strategy, "--out-dir", str(base / "scans"), *bench_args, *scan_cfg_args)
        scan_dir = _parse(out, "artifacts_dir")
        out = _run_cli("backtest", "review-scan", "--artifact-dir", scan_dir, "--out-dir", str(base / "reviews"),
                       *bench_args)
        print(strategy, out)


@staged("summary")
def stage_summary(_args) -> None:
    print(INDEX.read_text(encoding="utf-8"))


STAGES = {"check": stage_check, "reuse_data": stage_reuse_data, "fetch": stage_fetch, "register": stage_register,
          "gate": stage_gate,
          "trials": stage_trials,
          "judge": stage_judge, "backtest": stage_backtest, "finalize": stage_finalize, "review": stage_review,
          "summary": stage_summary}


def require_executable_declaration(decl: dict) -> None:
    """A declaration carrying an `execution_gate` runs no stage until the gate is explicitly executable.
    Declarations without the block (closed historical campaigns) are unaffected."""
    gate = decl.get("execution_gate")
    if gate is None:
        if "evidence_grade" in decl:
            raise SystemExit(f"fail-closed: {decl['batch_id']} declares an evidence_grade but carries no execution_gate; "
                             "a graded declaration is never runnable without an explicit executable gate")
        return
    if gate.get("executable") is not True:
        raise SystemExit(f"fail-closed: {decl['batch_id']} is {gate.get('status')} "
                         f"(blocker {gate.get('blocker')}); no stage may run until the declaration is re-issued executable")


def main() -> None:
    ap = argparse.ArgumentParser()
    ap.add_argument("stage", choices=sorted(STAGES))
    ap.add_argument("--execute", action="store_true")
    ap.add_argument("--resume", action="store_true", help="crash-safe convergence: skip what is already terminal")
    args = ap.parse_args()
    require_executable_declaration(DECL)
    STAGES[args.stage](args)


if __name__ == "__main__":
    main()
