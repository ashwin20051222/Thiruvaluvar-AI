#!/usr/bin/env bash
# Phase 6 Release Builder & Packager for Raspberry Pi 4B (ARM64) & Linux Desktop
# Enforces a space-free CARGO_TARGET_DIR=/tmp/kural-build so tikv-jemalloc-sys autoconf succeeds.
set -euo pipefail

ROOT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
DIST_DIR="${DIST_DIR:-${ROOT_DIR}/dist/thiruvalluvar-ai-pi}"
export CARGO_TARGET_DIR="${CARGO_TARGET_DIR:-/tmp/kural-build}"

log() {
    printf '[build_pi_release] %s\n' "$*"
}

if [[ "${CARGO_TARGET_DIR}" == *" "* ]]; then
    printf 'ERROR: CARGO_TARGET_DIR (%s) must not contain spaces (tikv-jemalloc-sys requirement).\n' "${CARGO_TARGET_DIR}" >&2
    exit 1
fi

ARCH="$(uname -m)"
if [[ "${ARCH}" == "aarch64" ]]; then
    export RUSTFLAGS="${RUSTFLAGS:-} -C target-cpu=cortex-a72"
    log "Building ARM64 release binaries with RUSTFLAGS='${RUSTFLAGS}' (target dir: ${CARGO_TARGET_DIR})"
else
    log "Building host '${ARCH}' release binaries cleanly (target dir: ${CARGO_TARGET_DIR})"
fi

cd "${ROOT_DIR}"

cargo build --release \
    --bin kural-app \
    --bin audio_test \
    --bin ai_cli \
    --bin kural_cli \
    --bin index_embeddings

RELEASE_DIR="${CARGO_TARGET_DIR}/release"
for bin in kural-app audio_test ai_cli kural_cli index_embeddings; do
    if [[ ! -x "${RELEASE_DIR}/${bin}" ]]; then
        printf 'ERROR: expected release binary missing: %s/%s\n' "${RELEASE_DIR}" "${bin}" >&2
        exit 1
    fi
done

if [[ ! -f "${ROOT_DIR}/data/thirukkural.db" ]]; then
    printf 'ERROR: missing %s/data/thirukkural.db\n' "${ROOT_DIR}" >&2
    exit 1
fi

# Package clean self-contained bundle
rm -rf "${DIST_DIR}"
mkdir -p "${DIST_DIR}/data" "${DIST_DIR}/deploy" "${DIST_DIR}/docs" "${DIST_DIR}/models" "${DIST_DIR}/scripts"

for bin in kural-app audio_test ai_cli kural_cli index_embeddings; do
    cp "${RELEASE_DIR}/${bin}" "${DIST_DIR}/${bin}"
    chmod 0755 "${DIST_DIR}/${bin}"
done

cp "${ROOT_DIR}/config.toml" "${DIST_DIR}/config.toml"
cp "${ROOT_DIR}/data/thirukkural.db" "${DIST_DIR}/data/thirukkural.db"
cp "${ROOT_DIR}/deploy/launch_kiosk.sh" "${DIST_DIR}/deploy/launch_kiosk.sh"
cp "${ROOT_DIR}/deploy/thiruvalluvar-ai.service" "${DIST_DIR}/deploy/thiruvalluvar-ai.service"
cp "${ROOT_DIR}/scripts/download_fast_models.sh" "${DIST_DIR}/scripts/download_fast_models.sh"
cp "${ROOT_DIR}/scripts/setup_pi.sh" "${DIST_DIR}/scripts/setup_pi.sh"
chmod 0755 "${DIST_DIR}/deploy/launch_kiosk.sh" "${DIST_DIR}/scripts/download_fast_models.sh" "${DIST_DIR}/scripts/setup_pi.sh"
if [[ -f "${ROOT_DIR}/README.md" ]]; then
    cp "${ROOT_DIR}/README.md" "${DIST_DIR}/README.md"
fi
if [[ -f "${ROOT_DIR}/docs/PI_DEPLOYMENT_GUIDE.md" ]]; then
    cp "${ROOT_DIR}/docs/PI_DEPLOYMENT_GUIDE.md" "${DIST_DIR}/docs/PI_DEPLOYMENT_GUIDE.md"
fi

# Force kiosk viewport mode inside packaged bundle config template
sed -i 's/^viewport_mode[[:space:]]*=.*/viewport_mode = "kiosk"/' "${DIST_DIR}/config.toml"

if [[ -d "${ROOT_DIR}/models" ]] && [[ -n "$(find "${ROOT_DIR}/models" -type f -print -quit 2>/dev/null)" ]]; then
    cp -a "${ROOT_DIR}/models/." "${DIST_DIR}/models/"
    MODELS_MB="$(du -sm "${DIST_DIR}/models" | awk '{print $1}')"
    log "Packaged models/ size: ${MODELS_MB} MB"
else
    log "NOTE: models/ directory empty locally; run ${DIST_DIR}/scripts/download_fast_models.sh on target Pi."
fi

cat > "${DIST_DIR}/install_to_opt.sh" <<'EOF'
#!/usr/bin/env bash
set -euo pipefail
BUNDLE_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
TARGET_DIR="/opt/thiruvalluvar-ai"
SERVICE_USER="${KURAL_KIOSK_USER:-pi}"

if [[ "${SERVICE_USER}" == "root" ]]; then
    printf 'ERROR: refusing to run kiosk service as root; set KURAL_KIOSK_USER=<non-root user>.\n' >&2
    exit 1
fi
if ! id "${SERVICE_USER}" >/dev/null 2>&1; then
    printf 'ERROR: service user "%s" does not exist. Create it or set KURAL_KIOSK_USER=<existing user>.\n' "${SERVICE_USER}" >&2
    exit 1
fi

if systemctl is-active --quiet thiruvalluvar-ai.service 2>/dev/null; then
    sudo systemctl stop thiruvalluvar-ai.service
fi

sudo mkdir -p "${TARGET_DIR}" "${TARGET_DIR}/data"
PRESERVED_CFG=""
if [[ -f "${TARGET_DIR}/config.toml" ]]; then
    PRESERVED_CFG="$(mktemp)"
    sudo cp "${TARGET_DIR}/config.toml" "${PRESERVED_CFG}"
fi

sudo cp -a "${BUNDLE_DIR}/." "${TARGET_DIR}/"
if [[ -n "${PRESERVED_CFG}" ]]; then
    sudo mv "${TARGET_DIR}/config.toml" "${TARGET_DIR}/config.toml.default"
    sudo cp "${PRESERVED_CFG}" "${TARGET_DIR}/config.toml"
    sudo chmod 0644 "${TARGET_DIR}/config.toml"
    rm -f "${PRESERVED_CFG}"
    printf '[install] Preserved existing %s/config.toml (new template saved to config.toml.default).\n' "${TARGET_DIR}"
fi

sudo chown -R "${SERVICE_USER}:${SERVICE_USER}" "${TARGET_DIR}"
sudo sed "s/^User=.*/User=${SERVICE_USER}/" "${TARGET_DIR}/deploy/thiruvalluvar-ai.service" \
    | sudo tee /etc/systemd/system/thiruvalluvar-ai.service >/dev/null
sudo systemctl daemon-reload
sudo systemctl enable thiruvalluvar-ai.service
printf '[install] Installed to %s and enabled thiruvalluvar-ai.service (user: %s).\n' "${TARGET_DIR}" "${SERVICE_USER}"
printf '[install] Start immediately with: sudo systemctl start thiruvalluvar-ai.service\n'
EOF
chmod 0755 "${DIST_DIR}/install_to_opt.sh"

(
    cd "${DIST_DIR}"
    sha256sum kural-app audio_test ai_cli kural_cli index_embeddings config.toml data/thirukkural.db deploy/* scripts/* > SHA256SUMS
)

BUNDLE_MB="$(du -sm "${DIST_DIR}" | awk '{print $1}')"
log "Self-contained release bundle packaged at: ${DIST_DIR} (${BUNDLE_MB} MB)"
ls -lh "${DIST_DIR}/kural-app" "${DIST_DIR}/audio_test" "${DIST_DIR}/ai_cli" "${DIST_DIR}/kural_cli"
