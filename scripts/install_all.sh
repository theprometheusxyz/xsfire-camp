#!/usr/bin/env bash
set -euo pipefail

ROOT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
INSTALL_PATH="${INSTALL_PATH:-$HOME/.local/bin/xsfire-camp}"
TARGET="${1:-all}"

echo "=================================================="
echo " 🔥 xsfire-camp All-in-One Installer"
echo " (Target: $TARGET)"
echo " Supported: Antigravity.app, Antigravity IDE.app, Zed, VS Code/Cursor/Windsurf"
echo "=================================================="

# 1. Build and install the binary
echo -e "\n[1/2] Building and installing xsfire-camp binary..."
"$ROOT_DIR/scripts/build_and_install.sh"

# Check if ~/.local/bin is in PATH
INSTALL_DIR="$(dirname "$INSTALL_PATH")"
if [[ ":$PATH:" != *":$INSTALL_DIR:"* ]]; then
  echo -e "\n💡 [Note] '$INSTALL_DIR' is not in your current PATH."
  echo "  Add this line to your ~/.zshrc or ~/.bashrc:"
  echo "    export PATH=\"$INSTALL_DIR:\$PATH\""
fi

# 2. Configure editors
echo -e "\n[2/2] Configuring target editors ($TARGET)..."
python3 "$ROOT_DIR/scripts/setup_editors.py" --target "$TARGET"

echo -e "\n=================================================="
echo " 🚀 xsfire-camp installation & setup complete!"
echo "=================================================="
echo ""
echo "How to use in your applications:"
echo " • Antigravity.app / Antigravity IDE.app :"
echo "     - Plugin and MCP server are automatically active."
echo "     - In chat, mention @xsfire-camp-agent or use /backend <gemini|codex|claude-code>."
echo " • Zed IDE :"
echo "     - Open AI panel (Cmd+Shift+P > 'agent: open') and select 'xsfire-camp'."
echo " • VS Code / Cursor / Windsurf :"
echo "     - xsfire-camp is registered as an active MCP tool server."
echo ""
echo "💡 Tip: You can configure individual apps anytime with:"
echo "    ./scripts/install_all.sh antigravity       # Antigravity.app & Antigravity IDE.app"
echo "    ./scripts/install_all.sh antigravity-app   # Antigravity.app only"
echo "    ./scripts/install_all.sh antigravity-ide   # Antigravity IDE.app only"
echo "    ./scripts/install_all.sh zed               # Zed IDE only"
echo "    ./scripts/install_all.sh vscode            # VS Code / Cursor / Windsurf"
echo ""
