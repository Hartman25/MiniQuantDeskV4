"""Controlled implementation-and-verification workflow for ideas that no verified executable can express.

The Factory never writes, compiles or admits trading code. For a recognized, fully specified idea that has no verified
engine it emits a deterministic IMPLEMENTATION REQUEST: the exact semantics a reviewer or an authorized development session
must implement and prove. `admission_checklist` then states, with evidence from the repository, whether a named native
strategy is actually ready to be used in a campaign. Generated or LLM-produced code is therefore never admitted because it
compiles: admission needs the engine registered in Rust, its semantic card, deterministic tests, and an explicit
operator authorization record committed to the repository.
"""

from __future__ import annotations

import json
import re
from pathlib import Path
from typing import Any, Mapping

from mqk_research.strategy_factory.contracts import sha
from mqk_research.strategy_factory.templates import CARD_BY_ID, TEMPLATES

REQUEST_SCHEMA = "strategy_factory_implementation_request_v1"
AUTHORIZATION_DIR = Path("docs/research/admissions")
ENGINES = Path("core-rs/crates/mqk-strategy/src/engines")

REQUIRED_PROOFS = (
    "exact integer (i128) arithmetic over completed positive closes; no floating point in a decision",
    "fail closed to flat on a short, incomplete, non-positive or malformed window; an incomplete latest bar can never act",
    "strict/inclusive boundary tests for every comparison, with a mutation proof that each boundary mutant turns a test red",
    "an independent naive reference implementation compared over a deterministic pseudo-random walk",
    "a semantic fingerprint that changes with every behaviour-bearing parameter and is symbol- and timeframe-bound",
    "restart equivalence: replay from bounded history (or a durable-state declaration) equals the continuous stream",
    "registration in REGISTERED_STRATEGY_IDS, a semantic card in templates.NATIVE_CARDS, and a known-index entry",
    "Python/Rust parity: the native CLI fingerprint and required_history_bars equal the Research-side values",
    "no downloaded or generated code is executed to produce or verify the rule",
)


def build_request(idea: Mapping[str, Any]) -> dict[str, Any] | None:
    """A deterministic request for a recognized, fully specified idea that needs a new engine; None otherwise."""
    if idea.get("disposition") != "NEEDS_IMPLEMENTATION":
        return None
    tpl = idea.get("template") or {}
    tid = tpl.get("template_id")
    if tid is None or tpl.get("missing_params"):
        return None
    spec = TEMPLATES[tid]
    params = {n: p["value"] for n, p in tpl["params"].items()}
    body = {
        "schema": REQUEST_SCHEMA, "intake_id": idea["intake_id"], "provenance": idea["provenance"]["catalog_ledger_sha256"],
        "template_id": tid, "family": spec.family.value, "parameters": params,
        "parameter_classes": {n: p["class"] for n, p in tpl["params"].items()}, "direction": idea["direction"]["value"],
        "asset_class": idea["asset_class"]["value"], "stateful": spec.stateful,
        "stateful_requirements": ("durable-restart semantics (BoundedHistoryReconstructible replay or durable held state) must be proven"
                                  if spec.stateful else "stateless: a pure function of the trailing completed closes"),
        "source_rule_text": idea["source_text"]["rule_text"], "reasons": idea["reasons"], "required_proofs": list(REQUIRED_PROOFS),
        "status": "AWAITING_AUTHORIZED_IMPLEMENTATION", "executable_now": False,
        "authority": "A request grants nothing: the engine, its tests and an operator authorization record must exist before any campaign may name it.",
    }
    body["request_sha256"] = sha(body)
    return body


def _registered_ids(repo_root: Path) -> set[str]:
    mod = (repo_root / ENGINES / "mod.rs").read_text(encoding="utf-8")
    block = re.search(r"REGISTERED_STRATEGY_IDS: &\[&str\] = &\[(.*?)\];", mod, re.S)
    ids: set[str] = set()
    for const in re.findall(r"(\w+::(?:SHORT_)?NAME)", block.group(1) if block else ""):
        module, kind = const.split("::")
        src = (repo_root / ENGINES / f"{module}.rs").read_text(encoding="utf-8")
        m = re.search(rf'const {kind}: &str = "([a-z0-9_]+)"', src)
        if m:
            ids.add(m.group(1))
    return ids


def admission_checklist(repo_root: Path, strategy_id: str) -> dict[str, Any]:
    """Evidence-based readiness of a named native strategy for use in a campaign. Reads the repository only."""
    repo_root = Path(repo_root)
    checks: dict[str, dict[str, Any]] = {}

    def put(name: str, ok: bool, detail: str) -> None:
        checks[name] = {"ok": bool(ok), "detail": detail}

    put("semantic_card", strategy_id in CARD_BY_ID, "templates.NATIVE_CARDS entry" if strategy_id in CARD_BY_ID else "no semantic card")
    try:
        registered = strategy_id in _registered_ids(repo_root)
    except OSError as exc:
        registered, _ = False, exc
    put("registered_in_rust", registered, "listed in REGISTERED_STRATEGY_IDS" if registered else "not a registered native identity")
    engine = next((p for p in (repo_root / ENGINES).glob("*.rs") if f'"{strategy_id}"' in p.read_text(encoding="utf-8") and "const NAME" in p.read_text(encoding="utf-8")), None)
    text = engine.read_text(encoding="utf-8") if engine else ""
    put("engine_has_deterministic_tests", bool(engine) and "#[cfg(test)]" in text and "semantic_fingerprint" in text and "#[test]" in text,
        engine.name if engine else "no engine source defines this identity")
    auth = repo_root / AUTHORIZATION_DIR / f"{strategy_id}.json"
    ok_auth = False
    if auth.is_file():
        try:
            rec = json.loads(auth.read_text(encoding="utf-8"))
            ok_auth = all(rec.get(k) for k in ("operator", "approval_ref", "strategy_id")) and rec["strategy_id"] == strategy_id
        except ValueError:
            ok_auth = False
    put("explicit_operator_authorization", ok_auth, str(AUTHORIZATION_DIR / f"{strategy_id}.json") if ok_auth else
        "no committed operator authorization record (docs/research/admissions/<strategy_id>.json with operator, approval_ref, strategy_id)")
    return {"strategy_id": strategy_id, "ready": all(c["ok"] for c in checks.values()), "checks": checks}
