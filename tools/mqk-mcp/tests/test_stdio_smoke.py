from __future__ import annotations

import asyncio
import subprocess
import sys
from pathlib import Path

from mcp import Client, StdioServerParameters
from mcp.client.stdio import stdio_client
from mcp.types import TextContent


_EXPECTED_TOOLS = {
    "mqk_repo_snapshot",
    "mqk_repo_status",
    "mqk_current_branch",
    "mqk_current_head",
    "mqk_list_files",
    "mqk_read_file",
    "mqk_search_code",
    "mqk_find_symbol",
    "mqk_git_diff",
    "mqk_git_log",
    "mqk_git_show",
    "mqk_list_smoke_logs",
}


def _git(repo: Path, *args: str) -> str:
    completed = subprocess.run(
        ["git", *args],
        cwd=repo,
        check=True,
        capture_output=True,
        text=True,
        timeout=10,
    )
    return completed.stdout.strip()


def _text(result: object) -> str:
    content = getattr(result, "content", [])
    return "\n".join(block.text for block in content if isinstance(block, TextContent))


def test_stdio_handshake_lists_and_calls_readonly_tools(tmp_path: Path) -> None:
    repo = tmp_path / "repo"
    repo.mkdir()
    (repo / "README.md").write_text("MiniQuantDesk MCP smoke proof\n", encoding="utf-8")
    (repo / ".env.local").write_text("SHOULD_NOT_LEAK=secret\n", encoding="utf-8")

    _git(repo, "init")
    _git(repo, "add", "README.md")
    _git(
        repo,
        "-c",
        "user.name=MiniQuantDesk MCP Test",
        "-c",
        "user.email=mcp-test@example.invalid",
        "commit",
        "-m",
        "fixture",
    )
    expected_head = _git(repo, "rev-parse", "HEAD")
    status_before = _git(repo, "status", "--porcelain=v1", "--untracked-files=all")

    async def exercise_stdio() -> None:
        params = StdioServerParameters(
            command=sys.executable,
            args=["-m", "mqk_mcp.server"],
            env={"MQK_MCP_REPO_ROOT": str(repo.resolve())},
        )

        async with Client(stdio_client(params)) as client:
            assert client.protocol_version
            assert client.server_capabilities.tools is not None

            tools = await client.list_tools()
            assert {tool.name for tool in tools.tools} == _EXPECTED_TOOLS

            head = await client.call_tool("mqk_current_head", {})
            assert not head.is_error
            assert head.structured_content == {"result": expected_head}

            readme = await client.call_tool(
                "mqk_read_file",
                {"path": "README.md", "start_line": 1, "end_line": 1},
            )
            assert not readme.is_error
            assert "MiniQuantDesk MCP smoke proof" in _text(readme)

            denied = await client.call_tool("mqk_read_file", {"path": ".env.local"})
            assert denied.is_error
            denied_text = _text(denied)
            assert "environment/secret files is denied" in denied_text
            assert "SHOULD_NOT_LEAK" not in denied_text
            assert "secret" not in denied_text

            snapshot = await client.call_tool("mqk_repo_snapshot", {})
            assert not snapshot.is_error
            assert snapshot.structured_content is not None
            assert snapshot.structured_content["repo_root"] == str(repo.resolve())
            assert snapshot.structured_content["head"] == expected_head

    asyncio.run(exercise_stdio())

    status_after = _git(repo, "status", "--porcelain=v1", "--untracked-files=all")
    assert status_after == status_before
