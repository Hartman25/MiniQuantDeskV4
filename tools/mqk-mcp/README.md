# MiniQuantDesk Read-Only MCP

This tool exposes a **read-only view of one local MiniQuantDesk working tree** to an MCP client. It is intentionally isolated from runtime, risk, broker, order, database-mutation, and live-routing code.

## Security contract

The server provides no generic shell, no write-file tool, no patch application, no database mutation, and no broker/order capability. Repository file access is canonicalized under one configured root. Absolute paths, traversal outside the root, symlink escapes, `.git`, `.ssh`, environment/secret files, private keys, binary files, oversized files, and oversized outputs are rejected. Git access uses fixed argument lists for read operations only.

## Tools

- `mqk_repo_snapshot`
- `mqk_repo_status`
- `mqk_current_branch`
- `mqk_current_head`
- `mqk_list_files`
- `mqk_read_file`
- `mqk_search_code`
- `mqk_find_symbol`
- `mqk_git_diff`
- `mqk_git_log`
- `mqk_git_show`
- `mqk_list_smoke_logs`

## Install

From the repository root in PowerShell:

```powershell
py -m venv tools\mqk-mcp\.venv
& tools\mqk-mcp\.venv\Scripts\python.exe -m pip install -e ".\tools\mqk-mcp[test]"
```

## Run locally over stdio

Set the exact worktree you want the MCP to expose, then start it:

```powershell
$env:MQK_MCP_REPO_ROOT = "C:\Users\Zacha\Desktop\MiniQuantDeskV4"
& tools\mqk-mcp\.venv\Scripts\mqk-readonly-mcp.exe
```

When `MQK_MCP_REPO_ROOT` is omitted, the package attempts to discover the enclosing MiniQuantDesk repository. Explicit configuration is preferred when multiple worktrees exist.

## MCP client configuration

For a local MCP client that supports stdio, configure the command as the virtual-environment executable and set `MQK_MCP_REPO_ROOT` in the client environment. Example shape:

```json
{
  "mcpServers": {
    "miniquantdesk-readonly": {
      "command": "C:\\Users\\Zacha\\Desktop\\MiniQuantDeskV4\\tools\\mqk-mcp\\.venv\\Scripts\\mqk-readonly-mcp.exe",
      "env": {
        "MQK_MCP_REPO_ROOT": "C:\\Users\\Zacha\\Desktop\\MiniQuantDeskV4"
      }
    }
  }
}
```

Do not expose the stdio server directly to the network. A future remote/HTTP transport, authentication, and approval boundary should be a separate patch.

## Tests

```powershell
& tools\mqk-mcp\.venv\Scripts\python.exe -m pytest tools\mqk-mcp\tests -q
```

The tests cover traversal denial, absolute-path denial, secret denial, `.env.local.example` allowance, symlink escape denial where supported, bounded reads/search, Git argument injection rejection, and a read-only Git-status check.
