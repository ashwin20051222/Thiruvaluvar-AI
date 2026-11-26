use crate::cache::{TtsPcmCache, VoiceTurnAudioStore};
use crate::devices::{find_input_device, find_output_device};
use crate::dsp::{AudioConditioner, Resampler16k, TARGET_SAMPLE_RATE_HZ, compute_rms};
use crate::earcon::{EARCON_SAMPLE_RATE_HZ, EarconBank};
use crate::error::AudioError;
use crate::types::{AudioHealthStatus, CapturedAudio, EarconKind, PcmChunk, VadBackend};
use crate::vad::{EnergyZcrVad, FRAME_SAMPLES_16K, FrameAction, UtteranceFramer};
use cpal::traits::{DeviceTrait, StreamTrait};
use cpal::{FromSample, SampleFormat, SizedSample, StreamConfig};
use kural_core::{AudioConfig, AudioState, CaptureMode, VoiceReplayMode};
use ringbuf::traits::{Consumer, Observer, Producer, Split};
use ringbuf::{HeapCons, HeapProd, HeapRb};
use rodio::Source;
use std::num::NonZero;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::thread::{self, JoinHandle};
use std::time::{Duration, Instant};
use tokio::sync::{mpsc, oneshot, watch};

const ACTOR_TICK_MS: u64 = 16;
const COMMAND_QUEUE_CAP: usize = 64;
const TRAILING_SILENCE_MS: u32 = 850;
const DEFAULT_TTS_CACHE_MAX_ENTRIES: usize = 16;
const WATCHDOG_TIMEOUT_MS: u64 = 500;
const REOPEN_BACKOFF_MS: [u64; 3] = [500, 1000, 2000];
const MAX_TICKS_RING_DRAIN_CHUNKS: usize = 16;

/// Sub-phases of the strict half-duplex Echo Guard state machine.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum EchoGuardPhase {
    Idle,
    /// Playing `EarconKind::ListenStart` before opening the microphone gate.
    ListenEarconPlaying,
    /// Waiting `earcon_guard_ms` (`220 ms`) after `sink.empty()` for acoustic room tail decay.
    ListenEarconGuard {
        guard_until: Instant,
    },
    /// Listening for voiced onset (`AudioState::Listening`).
    Listening,
    /// Actively capturing speech (`AudioState::CapturingSpeech`).
    CapturingSpeech,
    /// Post-capture processing (`AudioState::Processing`).
    Processing,
    /// Playing TTS or earcon (`AudioState::PlayingTts`).
    PlayingTts,
    /// Waiting `echo_cooldown_ms` (`250 ms`) after playback completion or abort.
    Cooldown {
        until: Instant,
    },
}

/// Action required from the actor after requesting capture start on [`EchoGuardState`].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CaptureStartAction {
    /// Abort any active playback, then play `EarconKind::ListenStart` before entering `Listening`.
    PlayListenStart { abort_existing_playback: bool },
    /// Output hardware is unavailable; open `Listening` immediately without earcon.
    EnterListeningImmediately { abort_existing_playback: bool },
}

/// Transition signal returned by [`EchoGuardState::advance`].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum GuardTransition {
    None,
    EnteredListening,
    EnteredCooldown,
    CooldownElapsed,
}

/// Action required from the actor when `stop_playback` is invoked.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum StopPlaybackAction {
    NoOp,
    CancelCapture,
    AbortListenEarconAndCancelCapture,
    AbortPlayback,
}

/// Pure-Rust half-duplex Echo Guard state machine driving mic gating, RMS clamping,
/// earcon tail guard (`220 ms`), and post-playback echo cooldown (`250 ms`).
#[derive(Debug, Clone)]
pub struct EchoGuardState {
    phase: EchoGuardPhase,
    earcon_guard_ms: u32,
    echo_cooldown_ms: u32,
    aec_enabled: bool,
    cooldown_floor: Option<Instant>,
}

impl EchoGuardState {
    /// Create a new Echo Guard state machine initialized to `Idle`.
    #[must_use]
    pub fn new(earcon_guard_ms: u32, echo_cooldown_ms: u32, aec_enabled: bool) -> Self {
        Self {
            phase: EchoGuardPhase::Idle,
            earcon_guard_ms,
            echo_cooldown_ms,
            aec_enabled,
            cooldown_floor: None,
        }
    }

    /// Return the current granular Echo Guard phase.
    #[must_use]
    pub fn phase(&self) -> EchoGuardPhase {
        self.phase
    }

    /// Map the internal Echo Guard phase to the public [`AudioState`].
    #[must_use]
    pub fn audio_state(&self) -> AudioState {
        match self.phase {
            EchoGuardPhase::Idle
            | EchoGuardPhase::ListenEarconPlaying
            | EchoGuardPhase::ListenEarconGuard { .. } => AudioState::Idle,
            EchoGuardPhase::Listening => AudioState::Listening,
            EchoGuardPhase::CapturingSpeech => AudioState::CapturingSpeech,
            EchoGuardPhase::Processing => AudioState::Processing,
            EchoGuardPhase::PlayingTts => AudioState::PlayingTts,
            EchoGuardPhase::Cooldown { .. } => AudioState::Cooldown,
        }
    }

    /// Return `true` only when the microphone gate is open (`Listening` or `CapturingSpeech`).
    ///
    /// Samples arriving in `Idle`, `ListenEarconPlaying`, `ListenEarconGuard`, `Processing`,
    /// `PlayingTts`, or `Cooldown` MUST still pass through `Resampler16k` + `AudioConditioner`
    /// to keep biquad filter state settled, and then be discarded.
    #[must_use]
    pub fn accepts_mic_samples(&self) -> bool {
        matches!(
            self.phase,
            EchoGuardPhase::Listening | EchoGuardPhase::CapturingSpeech
        )
    }

    /// Gate outgoing RMS level to `0.0` whenever microphone capture is gated closed.
    #[must_use]
    pub fn effective_rms(&self, measured_rms: f32) -> f32 {
        if self.accepts_mic_samples() && measured_rms.is_finite() {
            measured_rms.max(0.0)
        } else {
            0.0
        }
    }

    /// Begin a capture request (`StartVoice*`). Rejects overlapping captures with `CaptureInProgress`.
    pub fn begin_capture(
        &mut self,
        now: Instant,
        has_output: bool,
    ) -> Result<CaptureStartAction, AudioError> {
        match self.phase {
            EchoGuardPhase::ListenEarconPlaying
            | EchoGuardPhase::ListenEarconGuard { .. }
            | EchoGuardPhase::Listening
            | EchoGuardPhase::CapturingSpeech => return Err(AudioError::CaptureInProgress),
            EchoGuardPhase::Cooldown { until } if now < until => {
                self.cooldown_floor = Some(until);
            }
            _ => {
                self.cooldown_floor = None;
            }
        }

        let abort_existing_playback = matches!(self.phase, EchoGuardPhase::PlayingTts);
        if has_output {
            self.phase = EchoGuardPhase::ListenEarconPlaying;
            Ok(CaptureStartAction::PlayListenStart {
                abort_existing_playback,
            })
        } else if let Some(floor) = self.cooldown_floor.take() {
            self.phase = EchoGuardPhase::ListenEarconGuard { guard_until: floor };
            Ok(CaptureStartAction::EnterListeningImmediately {
                abort_existing_playback,
            })
        } else {
            self.phase = EchoGuardPhase::Listening;
            Ok(CaptureStartAction::EnterListeningImmediately {
                abort_existing_playback,
            })
        }
    }

    /// Advance time-driven and sink-driven Echo Guard transitions.
    pub fn advance(&mut self, now: Instant, sink_empty: bool) -> GuardTransition {
        match self.phase {
            EchoGuardPhase::ListenEarconPlaying if sink_empty => {
                let guard_ms = if self.aec_enabled {
                    0
                } else {
                    u64::from(self.earcon_guard_ms)
                };
                let mut guard_until = now + Duration::from_millis(guard_ms);
                if let Some(floor) = self.cooldown_floor.take() {
                    guard_until = guard_until.max(floor);
                }
                if guard_until <= now {
                    self.phase = EchoGuardPhase::Listening;
                    GuardTransition::EnteredListening
                } else {
                    self.phase = EchoGuardPhase::ListenEarconGuard { guard_until };
                    GuardTransition::None
                }
            }
            EchoGuardPhase::ListenEarconGuard { guard_until } if now >= guard_until => {
                self.phase = EchoGuardPhase::Listening;
                GuardTransition::EnteredListening
            }
            EchoGuardPhase::PlayingTts if sink_empty => {
                let until = now + Duration::from_millis(u64::from(self.echo_cooldown_ms));
                self.phase = EchoGuardPhase::Cooldown { until };
                GuardTransition::EnteredCooldown
            }
            EchoGuardPhase::Cooldown { until } if now >= until => {
                self.phase = EchoGuardPhase::Idle;
                GuardTransition::CooldownElapsed
            }
            _ => GuardTransition::None,
        }
    }

    /// Mark transition from `Listening` to `CapturingSpeech` once VAD detects speech onset.
    pub fn mark_capturing_speech(&mut self) {
        if matches!(self.phase, EchoGuardPhase::Listening) {
            self.phase = EchoGuardPhase::CapturingSpeech;
        }
    }

    /// Mark transition to `Processing` (used when speech capture hands off to inference).
    pub fn mark_processing(&mut self) {
        if matches!(
            self.phase,
            EchoGuardPhase::Listening | EchoGuardPhase::CapturingSpeech
        ) {
            self.phase = EchoGuardPhase::Processing;
        }
    }

    /// Return from `Listening` / `CapturingSpeech` / `Processing` to `Idle`.
    pub fn finish_capture(&mut self) {
        if matches!(
            self.phase,
            EchoGuardPhase::Listening
                | EchoGuardPhase::CapturingSpeech
                | EchoGuardPhase::Processing
        ) {
            self.phase = EchoGuardPhase::Idle;
        }
    }

    /// Cancel any active capture or pending pre-listen earcon sequence, returning to `Idle`.
    pub fn cancel_capture(&mut self) -> bool {
        match self.phase {
            EchoGuardPhase::ListenEarconPlaying
            | EchoGuardPhase::ListenEarconGuard { .. }
            | EchoGuardPhase::Listening
            | EchoGuardPhase::CapturingSpeech => {
                self.cooldown_floor = None;
                self.phase = EchoGuardPhase::Idle;
                true
            }
            _ => false,
        }
    }

    /// Transition into `PlayingTts`, returning `true` if an active capture was interrupted.
    pub fn begin_playback(&mut self) -> bool {
        let interrupted_capture = self.cancel_capture();
        self.cooldown_floor = None;
        self.phase = EchoGuardPhase::PlayingTts;
        interrupted_capture
    }

    /// Execute `stop_playback` rules across all Echo Guard states.
    pub fn stop_playback(&mut self, now: Instant) -> StopPlaybackAction {
        match self.phase {
            EchoGuardPhase::Idle | EchoGuardPhase::Processing | EchoGuardPhase::Cooldown { .. } => {
                StopPlaybackAction::NoOp
            }
            EchoGuardPhase::Listening | EchoGuardPhase::CapturingSpeech => {
                self.phase = EchoGuardPhase::Idle;
                StopPlaybackAction::CancelCapture
            }
            EchoGuardPhase::ListenEarconPlaying | EchoGuardPhase::ListenEarconGuard { .. } => {
                self.cooldown_floor = None;
                self.phase = EchoGuardPhase::Idle;
                StopPlaybackAction::AbortListenEarconAndCancelCapture
            }
            EchoGuardPhase::PlayingTts => {
                let until = now + Duration::from_millis(u64::from(self.echo_cooldown_ms));
                self.phase = EchoGuardPhase::Cooldown { until };
                StopPlaybackAction::AbortPlayback
            }
        }
    }
}

/// Compute bounded ALSA reopen backoff in milliseconds (`500 ms, 1s, 2s`, then exhausted).
#[must_use]
pub fn reopen_backoff_ms(failed_attempts: usize) -> Option<u64> {
    REOPEN_BACKOFF_MS.get(failed_attempts).copied()
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum WatchdogAction {
    Healthy,
    WaitingBackoff,
    AttemptReopen,
}

#[derive(Debug, Clone)]
struct WatchdogState {
    active: bool,
    failed_attempts: usize,
    next_retry_at: Option<Instant>,
}

impl WatchdogState {
    fn new(input_opened_at_boot: bool) -> Self {
        Self {
            active: input_opened_at_boot,
            failed_attempts: 0,
            next_retry_at: None,
        }
    }

    fn poll(
        &self,
        now: Instant,
        elapsed_since_callback_ms: u64,
        stream_errored: bool,
    ) -> WatchdogAction {
        if !self.active {
            return WatchdogAction::Healthy;
        }
        if let Some(retry_at) = self.next_retry_at {
            return if now < retry_at {
                WatchdogAction::WaitingBackoff
            } else {
                WatchdogAction::AttemptReopen
            };
        }
        if stream_errored || elapsed_since_callback_ms > WATCHDOG_TIMEOUT_MS {
            WatchdogAction::AttemptReopen
        } else {
            WatchdogAction::Healthy
        }
    }

    fn record_success(&mut self) {
        self.failed_attempts = 0;
        self.next_retry_at = None;
    }

    /// Record a failed reopen; returns `true` while bounded retries remain, `false` when exhausted.
    fn record_failure(&mut self, now: Instant) -> bool {
        if let Some(delay_ms) = reopen_backoff_ms(self.failed_attempts) {
            self.failed_attempts += 1;
            self.next_retry_at = Some(now + Duration::from_millis(delay_ms));
            true
        } else {
            self.active = false;
            self.next_retry_at = None;
            false
        }
    }
}

enum ActorCommand {
    StartCapture {
        mode: CaptureMode,
        reply: oneshot::Sender<Result<CapturedAudio, AudioError>>,
    },
    CancelCapture {
        reply: oneshot::Sender<Result<(), AudioError>>,
    },
    PlayPcmStream {
        pcm_rx: mpsc::Receiver<PcmChunk>,
        cache_key: Option<String>,
        reply: oneshot::Sender<Result<(), AudioError>>,
    },
    PlayPcmI16 {
        samples: Vec<i16>,
        sample_rate_hz: u32,
        cache_key: Option<String>,
        reply: oneshot::Sender<Result<(), AudioError>>,
    },
    PlayEarcon {
        kind: EarconKind,
        reply: oneshot::Sender<Result<(), AudioError>>,
    },
    ReplayLast {
        reply: oneshot::Sender<Result<(), AudioError>>,
    },
    RecordVoiceTurnUser {
        turn_id: u64,
        pcm_16k: Arc<[f32]>,
        reply: oneshot::Sender<Result<(), AudioError>>,
    },
    AttachVoiceTurnAi {
        turn_id: u64,
        pcm_i16: Arc<[i16]>,
        sample_rate: u32,
        synth_speed: f32,
        reply: oneshot::Sender<Result<(), AudioError>>,
    },
    CommitCompletedTts {
        cache_key: u64,
        pcm_i16: Arc<[i16]>,
        sample_rate: u32,
        synth_speed: f32,
        reply: oneshot::Sender<Result<(), AudioError>>,
    },
    PlayCachedTtsByKey {
        cache_key: u64,
        speed: f32,
        reply: oneshot::Sender<Result<bool, AudioError>>,
    },
    AttachCachedTtsToVoiceTurn {
        turn_id: u64,
        cache_key: u64,
        reply: oneshot::Sender<Result<bool, AudioError>>,
    },
    PlayLastCompletedTts {
        speed: f32,
        reply: oneshot::Sender<Result<(), AudioError>>,
    },
    PlayVoiceTurn {
        turn_id: Option<u64>,
        mode: VoiceReplayMode,
        speed: f32,
        reply: oneshot::Sender<Result<(), AudioError>>,
    },
    StopPlayback {
        reply: oneshot::Sender<Result<(), AudioError>>,
    },
    SetVolume {
        volume: f32,
        reply: oneshot::Sender<Result<(), AudioError>>,
    },
    SetMuted {
        muted: bool,
        reply: oneshot::Sender<Result<(), AudioError>>,
    },
    Shutdown,
}

struct AudioHandleInner {
    cmd_tx: mpsc::Sender<ActorCommand>,
    actor_thread: thread::Thread,
    rms_rx: watch::Receiver<f32>,
    state_rx: watch::Receiver<AudioState>,
    health_rx: watch::Receiver<AudioHealthStatus>,
    thread: Mutex<Option<JoinHandle<()>>>,
}

impl Drop for AudioHandleInner {
    fn drop(&mut self) {
        let _ = self.cmd_tx.try_send(ActorCommand::Shutdown);
        self.actor_thread.unpark();
        if let Ok(mut guard) = self.thread.lock() {
            if let Some(handle) = guard.take() {
                let _ = handle.join();
            }
        }
    }
}

/// Async handle to the dedicated single-owner `"kural-audio-actor"` OS thread.
#[derive(Clone)]
pub struct AudioHandle {
    inner: Arc<AudioHandleInner>,
}

impl AudioHandle {
    /// Spawn the dedicated `"kural-audio-actor"` OS thread owning ALSA capture and `rodio` playback.
    ///
    /// If hardware input or output devices fail to open (such as in headless CI), the actor starts
    /// in degraded mode with the corresponding `AudioHealthStatus` flag set to `false` instead of
    /// panicking.
    pub fn spawn(
        config: &AudioConfig,
        vad: Option<Box<dyn VadBackend>>,
        max_cache_bytes: usize,
    ) -> Self {
        Self::spawn_with_backend(config, vad, max_cache_bytes, BackendMode::Hardware)
    }

    fn spawn_with_backend(
        config: &AudioConfig,
        vad: Option<Box<dyn VadBackend>>,
        max_cache_bytes: usize,
        backend_mode: BackendMode,
    ) -> Self {
        let (cmd_tx, cmd_rx) = mpsc::channel(COMMAND_QUEUE_CAP);
        let (rms_tx, rms_rx) = watch::channel(0.0_f32);
        let (state_tx, state_rx) = watch::channel(AudioState::Idle);
        let initial_health = AudioHealthStatus {
            input_ok: false,
            output_ok: false,
            sample_rate_hz: TARGET_SAMPLE_RATE_HZ,
        };
        let (health_tx, health_rx) = watch::channel(initial_health);
        let (ready_tx, ready_rx) = std::sync::mpsc::sync_channel::<()>(1);

        let cfg = config.clone();
        let vad_impl = vad.unwrap_or_else(|| Box::new(EnergyZcrVad::default()));

        let join_handle = thread::Builder::new()
            .name("kural-audio-actor".to_string())
            .spawn(move || {
                let mut actor = AudioActor::boot(
                    cfg,
                    vad_impl,
                    max_cache_bytes,
                    cmd_rx,
                    rms_tx,
                    state_tx,
                    health_tx,
                    backend_mode,
                );
                let _ = ready_tx.send(());
                actor.run_loop();
            })
            .expect("failed to spawn kural-audio-actor thread");

        let _ = ready_rx.recv();
        let actor_thread = join_handle.thread().clone();

        Self {
            inner: Arc::new(AudioHandleInner {
                cmd_tx,
                actor_thread,
                rms_rx,
                state_rx,
                health_rx,
                thread: Mutex::new(Some(join_handle)),
            }),
        }
    }

    /// Start a capture session (`FixedAgeSample` or `VadCommand`) guarded by `ListenStart` and
    /// `earcon_guard_ms`.
    pub async fn start_capture(&self, mode: CaptureMode) -> Result<CapturedAudio, AudioError> {
        self.call(|reply| ActorCommand::StartCapture { mode, reply })
            .await
    }

    /// Cancel an active capture session, returning `AudioState::Idle`.
    pub async fn cancel_capture(&self) -> Result<(), AudioError> {
        self.call(|reply| ActorCommand::CancelCapture { reply })
            .await
    }

    /// Stream synthesized `i16` TTS chunks to the speaker and cache upon normal completion.
    pub async fn play_pcm_stream(
        &self,
        pcm_rx: mpsc::Receiver<PcmChunk>,
        cache_key: Option<String>,
    ) -> Result<(), AudioError> {
        self.call(|reply| ActorCommand::PlayPcmStream {
            pcm_rx,
            cache_key,
            reply,
        })
        .await
    }

    /// Play a complete in-memory `i16` mono clip and cache upon normal completion.
    pub async fn play_pcm_i16(
        &self,
        samples: Vec<i16>,
        sample_rate_hz: u32,
        cache_key: Option<String>,
    ) -> Result<(), AudioError> {
        self.call(|reply| ActorCommand::PlayPcmI16 {
            samples,
            sample_rate_hz,
            cache_key,
            reply,
        })
        .await
    }

    /// Play a synthesized system earcon (`ListenStart`, `CmdRecognized`, or `RetryTone`).
    pub async fn play_earcon(&self, kind: EarconKind) -> Result<(), AudioError> {
        self.call(|reply| ActorCommand::PlayEarcon { kind, reply })
            .await
    }

    /// Replay the last completed TTS clip from the byte-bounded `TtsPcmCache`.
    pub async fn replay_last(&self) -> Result<(), AudioError> {
        self.call(|reply| ActorCommand::ReplayLast { reply }).await
    }

    /// Record a captured `16 kHz` user utterance into the RAM-bounded `VoiceTurnAudioStore`.
    pub async fn record_voice_turn_user(
        &self,
        turn_id: u64,
        pcm_16k: Arc<[f32]>,
    ) -> Result<(), AudioError> {
        self.call(|reply| ActorCommand::RecordVoiceTurnUser {
            turn_id,
            pcm_16k,
            reply,
        })
        .await
    }

    /// Attach synthesized AI PCM to an already-recorded `turn_id` (`synth_speed = 1.0`).
    pub async fn attach_voice_turn_ai(
        &self,
        turn_id: u64,
        pcm_i16: Arc<[i16]>,
        sample_rate: u32,
    ) -> Result<(), AudioError> {
        self.attach_voice_turn_ai_with_speed(turn_id, pcm_i16, sample_rate, 1.0)
            .await
    }

    /// Attach synthesized AI PCM alongside its synthesis speed for natural pitch-accurate replay.
    pub async fn attach_voice_turn_ai_with_speed(
        &self,
        turn_id: u64,
        pcm_i16: Arc<[i16]>,
        sample_rate: u32,
        synth_speed: f32,
    ) -> Result<(), AudioError> {
        self.call(|reply| ActorCommand::AttachVoiceTurnAi {
            turn_id,
            pcm_i16,
            sample_rate,
            synth_speed,
            reply,
        })
        .await
    }

    /// Commit completed TTS audio to `TtsPcmCache` under a deterministic numeric key.
    pub async fn commit_completed_tts(
        &self,
        cache_key: u64,
        pcm_i16: Arc<[i16]>,
        sample_rate: u32,
    ) -> Result<(), AudioError> {
        self.commit_completed_tts_with_speed(cache_key, pcm_i16, sample_rate, 1.0)
            .await
    }

    /// Commit completed TTS audio under a numeric key with its exact synthesis speed.
    pub async fn commit_completed_tts_with_speed(
        &self,
        cache_key: u64,
        pcm_i16: Arc<[i16]>,
        sample_rate: u32,
        synth_speed: f32,
    ) -> Result<(), AudioError> {
        self.call(|reply| ActorCommand::CommitCompletedTts {
            cache_key,
            pcm_i16,
            sample_rate,
            synth_speed,
            reply,
        })
        .await
    }

    /// Play cached TTS audio by deterministic numeric key if present in RAM (`Ok(true)` hit, `Ok(false)` miss).
    pub async fn play_cached_tts_by_key(
        &self,
        cache_key: u64,
        speed: f32,
    ) -> Result<bool, AudioError> {
        self.call(|reply| ActorCommand::PlayCachedTtsByKey {
            cache_key,
            speed,
            reply,
        })
        .await
    }

    /// Link a cached numeric `cache_key` TTS clip to `turn_id` in `VoiceTurnAudioStore` (`0 ms`).
    pub async fn attach_cached_tts_to_voice_turn(
        &self,
        turn_id: u64,
        cache_key: u64,
    ) -> Result<bool, AudioError> {
        self.call(|reply| ActorCommand::AttachCachedTtsToVoiceTurn {
            turn_id,
            cache_key,
            reply,
        })
        .await
    }

    /// Replay the last completed TTS clip adjusted cleanly to `speed`.
    pub async fn play_last_completed_tts(&self, speed: f32) -> Result<(), AudioError> {
        self.call(|reply| ActorCommand::PlayLastCompletedTts { speed, reply })
            .await
    }

    /// Replay a stored Voice-to-Voice turn (`UserVoiceOnly`, `AiReplyOnly`, or `FullExchange`) inside
    /// a single uninterrupted Echo-Guard cycle.
    pub async fn play_voice_turn(
        &self,
        turn_id: Option<u64>,
        mode: VoiceReplayMode,
        speed: f32,
    ) -> Result<(), AudioError> {
        self.call(|reply| ActorCommand::PlayVoiceTurn {
            turn_id,
            mode,
            speed,
            reply,
        })
        .await
    }

    /// Immediately pause and clear playback (`< 20 ms`), aborting in-flight TTS without caching.
    ///
    /// No-op when `Idle`; cancels capture if `Listening` or `CapturingSpeech`.
    pub async fn stop_playback(&self) -> Result<(), AudioError> {
        self.call(|reply| ActorCommand::StopPlayback { reply })
            .await
    }

    /// Immediately enqueue `stop_playback` in FIFO order without spawning a racy task.
    pub fn stop_playback_nowait(&self) {
        self.enqueue_nowait(|reply| ActorCommand::StopPlayback { reply });
    }

    /// Immediately enqueue `cancel_capture` in FIFO order without spawning a racy task.
    pub fn cancel_capture_nowait(&self) {
        self.enqueue_nowait(|reply| ActorCommand::CancelCapture { reply });
    }

    /// Immediately enqueue an earcon playback in FIFO order.
    pub fn play_earcon_nowait(&self, kind: EarconKind) {
        self.enqueue_nowait(move |reply| ActorCommand::PlayEarcon { kind, reply });
    }

    /// Set normalized playback volume in `[0.0, 1.0]`.
    pub async fn set_volume(&self, volume: f32) -> Result<(), AudioError> {
        self.call(|reply| ActorCommand::SetVolume { volume, reply })
            .await
    }

    /// Enable or disable output mute without altering stored master volume.
    pub async fn set_muted(&self, muted: bool) -> Result<(), AudioError> {
        self.call(|reply| ActorCommand::SetMuted { muted, reply })
            .await
    }

    /// Subscribe to real-time microphone RMS updates (clamped to `0.0` outside active capture).
    #[must_use]
    pub fn subscribe_rms(&self) -> watch::Receiver<f32> {
        self.inner.rms_rx.clone()
    }

    /// Subscribe to the current half-duplex [`AudioState`].
    #[must_use]
    pub fn subscribe_state(&self) -> watch::Receiver<AudioState> {
        self.inner.state_rx.clone()
    }

    /// Return the latest hardware health snapshot.
    #[must_use]
    pub fn health(&self) -> AudioHealthStatus {
        *self.inner.health_rx.borrow()
    }

    /// Return `true` only when both audio input and audio output hardware are healthy.
    #[must_use]
    pub fn is_healthy(&self) -> bool {
        let h = self.health();
        h.input_ok && h.output_ok
    }

    /// Explicitly shut down and join the `"kural-audio-actor"` OS thread.
    pub fn shutdown(&self) {
        let _ = self.inner.cmd_tx.try_send(ActorCommand::Shutdown);
        self.inner.actor_thread.unpark();
        if let Ok(mut guard) = self.inner.thread.lock() {
            if let Some(handle) = guard.take() {
                let _ = handle.join();
            }
        }
    }

    fn enqueue_nowait<T: Send + 'static>(
        &self,
        build: impl FnOnce(oneshot::Sender<Result<T, AudioError>>) -> ActorCommand + Send + 'static,
    ) {
        let (tx, _rx) = oneshot::channel();
        match self.inner.cmd_tx.try_send(build(tx)) {
            Ok(()) => self.inner.actor_thread.unpark(),
            Err(mpsc::error::TrySendError::Full(cmd)) => {
                let tx_chan = self.inner.cmd_tx.clone();
                let thread = self.inner.actor_thread.clone();
                if let Ok(handle) = tokio::runtime::Handle::try_current() {
                    handle.spawn(async move {
                        if tx_chan.send(cmd).await.is_ok() {
                            thread.unpark();
                        }
                    });
                }
            }
            Err(mpsc::error::TrySendError::Closed(_)) => {}
        }
    }

    async fn call<T>(
        &self,
        build: impl FnOnce(oneshot::Sender<Result<T, AudioError>>) -> ActorCommand,
    ) -> Result<T, AudioError> {
        let (tx, rx) = oneshot::channel();
        self.inner
            .cmd_tx
            .send(build(tx))
            .await
            .map_err(|_| AudioError::StreamError("audio actor has shut down".to_string()))?;
        self.inner.actor_thread.unpark();
        rx.await
            .map_err(|_| AudioError::StreamError("audio actor dropped reply channel".to_string()))?
    }
}

enum BackendMode {
    Hardware,
    #[cfg(test)]
    Simulated {
        input_cons: Option<HeapCons<f32>>,
        input_rate_hz: u32,
        input_channels: u16,
        output: Option<SimulatedOutputHandle>,
    },
}

#[cfg(test)]
#[derive(Clone, Default)]
struct SimulatedOutputHandle {
    state: Arc<Mutex<SimulatedOutputState>>,
}

#[cfg(test)]
#[derive(Default)]
struct SimulatedOutputState {
    play_until: Option<Instant>,
    paused: bool,
    cleared_count: usize,
    appended_samples: Vec<Vec<f32>>,
    appended_rates: Vec<u32>,
    appended_speeds: Vec<f32>,
    volume: f32,
    force_empty: bool,
}

#[cfg(test)]
impl SimulatedOutputHandle {
    fn new() -> Self {
        Self {
            state: Arc::new(Mutex::new(SimulatedOutputState {
                volume: 1.0,
                ..Default::default()
            })),
        }
    }

    fn is_empty(&self, now: Instant) -> bool {
        let st = self.state.lock().expect("sim lock");
        if st.force_empty || st.paused {
            return st.play_until.is_none();
        }
        st.play_until.is_none_or(|until| now >= until)
    }

    fn append(&self, samples: Vec<f32>, rate_hz: u32, speed: f32, now: Instant) {
        let mut st = self.state.lock().expect("sim lock");
        let safe_spd = f64::from(sanitize_playback_speed(speed));
        let dur =
            Duration::from_secs_f64((samples.len() as f64 / f64::from(rate_hz.max(1))) / safe_spd);
        let base = st.play_until.filter(|&u| u > now).unwrap_or(now);
        st.play_until = Some(base + dur);
        st.paused = false;
        st.force_empty = false;
        st.appended_samples.push(samples);
        st.appended_rates.push(rate_hz);
        st.appended_speeds.push(speed);
    }

    fn stop_and_clear(&self) {
        let mut st = self.state.lock().expect("sim lock");
        st.paused = true;
        st.play_until = None;
        st.force_empty = true;
        st.cleared_count += 1;
    }

    fn set_volume(&self, v: f32) {
        let mut st = self.state.lock().expect("sim lock");
        st.volume = v;
    }

    fn cleared_count(&self) -> usize {
        self.state.lock().expect("sim lock").cleared_count
    }

    fn last_volume(&self) -> f32 {
        self.state.lock().expect("sim lock").volume
    }

    fn appended_count(&self) -> usize {
        self.state.lock().expect("sim lock").appended_samples.len()
    }

    fn appended_meta(&self) -> Vec<(usize, u32, f32)> {
        let st = self.state.lock().expect("sim lock");
        st.appended_samples
            .iter()
            .zip(st.appended_rates.iter())
            .zip(st.appended_speeds.iter())
            .map(|((s, &r), &spd)| (s.len(), r, spd))
            .collect()
    }
}

enum OutputSink {
    Unavailable,
    Hardware {
        _sink: rodio::MixerDeviceSink,
        player: rodio::Player,
        errored: Arc<AtomicBool>,
    },
    #[cfg(test)]
    Simulated(SimulatedOutputHandle),
}

impl OutputSink {
    fn is_available(&self) -> bool {
        !matches!(self, Self::Unavailable)
    }

    fn append_f32(
        &mut self,
        samples: Vec<f32>,
        sample_rate_hz: u32,
        now: Instant,
    ) -> Result<(), AudioError> {
        self.append_f32_with_speed(samples, sample_rate_hz, 1.0, now)
    }

    fn append_f32_with_speed(
        &mut self,
        samples: Vec<f32>,
        sample_rate_hz: u32,
        speed: f32,
        _now: Instant,
    ) -> Result<(), AudioError> {
        if samples.is_empty() {
            return Ok(());
        }
        let Some(sr) = NonZero::new(sample_rate_hz) else {
            return Err(AudioError::StreamError(
                "output sample rate must be > 0 Hz".to_string(),
            ));
        };
        let safe_speed = sanitize_playback_speed(speed);
        match self {
            Self::Unavailable => Err(AudioError::DeviceUnavailable(
                "audio output device unavailable".to_string(),
            )),
            Self::Hardware { player, .. } => {
                let ch = NonZero::new(1).expect("1 is non-zero");
                let buf = rodio::buffer::SamplesBuffer::new(ch, sr, samples);
                if (safe_speed - 1.0).abs() > 0.01 {
                    player.append(buf.speed(safe_speed));
                } else {
                    player.append(buf);
                }
                player.play();
                Ok(())
            }
            #[cfg(test)]
            Self::Simulated(sim) => {
                sim.append(samples, sample_rate_hz, safe_speed, _now);
                Ok(())
            }
        }
    }

    fn is_empty(&self, _now: Instant) -> bool {
        match self {
            Self::Unavailable => true,
            Self::Hardware {
                player, errored, ..
            } => errored.load(Ordering::Acquire) || player.empty(),
            #[cfg(test)]
            Self::Simulated(sim) => sim.is_empty(_now),
        }
    }

    fn stop_and_clear(&mut self) {
        match self {
            Self::Unavailable => {}
            Self::Hardware {
                _sink,
                player,
                errored,
            } => {
                player.pause();
                if errored.load(Ordering::Acquire) {
                    player.stop();
                    let replacement = rodio::Player::connect_new(_sink.mixer());
                    replacement.set_volume(player.volume());
                    *player = replacement;
                } else if !player.empty() {
                    player.clear();
                }
            }
            #[cfg(test)]
            Self::Simulated(sim) => sim.stop_and_clear(),
        }
    }

    fn set_volume(&mut self, effective_volume: f32) {
        match self {
            Self::Unavailable => {}
            Self::Hardware { player, .. } => {
                player.set_volume(effective_volume);
            }
            #[cfg(test)]
            Self::Simulated(sim) => sim.set_volume(effective_volume),
        }
    }

    fn has_errored(&self) -> bool {
        match self {
            Self::Hardware { errored, .. } => errored.load(Ordering::Acquire),
            Self::Unavailable => false,
            #[cfg(test)]
            Self::Simulated(_) => false,
        }
    }
}

enum PlaybackReply {
    Unit(oneshot::Sender<Result<(), AudioError>>),
    Hit(oneshot::Sender<Result<bool, AudioError>>),
}

impl PlaybackReply {
    fn send_ok(self) {
        match self {
            Self::Unit(tx) => {
                let _ = tx.send(Ok(()));
            }
            Self::Hit(tx) => {
                let _ = tx.send(Ok(true));
            }
        }
    }

    fn send_err(self, err: AudioError) {
        match self {
            Self::Unit(tx) => {
                let _ = tx.send(Err(err));
            }
            Self::Hit(tx) => {
                let _ = tx.send(Err(err));
            }
        }
    }
}

struct InputCaptureSlot {
    _stream: Option<cpal::Stream>,
    ring_cons: HeapCons<f32>,
    channels: usize,
    resampler: Resampler16k,
    last_callback_ms: Arc<AtomicU64>,
    stream_error_flag: Arc<AtomicBool>,
    pending_interleaved: Vec<f32>,
}

struct ActiveCaptureJob {
    mode: CaptureMode,
    framer: Option<UtteranceFramer>,
    reply: oneshot::Sender<Result<CapturedAudio, AudioError>>,
    pending_16k: Vec<f32>,
}

struct ActivePlaybackJob {
    pcm_rx: Option<mpsc::Receiver<PcmChunk>>,
    stream_finished: bool,
    cache_key: Option<String>,
    cache_accum: Vec<i16>,
    sample_rate_hz: Option<u32>,
    cache_on_complete: bool,
    played_any_audio: bool,
    reply: Option<PlaybackReply>,
}

struct AudioActor {
    config: AudioConfig,
    origin: Instant,
    input: Option<InputCaptureSlot>,
    output: OutputSink,
    conditioner: AudioConditioner,
    vad: Box<dyn VadBackend>,
    earcons: EarconBank,
    cache: TtsPcmCache,
    voice_turns: VoiceTurnAudioStore,
    echo_guard: EchoGuardState,
    watchdog: WatchdogState,
    health: AudioHealthStatus,
    master_volume: f32,
    muted: bool,
    active_capture: Option<ActiveCaptureJob>,
    active_playback: Option<ActivePlaybackJob>,
    cmd_rx: mpsc::Receiver<ActorCommand>,
    rms_tx: watch::Sender<f32>,
    state_tx: watch::Sender<AudioState>,
    health_tx: watch::Sender<AudioHealthStatus>,
    scratch_pop: Vec<f32>,
}

impl AudioActor {
    #[allow(clippy::too_many_arguments)]
    fn boot(
        config: AudioConfig,
        vad: Box<dyn VadBackend>,
        max_cache_bytes: usize,
        cmd_rx: mpsc::Receiver<ActorCommand>,
        rms_tx: watch::Sender<f32>,
        state_tx: watch::Sender<AudioState>,
        health_tx: watch::Sender<AudioHealthStatus>,
        backend_mode: BackendMode,
    ) -> Self {
        let origin = Instant::now();
        let master_volume = if config.master_volume.is_finite() {
            config.master_volume.clamp(0.0, 1.0)
        } else {
            0.85
        };

        let (input, output, watchdog_enabled, sample_rate_hz) = match backend_mode {
            BackendMode::Hardware => {
                let input_slot = open_hardware_input(&config.input_device, origin).ok();
                let sr = input_slot
                    .as_ref()
                    .map_or(TARGET_SAMPLE_RATE_HZ, |s| s.resampler.src_rate_hz());
                let out_sink = open_hardware_output(&config.output_device, master_volume);
                let wd = input_slot.is_some();
                (input_slot, out_sink, wd, sr)
            }
            #[cfg(test)]
            BackendMode::Simulated {
                input_cons,
                input_rate_hz,
                input_channels,
                output,
            } => {
                let input_slot = input_cons.and_then(|ring_cons| {
                    Resampler16k::new(input_rate_hz, input_channels)
                        .ok()
                        .map(|resampler| InputCaptureSlot {
                            _stream: None,
                            ring_cons,
                            channels: usize::from(input_channels),
                            resampler,
                            last_callback_ms: Arc::new(AtomicU64::new(1)),
                            stream_error_flag: Arc::new(AtomicBool::new(false)),
                            pending_interleaved: Vec::new(),
                        })
                });
                let out_sink = match output {
                    Some(sim) => {
                        sim.set_volume(master_volume);
                        OutputSink::Simulated(sim)
                    }
                    None => OutputSink::Unavailable,
                };
                (input_slot, out_sink, false, input_rate_hz)
            }
        };

        let health = AudioHealthStatus {
            input_ok: input.is_some(),
            output_ok: output.is_available(),
            sample_rate_hz,
        };
        let _ = health_tx.send(health);

        Self {
            echo_guard: EchoGuardState::new(
                config.earcon_guard_ms,
                config.echo_cooldown_ms,
                config.aec_enabled,
            ),
            config,
            origin,
            input,
            output,
            conditioner: AudioConditioner::new(TARGET_SAMPLE_RATE_HZ),
            vad,
            earcons: EarconBank::new(),
            cache: TtsPcmCache::new(DEFAULT_TTS_CACHE_MAX_ENTRIES, max_cache_bytes),
            voice_turns: VoiceTurnAudioStore::default(),
            watchdog: WatchdogState::new(watchdog_enabled),
            health,
            master_volume,
            muted: false,
            active_capture: None,
            active_playback: None,
            cmd_rx,
            rms_tx,
            state_tx,
            health_tx,
            scratch_pop: vec![0.0_f32; 4096],
        }
    }

    fn run_loop(&mut self) {
        let tick_dur = Duration::from_millis(ACTOR_TICK_MS);
        loop {
            let tick_start = Instant::now();
            if !self.drain_commands(tick_start) {
                self.fail_pending_on_shutdown();
                break;
            }

            let now = Instant::now();
            self.poll_watchdog(now);
            self.process_capture_stream();
            self.poll_active_playback(now);
            self.advance_echo_guard(now);
            self.publish_state();

            let elapsed = tick_start.elapsed();
            if elapsed < tick_dur {
                thread::park_timeout(tick_dur - elapsed);
            }
        }
    }

    fn drain_commands(&mut self, now: Instant) -> bool {
        while let Ok(cmd) = self.cmd_rx.try_recv() {
            match cmd {
                ActorCommand::Shutdown => return false,
                ActorCommand::StartCapture { mode, reply } => {
                    self.handle_start_capture(mode, reply, now);
                }
                ActorCommand::CancelCapture { reply } => {
                    self.handle_cancel_capture();
                    let _ = reply.send(Ok(()));
                }
                ActorCommand::PlayPcmStream {
                    pcm_rx,
                    cache_key,
                    reply,
                } => {
                    self.handle_play_pcm_stream(pcm_rx, cache_key, reply, now);
                }
                ActorCommand::PlayPcmI16 {
                    samples,
                    sample_rate_hz,
                    cache_key,
                    reply,
                } => {
                    self.handle_play_pcm_i16(samples, sample_rate_hz, cache_key, reply, now);
                }
                ActorCommand::PlayEarcon { kind, reply } => {
                    self.handle_play_earcon(kind, reply, now);
                }
                ActorCommand::ReplayLast { reply } => {
                    self.handle_replay_last(1.0, true, reply, now);
                }
                ActorCommand::RecordVoiceTurnUser {
                    turn_id,
                    pcm_16k,
                    reply,
                } => {
                    self.voice_turns.push_user_turn(turn_id, &pcm_16k);
                    let _ = reply.send(Ok(()));
                }
                ActorCommand::AttachVoiceTurnAi {
                    turn_id,
                    pcm_i16,
                    sample_rate,
                    synth_speed,
                    reply,
                } => {
                    self.voice_turns.attach_ai_audio_with_speed(
                        turn_id,
                        pcm_i16,
                        sample_rate,
                        synth_speed,
                    );
                    let _ = reply.send(Ok(()));
                }
                ActorCommand::CommitCompletedTts {
                    cache_key,
                    pcm_i16,
                    sample_rate,
                    synth_speed,
                    reply,
                } => {
                    self.cache.insert_completed_u64(
                        cache_key,
                        pcm_i16.to_vec(),
                        sample_rate,
                        synth_speed,
                    );
                    let _ = reply.send(Ok(()));
                }
                ActorCommand::PlayCachedTtsByKey {
                    cache_key,
                    speed,
                    reply,
                } => {
                    self.handle_play_cached_tts_by_key(cache_key, speed, reply, now);
                }
                ActorCommand::AttachCachedTtsToVoiceTurn {
                    turn_id,
                    cache_key,
                    reply,
                } => {
                    let attached = if let Some((clip, synth_speed)) = self
                        .cache
                        .get_cached_u64(cache_key)
                        .map(|(c, s)| (c.clone(), s))
                    {
                        self.voice_turns.attach_ai_audio_with_speed(
                            turn_id,
                            Arc::from(clip.samples),
                            clip.sample_rate_hz,
                            synth_speed,
                        )
                    } else {
                        false
                    };
                    let _ = reply.send(Ok(attached));
                }
                ActorCommand::PlayLastCompletedTts { speed, reply } => {
                    self.handle_replay_last(speed, false, reply, now);
                }
                ActorCommand::PlayVoiceTurn {
                    turn_id,
                    mode,
                    speed,
                    reply,
                } => {
                    self.handle_play_voice_turn(turn_id, mode, speed, reply, now);
                }
                ActorCommand::StopPlayback { reply } => {
                    self.handle_stop_playback(now);
                    let _ = reply.send(Ok(()));
                }
                ActorCommand::SetVolume { volume, reply } => {
                    let res = self.handle_set_volume(volume);
                    let _ = reply.send(res);
                }
                ActorCommand::SetMuted { muted, reply } => {
                    self.muted = muted;
                    self.output.set_volume(self.effective_volume());
                    let _ = reply.send(Ok(()));
                }
            }
        }
        true
    }

    fn effective_volume(&self) -> f32 {
        if self.muted { 0.0 } else { self.master_volume }
    }

    fn handle_set_volume(&mut self, volume: f32) -> Result<(), AudioError> {
        if !volume.is_finite() {
            return Err(AudioError::StreamError("volume must be finite".to_string()));
        }
        self.master_volume = volume.clamp(0.0, 1.0);
        self.output.set_volume(self.effective_volume());
        Ok(())
    }

    fn handle_start_capture(
        &mut self,
        mode: CaptureMode,
        reply: oneshot::Sender<Result<CapturedAudio, AudioError>>,
        now: Instant,
    ) {
        if !self.health.input_ok {
            let _ = reply.send(Err(AudioError::DeviceUnavailable(
                "audio input device unavailable".to_string(),
            )));
            return;
        }

        let has_output = self.output.is_available();
        let action = match self.echo_guard.begin_capture(now, has_output) {
            Ok(action) => action,
            Err(err) => {
                let _ = reply.send(Err(err));
                return;
            }
        };

        let abort_existing = match action {
            CaptureStartAction::PlayListenStart {
                abort_existing_playback,
            }
            | CaptureStartAction::EnterListeningImmediately {
                abort_existing_playback,
            } => abort_existing_playback,
        };

        if abort_existing {
            self.abort_active_playback_without_caching();
            self.output.stop_and_clear();
        }

        match action {
            CaptureStartAction::PlayListenStart { .. } => {
                let samples = self.earcons.samples_f32(EarconKind::ListenStart).to_vec();
                if let Err(err) = self.output.append_f32(samples, EARCON_SAMPLE_RATE_HZ, now) {
                    self.echo_guard.cancel_capture();
                    let _ = reply.send(Err(err));
                    return;
                }
                self.active_capture = Some(ActiveCaptureJob {
                    mode,
                    framer: None,
                    reply,
                    pending_16k: Vec::new(),
                });
            }
            CaptureStartAction::EnterListeningImmediately { .. } => {
                let framer = if self.echo_guard.accepts_mic_samples() {
                    self.vad.reset();
                    Some(UtteranceFramer::new(
                        mode,
                        self.config.vad_threshold,
                        self.config.listening_timeout_ms,
                        TRAILING_SILENCE_MS,
                    ))
                } else {
                    None
                };
                self.active_capture = Some(ActiveCaptureJob {
                    mode,
                    framer,
                    reply,
                    pending_16k: Vec::new(),
                });
            }
        }
        self.publish_rms(0.0);
        self.publish_state();
    }

    fn handle_cancel_capture(&mut self) {
        let was_earcon = matches!(self.echo_guard.phase(), EchoGuardPhase::ListenEarconPlaying);
        if self.echo_guard.cancel_capture() {
            if was_earcon {
                self.output.stop_and_clear();
            }
            if let Some(job) = self.active_capture.take() {
                let _ = job.reply.send(Err(AudioError::Cancelled));
            }
            self.publish_rms(0.0);
            self.publish_state();
        }
    }

    fn handle_play_pcm_stream(
        &mut self,
        pcm_rx: mpsc::Receiver<PcmChunk>,
        cache_key: Option<String>,
        reply: oneshot::Sender<Result<(), AudioError>>,
        now: Instant,
    ) {
        if !self.output.is_available() {
            let _ = reply.send(Err(AudioError::DeviceUnavailable(
                "audio output device unavailable".to_string(),
            )));
            return;
        }
        self.prepare_for_new_playback();
        self.active_playback = Some(ActivePlaybackJob {
            pcm_rx: Some(pcm_rx),
            stream_finished: false,
            cache_key,
            cache_accum: Vec::new(),
            sample_rate_hz: None,
            cache_on_complete: true,
            played_any_audio: false,
            reply: Some(PlaybackReply::Unit(reply)),
        });
        self.drain_pcm_stream_chunks(now);
        self.publish_state();
    }

    fn handle_play_pcm_i16(
        &mut self,
        samples: Vec<i16>,
        sample_rate_hz: u32,
        cache_key: Option<String>,
        reply: oneshot::Sender<Result<(), AudioError>>,
        now: Instant,
    ) {
        if !self.output.is_available() {
            let _ = reply.send(Err(AudioError::DeviceUnavailable(
                "audio output device unavailable".to_string(),
            )));
            return;
        }
        if !samples.is_empty() && sample_rate_hz == 0 {
            let _ = reply.send(Err(AudioError::StreamError(
                "PCM sample rate must be > 0 Hz".to_string(),
            )));
            return;
        }

        let (play_samples, play_rate, cache_on_complete) = if samples.is_empty() {
            if let Some(cached) = cache_key.as_deref().and_then(|k| self.cache.get_cached(k)) {
                (cached.samples.clone(), cached.sample_rate_hz, false)
            } else {
                let _ = reply.send(Ok(()));
                return;
            }
        } else {
            (samples, sample_rate_hz, true)
        };
        if play_samples.is_empty() || play_rate == 0 {
            let _ = reply.send(Ok(()));
            return;
        }

        self.prepare_for_new_playback();
        let f32_samples = i16_to_f32_vec(&play_samples);
        if let Err(err) = self.output.append_f32(f32_samples, play_rate, now) {
            let _ = self.echo_guard.stop_playback(now);
            let _ = reply.send(Err(err));
            self.publish_state();
            return;
        }

        self.active_playback = Some(ActivePlaybackJob {
            pcm_rx: None,
            stream_finished: true,
            cache_key,
            cache_accum: if cache_on_complete {
                play_samples
            } else {
                Vec::new()
            },
            sample_rate_hz: Some(play_rate),
            cache_on_complete,
            played_any_audio: true,
            reply: Some(PlaybackReply::Unit(reply)),
        });
        self.publish_state();
    }

    fn handle_play_earcon(
        &mut self,
        kind: EarconKind,
        reply: oneshot::Sender<Result<(), AudioError>>,
        now: Instant,
    ) {
        if !self.output.is_available() {
            let _ = reply.send(Err(AudioError::DeviceUnavailable(
                "audio output device unavailable".to_string(),
            )));
            return;
        }
        self.prepare_for_new_playback();
        let samples = self.earcons.samples_f32(kind).to_vec();
        if let Err(err) = self.output.append_f32(samples, EARCON_SAMPLE_RATE_HZ, now) {
            let _ = self.echo_guard.stop_playback(now);
            let _ = reply.send(Err(err));
            self.publish_state();
            return;
        }
        self.active_playback = Some(ActivePlaybackJob {
            pcm_rx: None,
            stream_finished: true,
            cache_key: None,
            cache_accum: Vec::new(),
            sample_rate_hz: Some(EARCON_SAMPLE_RATE_HZ),
            cache_on_complete: false,
            played_any_audio: true,
            reply: Some(PlaybackReply::Unit(reply)),
        });
        self.publish_state();
    }

    fn handle_replay_last(
        &mut self,
        requested_speed: f32,
        native_rate: bool,
        reply: oneshot::Sender<Result<(), AudioError>>,
        now: Instant,
    ) {
        if !self.output.is_available() {
            let _ = reply.send(Err(AudioError::DeviceUnavailable(
                "audio output device unavailable".to_string(),
            )));
            return;
        }
        let Some((clip, synth_speed)) = self
            .cache
            .last_completed_with_speed()
            .map(|(c, s)| (c.clone(), s))
        else {
            let _ = reply.send(Err(AudioError::StreamError(
                "no completed audio clip available to replay".to_string(),
            )));
            return;
        };
        if clip.samples.is_empty() || clip.sample_rate_hz == 0 {
            let _ = reply.send(Ok(()));
            return;
        }

        let play_speed = if native_rate {
            1.0
        } else {
            relative_playback_speed(requested_speed, synth_speed)
        };
        self.start_buffer_playback(&clip.samples, clip.sample_rate_hz, play_speed, reply, now);
    }

    fn handle_play_cached_tts_by_key(
        &mut self,
        cache_key: u64,
        requested_speed: f32,
        reply: oneshot::Sender<Result<bool, AudioError>>,
        now: Instant,
    ) {
        let Some((clip, synth_speed)) = self
            .cache
            .get_cached_u64(cache_key)
            .map(|(c, s)| (c.clone(), s))
        else {
            let _ = reply.send(Ok(false));
            return;
        };
        if !self.output.is_available() {
            let _ = reply.send(Err(AudioError::DeviceUnavailable(
                "audio output device unavailable".to_string(),
            )));
            return;
        }
        if clip.samples.is_empty() || clip.sample_rate_hz == 0 {
            let _ = reply.send(Ok(false));
            return;
        }

        self.cache.mark_last_completed(clip.clone(), synth_speed);
        let play_speed = relative_playback_speed(requested_speed, synth_speed);
        self.prepare_for_new_playback();
        let f32_samples = i16_to_f32_vec(&clip.samples);
        if let Err(err) =
            self.output
                .append_f32_with_speed(f32_samples, clip.sample_rate_hz, play_speed, now)
        {
            let _ = self.echo_guard.stop_playback(now);
            let _ = reply.send(Err(err));
            self.publish_state();
            return;
        }

        self.active_playback = Some(ActivePlaybackJob {
            pcm_rx: None,
            stream_finished: true,
            cache_key: None,
            cache_accum: Vec::new(),
            sample_rate_hz: Some(clip.sample_rate_hz),
            cache_on_complete: false,
            played_any_audio: true,
            reply: Some(PlaybackReply::Hit(reply)),
        });
        self.publish_state();
    }

    fn start_buffer_playback(
        &mut self,
        samples: &[i16],
        sample_rate_hz: u32,
        playback_speed: f32,
        reply: oneshot::Sender<Result<(), AudioError>>,
        now: Instant,
    ) {
        self.prepare_for_new_playback();
        let f32_samples = i16_to_f32_vec(samples);
        if let Err(err) =
            self.output
                .append_f32_with_speed(f32_samples, sample_rate_hz, playback_speed, now)
        {
            let _ = self.echo_guard.stop_playback(now);
            let _ = reply.send(Err(err));
            self.publish_state();
            return;
        }
        self.active_playback = Some(ActivePlaybackJob {
            pcm_rx: None,
            stream_finished: true,
            cache_key: None,
            cache_accum: Vec::new(),
            sample_rate_hz: Some(sample_rate_hz),
            cache_on_complete: false,
            played_any_audio: true,
            reply: Some(PlaybackReply::Unit(reply)),
        });
        self.publish_state();
    }

    fn handle_play_voice_turn(
        &mut self,
        turn_id: Option<u64>,
        mode: VoiceReplayMode,
        requested_speed: f32,
        reply: oneshot::Sender<Result<(), AudioError>>,
        now: Instant,
    ) {
        if !self.output.is_available() {
            let _ = reply.send(Err(AudioError::DeviceUnavailable(
                "audio output device unavailable".to_string(),
            )));
            return;
        }
        let Some(turn) = self.voice_turns.get(turn_id).cloned() else {
            let _ = reply.send(Err(AudioError::StreamError(
                "no voice conversation turn available to replay".to_string(),
            )));
            return;
        };

        let mut segments: Vec<(Vec<f32>, u32, f32)> = Vec::new();
        match mode {
            VoiceReplayMode::UserVoiceOnly => {
                if turn.user_pcm_16k.is_empty() {
                    let _ = reply.send(Err(AudioError::StreamError(
                        "user voice audio unavailable".to_string(),
                    )));
                    return;
                }
                segments.push((
                    i16_to_f32_vec(&turn.user_pcm_16k),
                    TARGET_SAMPLE_RATE_HZ,
                    1.0,
                ));
            }
            VoiceReplayMode::AiReplyOnly => {
                let Some((ai_pcm, ai_rate)) = turn.ai_pcm else {
                    let _ = reply.send(Err(AudioError::StreamError(
                        "AI voice reply unavailable".to_string(),
                    )));
                    return;
                };
                let speed = relative_playback_speed(requested_speed, turn.ai_synth_speed);
                segments.push((i16_to_f32_vec(&ai_pcm), ai_rate, speed));
            }
            VoiceReplayMode::FullExchange => {
                if turn.user_pcm_16k.is_empty() {
                    let _ = reply.send(Err(AudioError::StreamError(
                        "user voice audio unavailable for exchange replay".to_string(),
                    )));
                    return;
                }
                let Some((ai_pcm, ai_rate)) = turn.ai_pcm else {
                    let _ = reply.send(Err(AudioError::StreamError(
                        "AI voice reply unavailable for exchange replay".to_string(),
                    )));
                    return;
                };
                segments.push((
                    i16_to_f32_vec(&turn.user_pcm_16k),
                    TARGET_SAMPLE_RATE_HZ,
                    1.0,
                ));
                let gap_samples = (EARCON_SAMPLE_RATE_HZ as usize * 35) / 100;
                let mut bridge = vec![0.0_f32; gap_samples];
                bridge.extend_from_slice(self.earcons.samples_f32(EarconKind::CmdRecognized));
                segments.push((bridge, EARCON_SAMPLE_RATE_HZ, 1.0));
                let speed = relative_playback_speed(requested_speed, turn.ai_synth_speed);
                segments.push((i16_to_f32_vec(&ai_pcm), ai_rate, speed));
            }
        }

        self.prepare_for_new_playback();
        for (samples, rate, speed) in segments {
            if let Err(err) = self.output.append_f32_with_speed(samples, rate, speed, now) {
                self.output.stop_and_clear();
                let _ = self.echo_guard.stop_playback(now);
                let _ = reply.send(Err(err));
                self.publish_state();
                return;
            }
        }
        self.active_playback = Some(ActivePlaybackJob {
            pcm_rx: None,
            stream_finished: true,
            cache_key: None,
            cache_accum: Vec::new(),
            sample_rate_hz: Some(TARGET_SAMPLE_RATE_HZ),
            cache_on_complete: false,
            played_any_audio: true,
            reply: Some(PlaybackReply::Unit(reply)),
        });
        self.publish_state();
    }

    fn handle_stop_playback(&mut self, now: Instant) {
        match self.echo_guard.stop_playback(now) {
            StopPlaybackAction::NoOp => {}
            StopPlaybackAction::CancelCapture => {
                if let Some(job) = self.active_capture.take() {
                    let _ = job.reply.send(Err(AudioError::Cancelled));
                }
                self.publish_rms(0.0);
            }
            StopPlaybackAction::AbortListenEarconAndCancelCapture => {
                self.output.stop_and_clear();
                if let Some(job) = self.active_capture.take() {
                    let _ = job.reply.send(Err(AudioError::Cancelled));
                }
                self.publish_rms(0.0);
            }
            StopPlaybackAction::AbortPlayback => {
                self.output.stop_and_clear();
                self.abort_active_playback_without_caching();
                self.publish_rms(0.0);
            }
        }
        self.publish_state();
    }

    fn prepare_for_new_playback(&mut self) {
        if self.active_playback.is_some() {
            self.abort_active_playback_without_caching();
            self.output.stop_and_clear();
        } else if matches!(self.echo_guard.phase(), EchoGuardPhase::ListenEarconPlaying) {
            self.output.stop_and_clear();
        }
        if self.echo_guard.begin_playback() {
            if let Some(job) = self.active_capture.take() {
                let _ = job.reply.send(Err(AudioError::Cancelled));
            }
        }
        self.publish_rms(0.0);
    }

    fn abort_active_playback_without_caching(&mut self) {
        if let Some(mut job) = self.active_playback.take() {
            job.pcm_rx = None;
            job.cache_accum.clear();
            if let Some(reply) = job.reply.take() {
                reply.send_err(AudioError::Cancelled);
            }
        }
    }

    fn poll_watchdog(&mut self, now: Instant) {
        let (elapsed_since_cb, stream_errored) = match self.input.as_ref() {
            Some(input) => {
                let now_ms = self.origin.elapsed().as_millis() as u64 + 1;
                let last_cb = input.last_callback_ms.load(Ordering::Acquire);
                (
                    now_ms.saturating_sub(last_cb),
                    input.stream_error_flag.load(Ordering::Acquire),
                )
            }
            // Input was already dropped on a prior failed reopen attempt; keep polling backoff.
            None => (WATCHDOG_TIMEOUT_MS + 1, true),
        };

        if self.watchdog.poll(now, elapsed_since_cb, stream_errored)
            == WatchdogAction::AttemptReopen
        {
            // Drop old stream before reopening ALSA capture device.
            self.input = None;
            match open_hardware_input(&self.config.input_device, self.origin) {
                Ok(new_slot) => {
                    let new_rate = new_slot.resampler.src_rate_hz();
                    self.input = Some(new_slot);
                    self.watchdog.record_success();
                    self.update_health(true, self.health.output_ok, new_rate);
                }
                Err(_) => {
                    if !self.watchdog.record_failure(now) {
                        self.update_health(
                            false,
                            self.health.output_ok,
                            self.health.sample_rate_hz,
                        );
                        if self.active_capture.is_some() {
                            self.echo_guard.cancel_capture();
                            if let Some(job) = self.active_capture.take() {
                                let _ = job.reply.send(Err(AudioError::DeviceUnavailable(
                                    "microphone stream lost after bounded reopen attempts"
                                        .to_string(),
                                )));
                            }
                        }
                    }
                }
            }
        }
    }

    fn process_capture_stream(&mut self) {
        if let Some(job) = self.active_capture.as_ref() {
            if job.reply.is_closed() {
                self.handle_cancel_capture();
            }
        }

        let Some(input) = self.input.as_mut() else {
            return;
        };

        for _ in 0..MAX_TICKS_RING_DRAIN_CHUNKS {
            if input.ring_cons.is_empty() {
                break;
            }
            let n = input.ring_cons.pop_slice(&mut self.scratch_pop);
            if n == 0 {
                break;
            }
            input
                .pending_interleaved
                .extend_from_slice(&self.scratch_pop[..n]);
            let aligned_len = (input.pending_interleaved.len() / input.channels) * input.channels;
            if aligned_len == 0 {
                continue;
            }

            let mono_16k_res = input
                .resampler
                .push_interleaved(&input.pending_interleaved[..aligned_len]);
            input.pending_interleaved.drain(..aligned_len);

            let Ok(mut mono_16k) = mono_16k_res else {
                continue;
            };
            if mono_16k.is_empty() {
                continue;
            }

            // Always feed AudioConditioner continuously so 4th-order 80 Hz HPF and 50/60 Hz
            // notch biquad state stays settled without step-response spikes.
            self.conditioner.process_in_place(&mut mono_16k);

            if !self.echo_guard.accepts_mic_samples() {
                publish_rms_to(&self.rms_tx, 0.0);
                continue;
            }

            let mut capture_finished = false;
            if let Some(job) = self.active_capture.as_mut() {
                job.pending_16k.extend_from_slice(&mono_16k);
                while job.pending_16k.len() >= FRAME_SAMPLES_16K {
                    let frame: Vec<f32> = job.pending_16k.drain(..FRAME_SAMPLES_16K).collect();
                    let rms = self.echo_guard.effective_rms(compute_rms(&frame));
                    publish_rms_to(&self.rms_tx, rms);

                    let framer = job.framer.get_or_insert_with(|| {
                        self.vad.reset();
                        UtteranceFramer::new(
                            job.mode,
                            self.config.vad_threshold,
                            self.config.listening_timeout_ms,
                            TRAILING_SILENCE_MS,
                        )
                    });

                    match framer.feed_512_frame(&frame, &mut *self.vad) {
                        Ok(FrameAction::KeepWaiting) => {
                            if framer.is_capturing_speech() {
                                self.echo_guard.mark_capturing_speech();
                            }
                        }
                        Ok(FrameAction::CaptureComplete(captured)) => {
                            self.echo_guard.finish_capture();
                            if let Some(done_job) = self.active_capture.take() {
                                let _ = done_job.reply.send(Ok(captured));
                            }
                            publish_rms_to(&self.rms_tx, 0.0);
                            capture_finished = true;
                            break;
                        }
                        Ok(FrameAction::TimedOut) => {
                            self.echo_guard.finish_capture();
                            if let Some(done_job) = self.active_capture.take() {
                                let _ = done_job.reply.send(Err(AudioError::Timeout));
                            }
                            publish_rms_to(&self.rms_tx, 0.0);
                            capture_finished = true;
                            break;
                        }
                        Err(err) => {
                            self.echo_guard.finish_capture();
                            if let Some(done_job) = self.active_capture.take() {
                                let _ = done_job.reply.send(Err(err));
                            }
                            publish_rms_to(&self.rms_tx, 0.0);
                            capture_finished = true;
                            break;
                        }
                    }
                }
            }

            if capture_finished {
                break;
            }
        }

        if !self.echo_guard.accepts_mic_samples() {
            publish_rms_to(&self.rms_tx, 0.0);
        }
    }

    fn poll_active_playback(&mut self, now: Instant) {
        self.drain_pcm_stream_chunks(now);

        let sink_errored = self.output.has_errored();
        let done = match self.active_playback.as_ref() {
            Some(job) => (job.stream_finished && self.output.is_empty(now)) || sink_errored,
            None => false,
        };
        if !done {
            return;
        }

        let Some(mut job) = self.active_playback.take() else {
            return;
        };
        if sink_errored {
            job.cache_accum.clear();
            self.output.stop_and_clear();
            let _ = self.echo_guard.stop_playback(now);
            self.publish_state();
            if let Some(reply) = job.reply.take() {
                reply.send_err(AudioError::StreamError(
                    "audio output stream errored during playback".to_string(),
                ));
            }
            return;
        }
        if job.cache_on_complete && !job.cache_accum.is_empty() {
            let rate = job.sample_rate_hz.unwrap_or(22_050);
            self.cache
                .insert_completed(job.cache_key.take(), job.cache_accum, rate);
        }

        if !job.played_any_audio {
            // Nothing reached the speaker; skip echo cooldown.
            let _ = self.echo_guard.stop_playback(now);
            let _ = self.echo_guard.advance(
                now + Duration::from_millis(u64::from(self.config.echo_cooldown_ms) + 1),
                true,
            );
        } else {
            self.advance_echo_guard(now);
        }
        self.publish_state();

        if let Some(reply) = job.reply.take() {
            reply.send_ok();
        }
    }

    fn drain_pcm_stream_chunks(&mut self, now: Instant) {
        let mut fatal_err: Option<(AudioError, bool)> = None;

        if let Some(job) = self.active_playback.as_mut() {
            if let Some(rx) = job.pcm_rx.as_mut() {
                loop {
                    match rx.try_recv() {
                        Ok(chunk) => {
                            if !chunk.samples.is_empty() {
                                if chunk.sample_rate_hz == 0 {
                                    fatal_err = Some((
                                        AudioError::StreamError(
                                            "PCM chunk sample rate must be > 0 Hz".to_string(),
                                        ),
                                        true,
                                    ));
                                    break;
                                }
                                if job.sample_rate_hz.is_none() {
                                    job.sample_rate_hz = Some(chunk.sample_rate_hz);
                                }
                                let f32_samples = i16_to_f32_vec(&chunk.samples);
                                if let Err(err) =
                                    self.output
                                        .append_f32(f32_samples, chunk.sample_rate_hz, now)
                                {
                                    fatal_err = Some((err, false));
                                    break;
                                }
                                job.cache_accum.extend_from_slice(&chunk.samples);
                                job.played_any_audio = true;
                            }
                            if chunk.is_final {
                                job.stream_finished = true;
                                job.pcm_rx = None;
                                break;
                            }
                        }
                        Err(mpsc::error::TryRecvError::Empty) => break,
                        Err(mpsc::error::TryRecvError::Disconnected) => {
                            job.stream_finished = true;
                            job.pcm_rx = None;
                            break;
                        }
                    }
                }
            }
        }

        if let Some((err, clear_sink)) = fatal_err {
            if clear_sink {
                self.output.stop_and_clear();
            }
            if let Some(mut job) = self.active_playback.take() {
                job.pcm_rx = None;
                job.cache_accum.clear();
                if let Some(reply) = job.reply.take() {
                    reply.send_err(err);
                }
            }
            let _ = self.echo_guard.stop_playback(now);
        }
    }

    fn advance_echo_guard(&mut self, now: Instant) {
        let sink_empty = self.output.is_empty(now);
        if self.echo_guard.advance(now, sink_empty) == GuardTransition::EnteredListening {
            self.vad.reset();
            if let Some(job) = self.active_capture.as_mut() {
                job.pending_16k.clear();
                job.framer = Some(UtteranceFramer::new(
                    job.mode,
                    self.config.vad_threshold,
                    self.config.listening_timeout_ms,
                    TRAILING_SILENCE_MS,
                ));
            }
        }
    }

    fn fail_pending_on_shutdown(&mut self) {
        if let Some(job) = self.active_capture.take() {
            let _ = job.reply.send(Err(AudioError::Cancelled));
        }
        self.abort_active_playback_without_caching();
        self.output.stop_and_clear();
        self.publish_rms(0.0);
        let _ = self.state_tx.send_if_modified(|st| {
            if *st != AudioState::Idle {
                *st = AudioState::Idle;
                true
            } else {
                false
            }
        });
    }

    fn publish_rms(&self, rms: f32) {
        publish_rms_to(&self.rms_tx, rms);
    }

    fn publish_state(&self) {
        let next = self.echo_guard.audio_state();
        let _ = self.state_tx.send_if_modified(|cur| {
            if *cur != next {
                *cur = next;
                true
            } else {
                false
            }
        });
    }

    fn update_health(&mut self, input_ok: bool, output_ok: bool, sample_rate_hz: u32) {
        self.health = AudioHealthStatus {
            input_ok,
            output_ok,
            sample_rate_hz,
        };
        let next = self.health;
        let _ = self.health_tx.send_if_modified(|cur| {
            if *cur != next {
                *cur = next;
                true
            } else {
                false
            }
        });
    }
}

fn publish_rms_to(rms_tx: &watch::Sender<f32>, rms: f32) {
    let sanitized = if rms.is_finite() { rms.max(0.0) } else { 0.0 };
    let _ = rms_tx.send_if_modified(|cur| {
        if (*cur - sanitized).abs() > 1e-6 {
            *cur = sanitized;
            true
        } else {
            false
        }
    });
}

fn i16_to_f32_vec(samples: &[i16]) -> Vec<f32> {
    samples.iter().map(|&s| f32::from(s) / 32768.0).collect()
}

fn sanitize_playback_speed(speed: f32) -> f32 {
    if speed.is_finite() {
        speed.clamp(0.5, 2.0)
    } else {
        1.0
    }
}

fn relative_playback_speed(requested_speed: f32, synth_speed: f32) -> f32 {
    let req = sanitize_playback_speed(requested_speed);
    let base = sanitize_playback_speed(synth_speed);
    (req / base).clamp(0.5, 2.0)
}

fn open_hardware_input(pattern: &str, origin: Instant) -> Result<InputCaptureSlot, AudioError> {
    let device = find_input_device(pattern).ok_or_else(|| {
        AudioError::DeviceUnavailable("no ALSA/host audio input device found".to_string())
    })?;
    let supported = device.default_input_config().map_err(|err| {
        AudioError::DeviceUnavailable(format!("default input config failed: {err}"))
    })?;
    let sample_format = supported.sample_format();
    let stream_config: StreamConfig = supported.into();
    let sample_rate_hz = stream_config.sample_rate;
    let channels = stream_config.channels;

    let resampler = Resampler16k::new(sample_rate_hz, channels)?;
    let ring_capacity = ((sample_rate_hz as usize * usize::from(channels) * 3) / 2).max(4096);
    let rb = HeapRb::<f32>::new(ring_capacity);
    let (prod, cons) = rb.split();

    let now_ms = origin.elapsed().as_millis() as u64 + 1;
    let last_callback_ms = Arc::new(AtomicU64::new(now_ms));
    let stream_error_flag = Arc::new(AtomicBool::new(false));

    let stream = match sample_format {
        SampleFormat::F32 => build_input_stream_for_sample::<f32>(
            &device,
            stream_config,
            prod,
            Arc::clone(&last_callback_ms),
            Arc::clone(&stream_error_flag),
            origin,
        )?,
        SampleFormat::I16 => build_input_stream_for_sample::<i16>(
            &device,
            stream_config,
            prod,
            Arc::clone(&last_callback_ms),
            Arc::clone(&stream_error_flag),
            origin,
        )?,
        SampleFormat::U16 => build_input_stream_for_sample::<u16>(
            &device,
            stream_config,
            prod,
            Arc::clone(&last_callback_ms),
            Arc::clone(&stream_error_flag),
            origin,
        )?,
        SampleFormat::I32 => build_input_stream_for_sample::<i32>(
            &device,
            stream_config,
            prod,
            Arc::clone(&last_callback_ms),
            Arc::clone(&stream_error_flag),
            origin,
        )?,
        SampleFormat::F64 => build_input_stream_for_sample::<f64>(
            &device,
            stream_config,
            prod,
            Arc::clone(&last_callback_ms),
            Arc::clone(&stream_error_flag),
            origin,
        )?,
        other => {
            return Err(AudioError::StreamError(format!(
                "unsupported input sample format: {other:?}"
            )));
        }
    };

    stream
        .play()
        .map_err(|err| AudioError::StreamError(format!("failed starting input stream: {err}")))?;

    Ok(InputCaptureSlot {
        _stream: Some(stream),
        ring_cons: cons,
        channels: usize::from(channels),
        resampler,
        last_callback_ms,
        stream_error_flag,
        pending_interleaved: Vec::new(),
    })
}

fn build_input_stream_for_sample<T>(
    device: &cpal::Device,
    stream_config: StreamConfig,
    mut prod: HeapProd<f32>,
    last_callback_ms: Arc<AtomicU64>,
    stream_error_flag: Arc<AtomicBool>,
    origin: Instant,
) -> Result<cpal::Stream, AudioError>
where
    T: SizedSample,
    f32: FromSample<T>,
{
    let err_flag = Arc::clone(&stream_error_flag);
    device
        .build_input_stream::<T, _, _>(
            stream_config,
            move |data: &[T], _| {
                let ts = origin.elapsed().as_millis() as u64 + 1;
                last_callback_ms.store(ts, Ordering::Release);
                let mut buf = [0.0_f32; 512];
                for chunk in data.chunks(buf.len()) {
                    for (dst, &src) in buf.iter_mut().zip(chunk.iter()) {
                        *dst = f32::from_sample_(src);
                    }
                    let _ = prod.push_slice(&buf[..chunk.len()]);
                }
            },
            move |err| {
                err_flag.store(true, Ordering::Release);
                eprintln!("kural-audio input stream error: {err}");
            },
            None,
        )
        .map_err(|err| {
            AudioError::DeviceUnavailable(format!("failed building input stream: {err}"))
        })
}

fn open_hardware_output(pattern: &str, master_volume: f32) -> OutputSink {
    let errored = Arc::new(AtomicBool::new(false));
    let err_flag = Arc::clone(&errored);
    let err_cb = move |err: cpal::Error| {
        err_flag.store(true, Ordering::Release);
        eprintln!("kural-audio output stream error: {err}");
    };

    let selected = find_output_device(pattern);
    let sink_res = match selected {
        Some(dev) => open_sink_with_device(dev, err_cb),
        None => rodio::DeviceSinkBuilder::open_default_sink(),
    };

    let mut sink = match sink_res {
        Ok(s) => s,
        Err(_) => return OutputSink::Unavailable,
    };
    sink.log_on_drop(false);
    let player = rodio::Player::connect_new(sink.mixer());
    player.set_volume(master_volume);

    OutputSink::Hardware {
        _sink: sink,
        player,
        errored,
    }
}

fn open_sink_with_device<E>(
    device: cpal::Device,
    err_cb: E,
) -> Result<rodio::MixerDeviceSink, rodio::DeviceSinkError>
where
    E: FnMut(cpal::Error) + Send + Clone + 'static,
{
    let first = rodio::DeviceSinkBuilder::from_device(device.clone())?
        .with_buffer_size(cpal::BufferSize::Fixed(256))
        .with_error_callback(err_cb.clone())
        .open_stream();
    if first.is_ok() {
        return first;
    }

    rodio::DeviceSinkBuilder::from_device(device)?
        .with_buffer_size(cpal::BufferSize::Fixed(512))
        .with_error_callback(err_cb)
        .open_sink_or_fallback()
}

#[cfg(test)]
mod tests {
    use super::*;

    struct AlwaysSpeechVad;
    impl VadBackend for AlwaysSpeechVad {
        fn reset(&mut self) {}
        fn speech_probability(&mut self, _frame_512_16k: &[f32]) -> Result<f32, AudioError> {
            Ok(0.95)
        }
    }

    struct AlwaysSilenceVad;
    impl VadBackend for AlwaysSilenceVad {
        fn reset(&mut self) {}
        fn speech_probability(&mut self, _frame_512_16k: &[f32]) -> Result<f32, AudioError> {
            Ok(0.02)
        }
    }

    #[test]
    fn echo_guard_discards_mic_and_clamps_rms_during_playing_and_cooldown() {
        let mut guard = EchoGuardState::new(220, 250, false);
        let t0 = Instant::now();

        assert_eq!(guard.audio_state(), AudioState::Idle);
        assert!(!guard.accepts_mic_samples());
        assert_eq!(guard.effective_rms(0.42), 0.0);

        // Start playback -> PlayingTts
        assert!(!guard.begin_playback());
        assert_eq!(guard.audio_state(), AudioState::PlayingTts);
        assert!(!guard.accepts_mic_samples());
        assert_eq!(guard.effective_rms(0.85), 0.0);

        // Sink finishes at t0 + 100ms -> enters 250ms Cooldown
        let t1 = t0 + Duration::from_millis(100);
        assert_eq!(guard.advance(t1, true), GuardTransition::EnteredCooldown);
        assert_eq!(guard.audio_state(), AudioState::Cooldown);
        assert!(!guard.accepts_mic_samples());
        assert_eq!(guard.effective_rms(0.65), 0.0);

        // Still in cooldown at +249 ms
        assert_eq!(
            guard.advance(t1 + Duration::from_millis(249), true),
            GuardTransition::None
        );
        assert_eq!(guard.audio_state(), AudioState::Cooldown);
        assert!(!guard.accepts_mic_samples());
        assert_eq!(guard.effective_rms(0.30), 0.0);

        // Cooldown expires at +250 ms -> Idle
        assert_eq!(
            guard.advance(t1 + Duration::from_millis(250), true),
            GuardTransition::CooldownElapsed
        );
        assert_eq!(guard.audio_state(), AudioState::Idle);
        assert!(!guard.accepts_mic_samples());
    }

    #[test]
    fn echo_guard_listen_start_enforces_220ms_post_empty_guard_before_listening() {
        let mut guard = EchoGuardState::new(220, 250, false);
        let t0 = Instant::now();

        let action = guard.begin_capture(t0, true).expect("start capture");
        assert_eq!(
            action,
            CaptureStartAction::PlayListenStart {
                abort_existing_playback: false
            }
        );
        assert_eq!(guard.phase(), EchoGuardPhase::ListenEarconPlaying);
        assert!(!guard.accepts_mic_samples());
        assert_eq!(guard.effective_rms(0.5), 0.0);

        // Overlapping capture rejected cleanly
        assert_eq!(
            guard.begin_capture(t0 + Duration::from_millis(20), true),
            Err(AudioError::CaptureInProgress)
        );

        // Sink empties at t0 + 90ms -> enters 220ms ListenEarconGuard
        let t_empty = t0 + Duration::from_millis(90);
        assert_eq!(guard.advance(t_empty, true), GuardTransition::None);
        assert!(matches!(
            guard.phase(),
            EchoGuardPhase::ListenEarconGuard { .. }
        ));
        assert!(!guard.accepts_mic_samples());
        assert_eq!(guard.effective_rms(0.5), 0.0);

        // At +219 ms post-empty, mic must still remain gated closed
        assert_eq!(
            guard.advance(t_empty + Duration::from_millis(219), true),
            GuardTransition::None
        );
        assert!(!guard.accepts_mic_samples());

        // At +220 ms post-empty, transitions to Listening and accepts mic
        assert_eq!(
            guard.advance(t_empty + Duration::from_millis(220), true),
            GuardTransition::EnteredListening
        );
        assert_eq!(guard.audio_state(), AudioState::Listening);
        assert!(guard.accepts_mic_samples());
        assert!((guard.effective_rms(0.25) - 0.25).abs() < 1e-6);

        // Speech onset moves to CapturingSpeech
        guard.mark_capturing_speech();
        assert_eq!(guard.audio_state(), AudioState::CapturingSpeech);
        assert!(guard.accepts_mic_samples());

        // Processing gates mic closed again
        guard.mark_processing();
        assert_eq!(guard.audio_state(), AudioState::Processing);
        assert!(!guard.accepts_mic_samples());
        assert_eq!(guard.effective_rms(0.9), 0.0);
    }

    #[test]
    fn echo_guard_stop_playback_semantics_across_states() {
        let mut guard = EchoGuardState::new(220, 250, false);
        let t0 = Instant::now();

        // Idle -> NoOp
        assert_eq!(guard.stop_playback(t0), StopPlaybackAction::NoOp);
        assert_eq!(guard.audio_state(), AudioState::Idle);

        // PlayingTts -> AbortPlayback + Cooldown
        guard.begin_playback();
        assert_eq!(guard.stop_playback(t0), StopPlaybackAction::AbortPlayback);
        assert_eq!(guard.audio_state(), AudioState::Cooldown);

        // Cooldown -> NoOp
        assert_eq!(guard.stop_playback(t0), StopPlaybackAction::NoOp);

        // Wait out cooldown, enter Listening -> stop_playback cancels capture to Idle
        guard.advance(t0 + Duration::from_millis(250), true);
        guard
            .begin_capture(t0 + Duration::from_millis(260), false)
            .unwrap();
        assert_eq!(guard.audio_state(), AudioState::Listening);
        assert_eq!(
            guard.stop_playback(t0 + Duration::from_millis(270)),
            StopPlaybackAction::CancelCapture
        );
        assert_eq!(guard.audio_state(), AudioState::Idle);
    }

    #[test]
    fn watchdog_bounded_reopen_backoff_is_500_1000_2000_then_exhausted() {
        assert_eq!(reopen_backoff_ms(0), Some(500));
        assert_eq!(reopen_backoff_ms(1), Some(1000));
        assert_eq!(reopen_backoff_ms(2), Some(2000));
        assert_eq!(reopen_backoff_ms(3), None);

        let mut wd = WatchdogState::new(true);
        let t0 = Instant::now();
        assert_eq!(wd.poll(t0, 499, false), WatchdogAction::Healthy);
        assert_eq!(wd.poll(t0, 501, false), WatchdogAction::AttemptReopen);

        assert!(wd.record_failure(t0)); // schedules +500ms
        assert_eq!(
            wd.poll(t0 + Duration::from_millis(499), 1000, false),
            WatchdogAction::WaitingBackoff
        );
        assert_eq!(
            wd.poll(t0 + Duration::from_millis(500), 1000, false),
            WatchdogAction::AttemptReopen
        );

        let t1 = t0 + Duration::from_millis(500);
        assert!(wd.record_failure(t1)); // schedules +1000ms
        assert_eq!(
            wd.poll(t1 + Duration::from_millis(999), 2000, false),
            WatchdogAction::WaitingBackoff
        );
        assert_eq!(
            wd.poll(t1 + Duration::from_millis(1000), 2000, false),
            WatchdogAction::AttemptReopen
        );

        let t2 = t1 + Duration::from_millis(1000);
        assert!(wd.record_failure(t2)); // schedules +2000ms
        assert_eq!(
            wd.poll(t2 + Duration::from_millis(2000), 4000, false),
            WatchdogAction::AttemptReopen
        );

        let t3 = t2 + Duration::from_millis(2000);
        assert!(!wd.record_failure(t3)); // exhausted -> input_ok = false
        assert!(!wd.active);
    }

    #[tokio::test]
    async fn actor_stop_playback_aborts_under_20ms_and_never_caches_cancelled_tts() {
        let cfg = AudioConfig {
            echo_cooldown_ms: 60,
            earcon_guard_ms: 20,
            ..AudioConfig::default()
        };

        let sim_out = SimulatedOutputHandle::new();
        let rb = HeapRb::<f32>::new(16_000);
        let (_prod, cons) = rb.split();

        let handle = AudioHandle::spawn_with_backend(
            &cfg,
            Some(Box::new(AlwaysSpeechVad)),
            1024 * 1024,
            BackendMode::Simulated {
                input_cons: Some(cons),
                input_rate_hz: 16_000,
                input_channels: 1,
                output: Some(sim_out.clone()),
            },
        );
        assert!(handle.is_healthy());

        let state_rx = handle.subscribe_state();
        let rms_rx = handle.subscribe_rms();

        let (pcm_tx, pcm_rx) = mpsc::channel(4);
        let play_handle = handle.clone();
        let play_task = tokio::spawn(async move {
            play_handle
                .play_pcm_stream(pcm_rx, Some("kural-1".to_string()))
                .await
        });

        // Push a long non-final 2-second chunk
        pcm_tx
            .send(PcmChunk {
                samples: vec![5000_i16; 32_000],
                sample_rate_hz: 16_000,
                is_final: false,
            })
            .await
            .unwrap();

        // Wait until actor enters PlayingTts
        for _ in 0..20 {
            if *state_rx.borrow() == AudioState::PlayingTts {
                break;
            }
            tokio::time::sleep(Duration::from_millis(5)).await;
        }
        assert_eq!(*state_rx.borrow(), AudioState::PlayingTts);
        assert_eq!(*rms_rx.borrow(), 0.0);

        let abort_start = Instant::now();
        handle.stop_playback().await.unwrap();
        let abort_elapsed = abort_start.elapsed();
        assert!(
            abort_elapsed < Duration::from_millis(20),
            "stop_playback took {:?}, expected < 20 ms",
            abort_elapsed
        );
        assert_eq!(sim_out.cleared_count(), 1);
        assert_eq!(*state_rx.borrow(), AudioState::Cooldown);

        let play_res = play_task.await.unwrap();
        assert_eq!(play_res, Err(AudioError::Cancelled));

        // Cancelled TTS must NOT be committed to cache/replay_last
        let replay_err = handle.replay_last().await.unwrap_err();
        assert!(matches!(replay_err, AudioError::StreamError(_)));

        // Wait for cooldown to return to Idle
        tokio::time::sleep(Duration::from_millis(90)).await;
        assert_eq!(*state_rx.borrow(), AudioState::Idle);

        // No-op when Idle
        handle.stop_playback().await.unwrap();
        assert_eq!(*state_rx.borrow(), AudioState::Idle);

        handle.shutdown();
    }

    #[tokio::test]
    async fn actor_discards_mic_during_playing_and_streams_completed_tts_for_replay() {
        let cfg = AudioConfig {
            echo_cooldown_ms: 40,
            earcon_guard_ms: 20,
            ..AudioConfig::default()
        };

        let sim_out = SimulatedOutputHandle::new();
        let rb = HeapRb::<f32>::new(32_000);
        let (mut prod, cons) = rb.split();

        let handle = AudioHandle::spawn_with_backend(
            &cfg,
            Some(Box::new(AlwaysSpeechVad)),
            1024 * 1024,
            BackendMode::Simulated {
                input_cons: Some(cons),
                input_rate_hz: 16_000,
                input_channels: 1,
                output: Some(sim_out.clone()),
            },
        );
        let rms_rx = handle.subscribe_rms();

        // Feed loud echo samples while playing short 40 ms TTS clip
        let loud_echo = vec![0.75_f32; 4096];
        let _ = prod.push_slice(&loud_echo);

        let (pcm_tx, pcm_rx) = mpsc::channel(2);
        pcm_tx
            .send(PcmChunk {
                samples: vec![1200_i16; 640], // 40 ms @ 16 kHz
                sample_rate_hz: 16_000,
                is_final: true,
            })
            .await
            .unwrap();

        handle
            .play_pcm_stream(pcm_rx, Some("completed-clip".to_string()))
            .await
            .unwrap();

        // RMS must remain 0.0 during playback & cooldown
        assert_eq!(*rms_rx.borrow(), 0.0);

        // Wait for cooldown to finish
        tokio::time::sleep(Duration::from_millis(70)).await;
        assert_eq!(*handle.subscribe_state().borrow(), AudioState::Idle);

        // Replay must succeed cleanly using cached clip
        handle.replay_last().await.unwrap();
        assert_eq!(sim_out.appended_count(), 2);

        // Volume & mute propagation
        handle.set_volume(0.42).await.unwrap();
        assert!((sim_out.last_volume() - 0.42).abs() < 1e-5);
        handle.set_muted(true).await.unwrap();
        assert_eq!(sim_out.last_volume(), 0.0);
        handle.set_muted(false).await.unwrap();
        assert!((sim_out.last_volume() - 0.42).abs() < 1e-5);

        handle.shutdown();
    }

    #[tokio::test]
    async fn actor_capture_listening_timeout_and_stop_playback_cancels_capture() {
        let cfg = AudioConfig {
            earcon_guard_ms: 20,
            echo_cooldown_ms: 20,
            listening_timeout_ms: 96, // 3 frames * 32ms
            ..AudioConfig::default()
        };

        let sim_out = SimulatedOutputHandle::new();
        let rb = HeapRb::<f32>::new(32_000);
        let (mut prod, cons) = rb.split();

        let handle = AudioHandle::spawn_with_backend(
            &cfg,
            Some(Box::new(AlwaysSilenceVad)),
            1024 * 1024,
            BackendMode::Simulated {
                input_cons: Some(cons),
                input_rate_hz: 16_000,
                input_channels: 1,
                output: Some(sim_out.clone()),
            },
        );

        let cap_handle = handle.clone();
        let cap_task = tokio::spawn(async move {
            cap_handle
                .start_capture(CaptureMode::VadCommand {
                    max_duration_ms: 2000,
                })
                .await
        });

        // Wait until actor passes ListenStart + 20ms guard and enters Listening
        let state_rx = handle.subscribe_state();
        for _ in 0..40 {
            if *state_rx.borrow() == AudioState::Listening {
                break;
            }
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
        assert_eq!(*state_rx.borrow(), AudioState::Listening);

        // Feed 4 silence frames (128 ms >= 96 ms listening_timeout_ms)
        let silence = vec![0.0005_f32; 2048];
        let _ = prod.push_slice(&silence);

        let res = cap_task.await.unwrap();
        assert_eq!(res, Err(AudioError::Timeout));
        assert_eq!(*state_rx.borrow(), AudioState::Idle);

        // Start a second capture and cancel via stop_playback()
        let cap2_handle = handle.clone();
        let cap2_task = tokio::spawn(async move {
            cap2_handle
                .start_capture(CaptureMode::VadCommand {
                    max_duration_ms: 2000,
                })
                .await
        });
        for _ in 0..40 {
            if *state_rx.borrow() == AudioState::Listening {
                break;
            }
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
        assert_eq!(*state_rx.borrow(), AudioState::Listening);
        handle.stop_playback().await.unwrap();
        assert_eq!(cap2_task.await.unwrap(), Err(AudioError::Cancelled));
        assert_eq!(*state_rx.borrow(), AudioState::Idle);

        handle.shutdown();
    }

    #[tokio::test]
    async fn actor_nowait_cancel_capture_cancels_cleanly() {
        let cfg = AudioConfig {
            echo_cooldown_ms: 40,
            earcon_guard_ms: 15,
            ..AudioConfig::default()
        };
        let sim_out = SimulatedOutputHandle::new();
        let rb = HeapRb::<f32>::new(16_000);
        let (_prod, cons) = rb.split();
        let handle = AudioHandle::spawn_with_backend(
            &cfg,
            Some(Box::new(AlwaysSilenceVad)),
            1024 * 1024,
            BackendMode::Simulated {
                input_cons: Some(cons),
                input_rate_hz: 16_000,
                input_channels: 1,
                output: Some(sim_out),
            },
        );

        let state_rx = handle.subscribe_state();
        let cap_handle = handle.clone();
        let cap_task = tokio::spawn(async move {
            cap_handle
                .start_capture(CaptureMode::VadCommand {
                    max_duration_ms: 2000,
                })
                .await
        });

        for _ in 0..40 {
            if *state_rx.borrow() == AudioState::Listening {
                break;
            }
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
        assert_eq!(*state_rx.borrow(), AudioState::Listening);

        handle.cancel_capture_nowait();
        assert_eq!(cap_task.await.unwrap(), Err(AudioError::Cancelled));
        handle.stop_playback_nowait();
        handle.shutdown();
    }

    #[test]
    fn headless_hardware_spawn_never_panics() {
        let cfg = AudioConfig {
            input_device: "__nonexistent_ci_input__".to_string(),
            output_device: "__nonexistent_ci_output__".to_string(),
            ..AudioConfig::default()
        };
        let handle = AudioHandle::spawn(&cfg, None, 1024 * 1024);
        let _ = handle.health();
        handle.shutdown();
    }

    #[tokio::test]
    async fn actor_voice_turn_replay_and_cached_key_preserve_echo_guard_and_speed() {
        let cfg = AudioConfig {
            echo_cooldown_ms: 40,
            earcon_guard_ms: 20,
            ..AudioConfig::default()
        };
        let sim_out = SimulatedOutputHandle::new();
        let rb = HeapRb::<f32>::new(16_000);
        let (_prod, cons) = rb.split();
        let handle = AudioHandle::spawn_with_backend(
            &cfg,
            Some(Box::new(AlwaysSilenceVad)),
            2 * 1024 * 1024,
            BackendMode::Simulated {
                input_cons: Some(cons),
                input_rate_hz: 16_000,
                input_channels: 1,
                output: Some(sim_out.clone()),
            },
        );
        let state_rx = handle.subscribe_state();

        // Commit numeric cache key and verify hit + miss + speed
        assert!(!handle.play_cached_tts_by_key(999, 1.0).await.unwrap());
        let ai_clip: Arc<[i16]> = Arc::from(vec![2000_i16; 2205]); // 100 ms @ 22.05kHz
        handle
            .commit_completed_tts_with_speed(777, ai_clip.clone(), 22_050, 1.0)
            .await
            .unwrap();
        assert!(handle.play_cached_tts_by_key(777, 1.25).await.unwrap());
        tokio::time::sleep(Duration::from_millis(70)).await;
        assert_eq!(*state_rx.borrow(), AudioState::Idle);
        assert_eq!(sim_out.appended_meta()[0], (2205, 22_050, 1.25));

        // Record Voice-to-Voice turn -> UserVoiceOnly -> AiReplyOnly -> FullExchange -> abort
        let user_f32: Arc<[f32]> = Arc::from(vec![0.2_f32; 1600]); // 100 ms @ 16kHz
        handle.record_voice_turn_user(1, user_f32).await.unwrap();
        handle
            .attach_voice_turn_ai_with_speed(1, ai_clip, 22_050, 1.0)
            .await
            .unwrap();

        handle
            .play_voice_turn(Some(1), VoiceReplayMode::UserVoiceOnly, 1.25)
            .await
            .unwrap();
        tokio::time::sleep(Duration::from_millis(60)).await;
        assert_eq!(sim_out.appended_meta()[1], (1600, 16_000, 1.0));

        handle
            .play_voice_turn(Some(1), VoiceReplayMode::AiReplyOnly, 0.75)
            .await
            .unwrap();
        tokio::time::sleep(Duration::from_millis(60)).await;
        assert_eq!(sim_out.appended_meta()[2], (2205, 22_050, 0.75));

        // FullExchange queues 3 segments (user @ 16k 1.0x, 350ms gap + earcon @ 22.05k 1.0x, AI @ 22.05k 1.25x)
        let full_task = tokio::spawn({
            let h = handle.clone();
            async move {
                h.play_voice_turn(None, VoiceReplayMode::FullExchange, 1.25)
                    .await
            }
        });
        for _ in 0..20 {
            if *state_rx.borrow() == AudioState::PlayingTts {
                break;
            }
            tokio::time::sleep(Duration::from_millis(5)).await;
        }
        assert_eq!(*state_rx.borrow(), AudioState::PlayingTts);
        let meta = sim_out.appended_meta();
        assert_eq!(meta.len(), 6);
        assert_eq!(meta[3], (1600, 16_000, 1.0));
        assert_eq!(meta[4].1, 22_050);
        assert_eq!(meta[4].2, 1.0);
        assert_eq!(meta[5], (2205, 22_050, 1.25));

        // Mid-exchange stop must cancel cleanly in < 20 ms
        let t_abort = Instant::now();
        handle.stop_playback().await.unwrap();
        assert!(t_abort.elapsed() < Duration::from_millis(20));
        assert_eq!(full_task.await.unwrap(), Err(AudioError::Cancelled));

        handle.shutdown();
    }
}
