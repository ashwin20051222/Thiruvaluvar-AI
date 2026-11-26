#!/usr/bin/env bash
# Builds a native Arch Linux .pkg.tar.zst package using makepkg in a space-free temporary BUILDDIR.
set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
ROOT_DIR="$(cd "${SCRIPT_DIR}/../.." && pwd)"
export CARGO_TARGET_DIR="${CARGO_TARGET_DIR:-/tmp/kural-build}"
export CARGO_INCREMENTAL=0

OUT_DIR="${ROOT_DIR}/dist/packages"
BIN_DIR=""
WITH_MODELS=0

usage() {
    cat <<'EOF'
Usage: build_arch_pkg.sh [OPTIONS]

Options:
  --out-dir <DIR>    Output directory for .pkg.tar.zst (default: dist/packages)
  --bin-dir <DIR>    Directory containing prebuilt release binaries
  --with-models      Include models/ under /usr/share/thiruvalluvar-ai/models if populated
  -h, --help         Show this help message
EOF
}

while [[ $# -gt 0 ]]; do
    case "$1" in
        --out-dir)
            OUT_DIR="${2:-}"
            shift 2
            ;;
        --bin-dir)
            BIN_DIR="${2:-}"
            shift 2
            ;;
        --with-models)
            WITH_MODELS=1
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

if ! command -v makepkg >/dev/null 2>&1; then
    printf 'ERROR: makepkg not found (must run on Arch Linux / pacman-based host).\n' >&2
    exit 1
fi

if [[ "${CARGO_TARGET_DIR}" == *" "* ]]; then
    printf 'ERROR: CARGO_TARGET_DIR (%s) must not contain spaces.\n' "${CARGO_TARGET_DIR}" >&2
    exit 1
fi

mkdir -p "${OUT_DIR}"
ABS_OUT_DIR="$(cd "${OUT_DIR}" && pwd)"

TMP_MAKEPKG="$(mktemp -d /tmp/kural-makepkg.XXXXXX)"
trap 'rm -rf "${TMP_MAKEPKG}"' EXIT

export BUILDDIR="${TMP_MAKEPKG}/build"
export PKGDEST="${ABS_OUT_DIR}"
export SRCDEST="${TMP_MAKEPKG}/srcdest"
export LOGDEST="${TMP_MAKEPKG}/logdest"
export PKGEXT='.pkg.tar.zst'
export KURAL_SRC_ROOT="${ROOT_DIR}"
export KURAL_WITH_MODELS="${WITH_MODELS}"
if [[ -n "${BIN_DIR}" ]]; then
    export KURAL_PREBUILT_BIN_DIR="${BIN_DIR}"
fi

mkdir -p "${BUILDDIR}" "${SRCDEST}" "${LOGDEST}"

printf '[build_arch_pkg] Running makepkg -f -> %s\n' "${ABS_OUT_DIR}"
(
    cd "${SCRIPT_DIR}"
    makepkg -f --noconfirm
)

PKG_FILE="$(ls -1t "${ABS_OUT_DIR}"/thiruvalluvar-ai-*.pkg.tar.zst 2>/dev/null | head -n1 || true)"
if [[ -z "${PKG_FILE}" ]] || [[ ! -f "${PKG_FILE}" ]]; then
    printf 'ERROR: makepkg did not produce a .pkg.tar.zst file in %s\n' "${ABS_OUT_DIR}" >&2
    exit 1
fi

SIZE_HR="$(du -sh "${PKG_FILE}" | awk '{print $1}')"
printf '[build_arch_pkg] Created %s (%s)\n' "${PKG_FILE}" "${SIZE_HR}"

