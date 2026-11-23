#!/usr/bin/env bash
set -euo pipefail

# Install Node version pinned in .nvmrc under $HOME/.local so it persists
# across VM stop/wake and overrides older system/nvm Node versions.
NODE_VER="$(tr -d '[:space:]' < .nvmrc 2>/dev/null || echo '24.19.0')"
NODE_VER="${NODE_VER#v}"
NODE_DIR="$HOME/.local/node-v${NODE_VER}"

if [ "$(uname -s)" = "Linux" ] && [ ! -x "$NODE_DIR/bin/node" ]; then
  case "$(uname -m)" in
    x86_64) NODE_ARCH="x64" ;;
    aarch64|arm64) NODE_ARCH="arm64" ;;
    *) echo "Unsupported Linux architecture: $(uname -m)" >&2; exit 1 ;;
  esac
  mkdir -p "$HOME/.local"
  TMP_DIR="$(mktemp -d)"
  trap 'rm -rf "$TMP_DIR"' EXIT
  curl -fsSL "https://nodejs.org/dist/v${NODE_VER}/node-v${NODE_VER}-linux-${NODE_ARCH}.tar.xz" -o "$TMP_DIR/node.tar.xz"
  tar -xJf "$TMP_DIR/node.tar.xz" -C "$TMP_DIR"
  rm -rf "$NODE_DIR"
  mv "$TMP_DIR/node-v${NODE_VER}-linux-${NODE_ARCH}" "$NODE_DIR"
fi

if [ -x "$NODE_DIR/bin/node" ]; then
  touch "$HOME/.bashrc"
  PATH_LINE="export PATH=\"$NODE_DIR/bin:\$PATH\""
  HEAD_LINE="${PATH_LINE} # station-env-setup-head"
  TAIL_LINE="${PATH_LINE} # station-env-setup-tail"
  if ! grep -qxF "$HEAD_LINE" "$HOME/.bashrc"; then
    TMP_BASHRC="$(mktemp)"
    { printf '%s\n' "$HEAD_LINE"; cat "$HOME/.bashrc"; } > "$TMP_BASHRC"
    cp "$TMP_BASHRC" "$HOME/.bashrc"
    rm -f "$TMP_BASHRC"
  fi
  if ! grep -qxF "$TAIL_LINE" "$HOME/.bashrc"; then
    [ ! -s "$HOME/.bashrc" ] || [ -z "$(tail -c 1 "$HOME/.bashrc")" ] || printf '\n' >> "$HOME/.bashrc"
    printf '%s\n' "$TAIL_LINE" >> "$HOME/.bashrc"
  fi
  export PATH="$NODE_DIR/bin:$PATH"
fi

if ! command -v yarn >/dev/null 2>&1; then
  npm install -g --no-audit --no-fund yarn
fi

# System dev libraries required to link rust/crates/station-shell on Linux.
if [ "$(uname -s)" = "Linux" ] && command -v apt-get >/dev/null 2>&1; then
  MISSING_PKGS=()
  for pkg in libxkbcommon-dev libxkbcommon-x11-dev; do
    if ! dpkg -s "$pkg" >/dev/null 2>&1; then
      MISSING_PKGS+=("$pkg")
    fi
  done
  if [ "${#MISSING_PKGS[@]}" -gt 0 ]; then
    APT_PREFIX=()
    if [ "$(id -u)" -ne 0 ] && command -v sudo >/dev/null 2>&1; then
      APT_PREFIX=(sudo)
    fi
    "${APT_PREFIX[@]}" env DEBIAN_FRONTEND=noninteractive apt-get update -qq
    "${APT_PREFIX[@]}" env DEBIAN_FRONTEND=noninteractive apt-get install -y -qq "${MISSING_PKGS[@]}"
  fi
fi

# Install JS/TS workspace dependencies and prefetch Electron binary.
# Clear .bin/electron first to avoid Yarn 4 ELOOP on re-runs due to root package.json bin entry.
rm -f node_modules/.bin/electron
yarn install --immutable
yarn workspace station-desktop-app exec install-electron

# Prefetch Rust workspace dependencies for the host target.
if command -v cargo >/dev/null 2>&1 && [ -f rust/Cargo.toml ]; then
  HOST_TARGET="$(rustc -vV | awk '/^host:/ {print $2}')"
  cargo fetch --manifest-path rust/Cargo.toml --locked --target "$HOST_TARGET"
fi
