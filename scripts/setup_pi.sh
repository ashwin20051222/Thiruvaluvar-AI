#!/usr/bin/env bash
# Phase 6 Raspberry Pi 4B (aarch64) Hardware, Package, Model & Audio Setup Script
# Installs native ARM64 dependencies, downloads offline models (< 350 MB),
# indexes INT8 semantic embeddings if needed, configures ALSA & 3.5" LCD kiosk settings.
set -euo pipefail

ROOT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
export CARGO_TARGET_DIR="${CARGO_TARGET_DIR:-/tmp/kural-build}"

if [[ "${CARGO_TARGET_DIR}" == *" "* ]]; then
    printf 'ERROR: CARGO_TARGET_DIR (%s) must not contain spaces (tikv-jemalloc-sys requirement).\n' "${CARGO_TARGET_DIR}" >&2
    exit 1
fi

INPUT_DEV=""
OUTPUT_DEV=""
NON_INTERACTIVE=0
REINDEX=0
SKIP_APT=0
SKIP_AUDIO_TEST=0
WRITE_ASOUND=0

usage() {
    cat <<'EOF'
Usage: scripts/setup_pi.sh [OPTIONS]
  --input-device NAME    Set [audio].input_device in config.toml
  --output-device NAME   Set [audio].output_device in config.toml
  --write-asound         Configure /etc/asound.conf (dmix + dsnoop) for card 0/USB
  --reindex              Force re-indexing INT8 embeddings into data/thirukkural.db
  --skip-apt             Skip apt-get dependency installation
  --skip-audio-test      Skip interactive speaker/mic audio_test verification
  --non-interactive      Accept detected non-HDMI defaults without prompting
  -h, --help             Show this help
EOF
}

while [[ $# -gt 0 ]]; do
    case "$1" in
        --input-device)
            INPUT_DEV="${2:?missing value for --input-device}"
            shift 2
            ;;
        --output-device)
            OUTPUT_DEV="${2:?missing value for --output-device}"
            shift 2
            ;;
        --write-asound)
            WRITE_ASOUND=1
            shift
            ;;
        --reindex)
            REINDEX=1
            shift
            ;;
        --skip-apt)
            SKIP_APT=1
            shift
            ;;
        --skip-audio-test)
            SKIP_AUDIO_TEST=1
            shift
            ;;
        --non-interactive)
            NON_INTERACTIVE=1
            shift
            ;;
        -h|--help)
            usage
            exit 0
            ;;
        *)
            printf 'Unknown option: %s\n' "$1" >&2
            usage >&2
            exit 2
            ;;
    esac
done

log() {
    printf '[setup_pi] %s\n' "$*"
}

warn() {
    printf '[setup_pi][WARN] %s\n' "$*" >&2
}

ARCH="$(uname -m)"
if [[ "${ARCH}" != "aarch64" ]]; then
    warn "Host architecture is '${ARCH}' (expected Raspberry Pi 4B 64-bit 'aarch64')."
else
    export RUSTFLAGS="${RUSTFLAGS:-} -C target-cpu=cortex-a72"
fi

# ---------------------------------------------------------------------------
# 1. Native ARM64 System Dependencies (0 Python, minimal Wayland cage stack)
# ---------------------------------------------------------------------------
if [[ "${SKIP_APT}" -eq 0 ]]; then
    if command -v apt-get >/dev/null 2>&1; then
        log "Installing ARM64 system dependencies via apt-get..."
        sudo apt-get update
        sudo apt-get install -y --no-install-recommends \
            build-essential \
            cmake \
            pkg-config \
            curl \
            ca-certificates \
            alsa-utils \
            libasound2-dev \
            libwebkit2gtk-4.1-dev \
            libgtk-3-dev \
            espeak-ng \
            cage \
            fonts-noto-core \
            libinput-tools
    else
        warn "apt-get not found; ensure build-essential, cmake, pkg-config, libasound2-dev, libwebkit2gtk-4.1-dev, libgtk-3-dev, espeak-ng, cage, fonts-noto-core, and libinput-tools are installed."
    fi
fi

# Ensure Rust toolchain >= 1.85 is available
if ! command -v cargo >/dev/null 2>&1; then
    if [[ -x "${HOME}/.cargo/bin/cargo" ]]; then
        export PATH="${HOME}/.cargo/bin:${PATH}"
    else
        log "Rust toolchain not found; installing stable rustup toolchain..."
        curl --proto '=https' --tlsv1.2 -sSf https://sh.rustup.rs | sh -s -- -y --profile minimal
        export PATH="${HOME}/.cargo/bin:${PATH}"
    fi
fi
RUST_VER="$(rustc --version | awk '{print $2}')"
log "Rust toolchain: rustc ${RUST_VER}"
if [[ "$(printf '%s\n1.85.0\n' "${RUST_VER}" | sort -V | head -n1)" != "1.85.0" ]]; then
    printf 'ERROR: Rust >= 1.85.0 required (found %s). Run: rustup update stable\n' "${RUST_VER}" >&2
    exit 1
fi

# ---------------------------------------------------------------------------
# 2. Offline Edge Models (< 350 MB) & Database Embeddings Check
# ---------------------------------------------------------------------------
log "Fetching & verifying offline speech/embedding models (<= 350 MB gate)..."
"${ROOT_DIR}/scripts/download_fast_models.sh"

DB_PATH="${ROOT_DIR}/data/thirukkural.db"
if [[ ! -f "${DB_PATH}" ]]; then
    printf 'ERROR: %s not found. Ensure data/thirukkural.db is present before setup.\n' "${DB_PATH}" >&2
    exit 1
fi

log "Checking data/thirukkural.db stats & semantic vector count..."
STATS_OUT="$(cd "${ROOT_DIR}" && cargo run --release -p kural-db --features cli --bin kural_cli -- --db "${DB_PATH}" stats)"
printf '%s\n' "${STATS_OUT}"

NEED_INDEX=0
if [[ "${REINDEX}" -eq 1 ]]; then
    NEED_INDEX=1
elif ! printf '%s\n' "${STATS_OUT}" | grep -Eq 'kural_vectors:[[:space:]]*1330' \
    || ! printf '%s\n' "${STATS_OUT}" | grep -Eq 'kb_vectors:[[:space:]]*(4[5-9]|[5-9][0-9]|[1-9][0-9]{2,})'; then
    NEED_INDEX=1
fi

if [[ "${NEED_INDEX}" -eq 1 ]]; then
    log "Indexing 1,330 Kurals + 45 Knowledge Base INT8 vectors into data/thirukkural.db..."
    (cd "${ROOT_DIR}" && cargo run --release -p kural-db --features cli --bin index_embeddings -- --db "${DB_PATH}" --models-dir "${ROOT_DIR}/models/embeddings")
else
    log "Semantic embeddings already indexed (1330 Kurals + >= 45 KB); skipping re-index."
fi

# ---------------------------------------------------------------------------
# 3. ALSA Mic & Dual Amplified Stereo Speaker Configuration
# ---------------------------------------------------------------------------
log "Detected ALSA capture/playback hardware:"
arecord -l || true
aplay -l || true

CONFIG_FILE="${ROOT_DIR}/config.toml"
log "Building and inspecting audio_test hardware device enumeration..."
AUDIO_LIST_OUT="$(cd "${ROOT_DIR}" && cargo run --release -p kural-audio --features cli --bin audio_test -- --list 2>&1 || true)"
printf '%s\n' "${AUDIO_LIST_OUT}"

if [[ "${NON_INTERACTIVE}" -eq 0 && -z "${INPUT_DEV}" && -z "${OUTPUT_DEV}" && -t 0 ]]; then
    read -r -p "Enter ALSA input_device substring (blank = keep current in config.toml): " INPUT_DEV || true
    read -r -p "Enter ALSA output_device substring (blank = keep current in config.toml): " OUTPUT_DEV || true
fi

# Auto-detect first USB/I2S non-HDMI device when --non-interactive and unassigned
if [[ "${NON_INTERACTIVE}" -eq 1 && -z "${INPUT_DEV}" ]]; then
    INPUT_DEV="$(arecord -l 2>/dev/null | awk -F'[][]' '/^card / {print $2; exit}' || true)"
fi
if [[ "${NON_INTERACTIVE}" -eq 1 && -z "${OUTPUT_DEV}" ]]; then
    OUTPUT_DEV="$(aplay -l 2>/dev/null | awk -F'[][]' '/^card / && tolower($0) !~ /hdmi/ {print $2; exit}' || true)"
fi

if [[ "${WRITE_ASOUND}" -eq 1 ]]; then
    CAP_CARD="$(arecord -l 2>/dev/null | awk '/^card /{gsub(":", "", $2); print $2; exit}' || echo 0)"
    PLAY_CARD="$(aplay -l 2>/dev/null | awk '/^card / && tolower($0) !~ /hdmi/ {gsub(":", "", $2); print $2; exit}' || echo 0)"
    if [[ -f /etc/asound.conf ]]; then
        sudo cp /etc/asound.conf /etc/asound.conf.bak.kural
    fi
    sudo tee /etc/asound.conf >/dev/null <<EOF
# Generated by Thiruvalluvar AI scripts/setup_pi.sh
pcm.kural_play {
    type dmix
    ipc_key 1024
    slave {
        pcm "hw:${PLAY_CARD},0"
        period_time 0
        period_size 1024
        buffer_size 4096
        rate 48000
    }
}
pcm.kural_cap {
    type dsnoop
    ipc_key 1025
    slave {
        pcm "hw:${CAP_CARD},0"
        channels 1
        rate 48000
    }
}
pcm.!default {
    type asym
    playback.pcm "plug:kural_play"
    capture.pcm "plug:kural_cap"
}
ctl.!default {
    type hw
    card ${PLAY_CARD}
}
EOF
    sudo chmod 0644 /etc/asound.conf
    log "Wrote /etc/asound.conf (playback card ${PLAY_CARD} dmix, capture card ${CAP_CARD} dsnoop)."
fi

update_toml_audio_key() {
    local key="$1"
    local val="${2//\"/}"
    local tmp_cfg
    tmp_cfg="$(mktemp)"
    awk -v k="${key}" -v v="${val}" '
        /^\[audio\]/ { in_audio = 1 }
        /^\[/ && $0 !~ /^\[audio\]/ { in_audio = 0 }
        in_audio && $0 ~ "^" k "[[:space:]]*=" {
            printf "%s = \"%s\"\n", k, v
            next
        }
        { print }
    ' "${CONFIG_FILE}" > "${tmp_cfg}"
    mv "${tmp_cfg}" "${CONFIG_FILE}"
    chmod 0644 "${CONFIG_FILE}"
}

if [[ -n "${INPUT_DEV}" ]]; then
    update_toml_audio_key "input_device" "${INPUT_DEV}"
    log "Updated config.toml [audio].input_device = \"${INPUT_DEV}\""
fi
if [[ -n "${OUTPUT_DEV}" ]]; then
    update_toml_audio_key "output_device" "${OUTPUT_DEV}"
    log "Updated config.toml [audio].output_device = \"${OUTPUT_DEV}\""
fi

if [[ "${SKIP_AUDIO_TEST}" -eq 0 ]]; then
    log "Running 3s mic + stereo amplifier speaker verification (audio_test)..."
    if ! (cd "${ROOT_DIR}" && cargo run --release -p kural-audio --features cli --bin audio_test -- --config "${CONFIG_FILE}"); then
        warn "audio_test failed on active audio device. Re-run audio_test --list and adjust config.toml [audio] devices."
    fi
fi

# ---------------------------------------------------------------------------
# 4. Disable Screen Blanking & Configure 3.5-Inch LCD (480x320) + Touch
# ---------------------------------------------------------------------------
disable_screen_blanking() {
    if command -v systemctl >/dev/null 2>&1; then
        sudo mkdir -p /etc/systemd/logind.conf.d
        sudo tee /etc/systemd/logind.conf.d/99-kural-kiosk.conf >/dev/null <<'EOF'
[Login]
IdleAction=ignore
HandlePowerKey=ignore
EOF
        log "Configured logind idle/power-key guard (/etc/systemd/logind.conf.d/99-kural-kiosk.conf)."
    fi

    local cmdline=""
    for candidate in /boot/firmware/cmdline.txt /boot/cmdline.txt; do
        if [[ -f "${candidate}" ]]; then
            cmdline="${candidate}"
            break
        fi
    done
    if [[ -n "${cmdline}" ]]; then
        if ! grep -qw 'consoleblank=0' "${cmdline}"; then
            sudo cp "${cmdline}" "${cmdline}.bak.kural"
            sudo sed -i '1s/\r$//; 1s/$/ consoleblank=0/' "${cmdline}"
            log "Disabled Linux VC blanking via consoleblank=0 in ${cmdline} (backup: ${cmdline}.bak.kural)."
        else
            log "consoleblank=0 already active in ${cmdline}."
        fi
    fi
}

configure_lcd_and_touch() {
    local boot_cfg=""
    for candidate in /boot/firmware/config.txt /boot/config.txt; do
        if [[ -f "${candidate}" ]]; then
            boot_cfg="${candidate}"
            break
        fi
    done

    if [[ -n "${boot_cfg}" ]]; then
        log "Inspecting ${boot_cfg} for 3.5-inch 480x320 LCD KMS settings..."
        if ! grep -q 'vc4-kms-v3d' "${boot_cfg}"; then
            warn "${boot_cfg} does not reference vc4-kms-v3d; Wayland `cage` requires DRM/KMS (legacy fbtft /dev/fb1 without KMS cannot run cage)."
        fi
    fi

    if ! ls /dev/dri/card* >/dev/null 2>&1; then
        warn "No /dev/dri/card* DRM KMS device detected; Wayland cage kiosk requires KMS enabled."
    fi

    if command -v libinput >/dev/null 2>&1; then
        log "Detected libinput input devices:"
        sudo libinput list-devices | grep -E 'Device:|Kernel:|Calibration:' || true
    fi

    cat <<'EOF'
[setup_pi] 3.5" 480x320 Landscape Touchscreen & SD-Longevity Reference:
  - If touch axes are rotated vs display orientation, add:
      /etc/udev/rules.d/99-kural-touchscreen.rules
    e.g. 90-deg CW:
      ENV{ID_INPUT_TOUCHSCREEN}=="1", ENV{LIBINPUT_CALIBRATION_MATRIX}="0 1 0 -1 0 1"
    Apply via: sudo udevadm control --reload-rules && sudo udevadm trigger
  - For maximum microSD longevity on Raspberry Pi OS, add to /etc/fstab:
      tmpfs /tmp tmpfs defaults,noatime,nosuid,size=512M 0 0
EOF
}

if [[ "${ARCH}" == "aarch64" || -d /boot/firmware ]]; then
    disable_screen_blanking
fi
configure_lcd_and_touch

log "Pi setup finished. Next step: run ./scripts/build_pi_release.sh to compile & package /opt/thiruvalluvar-ai."
