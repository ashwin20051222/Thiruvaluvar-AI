#!/usr/bin/env bash
# Unified Phase 3 Linux Desktop & Raspberry Pi Package Builder for Thiruvalluvar AI.
# Pure Rust + POSIX/Linux tooling (zero Python). Builds .deb and (on Arch) .pkg.tar.zst + SHA256SUMS.
set -euo pipefail

ROOT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
export CARGO_TARGET_DIR="${CARGO_TARGET_DIR:-/tmp/kural-build}"
export CARGO_INCREMENTAL=0

OUT_DIR="${ROOT_DIR}/dist/packages"
ARM64_BIN_DIR=""
WITH_MODELS=0
SKIP_BUILD=0

usage() {
    cat <<'EOF'
Usage: build_all_packages.sh [OPTIONS]

Options:
  --out-dir <DIR>         Output directory for packages and SHA256SUMS (default: dist/packages)
  --arm64-bin-dir <DIR>   Optional directory of prebuilt aarch64 release binaries for arm64 .deb
  --with-models           Include models/ in generated packages when locally populated
  --skip-build            Reuse existing binaries in CARGO_TARGET_DIR/release
  -h, --help              Show this help message
EOF
}

while [[ $# -gt 0 ]]; do
    case "$1" in
        --out-dir)
            OUT_DIR="${2:-}"
            shift 2
            ;;
        --arm64-bin-dir)
            ARM64_BIN_DIR="${2:-}"
            shift 2
            ;;
        --with-models)
            WITH_MODELS=1
            shift
            ;;
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
    printf '[build_all_packages] %s\n' "$*"
}

HOST_ARCH="$(uname -m)"
case "${HOST_ARCH}" in
    x86_64|amd64) HOST_DEB_ARCH="amd64" ;;
    aarch64|arm64) HOST_DEB_ARCH="arm64" ;;
    *)
        printf 'ERROR: unsupported host architecture: %s\n' "${HOST_ARCH}" >&2
        exit 1
        ;;
esac

if [[ "${SKIP_BUILD}" -eq 0 ]]; then
    log "Building release binaries (CARGO_TARGET_DIR=${CARGO_TARGET_DIR}, CARGO_INCREMENTAL=${CARGO_INCREMENTAL})..."
    (
        cd "${ROOT_DIR}"
        if [[ "${HOST_ARCH}" == "aarch64" ]]; then
            export RUSTFLAGS="${RUSTFLAGS:-} -C target-cpu=cortex-a72"
        fi
        cargo build --release \
            --bin thiruvalluvar-ai \
            --bin audio_test \
            --bin ai_cli \
            --bin kural_cli \
            --bin index_embeddings
    )
fi

RELEASE_DIR="${CARGO_TARGET_DIR}/release"
for bin in thiruvalluvar-ai audio_test ai_cli kural_cli index_embeddings; do
    if [[ ! -x "${RELEASE_DIR}/${bin}" ]]; then
        printf 'ERROR: missing built binary: %s/%s\n' "${RELEASE_DIR}" "${bin}" >&2
        exit 1
    fi
done

mkdir -p "${OUT_DIR}"
ABS_OUT_DIR="$(cd "${OUT_DIR}" && pwd)"

EXTRA_ARGS=()
if [[ "${WITH_MODELS}" -eq 1 ]]; then
    EXTRA_ARGS+=(--with-models)
fi

# 1. Host Debian/Ubuntu/Raspberry Pi OS .deb package
log "Building ${HOST_DEB_ARCH} .deb package..."
"${ROOT_DIR}/packaging/linux/build_deb.sh" \
    --arch "${HOST_DEB_ARCH}" \
    --bin-dir "${RELEASE_DIR}" \
    --out-dir "${ABS_OUT_DIR}" \
    "${EXTRA_ARGS[@]}"

# 2. Optional arm64 .deb when running on x86_64 and prebuilt arm64 binaries are available
if [[ "${HOST_DEB_ARCH}" == "amd64" ]]; then
    CANDIDATE_ARM64_DIR="${ARM64_BIN_DIR}"
    if [[ -z "${CANDIDATE_ARM64_DIR}" ]] && [[ -x "${CARGO_TARGET_DIR}/aarch64-unknown-linux-gnu/release/thiruvalluvar-ai" ]]; then
        CANDIDATE_ARM64_DIR="${CARGO_TARGET_DIR}/aarch64-unknown-linux-gnu/release"
    elif [[ -z "${CANDIDATE_ARM64_DIR}" ]] && [[ -x "${ROOT_DIR}/dist/thiruvalluvar-ai-pi/kural-app" ]] \
        && command -v file >/dev/null 2>&1 \
        && file -bL "${ROOT_DIR}/dist/thiruvalluvar-ai-pi/kural-app" | grep -qi 'aarch64\|arm64'; then
        CANDIDATE_ARM64_DIR="${ROOT_DIR}/dist/thiruvalluvar-ai-pi"
    fi

    if [[ -n "${CANDIDATE_ARM64_DIR}" ]]; then
        log "Found prebuilt arm64 binaries at ${CANDIDATE_ARM64_DIR}; building arm64 .deb package..."
        "${ROOT_DIR}/packaging/linux/build_deb.sh" \
            --arch arm64 \
            --bin-dir "${CANDIDATE_ARM64_DIR}" \
            --out-dir "${ABS_OUT_DIR}" \
            "${EXTRA_ARGS[@]}"
    else
        log "NOTE: skipping arm64 .deb on x86_64 host (no arm64 WebKitGTK sysroot / --arm64-bin-dir)."
        log "      Build native arm64 .deb on Raspberry Pi OS via: ./packaging/linux/build_deb.sh --arch arm64 --build"
    fi
fi

# 3. Native Arch Linux .pkg.tar.zst package when makepkg is installed
if command -v makepkg >/dev/null 2>&1; then
    log "Building Arch Linux .pkg.tar.zst package..."
    "${ROOT_DIR}/packaging/arch/build_arch_pkg.sh" \
        --bin-dir "${RELEASE_DIR}" \
        --out-dir "${ABS_OUT_DIR}" \
        "${EXTRA_ARGS[@]}"
else
    log "NOTE: makepkg not installed; skipping Arch Linux .pkg.tar.zst package."
fi

# 4. Write deterministic SHA256SUMS for generated packages
(
    cd "${ABS_OUT_DIR}"
    PKG_LIST=()
    shopt -s nullglob
    for f in *.deb *.pkg.tar.zst; do
        PKG_LIST+=("$f")
    done
    shopt -u nullglob
    if [[ ${#PKG_LIST[@]} -eq 0 ]]; then
        printf 'ERROR: no package artifacts found in %s\n' "${ABS_OUT_DIR}" >&2
        exit 1
    fi
    sha256sum "${PKG_LIST[@]}" > SHA256SUMS
)

log "Generated package artifacts in ${ABS_OUT_DIR}:"
ls -lh "${ABS_OUT_DIR}"/*.deb "${ABS_OUT_DIR}"/*.pkg.tar.zst "${ABS_OUT_DIR}/SHA256SUMS" 2>/dev/null || true

