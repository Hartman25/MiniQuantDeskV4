"""`mqk-factory`: operator command line for the Strategy Factory (also `python -m mqk_research.strategy_factory`).

Read-only commands: status, ideas, campaign readiness/report, ai probe. Effectful commands only write the Factory's own
SQLite file and campaign directory; running stages additionally requires the operator-released gate and a signed stage
authorization, which this CLI neither creates nor bypasses.
"""

from __future__ import annotations

import argparse
import json
import os
import sys
from pathlib import Path
from typing import Any, Sequence

from mqk_research.strategy_factory import ai_normalize
from mqk_research.strategy_factory.campaign import CampaignError
from mqk_research.strategy_factory.scout import ScoutError
from mqk_research.strategy_factory.catalog_import import CatalogImportError, CatalogProfile
from mqk_research.strategy_factory.service import FactoryService
from mqk_research.strategy_factory.store import StoreError

EXIT_OK, EXIT_REFUSED, EXIT_BLOCKED, EXIT_FAILED = 0, 2, 3, 4


def _repo_root() -> Path:
    here = Path(__file__).resolve()
    for p in here.parents:
        if (p / "CLAUDE.md").is_file() and (p / "research-py").is_dir():
            return p
    raise SystemExit("cannot locate the repository root")


def _root(args: argparse.Namespace) -> Path:
    return Path(args.root or os.environ.get("MQK_FACTORY_ROOT") or _repo_root() / "research-py" / "runs" / "strategy_factory")


def _service(args: argparse.Namespace) -> FactoryService:
    cli = args.cli or os.environ.get("MQK_FACTORY_CLI") or os.environ.get("MQK_M1_CLI")
    return FactoryService(_root(args), _repo_root(), cli_path=Path(cli) if cli else None)


def _emit(obj: Any) -> None:
    print(json.dumps(obj, indent=1, sort_keys=True, ensure_ascii=False))


def _provider(args: argparse.Namespace):
    if not getattr(args, "ollama_model", None):
        return None
    return ai_normalize.OllamaProvider(args.ollama_model, args.ollama_url or "http://127.0.0.1:11434")


def build_parser() -> argparse.ArgumentParser:
    ap = argparse.ArgumentParser(prog="mqk-factory", description=__doc__.splitlines()[0])
    ap.add_argument("--root", help="Factory state directory (default research-py/runs/strategy_factory or $MQK_FACTORY_ROOT)")
    ap.add_argument("--cli", help="native mqk-cli binary (or $MQK_FACTORY_CLI)")
    sub = ap.add_subparsers(dest="cmd", required=True)

    p = sub.add_parser("import-catalog", help="hash-bind and import operator-supplied catalogs (xlsx/csv); registers nothing")
    p.add_argument("files", nargs="+", type=Path)
    p.add_argument("--profile", type=Path, help="operator profile JSON for a new schema (otherwise a built-in must match)")

    p = sub.add_parser("scout", help="fetch operator-approved public pages into quarantine and import them (fail-closed policy)")
    p.add_argument("--policy", type=Path, required=True, help="approved-source policy JSON (the default policy is empty: nothing is fetched)")
    p.add_argument("urls", nargs="+")

    p = sub.add_parser("intake", help="formalize, deduplicate and disposition every imported entry (deterministic; AI optional)")
    p.add_argument("--ollama-model")
    p.add_argument("--ollama-url")
    p.add_argument("--max-ai-calls", type=int)

    p = sub.add_parser("ideas", help="list the latest idea records")
    p.add_argument("--disposition")

    p = sub.add_parser("decide", help="record an operator decision (parameter | field | novelty)")
    p.add_argument("kind", choices=("parameter", "field", "novelty"))
    p.add_argument("file", type=Path)

    c = sub.add_parser("campaign", help="campaign lifecycle")
    csub = c.add_subparsers(dest="ccmd", required=True)
    x = csub.add_parser("compile", help="spec -> frozen predeclaration + trial population")
    x.add_argument("spec", type=Path)
    x = csub.add_parser("release", help="OPERATOR: release the execution gate (not an authorization)")
    x.add_argument("campaign_id")
    x.add_argument("--operator", required=True)
    x.add_argument("--approval-ref", required=True)
    for name in ("readiness", "report", "show"):
        x = csub.add_parser(name)
        x.add_argument("campaign_id")

    r = sub.add_parser("run", help="drain eligible work (scheduler pass)")
    r.add_argument("--workers", type=int, default=2)
    r.add_argument("--once", action="store_true", help="a single pass instead of until idle")
    r.add_argument("--max-jobs", type=int)

    st = sub.add_parser("status", help="queue and campaign snapshot (read-only; never creates a store)")
    st.add_argument("--export", type=Path, help="also write the snapshot atomically to this file for an operator surface to poll")
    r = sub.add_parser("retry", help="OPERATOR: retry an infrastructure-failed stage (appends an attempt)")
    r.add_argument("campaign_id")
    r.add_argument("stage")
    r.add_argument("--reason", required=True)

    im = sub.add_parser("implementation", help="controlled implementation workflow (never generates or admits code)")
    isub = im.add_subparsers(dest="icmd", required=True)
    x = isub.add_parser("request", help="deterministic implementation request for a recognized, fully specified idea that has no engine")
    x.add_argument("intake_id")
    x = isub.add_parser("check", help="evidence-based admission readiness of a named native strategy")
    x.add_argument("strategy_id")

    a = sub.add_parser("ai", help="AI backend checks")
    asub = a.add_subparsers(dest="acmd", required=True)
    x = asub.add_parser("probe", help="probe a local Ollama model and run the golden extraction conformance check")
    x.add_argument("--ollama-model", required=True)
    x.add_argument("--ollama-url")
    return ap


def main(argv: Sequence[str] | None = None) -> int:
    args = build_parser().parse_args(argv)
    try:
        if args.cmd == "ai":
            prov = _provider(args)
            probe = prov.probe()
            ok, detail = ai_normalize.conformance(prov) if probe.available else (False, probe.detail)
            _emit({"available": probe.available, "provider": probe.provider, "model": probe.model, "version": probe.version,
                   "conformant": ok, "detail": detail, "label": "FUNCTIONAL" if ok else "BLOCKED_DEPENDENCY"})
            return EXIT_OK if ok else EXIT_BLOCKED
        if args.cmd == "status":
            from mqk_research.strategy_factory.status import build_status, write_status
            db = _root(args) / "factory.sqlite3"
            snap = build_status(db)
            if args.export:
                write_status(db, args.export)
            _emit(snap)
            return EXIT_OK if snap["truth_state"] == "active" else EXIT_BLOCKED
        svc = _service(args)
        if args.cmd == "import-catalog":
            prof = CatalogProfile.from_json(json.loads(args.profile.read_text(encoding="utf-8"))) if args.profile else None
            _emit([svc.import_catalog(f, prof) for f in args.files])
        elif args.cmd == "implementation":
            from mqk_research.strategy_factory import implementation
            if args.icmd == "request":
                idea = svc.store.latest_ideas().get(args.intake_id)
                req = implementation.build_request(idea) if idea else None
                if req is None:
                    raise ValueError("no implementation request: the idea is unknown, not NEEDS_IMPLEMENTATION, or not fully specified")
                _emit(req)
            else:
                rep = implementation.admission_checklist(_repo_root(), args.strategy_id)
                _emit(rep)
                return EXIT_OK if rep["ready"] else EXIT_BLOCKED
        elif args.cmd == "scout":
            _emit(svc.scout(args.urls, json.loads(args.policy.read_text(encoding="utf-8"))))
        elif args.cmd == "intake":
            _emit(svc.run_intake(_provider(args), max_calls=args.max_ai_calls))
        elif args.cmd == "ideas":
            ideas = svc.store.latest_ideas().values()
            _emit([{k: i[k] for k in ("intake_id", "entry_id", "catalog_family", "proposal_kind", "disposition", "coarse_disposition", "reasons")}
                   | {"title": i["source_text"]["title"]} for i in ideas if not args.disposition or i["disposition"] == args.disposition])
        elif args.cmd == "decide":
            payload = json.loads(args.file.read_text(encoding="utf-8"))
            _emit({"recorded": svc.store.record_decision(args.kind if args.kind != "novelty" else "novelty", payload)})
        elif args.cmd == "campaign":
            if args.ccmd == "compile":
                _emit(svc.compile_campaign(json.loads(args.spec.read_text(encoding="utf-8"))))
            elif args.ccmd == "release":
                _emit(svc.release_gate(args.campaign_id, operator=args.operator, approval_ref=args.approval_ref))
            elif args.ccmd == "readiness":
                rep = svc.readiness(args.campaign_id)
                _emit(rep)
                return EXIT_OK if rep["ready"] else EXIT_BLOCKED
            elif args.ccmd == "report":
                print(svc.report(args.campaign_id))
            else:
                c = svc.store.get_campaign(args.campaign_id)
                _emit({"campaign": {k: c[k] for k in ("campaign_id", "state", "state_reason", "evidence_grade", "declaration_sha256", "run_dir")},
                       "jobs": [{"stage": j["stage"], "status": j["status"], "attempts": j["attempt_count"], "reason": j["last_reason"]}
                                for j in svc.store.list_jobs(args.campaign_id)]})
        elif args.cmd == "run":
            res = svc.run(workers=args.workers, until_idle=not args.once, max_jobs=args.max_jobs)
            _emit(res.as_dict())
            return {"NO_ELIGIBLE_WORK": EXIT_OK, "BLOCKED": EXIT_BLOCKED, "FAILED": EXIT_FAILED}.get(res.ended, EXIT_OK)
        elif args.cmd == "retry":
            svc.store.retry_failed(args.campaign_id, args.stage, args.reason)
            _emit({"requeued": [args.campaign_id, args.stage]})
        return EXIT_OK
    except (CatalogImportError, CampaignError, ScoutError, StoreError, ValueError, ai_normalize.ProviderError) as exc:
        print(f"REFUSED: {exc}", file=sys.stderr)
        return EXIT_REFUSED


if __name__ == "__main__":
    raise SystemExit(main())
