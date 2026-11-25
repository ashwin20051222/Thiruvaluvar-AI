#!/usr/bin/env bash
# Idempotent Phase 4/6 offline model downloader for Thiruvalluvar AI.
# Verifies non-empty assets, prunes redundant FP32/test payloads, locks/checks SHA256SUMS,
# and enforces the `<= 350 MB` models/ footprint gate.
set -euo pipefail

ROOT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
MODELS_DIR="${MODELS_DIR:-${ROOT_DIR}/models}"
TMP_DIR="$(mktemp -d)"
trap 'rm -rf "${TMP_DIR}"' EXIT

MAX_MODELS_MB=350

log() {
    printf '[models] %s\n' "$*"
}

fetch_file() {
    local url="$1"
    local dest="$2"
    if [[ -s "${dest}" ]]; then
        log "cached: ${dest#"${ROOT_DIR}/"}"
        return 0
    fi
    mkdir -p "$(dirname "${dest}")"
    local part="${dest}.part"
    log "downloading: ${url}"
    curl -fL --retry 3 --retry-delay 2 --connect-timeout 15 -o "${part}" "${url}"
    if [[ ! -s "${part}" ]]; then
        printf 'ERROR: downloaded file is empty: %s\n' "${url}" >&2
        exit 1
    fi
    mv "${part}" "${dest}"
}

extract_tar_bz2_once() {
    local url="$1"
    local dest_dir="$2"
    local prune_fp32="${3:-0}"
    if [[ -n "$(find "${dest_dir}" -maxdepth 3 -name "tokens.txt" -print -quit 2>/dev/null)" ]] \
        && [[ -n "$(find "${dest_dir}" -maxdepth 3 -name "*.onnx" -print -quit 2>/dev/null)" ]]; then
        log "cached dir: ${dest_dir#"${ROOT_DIR}/"}"
        return 0
    fi

    local stage_dir="${TMP_DIR}/extract_$(basename "${dest_dir}")"
    rm -rf "${stage_dir}"
    mkdir -p "${stage_dir}" "${dest_dir}"

    local archive="${TMP_DIR}/$(basename "${url}")"
    log "downloading archive: ${url}"
    curl -fL --retry 3 --retry-delay 2 --connect-timeout 15 -o "${archive}" "${url}"
    tar -xjf "${archive}" -C "${stage_dir}"
    rm -f "${archive}"

    # Prune sample test WAVs not needed at runtime
    find "${stage_dir}" -type d -name "test_wavs" -exec rm -rf {} +

    # Strip redundant FP32 .onnx models whenever an INT8 sibling exists
    if [[ "${prune_fp32}" -eq 1 ]]; then
        while IFS= read -r -d '' int8_file; do
            local fp32_sibling="${int8_file%.int8.onnx}.onnx"
            if [[ -f "${fp32_sibling}" ]]; then
                rm -f "${fp32_sibling}"
            fi
        done < <(find "${stage_dir}" -type f -name "*.int8.onnx" -print0)
    fi

    if [[ -z "$(find "${stage_dir}" -maxdepth 4 -name "tokens.txt" -print -quit)" ]] \
        || [[ -z "$(find "${stage_dir}" -maxdepth 4 -name "*.onnx" -print -quit)" ]]; then
        printf 'ERROR: extracted archive missing tokens.txt or .onnx: %s\n' "${url}" >&2
        exit 1
    fi

    # Atomic replace into destination directory
    rm -rf "${dest_dir}"
    mkdir -p "$(dirname "${dest_dir}")"
    mv "${stage_dir}" "${dest_dir}"
}

log "target directory: ${MODELS_DIR}"
mkdir -p \
    "${MODELS_DIR}/embeddings" \
    "${MODELS_DIR}/vad" \
    "${MODELS_DIR}/stt_en" \
    "${MODELS_DIR}/stt_ta" \
    "${MODELS_DIR}/tts/en" \
    "${MODELS_DIR}/tts/ta"

# 1. Semantic INT8 Embeddings (~64 MB)
EMB_BASE="https://huggingface.co/777Radik/potion-multilingual-128M-int8/resolve/main"
fetch_file "${EMB_BASE}/model.safetensors" "${MODELS_DIR}/embeddings/model.safetensors"
fetch_file "${EMB_BASE}/tokenizer.json" "${MODELS_DIR}/embeddings/tokenizer.json"
fetch_file "${EMB_BASE}/config.json" "${MODELS_DIR}/embeddings/config.json"

# 2. Silero VAD (~644 KB)
fetch_file \
    "https://github.com/k2-fsa/sherpa-onnx/releases/download/asr-models/silero_vad.onnx" \
    "${MODELS_DIR}/vad/silero_vad.onnx"

# 3. Streaming English Zipformer STT (~22 MB after pruning test_wavs & FP32 siblings)
extract_tar_bz2_once \
    "https://github.com/k2-fsa/sherpa-onnx/releases/download/asr-models/sherpa-onnx-streaming-zipformer-en-20M-2023-02-17.tar.bz2" \
    "${MODELS_DIR}/stt_en" \
    1

# 4. Offline Tamil NeMo CTC INT8 STT (~138 MB)
TA_STT_BASE="https://huggingface.co/csndhanasekar/sherpa-onnx-indicconformer-ta-int8/resolve/main"
fetch_file "${TA_STT_BASE}/model.int8.onnx" "${MODELS_DIR}/stt_ta/model.int8.onnx"
fetch_file "${TA_STT_BASE}/tokens.txt" "${MODELS_DIR}/stt_ta/tokens.txt"

# 5. English Piper VITS INT8 TTS (~21 MB)
extract_tar_bz2_once \
    "https://github.com/k2-fsa/sherpa-onnx/releases/download/tts-models/vits-piper-en_US-lessac-low-int8.tar.bz2" \
    "${MODELS_DIR}/tts/en" \
    1

# 6. Tamil Piper VITS TTS (~64 MB)
TA_TTS_BASE="https://huggingface.co/Jeyaram-K/piper-tamil-voices/resolve/main/ta_IN-ValluvarNeural-medium"
fetch_file \
    "${TA_TTS_BASE}/ta_IN-ValluvarNeural-medium.onnx" \
    "${MODELS_DIR}/tts/ta/ta_IN-ValluvarNeural-medium.onnx"
fetch_file \
    "${TA_TTS_BASE}/ta_IN-ValluvarNeural-medium.onnx.json" \
    "${MODELS_DIR}/tts/ta/ta_IN-ValluvarNeural-medium.onnx.json"

# Verify or record SHA256 checksums for tamper/corruption detection on subsequent runs
CHECKSUM_FILE="${MODELS_DIR}/SHA256SUMS"
if [[ -f "${CHECKSUM_FILE}" ]]; then
    log "verifying cached model checksums against ${CHECKSUM_FILE#"${ROOT_DIR}/"}..."
    (cd "${MODELS_DIR}" && sha256sum -c "${CHECKSUM_FILE}" >/dev/null)
else
    log "recording model SHA256 manifest at ${CHECKSUM_FILE#"${ROOT_DIR}/"}..."
    (
        cd "${MODELS_DIR}"
        find . -type f \( -name "*.onnx" -o -name "*.safetensors" -o -name "tokens.txt" -o -name "*.json" \) \
            | sort | xargs sha256sum > "${CHECKSUM_FILE}"
    )
fi

TOTAL_MB="$(du -sm "${MODELS_DIR}" | awk '{print $1}')"
log "total models/ size: ${TOTAL_MB} MB (budget <= ${MAX_MODELS_MB} MB)"
if (( TOTAL_MB > MAX_MODELS_MB )); then
    printf 'ERROR: models/ size (%s MB) exceeds %s MB budget\n' "${TOTAL_MB}" "${MAX_MODELS_MB}" >&2
    exit 1
fi
log "all Phase 4/6 offline models verified."
