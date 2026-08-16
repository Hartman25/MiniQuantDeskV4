from __future__ import annotations

from mcp.server.fastmcp import FastMCP

from .config import Settings
from .filesystem import find_symbol as _find_symbol
from .filesystem import list_files as _list_files
from .filesystem import read_file as _read_file
from .filesystem import search_code as _search_code
from .git_tools import current_branch as _current_branch
from .git_tools import current_head as _current_head
from .git_tools import git_diff as _git_diff
from .git_tools import git_log as _git_log
from .git_tools import git_show as _git_show
from .git_tools import repo_status as _repo_status

mcp = FastMCP("MiniQuantDesk Readonly")
_settings: Settings | None = None


def settings() -> Settings:
    global _settings
    if _settings is None:
        _settings = Settings.from_env()
    return _settings


@mcp.tool()
def mqk_repo_status() -> str:
    """Return read-only Git status, including branch and local changes."""
    return _repo_status(settings())


@mcp.tool()
def mqk_current_branch() -> str:
    """Return the current local Git branch."""
    return _current_branch(settings())


@mcp.tool()
def mqk_current_head() -> str:
    """Return the current local Git HEAD SHA."""
    return _current_head(settings())


@mcp.tool()
def mqk_list_files(path: str = ".", recursive: bool = False, limit: int = 500) -> list[str]:
    """List repository paths without reading files outside the configured repo root."""
    return _list_files(settings(), path, recursive=recursive, limit=limit)


@mcp.tool()
def mqk_read_file(path: str, start_line: int = 1, end_line: int | None = None) -> str:
    """Read a bounded UTF-8 text-file line range. Secrets, keys, binaries, and path escapes are denied."""
    return _read_file(settings(), path, start_line=start_line, end_line=end_line)


@mcp.tool()
def mqk_search_code(query: str, path: str = ".", limit: int = 100) -> list[dict[str, object]]:
    """Case-insensitive literal search over bounded source/config/docs text files."""
    return _search_code(settings(), query, path=path, limit=limit)


@mcp.tool()
def mqk_find_symbol(name: str, path: str = ".", limit: int = 100) -> list[dict[str, object]]:
    """Find likely Rust, Python, or TypeScript symbol definitions by name."""
    return _find_symbol(settings(), name, path=path, limit=limit)


@mcp.tool()
def mqk_git_diff(path: str | None = None, staged: bool = False) -> str:
    """Return a read-only working-tree or staged Git diff, optionally restricted to one repository path."""
    return _git_diff(settings(), path=path, staged=staged)


@mcp.tool()
def mqk_git_log(limit: int = 20) -> str:
    """Return a bounded recent Git commit log."""
    return _git_log(settings(), limit=limit)


@mcp.tool()
def mqk_git_show(commit: str) -> str:
    """Return a bounded patch/stat for a hexadecimal commit SHA only."""
    return _git_show(settings(), commit)


@mcp.tool()
def mqk_list_smoke_logs(limit: int = 200) -> list[str]:
    """List local smoke_logs paths if the directory exists."""
    root = settings().repo_root / "smoke_logs"
    if not root.exists():
        return []
    return _list_files(settings(), "smoke_logs", recursive=True, limit=limit)


@mcp.tool()
def mqk_repo_snapshot() -> dict[str, object]:
    """Return a compact read-only local repository snapshot for orientation."""
    cfg = settings()
    return {
        "repo_root": str(cfg.repo_root),
        "branch": _current_branch(cfg),
        "head": _current_head(cfg),
        "status": _repo_status(cfg),
        "top_level": _list_files(cfg, ".", recursive=False, limit=500),
    }


def main() -> None:
    mcp.run(transport="stdio")


if __name__ == "__main__":
    main()
