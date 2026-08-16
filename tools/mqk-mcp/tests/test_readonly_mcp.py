from __future__ import annotations

import os
import subprocess
from pathlib import Path

import pytest

from mqk_mcp.config import Settings
from mqk_mcp.filesystem import read_file, search_code
from mqk_mcp.git_tools import git_show
from mqk_mcp.security import AccessDenied, resolve_repo_path


def _settings(root: Path) -> Settings:
    return Settings(repo_root=root.resolve(), max_file_bytes=10_000, max_output_chars=20_000, max_search_results=20)


def test_path_traversal_is_denied(tmp_path: Path) -> None:
    repo = tmp_path / "repo"
    repo.mkdir()
    outside = tmp_path / "outside.txt"
    outside.write_text("secret", encoding="utf-8")
    with pytest.raises(AccessDenied):
        resolve_repo_path(repo, "../outside.txt")


def test_absolute_path_is_denied(tmp_path: Path) -> None:
    repo = tmp_path / "repo"
    repo.mkdir()
    target = repo / "a.txt"
    target.write_text("x", encoding="utf-8")
    with pytest.raises(AccessDenied):
        resolve_repo_path(repo, str(target.resolve()))


def test_secret_files_are_denied_but_example_is_allowed(tmp_path: Path) -> None:
    repo = tmp_path / "repo"
    repo.mkdir()
    (repo / ".env.local").write_text("TOKEN=x", encoding="utf-8")
    (repo / ".env.local.example").write_text("TOKEN=example", encoding="utf-8")
    with pytest.raises(AccessDenied):
        read_file(_settings(repo), ".env.local")
    assert "TOKEN=example" in read_file(_settings(repo), ".env.local.example")


def test_symlink_escape_is_denied_when_supported(tmp_path: Path) -> None:
    repo = tmp_path / "repo"
    repo.mkdir()
    outside = tmp_path / "outside.txt"
    outside.write_text("secret", encoding="utf-8")
    link = repo / "link.txt"
    try:
        link.symlink_to(outside)
    except (OSError, NotImplementedError):
        pytest.skip("symlink creation unavailable")
    with pytest.raises(AccessDenied):
        resolve_repo_path(repo, "link.txt")


def test_bounded_read_and_search(tmp_path: Path) -> None:
    repo = tmp_path / "repo"
    repo.mkdir()
    src = repo / "sample.py"
    src.write_text("alpha\nbeta needle\ngamma needle\n", encoding="utf-8")
    cfg = _settings(repo)
    assert read_file(cfg, "sample.py", 2, 2) == "2: beta needle"
    matches = search_code(cfg, "needle", limit=1)
    assert matches == [{"path": "sample.py", "line": 2, "text": "beta needle"}]


def test_git_show_rejects_argument_injection(tmp_path: Path) -> None:
    repo = tmp_path / "repo"
    repo.mkdir()
    with pytest.raises(AccessDenied):
        git_show(_settings(repo), "--help")


def test_git_status_path_is_read_only(tmp_path: Path) -> None:
    repo = tmp_path / "repo"
    repo.mkdir()
    subprocess.run(["git", "init", str(repo)], check=True, capture_output=True)
    before = sorted(p.relative_to(repo).as_posix() for p in repo.rglob("*") if ".git" not in p.parts)
    from mqk_mcp.git_tools import repo_status

    repo_status(_settings(repo))
    after = sorted(p.relative_to(repo).as_posix() for p in repo.rglob("*") if ".git" not in p.parts)
    assert before == after
