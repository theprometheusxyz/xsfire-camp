#!/usr/bin/env python3
"""setup_editors.py — Automatic installation and configuration injector for xsfire-camp.
Supports Antigravity.app, Antigravity IDE.app, Zed IDE, and VS Code-based editors (VS Code, Cursor, Windsurf).
"""

from __future__ import annotations

import argparse
import datetime
import json
import os
import re
import shutil
import sys
from pathlib import Path
from typing import Any, Dict, List, Optional, Tuple


def get_timestamp() -> str:
    return datetime.datetime.now(datetime.timezone.utc).strftime("%Y%m%dT%H%M%SZ")


def backup_file(file_path: Path) -> Optional[Path]:
    if not file_path.is_file():
        return None
    backup_path = file_path.with_name(f"{file_path.name}.bak-{get_timestamp()}")
    shutil.copy2(file_path, backup_path)
    return backup_path


def strip_jsonc_comments_and_trailing_commas(text: str) -> str:
    """Strip C/C++ style comments and trailing commas from JSONC string without breaking strings."""
    def replacer(match: re.Match) -> str:
        s = match.group(0)
        if s.startswith("/"):
            return ""
        return s

    pattern = re.compile(
        r'//.*?$|/\*.*?\*/|\'(?:\\.|[^\\\'])*\'|"(?:\\.|[^\\"])*"',
        re.DOTALL | re.MULTILINE,
    )
    cleaned = re.sub(pattern, replacer, text)
    cleaned = re.sub(r",\s*([\]}])", r"\1", cleaned)
    return cleaned


def safe_load_json(file_path: Path) -> Dict[str, Any]:
    if not file_path.is_file():
        return {}
    try:
        content = file_path.read_text(encoding="utf-8")
        if not content.strip():
            return {}
        try:
            return json.loads(content)
        except json.JSONDecodeError:
            stripped = strip_jsonc_comments_and_trailing_commas(content)
            return json.loads(stripped)
    except Exception as e:
        print(f"  [Warning] Failed to parse {file_path}: {e}", file=sys.stderr)
        return {}


def safe_write_json(file_path: Path, data: Dict[str, Any], dry_run: bool = False) -> None:
    if dry_run:
        print(f"  [Dry-Run] Would write updated JSON to: {file_path}")
        return
    file_path.parent.mkdir(parents=True, exist_ok=True)
    temp_file = file_path.with_name(f"{file_path.name}.tmp.{os.getpid()}")
    temp_file.write_text(json.dumps(data, indent=2, ensure_ascii=False) + "\n", encoding="utf-8")
    temp_file.replace(file_path)


def get_binary_cmd() -> str:
    binary_path = Path.home() / ".local" / "bin" / "xsfire-camp"
    return str(binary_path) if binary_path.is_file() else "xsfire-camp"


def get_python_cmd() -> str:
    which = shutil.which("python3")
    if which and Path(which).is_file():
        return which
    if Path("/usr/bin/python3").is_file():
        return "/usr/bin/python3"
    return sys.executable


# ---------------------------------------------------------------------------
# 1. Antigravity.app & Antigravity IDE.app Setup
# ---------------------------------------------------------------------------
def setup_antigravity_core(repo_root: Path, dry_run: bool = False, uninstall: bool = False) -> bool:
    gemini_config = Path.home() / ".gemini" / "config"
    target_plugin_dir = gemini_config / "plugins" / "xsfire-camp"
    target_skill_dir = gemini_config / "skills" / "xsfire-camp"
    target_agents_dir = gemini_config / "agents"
    mcp_config_path = gemini_config / "mcp_config.json"
    source_dir = repo_root / "plugins" / "xsfire-camp"

    print(f"\n[Antigravity.app / Global Config] Target: {gemini_config}")
    if uninstall:
        if target_plugin_dir.exists():
            if not dry_run:
                shutil.rmtree(target_plugin_dir)
            print("  ✓ Uninstalled xsfire-camp plugin from Antigravity.")
        if target_skill_dir.exists():
            if not dry_run:
                shutil.rmtree(target_skill_dir)
            print("  ✓ Removed xsfire-camp from global skills.")
        if (target_agents_dir / "xsfire-camp-agent.md").exists():
            if not dry_run:
                (target_agents_dir / "xsfire-camp-agent.md").unlink()
            print("  ✓ Removed xsfire-camp-agent.md from global agents.")
        if mcp_config_path.is_file():
            data = safe_load_json(mcp_config_path)
            servers = data.get("mcpServers", {})
            if "xsfire-camp" in servers:
                bak = backup_file(mcp_config_path)
                del servers["xsfire-camp"]
                data["mcpServers"] = servers
                safe_write_json(mcp_config_path, data, dry_run=dry_run)
                print("  ✓ Removed xsfire-camp from global mcp_config.json.")
        return True

    if not source_dir.is_dir():
        print(f"  [Error] Source plugin template not found at {source_dir}", file=sys.stderr)
        return False

    if dry_run:
        print(f"  [Dry-Run] Would copy {source_dir} -> {target_plugin_dir}")
        print(f"  [Dry-Run] Would copy skill -> {target_skill_dir}")
        print(f"  [Dry-Run] Would copy agent -> {target_agents_dir / 'xsfire-camp-agent.md'}")
        print(f"  [Dry-Run] Would register in -> {mcp_config_path}")
        return True

    # 1. Copy Plugin directory
    target_plugin_dir.parent.mkdir(parents=True, exist_ok=True)
    if target_plugin_dir.exists():
        shutil.rmtree(target_plugin_dir)
    shutil.copytree(source_dir, target_plugin_dir)
    print("  ✓ Installed xsfire-camp plugin to Antigravity (~/.gemini/config/plugins/xsfire-camp)")

    # 2. Copy Global Skill & Agent
    target_skill_dir.mkdir(parents=True, exist_ok=True)
    skill_src = source_dir / "skills" / "xsfire-camp" / "SKILL.md"
    if skill_src.is_file():
        shutil.copy2(skill_src, target_skill_dir / "SKILL.md")
        print("  ✓ Synchronized xsfire-camp skill to Antigravity (~/.gemini/config/skills/xsfire-camp/SKILL.md)")

    target_agents_dir.mkdir(parents=True, exist_ok=True)
    agent_src = source_dir / "agents" / "xsfire-camp-agent.md"
    if agent_src.is_file():
        shutil.copy2(agent_src, target_agents_dir / "xsfire-camp-agent.md")
        print("  ✓ Synchronized xsfire-camp agent to Antigravity (~/.gemini/config/agents/xsfire-camp-agent.md)")

    # 3. Update ~/.gemini/config/mcp_config.json
    mcp_server_script = target_plugin_dir / "scripts" / "xsfire_mcp_server.py"
    mcp_data = safe_load_json(mcp_config_path)
    mcp_servers = mcp_data.get("mcpServers", {})
    mcp_servers["xsfire-camp"] = {
        "command": get_python_cmd(),
        "args": [str(mcp_server_script)]
    }
    mcp_data["mcpServers"] = mcp_servers
    safe_write_json(mcp_config_path, mcp_data, dry_run=dry_run)
    print("  ✓ Registered xsfire-camp MCP adapter in global mcp_config.json (~/.gemini/config/mcp_config.json)")

    return True


def setup_antigravity_ide(repo_root: Path, dry_run: bool = False, uninstall: bool = False) -> bool:
    gemini_ide_dir = Path.home() / ".gemini" / "antigravity-ide"
    target_plugin_dir = gemini_ide_dir / "plugins" / "xsfire-camp"
    mcp_config_path = gemini_ide_dir / "mcp_config.json"
    source_dir = repo_root / "plugins" / "xsfire-camp"

    print(f"\n[Antigravity IDE.app] Target: {gemini_ide_dir}")
    if uninstall:
        if target_plugin_dir.exists():
            if not dry_run:
                shutil.rmtree(target_plugin_dir)
            print("  ✓ Uninstalled xsfire-camp plugin from Antigravity IDE.")
        if mcp_config_path.is_file():
            data = safe_load_json(mcp_config_path)
            servers = data.get("mcpServers", {})
            if "xsfire-camp" in servers:
                bak = backup_file(mcp_config_path)
                del servers["xsfire-camp"]
                data["mcpServers"] = servers
                safe_write_json(mcp_config_path, data, dry_run=dry_run)
                print("  ✓ Removed xsfire-camp from antigravity-ide/mcp_config.json.")
        return True

    if not source_dir.is_dir():
        print(f"  [Error] Source plugin template not found at {source_dir}", file=sys.stderr)
        return False

    if dry_run:
        print(f"  [Dry-Run] Would copy {source_dir} -> {target_plugin_dir}")
        print(f"  [Dry-Run] Would register in -> {mcp_config_path}")
        return True

    # Copy plugin to antigravity-ide/plugins
    if gemini_ide_dir.is_dir():
        target_plugin_dir.parent.mkdir(parents=True, exist_ok=True)
        if target_plugin_dir.exists():
            shutil.rmtree(target_plugin_dir)
        shutil.copytree(source_dir, target_plugin_dir)
        print("  ✓ Installed xsfire-camp plugin to Antigravity IDE (~/.gemini/antigravity-ide/plugins/xsfire-camp)")

        # Update antigravity-ide/mcp_config.json
        mcp_server_script = target_plugin_dir / "scripts" / "xsfire_mcp_server.py"
        mcp_data = safe_load_json(mcp_config_path)
        mcp_servers = mcp_data.get("mcpServers", {})
        mcp_servers["xsfire-camp"] = {
            "command": get_python_cmd(),
            "args": [str(mcp_server_script)]
        }
        mcp_data["mcpServers"] = mcp_servers
        safe_write_json(mcp_config_path, mcp_data, dry_run=dry_run)
        print("  ✓ Registered xsfire-camp MCP adapter in Antigravity IDE mcp_config.json (~/.gemini/antigravity-ide/mcp_config.json)")

    return True


# ---------------------------------------------------------------------------
# 2. Zed IDE Setup
# ---------------------------------------------------------------------------
def get_zed_settings_path() -> Path:
    if sys.platform == "win32":
        appdata = os.environ.get("APPDATA", "")
        return Path(appdata) / "Zed" / "settings.json"
    return Path.home() / ".config" / "zed" / "settings.json"


def setup_zed(dry_run: bool = False, uninstall: bool = False) -> bool:
    settings_path = get_zed_settings_path()
    print(f"\n[Zed IDE] Target: {settings_path}")

    data = safe_load_json(settings_path)
    agent_servers = data.get("agent_servers", {})

    if uninstall:
        if "xsfire-camp" in agent_servers:
            bak = backup_file(settings_path)
            if bak:
                print(f"  ✓ Backup created: {bak}")
            del agent_servers["xsfire-camp"]
            if not agent_servers:
                data.pop("agent_servers", None)
            else:
                data["agent_servers"] = agent_servers
            safe_write_json(settings_path, data, dry_run=dry_run)
            print("  ✓ Removed xsfire-camp from Zed agent_servers.")
        else:
            print("  - xsfire-camp not found in Zed settings.")
        return True

    server_spec = {
        "type": "custom",
        "command": "xsfire-camp",
        "args": ["--backend=multi"]
    }

    existing_entry = agent_servers.get("xsfire-camp")
    if existing_entry is not None:
        if isinstance(existing_entry, dict) and (existing_entry.get("command") == "xsfire-camp" or "xsfire-camp" in str(existing_entry.get("command", ""))):
            print("  ✓ xsfire-camp already configured in Zed settings.")
            return True

    if settings_path.is_file():
        bak = backup_file(settings_path)
        if bak:
            print(f"  ✓ Backup created: {bak}")

    agent_servers["xsfire-camp"] = server_spec
    data["agent_servers"] = agent_servers
    safe_write_json(settings_path, data, dry_run=dry_run)
    print("  ✓ Successfully registered xsfire-camp in Zed settings.json (`agent_servers.xsfire-camp`).")
    return True


# ---------------------------------------------------------------------------
# 3. VS Code / Cursor / Windsurf Setup (MCP integration)
# ---------------------------------------------------------------------------
def get_vscode_mcp_targets() -> List[Tuple[str, Path]]:
    home = Path.home()
    targets = []

    # 1. Cursor
    cursor_mcp = home / ".cursor" / "mcp.json"
    targets.append(("Cursor", cursor_mcp))

    # 2. Windsurf
    windsurf_mcp = home / ".codeium" / "windsurf" / "mcp_config.json"
    targets.append(("Windsurf", windsurf_mcp))

    # 3. VS Code - Cline extension
    if sys.platform == "darwin":
        cline_mcp = home / "Library" / "Application Support" / "Code" / "User" / "globalStorage" / "saoudrizwan.claude-dev" / "settings" / "cline_mcp_settings.json"
        roo_mcp = home / "Library" / "Application Support" / "Code" / "User" / "globalStorage" / "rooveterinaryinc.roo-cline" / "settings" / "cline_mcp_settings.json"
    elif sys.platform == "win32":
        appdata = Path(os.environ.get("APPDATA", ""))
        cline_mcp = appdata / "Code" / "User" / "globalStorage" / "saoudrizwan.claude-dev" / "settings" / "cline_mcp_settings.json"
        roo_mcp = appdata / "Code" / "User" / "globalStorage" / "rooveterinaryinc.roo-cline" / "settings" / "cline_mcp_settings.json"
    else:
        cline_mcp = home / ".config" / "Code" / "User" / "globalStorage" / "saoudrizwan.claude-dev" / "settings" / "cline_mcp_settings.json"
        roo_mcp = home / ".config" / "Code" / "User" / "globalStorage" / "rooveterinaryinc.roo-cline" / "settings" / "cline_mcp_settings.json"

    targets.append(("VS Code (Cline)", cline_mcp))
    targets.append(("VS Code (Roo Code)", roo_mcp))

    return targets


def setup_vscode_family(dry_run: bool = False, uninstall: bool = False) -> bool:
    print(f"\n[VS Code Family (VS Code, Cursor, Windsurf)] Scanning targets...")
    targets = get_vscode_mcp_targets()

    mcp_spec = {
        "command": "xsfire-camp",
        "args": ["--backend=multi"]
    }

    configured_count = 0
    for name, config_path in targets:
        parent_exists = config_path.parent.is_dir()
        file_exists = config_path.is_file()

        if not parent_exists and not file_exists and not uninstall:
            if name not in ("Cursor", "Windsurf"):
                continue

        print(f"  • {name} -> {config_path}")
        data = safe_load_json(config_path)
        mcp_servers = data.get("mcpServers", {})

        if uninstall:
            if "xsfire-camp" in mcp_servers:
                bak = backup_file(config_path)
                if bak:
                    print(f"    ✓ Backup created: {bak}")
                del mcp_servers["xsfire-camp"]
                if not mcp_servers:
                    data.pop("mcpServers", None)
                else:
                    data["mcpServers"] = mcp_servers
                safe_write_json(config_path, data, dry_run=dry_run)
                print(f"    ✓ Removed xsfire-camp from {name}.")
            continue

        if mcp_servers.get("xsfire-camp") == mcp_spec:
            print(f"    ✓ Already configured in {name}.")
            configured_count += 1
            continue

        if file_exists:
            bak = backup_file(config_path)
            if bak:
                print(f"    ✓ Backup created: {bak}")

        mcp_servers["xsfire-camp"] = mcp_spec
        data["mcpServers"] = mcp_servers
        safe_write_json(config_path, data, dry_run=dry_run)
        print(f"    ✓ Successfully registered xsfire-camp in {name}.")
        configured_count += 1

    return True


# ---------------------------------------------------------------------------
# Main CLI Entry Point
# ---------------------------------------------------------------------------
def main() -> int:
    parser = argparse.ArgumentParser(description="xsfire-camp multi-editor auto configuration tool")
    parser.add_argument(
        "--target",
        choices=["all", "antigravity", "antigravity-app", "antigravity-ide", "zed", "vscode"],
        default="all",
        help="Target editor to configure: all, antigravity (both app & ide), antigravity-app, antigravity-ide, zed, vscode (default: all)"
    )
    parser.add_argument("--dry-run", action="store_true", help="Simulate changes without writing files")
    parser.add_argument("--uninstall", action="store_true", help="Remove xsfire-camp configuration from editors")
    args = parser.parse_args()

    repo_root = Path(__file__).resolve().parent.parent
    action_verb = "Uninstalling" if args.uninstall else "Setting up"
    print(f"==================================================")
    print(f" 🔥 xsfire-camp Editor Auto-Setup ({action_verb})")
    print(f" Target(s): {args.target}")
    if args.dry_run:
        print(f" Mode: DRY-RUN (no files will be modified)")
    print(f"==================================================")

    success = True
    if args.target in ("all", "antigravity", "antigravity-app"):
        if not setup_antigravity_core(repo_root, dry_run=args.dry_run, uninstall=args.uninstall):
            success = False

    if args.target in ("all", "antigravity", "antigravity-ide"):
        if not setup_antigravity_ide(repo_root, dry_run=args.dry_run, uninstall=args.uninstall):
            success = False

    if args.target in ("all", "zed"):
        if not setup_zed(dry_run=args.dry_run, uninstall=args.uninstall):
            success = False

    if args.target in ("all", "vscode"):
        if not setup_vscode_family(dry_run=args.dry_run, uninstall=args.uninstall):
            success = False

    print("\n==================================================")
    if success:
        print(" 🎉 Setup completed successfully!")
    else:
        print(" ⚠️  Setup finished with some warnings.")
    print("==================================================")
    return 0 if success else 1


if __name__ == "__main__":
    raise SystemExit(main())
