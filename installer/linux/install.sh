#!/usr/bin/env bash
set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
SKIP_DEPS=false

for arg in "$@"; do
  case "$arg" in
    --skip-deps) SKIP_DEPS=true ;;
    -h|--help)
      echo "Usage: $0 [--skip-deps]"
      echo "Installs TiaRun binary, desktop entry, icons, and optional runtime packages."
      exit 0
      ;;
  esac
done

run_as_root() {
  if [ "$(id -u)" -eq 0 ]; then
    "$@"
  elif command -v sudo >/dev/null 2>&1; then
    sudo "$@"
  else
    echo "Warning: need root/sudo to install system packages ($*). Skipping." >&2
    return 1
  fi
}

install_runtime_deps() {
  if [ "$SKIP_DEPS" = true ]; then
    return 0
  fi

  if command -v apt-get >/dev/null 2>&1; then
    local pkgs=(libxkbcommon0 libxkbcommon-x11-0 libfontconfig1 libwayland-client0 libegl1 libgl1 libx11-6 libx11-xcb1)
    if [ -n "${WAYLAND_DISPLAY:-}" ] || [ "${XDG_SESSION_TYPE:-}" = "wayland" ]; then
      pkgs+=(wl-clipboard)
    fi
    run_as_root apt-get update && run_as_root apt-get install -y "${pkgs[@]}" || true
  elif command -v pacman >/dev/null 2>&1; then
    local pkgs=(libxkbcommon libxkbcommon-x11 fontconfig wayland libglvnd libx11 libxcb)
    if [ -n "${WAYLAND_DISPLAY:-}" ] || [ "${XDG_SESSION_TYPE:-}" = "wayland" ]; then
      pkgs+=(wl-clipboard)
    fi
    run_as_root pacman -Sy --needed --noconfirm "${pkgs[@]}" || true
  elif command -v dnf >/dev/null 2>&1; then
    local pkgs=(libxkbcommon libxkbcommon-x11 fontconfig wayland-client mesa-libEGL mesa-libGL libX11 libxcb)
    if [ -n "${WAYLAND_DISPLAY:-}" ] || [ "${XDG_SESSION_TYPE:-}" = "wayland" ]; then
      pkgs+=(wl-clipboard)
    fi
    run_as_root dnf install -y "${pkgs[@]}" || true
  elif command -v zypper >/dev/null 2>&1; then
    local pkgs=(libxkbcommon0 libxkbcommon-x11-0 fontconfig libwayland-client0 libEGL1 libGL1 libX11-6 libxcb1)
    if [ -n "${WAYLAND_DISPLAY:-}" ] || [ "${XDG_SESSION_TYPE:-}" = "wayland" ]; then
      pkgs+=(wl-clipboard)
    fi
    run_as_root zypper install -y "${pkgs[@]}" || true
  fi
}

if [ ! -f "$SCRIPT_DIR/tiarun" ]; then
  echo "Error: tiarun binary not found next to install.sh" >&2
  exit 1
fi

install_runtime_deps

if [ "$(id -u)" -eq 0 ]; then
  PREFIX="/usr/local"
else
  PREFIX="${HOME}/.local"
fi

BIN_DIR="${PREFIX}/bin"
APP_DIR="${PREFIX}/share/applications"
PIXMAP_DIR="${PREFIX}/share/pixmaps"
ICON_DIR="${PREFIX}/share/icons/hicolor/scalable/apps"

install -Dm755 "$SCRIPT_DIR/tiarun" "$BIN_DIR/tiarun"
if [ -f "$SCRIPT_DIR/assets/tiarun.desktop" ]; then
  install -Dm644 "$SCRIPT_DIR/assets/tiarun.desktop" "$APP_DIR/tiarun.desktop"
fi
if [ -f "$SCRIPT_DIR/assets/tiarun.png" ]; then
  install -Dm644 "$SCRIPT_DIR/assets/tiarun.png" "$PIXMAP_DIR/tiarun.png"
fi
if [ -f "$SCRIPT_DIR/assets/tiarun.svg" ]; then
  install -Dm644 "$SCRIPT_DIR/assets/tiarun.svg" "$ICON_DIR/tiarun.svg"
fi

if command -v update-desktop-database >/dev/null 2>&1; then
  update-desktop-database -q "$APP_DIR" || true
fi
if command -v gtk-update-icon-cache >/dev/null 2>&1; then
  gtk-update-icon-cache -q -t -f "${PREFIX}/share/icons/hicolor" 2>/dev/null || true
fi

echo "Installed tiarun to $BIN_DIR/tiarun"
