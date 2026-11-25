# Thiruvalluvar AI — Approved Backend Master Architecture (`backend_master_approved.md`)

- **Status:** **APPROVED (Phase 1 EGM Readiness Gate Passed — 0 Unresolved Critical/High Findings)**
- **Project:** Bilingual Offline AI-Based Thirukkural Search, Age-Adaptive Interface & Voice Interaction System
- **Workspace Base:** `thiruvalluvar-ai-rs/` (Edition `2024`, Cargo `resolver = "3"`, Rust MSRV `1.85+`)
- **Target Hardware & Viewports:**
  - **Primary Kiosk:** Raspberry Pi 4 Model B (`aarch64-unknown-linux-gnu`, 8 GB RAM, 32 GB SD) + 3.5-Inch Landscape LCD (`480×320`) + External Mic + Dual Amplified Stereo Speakers
  - **Development / Desktop:** Laptop / Desktop (`x86_64-unknown-linux-gnu`, `1280×720` / `1366×768`, `--pi-preview` `480×320`)
- **Non-Negotiable Mandates:** 100% Pure Rust (`0` `.py` files) • 100% Offline (`0` cloud calls, `0` API keys, `0` external URLs/CDNs) • Strict Literary Integrity (per `../Synopsis.pdf`: verbatim Tamil couplets, `1,330` Kurals, `133` Adhikarams, `Aram 1–380` / `Porul 381–1080` / `Inbam 1081–1330`, strict domain boundary) • Dual Modality (Touch + Bilingual Voice)

---

## 1. System Goals, Constraints, Budgets & Architectural Trade-Offs

### 1.1 Goals, Non-Goals & Anti-Goals
- **Goals:**
  1. Deliver an offline, instant-response bilingual (`ta` / `en`) Thirukkural kiosk on Raspberry Pi 4B (`480×320`) and Laptop (`1280×720`) with zero-scroll layout (`scrollHeight <= clientHeight && scrollWidth <= clientWidth` across all `4 AppScreen × 2 Language × 2 Theme = 16` combinations) and `>= 38 px` touch ergonomics on the 3.5" LCD.
  2. Retrieve all `1,330` Kurals, `133` Adhikarams, `3` Paals, and `9` Iyals verbatim from SQLite with zero generative alteration (`Synopsis.pdf`).
  3. Adapt explanations, visuals, gamification, and TTS cadence across `Kids` (`6–12`), `Youth` (`13–17`), and `Adult` (`18+`) via `< 30 ms` vocal-tract Voice Age Detection or manual card selection.
  4. Protect Raspberry Pi 4B SD-card longevity by keeping `data/thirukkural.db` read-only (`journal_mode = DELETE`, `?immutable=1`, `mmap`), isolating mutable state in WAL `data/user_state.db` (`synchronous = FULL`, debounced `last_kural_*` writes), disabling WebKitGTK persistent disk caches, and routing runtime caches to RAM / `/tmp`.
- **Non-Goals:**
  - Open-ended general-domain chat (math, coding, weather, politics): rejected deterministically at the 4-point semantic confidence gate with a polite bilingual limitation message.
  - Multi-user accounts, authentication, or cloud synchronization.
  - Web browsers (`Chromium`/`Electron`) or Python (`PyO3`, `python3` subprocesses).
- **Anti-Goals:**
  - Local quantized GGUF LLMs (`llama-cpp-2`), `whisper-small`, or `wav2vec2` that breach `700 MB` RAM or introduce `2–12 s` stalls on Pi 4B.
  - Linking `ort` alongside `sherpa-onnx` (causes static `libonnxruntime` symbol collisions and duplicate ORT arenas).
  - Self-triggering audio loops from dual amplified stereo speakers, or mutating `#kural-couplet` / `#kural-initial-seal` into English text when `Language::English` is active.

### 1.2 Chosen Pure-Rust Edge Stack
| Subsystem | Rejected Alternative & Why | Chosen Pure-Rust Edge Architecture |
|---|---|---|
| **3-Level Q&A & Persona** | Local `llama-cpp-2` GGUF (`1.5–2.2 GB` RAM, `3–8 s` latency on Pi 4B, hallucination risk) | Deterministic anchored router + `model2vec-rs` (`local-only` / `memmap2` INT8 fallback) + SQLite FTS5 cosine-gated RRF + pure-Rust 3-level persona composer over pre-verified DB columns (`< 15 ms`, zero generative synthesis). |
| **Voice Age Classification** | `wav2vec2` (`~380 MB`, `400+ ms`) or pitch-only threshold (confuses Kids with Adult Women) | Auditable `14`-feature pure-Rust DSP over top-`40%` vowel-nucleus frames (`rustfft` pYIN $F_0$ + multi-order Burg LPC $F_1$–$F_3$ + Fitch Vocal-Tract Length $L_{\text{vt}} = c / (2\Delta F)$ + `MFCC 1..=3` / centroid / tilt / HNR) + Logistic Softmax (`< 30 ms`, `< 2 MB`). |
| **Voice Activity Detection** | Standalone `ort` crate (static `libonnxruntime` collision with `sherpa-onnx`) | `kural_audio::VadBackend` trait with built-in `EnergyZcrVad` fallback + `SherpaSileroVad` implemented in `kural-ai` via `sherpa-onnx` native `VoiceActivityDetector` (`SileroVadModelConfig`). |
| **Speech Recognition (STT)** | `whisper.cpp` (`1.2–3.5 s` blocking on Pi 4B) | `sherpa-onnx` streaming Zipformer-EN-20M (`< 100 ms`) + `indicconformer-ta-ctc-int8` (`150–300 ms`), managed under `pi_compact` drop-before-load lifecycle. |
| **Text-to-Speech (TTS)** | Disk Opus cache (SD wear + missing Opus crate) or unchunked generation | Sentence-chunked `sherpa-onnx` Piper VITS (`< 80 ms` first audio) + byte-bounded `i16` RAM LRU replay cache (`<= 16 MB`, `<= 5 ms` replay) + `espeak-ng` RIFF PCM fallback (`< 10 ms`). |
| **Desktop / Kiosk UI** | Chromium / Electron (`350–600 MB` overhead) | Single-binary `dioxus-desktop` GUI with ephemeral `WebsiteDataManager`, `tikv-jemallocator` (Linux), `AppConfig` runtime, rendered on Pi 4B via `cage` Wayland kiosk. |

### 1.3 Pi 4B Latency, Disk & Runtime Memory Budgets

#### Latency Targets (Raspberry Pi 4B Cortex-A72 CPU)
| Operation | Target Ceiling | Hard Test Gate |
|---|---:|---:|
| Bilingual Voice Command Router (`kural-voice-cmd`) | `< 1 ms` | `<= 2 ms` |
| `explain_kural` (verbatim SQLite + persona format) | `< 1 ms` | `<= 5 ms` |
| Semantic 3-Level Q&A (`ask_question` p99) | `< 15 ms` | `<= 20 ms` |
| INT8 Vector + FTS5 RRF (`1,330` Kurals + `45` Knowledge Base entries) | `< 2 ms` | `<= 5 ms` |
| `StopAudio` Hardware Sink Abort (`sink.pause(); sink.clear();`) | `< 20 ms` | `<= 25 ms` |
| Voice Age Detection (Stage 1 + `14` features + Classifier, excl. `3.2 s` capture) | `< 30 ms` | `<= 50 ms` |
| Streaming TTS First Audio (`sherpa-onnx` Piper) / Cached RAM Replay | `< 80 ms` / `< 5 ms` | `<= 120 ms` / `<= 5 ms` |
| English Streaming STT (`zipformer-en-20M`) | `< 100 ms` | `<= 150 ms` |
| Tamil Offline CTC-INT8 STT (`indicconformer-ta`) | `150–300 ms` | `<= 350 ms` |

#### Offline Model Disk Footprint (`models/`, Budget `< 350 MB`)
| Subdirectory | Model Artifact | Expected Size | Role |
|---|---|---:|---|
| `models/embeddings/` | `potion-multilingual-128M-int8` (`tokenizer.json`, `model.safetensors`, `config.json`) | `~64 MB` | Semantic `256`-dim INT8 vectors (index + query) |
| `models/vad/` | `silero_vad.onnx` | `~2 MB` | Neural VAD via `sherpa-onnx` (`16 kHz`, `512`-sample window) |
| `models/stt_en/` | `sherpa-onnx-streaming-zipformer-en-20M` (INT8 ONNX + `tokens.txt`) | `~22 MB` | Streaming English ASR |
| `models/stt_ta/` | `indicconformer-ta-ctc-int8.onnx` + `tokens.txt` | `~145 MB` | Offline Tamil NeMo CTC ASR |
| `models/tts/` | `en_US-lessac-low.onnx` (`~18 MB`) + `ta_IN-ValluvaNeural-medium.onnx` (`~65 MB`) + `espeak-ng-data` (`~14 MB`, shared/symlinked or bundled) | `~97 MB` | Bilingual Piper VITS TTS + phonemizer data |
| **Total Models** | **All offline model assets** | **`~330 MB`** | **`20 MB` headroom below `350 MB` limit** (asserted in `download_fast_models.sh`) |

#### Runtime RAM Budget (`< 700 MB` RSS on Pi 4B)
All Pi 4B and Desktop runtime builds standardize on a single active-language-resident strategy (**drop inactive heavy models before loading the newly selected language**, with `tikv-jemallocator` returning freed pages immediately to the OS and `onnx_intra_threads = 1`, spin-waiting disabled):
- **Mandatory Phase 2 RSS Gate on `model2vec-rs`:** Measure `Model2VecEmbedder::load` RSS delta on `potion-multilingual-128M-int8`. If `model2vec-rs` expands weights from `i8` into `f32` (`> 96 MB` RSS delta), Phase 2 switches the runtime query encoder in `crates/kural-db/src/embedder.rs` to a pure-Rust `memmap2` + `safetensors` + `tokenizers` **INT8 row-lookup mean-pooler** (`~25 MB` active page cache), keeping total RSS well inside budget.

| Process Tree Component (Pi 4B ARM64) | Active Tamil Peak RSS | Active English RSS |
|---|---:|---:|
| `kural-app` + WebKitGTK (`WebsiteDataManager::set_is_ephemeral(true)`, `XDG_CACHE_HOME=/tmp`) | `~195 MB` | `~195 MB` |
| `thirukkural.db` `mmap` + `user_state.db` + In-Memory INT8 Matrix (`~357.5 KB`) | `~44 MB` | `~44 MB` |
| Query Embedder (`memmap2` INT8 lookup `~25 MB`, max ceiling `<= 72 MB`) | `~25–72 MB` | `~25–72 MB` |
| Audio Ring Buffers + `Silero VAD` (`~2 MB`) + `i16` PCM LRU Cache (`<= 16 MB`) | `~26 MB` | `~26 MB` |
| Active STT Engine (`stt_ta` `~185 MB` OR `stt_en` `~35 MB`) | `~185 MB` | `~35 MB` |
| Active TTS Engine (`ta_IN` `~80 MB` OR `en_US` `~28 MB`) | `~80 MB` | `~28 MB` |
| Age AI Workspace + Tokio Runtime (`2` workers) | `~12 MB` | `~12 MB` |
| **Estimated Worst-Case Process Tree RSS** | **`~567–614 MB`** | **`~365–412 MB`** |

> **Systemd & RSS Verification:** `deploy/thiruvalluvar-ai.service` sets `MemoryHigh=700M` and `MemoryMax=850M`. Phase 6 verification asserts `< 700 MiB` (`memory.current` / `VmRSS`) across 5 consecutive `Tamil <-> English` switches.

---

## 2. Multi-Crate Workspace Topology, Dependency Pins & Acyclic DAG

### 2.1 Directory Tree (`thiruvalluvar-ai-rs/`)
```text
thiruvalluvar-ai-rs/
├── Cargo.toml                         # Virtual manifest root (Edition 2024, resolver = "3", [workspace.lints])
├── .rustfmt.toml
├── config.toml                        # Hardware ALSA devices, mic calibration, display mode, thresholds
├── data/
│   ├── thirukkural.db                 # Read-only compiled DB (journal_mode=DELETE, 1,330 Kurals + FTS5 + INT8)
│   └── user_state.db                  # Mutable WAL DB (synchronous=FULL): prefs, Kids/Youth progress, bookmarks
├── models/                            # <= 350 MB offline edge models
│   ├── embeddings/
│   ├── vad/
│   ├── stt_en/
│   ├── stt_ta/
│   └── tts/
├── docs/designs/
│   ├── db_design_approved.md
│   ├── frontend_audit.md
│   ├── backend_master_draft.md
│   └── backend_master_approved.md
├── seed_data/
├── stitch_extracted/
├── scripts/
│   ├── download_fast_models.sh        # Idempotent + SHA256 verified + du -sm <= 350 check
│   ├── setup_pi.sh                    # ARM64 packages (incl. espeak-ng), /etc/asound.conf dsnoop/dmix, LCD
│   └── build_pi_release.sh            # Cortex-A72 release build (SHERPA_ONNX_LIB_DIR offline support)
├── deploy/
│   ├── launch_kiosk.sh                # Wayland `cage`, GST_AUDIO_SINK=fakesink, XDG_CACHE_HOME=/tmp/kural-cache
│   └── thiruvalluvar-ai.service       # Systemd unit (MemoryHigh=700M, MemoryMax=850M, Restart=on-failure)
└── crates/
    ├── kural-core/                    # L0: Pure domain types, NFC/text, validators, AppConfig, BackendCommand
    ├── kural-db/                      # L1: Read-only KuralDb + INT8 vectors/RRF + UserStateStore (0 runtime HTTP)
    ├── kural-enricher/                # L1: 100% offline deterministic Layer 2 synthesizer
    ├── kural-audio/                   # L1: Single-owner ALSA actor (cpal + rodio + rubato + VadBackend + Echo Guard)
    ├── kural-voice-cmd/               # L1: Deterministic bilingual voice command router (< 1 ms, sync/pure)
    ├── kural-ai/                      # L2: Age AI (< 30 ms) + Explainer (< 15 ms) + SherpaSileroVad + STT/TTS + ai_cli
    ├── kural-backend/                 # L3: Unified BackendController + BackendHandle (watch + mpsc) + AiWorker
    └── kural-app/                     # L4: 100% Pure-Rust Dioxus Desktop GUI (0 external URLs, tikv-jemallocator)
```

### 2.2 Acyclic Crate Dependency DAG (Compile-Time Enforced)
```text
L0:  kural-core  (no tokio, no rusqlite, no HTTP, no audio, no UI, no FFI)
        ▲
        ├───────────────┬──────────────────┬───────────────────┐
        │               │                  │                   │
L1:  kural-db      kural-enricher     kural-audio       kural-voice-cmd
        ▲                                  ▲                   │
        │                                  │                   │
        └───────────────┬──────────────────┘                   │
                        ▼                                      │
L2:                  kural-ai  (depends on: kural-core, kural-db, kural-audio)
                        ▲                                      │
                        ├──────────────────┬───────────────────┘
                        │  (plus kural-db, kural-audio directly)
                        ▼
L3:                 kural-backend  (deps: kural-core, kural-db, kural-audio, kural-ai, kural-voice-cmd)
                        ▲
                        │
L4:                 kural-app  (depends ONLY on: kural-core, kural-backend)
```

### 2.3 Locked Workspace `Cargo.toml` Hygiene & Version Pins
- **Root `Cargo.toml` updates (executed in Phase 2+):**
  - `resolver = "3"`, `edition = "2024"`, `rust-version = "1.85"`.
  - `[workspace.lints.rust]`: `unsafe_code = "forbid"` globally; ALL 8 member crates add `[lints] workspace = true`. (Because `sherpa-onnx` 2.0 provides safe RAII Rust wrappers and `dot_i8` uses safe iterators, **all 8 workspace crates compile cleanly with `unsafe_code = "forbid"`**).
  - `[workspace.dependencies]` pinned targets & minimal features:
    ```toml
    anyhow = "1"
    chrono = { version = "0.4", features = ["serde"] }
    clap = { version = "4", features = ["derive"] }
    cpal = "0.18"
    dioxus = { version = "0.7", features = ["desktop"] }
    memmap2 = "0.9"
    model2vec-rs = { version = "0.5", default-features = false, features = ["onig", "local-only"] }
    reqwest = { version = "0.12", default-features = false, features = ["json", "rustls-tls"] }
    ringbuf = "0.6"
    rodio = "0.21"
    rubato = "1.0"
    rusqlite = { version = "0.32", features = ["bundled", "modern_sqlite"] }
    rustfft = "6.4"
    safetensors = "0.4"
    serde = { version = "1", features = ["derive"] }
    serde_json = "1"
    sherpa-onnx = "2.0"
    strsim = "0.11"
    tempfile = "3"
    thiserror = "2"
    tikv-jemallocator = "0.6"
    tokenizers = { version = "0.21", default-features = false, features = ["onig"] }
    tokio = { version = "1", features = ["sync"] }
    tokio-util = "0.7"
    toml = "0.8"
    unicode-normalization = "0.1"
    unicode-segmentation = "1"
    ```
  - **Crate Dependency Rules:**
    - `kural-db/Cargo.toml`: `fetcher = ["dep:reqwest", "dep:tokio"]` (`[[bin]] name = "fetch_canonical" required-features = ["fetcher"]`); `cli = ["dep:clap", "dep:anyhow"]` (`kural_cli`, `seed_db`, `index_embeddings` use `required-features = ["cli"]`); `tokio` gets `rt-multi-thread`, `macros`, `time` only via `fetcher` / `kural-backend`. Verify `cargo tree -p kural-db --no-default-features` contains **zero** `reqwest`, `hf-hub`, `ureq`, `clap`, or `ort`.
    - `kural-audio/Cargo.toml`: pulls only `kural-core`, `cpal`, `rodio`, `ringbuf`, `rubato`, `tokio` (`sync` feature only). **Zero `ort`.**
    - `kural-voice-cmd/Cargo.toml`: pulls only `kural-core` and `strsim`.
    - `kural-app/Cargo.toml`: pulls only `kural-core`, `kural-backend`, `dioxus`, `tokio-util` (if needed), and `tikv-jemallocator` (`[target.'cfg(target_os = "linux")'.dependencies]`). Never pulls `rusqlite`, `cpal`, `rodio`, `sherpa-onnx`, `ort`, `reqwest`, `kural-db`, `kural-audio`, `kural-ai`, or `kural-voice-cmd`.

---

## 3. Configuration File & `AppConfig` Contract (`config.toml`)

`thiruvalluvar-ai-rs/config.toml` holds deployment-tunable hardware, calibration, and confidence values cleanly separated from DSP code constants.

### 3.1 Path, Override & Seed Precedence
1. **Resolution Order:** `--config <PATH>` CLI arg → `./config.toml` → `Config::default()`. Malformed TOML syntax or out-of-range values exit startup with code `1` and an explicit error diagnostic.
2. **Relative Path Resolution:** Relative paths inside `[paths]` resolve relative to the loaded `config.toml` parent directory (or `CARGO_MANIFEST_DIR` / `current_exe()` parent if running defaults without a file), guaranteeing launch safety from any working directory.
3. **`user_state.db` & CLI Overrides:** `[audio] master_volume` seeds `preferences.volume` ONLY on initial `user_state.db` creation; thereafter `user_state.db` governs volume. CLI flags (`--pi-preview`, `--kiosk`) unconditionally override `[ui] viewport_mode`.

### 3.2 `config.toml` Schema & Defaults
```toml
[paths]
kural_db = "data/thirukkural.db"
user_state_db = "data/user_state.db"
models_dir = "models"

[audio]
input_device = "default"           # ALSA capture card substring or "default"
output_device = "default"          # ALSA stereo playback card substring or "default"
master_volume = 0.85               # Initial seed volume [0.0, 1.0]
earcon_guard_ms = 220              # Post-sink.empty() wait (DMA drain + speaker tail)
echo_cooldown_ms = 250             # Max mic mute cooldown after TTS finishes/aborts
listening_timeout_ms = 5000        # Max Listening wait before clean Timeout return
max_command_duration_ms = 7000     # Max CapturingSpeech utterance ceiling
age_sample_duration_ms = 3200      # Fixed capture window for StartVoiceAgeDetect
vad_threshold = 0.50               # Silero VAD speech probability cutoff
aec_enabled = false                # Optional 32-tap NLMS chime/echo reducer + energy gate
mic_to_listen_guard_ms = 400       # Debounces accidental Mic->Listen kid tap collisions

[age_ai]
min_voiced_ms = 1200
min_snr_db = 8.0
max_clipping_ratio = 0.02
min_rms_level = 0.015              # Lowered softly to 0.005 when snr_db >= 12.0 dB
low_confidence_threshold = 0.70
f0_offset_hz = 0.0                 # Per-mic pitch calibration offset
vtl_offset_cm = 0.0                # Per-mic vocal-tract length calibration offset

[ai]
onnx_intra_threads = 1             # Single ONNX intra thread avoids Pi audio contention
onnx_allow_spinning = false
min_vec_candidate_cosine = 0.25    # Pre-RRF floor (prevents noise vectors scoring RRF)
oos_min_vector_cosine = 0.36
oos_min_rrf_score = 0.022
kb_min_vector_cosine = 0.42
kb_min_rrf_score = 0.025
tts_cache_max_entries = 16
tts_cache_max_bytes = 16777216     # Strict 16 MiB RAM cap for i16 PCM replay LRU

[ui]
viewport_mode = "desktop"          # "desktop" (1280x720) | "pi_preview" (480x320) | "kiosk"
toast_duration_ms = 1800
```

### 3.3 `AppConfig` Rust Definition (`crates/kural-core/src/config.rs`)
```rust
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct AppConfig {
    pub paths: PathsConfig,
    pub audio: AudioConfig,
    pub age_ai: AgeAiConfig,
    pub ai: AiConfig,
    pub ui: UiConfig,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct PathsConfig {
    pub kural_db: PathBuf,
    pub user_state_db: PathBuf,
    pub models_dir: PathBuf,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct AudioConfig {
    pub input_device: String,
    pub output_device: String,
    pub master_volume: f32,
    pub earcon_guard_ms: u32,
    pub echo_cooldown_ms: u32,
    pub listening_timeout_ms: u32,
    pub max_command_duration_ms: u32,
    pub age_sample_duration_ms: u32,
    pub vad_threshold: f32,
    pub aec_enabled: bool,
    pub mic_to_listen_guard_ms: u64,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct AgeAiConfig {
    pub min_voiced_ms: u32,
    pub min_snr_db: f32,
    pub max_clipping_ratio: f32,
    pub min_rms_level: f32,
    pub low_confidence_threshold: f32,
    pub f0_offset_hz: f32,
    pub vtl_offset_cm: f32,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct AiConfig {
    pub onnx_intra_threads: i32,
    pub onnx_allow_spinning: bool,
    pub min_vec_candidate_cosine: f32,
    pub oos_min_vector_cosine: f32,
    pub oos_min_rrf_score: f32,
    pub kb_min_vector_cosine: f32,
    pub kb_min_rrf_score: f32,
    pub tts_cache_max_entries: usize,
    pub tts_cache_max_bytes: usize,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct UiConfig {
    pub viewport_mode: String,
    pub toast_duration_ms: u64,
}
```
- `AppConfig::load(cli_config: Option<&Path>) -> Result<Self, CoreError>` loads, validates bounds (`master_volume in 0.0..=1.0`, `vad_threshold in 0.0..=1.0`, `onnx_intra_threads >= 1`), and resolves relative `[paths]`.

---

## 4. Locked Shared Types, Commands & Bilingual Copy Contracts (`crates/kural-core`)

`kural-core` retains all existing canonical types (`AgeGroup`, `Language`, `ResearchTheme`, `Division`, `Iyal`, `Adhikaram`, `KuralCore`, `KuralChild`, `KuralYouth`, `KuralAdult`, `Tag`, `FullKural`, `KuralSummary`, `SearchHit`, `SearchMatchKind`, `SearchOutcome`, `KnowledgeEntry`, `DbStats`). Existing `Language` (`models.rs`) gains `as_str(&self) -> &'static str` and `fmt::Display` returning `"tamil"` / `"english"`.

### 4.1 New Domain Types & Helpers (`crates/kural-core/src/contract.rs`)
```rust
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Theme { Dark, Light }

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum AppScreen { Start, Kids, Youth, Adult }

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SearchTab { Query, Divisions }

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ModalKind {
    Search { initial_tab: SearchTab },
    AiChat { seed_kural: Option<u16> },
    KidsStory,
    KidsWordGame,
    YouthQuiz,
    YouthDeepDive,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Scholar { MuVa, Solomon, Kalaignar, Parimel, English }

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SpeakTarget {
    KuralOnly,
    KuralAndMeaning,
    KuttyStory,
    YouthBriefing,
    ScholarCommentary(Scholar),
    AiAnswer,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum CompactTabHint { Verse, Analysis, Quiz, Commentary, Study }

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum AudioState { Idle, Listening, CapturingSpeech, Processing, PlayingTts, Cooldown }

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum CaptureMode {
    FixedAgeSample { duration_ms: u32 },
    VadCommand { max_duration_ms: u32 },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum RetryReason { TooQuiet, TooShort, TooNoisy, Clipped, HardwareUnavailable }

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct AgeDetection {
    pub age: AgeGroup,
    pub confidence: f32,       // [0.0, 1.0]
    pub low_confidence: bool,
    pub f0_hz: f32,
    pub formant_dispersion_hz: f32,
    pub vtl_cm: f32,
    pub latency_ms: u32,       // DSP + classifier time only (excludes capture window)
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub enum AgeOutcome {
    Detected(AgeDetection),
    Retry(RetryReason),
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum ActiveFilter {
    Division(u8),
    Tag(String),
    ResearchTheme(ResearchTheme),
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct CorpusCatalog {
    pub divisions: Vec<Division>,
    pub iyals: Vec<Iyal>,
    pub adhikarams: Vec<Adhikaram>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct GroundedAnswer {
    pub answer_ta: String,
    pub answer_en: String,
    pub cited_kurals: Vec<FullKural>,
    pub kb_hits: Vec<KnowledgeEntry>,
    pub out_of_scope: bool,
    pub latency_ms: u32,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct SystemHealth {
    pub db_ok: bool,
    pub user_state_ok: bool,
    pub audio_ok: bool,
    pub embeddings_loaded: bool,
    pub vad_loaded: bool,
    pub active_stt_loaded: bool,
    pub active_tts_loaded: bool,
}
```
- **Exact Trait Implementations:** `Theme`, `Scholar`, `AppScreen`, and `SearchTab` implement `as_str(&self) -> &'static str`, `fmt::Display`, and `FromStr` matching SQLite `CHECK` literals (`"dark"/"light"`, `"mu_va"/"solomon"/"kalaignar"/"parimel"/"english"`, etc.).
- **Speech Speed Helpers in `kural-core`:**
  - `pub const SPEECH_SPEED_PRESETS: [f32; 3] = [0.75, 1.0, 1.25];`
  - `pub fn snap_speech_speed(raw: f32) -> f32` (returns closest preset).
  - `pub fn step_speech_speed(current: f32, delta: i8) -> f32` (moves index `-1`/`+1` clamped to `0..=2`).
  - `pub fn age_cadence_factor(screen: AppScreen) -> f32` (`Kids => 0.95`, `Youth => 1.02`, `Adult | Start => 1.00`).
  - `pub fn effective_tts_speed(user_speed: f32, screen: AppScreen) -> f32` (`(snap_speech_speed(user_speed) * age_cadence_factor(screen)).clamp(0.65, 1.40)`).

### 4.2 Persistent User State Types (`BTreeSet<u16>` Normalized Parity)
```rust
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct UserPrefs {
    pub lang: Language,
    pub theme: Theme,
    pub speech_speed: f32,        // Strictly in {0.75, 1.0, 1.25}
    pub volume: f32,              // Clamped [0.0, 1.0]
    pub muted: bool,
    pub default_age: AgeGroup,
    pub last_kural_kids: u16,
    pub last_kural_youth: u16,
    pub last_kural_adult: u16,
}

#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize)]
pub struct KidsProgress {
    pub stars_earned: u32,
    pub completed_stories: BTreeSet<u16>,
    pub completed_games: BTreeSet<u16>,
}

#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize)]
pub struct YouthProgress {
    pub daily_streak: u32,
    pub last_active_date: String, // YYYY-MM-DD local date
    pub quiz_xp: u32,
    pub answered_quizzes: BTreeSet<u16>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct BookmarkEntry {
    pub kural_number: u16,
    pub preferred_scholar: Scholar,
    pub adhikaram_ta: String,     // Hydrated at load/toggle so S4 Bookmarks subcard needs no DB call
    pub adhikaram_en: String,
    pub line1_ta: String,
    pub created_at: String,
}
```

### 4.3 Unified `BackendCommand` Enum (1:1 Frontend + Voice Parity)
```rust
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[non_exhaustive]
pub enum BackendCommand {
    // Appearance & Navigation
    SetLanguage(Language),
    SetTheme(Theme),
    Navigate(AppScreen),
    SelectAge(AgeGroup),
    OpenModal(ModalKind),
    CloseModal,

    // Voice Lifecycle
    StartVoiceAgeDetect,
    StartVoiceCommand,
    CancelVoice,

    // Search, Navigation & Filters
    SubmitTextSearch { query: String, division_id: Option<u8> },
    OpenKural(u16),
    NextKural,
    PrevKural,
    OpenAdhikaram(u8),
    FilterByDivision(Option<u8>),
    FilterByTag(Option<String>),
    FilterByResearchTheme(Option<ResearchTheme>),
    ClearFilter,

    // Scholar & Speech Playback
    SelectCommentary(Scholar),
    SpeakCurrent(SpeakTarget),
    ReplayAudio,
    StopAudio,
    SetSpeechSpeed(f32),
    SetMuted(bool),
    AdjustVolume(f32),            // Signed delta applied to prefs.volume, clamped [0.0, 1.0]

    // Interactive Gamification & Bookmarks
    SubmitKidsGameAnswer { kural_number: u16, choice_idx: u8 },
    CompleteKidsStory(u16),       // Fired by Story audio finish or explicit "Done" button
    SubmitYouthQuizAnswer { kural_number: u16, choice_idx: u8 },
    ToggleBookmark(u16),

    // Grounded Offline AI Q&A
    AskAiQuestion { query: String, seed_kural: Option<u16> },
}
```

### 4.4 Dual-Channel UI Hydration (`watch<Arc<UiSnapshot>>` + `mpsc<UiTransientEvent>`)
Replacing a single `broadcast::Sender` eliminates `RecvError::Lagged` UI desyncs cleanly:

1. **Snapshot Channel (`tokio::sync::watch::Receiver<Arc<UiSnapshot>>`):** Always reflects latest coherent state with zero dropped updates:
   ```rust
   #[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
   pub struct UiSnapshot {
       pub prefs: UserPrefs,
       pub screen: AppScreen,
       pub modal: Option<ModalKind>,
       pub current_kural: FullKural,
       pub requested_scholar: Scholar,
       pub effective_scholar: Scholar, // Auto-resolved (#395/#648 Parimel -> MuVa)
       pub active_filter: Option<ActiveFilter>,
       pub pool_len: usize,
       pub pool_idx: usize,
       pub kids_progress: KidsProgress,
       pub youth_progress: YouthProgress,
       pub bookmarks: Vec<BookmarkEntry>,
       pub audio_state: AudioState,
       pub has_replay_clip: bool,
       pub system_health: SystemHealth,
   }
   ```
2. **Transient Event Channel (`tokio::sync::mpsc::Receiver<UiTransientEvent>`, bounded `64`):** Delivers one-shot results, evaluations, and notifications:
   ```rust
   #[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
   #[non_exhaustive]
   pub enum UiTransientEvent {
       SearchResultsReady(SearchOutcome),
       AdhikaramPreviewReady { adhikaram_id: u8, kurals: Vec<KuralSummary> },
       AgeResult(AgeOutcome),
       VoiceCommandRecognized {
           transcript: String,
           toast_ta: String,
           toast_en: String,
           compact_tab: Option<CompactTabHint>,
       },
       AiAnswerReady(GroundedAnswer),
       KidsGameEvaluated { kural_number: u16, correct: bool, correct_idx: u8, stars_earned: u32 },
       YouthQuizEvaluated { kural_number: u16, correct: bool, correct_idx: u8, daily_streak: u32, quiz_xp: u32 },
       CommentaryFallbackNotice { requested: Scholar, effective: Scholar, notice_ta: String, notice_en: String },
       InfoToast { message_ta: String, message_en: String },
       ErrorToast { message_ta: String, message_en: String },
   }
   ```
3. **Live RMS Stream (`tokio::sync::watch::Receiver<f32>`):** Separate `30 Hz` microphone amplitude stream `[0.0, 1.0]` for `.eq-bar` animations.

> *Audit Alignment Note (`frontend_audit.md`):* `#pi-mode-strip` view tabs (`Verse`, `Analysis`, `Commentary`, `Study`) live in local component state inside `kural-app` (synchronized optionally when voice emits `CompactTabHint`), while selecting `சவால் / Quiz` dispatches `OpenModal(ModalKind::YouthQuiz)`.

### 4.5 Critical Behavioral & `SpeakTarget` Text Mapping Contracts

1. **Startup State & `SelectAge` Persistence:**
   - Application always boots onto `AppScreen::Start` with `requested_scholar = Scholar::MuVa`, `modal = None`, `active_filter = None`, and loads `KuralDb::get_kural_full(prefs.last_kural_for(prefs.default_age))` validated against `1..=1330`.
   - `SelectAge(age)` sets `screen = AppScreen::from(age)`, persists `default_age = age`, clears incompatible filters (`None`), loads the age pool (`kids_featured = 1 in 1..=1080` on `Kids`; `1..=1330` on `Youth | Adult`), and opens `prefs.last_kural_for(age)` (falling back to `pool[0]` if absent).
   - `OpenAdhikaram(id)` resolves `adhikaram.kural_start`, clears active filters, opens `kural_start`, closes `Search` modal if open, and updates `last_kural_*` (debounced).
   - Clearing a filter (`ClearFilter` / `FilterBy*(None)`) restores the ordered age pool; keeps `pool_idx` if `current_kural` exists in the restored pool, otherwise loads `pool[0]`.
2. **Kids Word Game & Youth Quiz Bounds Validation:**
   - `SubmitKidsGameAnswer { kural_number, choice_idx }` & `SubmitYouthQuizAnswer { kural_number, choice_idx }`: require `choice_idx < 4` and `(1..=1330).contains(&kural_number)`. Invalid inputs emit `ErrorToast` with zero panics. On `Kids`, `correct_idx = child.game_options_ta.iter().position(|w| w == &child.game_blank_word_ta)` matches identical index `s` across `game_options_ta[s]` and `game_options_en[s]`.
3. **`meaning_parimel` NULL Fallback (`#395` and `#648`):**
   - `requested_scholar` preserves user preference across Kurals. Whenever `requested_scholar == Scholar::Parimel` and `current_kural.kural.meaning_parimel.is_none()` (upon `SelectCommentary`, `NextKural`, `PrevKural`, `OpenKural`, `OpenAdhikaram`, or `SpeakCurrent(ScholarCommentary(Parimel))`), `effective_scholar` automatically resolves to `Scholar::MuVa` and emits `UiTransientEvent::CommentaryFallbackNotice`. Stepping away to a Kural with `meaning_parimel.is_some()` restores `effective_scholar = Scholar::Parimel`.
4. **`SpeakTarget` Text Mapping Table (`tts_kural_ta` / `tts_kural_en` Safe):**
   - When `Language::English` is active, **never pass Tamil script to `en_US-lessac-low`**, using `kural.tts_kural_en` / `translation_en`:

   | `SpeakTarget` | `Language::Tamil` Spoken Text | `Language::English` Spoken Text |
   |---|---|---|
   | `KuralOnly` | `[kural.line1_ta, kural.line2_ta]` (`220 ms` gap between lines) | `kural.tts_kural_en` (`translation_en`) |
   | `KuralAndMeaning` | `[line1_ta, line2_ta]` + `child.simple_meaning_ta` *(Kids)* / `kural.meaning_mu_va` | `kural.tts_kural_en` + `child.simple_meaning_en` *(Kids)* / `kural.explanation_en` |
   | `KuttyStory` | `child.story_title_ta` + `child.kutty_story_ta` + `child.moral_ta` | `child.story_title_en` + `child.kutty_story_en` + `child.moral_en` |
   | `YouthBriefing` | `[line1_ta, line2_ta]` + `youth.analysis_ta` + `youth.action_tip_ta` | `kural.tts_kural_en` + `youth.analysis_en` + `youth.action_tip_en` |
   | `ScholarCommentary(s)` | Active `effective_scholar` Tamil text (`meaning_parimel` falls back to `meaning_mu_va`); `English` uses `meaning_mu_va` in `ta` mode | `kural.translation_en` + `kural.explanation_en` |
   | `AiAnswer` | `last_ai_answer.answer_ta` (if `Some`) | `last_ai_answer.answer_en` (if `Some`) |

5. **Verbatim Tamil Display Invariant:** `#kural-couplet` ALWAYS renders `kural.line1_ta` + `kural.line2_ta`, `#kural-initial-seal` ALWAYS renders `kural.opening_letter_ta`, and `chat_modal.rs` ALWAYS renders verbatim Tamil couplet badges for `GroundedAnswer.cited_kurals` regardless of `prefs.lang`.

### 4.6 Locked Bilingual Copy Contracts
| Scenario | Tamil (`ta`) | English (`en`) |
|---|---|---|
| **Out-of-Scope Refusal (`AiChat`)** | `மன்னிக்கவும், திருவள்ளுவர் AI திருக்குறள், அதன் உரைகள் மற்றும் திருவள்ளுவர் வரலாறு தொடர்பான கேள்விகளுக்கு மட்டுமே பதிலளிக்கும்.` | `Thiruvalluvar AI only answers questions about Thirukkural couplets, commentaries, and Thiruvalluvar's biography.` |
| **Concise Hands-Free OOS TTS (`<= 12` words)** | `திருக்குறள் தொடர்பான கேள்விகளுக்கு மட்டுமே என்னால் உதவ முடியும்.` | `I can only help with questions about the Thirukkural.` |
| **Parimel `#395`/`#648` Fallback** | `இந்தக் குறளுக்குப் பரிமேலழகர் உரை இல்லை; மு.வ. உரை காட்டப்படுகிறது.` | `Parimelazhagar commentary is unavailable for this Kural; showing Mu. Va. instead.` |
| **`RetryReason::TooShort`** | `குரல் பதிவு மிகக் குறுகியது; குறைந்தது 1.2 வினாடிகள் பேசவும்.` | `Speech sample too short; please speak for at least 1.2 seconds.` |
| **`RetryReason::TooQuiet`** | `குரல் மிக மெதுவாக உள்ளது; மைக்கிற்கு அருகில் பேசவும்.` | `Audio level too low; please speak closer to the microphone.` |
| **`RetryReason::TooNoisy`** | `பின்னணி இரைச்சல் அதிகம்; சற்று அமைதியான சூழலில் மீண்டும் முயலவும்.` | `Background noise is too high; please retry or choose an age card.` |
| **`RetryReason::Clipped`** | `ஒலி அளவு அதிகமாகிவிட்டது; மைக்கிலிருந்து சற்று தள்ளிப் பேசவும்.` | `Audio clipped; please step slightly back from the microphone.` |
| **`RetryReason::HardwareUnavailable`** | `ஒலிவாங்கி கிடைக்கவில்லை; கீழே உள்ள வயது அட்டையைத் தேர்ந்தெடுக்கவும்.` | `Microphone unavailable; please select an age card below.` |
| **Voice Number Out-of-Range** | `குறள் எண் 1 முதல் 1330 வரை மட்டுமே இருக்க வேண்டும்.` | `Kural number must be between 1 and 1330.` |
| **Voice Command Toast** | `கேட்டது: {transcript}` | `Heard: {transcript}` |

---

## 5. Offline Storage & Semantic Indexing (`crates/kural-db` — Phase 2)

### 5.1 Read-Only Corpus & INT8 Embeddings (`data/thirukkural.db`)
Add two vector tables to `crates/kural-db/src/schema.rs` (built with `PRAGMA journal_mode = DELETE; VACUUM;` at end of `seed_db` / `index_embeddings` and opened read-only via `?immutable=1` + `PRAGMA query_only = ON`):

```sql
CREATE TABLE IF NOT EXISTS kural_embeddings (
    kural_number INTEGER PRIMARY KEY REFERENCES kurals(number) ON DELETE CASCADE,
    vec_i8       BLOB NOT NULL,
    norm         REAL NOT NULL
);

CREATE TABLE IF NOT EXISTS knowledge_embeddings (
    kb_id        INTEGER PRIMARY KEY REFERENCES knowledge_base(id) ON DELETE CASCADE,
    vec_i8       BLOB NOT NULL,
    norm         REAL NOT NULL
);
```

- **`crates/kural-db/src/embedder.rs` & `src/bin/index_embeddings.rs`:**
  - Concatenates `"{adhikaram_ta} {adhikaram_en} {meaning_mu_va} {translation_en} {explanation_en} {tags}"` for Kurals (`1,330` rows) and `"{question_ta} {question_en} {answer_ta} {answer_en} {keywords}"` for Knowledge Base (`45` rows).
  - Loads `models/embeddings/` locally (`model2vec-rs` `default-features = false, features = ["onig", "local-only"]`, switching to `memmap2` + `safetensors` + `tokenizers` INT8 row-lookup if RSS exceeds `96 MB` in Phase 2 benchmarking).
  - **No `hash_fallback` in production binaries:** Only compiled under `#[cfg(any(test, feature = "test-fallback"))]` so production systems never index synthetic hashes; if `models/embeddings/` is missing at runtime, `KuralDb::open` sets `embeddings_loaded = false` cleanly and continues via FTS5 + `strsim`.
  - Quantization helper:
    ```rust
    pub fn quantize_l2_i8(vec: &[f32]) -> (Vec<i8>, f32) // scale = max_abs / 127.0; norm = sqrt(sum((v as f32)^2))
    pub fn cosine_i8(q: &[i8], q_norm: f32, r: &[i8], r_norm: f32) -> f32 // i32 dot product; 0.0 if either norm == 0.0
    ```
- **Exact `KuralDb` Methods Added in Phase 2:**
  - `pub fn embeddings_loaded(&self) -> bool`
  - `pub fn get_catalog(&self) -> Result<CorpusCatalog, DbError>`
  - `pub fn get_kurals_by_division(&self, division_id: u8) -> Result<Vec<KuralSummary>, DbError>`
  - `pub fn get_pool_numbers_for_filter(&self, filter: Option<&ActiveFilter>, kids_only: bool) -> Result<Vec<u16>, DbError>` (`kids_only == true` queries `kids_featured = 1 AND number <= 1080`)
  - `pub fn search_hybrid_rrf(&self, query: &str, division_id: Option<u8>, min_vec_cosine: f32, rrf_k: f32, limit: usize) -> Result<HybridRrfOutcome, DbError>`
    - Top `50` FTS5 BM25 + top `50` vector hits filtered strictly by `cosine >= min_vec_cosine` *before* assigning 1-based rank (`absent = 0.0`): `score_rrf = 1/(k + rank_fts) + 1/(k + rank_vec)`. Returns `HybridRrfOutcome { pub outcome: SearchOutcome, pub max_vec_cosine: f32, pub max_rrf_score: f32 }`.
  - `pub fn search_knowledge_rrf(&self, query: &str, min_vec_cosine: f32, rrf_k: f32, limit: usize) -> Result<Vec<KnowledgeRrfResult>, DbError>` where `KnowledgeRrfResult { pub entry: KnowledgeEntry, pub vec_cosine: f32, pub rrf_score: f32, pub has_fts_match: bool }`.

### 5.2 Mutable Normalized User State Store (`data/user_state.db`)
Implemented in `crates/kural-db/src/user_state.rs` using `create_dir_all`, WAL (`PRAGMA synchronous = FULL; PRAGMA busy_timeout = 3000;`), normalized set tables (no JSON array blobs), integer speed percentage, and singleton `INSERT OR IGNORE`:

```sql
CREATE TABLE IF NOT EXISTS preferences (
    id                 INTEGER PRIMARY KEY CHECK (id = 1),
    lang               TEXT NOT NULL CHECK (lang IN ('tamil', 'english')) DEFAULT 'tamil',
    theme              TEXT NOT NULL CHECK (theme IN ('dark', 'light')) DEFAULT 'dark',
    speech_speed_pct   INTEGER NOT NULL CHECK (speech_speed_pct IN (75, 100, 125)) DEFAULT 100,
    volume             REAL NOT NULL CHECK (volume >= 0.0 AND volume <= 1.0) DEFAULT 0.85,
    muted              INTEGER NOT NULL CHECK (muted IN (0, 1)) DEFAULT 0,
    default_age        TEXT NOT NULL CHECK (default_age IN ('kids', 'youth', 'adult')) DEFAULT 'youth',
    last_kural_kids    INTEGER NOT NULL CHECK (last_kural_kids BETWEEN 1 AND 1330) DEFAULT 1,
    last_kural_youth   INTEGER NOT NULL CHECK (last_kural_youth BETWEEN 1 AND 1330) DEFAULT 481,
    last_kural_adult   INTEGER NOT NULL CHECK (last_kural_adult BETWEEN 1 AND 1330) DEFAULT 423
);

CREATE TABLE IF NOT EXISTS kids_progress (
    id           INTEGER PRIMARY KEY CHECK (id = 1),
    stars_earned INTEGER NOT NULL CHECK (stars_earned >= 0) DEFAULT 0
);

CREATE TABLE IF NOT EXISTS kids_completed_stories (
    kural_number INTEGER PRIMARY KEY CHECK (kural_number BETWEEN 1 AND 1330)
);

CREATE TABLE IF NOT EXISTS kids_completed_games (
    kural_number INTEGER PRIMARY KEY CHECK (kural_number BETWEEN 1 AND 1330)
);

CREATE TABLE IF NOT EXISTS youth_progress (
    id               INTEGER PRIMARY KEY CHECK (id = 1),
    daily_streak     INTEGER NOT NULL CHECK (daily_streak >= 0) DEFAULT 0,
    last_active_date TEXT NOT NULL DEFAULT '',
    quiz_xp          INTEGER NOT NULL CHECK (quiz_xp >= 0) DEFAULT 0
);

CREATE TABLE IF NOT EXISTS youth_answered_quizzes (
    kural_number  INTEGER PRIMARY KEY CHECK (kural_number BETWEEN 1 AND 1330),
    first_correct INTEGER NOT NULL CHECK (first_correct IN (0, 1))
);

CREATE TABLE IF NOT EXISTS adult_bookmarks (
    kural_number      INTEGER PRIMARY KEY CHECK (kural_number BETWEEN 1 AND 1330),
    preferred_scholar TEXT NOT NULL CHECK (preferred_scholar IN ('mu_va', 'solomon', 'kalaignar', 'parimel', 'english')),
    created_at        TEXT NOT NULL
);
```

- **Public Rust API (`UserStateStore`):**
  - `open(path: &Path, seed_volume: f32) -> Result<Self, DbError>` (ensures `INSERT OR IGNORE (id = 1)` rows exist).
  - `load_snapshot_state(&self, corpus: &KuralDb) -> Result<(UserPrefs, KidsProgress, YouthProgress, Vec<BookmarkEntry>), DbError>`
  - `save_prefs(&self, prefs: &UserPrefs) -> Result<(), DbError>`
  - `record_kids_story_completed(&self, kural_number: u16) -> Result<KidsProgress, DbError>`
  - `record_kids_game_attempt(&self, kural_number: u16, correct: bool) -> Result<KidsProgress, DbError>` (awards `+1` star atomically on first correct solution)
  - `record_youth_quiz_attempt(&self, kural_number: u16, correct: bool, today_ymd: &str, yesterday_ymd: &str) -> Result<YouthProgress, DbError>` (`if last_active_date >= today_ymd { keep streak }` guards offline RTC clock drift; `+20 XP` first correct / `+5 XP` participation)
  - `toggle_bookmark(&self, kural_number: u16, scholar: Scholar, now_rfc3339: &str, corpus: &KuralDb) -> Result<Vec<BookmarkEntry>, DbError>`

---

## 6. Single-Owner Echo-Safe Hardware Audio Engine (`crates/kural-audio` — Phase 3)

### 6.1 Actor Contracts & Types
One dedicated OS thread (`"kural-audio-actor"`) owns the persistent `cpal` input stream and `rodio` `OutputStream`/`Sink`.

```rust
pub trait VadBackend: Send {
    fn reset(&mut self);
    /// Evaluates a single 512-sample (32 ms) 16 kHz mono chunk and returns speech prob [0.0, 1.0].
    fn speech_probability(&mut self, frame_512_16k: &[f32]) -> Result<f32, AudioError>;
}

#[derive(Debug, Clone)]
pub struct PcmChunk {
    pub samples: Vec<i16>,
    pub sample_rate_hz: u32,
    pub is_final: bool,
}

#[derive(Debug, Clone)]
pub struct CapturedAudio {
    pub samples_16k: Vec<f32>,
    pub speech_only_16k: Vec<f32>,
    pub pre_roll_noise_16k: Vec<f32>, // Fed to Stage 1 spectral noise-floor subtraction
    pub voiced_ms: u32,
    pub snr_db: f32,
    pub clipping_ratio: f32,
    pub rms_level: f32,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum EarconKind { ListenStart, CmdRecognized, RetryTone }

#[derive(Debug, thiserror::Error)]
pub enum AudioError {
    #[error("audio hardware unavailable: {0}")] DeviceUnavailable(String),
    #[error("capture already active")] CaptureInProgress,
    #[error("listening timed out with no speech")] Timeout,
    #[error("capture cancelled")] Cancelled,
    #[error("audio stream error: {0}")] StreamError(String),
}
```

```rust
impl AudioHandle {
    pub fn spawn(config: &AudioConfig, vad: Option<Box<dyn VadBackend>>, max_cache_bytes: usize) -> Self;
    pub async fn start_capture(&self, mode: CaptureMode) -> Result<CapturedAudio, AudioError>;
    pub async fn cancel_capture(&self) -> Result<(), AudioError>;
    pub async fn play_pcm_stream(&self, pcm_rx: mpsc::Receiver<PcmChunk>, cache_key: Option<String>) -> Result<(), AudioError>;
    pub async fn play_pcm_i16(&self, samples: Vec<i16>, sample_rate_hz: u32, cache_key: Option<String>) -> Result<(), AudioError>;
    pub async fn play_earcon(&self, kind: EarconKind) -> Result<(), AudioError>;
    pub async fn replay_last(&self) -> Result<(), AudioError>;
    pub async fn stop_playback(&self) -> Result<(), AudioError>; // Calls sink.pause(); sink.clear(); (< 20 ms)
    pub async fn set_volume(&self, volume: f32) -> Result<(), AudioError>;
    pub async fn set_muted(&self, muted: bool) -> Result<(), AudioError>;
    pub fn subscribe_rms(&self) -> watch::Receiver<f32>;
    pub fn subscribe_state(&self) -> watch::Receiver<AudioState>;
    pub fn is_healthy(&self) -> bool;
}
```

### 6.2 Conditioning, Resampling, Filters & Watchdogs
1. **Persistent Capture & Watchdog:** `cpal` writes lock-free into a `ringbuf` SPSC (`1.5 s` capacity) and stamps `last_callback_ms: AtomicU64`. If no callback fires for `> 500 ms` on an open stream, `AudioActor` drops and reopens ALSA with bounded backoff (`500 ms, 1s, 2s`) before updating `audio_ok = false`.
2. **Resampling & Continuous IIR Filtering:** `rubato::SincFixedIn` resamples hardware input (`48/44.1/16 kHz`) cleanly to `16 kHz mono f32`. Every frame runs continuously (even when output is discarded during `Idle`/`Cooldown` so biquad state stays settled without step-response spikes) through:
   - Mean DC-offset removal,
   - **4th-order `80 Hz` Butterworth HPF** (`24 dB/oct` = 2 cascaded biquads), and
   - **`50 Hz` + `60 Hz` biquad notches (`Q = 8.0`)** guaranteeing **`>= 18 dB` mains hum attenuation** at `50/60 Hz`.
3. **VAD & Timeouts:** `AudioActor` maintains `200 ms` circular pre-roll, checks injected `VadBackend` (`SherpaSileroVad` or built-in `EnergyZcrVad`: RMS `> 2.5x` calibrated noise floor + ZCR `0.02..=0.25`) in `512`-sample (`32 ms`) windows, caps `Listening` waiting at `listening_timeout_ms = 5000` (returning `AudioError::Timeout`), and auto-stops `CapturingSpeech` after `850 ms` trailing silence or `max_command_duration_ms`.

### 6.3 Strict Half-Duplex Echo Guard, Earcons & Byte-Bounded `i16` Cache
- **Earcon Sequence:** `StartVoice*` plays `ListenStart` (`90 ms`, `523.25 -> 659.25 Hz`), awaits `sink.empty()`, waits hardware-tail `earcon_guard_ms = 220 ms`, resets `VadBackend` + pre-roll, and enters `AudioState::Listening`. (If `config.audio.aec_enabled = true`, runs optional `32`-tap NLMS chime reducer so `Listening` pre-roll can start at `t = 0`).
- **`CmdRecognized`:** Gentle two-tone interval chime (`659.25 Hz -> 783.99 Hz`, `80 ms`), dispatched by `BackendController` upon valid voice command recognition; `RetryTone` (`330 Hz -> 261.63 Hz`, `140 ms`) on retry/timeout.
- **Speaker Echo Lock:** During `PlayingTts` and `Cooldown` (`echo_cooldown_ms = 250 ms` after `sink.empty()` or `stop_playback()`), ring-buffer samples are discarded and `rms_rx` emits `0.0`.
- **Byte-Bounded `i16` PCM Cache:** Audio commits `(cache_key, Vec<i16>, sample_rate_hz)` into the LRU cache (bounded by BOTH `tts_cache_max_entries = 16` AND `tts_cache_max_bytes = 16_777_216`) ONLY when `play_pcm_stream` completes normally without `StopAudio` abort. Replay feeds `rodio::buffer::SamplesBuffer` in `<= 5 ms`. `stop_playback()` while `Idle` is a no-op; while `Listening`/`CapturingSpeech` behaves cleanly as `cancel_capture()` -> `AudioState::Idle`.

---

## 7. Zero-Latency Edge AI & Voice Command Router (`crates/kural-ai`, `crates/kural-voice-cmd` — Phase 4)

### 7.1 Instant Vocal-Tract Voice Age AI (`crates/kural-ai/src/age.rs` — `< 30 ms`)
1. **Stage 1 — Pre-Roll Noise Subtraction & SNR-Coupled Speech Gate (`< 3 ms`):**
   - Computes baseline ambient noise RMS/spectrum from `pre_roll_noise_16k` for spectral floor subtraction.
   - Quality gates:
     - `voiced_ms < config.min_voiced_ms (1200)` → `RetryReason::TooShort`
     - `clipping_ratio >= config.max_clipping_ratio (0.02)` → `RetryReason::Clipped`
     - `snr_db < config.min_snr_db (8.0)` → `RetryReason::TooNoisy`
     - `rms_level < 0.005 || (rms_level < config.min_rms_level && snr_db < 12.0)` → `RetryReason::TooQuiet` (permits soft off-axis children when room is quiet!).
2. **Stage 2 — Auditable `14`-Feature Vowel-Nucleus Extractor (`< 18 ms`):**
   - Selects the **top `40%` steady-state vowel-nucleus frames** (ranked by `frame_rms * frame_hnr / (1.0 + spectral_flux)`) to suppress nasal anti-resonances and consonant bursts.
   - **Pitch (`4` features):** `rustfft` + pYIN (`25 ms` frames, `10 ms` hop, `70..=500 Hz`) extracting `f0_median`, `f0_iqr`, `f0_p10`, `f0_p90` (+ `config.f0_offset_hz`).
   - **Multi-Order Burg LPC & Vocal-Tract Length (`4` features):** Downsamples vowel nuclei to `10 kHz` (`0.97` pre-emphasis, Hamming window), computes Burg LPC (`order = 10` for $F_0 > 240\text{ Hz}$, consensus across `order ∈ {10, 12, 14}` otherwise), extracts conjugate poles (`250..=4600 Hz`, adaptive `bw < 450 Hz` below `2.2 kHz` / `< 600 Hz` above `2.2 kHz`; frames with `< 3` poles skipped; if `< 5` valid frames remain, falls back to $F_0$ + MFCC weights). Computes medians `(F1, F2, F3)`, Fitch regression formant dispersion $\Delta F$, and physical Vocal-Tract Length:
     $$L_{\text{vt}} = \frac{35000}{2\,\Delta F} + \text{vtl\_offset\_cm} \quad (c = 35{,}000\text{ cm/s})$$
     - **Kids (6–12):** $L_{\text{vt}} \in [10.5, 13.0]\text{ cm}$ ($\Delta F \approx 1345\text{–}1665\text{ Hz}$) — cleanly separated from **Adult Female (18+):** $L_{\text{vt}} \approx 14.0\text{–}15.8\text{ cm}$ ($\Delta F \approx 1105\text{–}1250\text{ Hz}$) and **Adult Male:** $L_{\text{vt}} \approx 16.5\text{–}18.5\text{ cm}$ ($\Delta F \approx 945\text{–}1060\text{ Hz}$); **Youth (13–17):** $L_{\text{vt}} \in [13.1, 14.9]\text{ cm}$.
   - **Spectral / Cepstral Backup (`6` features):** `MFCC 1..=3` means, `spectral_centroid_hz`, `spectral_tilt_db`, `hnr_db`.
3. **Stage 3 — Calibrated Softmax Scorer (`< 1 ms`):**
   - `pub struct AgeClassifier { config: AgeAiConfig }` exposing `classify(&self, audio: &CapturedAudio) -> AgeOutcome` using auditable `const` linear-logistic weights + temperature-scaled softmax.

### 7.2 Hardened Low-Latency Semantic Explainer & 4-Point OOS Guardrail (`crates/kural-ai/src/explainer.rs` — `< 15 ms`)
```rust
pub struct SemanticExplainer { /* holds KuralDb + AiConfig */ }
impl SemanticExplainer {
    pub fn explain_kural(&self, number: u16, age: AgeGroup, lang: Language, scholar: Scholar) -> Result<GroundedAnswer, AiError>;
    pub fn ask_question(&self, query: &str, age: AgeGroup, lang: Language, seed_kural: Option<u16>) -> Result<GroundedAnswer, AiError>;
}
```
- **Hardened 4-Point OOS Guardrail in `ask_question`:**
  1. **Anchored Structural/Number Router:** Matches Kural numbers ONLY when anchored (`kural <N>`, `couplet <N>`, `#<N>`, `குறள் <N>`, or entire trimmed string is `1..=1330`), preventing `"What is 42 * 10?"` or `"Article 370"` from hijacking Kural lookup.
  2. **Anaphoric `seed_kural` Gate:** Considers `seed_kural` ONLY if the normalized query contains an explicit explanation/story/meaning intent (`explain this`, `meaning`, `story`, `moral`, `இந்தக் குறள்`, `விளக்கு`, `பொருள்`, `கதை`, `கருத்து`). Otherwise `seed_kural` does NOT bypass OOS!
  3. **Cosine-Filtered RRF:** `KuralDb::search_hybrid_rrf` excludes vector candidates below `min_vec_candidate_cosine = 0.25` before rank assignment.
  4. **Strict Dual Confidence Threshold:** Unless fast-path matched, refuses with `out_of_scope: true`, `cited_kurals: vec![]`, `kb_hits: vec![]`, and `§4.6` bilingual copy whenever **both** `(kural_hits.max_vec_cosine < oos_min_vector_cosine && kural_hits.max_rrf_score < oos_min_rrf_score)` **AND** no KB hit satisfies `(vec_cosine >= kb_min_vector_cosine || rrf_score >= kb_min_rrf_score)`.
- **3-Level Composer Field Integrity:** Always uses verbatim `KuralCore` (`line1_ta`, `line2_ta`, `transliteration1`, `transliteration2`, `translation_en`, `meaning_*` with `#395`/`#648` fallback) and age-tier DB columns.

### 7.3 Offline Speech Engine & `SherpaSileroVad` (`crates/kural-ai/src/{vad,stt,tts}.rs`)
- **`SherpaSileroVad`:** Wraps `sherpa_onnx::VoiceActivityDetector` (`SileroVadModelConfig`, `intra_op_num_threads = 1`) implementing `kural_audio::VadBackend`.
- **`SpeechEngine`:**
  - `new(models_dir: &Path, initial_lang: Language, ai_cfg: &AiConfig) -> Result<Self, AiError>`
  - `ensure_language(&mut self, lang: Language) -> Result<(), AiError>` (enforces **drop-before-load** under single-active-language mode; `onnx_intra_threads = 1`, spin-waiting disabled; keeps non-blocking via dedicated `AiWorker`).
  - `transcribe(&mut self, samples_16k: &[f32], lang: Language) -> Result<String, AiError>` (`zipformer-en-20M` / `indicconformer-ta-ctc-int8`).
  - `synthesize_streaming(&mut self, text_chunks: &[&str], lang: Language, speed: f32, cancel: &CancellationToken, pcm_tx: mpsc::Sender<PcmChunk>) -> Result<(), AiError>`:
    - Splits Tamil couplets into `[line1_ta, line2_ta]`, inserting a **`220 ms` zero-PCM silent `PcmChunk`** between lines (Piper VITS ignores textual pause tags).
    - Checks `cancel.is_cancelled()` and `pcm_tx.send().is_err()` before every sentence so `StopAudio` halts synthesis immediately.
    - Fallback chain: `sherpa-onnx` Piper VITS (`< 80 ms` first chunk) → local `espeak-ng` subprocess (`-v ta` / `-v en --stdout`), parsing RIFF `"data"` subchunk header dynamically (`AiError::FallbackTtsUnavailable` if binary absent).

### 7.4 Deterministic Voice Command Router (`crates/kural-voice-cmd` — `< 1 ms`)
```rust
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct RouterContext {
    pub screen: AppScreen,
    pub modal: Option<ModalKind>,
    pub active_kural: u16,
    pub active_scholar: Scholar,
    pub speech_speed: f32,
    pub muted: bool,
}

#[derive(Debug, Clone, PartialEq)]
pub struct VoiceRouteOutcome {
    pub cmd: BackendCommand,
    pub compact_tab: Option<CompactTabHint>,
    pub toast_ta: String,
    pub toast_en: String,
}

pub fn route_transcript(raw: &str, lang: Language, ctx: RouterContext) -> VoiceRouteOutcome
```
- Uses `strsim::normalized_levenshtein >= 0.80` on NFC-normalized lowercase tokens (`>= 4` chars), plus Tamil/English cardinal parser (`1..=1330`).
- Emits concrete qualified variants only (`OpenModal(ModalKind::Search { initial_tab: SearchTab::Query })`, `SetSpeechSpeed(step_speech_speed(ctx.speech_speed, -1|+1))`, `SetMuted(!ctx.muted)`, `ClearFilter`, `ToggleBookmark(ctx.active_kural)`, plus `"paals"`/`"divisions"`/`"பால்"` → `OpenModal(ModalKind::Search { initial_tab: SearchTab::Divisions })`, `"adult"`/`"adults"`, `"dark"`/`"light"`, and `"1..=4"` / `"A..=D"` quiz/game choice commands when `KidsWordGame` or `YouthQuiz` modal is open). Out-of-range spoken numbers route cleanly to `ErrorToast` via controller/helper.
- `screen_default_target(screen, active_scholar)` maps: `Start => KuralOnly`, `Kids => KuralAndMeaning`, `Youth => YouthBriefing`, `Adult => ScholarCommentary(active_scholar)`.

---

## 8. Unified Controller & 100% Pure-Rust Dioxus UI (`crates/kural-backend`, `crates/kural-app` — Phase 5)

### 8.1 `BackendHandle` & Non-Blocking Actor Architecture (`crates/kural-backend`)
```rust
#[derive(Clone)]
pub struct BackendHandle { /* owns Arc<tokio::runtime::Runtime>, cmd_tx, snapshot_rx, rms_rx, catalog */ }

impl BackendHandle {
    /// Synchronously boots runtime, DBs, AudioActor, AgeClassifier, SemanticExplainer, AiWorker, and initial UiSnapshot.
    pub fn start(config: AppConfig) -> Result<Self, BackendError>;
    pub fn dispatch(&self, cmd: BackendCommand);
    pub fn current_snapshot(&self) -> Arc<UiSnapshot>;
    pub fn subscribe_snapshot(&self) -> watch::Receiver<Arc<UiSnapshot>>;
    pub fn subscribe_events(&self) -> mpsc::Receiver<UiTransientEvent>;
    pub fn subscribe_rms(&self) -> watch::Receiver<f32>;
    pub fn catalog(&self) -> &CorpusCatalog; // Zero DB import in kural-app
    pub fn shutdown(self);
}
```
- **Deadlock-Free Concurrency & TOCTOU Protection:**
  - `BackendHandle::start` creates a dedicated `tokio::runtime::Runtime` (2 workers + blocking pool) so `kural-app::main` stays simple and non-blocking alongside `dioxus-desktop`.
  - `BackendController` never awaits blocking SQLite/DSP/ONNX calls on the command loop: DB queries & `AgeClassifier` & `SemanticExplainer` use `tokio::task::spawn_blocking`; `SpeechEngine` (`stt`/`tts`/`ensure_language`) runs on a single dedicated `"kural-ai-worker"` OS thread via bounded channel + `CancellationToken`.
  - Voice captures run in supervised spawned tasks posting `InternalMsg::VoiceCompleted { session_id, nav_epoch, ... }` back into `cmd_rx`. Any touch action (`Navigate`, `SelectAge`, `OpenKural`, `NextKural`, `PrevKural`, `SetLanguage`, `CancelVoice`, `StopAudio`) cancels active capture/TTS tokens and increments `nav_epoch` so stale voice completions are discarded cleanly.
  - **Child `Mic -> Listen` Guard (`400 ms`):** `SpeakCurrent` arriving within `mic_to_listen_guard_ms = 400` of entering `AudioState::Listening` is ignored; thereafter cancels capture cleanly and starts TTS.
  - **Debounced `last_kural_*` Flush:** `OpenKural`/`NextKural`/`PrevKural` flush `last_kural_*` to `user_state.db` on a `5 s` trailing debounce timer (plus immediate flush on `Navigate`/`SelectAge`/`shutdown`).
  - **Voice Fallback Guards:** Empty/noise STT (`< 2` valid graphemes) returns `Idle` with brief retry toast; unmatched voice Q&A speaks the concise `<= 12`-word OOS message only if `out_of_scope == false` (or speaks the short OOS refusal when triggered cleanly inside `AiChat`).

### 8.2 Pure-Rust Dioxus UI & `480×320` Zero-Scroll Architecture (`crates/kural-app`)
- **Runtime Hygiene:** Uses `#[global_allocator]` = `tikv_jemallocator::Jemalloc` on Linux (`main.rs`), configures WebKitGTK `WebsiteDataManager::set_is_ephemeral(true)`, and ships zero `https?://` asset references (`grep -rnE "https?://" crates/kural-app/src/ crates/kural-app/assets/ | grep -v "http://www.w3.org/2000/svg"` returns `0`).
- **Critical `480×320` Pi Kiosk Viewport Fix & Layout (`40 px + 204 px + 76 px = 320 px`):**
  1. `crates/kural-app/assets/app.css` overrides Stitch's `1024px` defect cleanly: `.pi-root, #app-shell { height: 100dvh !important; max-height: 100dvh !important; width: 100vw !important; overflow: hidden !important; }`.
  2. **Reclaimed Stage (`>= 38 px` touch targets, `>= 14 px` Tamil prose):**
     - **Header (`40 px`):** Back (`←`), compact Age badge, `தமிழ்/EN`, Theme toggle, `#2ECC8F` passive offline indicator.
     - **Main Stage (`204 px`):** On `S3`/`S4`, replaces the cramped `26 px` 5-tab strip with a **`38 px` 3-tab Segmented Bar** (`S3`: `குறள்` | `பொருள்` | `சவால்/Quiz`; `S4`: `குறள்` | `உரை` | `ஆய்வு`) + an inline **`38 px` Scholar Cycle Pill** (`மு.வ. ↻` cycling all 5 scholars incl. `Parimel` & `English` with `#395`/`#648` fallback notice) and `◀ 1/N ▶` sentence-boundary pagination (`166 px` readable card, zero scroll).
     - **Dock (`76 px`):** `>= 44 px` touch tiles + central `#pi-voice-orb` (`56 px`), including Kids 5-slot dock (`Story`, `Game`, `Orb`, `Audio`, `Search`), Youth/Adult 5-slot dock (`Paals`, `Deep/Scholars`, `Orb`, `Search`, `AI/Discuss`), and state-morphing `Listen <-> Stop` primary audio button.
  3. **Modals on `480×320` (`max-height: 304px`):** `KidsWordGame` and `YouthQuiz` lock strictly to a **`2×2` grid of `220×42 px` option buttons** (`<= 242 px` content height); `AiChat` leads with `48×48 px` Voice Orb + `3` prompt chips (`38 px`) + paginated `14 px` grounded answers & clickable `#N` couplet badges; `Search` uses single-level drill-down (`3–4` `40 px` rows/page + `◀/▶`).
  4. **Desktop (`1280×720`) Audit Fixes:** Fixes `S1` `home_ta_dark` footer parity, populates `S3` Analytics + Tag Chips + Daily Quiz + unhidden `.pi-streak-pill` + `Stop`/`Mute`/`#pi-speed-chip` `[0.75x, 1.0x, 1.25x]`, and populates `S4` right pane with Daily Reflection + `4` `.royal-subcard` Research Themes + Bookmarks + `5th` `Parimel` tab (`#395`/`#648` tooltip guard).
  5. **CLI Launch Flags:** Default resizable Laptop (`1280×720`), `--pi-preview` (`480×320`), `--kiosk` (borderless fullscreen for `cage`).

---

## 9. Phase Verification Gates (Phases 2–6)

| Phase | Required Verification & Acceptance Commands |
|---|---|
| **Phase 2** | `cargo run --release --features cli --bin kural_cli -- stats` prints `1330/1330`, `integrity_check: ok`; `index_embeddings` populates `1,330` + `45` vectors; RSS check confirms `embedder` load `<= 96 MB` delta (`memmap2` INT8 fallback if needed); `UserStateStore` normalized SQLite + `?immutable=1` tests pass; `cargo tree -p kural-db` shows zero `reqwest`/`ureq`/`hf-hub`; `cargo test --workspace` & `cargo clippy --workspace --all-targets -- -D warnings` pass. |
| **Phase 3** | `audio_test` runs cleanly; unit tests prove: Echo Guard discards samples during `PlayingTts` + `Cooldown`, `ListenStart` enforces `220 ms` post-empty guard, `listening_timeout_ms` (`5000 ms`) returns `AudioError::Timeout`, `rubato` `48k -> 16k` tone check passes, 4th-order `80 Hz` HPF + `50/60 Hz` notch achieves `>= 18 dB` rejection, `stop_playback()` aborts `< 20 ms`, `i16` LRU respects `16 MB` ceiling, zero `ort` in workspace. |
| **Phase 4** | `ai_cli` benchmarks pass (`Age < 30 ms` / `<= 50 ms` gate, `Explainer < 15 ms` / `<= 20 ms` p99, `VoiceCmd < 1 ms` / `<= 2 ms`, `STT <= 300 ms`); unit tests prove Kid (`~11.5 cm`) vs Adult-Female (`~14.8 cm`) $L_{\text{vt}} = c/(2\Delta F)$ separation, verbatim cited Kurals, 4-point OOS rejection (`"42 * 10"`, unrelated seeded query, rank-100 noise vector), Tamil/English fuzzy commands (`RouterContext`). |
| **Phase 5** | `cargo run --release -p kural-app` (`1280×720`) & `-- --pi-preview` (`480×320`) achieve `scrollHeight <= clientHeight && scrollWidth <= clientWidth` across all `16` screen/lang/theme combinations with `>= 14 px` Tamil text and `>= 38 px` Pi touch targets; `grep -rnE "https?://" crates/kural-app/src/ crates/kural-app/assets/ | grep -v "http://www.w3.org/2000/svg"` returns `0`. |
| **Phase 6** | `scripts/download_fast_models.sh` (`du -sm models <= 350`), `scripts/setup_pi.sh`, `scripts/build_pi_release.sh`, `deploy/launch_kiosk.sh`, `deploy/thiruvalluvar-ai.service` (`MemoryHigh=700M`, `MemoryMax=850M`); `VmRSS < 700 MB` verified across `5` language switches; `find . -name "*.py"` = `0`; `cargo clippy --workspace --all-targets -- -D warnings` & `cargo test --workspace` green; `README.md` & `docs/PI_DEPLOYMENT_GUIDE.md` complete. |

---

## Appendix A — EGM Multi-Agent Review & Resolution Log

| Severity | Source Agent(s) | Summary of Finding | Resolution in Approved Spec |
|---|---|---|---|
| **Critical** | `egm-implementation-readiness` | `sherpa-onnx` + `ort` static `libonnxruntime` symbol linker collision | Eliminated `ort`; defined `VadBackend` trait in `kural-audio` + `SherpaSileroVad` in `kural-ai` (§1.2, §2.2, §6.1). |
| **Critical** | `egm-design`, `egm-zoom-out` | `< 700 MB` Pi RAM threatened by WebKitGTK, `model2vec-rs` `f32` upcast, `glibc` arenas, `MemoryMax=2G` | Added `tikv-jemallocator`, `drop-before-load`, ephemeral WebKitGTK + `/tmp` cache, Phase 2 `> 96 MB` `memmap2` INT8 trigger, `MemoryHigh=700M`/`MemoryMax=850M` (§1.3, §8.2). |
| **Critical** | `egm-implementation-readiness` | `model2vec-rs` default features pull remote `hf-hub` HTTP crate | Pinned `default-features = false, features = ["onig", "local-only"]`, zero HTTP in `cargo tree` (§2.3, §5.1). |
| **Critical** | `egm-design`, `egm-implementation-readiness` | Single-task `BackendController` deadlocks on inline `start_capture` / blocking FFI; `broadcast` lag drops state | Offloaded DB/DSP/AI to `spawn_blocking` + `AiWorker`; supervised voice tasks with `(voice_session_id, nav_epoch)`; split `watch<Arc<UiSnapshot>>` + bounded `mpsc<UiTransientEvent>` (§4.4, §8.1). |
| **Critical** | `egm-design` | ALSA DMA drain overlap on `earcon_guard_ms`, unbounded `Listening`, WebKitGTK audio clash, missing xrun recovery | Increased `earcon_guard_ms = 220` post-`sink.empty()`, added `listening_timeout_ms = 5000`, `last_callback_ms` ALSA reopen watchdog, `GST_AUDIO_SINK=fakesink` (§3, §6.2–6.3). |
| **Critical** | `egm-design` | 4-way OOS bypass (`seed_kural`, rank-100 noise vector in RRF, unanchored numbers, missing KB floor) | Added anaphoric `seed_kural` gate, `min_vec_candidate_cosine = 0.25` pre-RRF filter, `kb_min_*` config keys, anchored number parser (§3.2, §7.2). |
| **Critical** | `egm-style`, `egm-inventive-catalyst` | Factor-of-2 physics bug in $L_{\text{vt}} = c / (4\Delta F)$ and noisy child off-axis rejection | Fixed $L_{\text{vt}} = 35000 / (2\Delta F)$, vowel-nucleus extraction, pre-roll noise subtraction, multi-order Burg `{10,12,14}` (§7.1). |
| **Critical** | `egm-style` | `80 Hz` 2nd-order HPF gives only `8.78 dB` @ `50 Hz`, failing `>= 18 dB` gate | Upgraded to 4th-order `80 Hz` Butterworth HPF + `50/60 Hz` biquad notches (`Q = 8.0`) fed continuously (§6.2, §9). |
| **Critical** | `egm-structural`, `egm-style`, `egm-zoom-out`, `egm-implementation-readiness` | `StepSpeed`, `OpenModal(Search)`, `RouterContext` & `SetCompactTab` mismatches; missing `[workspace.lints]` / `resolver = "3"` | Unified `RouterContext`/`VoiceRouteOutcome`, qualified `ModalKind::Search { initial_tab }`, documented local UI `#pi-mode-strip`, pinned `resolver = "3"` & `unsafe_code = "forbid"` (§2.3, §4.1, §7.4). |
| **Critical** | `egm-inventive-catalyst` | `26 px` 5-tab `#pi-mode-strip` on `480×320` violates `36 px` touch ergonomics and truncates prose | Adopted `40 + 204 + 76 px` budget, `38 px` 3-tab bar + Scholar Cycle pill, sentence pagination, `2×2` `220×42 px` Quiz/Game grids (§8.2). |
| **High** | `egm-design`, `egm-structural`, `egm-zoom-out`, `egm-style`, `egm-implementation-readiness` | `tts_cache_dir`/Opus conflict, `espeak-ng-data` budget, `BTreeSet<u16>`, `#395`/`#648` nav fallback, `user_state.db` CHECK/SD durability, Piper pause tags, `§4.6` bilingual copy, `CorpusCatalog` access | Removed disk Opus cache in favor of `<= 16 MB` `i16` RAM LRU; normalized `user_state.db` sets; budgeted `espeak-ng-data`; locked `§4.6` strings, `SpeakTarget` table, couplet `220 ms` silent `PcmChunk`, `CorpusCatalog` on `BackendHandle`, and `<= 5 s` debounced `last_kural_*` writes (§1.3, §4.1–§8.2). |

