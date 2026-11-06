#!/usr/bin/env bash
# Phase 0 only. Run as an ordinary user inside the target desktop session.
set -euo pipefail

root="$(cd -- "$(dirname -- "${BASH_SOURCE[0]}")/.." && pwd)"
step="${1:---headless}"

case "$step" in
  --headless|--native|--run) ;;
  *) printf 'Usage: bash scripts/verify-nsh-linux.sh [--headless|--native|--run]\n' >&2; exit 2 ;;
esac

if [[ "$(uname -s)" != Linux ]]; then
  printf 'This verification must run on Linux.\n' >&2
  exit 1
fi
if [[ "$(id -u)" == 0 ]]; then
  printf 'Run the GUI verification as an ordinary desktop user, without sudo.\n' >&2
  exit 1
fi
for tool in cargo rustc pkg-config nsh; do
  if ! command -v "$tool" >/dev/null; then
    printf 'Missing build tool: %s. See docs/nsh-spike.md.\n' "$tool" >&2
    exit 1
  fi
done
if ! pkg-config --atleast-version=4.14 gtk4; then
  printf 'GTK 4.14+ development files are required (Arch package: gtk4).\n' >&2
  exit 1
fi
if [[ "$(nsh --version | awk '{print $2}')" != 0.4.0 ]]; then
  printf 'Use NSH CLI 0.4.0, matching the pinned Rust crates.\n' >&2
  exit 1
fi
mkdir -p "$root/artifacts"
{
  date --iso-8601=seconds
  uname -srmo
  rustc --version
  cargo --version
  nsh --version
  pkg-config --modversion gtk4
  printf 'XDG_SESSION_TYPE=%s\nWAYLAND_DISPLAY=%s\nXDG_CURRENT_DESKTOP=%s\n' \
    "${XDG_SESSION_TYPE:-}" "${WAYLAND_DISPLAY:-}" "${XDG_CURRENT_DESKTOP:-}"
} > "$root/artifacts/nsh-linux-environment.txt"

cd "$root/devguard-gui"
case "$step" in
  --headless)
    cargo fmt -p devguard-gui -- --check
    nsh check
    nsh test --junit "$root/artifacts/nsh-linux-headless.xml"
    cargo clippy -p devguard-gui --all-targets -- -D warnings
    ;;
  --native)
    if [[ -z "${WAYLAND_DISPLAY:-}" ]]; then
      printf 'Run this inside the Wayland/Hyprland desktop session.\n' >&2
      exit 1
    fi
    export GDK_BACKEND=wayland
    nsh test nsh.json --target linux < "$root/scripts/nsh-native.steps.js"
    ;;
  --run)
    cargo run -p devguard-gui
    ;;
esac
