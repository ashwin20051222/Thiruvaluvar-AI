# Thiruvalluvar AI — Backend Master Architecture Draft (100% Pure Rust)

- **Document Role:** Phase 1 Architecture Draft (`backend_master_draft.md`) prior to Multi-Agent EGM Review & Synthesis
- **Project:** Bilingual Offline AI-Based Thirukkural Search, Age-Adaptive Interface & Voice Interaction System
- **Workspace Base:** `thiruvalluvar-ai-rs/` (Edition `2024`, Cargo `resolver = "3"`, Rust MSRV `1.85+`)
- **Target Hardware & Viewports:**
  - Raspberry Pi 4 Model B (`aarch64-unknown-linux-gnu`, 8 GB RAM, 32 GB SD) + 3.5-Inch Landscape LCD (`480×320`) + External Mic + Dual Amplified Stereo Speakers
  - Laptop / Desktop (`x86_64-unknown-linux-gnu`, `1280×720` / `1366×768`)
- **Non-Negotiable Mandates:** 100% Pure Rust (`0` Python files) • 100% Offline (`0` cloud calls, `0` API keys, `0` external CDNs) • Strict Literary Integrity (`Synopsis.pdf`) • Dual Modality (Touch + Bilingual Voice)

---

## 1. System Goals, Constraints, Budgets & Architectural Trade-Offs

### 1.1 Goals, Non-Goals & Anti-Goals
- **Goals:**
  1. Deliver an offline, instant-response bilingual (`ta` / `en`) Thirukkural kiosk on Raspberry Pi 4B (`480×320`) and Laptop (`1280×720`) with zero-scroll ergonomics and 1:1 touch + voice parity.
  2. Retrieve all `1,330` Kurals, `133` Adhikarams, `3` Paals (`Aram 1..=380`, `Porul 381..=1080`, `Inbam 1081..=1330`), and `9` Iyals verbatim from SQLite with zero alteration or fabrication (`Synopsis.pdf`).
  3. Adapt explanations, visuals, gamification, and TTS cadence dynamically to three age cohorts (`Kids` 6–12, `Youth` 13–17, `Adult` 18+) using either `< 30 ms` vocal-tract Voice Age Detection or manual card selection.
  4. Protect SD card endurance on Raspberry Pi 4B by keeping `data/thirukkural.db` strictly read-only (`mmap` + `PRAGMA query_only = ON`) and isolating all mutable user preferences/progress in `data/user_state.db`.
- **Non-Goals:**
  - Open-ended conversational generative AI (math homework, coding help, weather, current events): rejected at the semantic confidence gate with a polite bilingual limitation message.
  - Multi-user account authentication or cloud synchronization.
  - Web browser runtime (Chromium/Electron) or Python bindings (`PyO3`, `subprocess("python3")`).
- **Anti-Goals:**
  - Heavy quantized LLMs (`llama-cpp-2`, GGUF `1.5B–3B`), `whisper-small`, or `wav2vec2` that exceed `700 MB` RAM or introduce `2–12 s` CPU stalls on Pi 4B.
  - Self-triggering audio loops where dual amplified speakers feed back into the external mic.
  - Modifying `line1_ta`, `line2_ta`, or `#kural-initial-seal` into English when `Language::English` is active.

### 1.2 Approaches Considered & Chosen Stack
| Subsystem | Rejected Alternative & Why | Chosen Pure-Rust Edge Architecture |
|---|---|---|
| **3-Level Q&A & Persona** | Local `llama-cpp-2` GGUF (`1.5–2.2 GB` RAM, `3–8 s` latency on Cortex-A72, verse hallucination risk) | Deterministic intent router + `model2vec-rs` (`potion-multilingual-128M-int8`) + SQLite FTS5 RRF + pure-Rust 3-level composer over pre-verified `kural_child`/`kural_youth`/`kural_adult` columns (`< 15 ms`, `0%` hallucination). |
| **Voice Age Classification** | `wav2vec2` transformer (`~380 MB`, `400+ ms` CPU) or pitch-only threshold (confuses children with adult women) | Pure-Rust `rustfft` + pYIN $F_0$ + Burg LPC formants ($F_1$–$F_3$) + Vocal-Tract Length $L_{\text{vt}}$ + MFCC/HNR (`38` features, `< 30 ms`, `< 2 MB`). |
| **Speech Recognition (STT)** | `whisper.cpp` `small`/`base` (`1.2–3.5 s` non-streaming on Pi 4B) | `sherpa-onnx` streaming Zipformer-EN-20M (`< 100 ms`) + `indicconformer-ta-ctc-int8` (`150–300 ms`), lazy-switched in `pi_compact` mode. |
| **Text-to-Speech (TTS)** | Cloud neural TTS or blocking unchunked generation | Sentence-chunked `sherpa-onnx` Piper VITS (`< 80 ms` first audio) + PCM LRU replay cache (`0 ms`) + guaranteed `espeak-ng` fallback (`< 10 ms`). |
| **Desktop / Kiosk UI** | Chromium kiosk / Tauri web server (`350–600 MB` RSS overhead) | Native `dioxus-desktop` single-process binary rendered under `cage` Wayland compositor (`480×320` Pi / `1280×720` Laptop). |

### 1.3 Pi 4B Latency, Disk & Runtime Memory Budgets

#### Latency Targets (Raspberry Pi 4B Cortex-A72 CPU)
| Operation | Target Ceiling | Hard Test Threshold |
|---|---:|---:|
| Voice Command Router (`kural-voice-cmd`) | `< 1 ms` | `<= 2 ms` |
| `explain_kural` (direct DB + persona composition) | `< 1 ms` | `<= 5 ms` |
| 3-Level Semantic Explainer & Q&A (`ask_question` p99) | `< 15 ms` | `<= 20 ms` |
| INT8 Vector + FTS5 RRF Search (`1,330` Kurals + `45` KB) | `< 2 ms` | `<= 5 ms` |
| `StopAudio` Hardware Sink Abort | `< 20 ms` | `<= 25 ms` |
| Voice Age Detection (Stage 1 + 38 features + Classifier) | `< 30 ms` | `<= 50 ms` |
| Streaming TTS First Audio (`sherpa-onnx` Piper) / Cached Replay | `< 80 ms` / `0 ms` | `<= 120 ms` / `<= 5 ms` |
| English Streaming STT (`zipformer-en-20M`) | `< 100 ms` | `<= 150 ms` |
| Tamil Offline CTC-INT8 STT (`indicconformer-ta`) | `150–300 ms` | `<= 350 ms` |

#### Offline Model Disk Footprint (`models/`, Budget `< 350 MB`)
| Subdirectory | Model Artifact | Expected Size | Role |
|---|---|---:|---|
| `models/embeddings/` | `potion-multilingual-128M-int8` (`tokenizer.json`, `model.safetensors`, `config.json`) | `~64 MB` | Index-time + query-time multilingual semantic embeddings (`256`-dim INT8) |
| `models/vad/` | `silero_vad.onnx` | `~2 MB` | Neural speech start/end detector (`16 kHz` `512`-sample frames) |
| `models/stt_en/` | `sherpa-onnx-streaming-zipformer-en-20M` (`encoder/decoder/joiner-int8.onnx`, `tokens.txt`) | `~22 MB` | Streaming English voice command & Q&A ASR |
| `models/stt_ta/` | `indicconformer-ta-ctc-int8.onnx` + `tokens.txt` | `~145 MB` | Offline Tamil NeMo CTC ASR |
| `models/tts/` | `en_US-lessac-low.onnx` (`~18 MB`) + `ta_IN-ValluvaNeural-medium.onnx` (`~65 MB`) + configs/espeak-ng-data | `~83 MB` | Bilingual VITS voice synthesis |
| **Total Models** | **All offline ONNX / Safetensors weights** | **`~316 MB`** | **`34 MB` headroom under `350 MB` ceiling** |

#### Runtime RAM Budget (`< 700 MB` RSS on Pi 4B)
Two configurable `memory_mode` profiles in `config.toml` reconcile Pi 4B's `< 700 MB` ceiling with bilingual responsiveness:
- **`pi_compact` (Default on Pi 4B):** Keeps active `Language`'s STT + TTS resident; on `SetLanguage` switch, drops inactive recognizer/synthesizer arena (`ort` arena shrink / handle drop) and loads the requested language's models once (`~350–600 ms` one-time cold switch) while updating UI copy immediately.
- **`preload_both` (Optional on Laptop/8 GB Pi if desired):** Preloads both English and Tamil STT/TTS engines in memory (`~760 MB` peak).

| Memory Component (`pi_compact`) | Active Tamil Worst-Case RSS | Active English RSS |
|---|---:|---:|
| Linux/Dioxus WebKitGTK/GTK3 Shell + `app.css` + Fonts + Image | `~140 MB` | `~140 MB` |
| `thirukkural.db` `mmap` (`~31 MB`) + `user_state.db` + In-Memory INT8 Matrix (`~355 KB`) | `~48 MB` | `~48 MB` |
| `model2vec-rs` Query Encoder (`potion-multilingual-128M-int8`, mmap/resident) | `~72 MB` | `~72 MB` |
| `silero_vad.onnx` + `cpal`/`rodio` Ring Buffers + PCM Replay Cache (`16` clips max `24 MB`) | `~32 MB` | `~32 MB` |
| Active STT Engine (`stt_ta` `~165 MB` or `stt_en` `~35 MB`) | `~165 MB` | `~35 MB` |
| Active TTS Engine (`ta_IN` `~80 MB` or `en_US` `~28 MB`) | `~80 MB` | `~28 MB` |
| Age AI Workspace (`rustfft`/LPC buffers) + Tokio Threads | `~12 MB` | `~12 MB` |
| **Total Process RSS (`pi_compact`)** | **`~549 MB`** | **`~367 MB`** |

> **Hard Invariant:** Even worst-case Tamil STT + Tamil TTS + WebKitGTK (`~549 MB`) stays **`~151 MB` below the `700 MB` hard RAM cap**.

---

## 2. Multi-Crate Workspace Topology & Acyclic Dependency DAG

### 2.1 Directory Layout (`thiruvalluvar-ai-rs/`)
```text
thiruvalluvar-ai-rs/
├── Cargo.toml                         # Virtual manifest workspace root (Edition 2024, resolver 3)
├── .rustfmt.toml
├── config.toml                        # Hardware ALSA cards, mic VTL offsets, display mode, model paths
├── data/
│   ├── thirukkural.db                 # Read-only compiled DB: 1,330 Kurals + FTS5 + INT8 vectors
│   └── user_state.db                  # Mutable WAL DB: prefs, Kids stars, Youth streaks, Adult bookmarks
├── models/                            # < 350 MB offline edge models
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
├── seed_data/                         # Canonical JSON + 133 enriched JSON chapters + knowledge_base.json
├── stitch_extracted/                  # 16 labeled reference Stitch HTML files
├── scripts/
│   ├── download_fast_models.sh        # Idempotent + SHA256 verified model fetcher (< 350 MB)
│   ├── setup_pi.sh                    # ARM64 apt deps, ALSA config, LCD calibration, audio_test
│   └── build_pi_release.sh            # Cortex-A72 optimized release build
├── deploy/
│   ├── launch_kiosk.sh                # Wayland `cage` single-app kiosk launcher
│   └── thiruvalluvar-ai.service       # Systemd unit (`MemoryMax=2G`, auto-restart)
└── crates/
    ├── kural-core/                    # L0: Domain types, NFC helpers, validators, Command/Event contracts
    ├── kural-db/                      # L1: Read-only KuralDb + INT8 vectors + RRF + mutable UserStateStore
    ├── kural-enricher/                # L1: 100% offline deterministic synthesizer (existing)
    ├── kural-audio/                   # L1: Single-owner ALSA actor (cpal + rodio + rubato + Silero VAD + Echo Guard)
    ├── kural-voice-cmd/               # L1: Deterministic bilingual voice command router (< 1 ms)
    ├── kural-ai/                      # L2: Age AI (< 30 ms) + 3-Level Explainer (< 15 ms) + STT/TTS + ai_cli
    ├── kural-backend/                 # L3: Unified BackendController orchestrating L1/L2 + state sync
    └── kural-app/                     # L4: 100% Pure-Rust Dioxus Desktop GUI (0 JS app logic, 0 external URLs)
```

### 2.2 Acyclic Crate Dependency Graph (Compile-Time Enforced)
```text
L0:  kural-core  (no tokio, no rusqlite, no HTTP, no audio, no UI, no FFI)
        ▲
        ├───────────────┬──────────────────┬───────────────────┐
L1:  kural-db      kural-enricher     kural-audio       kural-voice-cmd
        ▲                                  │                   │
        ├──────────────────────────────────┼───────────────────┘
L2:  kural-ai  (depends on: kural-core, kural-db)
        ▲
        │
L3:  kural-backend (depends on: kural-core, kural-db, kural-audio, kural-ai, kural-voice-cmd)
        ▲
        │
L4:  kural-app  (depends ONLY on: kural-core, kural-backend)
```

- **Strict Layering Rules:**
  - `kural-app` MUST NOT depend directly on `rusqlite`, `cpal`, `rodio`, `ort`, `sherpa-onnx`, `kural-db`, `kural-audio`, `kural-ai`, or `kural-voice-cmd`. All hardware and database access flows strictly through `kural-backend` (`BackendHandle`).
  - `kural-db` runtime library target MUST have **zero** `reqwest` dependency: `fetch_canonical.rs` is gated behind `fetcher = ["dep:reqwest", "dep:tokio"]` and `required-features = ["fetcher"]`.
  - Workspace `[workspace.lints.rust]` forbids `unsafe_code` everywhere except FFI glue strictly encapsulated in `kural-ai` / `kural-audio` if needed by C-bindings (`#[allow(unsafe_code)]` prohibited in `kural-core`, `kural-db`, `kural-enricher`, `kural-voice-cmd`, `kural-backend`, `kural-app`).

---

## 3. Configuration File Contract (`config.toml`)

`thiruvalluvar-ai-rs/config.toml` provides deterministic hardware and threshold overrides, with compiled defaults if absent:

```toml
[paths]
kural_db = "data/thirukkural.db"
user_state_db = "data/user_state.db"
models_dir = "models"
tts_cache_dir = "data/tts_cache"

[audio]
input_device = "default"           # ALSA device substring or "default"
output_device = "default"          # ALSA stereo playback substring or "default"
target_sample_rate_hz = 16000
highpass_hz = 80.0
echo_cooldown_ms = 250             # Mute mic 250 ms after speaker buffer drains
earcon_guard_ms = 120              # Delay mic arming after start chime
vad_pre_roll_ms = 200
vad_trailing_silence_ms = 850
vad_threshold = 0.50               # Silero VAD speech probability cutoff
age_sample_duration_ms = 3200      # Capture window for S1 Voice Age Detection
max_command_duration_ms = 7000     # Max utterance duration for S2-S4 voice commands
rms_publish_hz = 30
master_volume = 0.85

[age_ai]
min_voiced_ms = 1200               # Reject < 1.2 s voiced speech with RetryReason::TooShort
min_snr_db = 8.0                   # Reject noisy room below 8 dB with RetryReason::TooNoisy
max_clipping_ratio = 0.02          # Reject >= 2% clipped samples with RetryReason::Clipped
min_rms_level = 0.015              # Reject whisper/dead mic with RetryReason::TooQuiet
low_confidence_threshold = 0.70    # Highlight manual cards below 70%
f0_offset_hz = 0.0                 # Per-microphone calibration offset
vtl_offset_cm = 0.0                # Per-microphone VTL calibration offset
formant_scale = 1.0                # Optional linear multiplier for room/mic spectral tilt

[ai]
memory_mode = "pi_compact"         # "pi_compact" (< 700 MB RAM) | "preload_both"
ort_intra_threads = 2              # Optimal on Pi 4B 4-core Cortex-A72 without audio contention
rrf_k = 60.0
oos_min_vector_cosine = 0.36       # Out-of-scope cosine floor when FTS5 + KB have no match
oos_min_rrf_score = 0.022          # Combined RRF threshold
tts_cache_entries = 16             # In-memory LRU PCM buffers for instant Replay

[ui]
viewport_mode = "desktop"          # "desktop" (1280x720) | "pi_preview" (480x320) | "kiosk"
toast_duration_ms = 1800
```

---

## 4. Locked Shared Types & Backend Contracts (`crates/kural-core`)

`kural-core` retains all existing types (`AgeGroup`, `Language`, `ResearchTheme`, `Division`, `Iyal`, `Adhikaram`, `KuralCore`, `KuralChild`, `KuralYouth`, `KuralAdult`, `Tag`, `FullKural`, `KuralSummary`, `SearchHit`, `SearchMatchKind`, `SearchOutcome`, `KnowledgeEntry`, `DbStats`) and adds the application-layer contract types in `crates/kural-core/src/contract.rs` (zero external dependencies beyond `serde` + `thiserror`).

### 4.1 New Domain Enums & Value Types
```rust
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Theme {
    Dark,
    Light,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum AppScreen {
    Start,
    Kids,
    Youth,
    Adult,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SearchTab {
    Query,
    Divisions,
}

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
pub enum Scholar {
    MuVa,
    Solomon,
    Kalaignar,
    Parimel,
    English,
}

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
pub enum AudioState {
    Idle,
    Listening,
    CapturingSpeech,
    Processing,
    PlayingTts,
    Cooldown,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum CaptureMode {
    FixedAgeSample { duration_ms: u32 },
    VadCommand { max_duration_ms: u32 },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum RetryReason {
    TooQuiet,
    TooShort,
    TooNoisy,
    Clipped,
    HardwareUnavailable,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct AgeDetection {
    pub age: AgeGroup,
    pub confidence: f32,
    pub low_confidence: bool,
    pub f0_hz: f32,
    pub formant_dispersion_hz: f32,
    pub vtl_cm: f32,
    pub latency_ms: u32,
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
    pub stt_en_loaded: bool,
    pub stt_ta_loaded: bool,
    pub tts_loaded: bool,
}
```

### 4.2 Persistent User State Types (`data/user_state.db`)
```rust
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct UserPrefs {
    pub lang: Language,
    pub theme: Theme,
    /// User-selected TTS speed multiplier: strictly {0.75, 1.0, 1.25}.
    pub speech_speed: f32,
    /// Master output volume [0.0, 1.0].
    pub volume: f32,
    pub muted: bool,
    pub default_age: AgeGroup,
    pub last_kural_kids: u16,
    pub last_kural_youth: u16,
    pub last_kural_adult: u16,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct KidsProgress {
    pub stars_earned: u32,
    pub completed_stories: Vec<u16>,
    pub completed_games: Vec<u16>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct YouthProgress {
    pub daily_streak: u32,
    pub last_active_date: String, // YYYY-MM-DD local date
    pub quiz_xp: u32,
    pub answered_quizzes: Vec<u16>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct BookmarkEntry {
    pub kural_number: u16,
    pub preferred_scholar: Scholar,
    pub created_at: String, // RFC3339 timestamp
}
```

### 4.3 Unified `BackendCommand` Enum (1:1 Frontend & Voice Coverage)
Every interaction cataloged in `docs/designs/frontend_audit.md` §3 and every intent emitted by `kural-voice-cmd` maps to one `BackendCommand` variant:

```rust
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[non_exhaustive]
pub enum BackendCommand {
    // Global Appearance & Navigation
    SetLanguage(Language),
    SetTheme(Theme),
    Navigate(AppScreen),
    SelectAge(AgeGroup),
    OpenModal(ModalKind),
    CloseModal,

    // Voice Interaction Lifecycle
    StartVoiceAgeDetect,
    StartVoiceCommand,
    CancelVoice,

    // Search & Navigation
    SubmitTextSearch { query: String, division_id: Option<u8> },
    OpenKural(u16),
    NextKural,
    PrevKural,
    OpenAdhikaram(u8),
    FilterByDivision(Option<u8>),
    FilterByTag(Option<String>),
    FilterByResearchTheme(Option<ResearchTheme>),

    // Scholar & Speech Playback
    SelectCommentary(Scholar),
    SpeakCurrent(SpeakTarget),
    ReplayAudio,
    StopAudio,
    SetSpeechSpeed(f32),
    SetMuted(bool),
    AdjustVolume(f32),

    // Interactive Gamification & Bookmarks
    /// Validates `choice_idx` (`0..=3`) identically across Tamil & English word options.
    SubmitKidsGameAnswer { kural_number: u16, choice_idx: u8 },
    SubmitYouthQuizAnswer { kural_number: u16, choice_idx: u8 },
    ToggleBookmark(u16),

    // Offline Grounded AI Q&A
    AskAiQuestion { query: String, seed_kural: Option<u16> },
}
```

### 4.4 `BackendEvent` & `UiSnapshot` Contracts
`UiSnapshot` captures the full initial state required to render any screen immediately upon `kural-app` startup without incremental flicker:

```rust
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct UiSnapshot {
    pub prefs: UserPrefs,
    pub screen: AppScreen,
    pub modal: Option<ModalKind>,
    pub current_kural: FullKural,
    pub active_scholar: Scholar,
    pub active_filter: Option<ActiveFilter>,
    pub pool_len: usize,
    pub pool_idx: usize,
    pub kids_progress: KidsProgress,
    pub youth_progress: YouthProgress,
    pub bookmarks: Vec<BookmarkEntry>,
    pub audio_state: AudioState,
    pub system_health: SystemHealth,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[non_exhaustive]
pub enum BackendEvent {
    Snapshot(Box<UiSnapshot>),
    PrefsUpdated(UserPrefs),
    ScreenChanged(AppScreen),
    ModalChanged(Option<ModalKind>),
    KuralLoaded {
        kural: Box<FullKural>,
        pool_len: usize,
        pool_idx: usize,
    },
    CommentarySelected {
        requested: Scholar,
        effective: Scholar,
        fallback_notice_ta: Option<String>,
        fallback_notice_en: Option<String>,
    },
    FilterUpdated {
        active: Option<ActiveFilter>,
        total_in_pool: usize,
        current_index: usize,
    },
    SearchResultsReady(SearchOutcome),
    AudioStateChanged(AudioState),
    AgeResult(AgeOutcome),
    VoiceCommandRecognized {
        transcript: String,
        toast_ta: String,
        toast_en: String,
    },
    AiAnswerReady(GroundedAnswer),
    KidsProgressUpdated(KidsProgress),
    KidsGameEvaluated {
        kural_number: u16,
        correct: bool,
        correct_idx: u8,
        stars_earned: u32,
    },
    YouthProgressUpdated(YouthProgress),
    YouthQuizEvaluated {
        kural_number: u16,
        correct: bool,
        correct_idx: u8,
        daily_streak: u32,
        quiz_xp: u32,
    },
    BookmarksUpdated(Vec<BookmarkEntry>),
    SystemHealthUpdated(SystemHealth),
    ErrorToast {
        message_ta: String,
        message_en: String,
    },
}
```
> **High-Frequency Telemetry Channel:** Live microphone amplitude (`30 Hz` RMS `f32`) travels over a separate `tokio::sync::watch::Receiver<f32>` exposed on `BackendHandle::subscribe_rms()` instead of the `BackendEvent` mpsc channel, preventing `1,800 events/min` from queueing behind or lagging state transitions.

### 4.5 Critical Frontend Audit Reconciliations Baked into Contracts
1. **Kids Word Game Bilingual Validation:** `SubmitKidsGameAnswer { kural_number, choice_idx }` computes `correct_idx = child.game_options_ta.iter().position(|w| w == &child.game_blank_word_ta)` (guaranteed `0..=3` by `kural-core` validation, where slot `s` holds the Tamil word in `game_options_ta[s]` and its matching English meaning in `game_options_en[s]`). First correct completion on `kural_number` awards `+1` star atomically in `user_state.db`.
2. **`meaning_parimel` NULL Handling (`#395` and `#648`):** `KuralDb` already stores `meaning_parimel: Option<String>` (`None` only on Kurals `#395` and `#648`). Selecting `Scholar::Parimel` when `kural.meaning_parimel.is_none()` emits `CommentarySelected { requested: Parimel, effective: MuVa, fallback_notice_ta: Some("இந்தக் குறளுக்குப் பரிமேலழகர் உரை இல்லை; மு.வ. உரை காட்டப்படுகிறது."), fallback_notice_en: Some("Parimelazhagar commentary is unavailable for this Kural; showing Mu. Va. instead.") }`. UI disables the `Parimel` tab tooltip accordingly.
3. **Unified Speech Speed Precedence:** All screens `S2`–`S4` standardize on user pills `[0.75, 1.0, 1.25]` persisted in `prefs.speech_speed`. Runtime TTS playback multiplies `prefs.speech_speed * age_cadence(screen)` (`0.95` Kids, `1.02` Youth, `1.00` Adult) clamped to `[0.65, 1.40]` so changing screens preserves user speed selection.
4. **Verbatim Tamil Script Invariant:** `#kural-couplet` ALWAYS binds `kural.line1_ta` / `kural.line2_ta` and `#kural-initial-seal` ALWAYS binds `kural.opening_letter_ta` regardless of `prefs.lang`; English mode renders `translation_en` / `couplet_en` underneath without replacing Tamil script.

---

## 5. Offline Storage & Semantic Indexing (`crates/kural-db` — Phase 2)

### 5.1 Read-Only Corpus Additions (`data/thirukkural.db`)
Add two foreign-keyed vector tables inside `crates/kural-db/src/schema.rs`:

```sql
CREATE TABLE IF NOT EXISTS kural_embeddings (
    kural_number INTEGER PRIMARY KEY REFERENCES kurals(number) ON DELETE CASCADE,
    vec_i8       BLOB NOT NULL, -- 256 signed i8 bytes packed as u8 slice
    norm         REAL NOT NULL  -- precomputed Euclidean L2 norm of quantized vec_i8
);

CREATE TABLE IF NOT EXISTS knowledge_embeddings (
    kb_id        INTEGER PRIMARY KEY REFERENCES knowledge_base(id) ON DELETE CASCADE,
    vec_i8       BLOB NOT NULL,
    norm         REAL NOT NULL
);
```

- **Indexing Binary (`src/bin/index_embeddings.rs`):**
  - Embeds concatenated bilingual text per Kural (`"{adhikaram_ta} {adhikaram_en} {meaning_mu_va} {translation_en} {explanation_en} {tags}"`) and per Knowledge Base item (`"{question_ta} {question_en} {answer_en} {keywords}"`).
  - Uses `model2vec-rs` loading `./models/embeddings/` (`potion-multilingual-128M-int8`, `256`-dim).
  - **Deterministic Offline Fallback:** If `./models/embeddings/` is absent during unit tests or CI before model download, falls back deterministically to a hashed feature-projection (`256`-dim L2-normalized character 3-gram + word unigram projection) tagged with `db_meta['embeddings_mode'] = 'hash_fallback'` vs `'model2vec'`, allowing all `cargo test` vector/RRF suites to run cleanly in zero-network sandboxes.
  - Quantizes each float vector to `i8` via `scale = max_abs / 127.0` and stores `vec_i8` (`256` bytes) + `norm = sqrt(sum(x_i^2))`. Total DB footprint increase: `(1330 + 45) × 268 B ≈ 368 KB`.
- **Startup Load & `< 2 ms` Similarity Engine (`src/vector.rs`):**
  - Loads all `1,330` Kural vectors into one cache-friendly flat `Vec<i8>` (`340,480` bytes) + `Vec<f32>` norms once on `KuralDb::open`.
  - Computes cosine similarity `dot_i8(query_i8, row_i8) / (query_norm * row_norm)` cleanly with LLVM auto-vectorizable inner loops (NEON on Pi 4B ARM64, AVX2/SSE on x86_64) in `< 0.8 ms`.
  - Merges top-`N` vector candidates with FTS5 BM25 results via Reciprocal Rank Fusion: `score_rrf(id) = 1.0 / (k + rank_fts) + 1.0 / (k + rank_vec)` (`k = 60.0`), preserving optional `division_id` filters.

### 5.2 Mutable User State Store (`data/user_state.db`)
Isolated completely from `data/thirukkural.db` via `UserStateStore` (`crates/kural-db/src/user_state.rs`):

```sql
PRAGMA journal_mode = WAL;
PRAGMA synchronous = NORMAL;
PRAGMA foreign_keys = ON;
PRAGMA busy_timeout = 3000;

CREATE TABLE IF NOT EXISTS preferences (
    id                 INTEGER PRIMARY KEY CHECK (id = 1),
    lang               TEXT NOT NULL CHECK (lang IN ('tamil', 'english')) DEFAULT 'tamil',
    theme              TEXT NOT NULL CHECK (theme IN ('dark', 'light')) DEFAULT 'dark',
    speech_speed       REAL NOT NULL CHECK (speech_speed IN (0.75, 1.0, 1.25)) DEFAULT 1.0,
    volume             REAL NOT NULL CHECK (volume >= 0.0 AND volume <= 1.0) DEFAULT 0.85,
    muted              INTEGER NOT NULL CHECK (muted IN (0, 1)) DEFAULT 0,
    default_age        TEXT NOT NULL CHECK (default_age IN ('kids', 'youth', 'adult')) DEFAULT 'youth',
    last_kural_kids    INTEGER NOT NULL DEFAULT 1,
    last_kural_youth   INTEGER NOT NULL DEFAULT 481,
    last_kural_adult   INTEGER NOT NULL DEFAULT 423
);

CREATE TABLE IF NOT EXISTS kids_progress (
    id                     INTEGER PRIMARY KEY CHECK (id = 1),
    stars_earned           INTEGER NOT NULL DEFAULT 0,
    completed_stories_json TEXT NOT NULL DEFAULT '[]',
    completed_games_json   TEXT NOT NULL DEFAULT '[]'
);

CREATE TABLE IF NOT EXISTS youth_progress (
    id                    INTEGER PRIMARY KEY CHECK (id = 1),
    daily_streak          INTEGER NOT NULL DEFAULT 0,
    last_active_date      TEXT NOT NULL DEFAULT '',
    quiz_xp               INTEGER NOT NULL DEFAULT 0,
    answered_quizzes_json TEXT NOT NULL DEFAULT '[]'
);

CREATE TABLE IF NOT EXISTS adult_bookmarks (
    kural_number      INTEGER PRIMARY KEY CHECK (kural_number BETWEEN 1 AND 1330),
    preferred_scholar TEXT NOT NULL CHECK (preferred_scholar IN ('mu_va', 'solomon', 'kalaignar', 'parimel', 'english')),
    created_at        TEXT NOT NULL
);
```

- **Daily Streak Logic:** When `SubmitYouthQuizAnswer` records a quiz attempt, if `last_active_date == today`, `daily_streak` stays unchanged; if `last_active_date == yesterday`, `daily_streak += 1`; otherwise `daily_streak = 1`. XP awards `+20 XP` on first correct answer per Kural, `+5 XP` on participation.

### 5.3 Dependency Hygiene in `crates/kural-db/Cargo.toml`
Move `reqwest` and `tokio` out of unconditional `[dependencies]` into `[features] fetcher = ["dep:reqwest", "dep:tokio"]` and gate `[[bin]] name = "fetch_canonical"` with `required-features = ["fetcher"]`. Runtime builds (`kural-backend`, `kural-app`) link **zero** HTTP crates.

---

## 6. Single-Owner Echo-Safe Audio Engine (`crates/kural-audio` — Phase 3)

### 6.1 Actor Architecture
One dedicated real-time-safe OS thread (`std::thread::Builder::new().name("kural-audio-actor")`) owns ALL `cpal::Stream` and `rodio::OutputStream` / `rodio::Sink` handles exclusively. Because `cpal::Stream` is `!Send` on several ALSA/platform backends and ALSA device opens deadlock if shared across threads, no other crate touches audio hardware.

Async callers communicate through `AudioHandle`:
```rust
#[derive(Clone)]
pub struct AudioHandle {
    cmd_tx: tokio::sync::mpsc::Sender<AudioActorCmd>,
    state_rx: tokio::sync::watch::Receiver<AudioState>,
    rms_rx: tokio::sync::watch::Receiver<f32>,
}

impl AudioHandle {
    pub async fn start_capture(&self, mode: CaptureMode) -> Result<CapturedAudio, AudioError>;
    pub async fn cancel_capture(&self) -> Result<(), AudioError>;
    pub async fn play_pcm_stream(&self, pcm_rx: mpsc::Receiver<PcmChunk>, cache_key: Option<String>) -> Result<(), AudioError>;
    pub async fn play_wav_bytes(&self, wav: &[u8]) -> Result<(), AudioError>;
    pub async fn replay_last(&self) -> Result<(), AudioError>;
    pub async fn stop_playback(&self) -> Result<(), AudioError>; // Aborts Sink in < 20 ms
    pub async fn set_volume(&self, volume: f32) -> Result<(), AudioError>;
    pub async fn set_muted(&self, muted: bool) -> Result<(), AudioError>;
    pub fn subscribe_rms(&self) -> watch::Receiver<f32>;
    pub fn subscribe_state(&self) -> watch::Receiver<AudioState>;
}
```
where `CapturedAudio` returns `samples_16k: Vec<f32>`, `speech_only_16k: Vec<f32>`, `voiced_ms: u32`, `snr_db: f32`, `clipping_ratio: f32`, and `rms_level: f32`.

### 6.2 Conditioning, Resampling & Silero VAD Pipeline
1. **Capture Callback:** `cpal` writes lock-free into a bounded SPSC ring buffer (`ringbuf`, `1.5 s` capacity). Audio callback performs zero heap allocations and zero locks.
2. **Actor Loop (every `16 ms` tick / `256` target samples):**
   - Multi-channel input downmixed to `f32` mono.
   - `rubato` (`SincFixedIn` / `FftFixedInOut`) resamples native ALSA rate (`44.1 kHz` / `48 kHz` / `16 kHz`) cleanly to `16,000 Hz`.
   - **DC-Offset Removal + `80 Hz` 2nd-Order Butterworth High-Pass Biquad:** Strips Pi SMPS `50/60 Hz` mains hum and desk thumps before RMS & VAD calculation.
   - **RMS Publication:** Exponentially smoothed RMS normalized to `[0.0, 1.0]` published to `rms_rx` at `30 Hz` ONLY while capturing; emits `0.0` during `Idle`, `PlayingTts`, and `Cooldown`.
   - **Silero VAD (`silero_vad.onnx` via `ort`, `512`-sample `32 ms` hop at `16 kHz`):** Maintains stateful `(h, c)` hidden tensors reset per utterance, `200 ms` circular pre-roll buffer, transitions `Listening → CapturingSpeech` when probability `>= vad_threshold`, and auto-stops after `850 ms` continuous trailing silence (or `max_duration_ms`). Falls back cleanly to a pure-Rust short-time energy + zero-crossing-rate (ZCR) detector when `models/vad/silero_vad.onnx` is unavailable.

### 6.3 Strict Half-Duplex Echo Guard & Earcons
```text
               StartVoice*
  [Idle] ────────────────────────► (Play Listen Earcon + 120 ms Guard) ─► [Listening]
    ▲                                                                          │
    │                                                           VAD speech     ▼
    │                                                                  [CapturingSpeech]
    │   250 ms timer                                                           │
    ├── [Cooldown] ◄─── Sink drained / StopAudio ◄──── [PlayingTts] ◄── [Processing]
                                                        (MIC MUTED)       (STT/Explainer)
```

- **Strict Hardware Mute Gate:** Whenever `AudioState` is `PlayingTts`, `Processing`, or `Cooldown(250ms)`, the capture consumer drains and **discards** every ring-buffer sample and clamps `rms_rx = 0.0`. Dual amplified speakers cannot re-trigger STT or corrupt voice age samples.
- **Touch Barge-In:** Calling `stop_playback()` sends a high-priority command that calls `sink.stop()` + `sink.clear()` immediately (`< 20 ms`), transitions to `Cooldown(250ms)`, and returns control cleanly.
- **Pure-Rust PCM Earcons:** Pre-synthesized at startup (`~120 KB` total RAM):
  - `ListenStart`: two soft ascending sine-enveloped tones (`523.25 Hz → 659.25 Hz`, `90 ms` total, followed by `120 ms` guard before mic arming).
  - `CmdRecognized`: gentle harmonic triad (`659.25 Hz → 783.99 Hz`, `80 ms`).
  - `RetryTone`: warm low double-pulse (`330 Hz → 261.63 Hz`, `140 ms`).
- **PCM Replay Buffer:** `AudioActor` retains the last completed TTS `Vec<f32>` buffer (plus `tts_cache_entries` LRU keyed by `(target_hash, lang, speed)`) so `ReplayAudio` starts playback in `0 ms` with zero re-synthesis.

### 6.4 Diagnostic Binary (`crates/kural-audio/src/bin/audio_test.rs`)
- `--list`: enumerates ALSA capture/playback devices, supported sample rates, and channels.
- Default run: measures `1.5 s` ambient noise floor (dB RMS), plays stereo earcon (`L`/`R`), records `3.0 s` speech clip, computes SNR, peak, clipping `%`, and echoes back through `rodio` verifying Echo Guard state transitions.

---

## 7. Zero-Latency Edge AI & Voice Command Router (`crates/kural-ai`, `crates/kural-voice-cmd` — Phase 4)

### 7.1 Instant Vocal-Tract Voice Age AI (`crates/kural-ai/src/age.rs` — `< 30 ms`)
1. **Stage 1 — Speech Quality Gate (`< 3 ms`):**
   - Evaluates captured voiced segments (`speech_only_16k`):
     - `voiced_ms < 1200` → `AgeOutcome::Retry(RetryReason::TooShort)`
     - `rms_level < 0.015` → `AgeOutcome::Retry(RetryReason::TooQuiet)`
     - `clipping_ratio >= 0.02` → `AgeOutcome::Retry(RetryReason::Clipped)`
     - `snr_db < 8.0` → `AgeOutcome::Retry(RetryReason::TooNoisy)`
2. **Stage 2 — 38-Feature Vocal-Tract Extractor (`< 15 ms`, zero ONNX overhead):**
   - **Pitch ($F_0$, 4 features):** `rustfft` + pYIN cumulative mean normalized difference across `25 ms` frames (`10 ms` hop, `70..=500 Hz`), extracting `f0_median`, `f0_iqr`, `f0_p10`, `f0_p90` (+ `config.age_ai.f0_offset_hz`).
   - **Burg LPC Formants & Vocal-Tract Length (5 features):** Downsample voiced frames to `10 kHz` (Nyquist `5 kHz` isolates `F1..F4` cleanly), apply pre-emphasis (`0.97`) + Hamming window + **Burg algorithm (`order = 12`)** → Durand-Kerner / companion-matrix polynomial roots → select conjugate poles (`bandwidth < 450 Hz`, `250 Hz <= f <= 4200 Hz`) → compute medians `(F1, F2, F3)`, Formant Dispersion $\Delta F = \frac{1}{2}((F_2 - F_1) + (F_3 - F_2))$ (and Fitch 4-formant dispersion if $F_4$ present), and physical Vocal-Tract Length $L_{\text{vt}} = \frac{35000}{4 \Delta F} + \text{vtl\_offset\_cm}$:
     - **Kids (Ages 6–12):** $L_{\text{vt}} \approx 10.5\text{–}13.2\text{ cm}$, higher $\Delta F > 1020\text{ Hz}$, even when $F_0$ overlaps Adult Female (`210–280 Hz`).
     - **Youth (Ages 13–17):** transitional $L_{\text{vt}} \approx 13.3\text{–}15.2\text{ cm}$ (`Male` cracking $F_0$ drop / higher `f0_iqr`; `Female` mid formant spacing).
     - **Adult (Ages 18+):** $L_{\text{vt}} \ge 15.0\text{ cm}$ (`Female ~14.0–15.8 cm` with tighter $\Delta F < 990\text{ Hz}$, `Male ~16.5–18.5 cm` with lower $F_0$).
   - **Spectral & MFCC Features (29 features):** `13` Mel-Frequency Cepstral Coefficients (`mean` + `std` = `26`), spectral centroid, spectral tilt (`log10` band slope `0.3–3 kHz`), and Harmonic-to-Noise Ratio (`HNR`).
3. **Stage 3 — Calibrated 3-Bracket Classifier (`< 1 ms`):**
   - Pure-Rust linear-quadratic discriminant + softmax calibration weights embedded as `const` arrays (`< 16 KB`).
   - Outputs `AgeDetection { age, confidence, low_confidence: confidence < config.low_confidence_threshold, f0_hz, formant_dispersion_hz, vtl_cm, latency_ms }`.

### 7.2 True 0-Latency 3-Level Explainer & Out-of-Scope Guardrail (`crates/kural-ai/src/explainer.rs` — `< 15 ms`)
- **Tier 1 (`explain_kural(number, age, lang, scholar)` — `< 1 ms`):**
  - Calls `KuralDb::get_kural_full(number)` and formats age-matched delivery directly from pre-verified database columns (`kural_child`, `kural_youth`, `kural_adult`, and `Scholar` with explicit `#395`/`#648` `Parimel -> MuVa` fallback).
- **Tier 2 (`ask_question(query, age, lang, seed_kural)` — `< 10 ms`):**
  1. **Deterministic Fast Path (`< 1 ms`):**
     - Detects Kural numbers (`kural 42`, `குறள் 481`), Adhikaram queries (`அதிகாரம் 5`), division/count queries (`how many kurals`, `எத்தனை அதிகாரம்`, `மூன்று பால்`), or `"explain this"` / `"விளக்கு"` (binds `seed_kural`).
  2. **Hybrid RRF Retrieval (`< 5 ms`):**
     - Runs `KuralDb::search_hybrid_rrf(query, None, 5)` (FTS5 + `model2vec-rs` INT8 cosine + `strsim` typo recovery) and `KuralDb::search_knowledge_rrf(query, 3)` (all `45` Thiruvalluvar bio/structure/FAQ rows).
  3. **Strict Out-of-Scope Guardrail (`Synopsis.pdf`):**
     - If no deterministic structural/number/seed match fired AND best Kural RRF score `< config.oos_min_rrf_score` AND best vector cosine `< config.oos_min_vector_cosine` AND no Knowledge Base hit crosses confidence threshold: returns `GroundedAnswer { out_of_scope: true, cited_kurals: vec![], kb_hits: vec![], answer_ta: "...", answer_en: "...", latency_ms }` politely stating that Thiruvalluvar AI strictly answers Thirukkural couplet, commentary, and Thiruvalluvar biography questions.
  4. **Pure-Rust 3-Level Persona Composer (`< 1 ms`):**
     - **Kids (`AgeGroup::Kids`):** Grandpa Valluvar warm greeting + `simple_meaning_*` + `kutty_story_*` excerpt + `moral_*` + verbatim Kural couplet.
     - **Youth (`AgeGroup::Youth`):** Royal Mentor direct framing + verbatim Kural + `analysis_*` + practical `action_tip_*` + topic tags.
     - **Adult (`AgeGroup::Adult`):** Scholarly synthesis + verbatim Kural + `transliteration` + primary scholar commentary (`MuVa`/`Solomon`/`Kalaignar`/`Parimel`) + `reflection_*` + `research_theme`.
     - **Invariant:** `cited_kurals` contains ONLY exact `FullKural` records fetched from `KuralDb::get_kural_full`.

### 7.3 Offline Bilingual Speech Engine (`crates/kural-ai/src/stt.rs` & `tts.rs`)
Both STT and TTS define clean internal traits (`SttBackend`, `TtsBackend`) so `sherpa-onnx` FFI implementations are isolated to two modules while deterministic mock/fallback implementations power offline CI tests:
- **STT (`stt.rs`):**
  - `Language::English`: `sherpa-onnx` `OnlineRecognizer` (`sherpa-onnx-streaming-zipformer-en-20M`, `100 ms` endpoint chunks, `< 100 ms` finalization).
  - `Language::Tamil`: `sherpa-onnx` `OfflineRecognizer` NeMo CTC (`indicconformer-ta-ctc-int8.onnx`, `~150–300 ms`).
  - Under `memory_mode = "pi_compact"`, `SpeechEngine` loads only the active `Language`'s STT/TTS pair at startup and swaps cleanly when `SetLanguage` arrives.
- **TTS (`tts.rs`):**
  - **Spoken-Text Normalizer:** Converts numbers `1..=1330` into spoken Tamil (`ஒன்று`, `நானூற்று எண்பத்தொன்று`, ...) or English words, strips Markdown/emojis, NFC-normalizes Tamil, inserts natural `220 ms` pause markers between `line1_ta` and `line2_ta`.
  - **Priority Chain:**
    1. In-memory LRU PCM cache / local pre-cached Opus cache (`0 ms`).
    2. Sentence-chunked streaming `sherpa-onnx` `OfflineTts` (`en_US-lessac-low.onnx` / `ta_IN-ValluvaNeural-medium.onnx`, streaming sentence 1 immediately `< 80 ms` while synthesizing sentences 2..N in worker queue).
    3. Guaranteed local `espeak-ng` binary fallback (`espeak-ng -v ta` / `-v en --stdout`, `< 10 ms`) piped into `AudioHandle::play_wav_bytes` / `play_pcm_stream`.

### 7.4 Deterministic Bilingual Voice Command Router (`crates/kural-voice-cmd`)
Pure, allocation-light function:
```rust
pub fn route_transcript(
    raw_transcript: &str,
    lang_hint: Language,
    context: RouterContext, // { screen: AppScreen, modal: Option<ModalKind>, active_kural: u16 }
) -> VoiceRouteOutcome
```
- **Supported Commands (`< 1 ms`, `strsim >= 0.80` typo tolerance on commands >= 4 chars):**
  - **Navigation & Modals:** `முகப்பு`/`home`/`start` → `Navigate(Start)`; `பின்செல்`/`back` → `CloseModal` if modal open else `Navigate(Start)`; `சிறுவர்`/`kids`/`child` → `SelectAge(Kids)`; `இளைஞர்`/`youth`/`teen` → `SelectAge(Youth)`; `பெரியோர்`/`adults`/`scholar` → `SelectAge(Adult)`; `வயது அறி`/`detect age` → `StartVoiceAgeDetect`; `தேடல்`/`search` → `OpenModal(Search)`; `உரையாடல்`/`ai`/`chat` → `OpenModal(AiChat { seed_kural: Some(active_kural) })`; `மூடு`/`close` → `CloseModal`.
  - **Theme & Language:** `தமிழ்`/`tamil` → `SetLanguage(Tamil)`; `ஆங்கிலம்`/`english` → `SetLanguage(English)`; `இருள்`/`dark mode` → `SetTheme(Dark)`; `ஒளி`/`light mode` → `SetTheme(Light)`.
  - **Kural & Playback:** `குறள் <N>`/`kural <N>` (parses both digits and spoken Tamil/English number words `1..=1330`) → `OpenKural(n)`; `அதிகாரம் <N>`/`chapter <N>` → `OpenAdhikaram(n)`; `அடுத்தது`/`next` → `NextKural`; `முந்தையது`/`previous`/`prev` → `PrevKural`; `படி`/`கேள்`/`listen`/`read` → `SpeakCurrent(screen_default_target)`; `மீண்டும்`/`replay` → `ReplayAudio`; `நிறுத்து`/`stop` → `StopAudio`; `மெதுவாக`/`slower` → `StepSpeed(-1)`; `வேகமாக`/`faster` → `StepSpeed(+1)`.
  - **Screen-Specific Learning:** `கதை`/`story` → `OpenModal(KidsStory)`; `விளையாட்டு`/`game` → `OpenModal(KidsWordGame)`; `சவால்`/`quiz` → `OpenModal(YouthQuiz)`; `விரிவான உரை`/`deep dive` → `OpenModal(YouthDeepDive)`; `மு.வ.`/`mu va` → `SelectCommentary(MuVa)`; `சாலமன்`/`solomon` → `SelectCommentary(Solomon)`; `கலைஞர்`/`kalaignar` → `SelectCommentary(Kalaignar)`; `பரிமேலழகர்`/`parimel` → `SelectCommentary(Parimel)`; `விவாதி`/`discuss` → `OpenModal(AiChat { seed_kural: Some(active_kural) })`.
  - **Fallback Routing:** Unmatched transcripts route deterministically by context: if `modal == Some(ModalKind::Search { .. })` → `SubmitTextSearch`; else → `AskAiQuestion { query, seed_kural: Some(active_kural) }` (and `BackendController` automatically speaks the short answer summary via TTS on hands-free queries).

---

## 8. Unified Controller & 100% Pure-Rust Dioxus UI (`crates/kural-backend`, `crates/kural-app` — Phase 5)

### 8.1 `BackendController` Single-Actor State Machine (`crates/kural-backend`)
- Runs a single serialized `tokio` task consuming `cmd_rx: mpsc::Receiver<BackendCommand>`.
- Holds `KuralDb`, `UserStateStore`, `AudioHandle`, `AgeClassifier`, `SemanticExplainer`, `SpeechEngine`, `BackendState`, and `event_tx: broadcast::Sender<BackendEvent>`.
- **Touch vs. Voice Serialization:** UI touch callbacks and asynchronous `VoiceCommandRouter` completions both push `BackendCommand` into the same bounded `mpsc` queue. If a user taps `PrevKural` while STT was finishing, both commands execute sequentially against consistent state with zero data races.
- **Filter & Pool Rules:**
  - Setting `FilterByDivision(Some(div))`, `FilterByTag(Some(slug))`, or `FilterByResearchTheme(Some(theme))` builds an ordered `Vec<u16>` pool (`<= 1330` items) and loads `pool[0]`. Clearing (`None`) restores the age pool (`1..=1080` `kids_featured` on `S2`, `1..=1330` on `S3`/`S4`) anchored cleanly to the current Kural if present.

### 8.2 Offline Design System & Phase 0 Audit Remediation Checklist (`crates/kural-app`)
1. **Viewport & Overflow Lock (`app.css`):**
   ```css
   @media (max-width: 560px) and (max-height: 380px) {
     .pi-root, #app-shell {
       height: 100dvh !important;
       max-height: 100dvh !important;
       width: 100vw !important;
       overflow: hidden !important;
     }
   }
   ```
   Fixes Stitch's `height: 1024px` defect across all `4` screens.
2. **Zero External URLs (`grep -rn "https://" crates/kural-app/assets/` == 0):**
   - Local `assets/app.css` replacing `https://cdn.tailwindcss.com`.
   - Local `.woff2` fonts in `assets/fonts/` (`Inter`, `NotoSansTamil`, `NotoSerifTamil`, `BalooThambi2`).
   - Local `assets/valluvar-portrait.png` replacing all `12` `lh3.googleusercontent.com` `<img>` tags across `S1`/`S2`.
3. **Complete Control & Modal Parity:**
   - **Component Tree:** `app.rs`, `top_bar.rs` (including `#2ECC8F` Dark offline dot + desktop mute + Age switcher), `voice_hud.rs` (`30 Hz` 5-bar RMS equalizer + `1.8 s` toast), `start_screen.rs` (`S1` + fixed `<footer class="pi-footer">` across both languages), `child_view.rs` (`S2` with expanded **5-slot** Pi dock: `Story`, `Game`, `#pi-voice-orb`, `Audio`, `Search` + Stop button), `youth_view.rs` (`S3` with unhidden `.pi-streak-pill`, `Stop` button, clickable Topic Tag Chips, `Daily Quiz`, `Deep Dive`, unified `0.75x/1.0x/1.25x` speed pill), `adult_view.rs` (`S4` with 5 scholar tabs including optional `Parimel` with `#395`/`#648` notice, button `#pi-speed-chip`, mute button, right-col Reflection + 4 `.royal-subcard` Research Themes + Bookmarks, and immutable Tamil `#kural-couplet`/`#kural-initial-seal`), and all 6 zero-scroll overlays (`search_modal.rs`, `chat_modal.rs`, `learning_modals.rs` for `KidsStory`, `KidsWordGame`, `YouthQuiz`, `YouthDeepDive`).
4. **CLI Viewport Modes:** Default `1280×720` resizable Laptop window, `--pi-preview` exact `480×320` non-resizable preview window on Laptop, `--kiosk` borderless fullscreen for `cage` on Pi 4B.

---

## 9. Deployment & Phase Verification Matrix (Phases 2–6)

| Phase | Gate Checks & Commands |
|---|---|
| **Phase 2** | `kural_cli stats` prints `1330/1330`, `integrity_check: ok`; `index_embeddings` populates `kural_embeddings` (`1330`) & `knowledge_embeddings` (`45`); `UserStateStore` round-trip tests pass; zero `reqwest` in runtime `kural-db` tree (`cargo tree -p kural-db` shows no `reqwest`); `cargo test --workspace` & `cargo clippy` pass. |
| **Phase 3** | `audio_test` runs cleanly; unit tests verify Echo Guard discards samples during `PlayingTts` + `Cooldown(250ms)`, `rubato` `48k → 16k` tone preservation, `80 Hz` HPF attenuates `50/60 Hz` hum (`>= 18 dB`), and `stop_playback()` aborts `< 20 ms`. |
| **Phase 4** | `ai_cli` benchmarks pass (`Age < 30 ms` CPU / `< 50 ms` gate, `Explainer < 15 ms` p99 `< 20 ms`, `VoiceCmd < 1 ms` gate `< 2 ms`, `STT < 300 ms`); unit tests verify Kid vs Adult-Female VTL separation (`11.5 cm` vs `14.8 cm`), exact verbatim SQLite Kurals, out-of-scope bilingual rejection, and Tamil/English fuzzy commands. |
| **Phase 5** | `cargo run --release -p kural-app` (`1280×720`) & `--pi-preview` (`480×320`) render zero-scroll across all `4 × 2 × 2 = 16` combinations; `grep -rn "https://" crates/kural-app/assets/` returns `0`. |
| **Phase 6** | `scripts/setup_pi.sh`, `scripts/build_pi_release.sh`, `deploy/launch_kiosk.sh`, `deploy/thiruvalluvar-ai.service` generated & shellchecked; `find . -name "*.py"` returns `0`; `cargo clippy --workspace --all-targets -- -D warnings` & `cargo test --workspace` pass; `README.md` & `docs/PI_DEPLOYMENT_GUIDE.md` complete. |

