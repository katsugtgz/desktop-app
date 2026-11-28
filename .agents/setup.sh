#!/usr/bin/env bash
set -euo pipefail

cd "$(dirname "${BASH_SOURCE[0]}")/../rust"

# Rune links against the XKB libraries on Linux, including for headless tests.
if [[ $(uname -s) == Linux ]] && command -v apt-get >/dev/null; then
  missing=()
  for package in libxkbcommon-dev libxkbcommon-x11-dev; do
    if [[ $(dpkg-query -W -f='${db:Status-Status}' "$package" 2>/dev/null) != installed ]]; then
      missing+=("$package")
    fi
  done
  if (( ${#missing[@]} )); then
    apt_prefix=()
    if (( EUID != 0 )); then
      if ! command -v sudo >/dev/null; then
        printf 'Install the required Linux packages: %s\n' "${missing[*]}" >&2
        exit 1
      fi
      apt_prefix=(sudo)
    fi
    "${apt_prefix[@]}" apt-get update -qq
    "${apt_prefix[@]}" apt-get install -y -qq "${missing[@]}"
  fi
fi

# Reuse an existing Rust installation, including rustup outside PATH.
rust_bin="${CARGO_HOME:-$HOME/.cargo}/bin"
if [[ -x "$rust_bin/cargo" ]]; then
  export PATH="$rust_bin:$PATH"
fi
if ! command -v cargo >/dev/null; then
  installer=$(mktemp)
  trap 'rm -f "$installer"' EXIT
  curl -fsSL https://sh.rustup.rs -o "$installer"
  sh "$installer" -y --no-modify-path --profile minimal --component rustfmt --component clippy
  export PATH="$rust_bin:$PATH"
fi

host_target=$(rustc -vV | awk '/^host:/ {print $2}')
cargo fetch --locked --target "$host_target"
