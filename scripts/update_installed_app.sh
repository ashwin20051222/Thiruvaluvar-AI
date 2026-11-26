#!/usr/bin/env bash
# Update the system-installed Thiruvalluvar AI package (Arch / Debian / Ubuntu / Raspberry Pi OS),
# patch installed Tamil Piper ONNX metadata, re-index embeddings, and run TTS verification.
set -euo pipefail

REPO_ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
export CARGO_TARGET_DIR="${CARGO_TARGET_DIR:-/tmp/kural-build}"
export CARGO_INCREMENTAL=0

INSTALLED_ROOT="/usr/share/thiruvalluvar-ai"
INSTALLED_MODELS_DIR="${INSTALLED_ROOT}/models"
INSTALLED_TA_TTS_DIR="${INSTALLED_MODELS_DIR}/tts/ta"
INSTALLED_EMB_DIR="${INSTALLED_MODELS_DIR}/embeddings"
INSTALLED_DB="${INSTALLED_ROOT}/data/thirukkural.db"
INDEX_BIN="/usr/bin/thiruvalluvar-ai-index-embeddings"

SKIP_BUILD=0

usage() {
    cat <<'EOF'
Usage: scripts/update_installed_app.sh [OPTIONS]

Options:
  --skip-build    Skip rebuilding packages and ai_cli; install latest built package in dist/packages/
  -h, --help      Show this help message
EOF
}

while [[ $# -gt 0 ]]; do
    case "$1" in
        --skip-build)
            SKIP_BUILD=1
            shift
            ;;
        -h|--help)
            usage
            exit 0
            ;;
        *)
            printf 'ERROR: unknown option: %s\n' "$1" >&2
            usage >&2
            exit 1
            ;;
    esac
done

if [[ "${CARGO_TARGET_DIR}" == *" "* ]]; then
    printf 'ERROR: CARGO_TARGET_DIR (%s) must not contain spaces.\n' "${CARGO_TARGET_DIR}" >&2
    exit 1
fi

log() {
    printf '[update_installed_app] %s\n' "$*"
}

warn() {
    printf '[update_installed_app][WARN] %s\n' "$*" >&2
}

build_ai_cli() {
    log "Building ai_cli release binary (CARGO_TARGET_DIR=${CARGO_TARGET_DIR}, CARGO_INCREMENTAL=${CARGO_INCREMENTAL})..."
    (
        cd "${REPO_ROOT}"
        CARGO_TARGET_DIR="${CARGO_TARGET_DIR}" CARGO_INCREMENTAL=0 \
            cargo build --release -p kural-ai --features cli --bin ai_cli
    )
}

if [[ "${SKIP_BUILD}" -eq 0 ]]; then
    log "Building Linux packages via scripts/build_all_packages.sh..."
    CARGO_TARGET_DIR="${CARGO_TARGET_DIR}" CARGO_INCREMENTAL=0 \
        "${REPO_ROOT}/scripts/build_all_packages.sh"
    build_ai_cli
fi

PKG_DIR="${REPO_ROOT}/dist/packages"
if [[ ! -d "${PKG_DIR}" ]]; then
    printf 'ERROR: package directory not found: %s\n' "${PKG_DIR}" >&2
    exit 1
fi

if command -v pacman >/dev/null 2>&1; then
    PKG="$(ls -1t "${PKG_DIR}"/thiruvalluvar-ai-*.pkg.tar.zst 2>/dev/null | head -n1 || true)"
    if [[ -z "${PKG}" ]] || [[ ! -f "${PKG}" ]]; then
        printf 'ERROR: no matching .pkg.tar.zst package found in %s\n' "${PKG_DIR}" >&2
        exit 1
    fi
    log "Installing Arch Linux package: ${PKG}"
    sudo pacman -U --noconfirm "${PKG}"
elif command -v dpkg >/dev/null 2>&1 && command -v apt-get >/dev/null 2>&1; then
    DEB_ARCH="$(dpkg --print-architecture)"
    PKG="$(ls -1t "${PKG_DIR}"/thiruvalluvar-ai_*_"${DEB_ARCH}".deb 2>/dev/null | head -n1 || true)"
    if [[ -z "${PKG}" ]] || [[ ! -f "${PKG}" ]]; then
        printf 'ERROR: no matching %s .deb package found in %s\n' "${DEB_ARCH}" "${PKG_DIR}" >&2
        exit 1
    fi
    log "Installing Debian/Ubuntu/Raspberry Pi OS package (${DEB_ARCH}): ${PKG}"
    if ! sudo apt-get install -y "${PKG}"; then
        warn "apt-get install failed for ${PKG}; falling back to dpkg -i + apt-get -f install."
        sudo dpkg -i "${PKG}"
        sudo apt-get -f install -y
    fi
else
    printf 'ERROR: unsupported Linux distribution (neither pacman nor dpkg+apt-get found).\n' >&2
    exit 1
fi

AI_CLI=""
if [[ -x "${CARGO_TARGET_DIR}/release/ai_cli" ]]; then
    AI_CLI="${CARGO_TARGET_DIR}/release/ai_cli"
elif [[ -x "/tmp/kural-build/release/ai_cli" ]]; then
    AI_CLI="/tmp/kural-build/release/ai_cli"
else
    build_ai_cli
    AI_CLI="${CARGO_TARGET_DIR}/release/ai_cli"
fi

if [[ ! -x "${AI_CLI}" ]]; then
    printf 'ERROR: ai_cli binary not found at %s\n' "${AI_CLI}" >&2
    exit 1
fi

if [[ -d "${INSTALLED_TA_TTS_DIR}" ]] && [[ -n "$(find "${INSTALLED_TA_TTS_DIR}" -maxdepth 2 -name "*.onnx" -print -quit 2>/dev/null)" ]]; then
    log "Patching installed Tamil Piper ONNX metadata in ${INSTALLED_TA_TTS_DIR}..."
    sudo "${AI_CLI}" patch-piper-onnx "${INSTALLED_TA_TTS_DIR}"
else
    warn "Installed Tamil TTS directory ${INSTALLED_TA_TTS_DIR} has no .onnx model; skipping patch-piper-onnx."
fi

if [[ ! -x "${INDEX_BIN}" ]]; then
    printf 'ERROR: missing installed embedding indexer at %s\n' "${INDEX_BIN}" >&2
    exit 1
fi

if [[ -f "${INSTALLED_EMB_DIR}/model.safetensors" ]] && [[ -f "${INSTALLED_DB}" ]]; then
    log "Re-indexing embeddings in ${INSTALLED_DB} using ${INSTALLED_EMB_DIR}..."
    if [[ -w "${INSTALLED_DB}" ]]; then
        "${INDEX_BIN}" --db "${INSTALLED_DB}" --models-dir "${INSTALLED_EMB_DIR}"
    else
        sudo "${INDEX_BIN}" --db "${INSTALLED_DB}" --models-dir "${INSTALLED_EMB_DIR}"
    fi
else
    warn "Installed embeddings (${INSTALLED_EMB_DIR}/model.safetensors) or DB (${INSTALLED_DB}) missing; skipping re-index."
fi

if [[ -d "${INSTALLED_MODELS_DIR}" ]] && [[ -n "$(find "${INSTALLED_MODELS_DIR}" -maxdepth 3 -name "*.onnx" -print -quit 2>/dev/null)" ]]; then
    log "Running TTS verification against ${INSTALLED_MODELS_DIR}..."
    "${AI_CLI}" --tts-check --models-dir "${INSTALLED_MODELS_DIR}"
else
    warn "Installed models directory ${INSTALLED_MODELS_DIR} is unpopulated; skipping --tts-check."
fi

log "Installed app update complete."
