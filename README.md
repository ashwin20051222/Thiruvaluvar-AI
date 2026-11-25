# Thiruvalluvar AI (`thiruvalluvar-ai-rs`)

**100% Pure-Rust Offline Bilingual Thirukkural Companion** built for both Linux Desktop (`1280×720`) and Raspberry Pi 4B + 3.5-inch LCD (`480×320`) in Wayland `cage` kiosk mode.

- **Zero Python**, **Zero Node.js / Chromium**, **Zero cloud API calls**, **Zero external web URLs**
- **`#![forbid(unsafe_code)]`** enforced across all 8 workspace crates (with `tikv-jemallocator` on Linux)
- **Complete 1,330 Kurals** with Tamil couplets, transliteration, English translation, 3 classical Tamil commentaries (`MuVa`, `Solomon Paapaiah`, `Parimelazhagar`), kids stories/quizzes, youth quizzes/modern meanings, 45 curated knowledge-base passages, and INT8 semantic embeddings in `data/thirukkural.db`

---

## Workspace Architecture (8 Crates)

| Crate | Responsibility |
| :--- | :--- |
| `crates/kural-core` | Shared domain models, `AppConfig` loader, `BackendCommand` / `UiSnapshot` / `UiTransientEvent` contracts |
| `crates/kural-db` | Read-only SQLite Kural/KB/FTS5 + cosine vector store (`?mode=ro&immutable=1`) and `UserStateStore` (`data/user_state.db`) |
| `crates/kural-enricher` | Offline corpus validation, Tamil morphological search normalization, seed/export utilities |
| `crates/kural-audio` | Dedicated OS-thread `cpal` full-duplex audio actor, 16 kHz linear resampler, Silero VAD (`silero-vad-rs`), earcon mixer, TTS playback queue (`0.75x` / `1.0x` / `1.25x`), and USB/I2S hot-plug watchdog |
| `crates/kural-ai` | `sherpa-rs` English + Tamil STT, Piper VITS INT8 EN/TA TTS, `model2vec-rs` INT8 semantic explainer (`0.53 ms` mean) + OOS bilingual guardrails, and pure-Rust DSP + 5-class vocal-tract age classifier |
| `crates/kural-voice-cmd` | Offline bilingual (`en`/`ta`) touch-free voice command router & slot extractor |
| `crates/kural-backend` | Supervised `BackendController` & `BackendHandle` with `(voice_session_id, nav_epoch)` + `search_epoch` cancellation and unified touch/voice dispatch |
| `crates/kural-app` | 100% pure-Rust `dioxus-desktop` UI wiring all 16 screens (Start, Kids, Youth, Adult, modals, Voice HUD) and self-contained offline `.woff2` + SVG/PNG assets |

---

## Requirements

- **Rust**: `rustc >= 1.85.0` (Edition `2024`)
- **Linux system libraries**:
  - `build-essential`, `cmake`, `pkg-config`
  - `libasound2-dev` (ALSA)
  - `libwebkit2gtk-4.1-dev`, `libgtk-3-dev` (Dioxus WebKitGTK desktop shell)
  - `espeak-ng` (Piper TTS phonemization), `fonts-noto-core`
  - Raspberry Pi kiosk only: `cage`, `alsa-utils`, `libinput-tools`

> **Important Build Path Note (`tikv-jemalloc-sys`)**  
> If your workspace directory path contains spaces (such as `.../Fun Project/Thiruvalluvar Ai/...`), always set a space-free `CARGO_TARGET_DIR` (for example `CARGO_TARGET_DIR=/tmp/kural-build`) so `tikv-jemalloc-sys` `./configure` succeeds cleanly.

---

## Quick Start (Linux Desktop)

```bash
git clone https://github.com/ashwin20051222/Thiruvaluvar-AI.git
cd Thiruvaluvar-AI

# 1. Download offline speech + embedding models (<= 350 MB total)
./scripts/download_fast_models.sh

# 2. Check workspace build & tests
export CARGO_TARGET_DIR=/tmp/kural-build
cargo clippy --workspace --all-targets -- -D warnings
cargo test --workspace

# 3. Run in Desktop mode (1280x720)
cargo run --release --bin kural-app -- --config config.toml

# 4. Run exact 3.5" Raspberry Pi LCD preview (480x320) on your laptop
cargo run --release --bin kural-app -- --config config.toml --pi-preview
```

---

## CLI Utilities

- **GUI App (`kural-app`)**:
  ```text
  kural-app [--config PATH] [--pi-preview | --kiosk] [--verify-layout]
  ```
- **Database & Vector CLI (`kural_cli`, feature `cli` in `kural-db`)**:
  ```bash
  cargo run --release -p kural-db --features cli --bin kural_cli -- --db data/thirukkural.db stats
  cargo run --release -p kural-db --features cli --bin kural_cli -- --db data/thirukkural.db get 1
  cargo run --release -p kural-db --features cli --bin kural_cli -- --db data/thirukkural.db search "friendship"
  ```
- **Embedding Indexer (`index_embeddings`, feature `cli` in `kural-db`)**:
  ```bash
  cargo run --release -p kural-db --features cli --bin index_embeddings -- --db data/thirukkural.db --models-dir models/embeddings
  ```
- **Audio Hardware Diagnostic (`audio_test`, feature `cli` in `kural-audio`)**:
  ```bash
  cargo run --release -p kural-audio --features cli --bin audio_test -- --list
  cargo run --release -p kural-audio --features cli --bin audio_test -- --config config.toml
  ```
- **Speech & AI Diagnostic (`ai_cli`, feature `cli` in `kural-ai`)**:
  ```bash
  cargo run --release -p kural-ai --features cli --bin ai_cli -- benchmark --config config.toml
  ```

---

## Raspberry Pi 4B Kiosk Deployment

Full ARM64 setup, 3.5-inch `480×320` DRM/KMS + `libinput` touch calibration, ALSA mic/speaker tuning, and `systemd` + `cage` deployment instructions are documented in [`docs/PI_DEPLOYMENT_GUIDE.md`](docs/PI_DEPLOYMENT_GUIDE.md).

