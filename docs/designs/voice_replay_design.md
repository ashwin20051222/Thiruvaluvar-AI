# Text-to-Voice & Voice-to-Voice (Reply + Replay) Design Addendum

This design document extends [`backend_master_approved.md`](backend_master_approved.md) to deliver 100% pure-Rust, offline **Text-to-Voice (Reply + Replay)** and **Voice-to-Voice (Reply + Replay)** across Desktop (`1280×720`), 7-inch Pi (`800×480` / `1024×600`), and 3.5-inch Pi (`480×320`) displays with zero SD-card audio writes.

---

## 1. Contract & Persistence Additions (`kural-core`, `kural-db`)

### 1.1 `crates/kural-core/src/models.rs` & `contract.rs`
- Export `pub type Lang = Language;` for concise signature compatibility.
- Add `VoiceReplayMode`:
  ```rust
  #[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
  #[serde(rename_all = "snake_case")]
  pub enum VoiceReplayMode {
      AiReplyOnly,
      UserVoiceOnly,
      FullExchange,
  }
  ```
- Add `VoiceTurnSummary` (ring-capped at `5` newest-first turns in `UiSnapshot`):
  ```rust
  #[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
  pub struct VoiceTurnSummary {
      pub id: u64,
      pub user_text: String,
      pub ai_text: String,
      pub lang: Lang,
      pub has_user_audio: bool,
      pub has_ai_audio: bool,
      pub cited_kurals: Vec<u16>,
  }
  ```
- Extend `UserPrefs`:
  - `#[serde(default = "default_auto_speak_text")] pub auto_speak_text: bool` (default `true`).
- Extend `BackendCommand`:
  - `SpeakCustomText { text: String, lang: Lang, label: Option<String> }`
  - `ReplayLastTts` (kept alongside existing `ReplayAudio` for backward compatibility)
  - `ReplayVoiceTurn { turn_id: Option<u64>, mode: VoiceReplayMode }`
  - `SetAutoSpeakText(bool)`
  - `SetVoiceToVoiceLoop(bool)`
- Extend `UiSnapshot`:
  - `recent_voice_turns: Vec<VoiceTurnSummary>`
  - `can_replay_tts: bool` (kept synchronized with `has_replay_clip: bool`)
  - `can_replay_voice_turn: bool`
  - `auto_speak_text: bool` (default `true`)
  - `voice_to_voice_loop: bool` (default `false`)
  - `active_replay_label: Option<String>`

### 1.2 Locked Bilingual Copy (`crates/kural-core/src/copy.rs`)
- `REPLAY_AUDIO_NOTICE`: `"மீண்டும் ஒலிக்கிறது..."` / `"Replaying audio..."`
- `REPLAY_USER_VOICE_NOTICE`: `"உங்கள் குரல் மீண்டும் ஒலிக்கிறது..."` / `"Replaying your voice..."`
- `REPLAY_FULL_EXCHANGE_NOTICE`: `"முழு உரையாடலும் ஒலிக்கிறது..."` / `"Replaying conversation..."`
- `NOTHING_TO_REPLAY_NOTICE`: `"மீண்டும் ஒலிக்க எதுவும் இல்லை."` / `"Nothing to replay yet."`
- `AUTO_SPEAK_ON_NOTICE` / `AUTO_SPEAK_OFF_NOTICE`
- `VOICE_LOOP_ON_NOTICE` / `VOICE_LOOP_OFF_NOTICE` / `VOICE_LOOP_SILENCE_STOP_NOTICE`

### 1.3 Persistence Migration (`crates/kural-db/src/user_state.rs`)
- `preferences` table adds `auto_speak_text INTEGER NOT NULL CHECK (auto_speak_text IN (0, 1)) DEFAULT 1`.
- `UserStateStore::open` inspects `PRAGMA table_info(preferences)` and idempotently executes `ALTER TABLE preferences ADD COLUMN ... DEFAULT 1` on existing databases with zero data loss.

---

## 2. Bounded RAM Replay Caches & OS Audio Actor (`kural-audio`)

### 2.1 Memory Budget (0 Bytes on microSD / Disk)
| Structure | Retention Bound | RAM Cap | Sample Format | Eviction / Safety Rule |
| :--- | :--- | :--- | :--- | :--- |
| `TtsPcmCache` | `<= 16` keyed clips + `last_completed` | `<= 16 MB` (`config.ai.tts_cache_max_bytes`) | `i16` mono @ synth Hz | LRU eviction; deterministic `u64` key `fnv1a64(content_hash, lang_code, persona_code)` alongside string keys; cancelled clips never inserted |
| `VoiceTurnAudioStore` | `max_turns = 5` | `<= 8 MB` (`8 * 1024 * 1024`) | User `i16` @ `16 kHz`; AI `i16` @ synth Hz | FIFO turn eviction (`total_bytes + entry <= 8 MB`) across `push_user_turn` and `attach_ai_audio` |
| Active STT / TTS models | Single language pair | `<= 250 MB` | ONNX Int8 / Piper VITS | Strict drop-before-load inside `SpeechEngine` |

Total audio replay footprint stays strictly bounded at `<= 24 MB` in RAM (`16 MB + 8 MB`), well within the `MemoryHigh=700M` / `MemoryMax=850M` Raspberry Pi cgroup limits.

### 2.2 Playback Speed Calibration (Zero Double-Speeding)
- Each cached AI clip / AI voice turn records `synth_speed: f32` (default `1.0`).
- During `PlayCachedTtsByKey { cache_key, speed }`, `PlayLastCompletedTts { speed }`, or `PlayVoiceTurn { .., speed }`, `AudioActor` computes `playback_factor = (speed / synth_speed).clamp(0.5, 2.0)` via `rodio::Source::speed`.
- In `VoiceReplayMode::FullExchange`, user audio (`16 kHz`) plays with `.speed(1.0)` -> `350 ms` silent gap + `CmdRecognized` earcon (`.speed(1.0)`) -> AI audio (`ai_sample_rate_hz`, `.speed(playback_factor)`), queued cleanly on the single `rodio::Player`.

### 2.3 Echo-Guard State Transitions for Replay & Voice-to-Voice Loop
```text
[Idle] ──(PlayVoiceTurn / PlayCachedTts / PlayLastCompletedTts)──▶ [PlayingTts (mic gated closed, RMS=0)]
                                                                       │
       ┌─────────────────(Stop / <20ms sink.clear())───────────────────┤
       ▼                                                               │ (sink.empty())
   [Cooldown (250 ms, mic gated closed)] ◀─────────────────────────────┘
       │
       ├──(timer >= 250ms & loop OFF / replay / cancelled)──▶ [Idle]
       │
       └──(timer >= 250ms & voice_to_voice_loop == true & non-replay clean completion & !SilenceStop)
             │
             ▼
          [ListenEarconPlaying (ListenStart 90ms)]
             │ (sink.empty())
             ▼
          [ListenEarconGuard (220 ms)] ──▶ [Listening] ──▶ [CapturingSpeech] ──▶ [Processing] ──▶ [PlayingTts]
```

Safeguards:
- **Replays never re-arm capture**: `ReplayLastTts`, `ReplayAudio`, and `ReplayVoiceTurn` cancel any pending auto-rearm and return to `Idle` after `Cooldown (250 ms)`.
- **Auto-Disarm on Silence or Stop**: `StopAudio`, `CancelVoice`, screen navigation (`Navigate` / `SelectAge` / switching modals away), or `2` consecutive `AudioError::Timeout` / empty speech captures set `voice_to_voice_loop = false` and emit a bilingual notification.
- **Immediate Abort**: `StopPlayback` clears the entire `rodio::Player` queue (aborting `FullExchange` mid-turn too) within `< 20 ms`, enters `Cooldown (250 ms)`, and never commits partial/cancelled audio.

---

## 3. Streaming-While-Caching TTS, Language Detection & Intents (`kural-ai`, `kural-voice-cmd`)

- `kural_ai::detect_text_language(text: &str, fallback: Language) -> Language` counts Tamil block codepoints (`\u{0B80}..=\u{0BFF}`) vs ASCII Latin letters to match the spoken voice to typed/displayed script cleanly while respecting `SpeechEngine`'s drop-before-load memory guarantee.
- `SpeechEngine::synthesize_streaming_collecting` streams each synthesized sentence over `pcm_tx` immediately (`< 80 ms` first-audio latency) while collecting the completed `Vec<i16>` + `sample_rate_hz` for `TtsPcmCache` & `VoiceTurnAudioStore` insertion only upon clean `Ok` completion.
- `kural-voice-cmd` adds `< 1 ms` exact + `strsim` (`>= 0.80`) bilingual routing for:
  - `ReplayLastTts` (`"replay"`, `"say that again"`, `"மீண்டும் சொல்"`, etc.)
  - `ReplayVoiceTurn { turn_id: None, mode: UserVoiceOnly }` (`"replay my voice"`, `"என் குரலைப் போடு"`, etc.)
  - `ReplayVoiceTurn { turn_id: None, mode: FullExchange }` (`"replay conversation"`, `"முழு உரையாடலையும் போடு"`, etc.)
  - `SetVoiceToVoiceLoop(true | false)` (`"start voice chat"`, `"voice to voice"`, `"தொடர் குரல் உரையாடல்"` / `"stop voice chat"`, `"தொடர் குரல் நிறுத்து"`).

---

## 4. Tri-Display UI Geometry (`1280×720`, `1024×600`, `800×480`, `480×320`)

- **Main Screens (`S2/S3/S4`)**: Existing Replay (`🔁`) buttons invoke `BackendCommand::ReplayLastTts` (`can_replay_tts`), keeping button count and `40 + 204 + 76 px` Pi geometry unchanged.
- **Search Modal**: Each result row renders in a flex container with a dedicated `>= 38 px` `🔊` Listen button (`SpeakCustomText` for the Kural couplet + meaning) while preserving strict `2–3` rows/page pagination on `480×320` and `800×480`.
- **AI Chat Modal (`AiChatModal`)**:
  - Header/input bar includes a compact single-row toggle bar (`min-height: 38px`) with **`🔊 Auto-Speak`** (`SetAutoSpeakText`) and **`🎙️ Auto-Voice`** (`SetVoiceToVoiceLoop`). On `<= 560px` width, toggle text condenses concisely while retaining full `aria_label`s so zero wrapping/overflow occurs inside `.modal-card` (`max-height: 304px` on `480×320`).
  - Answer card footer renders compact `>= 38 px` **`🔊 Hear` / `🔁 Replay`** (or `⏹ Stop` while speaking).
  - When `snap.can_replay_voice_turn` is `true`, a single slim `.voice-replay-strip` bar appears above/within the response footer with 3 equal/compact `>= 38 px` buttons: **`🎙️ My Voice`**, **`🔊 AI Reply`**, and **`🔄 Full`**, styled cleanly with `flex-wrap: nowrap` and condensed labels on `480×320` (`max-height: 304px` strictly respected with `overflow: hidden` / zero clip).

