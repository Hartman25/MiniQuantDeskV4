"""Canonical Strategy Factory entrypoint: one facade over intake, the control-plane store, the compiler and the scheduler.

It coordinates existing owners (ResearchResultStore through the stage-authorized runner, the Rust native engines, the
judge, scanner/review). It adds no economics, no trial identity and no authority: every effectful decision remains with
the operator (gate release, stage authorization) and with the accepted deterministic code.
"""

from __future__ import annotations

import json
import subprocess
from pathlib import Path
from typing import Any, Callable, Mapping, Sequence

from mqk_research.strategy_factory import ai_normalize, campaign as campaign_mod, catalog_import, knowledge as knowledge_mod, pipeline
from mqk_research.strategy_factory.contracts import sha
from mqk_research.strategy_factory.executor import StageExecutor
from mqk_research.strategy_factory.formalize import formalize_entry
from mqk_research.strategy_factory.known_index import build_index
from mqk_research.strategy_factory.scheduler import PassResult, run_pass, run_until_idle
from mqk_research.strategy_factory.store import FactoryStore, StoreError

GRAMMAR_PROBE = "grammar_v1__sma_trend_gate__window_2"


def detect_grammar(cli_path: Path | None) -> bool:
    """True only if the configured native binary really resolves a grammar_v1 strategy (a read-only identity probe)."""
    if cli_path is None or not Path(cli_path).is_file():
        return False
    try:
        r = subprocess.run([str(cli_path), "backtest", "native-fingerprint", "--strategy", GRAMMAR_PROBE, "--symbol", "SPY"],
                           capture_output=True, text=True, timeout=60)
    except (OSError, subprocess.SubprocessError):
        return False
    return r.returncode == 0 and f"strategy={GRAMMAR_PROBE}" in r.stdout


class FactoryService:
    def __init__(self, root: Path, repo_root: Path, *, cli_path: Path | None = None, env: Mapping[str, str] | None = None,
                 grammar_available: bool | None = None) -> None:
        self.root, self.repo_root = Path(root).resolve(), Path(repo_root).resolve()
        self.store = FactoryStore(self.root / "factory.sqlite3")
        self.cli_path = Path(cli_path) if cli_path else None
        self.env = env
        self.grammar_available = detect_grammar(self.cli_path) if grammar_available is None else grammar_available
        self.knowledge = knowledge_mod.try_load(self.repo_root)

    # ------------------------------------------------------------------ intake
    def import_catalog(self, path: Path, profile: catalog_import.CatalogProfile | None = None) -> dict[str, Any]:
        data = Path(path).read_bytes()
        ledger = catalog_import.import_catalog(data, Path(path).name, profile=profile)
        new = self.store.record_import(ledger)
        return {"ledger_sha256": ledger["ledger_sha256"], "family": ledger["catalog_family"], "profile_id": ledger["profile_id"],
                "entries": ledger["counts"]["entries"], "new": new}

    def run_intake(self, provider: ai_normalize.Provider | None = None, *, max_calls: int | None = None) -> dict[str, Any]:
        """Deterministic by default; with a provider, the AI proposes and the deterministic validator decides."""
        ledgers = self.store.list_imports()
        if not ledgers:
            raise StoreError("no catalog has been imported")
        known = build_index(self.repo_root)
        conf, calls, norms = None, [max_calls] if max_calls is not None else None, []
        if provider is not None:
            conf, _ = ai_normalize.conformance(provider)

        def formalizer(entry: Mapping[str, Any], ledger: Mapping[str, Any]) -> dict[str, Any]:
            if provider is None:
                return formalize_entry(entry, ledger)
            idea, rec = ai_normalize.normalize_entry(entry, ledger, provider, conformant=conf, calls_left=calls, knowledge=self.knowledge)
            norms.append(rec)
            return idea

        decisions = [d for kind in ("parameter", "field") for d in self.store.decisions(kind)]
        reviews = self.store.decisions("novelty")
        out = pipeline.process(ledgers, known, grammar_available=self.grammar_available, decisions=decisions, reviews=reviews,
                               formalizer=formalizer)
        added = self.store.record_ideas(out["records"])
        for rec in norms:
            self.store.record_normalization(rec)
        statuses: dict[str, int] = {}
        for r in norms:
            statuses[r["status"]] = statuses.get(r["status"], 0) + 1
        return {"result": {k: v for k, v in out.items() if k != "records"}, "ideas_versions_added": added,
                "ai": {"configured": provider is not None, "conformant": conf, "status_counts": dict(sorted(statuses.items()))}}

    # ------------------------------------------------------------------ campaigns
    def compile_campaign(self, spec: Mapping[str, Any]) -> dict[str, Any]:
        ideas = self.store.latest_ideas()
        compiled = campaign_mod.compile_campaign(spec, repo_root=self.repo_root, run_root=self.root / "campaigns", ideas=ideas,
                                                 grammar_available=self.grammar_available)
        cid = spec["campaign_id"]
        cdir = self.root / "campaigns" / cid
        cdir.mkdir(parents=True, exist_ok=True)
        decl_path = cdir / "declaration.json"
        body = json.dumps(compiled.declaration, indent=1, sort_keys=True)
        if decl_path.exists():
            existing = json.loads(decl_path.read_text(encoding="utf-8"))
            if campaign_mod.declaration_identity(existing) != compiled.declaration_sha256:
                raise StoreError(f"{decl_path} exists with a different declaration identity; a predeclaration is immutable")
        else:
            decl_path.write_text(body, encoding="utf-8")
        (cdir / "spec.json").write_text(json.dumps(spec, indent=1, sort_keys=True), encoding="utf-8")
        created = self.store.create_campaign(campaign_id=cid, spec=spec, declaration_sha256=compiled.declaration_sha256,
                                             declaration_path=str(decl_path).replace("\\", "/"), run_dir=compiled.declaration["run_dir"],
                                             evidence_grade=spec["evidence_grade"], trials=compiled.trials)
        return {"campaign_id": cid, "created": created, "declaration_sha256": compiled.declaration_sha256, "trials": len(compiled.trials),
                "declaration_path": str(decl_path)}

    def release_gate(self, campaign_id: str, *, operator: str, approval_ref: str) -> dict[str, Any]:
        """OPERATOR command: re-issue the declaration with its execution gate released. The gate is excluded from the declaration
        identity (accepted design), so this changes no identity; it is NOT an authorization (stages still need a signed one)."""
        if not operator.strip() or not approval_ref.strip():
            raise StoreError("a gate release names an operator and an approval reference")
        c = self.store.get_campaign(campaign_id)
        path = Path(c["declaration_path"])
        decl = json.loads(path.read_text(encoding="utf-8"))
        decl["execution_gate"] = {"status": "RELEASED_BY_OPERATOR", "executable": True, "blocker": None, "operator": operator,
                                  "approval_ref": approval_ref,
                                  "rule": "Released; every effectful stage still requires a signed stage authorization bound to this declaration identity."}
        if campaign_mod.declaration_identity(decl) != c["declaration_sha256"]:
            raise StoreError("gate release would change the declaration identity: refused")
        path.write_text(json.dumps(decl, indent=1, sort_keys=True), encoding="utf-8")
        return {"campaign_id": campaign_id, "gate": "RELEASED", "declaration_sha256": c["declaration_sha256"]}

    # ------------------------------------------------------------------ execution
    def executor(self, **kw: Any) -> StageExecutor:
        return StageExecutor(self.repo_root, cli_path=self.cli_path, env=self.env, **kw)

    def readiness(self, campaign_id: str) -> dict[str, Any]:
        return self.executor().readiness(self.store.get_campaign(campaign_id))

    def run(self, *, workers: int = 2, until_idle: bool = True, **kw: Any) -> PassResult:
        ex = self.executor(**{k: kw.pop(k) for k in ("stage_timeout",) if k in kw})
        return (run_until_idle if until_idle else run_pass)(self.store, ex, workers=workers, **kw)

    def status(self) -> dict[str, Any]:
        return self.store.snapshot()

    def report(self, campaign_id: str) -> Path:
        from mqk_research.strategy_factory.reporting import write_campaign_report
        return write_campaign_report(self.store.get_campaign(campaign_id), self.repo_root, self.store)
