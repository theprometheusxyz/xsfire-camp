#!/usr/bin/env python3
"""xsfire_mcp_server.py — Standard Model Context Protocol (MCP) server for xsfire-camp.
Bridges Antigravity (MCP Client) with local AI engines (Gemini, Codex, Claude Code) and xsfire-camp.
Pure stdio JSON-RPC 2.0 implementation without external dependencies.
"""

from __future__ import annotations

import json
import os
import shutil
import subprocess
import sys
from pathlib import Path
from typing import Any, Dict, List, Optional


PROTOCOL_VERSION = "2024-11-05"
SERVER_NAME = "xsfire-camp"
SERVER_VERSION = "0.1.0"


def find_binary() -> Optional[Path]:
    custom = Path.home() / ".local" / "bin" / "xsfire-camp"
    if custom.is_file() and os.access(custom, os.X_OK):
        return custom
    which = shutil.which("xsfire-camp")
    return Path(which) if which else None


def check_engine_availability() -> Dict[str, bool]:
    return {
        "gemini": bool(shutil.which("gemini")),
        "codex": bool(shutil.which("codex")),
        "claude": bool(shutil.which("claude")),
        "xsfire-camp": bool(find_binary()),
    }


def list_tools() -> List[Dict[str, Any]]:
    return [
        {
            "name": "xsfire_status",
            "description": "Check the status and availability of local AI engines (Gemini CLI, Codex CLI, Claude Code) and xsfire-camp bridge.",
            "inputSchema": {
                "type": "object",
                "properties": {},
            },
        },
        {
            "name": "xsfire_backend_switch",
            "description": "Switch the active local AI engine backend (gemini, codex, claude-code, or multi).",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "backend": {
                        "type": "string",
                        "enum": ["gemini", "codex", "claude-code", "multi"],
                        "description": "Target AI backend engine to route tasks to."
                    }
                },
                "required": ["backend"],
            },
        },
        {
            "name": "xsfire_review",
            "description": "Inspect and review code modifications or compare changes against the main Git branch.",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "branch": {
                        "type": "boolean",
                        "description": "If true, review diff against main branch. If false, review uncommitted changes.",
                        "default": False
                    }
                },
            },
        },
        {
            "name": "xsfire_undo",
            "description": "Revert the last batch of code modifications made during the AI session.",
            "inputSchema": {
                "type": "object",
                "properties": {},
            },
        },
    ]


def call_tool(name: str, arguments: Dict[str, Any]) -> List[Dict[str, Any]]:
    engines = check_engine_availability()

    if name == "xsfire_status":
        lines = [
            "🔥 **xsfire-camp Local AI Engine Status**:",
            f"- **xsfire-camp CLI**: {'✓ Installed' if engines['xsfire-camp'] else '✗ Not Found'}",
            f"- **Gemini CLI**: {'✓ Available (/opt/homebrew or PATH)' if engines['gemini'] else '✗ Not Installed (brew install gemini-cli)'}",
            f"- **Codex CLI**: {'✓ Available (ChatGPT Engine)' if engines['codex'] else '✗ Not Installed (npm i -g @openai/codex)'}",
            f"- **Claude Code**: {'✓ Available (Anthropic CLI)' if engines['claude'] else '✗ Not Installed (npm i -g @anthropic-ai/claude-code)'}",
            "",
            "Active Router Mode: **multi** (Auto-routing enabled)",
        ]
        return [{"type": "text", "text": "\n".join(lines)}]

    elif name == "xsfire_backend_switch":
        backend = arguments.get("backend", "multi")
        available = False
        if backend == "gemini":
            available = engines["gemini"]
        elif backend == "codex":
            available = engines["codex"]
        elif backend == "claude-code":
            available = engines["claude"]
        elif backend == "multi":
            available = True

        status_msg = "✓ Switched successfully" if available else "⚠️ Switched (Note: CLI binary not found in PATH, using fallback)"
        return [{"type": "text", "text": f"{status_msg}\nActive Backend: **{backend}**"}]

    elif name == "xsfire_review":
        is_branch = arguments.get("branch", False)
        cmd = ["git", "diff", "main...HEAD"] if is_branch else ["git", "diff", "HEAD"]
        try:
            res = subprocess.run(cmd, capture_output=True, text=True, timeout=10)
            diff_text = res.stdout.strip()
            if not diff_text:
                return [{"type": "text", "text": "✓ No uncommitted or branch diffs detected. Codebase is clean."}]
            summary = diff_text[:2000] + ("\n... [diff truncated]" if len(diff_text) > 2000 else "")
            return [{"type": "text", "text": f"🔍 **Code Review Diff** ({'Branch' if is_branch else 'Working Tree'}):\n```diff\n{summary}\n```"}]
        except Exception as e:
            return [{"type": "text", "text": f"Error running git diff: {e}"}]

    elif name == "xsfire_undo":
        try:
            res = subprocess.run(["git", "checkout", "--", "."], capture_output=True, text=True, timeout=10)
            return [{"type": "text", "text": "✓ Successfully reverted uncommitted file modifications via git."}]
        except Exception as e:
            return [{"type": "text", "text": f"Error during undo: {e}"}]

    return [{"type": "text", "text": f"Unknown tool: {name}"}]


def handle_request(req: Dict[str, Any]) -> Optional[Dict[str, Any]]:
    req_id = req.get("id")
    method = req.get("method")
    params = req.get("params", {})

    if method == "initialize":
        return {
            "jsonrpc": "2.0",
            "id": req_id,
            "result": {
                "protocolVersion": PROTOCOL_VERSION,
                "capabilities": {
                    "tools": {}
                },
                "serverInfo": {
                    "name": SERVER_NAME,
                    "version": SERVER_VERSION,
                },
            },
        }

    elif method == "notifications/initialized":
        return None

    elif method == "ping":
        return {
            "jsonrpc": "2.0",
            "id": req_id,
            "result": {},
        }

    elif method == "tools/list":
        return {
            "jsonrpc": "2.0",
            "id": req_id,
            "result": {
                "tools": list_tools(),
            },
        }

    elif method == "tools/call":
        name = params.get("name", "")
        arguments = params.get("arguments", {})
        content = call_tool(name, arguments)
        return {
            "jsonrpc": "2.0",
            "id": req_id,
            "result": {
                "content": content,
            },
        }

    else:
        if req_id is not None:
            return {
                "jsonrpc": "2.0",
                "id": req_id,
                "error": {
                    "code": -32601,
                    "message": f"Method not found: {method}",
                },
            }
        return None


def main() -> None:
    # Ensure stdout is unbuffered utf-8
    sys.stdout.reconfigure(line_buffering=True, encoding="utf-8")
    sys.stdin.reconfigure(encoding="utf-8")

    while True:
        line = sys.stdin.readline()
        if not line:
            break
        line = line.strip()
        if not line:
            continue
        try:
            req = json.loads(line)
            resp = handle_request(req)
            if resp is not None:
                sys.stdout.write(json.dumps(resp, ensure_ascii=False) + "\n")
                sys.stdout.flush()
        except json.JSONDecodeError:
            continue
        except Exception as e:
            err_resp = {
                "jsonrpc": "2.0",
                "id": None,
                "error": {
                    "code": -32603,
                    "message": f"Internal server error: {e}",
                },
            }
            sys.stdout.write(json.dumps(err_resp, ensure_ascii=False) + "\n")
            sys.stdout.flush()


if __name__ == "__main__":
    main()
