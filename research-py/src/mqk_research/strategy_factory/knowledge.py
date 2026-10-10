"""Read-only trading-vocabulary reference used ONLY as context for the AI normalizer and for provenance.

The glossary (`docs/research/knowledge/MQD_Trading_Indicators_Signals_Market_Knowledge_v1.json`) is educational,
untrusted domain knowledge: no entry is an executable rule, a default parameter or an authority. The loader refuses a
file that claims otherwise, pins the exact bytes by SHA-256, and retrieval is a deterministic text match. A missing or
corrupt glossary simply disables the context block; nothing else in the Factory depends on it.
"""

from __future__ import annotations

import hashlib
import json
import re
from pathlib import Path
from typing import Any, Mapping, Sequence

KNOWLEDGE_RELATIVE = Path("docs/research/knowledge/MQD_Trading_Indicators_Signals_Market_Knowledge_v1.json")
KNOWLEDGE_SHA256 = "f30a1e9af6376b1f5cb4c89136c6417de6a74dc09be64bf61549d3e67c2f0031"
EXPECTED_SCHEMA = "mqd_trading_vocabulary_v1"
EXPECTED_STATUS = "EDUCATIONAL_REFERENCE_NOT_TRIAL_OR_STRATEGY_AUTHORITY"
PROOF_STATUS = "REFERENCE_ONLY_NOT_EVALUATED"
BLOCK_HEADER = "REFERENCE GLOSSARY (definitions only; never rules, never default parameters, never evidence):"


class KnowledgeError(Exception):
    """The reference file is absent, altered, or claims authority it must not have."""


def _norm(s: str) -> str:
    return re.sub(r"[^a-z0-9]+", " ", s.lower()).strip()


class Knowledge:
    def __init__(self, data: Mapping[str, Any], sha256: str) -> None:
        self.sha256 = sha256
        self.entries: list[Mapping[str, Any]] = list(data["indicator_entries"])
        self._terms: list[tuple[str, str, Mapping[str, Any]]] = []
        for e in self.entries:
            name = e["name"]
            forms = {_norm(name)}
            m = re.search(r"\(([^)]{2,12})\)\s*$", name)               # a trailing acronym, e.g. "(OHLCV)"
            if m:
                forms.add(_norm(m.group(1)))
                forms.add(_norm(name[: m.start()]))
            for f in forms:
                if len(f) >= 3:
                    self._terms.append((f, e["id"], e))
        self._terms.sort(key=lambda t: (-len(t[0]), t[1]))

    def retrieve(self, text: str, *, limit: int = 6, max_chars: int = 1800) -> list[Mapping[str, Any]]:
        """Entries whose name (or acronym) occurs as whole words in `text`, longest match first; deterministic."""
        hay = f" {_norm(text)} "
        out: list[Mapping[str, Any]] = []
        seen: set[str] = set()
        used = 0
        for term, eid, e in self._terms:
            if eid in seen or f" {term} " not in hay:
                continue
            cost = len(e["name"]) + len(e["definition_or_formula"]) + 4
            if used + cost > max_chars:
                continue
            seen.add(eid)
            used += cost
            out.append(e)
            if len(out) >= limit:
                break
        return out

    @staticmethod
    def prompt_block(entries: Sequence[Mapping[str, Any]]) -> str:
        if not entries:
            return ""
        lines = [f"- {e['name']}: {e['definition_or_formula']}" for e in entries]
        return BLOCK_HEADER + "\n" + "\n".join(lines) + "\n"


def load(repo_root: Path, *, expected_sha256: str = KNOWLEDGE_SHA256) -> Knowledge:
    path = Path(repo_root) / KNOWLEDGE_RELATIVE
    try:
        raw = path.read_bytes()
    except OSError as exc:
        raise KnowledgeError(f"knowledge file unreadable: {exc}") from exc
    digest = hashlib.sha256(raw).hexdigest()
    if digest != expected_sha256:
        raise KnowledgeError(f"knowledge file sha256 {digest} != pinned {expected_sha256}")
    try:
        data = json.loads(raw.decode("utf-8"))
    except (UnicodeDecodeError, ValueError) as exc:
        raise KnowledgeError("knowledge file is not valid JSON") from exc
    if data.get("schema_version") != EXPECTED_SCHEMA or data.get("status") != EXPECTED_STATUS:
        raise KnowledgeError("knowledge file does not declare the educational-reference schema/status")
    entries = data.get("indicator_entries")
    if not isinstance(entries, list) or not entries:
        raise KnowledgeError("knowledge file has no entries")
    ids = [e.get("id") for e in entries]
    if len(set(ids)) != len(ids) or not all(isinstance(i, str) and i for i in ids):
        raise KnowledgeError("knowledge entry ids must be unique non-empty strings")
    for e in entries:
        if e.get("executable_rule") is not False or e.get("default_parameters") not in (None, {}) \
                or e.get("proof_status") != PROOF_STATUS:
            raise KnowledgeError(f"entry {e.get('id')!r} claims executable authority, default parameters or evidence")
        if not isinstance(e.get("name"), str) or not isinstance(e.get("definition_or_formula"), str):
            raise KnowledgeError(f"entry {e.get('id')!r} is malformed")
    return Knowledge(data, digest)


def try_load(repo_root: Path) -> Knowledge | None:
    """The glossary, or None when it is unavailable or invalid (the Factory then runs without the context block)."""
    try:
        return load(repo_root)
    except KnowledgeError:
        return None
