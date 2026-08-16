from __future__ import annotations

import re
from pathlib import Path

from .config import Settings
from .security import AccessDenied, ensure_text_file, resolve_repo_path

_SKIP_DIRS = {".git", "target", "node_modules", ".venv", "venv", "__pycache__", ".pytest_cache"}
_TEXT_SUFFIXES = {
    ".rs", ".py", ".toml", ".yaml", ".yml", ".json", ".md", ".txt", ".ps1",
    ".sh", ".ts", ".tsx", ".js", ".jsx", ".css", ".html", ".sql", ".csv",
}


def _relative(root: Path, path: Path) -> str:
    return path.relative_to(root).as_posix()


def list_files(settings: Settings, path: str = ".", *, recursive: bool = False, limit: int = 500) -> list[str]:
    base = resolve_repo_path(settings.repo_root, path)
    if not base.is_dir():
        raise AccessDenied("list_files path must be a directory")
    limit = max(1, min(int(limit), 2000))

    out: list[str] = []
    iterator = base.rglob("*") if recursive else base.iterdir()
    for item in iterator:
        try:
            rel = item.relative_to(settings.repo_root)
        except ValueError:
            continue
        if any(part in _SKIP_DIRS for part in rel.parts):
            continue
        if item.is_symlink():
            try:
                item.resolve(strict=True).relative_to(settings.repo_root)
            except (FileNotFoundError, ValueError):
                continue
        out.append(_relative(settings.repo_root, item))
        if len(out) >= limit:
            break
    return sorted(out)


def read_file(settings: Settings, path: str, start_line: int = 1, end_line: int | None = None) -> str:
    target = resolve_repo_path(settings.repo_root, path)
    ensure_text_file(target, max_bytes=settings.max_file_bytes)
    start = max(1, int(start_line))
    lines = target.read_text(encoding="utf-8", errors="strict").splitlines()
    end = len(lines) if end_line is None else min(len(lines), max(start, int(end_line)))
    selected = lines[start - 1 : end]
    rendered = "\n".join(f"{idx}: {line}" for idx, line in enumerate(selected, start=start))
    if len(rendered) > settings.max_output_chars:
        raise AccessDenied("requested line range exceeds MCP output limit")
    return rendered


def _iter_search_files(settings: Settings, path: str = "."):
    base = resolve_repo_path(settings.repo_root, path)
    roots = [base] if base.is_file() else base.rglob("*")
    for candidate in roots:
        if not candidate.is_file():
            continue
        rel = candidate.relative_to(settings.repo_root)
        if any(part in _SKIP_DIRS for part in rel.parts):
            continue
        if candidate.suffix.lower() not in _TEXT_SUFFIXES and candidate.name not in {"Cargo.toml", "pyproject.toml"}:
            continue
        try:
            ensure_text_file(candidate, max_bytes=settings.max_file_bytes)
        except (AccessDenied, OSError):
            continue
        yield candidate


def search_code(settings: Settings, query: str, path: str = ".", limit: int = 100) -> list[dict[str, object]]:
    if not query or not query.strip():
        raise ValueError("query must be non-empty")
    needle = query.casefold()
    limit = max(1, min(int(limit), settings.max_search_results))
    results: list[dict[str, object]] = []
    for candidate in _iter_search_files(settings, path):
        try:
            lines = candidate.read_text(encoding="utf-8", errors="strict").splitlines()
        except (UnicodeDecodeError, OSError):
            continue
        for line_no, line in enumerate(lines, start=1):
            if needle in line.casefold():
                results.append({"path": _relative(settings.repo_root, candidate), "line": line_no, "text": line[:500]})
                if len(results) >= limit:
                    return results
    return results


def find_symbol(settings: Settings, name: str, path: str = ".", limit: int = 100) -> list[dict[str, object]]:
    if not name or not name.strip():
        raise ValueError("symbol name must be non-empty")
    escaped = re.escape(name.strip())
    patterns = [
        re.compile(rf"\b(?:fn|struct|enum|trait|type|const|static|mod)\s+{escaped}\b"),
        re.compile(rf"\b(?:def|class)\s+{escaped}\b"),
        re.compile(rf"\b(?:function|class|interface|type|const|let|var)\s+{escaped}\b"),
    ]
    limit = max(1, min(int(limit), settings.max_search_results))
    results: list[dict[str, object]] = []
    for candidate in _iter_search_files(settings, path):
        try:
            lines = candidate.read_text(encoding="utf-8", errors="strict").splitlines()
        except (UnicodeDecodeError, OSError):
            continue
        for line_no, line in enumerate(lines, start=1):
            if any(pattern.search(line) for pattern in patterns):
                results.append({"path": _relative(settings.repo_root, candidate), "line": line_no, "text": line[:500]})
                if len(results) >= limit:
                    return results
    return results
