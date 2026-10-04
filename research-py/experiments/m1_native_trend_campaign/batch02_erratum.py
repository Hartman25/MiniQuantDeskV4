"""Audit authority for the Batch 02 post-run predeclaration erratum (IR-B02-01).

PREDECLARED_BATCH_02.json is immutable. This module applies the erratum's corrected descriptive values to a
disposable in-memory copy and recomputes every identity and executable input through the real run_batch.py
functions, so "these strings are descriptive" is a measured fact rather than a label.

    python batch02_erratum.py --real [--out proof.json]

re-proves it against the local (git-excluded) Batch 02 run artifacts: no CLI economics, no Research fold, no
Backtest run and no provider call is made, and the run directory is never written.
"""

from __future__ import annotations

import contextlib
import copy
import hashlib
import importlib.util
import json
import os
import re
import shutil
import sys
import tempfile
from pathlib import Path

HERE = Path(__file__).resolve().parent
ORIGINAL_NAME = "PREDECLARED_BATCH_02.json"
ERRATUM_NAME = "PREDECLARED_BATCH_02_ERRATUM.json"
# The digest the independent reviewer observed on the Windows (CRLF) checkout, and the digest of the committed
# LF blob (.gitattributes: *.json eol=lf). They differ by line endings only.
ORIGINAL_SHA256 = "bee592611550ddbcb282a861754eb2e5dd02e2f0f699b84b203b429fb7ffed05"
ORIGINAL_SHA256_LF_BLOB = "6810358253d2d586b527d0a1588044a9ec31178c94485d567a0a074385952b82"
DESCRIPTIVE_MIN_CHARS = 20
_IDENTIFIER_TOKEN = re.compile(r"[A-Za-z0-9_.:+-]+")


def sha256_bytes(raw: bytes) -> str:
    return hashlib.sha256(raw).hexdigest()


def original_bytes_intact(raw: bytes) -> bool:
    """The original declaration's content is exactly the committed one, in either line-ending form."""
    lf_form = raw.replace(b"\r\n", b"\n")
    return (sha256_bytes(lf_form) == ORIGINAL_SHA256_LF_BLOB
            and sha256_bytes(raw) in (ORIGINAL_SHA256, ORIGINAL_SHA256_LF_BLOB))


def load_json(name: str) -> dict:
    return json.loads((HERE / name).read_text(encoding="utf-8"))


def _unescape(token: str) -> str:
    return token.replace("~1", "/").replace("~0", "~")


def pointer_get(doc, pointer: str):
    node = doc
    for token in pointer.split("/")[1:]:
        node = node[int(token)] if isinstance(node, list) else node[_unescape(token)]
    return node


def pointer_set(doc, pointer: str, value) -> None:
    *parents, last = pointer.split("/")[1:]
    node = doc
    for token in parents:
        node = node[int(token)] if isinstance(node, list) else node[_unescape(token)]
    if isinstance(node, list):
        node[int(last)] = value
    else:
        node[_unescape(last)] = value


def leaves(doc, prefix: str = "") -> dict:
    out = {}
    if isinstance(doc, dict):
        for key, value in doc.items():
            out.update(leaves(value, f"{prefix}/{key.replace('~', '~0').replace('/', '~1')}"))
    elif isinstance(doc, list):
        for index, value in enumerate(doc):
            out.update(leaves(value, f"{prefix}/{index}"))
    else:
        out[prefix] = doc
    return out


def corrected_declaration(original: dict, erratum: dict, only: set[str] | None = None) -> dict:
    """A deep copy of `original` with the erratum's corrected descriptive values written at their paths."""
    out = copy.deepcopy(original)
    for field in erratum["stale_fields"]:
        if only is not None and field["id"] not in only:
            continue
        if pointer_get(out, field["json_path"]) != field["original_value"]:
            raise ValueError(f"{field['id']}: the declaration no longer carries the recorded original value")
        pointer_set(out, field["json_path"], field["corrected_value"])
    return out


def descriptive_leaves(doc: dict) -> dict:
    """String leaves that read as a description or layout rather than a bare identifier, hash or timestamp
    (those are frozen as explicit policy values by the audit tests)."""
    return {p: v for p, v in leaves(doc).items()
            if isinstance(v, str) and len(v) >= DESCRIPTIVE_MIN_CHARS and not _IDENTIFIER_TOKEN.fullmatch(v)}


def unreviewed_copied_descriptions(new: dict, old: dict, reviewed: set[str]) -> list[str]:
    """Descriptive leaves of `new` byte-identical to `old`'s at the same path that nobody listed as reviewed.
    Byte-equality with an earlier campaign proves nothing about a description; it only proves it was copied."""
    old_text = descriptive_leaves(old)
    return sorted(p for p, v in descriptive_leaves(new).items() if old_text.get(p) == v and p not in reviewed)


def load_run_batch():
    prior = os.environ.get("MQK_M1_BATCH_DECLARATION")
    os.environ["MQK_M1_BATCH_DECLARATION"] = ORIGINAL_NAME
    try:
        spec = importlib.util.spec_from_file_location("run_batch_erratum_audit", HERE / "run_batch.py")
        module = importlib.util.module_from_spec(spec)
        spec.loader.exec_module(module)
        return module
    finally:
        if prior is None:
            del os.environ["MQK_M1_BATCH_DECLARATION"]
        else:
            os.environ["MQK_M1_BATCH_DECLARATION"] = prior


@contextlib.contextmanager
def using_declaration(rb, decl: dict):
    """Point run_batch's module-level declaration and everything derived from it at `decl`."""
    names = ("DECL", "HYP", "TRIALS", "STRATEGIES", "EXPERIMENT", "RUN", "REGISTRY")
    saved = {n: getattr(rb, n) for n in names}
    rb.DECL = copy.deepcopy(decl)
    rb.HYP = {h["strategy_id"]: h for h in rb.DECL["hypotheses"]}
    rb.TRIALS = [(t["strategy_id"], t["symbol"]) for t in rb.DECL["universe"]["trials"]]
    rb.STRATEGIES = [h["strategy_id"] for h in rb.DECL["hypotheses"]]
    rb.EXPERIMENT = rb.DECL["experiment"]["real_experiment_id"]
    rb.RUN = rb.HERE / rb.DECL["run_dir"]
    rb.REGISTRY = rb.HERE / rb.DECL["experiment"]["registry_db_relative_path"]
    try:
        yield rb
    finally:
        for n, v in saved.items():
            setattr(rb, n, v)


def identity_snapshot(rb, fingerprints: dict, manifest: dict) -> dict:
    """Every identity and executable input run_batch.py, summarize/select_batch.py and holdout_guard.py derive
    from the current `rb.DECL`, as canonical JSON. Only fields those programs actually read are included, so
    equality across a descriptive-only edit means that edit reached none of them."""
    d = rb.DECL
    rb._require_exact_target_protocol()
    rb._require_frozen_trial_structure()
    plan = rb.stress_plan(d)
    stress_bps = plan.get("allocation_fraction_bps")
    nb, p, rob = d["native_backtest"], d["economic_protocol"], d["robustness"]
    bench = d["scanner_review"].get("benchmark_policy")
    snapshot = {
        "hypotheses": [[h["hypothesis_id"], h["strategy_id"], h["timeframe_secs"], h["required_history_bars"],
                        h["economic_rationale"]] for h in d["hypotheses"]],
        "trials": [[t["order"], t["strategy_id"], t["symbol"]] for t in d["universe"]["trials"]],
        "symbols": d["universe"]["symbols"],
        "max_trials": d["universe"]["max_trials"],
        "trial_ids": [[s, y, trial_id, identity] for s, y, trial_id, identity in rb.expected_trial_ids(fingerprints, manifest)],
        "sizing_args": rb.sizing_args(d),
        "research_capital_sizing": rb.research_capital_sizing(d),
        "native_bridge_args": rb.native_bridge_args(d),
        "stress_plan": plan,
        "stress_native_bridge_args": rb.native_bridge_args(d, stress_bps) if stress_bps is not None else None,
        "economic_spec": repr(rb._economic_spec()),
        "economic_inputs": {k: p[k] for k in ("signal_policy", "cost_model", "execution_pricing", "weight_to_share",
                                              "annualization")},
        "benchmark": {
            "scanner_policy": bench,
            "policy_id": d["benchmark"]["policy_id"],
            "scan_review_args": ["--benchmark-policy", bench],
            "scan_strategies_config_args": [*rb.INTEGRITY_ARGS, "--initial-cash-micros", str(nb["initial_cash_micros"]),
                                            *rb.sizing_args(d)],
        },
        "backtest_csv_args": ["--timeframe-secs", str(nb["timeframe_secs"]), "--initial-cash-micros",
                              str(nb["initial_cash_micros"]), *rb.INTEGRITY_ARGS, *rb.sizing_args(d)],
        "finalize": {k: rob[k] for k in ("block_counts", "dsr_max_sensitivity_range", "pbo_max_sensitivity_range")}
        | {k: rob["p7a_p7b_stress"][k] for k in ("stress_execution_slippage_bps", "stress_execution_volatility_mult_bps",
                                                  "max_drawdown_ceiling")},
        "run": {"run_dir": d["run_dir"], "registry": d["experiment"]["registry_db_relative_path"],
                "experiment": d["experiment"]["real_experiment_id"],
                "partition": {k: d["partition"][k] for k in ("evaluation_start_utc", "test_months", "holdout_months")}},
        "selection": {"promotion_policy": d["promotion_policy"], "max_trials": d["universe"]["max_trials"]},
        "data_pin": d["data"]["reuse_verified_data_from"],
    }
    return json.loads(json.dumps(snapshot, sort_keys=True))


def snapshot_under(rb, decl: dict, fingerprints: dict, manifest: dict) -> dict:
    with using_declaration(rb, decl):
        return identity_snapshot(rb, fingerprints, manifest)


def snapshot_diff(a: dict, b: dict) -> list[str]:
    return sorted(k for k in a.keys() | b.keys() if a.get(k) != b.get(k))


# ---------------------------------------------------------------------------------------------------------
# Local real-artifact proof (git-excluded run directory; read-only).
# ---------------------------------------------------------------------------------------------------------

_RUN_FILES = ("batch_outcome.json", "batch_results.json", "trials_index.json", "judge/judge.json",
              "judge/judge_sha256.txt", "holdout_guard_post.json", "registration_proof_before_first_attempt.json")


def _run_hashes(run: Path) -> dict:
    return {name: sha256_bytes((run / name).read_bytes()) for name in _RUN_FILES}


def real_proof() -> dict:
    sys.path.insert(0, str(HERE.parents[1] / "src"))
    from mqk_research.exp_distributed.storage import ResearchResultStore
    import holdout_guard

    rb = load_run_batch()
    raw = (HERE / ORIGINAL_NAME).read_bytes()
    erratum, original = load_json(ERRATUM_NAME), json.loads(raw)
    if not original_bytes_intact(raw) or erratum["original_predeclaration_sha256"] != ORIGINAL_SHA256:
        raise SystemExit("PREDECLARATION_BYTES_CHANGED")
    corrected = corrected_declaration(original, erratum)
    run = HERE / original["run_dir"]
    before = _run_hashes(run)
    index = json.loads((run / "trials_index.json").read_text(encoding="utf-8"))
    manifest = json.loads((run / "data" / "research_bars_provenance.json").read_text(encoding="utf-8"))
    fingerprints = {(t["strategy_id"], t["symbol"]): (index[f"{t['strategy_id']}/{t['symbol']}"]["semantic_fingerprint"],
                                                        index[f"{t['strategy_id']}/{t['symbol']}"]["required_history_bars"])
                    for t in original["universe"]["trials"]}
    recorded_ids = [index[f"{t['strategy_id']}/{t['symbol']}"]["trial_id"] for t in original["universe"]["trials"]]

    snap_original = snapshot_under(rb, original, fingerprints, manifest)
    snap_corrected = snapshot_under(rb, corrected, fingerprints, manifest)
    reproduced = [row[2] for row in snap_original["trial_ids"]]
    per_field = {}
    for field in erratum["stale_fields"]:
        solo = snapshot_under(rb, corrected_declaration(original, erratum, {field["id"]}), fingerprints, manifest)
        per_field[field["id"]] = {"json_path": field["json_path"], "snapshot_equal": solo == snap_original}

    with tempfile.TemporaryDirectory() as tmp:  # never open the live registry for writing
        registry_copy = Path(tmp) / "research.sqlite3"
        shutil.copyfile(HERE / original["experiment"]["registry_db_relative_path"], registry_copy)
        store = ResearchResultStore(registry_copy)
        with using_declaration(rb, corrected):
            expected = rb.expected_trial_ids(fingerprints, manifest)
            inventory = rb.registration_gate(store, rb.EXPERIMENT, expected, require_zero_attempts=False)
        attempts = {t["trial_id"]: len(store.list_attempts(t["trial_id"]))
                    for t in store.list_trials(experiment_id=original["experiment"]["real_experiment_id"])}

    hold_original = holdout_guard.check(original, run, HERE / original["experiment"]["registry_db_relative_path"], "post")
    hold_corrected = holdout_guard.check(corrected, run, HERE / original["experiment"]["registry_db_relative_path"], "post")
    after = _run_hashes(run)

    proof = {
        "original_sha256": sha256_bytes(raw),
        "original_sha256_expected": ORIGINAL_SHA256,
        "corrected_paths": [f["json_path"] for f in erratum["stale_fields"]],
        "snapshot_keys_compared": sorted(snap_original),
        "snapshot_original_equals_corrected": snap_original == snap_corrected,
        "snapshot_sha256": sha256_bytes(json.dumps(snap_original, sort_keys=True).encode("utf-8")),
        "per_field_snapshot_equal": per_field,
        "trial_ids_recorded": recorded_ids,
        "trial_ids_reproduced_from_recorded_fingerprints_and_provenance": reproduced,
        "trial_ids_reproduce_recorded": reproduced == recorded_ids,
        "trial_ids_digest": sha256_bytes("\n".join(recorded_ids).encode("utf-8")),
        "trial_ids_digest_matches_erratum": sha256_bytes("\n".join(recorded_ids).encode("utf-8"))
        == erratum["recorded_identity"]["trial_ids_in_declared_order_sha256"],
        "registration_inventory_under_corrected_declaration": inventory,
        "attempts_per_trial_in_registry_copy": attempts,
        "holdout_guard_post_original_equals_corrected": hold_original == hold_corrected,
        "holdout_guard_post_equals_recorded": hold_original == json.loads(
            (run / "holdout_guard_post.json").read_text(encoding="utf-8")),
        "run_artifact_sha256_before": before,
        "run_artifact_sha256_after": after,
        "run_artifacts_unchanged": before == after,
        "economic_attempts_rerun": False,
        "cli_invoked": False,
    }
    proof["ok"] = all([proof["snapshot_original_equals_corrected"], proof["trial_ids_reproduce_recorded"],
                       proof["trial_ids_digest_matches_erratum"], proof["run_artifacts_unchanged"],
                       proof["holdout_guard_post_original_equals_corrected"], proof["holdout_guard_post_equals_recorded"],
                       all(v["snapshot_equal"] for v in per_field.values()),
                       inventory["registered"] == 15])
    return proof


def main() -> None:
    if "--real" not in sys.argv:
        raise SystemExit("usage: batch02_erratum.py --real [--out PATH]")
    proof = real_proof()
    text = json.dumps(proof, indent=1, sort_keys=True)
    if "--out" in sys.argv:
        Path(sys.argv[sys.argv.index("--out") + 1]).write_text(text + "\n", encoding="utf-8")
    print(text)
    if not proof["ok"]:
        raise SystemExit("BATCH02_PREDECLARATION_ECONOMIC_CONTRADICTION")


if __name__ == "__main__":
    main()
