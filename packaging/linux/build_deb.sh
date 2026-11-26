#!/usr/bin/env bash
# Builds a native Debian / Ubuntu / Raspberry Pi OS .deb package for Thiruvalluvar AI.
# Uses dpkg-deb when available, or standard GNU ar + tar (Debian binary format 2.0) on non-Debian hosts.
set -euo pipefail

ROOT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")/../.." && pwd)"
export CARGO_TARGET_DIR="${CARGO_TARGET_DIR:-/tmp/kural-build}"
export CARGO_INCREMENTAL=0

HOST_UNAME_ARCH="$(uname -m)"
case "${HOST_UNAME_ARCH}" in
    x86_64|amd64) DEFAULT_DEB_ARCH="amd64" ;;
    aarch64|arm64) DEFAULT_DEB_ARCH="arm64" ;;
    *)
        printf 'ERROR: unsupported host architecture "%s"; pass --arch amd64|arm64 with --bin-dir.\n' "${HOST_UNAME_ARCH}" >&2
        exit 1
        ;;
esac

DEB_ARCH="${DEFAULT_DEB_ARCH}"
OUT_DIR="${ROOT_DIR}/dist/packages"
BIN_DIR=""
WITH_MODELS=0
DO_BUILD=0

usage() {
    cat <<'EOF'
Usage: build_deb.sh [OPTIONS]

Options:
  --arch <amd64|arm64>   Target Debian architecture (default: host arch)
  --bin-dir <DIR>        Directory containing prebuilt release binaries
  --out-dir <DIR>        Output directory for the .deb file (default: dist/packages)
  --with-models          Include models/ under /usr/share/thiruvalluvar-ai/models if populated
  --build                Run cargo build --release for target binaries before packaging
  -h, --help             Show this help message
EOF
}

while [[ $# -gt 0 ]]; do
    case "$1" in
        --arch)
            DEB_ARCH="${2:-}"
            shift 2
            ;;
        --bin-dir)
            BIN_DIR="${2:-}"
            shift 2
            ;;
        --out-dir)
            OUT_DIR="${2:-}"
            shift 2
            ;;
        --with-models)
            WITH_MODELS=1
            shift
            ;;
        --build)
            DO_BUILD=1
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

case "${DEB_ARCH}" in
    amd64|arm64) ;;
    aarch64) DEB_ARCH="arm64" ;;
    x86_64) DEB_ARCH="amd64" ;;
    *)
        printf 'ERROR: --arch must be amd64 or arm64 (got "%s").\n' "${DEB_ARCH}" >&2
        exit 1
        ;;
esac

if [[ "${CARGO_TARGET_DIR}" == *" "* ]]; then
    printf 'ERROR: CARGO_TARGET_DIR (%s) must not contain spaces.\n' "${CARGO_TARGET_DIR}" >&2
    exit 1
fi

log() {
    printf '[build_deb:%s] %s\n' "${DEB_ARCH}" "$*"
}

PKG_VERSION="${PKG_VERSION:-$(awk -F'"' '/^version[[:space:]]*=[[:space:]]*"/ { print $2; exit }' "${ROOT_DIR}/Cargo.toml")}"
PKG_VERSION="${PKG_VERSION:-0.1.0}"

if [[ -z "${BIN_DIR}" ]]; then
    if [[ "${DEB_ARCH}" == "${DEFAULT_DEB_ARCH}" ]]; then
        BIN_DIR="${CARGO_TARGET_DIR}/release"
    elif [[ -d "${CARGO_TARGET_DIR}/aarch64-unknown-linux-gnu/release" ]]; then
        BIN_DIR="${CARGO_TARGET_DIR}/aarch64-unknown-linux-gnu/release"
    elif [[ -d "${ROOT_DIR}/dist/thiruvalluvar-ai-pi" ]]; then
        BIN_DIR="${ROOT_DIR}/dist/thiruvalluvar-ai-pi"
    else
        BIN_DIR="${CARGO_TARGET_DIR}/aarch64-unknown-linux-gnu/release"
    fi
fi

if [[ "${DO_BUILD}" -eq 1 ]]; then
    if [[ "${DEB_ARCH}" != "${DEFAULT_DEB_ARCH}" ]]; then
        if [[ "${DEB_ARCH}" == "arm64" ]]; then
            if ! rustup target list --installed 2>/dev/null | grep -qx 'aarch64-unknown-linux-gnu'; then
                printf 'ERROR: cannot cross-compile arm64 on %s: Rust target aarch64-unknown-linux-gnu is not installed.\n' "${HOST_UNAME_ARCH}" >&2
                printf 'Run natively on Raspberry Pi 4B/5, or pass --bin-dir <dir-with-arm64-binaries>.\n' >&2
                exit 1
            fi
            if [[ -z "${PKG_CONFIG_SYSROOT_DIR:-}" ]] && [[ -z "${PKG_CONFIG_ALLOW_CROSS:-}" ]]; then
                printf 'ERROR: cross-compiling WebKitGTK/GTK3/ALSA for arm64 requires an arm64 sysroot (PKG_CONFIG_SYSROOT_DIR).\n' >&2
                printf 'Build on Raspberry Pi OS 64-bit, inside an arm64 container, or pass --bin-dir <arm64-bin-dir>.\n' >&2
                exit 1
            fi
            (
                cd "${ROOT_DIR}"
                export CARGO_TARGET_AARCH64_UNKNOWN_LINUX_GNU_LINKER="${CARGO_TARGET_AARCH64_UNKNOWN_LINUX_GNU_LINKER:-aarch64-linux-gnu-gcc}"
                export RUSTFLAGS="${RUSTFLAGS:-} -C target-cpu=cortex-a72"
                cargo build --release --target aarch64-unknown-linux-gnu \
                    --bin thiruvalluvar-ai \
                    --bin audio_test \
                    --bin ai_cli \
                    --bin kural_cli \
                    --bin index_embeddings
            )
            BIN_DIR="${CARGO_TARGET_DIR}/aarch64-unknown-linux-gnu/release"
        else
            printf 'ERROR: cross-compiling amd64 on %s is unsupported without an amd64 sysroot; pass --bin-dir.\n' "${HOST_UNAME_ARCH}" >&2
            exit 1
        fi
    else
        (
            cd "${ROOT_DIR}"
            if [[ "${HOST_UNAME_ARCH}" == "aarch64" ]]; then
                export RUSTFLAGS="${RUSTFLAGS:-} -C target-cpu=cortex-a72"
            fi
            cargo build --release \
                --bin thiruvalluvar-ai \
                --bin audio_test \
                --bin ai_cli \
                --bin kural_cli \
                --bin index_embeddings
        )
        BIN_DIR="${CARGO_TARGET_DIR}/release"
    fi
fi

# Locate required binaries (supports thiruvalluvar-ai or kural-app naming)
MAIN_BIN=""
if [[ -x "${BIN_DIR}/thiruvalluvar-ai" ]]; then
    MAIN_BIN="${BIN_DIR}/thiruvalluvar-ai"
elif [[ -x "${BIN_DIR}/kural-app" ]]; then
    MAIN_BIN="${BIN_DIR}/kural-app"
else
    printf 'ERROR: missing executable "%s/thiruvalluvar-ai" (or kural-app).\n' "${BIN_DIR}" >&2
    printf 'Build release binaries first or pass --build / --bin-dir <DIR>.\n' >&2
    exit 1
fi

AUDIO_BIN="${BIN_DIR}/audio_test"
AI_CLI_BIN="${BIN_DIR}/ai_cli"
KURAL_CLI_BIN="${BIN_DIR}/kural_cli"
INDEX_EMB_BIN="${BIN_DIR}/index_embeddings"

for req_bin in "${MAIN_BIN}" "${AUDIO_BIN}" "${AI_CLI_BIN}"; do
    if [[ ! -x "${req_bin}" ]]; then
        printf 'ERROR: missing required executable: %s\n' "${req_bin}" >&2
        exit 1
    fi
done

if command -v file >/dev/null 2>&1; then
    ELF_DESC="$(file -bL "${MAIN_BIN}")"
    if [[ "${DEB_ARCH}" == "arm64" ]] && ! grep -qi 'aarch64\|arm64' <<<"${ELF_DESC}"; then
        printf 'ERROR: binary %s is not arm64 ELF ("%s").\n' "${MAIN_BIN}" "${ELF_DESC}" >&2
        exit 1
    fi
    if [[ "${DEB_ARCH}" == "amd64" ]] && ! grep -qi 'x86-64\|x86_64' <<<"${ELF_DESC}"; then
        printf 'ERROR: binary %s is not amd64 ELF ("%s").\n' "${MAIN_BIN}" "${ELF_DESC}" >&2
        exit 1
    fi
fi

# Validate static packaging resources
for req_file in \
    "${ROOT_DIR}/data/thirukkural.db" \
    "${ROOT_DIR}/packaging/linux/config.package.toml" \
    "${ROOT_DIR}/packaging/linux/thiruvalluvar-ai.desktop" \
    "${ROOT_DIR}/packaging/linux/thiruvalluvar-ai-kiosk.service" \
    "${ROOT_DIR}/scripts/download_fast_models.sh"; do
    if [[ ! -f "${req_file}" ]]; then
        printf 'ERROR: missing required package asset: %s\n' "${req_file}" >&2
        exit 1
    fi
done

TMP_ROOT="$(mktemp -d "/tmp/kural-deb-${DEB_ARCH}.XXXXXX")"
trap 'rm -rf "${TMP_ROOT}"' EXIT

STAGE_DIR="${TMP_ROOT}/pkg"
AR_WORK_DIR="${TMP_ROOT}/ar"
mkdir -p \
    "${STAGE_DIR}/DEBIAN" \
    "${STAGE_DIR}/usr/bin" \
    "${STAGE_DIR}/usr/share/thiruvalluvar-ai/data" \
    "${STAGE_DIR}/usr/share/thiruvalluvar-ai/scripts" \
    "${STAGE_DIR}/usr/share/applications" \
    "${STAGE_DIR}/usr/share/pixmaps" \
    "${STAGE_DIR}/usr/share/doc/thiruvalluvar-ai" \
    "${STAGE_DIR}/lib/systemd/system" \
    "${AR_WORK_DIR}"

install -m 0755 "${MAIN_BIN}" "${STAGE_DIR}/usr/bin/thiruvalluvar-ai"
ln -s thiruvalluvar-ai "${STAGE_DIR}/usr/bin/kural-app"
install -m 0755 "${AUDIO_BIN}" "${STAGE_DIR}/usr/bin/thiruvalluvar-ai-audio-test"
install -m 0755 "${AI_CLI_BIN}" "${STAGE_DIR}/usr/bin/thiruvalluvar-ai-cli"

if [[ -x "${KURAL_CLI_BIN}" ]]; then
    install -m 0755 "${KURAL_CLI_BIN}" "${STAGE_DIR}/usr/bin/kural-cli"
fi
if [[ -x "${INDEX_EMB_BIN}" ]]; then
    install -m 0755 "${INDEX_EMB_BIN}" "${STAGE_DIR}/usr/bin/thiruvalluvar-ai-index-embeddings"
fi

# Strip ELF symbols cleanly for compact package footprint
STRIP_TOOL=""
if [[ "${DEB_ARCH}" == "${DEFAULT_DEB_ARCH}" ]] && command -v strip >/dev/null 2>&1; then
    STRIP_TOOL="strip"
elif [[ "${DEB_ARCH}" == "arm64" ]] && command -v aarch64-linux-gnu-strip >/dev/null 2>&1; then
    STRIP_TOOL="aarch64-linux-gnu-strip"
fi
if [[ -n "${STRIP_TOOL}" ]]; then
    find "${STAGE_DIR}/usr/bin" -maxdepth 1 -type f -exec "${STRIP_TOOL}" --strip-unneeded {} + 2>/dev/null || true
fi

install -m 0644 "${ROOT_DIR}/data/thirukkural.db" "${STAGE_DIR}/usr/share/thiruvalluvar-ai/data/thirukkural.db"
install -m 0644 "${ROOT_DIR}/packaging/linux/config.package.toml" "${STAGE_DIR}/usr/share/thiruvalluvar-ai/config.toml"
install -m 0755 "${ROOT_DIR}/scripts/download_fast_models.sh" "${STAGE_DIR}/usr/share/thiruvalluvar-ai/scripts/download_fast_models.sh"
install -m 0644 "${ROOT_DIR}/packaging/linux/thiruvalluvar-ai.desktop" "${STAGE_DIR}/usr/share/applications/thiruvalluvar-ai.desktop"
install -m 0644 "${ROOT_DIR}/packaging/linux/thiruvalluvar-ai-kiosk.service" "${STAGE_DIR}/lib/systemd/system/thiruvalluvar-ai-kiosk.service"

if [[ -f "${ROOT_DIR}/README.md" ]]; then
    install -m 0644 "${ROOT_DIR}/README.md" "${STAGE_DIR}/usr/share/doc/thiruvalluvar-ai/README.md"
fi

for sz in 32 48 64 128 256 512; do
    ICON_SRC="${ROOT_DIR}/packaging/linux/icons/${sz}x${sz}.png"
    if [[ ! -f "${ICON_SRC}" ]]; then
        printf 'ERROR: missing icon %s\n' "${ICON_SRC}" >&2
        exit 1
    fi
    ICON_DEST_DIR="${STAGE_DIR}/usr/share/icons/hicolor/${sz}x${sz}/apps"
    mkdir -p "${ICON_DEST_DIR}"
    install -m 0644 "${ICON_SRC}" "${ICON_DEST_DIR}/thiruvalluvar-ai.png"
done
install -m 0644 "${ROOT_DIR}/packaging/linux/icons/256x256.png" "${STAGE_DIR}/usr/share/pixmaps/thiruvalluvar-ai.png"

if [[ "${WITH_MODELS}" -eq 1 ]] && [[ -d "${ROOT_DIR}/models" ]] \
    && [[ -n "$(find "${ROOT_DIR}/models" -type f -print -quit 2>/dev/null)" ]]; then
    mkdir -p "${STAGE_DIR}/usr/share/thiruvalluvar-ai/models"
    cp -a "${ROOT_DIR}/models/." "${STAGE_DIR}/usr/share/thiruvalluvar-ai/models/"
    find "${STAGE_DIR}/usr/share/thiruvalluvar-ai/models" -type d -exec chmod 0755 {} +
    find "${STAGE_DIR}/usr/share/thiruvalluvar-ai/models" -type f -exec chmod 0644 {} +
    log "Bundled offline models into package."
fi

INSTALLED_KB="$(du -sk --exclude="${STAGE_DIR}/DEBIAN" "${STAGE_DIR}" | awk '{print $1}')"

cat > "${STAGE_DIR}/DEBIAN/control" <<EOF
Package: thiruvalluvar-ai
Version: ${PKG_VERSION}
Section: education
Priority: optional
Architecture: ${DEB_ARCH}
Maintainer: Thiruvalluvar AI Contributors <https://github.com/ashwin20051222/Thiruvaluvar-AI>
Homepage: https://github.com/ashwin20051222/Thiruvaluvar-AI
Installed-Size: ${INSTALLED_KB}
Depends: libc6, libwebkit2gtk-4.1-0, libgtk-3-0, libasound2t64 | libasound2, espeak-ng, fonts-noto-core, fonts-noto-extra
Recommends: alsa-utils, cage, libinput-tools
Description: Offline bilingual Tamil and English Thirukkural companion
 Thiruvalluvar AI is a pure-Rust offline desktop and Raspberry Pi kiosk
 application offering all 1,330 Thirukkural couplets with semantic search,
 bilingual commentary, age-adaptive modes, and local speech interaction.
EOF

cat > "${STAGE_DIR}/DEBIAN/postinst" <<'EOF'
#!/bin/sh
set -e

if command -v gtk-update-icon-cache >/dev/null 2>&1; then
    gtk-update-icon-cache -q -t -f /usr/share/icons/hicolor || true
fi
if command -v update-desktop-database >/dev/null 2>&1; then
    update-desktop-database -q /usr/share/applications || true
fi

if [ "$1" = "configure" ] && [ ! -d /usr/share/thiruvalluvar-ai/models/embeddings ]; then
    echo "Thiruvalluvar AI installed."
    echo "To download optional offline voice & semantic models (<350 MB), run:"
    echo "  sudo MODELS_DIR=/usr/share/thiruvalluvar-ai/models /usr/share/thiruvalluvar-ai/scripts/download_fast_models.sh"
fi
EOF

cat > "${STAGE_DIR}/DEBIAN/prerm" <<'EOF'
#!/bin/sh
set -e

if [ "$1" = "remove" ] && command -v systemctl >/dev/null 2>&1; then
    if systemctl is-active --quiet thiruvalluvar-ai-kiosk.service 2>/dev/null; then
        systemctl stop thiruvalluvar-ai-kiosk.service || true
    fi
fi
EOF

cat > "${STAGE_DIR}/DEBIAN/postrm" <<'EOF'
#!/bin/sh
set -e

if command -v gtk-update-icon-cache >/dev/null 2>&1; then
    gtk-update-icon-cache -q -t -f /usr/share/icons/hicolor || true
fi
if command -v update-desktop-database >/dev/null 2>&1; then
    update-desktop-database -q /usr/share/applications || true
fi
EOF

chmod 0644 "${STAGE_DIR}/DEBIAN/control"
chmod 0755 "${STAGE_DIR}/DEBIAN/postinst" "${STAGE_DIR}/DEBIAN/prerm" "${STAGE_DIR}/DEBIAN/postrm"

if command -v md5sum >/dev/null 2>&1; then
    (
        cd "${STAGE_DIR}"
        find usr lib -type f -print0 | LC_ALL=C sort -z | xargs -0 md5sum > DEBIAN/md5sums
        chmod 0644 DEBIAN/md5sums
    )
fi

mkdir -p "${OUT_DIR}"
OUT_DEB="${OUT_DIR}/thiruvalluvar-ai_${PKG_VERSION}_${DEB_ARCH}.deb"
rm -f "${OUT_DEB}"

if command -v dpkg-deb >/dev/null 2>&1; then
    log "Building .deb with dpkg-deb -> ${OUT_DEB}"
    if dpkg-deb --help 2>/dev/null | grep -q -- '--root-owner-group'; then
        dpkg-deb --root-owner-group --build "${STAGE_DIR}" "${OUT_DEB}"
    else
        dpkg-deb --build "${STAGE_DIR}" "${OUT_DEB}"
    fi
else
    log "dpkg-deb unavailable on host; building Debian 2.0 archive via GNU ar + tar -> ${OUT_DEB}"
    printf '2.0\n' > "${AR_WORK_DIR}/debian-binary"
    (
        cd "${STAGE_DIR}/DEBIAN"
        tar --numeric-owner --owner=0 --group=0 -czf "${AR_WORK_DIR}/control.tar.gz" .
    )
    if command -v xz >/dev/null 2>&1; then
        DATA_TAR="data.tar.xz"
        (
            cd "${STAGE_DIR}"
            XZ_OPT="${XZ_OPT:--T0 -6}" tar --numeric-owner --owner=0 --group=0 --exclude='./DEBIAN' -cJf "${AR_WORK_DIR}/${DATA_TAR}" .
        )
    else
        DATA_TAR="data.tar.gz"
        (
            cd "${STAGE_DIR}"
            tar --numeric-owner --owner=0 --group=0 --exclude='./DEBIAN' -czf "${AR_WORK_DIR}/${DATA_TAR}" .
        )
    fi
    (
        cd "${AR_WORK_DIR}"
        ar rcSD "${OUT_DEB}" debian-binary control.tar.gz "${DATA_TAR}" 2>/dev/null \
            || ar rc "${OUT_DEB}" debian-binary control.tar.gz "${DATA_TAR}"
    )
fi

SIZE_HR="$(du -sh "${OUT_DEB}" | awk '{print $1}')"
log "Created ${OUT_DEB} (${SIZE_HR})"

