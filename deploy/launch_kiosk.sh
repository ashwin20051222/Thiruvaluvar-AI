#!/usr/bin/env bash
# Wayland `cage` single-application kiosk launcher for Raspberry Pi 4B.
# Supports single or dual displays cleanly with zero Rust code changes:
#   - 3.5-inch LCD only (480x320)            -> automatic native fullscreen
#   - 7-inch DSI/HDMI LCD only (800x480 / 1024x600) -> automatic native fullscreen
#   - Both 3.5" & 7" attached simultaneously -> set KURAL_DISPLAY_OUTPUT (and optional KURAL_DISPLAY_MODE)
#
# Environment Variables:
#   APP_DIR               Root bundle directory (default: parent of deploy/)
#   KURAL_CONFIG          Path to config.toml (default: ${APP_DIR}/config.toml)
#   KURAL_DISPLAY_OUTPUT  Optional Wayland output connector (e.g. HDMI-A-1, DSI-1, SPI-1, or "extend").
#                         Unset = auto-detect single connected display.
#                         Set to a connector name = enable that output via wlr-randr (if installed)
#                         and turn off secondary outputs so `cage` renders cleanly on that panel.
#   KURAL_DISPLAY_MODE    Optional WxH mode passed to wlr-randr (e.g. 800x480, 1024x600, 480x320).
#   KURAL_DISPLAY_MIRROR  Set to "1" when KURAL_DISPLAY_OUTPUT is set to keep secondary outputs active
#                         instead of turning them off (requires compatible KMS resolutions).
#
# Runs with zero desktop environment, zero Chromium, WebKitGTK audio isolated to fakesink,
# and ephemeral /tmp caches to protect Pi microSD longevity.
set -euo pipefail

usage() {
    cat <<'EOF'
Usage: deploy/launch_kiosk.sh [--help]

Launches Thiruvalluvar AI inside the Wayland `cage` single-app kiosk compositor.

Environment options:
  APP_DIR=/opt/thiruvalluvar-ai     Application directory (auto-detected by default)
  KURAL_CONFIG=<path>               Custom config.toml path
  KURAL_DISPLAY_OUTPUT=<connector>  Optional Wayland connector when 3.5" and 7" displays
                                    are both attached (e.g. HDMI-A-1, DSI-1, SPI-1, or "extend").
                                    Omit for automatic single-display fullscreen.
  KURAL_DISPLAY_MODE=<WxH>          Optional explicit mode for wlr-randr (e.g. 800x480, 1024x600)
  KURAL_DISPLAY_MIRROR=1            Keep secondary displays enabled while targeting KURAL_DISPLAY_OUTPUT

Examples:
  # Single display attached (3.5" 480x320 OR 7" 800x480 / 1024x600):
  ./deploy/launch_kiosk.sh

  # Both 3.5" and 7" attached — target 7" HDMI explicitly:
  KURAL_DISPLAY_OUTPUT=HDMI-A-1 KURAL_DISPLAY_MODE=1024x600 ./deploy/launch_kiosk.sh

  # Both attached — target 3.5" panel explicitly:
  KURAL_DISPLAY_OUTPUT=SPI-1 KURAL_DISPLAY_MODE=480x320 ./deploy/launch_kiosk.sh
EOF
}

if [[ "${1:-}" == "-h" || "${1:-}" == "--help" ]]; then
    usage
    exit 0
fi

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
APP_DIR="${APP_DIR:-$(cd "${SCRIPT_DIR}/.." && pwd)}"
KURAL_CONFIG="${KURAL_CONFIG:-${APP_DIR}/config.toml}"

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

TARGET_OUT="${KURAL_DISPLAY_OUTPUT:-}"
TARGET_MODE="${KURAL_DISPLAY_MODE:-}"
MIRROR_OUT="${KURAL_DISPLAY_MIRROR:-0}"

if [[ -z "${TARGET_OUT}" ]]; then
    exec cage -- "${APP_DIR}/kural-app" --config "${KURAL_CONFIG}" --kiosk
elif [[ "${TARGET_OUT}" == "extend" ]]; then
    exec cage -m extend -- "${APP_DIR}/kural-app" --config "${KURAL_CONFIG}" --kiosk
else
    export TARGET_OUT TARGET_MODE MIRROR_OUT APP_DIR KURAL_CONFIG
    exec cage -- sh -c '
        if command -v wlr-randr >/dev/null 2>&1; then
            if [ -n "${TARGET_MODE}" ]; then
                wlr-randr --output "${TARGET_OUT}" --on --mode "${TARGET_MODE}" || true
            else
                wlr-randr --output "${TARGET_OUT}" --on || true
            fi
            if [ "${MIRROR_OUT}" != "1" ]; then
                for out in $(wlr-randr 2>/dev/null | awk "/^[^ ]/ {print \$1}"); do
                    if [ "${out}" != "${TARGET_OUT}" ]; then
                        wlr-randr --output "${out}" --off || true
                    fi
                done
            fi
        else
            printf "[launch_kiosk][WARN] wlr-randr not found; ignoring KURAL_DISPLAY_OUTPUT=%s\n" "${TARGET_OUT}" >&2
        fi
        exec "${APP_DIR}/kural-app" --config "${KURAL_CONFIG}" --kiosk
    '
fi
