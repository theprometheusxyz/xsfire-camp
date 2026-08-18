#!/usr/bin/env bash
set -euo pipefail

ROOT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
INSTALL_PATH="${INSTALL_PATH:-$HOME/.local/bin/xsfire-camp}"

CARGO_HOME_VALUE="${CARGO_HOME:-}"
CARGO_TARGET_DIR_VALUE="${CARGO_TARGET_DIR:-}"

mkdir -p "$(dirname "$INSTALL_PATH")"

ENV_PREFIX=()
if [[ -n "$CARGO_HOME_VALUE" ]]; then
  ENV_PREFIX+=("CARGO_HOME=$CARGO_HOME_VALUE")
fi
if [[ -n "$CARGO_TARGET_DIR_VALUE" ]]; then
  ENV_PREFIX+=("CARGO_TARGET_DIR=$CARGO_TARGET_DIR_VALUE")
fi

(
  cd "$ROOT_DIR"
  if [[ ${#ENV_PREFIX[@]} -gt 0 ]]; then
    "${ENV_PREFIX[@]}" cargo build --release
  else
    cargo build --release
  fi
)

BIN_PATH="$ROOT_DIR/target/release/xsfire-camp"
if [[ -n "$CARGO_TARGET_DIR_VALUE" ]]; then
  BIN_PATH="$CARGO_TARGET_DIR_VALUE/release/xsfire-camp"
fi

if [[ ! -f "$BIN_PATH" ]]; then
  echo "Build output not found: $BIN_PATH" >&2
  exit 1
fi

INSTALL_DIR="$(dirname "$INSTALL_PATH")"
TMP_PATH="$INSTALL_DIR/.xsfire-camp.tmp.$$"

# Install via atomic rename so we don't follow an existing symlink target.
cp -f "$BIN_PATH" "$TMP_PATH"
chmod +x "$TMP_PATH"
mv -f "$TMP_PATH" "$INSTALL_PATH"

echo "Installed: $INSTALL_PATH"

if [[ "${SETUP_EDITORS:-0}" == "1" || "${1:-}" == "--setup-all" ]]; then
  echo ""
  python3 "$ROOT_DIR/scripts/setup_editors.py" --target all
fi
