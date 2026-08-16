from __future__ import annotations

import re
import subprocess
from pathlib import Path

from .config import Settings
from .security import AccessDenied, resolve_repo_path

_SHA_RE = re.compile(r"^[0-9a-fA-F]{7,40}$")


def _run_git(settings: Settings, args: list[str]) -> str:
    completed = subprocess.run(
        ["git", "-C", str(settings.repo_root), *args],
        check=False,
        capture_output=True,
        text=True,
        encoding="utf-8",
        errors="strict",
        timeout=15,
        env={"PATH": __import__("os").environ.get("PATH", "")},
    )
    output = completed.stdout
    if completed.returncode != 0:
        message = completed.stderr.strip() or f"git exited with {completed.returncode}"
        raise RuntimeError(message[:4000])
    if len(output) > settings.max_output_chars:
        raise AccessDenied("git output exceeds MCP output limit")
    return output.rstrip()


def repo_status(settings: Settings) -> str:
    return _run_git(settings, ["status", "--porcelain=v1", "--branch"])


def current_branch(settings: Settings) -> str:
    return _run_git(settings, ["branch", "--show-current"])


def current_head(settings: Settings) -> str:
    return _run_git(settings, ["rev-parse", "HEAD"])


def git_diff(settings: Settings, path: str | None = None, *, staged: bool = False) -> str:
    args = ["diff", "--no-ext-diff"]
    if staged:
        args.append("--cached")
    args.append("--")
    if path is not None:
        target = resolve_repo_path(settings.repo_root, path)
        args.append(target.relative_to(settings.repo_root).as_posix())
    return _run_git(settings, args)


def git_log(settings: Settings, limit: int = 20) -> str:
    limit = max(1, min(int(limit), 100))
    return _run_git(settings, ["log", f"-{limit}", "--date=iso-strict", "--pretty=format:%H%x09%ad%x09%s"])


def git_show(settings: Settings, commit: str) -> str:
    commit = commit.strip()
    if not _SHA_RE.fullmatch(commit):
        raise AccessDenied("git_show accepts only a 7-40 character hexadecimal commit SHA")
    return _run_git(settings, ["show", "--no-ext-diff", "--format=fuller", "--stat", "--patch", commit, "--"])
