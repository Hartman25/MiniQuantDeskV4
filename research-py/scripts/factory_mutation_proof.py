"""Mutation proof for Strategy Factory invariants.

Each mutant replaces exactly one source fragment in place, runs the named focused tests, and MUST turn them red; the
original bytes are then restored and re-hashed. A surviving mutant (tests stay green), a broken fragment or a failed
byte-exact restore fails the run. Usage:  python scripts/factory_mutation_proof.py <set-name>

Fragments use \n for line breaks; they are matched against the file's own line endings.
"""

from __future__ import annotations

import hashlib
import subprocess
import sys
from pathlib import Path

ROOT = Path(__file__).resolve().parents[1]
SRC = "src/mqk_research/strategy_factory/"
T_CAT = "tests/test_strategy_factory_catalog_import.py"
T_INT = "tests/test_strategy_factory_intake.py"
T_AI = "tests/test_strategy_factory_ai_normalize.py"

# (id, file relative to research-py, old fragment (must occur exactly once), new fragment, pytest args)
MUTANTS: dict[str, list[tuple[str, str, str, str, list[str]]]] = {
    "intake": [
        ("CI-1 subset check removed", SRC + "catalog_import.py", "if eid not in entries:\n                    raise CatalogImportError(f\"subset sheet",
         "if False:\n                    raise CatalogImportError(f\"subset sheet", [T_CAT]),
        ("CI-2 entry-sheet formula allowed", SRC + "catalog_import.py", "if ledger[\"formula_cells_in_entry_sheets\"]:", "if False:", [T_CAT]),
        ("CI-3 header match always true", SRC + "catalog_import.py",
         "def _header_ok(profile: CatalogProfile, sheets: Mapping[str, list[list[str]]]) -> bool:\n",
         "def _header_ok(profile: CatalogProfile, sheets: Mapping[str, list[list[str]]]) -> bool:\n    return True\n", [T_CAT]),
        ("CI-4 empty id accepted", SRC + "catalog_import.py", "if not row[idx].strip():", "if False:", [T_CAT]),
        ("CI-5 duplicate id accepted", SRC + "catalog_import.py", "if len(set(ids)) != len(ids):", "if False:", [T_CAT]),
        ("FZ-1 missing parameter defaulted", SRC + "formalize.py", "params[p.name] = _fv(None, FieldClass.UNDERSPECIFIED, \"not stated in source\")",
         "params[p.name] = _fv(1, FieldClass.EXPLICIT_SOURCE_RULE, \"default\")", [T_INT]),
        ("FZ-2 explicit value overridable", SRC + "formalize.py", "or tpl[\"params\"][name][\"value\"] is not None:", "or False:", [T_INT]),
        ("FZ-3 out-of-domain decision accepted", SRC + "formalize.py", "or not spec.lo <= v <= spec.hi:", "or False:", [T_INT]),
        ("FZ-4 futures not a blocker", SRC + "formalize.py", "if asset_class in (\"options\", \"fx\", \"crypto\", \"futures\") or", "if False and (", [T_INT]),
        ("DD-1 partial parameters may be exact", SRC + "dedup.py", "if sig[\"complete\"] else []", "", [T_INT]),
        ("DD-2 order dependent population", SRC + "dedup.py", "for idea in sorted(ideas, key=lambda i: i[\"intake_id\"]):", "for idea in list(ideas):", [T_INT]),
        ("DD-3 novelty review overrides any relationship", SRC + "dedup.py",
         "if rec[\"relationship\"] != Relationship.UNKNOWN_NEEDS_REVIEW.value:\n        raise", "if False:\n        raise", [T_INT]),
        ("AD-1 missing parameters admitted", SRC + "admission.py", "if tpl[\"missing_params\"]:", "if False:", [T_INT]),
        ("AD-2 grammar availability ignored", SRC + "admission.py", "if spec.grammar_v1 and grammar_available:", "if spec.grammar_v1:", [T_INT]),
        ("AD-3 exact duplicate re-admitted", SRC + "admission.py", "if rel == Relationship.EXACT_DUPLICATE.value:", "if False:", [T_INT]),
        ("AD-4 data blocker ignored", SRC + "admission.py", "if \"L\" in blockers or \"D\" in blockers:", "if False:", [T_INT]),
        ("PL-1 source conflict accepted", SRC + "pipeline.py",
         "elif prior[\"provenance\"][\"entry_canonical_hash\"] == idea[\"provenance\"][\"entry_canonical_hash\"]:", "elif True:", [T_INT]),
        ("PL-2 source copy not accounted", SRC + "pipeline.py", "copies.append({\"intake_id\": idea[\"intake_id\"],",
         "(lambda *_: None)({\"intake_id\": idea[\"intake_id\"],", [T_INT]),
        ("TP-1 non-canonical grammar name accepted", SRC + "templates.py", "if grammar_strategy_name(t.template_id, params) != name:", "if False:", [T_INT]),
        ("TP-2 parameter bounds not enforced", SRC + "templates.py", "or not p.lo <= v <= p.hi:", "or False:", [T_INT]),
    ],
    "ai": [
        ("AI-1 evidence not verified", SRC + "ai_normalize.py",
         "if isinstance(evidence, str) and evidence.strip() and _norm(evidence) in norm_source and _value_in_span(name, value, evidence):",
         "if True:", [T_AI]),
        ("AI-2 evidence need not contain the value", SRC + "ai_normalize.py",
         "    return bool(re.search(rf\"(?<!\\d){value}(?!\\d)\", s))", "    return True", [T_AI]),
        ("AI-3 model may override a recognized idea", SRC + "ai_normalize.py",
         "if tid is None or idea[\"kind\"] not in (\"RULE_TEXT_UNMAPPED\", \"UNRECOGNIZED\", \"DIAGNOSTIC_QUESTION\") or not v[\"verified\"]:",
         "if tid is None or not v[\"verified\"]:", [T_AI]),
        ("AI-4 remote ollama endpoint allowed", SRC + "ai_normalize.py", "if not _loopback(base_url):", "if False:", [T_AI]),
        ("AI-5 cloud without cost authorization", SRC + "ai_normalize.py",
         "if not cost_authorization_ref or not cost_authorization_ref.strip():", "if False:", [T_AI]),
        ("AI-6 non-object response accepted", SRC + "ai_normalize.py",
         "if not isinstance(obj, dict):\n        raise ValueError(\"response JSON is not an object\")",
         "if False:\n        raise ValueError(\"response JSON is not an object\")", [T_AI]),
        ("AI-7 unknown template accepted", SRC + "ai_normalize.py",
         "if tid is not None and (tid not in TEMPLATES or tid == \"legacy_engine\"):", "if False:", [T_AI]),
        ("AI-8 out-of-domain value accepted", SRC + "ai_normalize.py",
         "if isinstance(value, bool) or not isinstance(value, int) or not p.lo <= value <= p.hi:", "if False:", [T_AI]),
        ("AI-9 call budget ignored", SRC + "ai_normalize.py", "if calls_left[0] <= 0:", "if False:", [T_AI]),
        ("AI-10 nonconformant backend used", SRC + "ai_normalize.py", "if conformant is False:", "if False:", [T_AI]),
        ("AI-11 response size unbounded", SRC + "ai_normalize.py", "if len(raw.encode(\"utf-8\")) > MAX_RESPONSE_BYTES:", "if False:", [T_AI]),
        ("AI-12 provider fault drops the entry", SRC + "ai_normalize.py",
         "    except ProviderError as exc:\n        rec.update(status=STATUS_FAILED, notes=[str(exc)])\n        return _with_ai(base, rec), rec",
         "    except ProviderError as exc:\n        raise", [T_AI]),
        ("AD-5 unrecognized text not rejected", SRC + "admission.py", "if kind == \"UNRECOGNIZED\":", "if False:", [T_AI]),
    ],
}


def sha(b: bytes) -> str:
    return hashlib.sha256(b).hexdigest()


def run_set(name: str) -> int:
    results, bad = [], 0
    for mid, rel, old, new, tests in MUTANTS[name]:
        path = ROOT / rel
        orig = path.read_bytes()
        text = orig.decode("utf-8")
        nl = "\r\n" if "\r\n" in text else "\n"
        old, new = old.replace("\n", nl), new.replace("\n", nl)
        if text.count(old) != 1:
            print(f"BROKEN MUTANT {mid}: fragment occurs {text.count(old)} times")
            bad += 1
            continue
        try:
            path.write_bytes(text.replace(old, new).encode("utf-8"))
            proc = subprocess.run([sys.executable, "-m", "pytest", *tests, "-q", "-x", "--tb=no", "-p", "no:cacheprovider"],
                                  cwd=ROOT, capture_output=True, text=True)
            killed = proc.returncode != 0
        finally:
            path.write_bytes(orig)
        restored = sha(path.read_bytes()) == sha(orig)
        results.append((mid, killed, restored))
        if not killed or not restored:
            bad += 1
        print(f"{'KILLED  ' if killed else 'SURVIVED'} restored={restored} {mid}")
    print(f"{sum(1 for _, k, _ in results if k)}/{len(MUTANTS[name])} killed, byte-exact restore {all(r for *_, r in results)}")
    return 1 if bad else 0


if __name__ == "__main__":
    if len(sys.argv) < 2 or sys.argv[1] not in MUTANTS:
        print("sets:", ", ".join(MUTANTS))
        raise SystemExit(2)
    raise SystemExit(run_set(sys.argv[1]))
