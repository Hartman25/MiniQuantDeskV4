"""Policy-gated web scout: fetch operator-approved public pages into an untrusted quarantine and present them to the same
intake pipeline as any other catalog.

* Fail closed: with no approved-source policy nothing is fetched. A URL must be https, on an approved domain, under an
  approved path prefix, allowed by that domain's robots.txt for our user agent, within the domain's rate limit and size cap.
* Web content is hostile data. It is parsed with the standard-library HTML parser into plain text (script/style/embedded
  content dropped), hashed, and stored byte-for-byte in quarantine; nothing downloaded is ever executed, imported or
  followed beyond the single approved URL. Prompt injection inside a page is text like any other.
* The scout decides nothing about ideas. It emits a catalog ledger (family `scout:<source_id>`); formalization, optional AI
  normalization, deduplication and admission are the ordinary deterministic stages.
Live network fetching is implemented (`UrllibFetcher`) but is operator-enabled only; tests use a fake fetcher.
"""

from __future__ import annotations

import re
import urllib.error
import urllib.request
from dataclasses import dataclass, field
from html.parser import HTMLParser
from pathlib import Path
from typing import Any, Callable, Mapping, Protocol, Sequence
from urllib import robotparser
from urllib.parse import urlparse

from mqk_research.exp_distributed.hashing import canonical_json, sha256_bytes

USER_AGENT = "MQD-StrategyFactory-Scout/1 (research; honors robots.txt)"
POLICY_SCHEMA = "strategy_factory_source_policy_v1"
ACCEPTED_TYPES = ("text/html", "text/plain", "text/markdown")
MAX_EXCERPT = 4000
SOURCE_CLASSES = ("academic", "official_documentation", "public_forum", "public_repository_docs", "blog", "broker_research")


class ScoutError(Exception):
    """A refusal to fetch or accept content (policy, robots, size, type, rate)."""


@dataclass(frozen=True)
class ApprovedSource:
    source_id: str
    domain: str
    source_class: str
    path_prefix: str = "/"
    rate_limit_seconds: float = 5.0
    max_bytes: int = 2_000_000
    license_note: str = "unspecified: quote sparingly, keep attribution, no redistribution"


@dataclass(frozen=True)
class SourcePolicy:
    sources: tuple[ApprovedSource, ...] = ()

    @staticmethod
    def from_json(obj: Mapping[str, Any]) -> "SourcePolicy":
        if obj.get("schema") != POLICY_SCHEMA:
            raise ScoutError(f"policy schema must be {POLICY_SCHEMA!r}")
        out = []
        for s in obj.get("sources", []):
            a = ApprovedSource(**s)
            if a.source_class not in SOURCE_CLASSES or not re.fullmatch(r"[a-z0-9.-]+\.[a-z]{2,}", a.domain) or not a.path_prefix.startswith("/") \
                    or a.rate_limit_seconds < 1 or not 1 <= a.max_bytes <= 10_000_000 or not re.fullmatch(r"[A-Za-z0-9_.-]{2,40}", a.source_id):
                raise ScoutError(f"invalid approved source {s!r}")
            out.append(a)
        if len({a.source_id for a in out}) != len(out):
            raise ScoutError("duplicate source_id in policy")
        return SourcePolicy(tuple(out))

    def match(self, url: str) -> ApprovedSource:
        p = urlparse(url)
        if p.scheme != "https" or not p.hostname or p.username or p.password or p.port not in (None, 443):
            raise ScoutError("only plain https URLs on the default port without credentials are fetchable")
        for a in self.sources:
            if p.hostname == a.domain and (p.path or "/").startswith(a.path_prefix):
                return a
        raise ScoutError(f"{p.hostname}{p.path} is not an operator-approved source (the default policy is empty: nothing is fetched)")


@dataclass(frozen=True)
class FetchResult:
    status: int
    content_type: str
    body: bytes
    final_url: str


class Fetcher(Protocol):
    def fetch(self, url: str, *, timeout: float, max_bytes: int) -> FetchResult: ...


class UrllibFetcher:
    """Live fetcher: https only, redirects must stay on the same host, bounded body. Operator-enabled; not used in tests."""

    class _SameHost(urllib.request.HTTPRedirectHandler):
        def redirect_request(self, req, fp, code, msg, headers, newurl):
            if urlparse(newurl).hostname != urlparse(req.full_url).hostname or urlparse(newurl).scheme != "https":
                raise urllib.error.URLError("redirect leaves the approved host")
            return super().redirect_request(req, fp, code, msg, headers, newurl)

    def fetch(self, url: str, *, timeout: float, max_bytes: int) -> FetchResult:
        opener = urllib.request.build_opener(self._SameHost)
        req = urllib.request.Request(url, headers={"User-Agent": USER_AGENT, "Accept": ", ".join(ACCEPTED_TYPES)})
        try:
            with opener.open(req, timeout=timeout) as r:                       # noqa: S310 - scheme/host validated by the policy
                return FetchResult(r.status, (r.headers.get("Content-Type") or "").split(";")[0].strip().lower(), r.read(max_bytes + 1), r.geturl())
        except urllib.error.HTTPError as exc:
            return FetchResult(exc.code, "", b"", url)
        except (urllib.error.URLError, TimeoutError, OSError) as exc:
            raise ScoutError(f"transport failure: {exc}") from exc


class _Text(HTMLParser):
    SKIP = {"script", "style", "iframe", "object", "embed", "noscript", "template", "svg", "canvas"}

    def __init__(self) -> None:
        super().__init__(convert_charrefs=True)
        self.parts: list[str] = []
        self.title = ""
        self._skip = 0
        self._in_title = False

    def handle_starttag(self, tag, attrs):
        if tag in self.SKIP:
            self._skip += 1
        elif tag == "title":
            self._in_title = True

    def handle_endtag(self, tag):
        if tag in self.SKIP and self._skip:
            self._skip -= 1
        elif tag == "title":
            self._in_title = False

    def handle_data(self, data):
        if self._skip:
            return
        if self._in_title:
            self.title += data
        else:
            self.parts.append(data)


def extract_text(body: bytes, content_type: str) -> tuple[str, str]:
    """(title, plain text). HTML scripts/styles/embeds are dropped, never run; other text is taken as-is."""
    text = body.decode("utf-8", errors="replace")
    if content_type == "text/html":
        p = _Text()
        p.feed(text)
        p.close()
        return " ".join(p.title.split()), " ".join(" ".join(p.parts).split())
    first = next((ln.strip(" #") for ln in text.splitlines() if ln.strip()), "")
    return first[:200], " ".join(text.split())


@dataclass
class Scout:
    policy: SourcePolicy
    fetcher: Fetcher
    clock: Callable[[], float]
    quarantine_dir: Path
    timeout: float = 20.0
    _last: dict[str, float] = field(default_factory=dict)
    _robots: dict[str, robotparser.RobotFileParser] = field(default_factory=dict)

    def _allowed_by_robots(self, src: ApprovedSource, url: str) -> bool:
        rp = self._robots.get(src.domain)
        if rp is None:
            rp = robotparser.RobotFileParser()
            res = self.fetcher.fetch(f"https://{src.domain}/robots.txt", timeout=self.timeout, max_bytes=500_000)
            if res.status == 200 and res.content_type in ("text/plain", ""):
                rp.parse(res.body.decode("utf-8", errors="replace").splitlines())
            elif res.status in (401, 403):
                rp.disallow_all = True                 # an access-controlled robots.txt means: stay out
            else:
                rp.parse([])                           # absent robots.txt: no restriction is declared
            self._robots[src.domain] = rp
        return rp.can_fetch(USER_AGENT, url)

    def scout_url(self, url: str) -> dict[str, Any]:
        src = self.policy.match(url)
        if not self._allowed_by_robots(src, url):
            raise ScoutError(f"robots.txt of {src.domain} disallows {url}")
        now = self.clock()
        if now - self._last.get(src.domain, -1e18) < src.rate_limit_seconds:
            raise ScoutError(f"rate limit for {src.domain}: wait {src.rate_limit_seconds}s between requests")
        self._last[src.domain] = now
        res = self.fetcher.fetch(url, timeout=self.timeout, max_bytes=src.max_bytes)
        if res.status != 200:
            raise ScoutError(f"{url} returned status {res.status}")
        if urlparse(res.final_url).hostname != src.domain:
            raise ScoutError("the response came from a different host than the approved one")
        if len(res.body) > src.max_bytes:
            raise ScoutError(f"{url} exceeds the {src.max_bytes} byte cap")
        if res.content_type not in ACCEPTED_TYPES:
            raise ScoutError(f"content type {res.content_type!r} is not accepted (text only; binaries and code are never fetched)")
        digest = sha256_bytes(res.body)
        self.quarantine_dir.mkdir(parents=True, exist_ok=True)
        raw_path = self.quarantine_dir / f"{digest}.bin"
        if not raw_path.exists():
            raw_path.write_bytes(res.body)                    # immutable, content-addressed, never executed
        title, text = extract_text(res.body, res.content_type)
        return {"source_id": src.source_id, "source_class": src.source_class, "url": url, "final_url": res.final_url, "sha256": digest,
                "bytes": len(res.body), "content_type": res.content_type, "title": title, "text": text, "license_note": src.license_note,
                "retrieved_at": self.clock(), "quarantine_path": str(raw_path).replace("\\", "/")}


def to_ledger(records: Sequence[Mapping[str, Any]], source_id: str) -> dict[str, Any]:
    """A catalog ledger the ordinary pipeline accepts. One entry per page; the page text is the rule_text, untrusted."""
    entries = []
    for n, r in enumerate(sorted(records, key=lambda r: r["url"]), start=1):
        fields = {"title": r["title"] or r["url"], "hypothesis": "", "mechanism": "", "rule_text": r["text"][:MAX_EXCERPT], "assets": "", "horizon": "",
                  "direction": "", "data_needed": "", "family": r["source_class"], "complexity": "", "risk": "", "negative_control": "", "as_of": "",
                  "source_refs": [r["sha256"][:16]], "source_urls": [r["url"]], "source_finding": "", "source_limitation": r["license_note"],
                  "source_composite_refs": [f"scout:{source_id}:{r['sha256'][:16]}"], "unresolved_source_refs": []}
        eid = f"{source_id}-{r['sha256'][:12]}"
        original = {k: r[k] for k in ("url", "final_url", "sha256", "bytes", "content_type", "title")}
        entries.append({"entry_id": eid, "sheet": "scout", "row_number": n, "entry_kind": "idea", "fields": fields, "original": original,
                        "labels": {"source_class": r["source_class"], "claims": "UNVERIFIED_WEB_CONTENT"}, "views": {},
                        "content_hash": sha256_bytes(canonical_json({"f": fields, "o": original}).encode("utf-8")),
                        "canonical_hash": sha256_bytes(canonical_json({k: fields[k] for k in ("title", "rule_text")}).encode("utf-8"))})
    body = {"schema": "strategy_factory_catalog_ledger_v1", "catalog_family": f"scout:{source_id}", "profile_id": "scout_v1",
            "profile_identity": sha256_bytes(b"scout_v1"),
            "source": {"filename": f"scout-{source_id}", "format": "web", "sha256": sha256_bytes(canonical_json([r["sha256"] for r in records]).encode("utf-8")),
                       "bytes": sum(r["bytes"] for r in records)},
            "entries": entries, "sources": {}, "context_sheets": {}, "formula_cells_in_entry_sheets": {},
            "counts": {"entries": len(entries), "ideas": len(entries), "controls": 0, "sources": 0, "context_sheets": 0},
            "trial_registered": False, "economic_attempt": False, "authority": "UNTRUSTED_IDEA_INTAKE"}
    body["ledger_sha256"] = sha256_bytes(canonical_json(body).encode("utf-8"))
    return body
