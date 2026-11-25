#!/usr/bin/env bash
# Wayland `cage` single-application kiosk launcher for Raspberry Pi 4B + 3.5" LCD (480x320).
# Runs with zero desktop environment, zero Chromium, WebKitGTK audio isolated to fakesink,
# and ephemeral/TMP caches to protect Pi microSD longevity.
set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
APP_DIR="${APP_DIR:-$(cd "${SCRIPT_DIR}/.." && pwd)}"

# Isolate WebKitGTK GStreamer audio so kural-audio retains exclusive ALSA hardware ownership
export GST_AUDIO_SINK=fakesink

# Route transient caches & XDG runtime scratch to RAM (/tmp) instead of microSD
export XDG_CACHE_HOME=/tmp/kural-cache
export XDG_DATA_HOME=/tmp/kural-xdg-data
mkdir -p "${XDG_CACHE_HOME}" "${XDG_DATA_HOME}"

# Ensure XDG_RUNTIME_DIR exists cleanly for Wayland socket creation
if [[ -z "${XDG_RUNTIME_DIR:-}" ]]; then
    export XDG_RUNTIME_DIR="/tmp/kural-runtime-$(id -u)"
    mkdir -p "${XDG_RUNTIME_DIR}"
    chmod 0700 "${XDG_RUNTIME_DIR}"
fi

# Prevent WebKitGTK DMA-BUF blank frames on Pi 4B V3D Mesa & force native Wayland
export GDK_BACKEND=wayland
export WEBKIT_DISABLE_DMABUF_RENDERER="${WEBKIT_DISABLE_DMABUF_RENDERER:-1}"
export WLR_NO_HARDWARE_CURSORS="${WLR_NO_HARDWARE_CURSORS:-1}"

if ! command -v cage >/dev/null 2>&1; then
    printf 'ERROR: Wayland compositor `cage` is not installed. Run scripts/setup_pi.sh first.\n' >&2
    exit 1
fi

cd "${APP_DIR}"
exec cage -- "${APP_DIR}/kural-app" --config "${APP_DIR}/config.toml" --kiosk

