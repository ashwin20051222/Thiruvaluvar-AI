use crate::BackendError;
use crate::ai_worker::{AiWorkerHandle, SpeechCapabilities};
use crate::speech_text::{build_speech_payload, resolve_effective_scholar};
use chrono::{Days, Local};
use kural_ai::{AgeClassifier, AiError, SemanticExplainer, SherpaSileroVad, detect_text_language};
use kural_audio::{
    AudioError, AudioHandle, EarconKind, MAX_VOICE_TURNS, VadBackend, compute_tts_cache_key,
    hash_tts_content,
};
use kural_core::{
    AGE_MODE_OPENED_ADULT, AGE_MODE_OPENED_KIDS, AGE_MODE_OPENED_YOUTH, AUTO_SPEAK_OFF_NOTICE,
    AUTO_SPEAK_ON_NOTICE, ActiveFilter, AgeDetection, AgeGroup, AgeOutcome, AppConfig, AppScreen,
    AudioPlaybackStatus, AudioState, BackendCommand, CaptureMode, CorpusCatalog, GroundedAnswer,
    KuralSummary, ModalKind, NOTHING_TO_REPLAY_NOTICE, OOS_VOICE_TTS_REFUSAL,
    PARIMEL_FALLBACK_NOTICE, REPLAY_AUDIO_NOTICE, REPLAY_FULL_EXCHANGE_NOTICE,
    REPLAY_USER_VOICE_NOTICE, RetryReason, Scholar, SystemHealth, UiSnapshot, UiTransientEvent,
    VOICE_ADHIKARAM_RANGE_ERROR, VOICE_KURAL_RANGE_ERROR, VOICE_LOOP_OFF_NOTICE,
    VOICE_LOOP_ON_NOTICE, VOICE_LOOP_SILENCE_STOP_NOTICE, VOICE_UNCLEAR_RETRY_NOTICE,
    VoiceReplayMode, VoiceTurnSummary, effective_tts_speed, normalize_nfc_trimmed, retry_copy,
    snap_speech_speed,
};
use kural_db::{KuralDb, UserStateStore};
use kural_voice_cmd::{RouterContext, parse_quiz_choice, validate_voice_transcript};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};
use tokio::runtime::{Builder as RuntimeBuilder, Runtime};
use tokio::sync::{mpsc, watch};
use tokio::task::JoinHandle;
use tokio_util::sync::CancellationToken;

const CMD_QUEUE_CAP: usize = 128;
const INTERNAL_QUEUE_CAP: usize = 128;
const EVENT_QUEUE_CAP: usize = 64;
const PREFS_DEBOUNCE_SECS: u64 = 5;
const RRF_K: f32 = 60.0;
const TEXT_SEARCH_LIMIT: usize = 24;

enum ControllerMsg {
    Command(BackendCommand),
    AddEventSubscriber(mpsc::Sender<UiTransientEvent>),
    QueryAdhikaramKurals {
        adhikaram_id: u8,
        reply: std::sync::mpsc::SyncSender<Result<Vec<KuralSummary>, BackendError>>,
    },
    Shutdown(std::sync::mpsc::SyncSender<()>),
}

struct BilingualMsg {
    ta: String,
    en: String,
}

fn ai_inference_timeout_msg(timeout_ms: u64) -> BilingualMsg {
    BilingualMsg {
        ta: format!("AI நேர வரம்பு முடிந்தது ({timeout_ms} ms) — மீண்டும் முயற்சிக்கவும்."),
        en: format!("AI inference timed out after {timeout_ms} ms — please try again."),
    }
}

fn stt_timeout_msg(timeout_ms: u64) -> BilingualMsg {
    BilingualMsg {
        ta: format!("குரல் உணர்வு நேர வரம்பு முடிந்தது ({timeout_ms} ms) — மீண்டும் பேசவும்."),
        en: format!("Voice recognition timed out after {timeout_ms} ms — please speak again."),
    }
}

fn tts_timeout_msg(timeout_ms: u64) -> BilingualMsg {
    BilingualMsg {
        ta: format!("குரல் ஒலி உருவாக்க நேர வரம்பு முடிந்தது ({timeout_ms} ms)."),
        en: format!("Speech synthesis timed out after {timeout_ms} ms."),
    }
}

enum InternalMsg {
    SpeechCapsUpdated(SpeechCapabilities),
    StartVoiceAutoRouteDone {
        session_id: u64,
        nav_epoch: u64,
        lang: kural_core::Language,
        auto_triggered: bool,
        age_timed_out: bool,
        outcome: AgeOutcome,
        transcript: Option<String>,
        caps: Option<SpeechCapabilities>,
    },
    VoiceCmdTranscribed {
        session_id: u64,
        nav_epoch: u64,
        lang: kural_core::Language,
        is_timeout: bool,
        captured_user_16k: Option<Arc<[f32]>>,
        transcript: Result<String, BilingualMsg>,
        caps: Option<SpeechCapabilities>,
    },
    VoiceAiDone {
        session_id: u64,
        nav_epoch: u64,
        turn_id: Option<u64>,
        seed_kural: Option<u16>,
        lang: kural_core::Language,
        result: Result<GroundedAnswer, BilingualMsg>,
    },
    TextAiDone {
        ai_epoch: u64,
        result: Result<GroundedAnswer, BilingualMsg>,
    },
    SearchDone {
        search_epoch: u64,
        result: Result<kural_core::SearchOutcome, String>,
    },
    TtsFinished {
        tts_epoch: u64,
        nav_epoch: u64,
        target: kural_core::SpeakTarget,
        kural_number: u16,
        voice_turn_id: Option<u64>,
        has_saved_audio: bool,
        voice_loop_rearm: bool,
        ok: bool,
        error_msg: Option<BilingualMsg>,
    },
}

struct BackendHandleInner {
    cmd_tx: mpsc::Sender<ControllerMsg>,
    snapshot_rx: watch::Receiver<Arc<UiSnapshot>>,
    rms_rx: watch::Receiver<f32>,
    initial_events_rx: Mutex<Option<mpsc::Receiver<UiTransientEvent>>>,
    catalog: CorpusCatalog,
    audio: AudioHandle,
    runtime: Mutex<Option<Runtime>>,
    loop_done_rx: Mutex<Option<std::sync::mpsc::Receiver<()>>>,
}

/// Thread-safe, non-blocking backend handle shared with the Dioxus GUI (`kural-app`).
#[derive(Clone)]
pub struct BackendHandle {
    inner: Arc<BackendHandleInner>,
}

impl BackendHandle {
    /// Synchronously boot DBs, `AudioHandle`, `AgeClassifier`, `SemanticExplainer`, `AiWorker`,
    /// Tokio runtime, and initial [`UiSnapshot`].
    pub fn start(config: AppConfig) -> Result<Self, BackendError> {
        crate::init_tracing();
        config.validate()?;

        let corpus =
            KuralDb::open_with_models_dir(&config.paths.kural_db, &config.paths.models_dir)?;
        let catalog = corpus.get_catalog()?;
        let user_state =
            UserStateStore::open(&config.paths.user_state_db, config.audio.master_volume)?;
        let (mut prefs, kids_progress, youth_progress, bookmarks) =
            user_state.load_snapshot_state(&corpus)?;
        prefs.speech_speed = snap_speech_speed(prefs.speech_speed);

        let initial_kural_num = prefs.last_kural_for(prefs.default_age);
        let current_kural = corpus
            .get_kural_full(initial_kural_num)
            .or_else(|_| corpus.get_kural_full(1))?;
        let pool = default_pool_or_fallback(&corpus, AppScreen::Start, current_kural.kural.number)?;
        let pool_idx = find_kural_idx(&pool, current_kural.kural.number);

        let vad_impl: Option<Box<dyn VadBackend>> =
            match SherpaSileroVad::try_load(&config.paths.models_dir, &config.audio, &config.ai) {
                Ok(Some(v)) => Some(Box::new(v)),
                Ok(None) | Err(_) => None,
            };
        let vad_loaded = vad_impl.is_some();

        let (ai_worker, speech_caps) =
            AiWorkerHandle::spawn(&config.paths.models_dir, prefs.lang, &config.ai)?;
        let audio = AudioHandle::spawn(&config.audio, vad_impl, config.ai.tts_cache_max_bytes);
        let age_classifier = Arc::new(AgeClassifier::new(config.age_ai.clone()));
        let explainer = Arc::new(SemanticExplainer::new(corpus.clone(), config.ai.clone()));

        let requested_scholar = Scholar::MuVa;
        let effective_scholar = resolve_effective_scholar(&current_kural, requested_scholar);
        let embeddings_loaded = corpus.embeddings_loaded();
        let mut degraded_parts: Vec<&'static str> = Vec::new();
        if !embeddings_loaded {
            degraded_parts.push("semantic embeddings (lexical FTS5 fallback)");
        }
        if !vad_loaded {
            degraded_parts.push("Silero VAD (RMS energy fallback)");
        }
        if !speech_caps.active_stt_loaded {
            degraded_parts.push("offline STT model");
        }
        if !speech_caps.tts_neural_loaded {
            if speech_caps.active_tts_loaded {
                degraded_parts.push("neural Piper TTS (espeak-ng fallback)");
            } else {
                degraded_parts.push("TTS voice synthesis");
            }
        }
        let degraded_reason = if degraded_parts.is_empty() {
            None
        } else {
            let reason = degraded_parts.join(", ");
            tracing::warn!(
                degraded_reason = %reason,
                "Starting with optional AI/audio models in fallback/absent mode"
            );
            Some(reason)
        };

        let system_health = SystemHealth {
            db_ok: true,
            user_state_ok: true,
            audio_ok: audio.is_healthy(),
            embeddings_loaded,
            vad_loaded,
            active_stt_loaded: speech_caps.active_stt_loaded,
            active_tts_loaded: speech_caps.active_tts_loaded,
            degraded_reason: degraded_reason.clone(),
        };

        let initial_snapshot = Arc::new(UiSnapshot {
            prefs: prefs.clone(),
            screen: AppScreen::Start,
            modal: None,
            current_kural,
            requested_scholar,
            effective_scholar,
            active_filter: None,
            pool_len: pool.len(),
            pool_idx,
            kids_progress,
            youth_progress,
            bookmarks,
            audio_state: AudioState::Idle,
            playback_status: AudioPlaybackStatus::Idle,
            has_replay_clip: false,
            recent_voice_turns: Vec::new(),
            can_replay_tts: false,
            can_replay_voice_turn: false,
            auto_speak_text: prefs.auto_speak_text,
            auto_age_route_on_start: true,
            voice_to_voice_loop: false,
            active_replay_label: None,
            system_health,
        });

        let (snapshot_tx, snapshot_rx) = watch::channel(initial_snapshot.clone());
        let (event_tx, event_rx) = mpsc::channel(EVENT_QUEUE_CAP);
        if let Some(reason) = degraded_reason.as_ref() {
            let _ = event_tx.try_send(UiTransientEvent::InfoToast {
                message_ta: format!("மாற்று முறையில் இயங்குகிறது ({reason})."),
                message_en: format!("Running in fallback mode ({reason})."),
            });
        }
        let (cmd_tx, cmd_rx) = mpsc::channel(CMD_QUEUE_CAP);
        let rms_rx = audio.subscribe_rms();

        let rt = RuntimeBuilder::new_multi_thread()
            .worker_threads(2)
            .thread_name("kural-tokio")
            .enable_all()
            .build()
            .map_err(|err| BackendError::Init(format!("failed to build tokio runtime: {err}")))?;

        // Seed hardware output mute + volume cleanly.
        let boot_audio = audio.clone();
        let boot_vol = prefs.volume;
        let boot_muted = prefs.muted;
        rt.spawn(async move {
            let _ = boot_audio.set_volume(boot_vol).await;
            let _ = boot_audio.set_muted(boot_muted).await;
        });

        let (loop_done_tx, loop_done_rx) = std::sync::mpsc::sync_channel::<()>(1);
        let controller = BackendController::new(
            config,
            corpus,
            catalog.clone(),
            user_state,
            audio.clone(),
            age_classifier,
            explainer,
            ai_worker,
            snapshot_tx,
            initial_snapshot,
            event_tx,
            pool,
            speech_caps.tts_model_fingerprint,
        );

        rt.spawn(async move {
            controller.run(cmd_rx).await;
            let _ = loop_done_tx.send(());
        });

        Ok(Self {
            inner: Arc::new(BackendHandleInner {
                cmd_tx,
                snapshot_rx,
                rms_rx,
                initial_events_rx: Mutex::new(Some(event_rx)),
                catalog,
                audio,
                runtime: Mutex::new(Some(rt)),
                loop_done_rx: Mutex::new(Some(loop_done_rx)),
            }),
        })
    }

    /// Dispatch a UI touch or voice command cleanly without blocking the caller thread.
    pub fn dispatch(&self, cmd: BackendCommand) {
        if let Err(mpsc::error::TrySendError::Full(msg)) =
            self.inner.cmd_tx.try_send(ControllerMsg::Command(cmd))
        {
            let tx = self.inner.cmd_tx.clone();
            if let Ok(guard) = self.inner.runtime.lock()
                && let Some(rt) = guard.as_ref()
            {
                rt.spawn(async move {
                    let _ = tx.send(msg).await;
                });
            }
        }
    }

    /// Return the latest immutable UI snapshot synchronously.
    #[must_use]
    pub fn current_snapshot(&self) -> Arc<UiSnapshot> {
        self.inner.snapshot_rx.borrow().clone()
    }

    /// Subscribe to coherent [`UiSnapshot`] updates via `tokio::sync::watch`.
    #[must_use]
    pub fn subscribe_snapshot(&self) -> watch::Receiver<Arc<UiSnapshot>> {
        self.inner.snapshot_rx.clone()
    }

    /// Subscribe to bounded transient UI events (`64` capacity).
    #[must_use]
    pub fn subscribe_events(&self) -> mpsc::Receiver<UiTransientEvent> {
        if let Ok(mut guard) = self.inner.initial_events_rx.lock()
            && let Some(rx) = guard.take()
        {
            return rx;
        }
        let (tx, rx) = mpsc::channel(EVENT_QUEUE_CAP);
        let _ = self
            .inner
            .cmd_tx
            .try_send(ControllerMsg::AddEventSubscriber(tx));
        rx
    }

    /// Subscribe to `30 Hz` live microphone RMS amplitude `[0.0, 1.0]`.
    #[must_use]
    pub fn subscribe_rms(&self) -> watch::Receiver<f32> {
        self.inner.rms_rx.clone()
    }

    /// Cached read-only corpus hierarchy for Search Divisions view (zero DB import in `kural-app`).
    #[must_use]
    pub fn catalog(&self) -> &CorpusCatalog {
        &self.inner.catalog
    }

    /// Fetch ordered Kural summaries inside an Adhikaram for Search modal drill-down without
    /// exposing `kural-db` to `kural-app`.
    pub fn kurals_in_adhikaram(&self, adhikaram_id: u8) -> Result<Vec<KuralSummary>, BackendError> {
        let (reply_tx, reply_rx) = std::sync::mpsc::sync_channel(1);
        self.inner
            .cmd_tx
            .try_send(ControllerMsg::QueryAdhikaramKurals {
                adhikaram_id,
                reply: reply_tx,
            })
            .map_err(|_| BackendError::Closed)?;
        reply_rx.recv().map_err(|_| BackendError::Closed)?
    }

    /// Gracefully flush pending state, shut down workers, audio actor, and Tokio runtime.
    pub fn shutdown(self) {
        self.shutdown_ref();
    }

    /// Idempotently shut down backend workers and flush pending state via shared handle reference.
    /// Safe to invoke from inside an active Tokio runtime (e.g., Dioxus cleanup callbacks).
    pub fn shutdown_ref(&self) {
        if tokio::runtime::Handle::try_current().is_ok() {
            let inner = Arc::clone(&self.inner);
            let _ = std::thread::spawn(move || Self::shutdown_inner(&inner)).join();
        } else {
            Self::shutdown_inner(&self.inner);
        }
    }

    fn shutdown_inner(inner: &BackendHandleInner) {
        let (tx, rx) = std::sync::mpsc::sync_channel(1);
        if inner.cmd_tx.try_send(ControllerMsg::Shutdown(tx)).is_ok() {
            let _ = rx.recv_timeout(Duration::from_secs(10));
        }
        if let Ok(mut done_guard) = inner.loop_done_rx.lock()
            && let Some(done_rx) = done_guard.take()
        {
            let _ = done_rx.recv_timeout(Duration::from_secs(10));
        }
        inner.audio.shutdown();
        if let Ok(mut rt_guard) = inner.runtime.lock()
            && let Some(rt) = rt_guard.take()
        {
            rt.shutdown_timeout(Duration::from_secs(5));
        }
    }
}

struct BackendController {
    config: AppConfig,
    corpus: KuralDb,
    catalog: CorpusCatalog,
    user_state: UserStateStore,
    audio: AudioHandle,
    age_classifier: Arc<AgeClassifier>,
    explainer: Arc<SemanticExplainer>,
    ai_worker: AiWorkerHandle,
    snapshot_tx: watch::Sender<Arc<UiSnapshot>>,
    state: UiSnapshot,
    event_sinks: Vec<mpsc::Sender<UiTransientEvent>>,
    internal_tx: mpsc::Sender<InternalMsg>,
    internal_rx: mpsc::Receiver<InternalMsg>,
    pool: Vec<u16>,
    last_ai_answer: Option<GroundedAnswer>,

    nav_epoch: u64,
    voice_session_id: u64,
    tts_epoch: u64,
    ai_epoch: u64,
    search_epoch: u64,
    next_voice_turn_id: u64,
    pending_loop_rearm: Option<u64>,
    consecutive_silence_timeouts: u8,
    active_voice_task: Option<(JoinHandle<()>, CancellationToken)>,
    active_tts_task: Option<(JoinHandle<()>, CancellationToken)>,
    listening_entered_at: Option<Instant>,
    prefs_dirty_deadline: Option<tokio::time::Instant>,
    active_content_key: Option<ContentAudioKey>,
    active_tts_model_fingerprint: u64,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct ContentAudioKey {
    screen: AppScreen,
    kural_number: u16,
    lang: kural_core::Language,
    effective_scholar: Scholar,
    target: kural_core::SpeakTarget,
}

impl BackendController {
    #[allow(clippy::too_many_arguments)]
    fn new(
        config: AppConfig,
        corpus: KuralDb,
        catalog: CorpusCatalog,
        user_state: UserStateStore,
        audio: AudioHandle,
        age_classifier: Arc<AgeClassifier>,
        explainer: Arc<SemanticExplainer>,
        ai_worker: AiWorkerHandle,
        snapshot_tx: watch::Sender<Arc<UiSnapshot>>,
        initial_snapshot: Arc<UiSnapshot>,
        initial_event_tx: mpsc::Sender<UiTransientEvent>,
        pool: Vec<u16>,
        active_tts_model_fingerprint: u64,
    ) -> Self {
        let (internal_tx, internal_rx) = mpsc::channel(INTERNAL_QUEUE_CAP);
        Self {
            config,
            corpus,
            catalog,
            user_state,
            audio,
            age_classifier,
            explainer,
            ai_worker,
            snapshot_tx,
            state: (*initial_snapshot).clone(),
            event_sinks: vec![initial_event_tx],
            internal_tx,
            internal_rx,
            pool,
            last_ai_answer: None,
            nav_epoch: 0,
            voice_session_id: 0,
            tts_epoch: 0,
            ai_epoch: 0,
            search_epoch: 0,
            next_voice_turn_id: 1,
            pending_loop_rearm: None,
            consecutive_silence_timeouts: 0,
            active_voice_task: None,
            active_tts_task: None,
            listening_entered_at: None,
            prefs_dirty_deadline: None,
            active_content_key: None,
            active_tts_model_fingerprint,
        }
    }

    fn apply_speech_caps(&mut self, caps: SpeechCapabilities) {
        self.state.system_health.active_stt_loaded = caps.active_stt_loaded;
        self.state.system_health.active_tts_loaded = caps.active_tts_loaded;
        self.active_tts_model_fingerprint = caps.tts_model_fingerprint;
    }

    async fn run(mut self, mut cmd_rx: mpsc::Receiver<ControllerMsg>) {
        let mut audio_state_rx = self.audio.subscribe_state();

        loop {
            let flush_fut = async {
                match self.prefs_dirty_deadline {
                    Some(deadline) => tokio::time::sleep_until(deadline).await,
                    None => std::future::pending::<()>().await,
                }
            };

            tokio::select! {
                maybe_msg = cmd_rx.recv() => {
                    match maybe_msg {
                        Some(ControllerMsg::Command(cmd)) => {
                            self.handle_command(cmd);
                        }
                        Some(ControllerMsg::AddEventSubscriber(tx)) => {
                            self.event_sinks.push(tx);
                        }
                        Some(ControllerMsg::QueryAdhikaramKurals { adhikaram_id, reply }) => {
                            let corpus = self.corpus.clone();
                            tokio::task::spawn_blocking(move || {
                                let res = corpus
                                    .get_kurals_by_adhikaram(adhikaram_id)
                                    .map_err(BackendError::from);
                                let _ = reply.send(res);
                            });
                        }
                        Some(ControllerMsg::Shutdown(reply)) => {
                            self.cancel_active_capture();
                            self.cancel_active_tts();
                            self.flush_prefs_now_sync();
                            self.ai_worker.shutdown();
                            let _ = reply.send(());
                            break;
                        }
                        None => {
                            self.flush_prefs_now_sync();
                            self.ai_worker.shutdown();
                            break;
                        }
                    }
                }
                maybe_internal = self.internal_rx.recv() => {
                    if let Some(msg) = maybe_internal {
                        self.handle_internal(msg);
                    }
                }
                changed = audio_state_rx.changed() => {
                    if changed.is_ok() {
                        let new_state = *audio_state_rx.borrow_and_update();
                        if new_state != self.state.audio_state {
                            if new_state == AudioState::Listening {
                                self.listening_entered_at = Some(Instant::now());
                            } else {
                                self.listening_entered_at = None;
                            }
                            self.state.audio_state = new_state;
                            if new_state == AudioState::PlayingTts
                                && self.state.playback_status == AudioPlaybackStatus::Loading
                            {
                                self.state.playback_status = AudioPlaybackStatus::Playing;
                            }
                            self.state.system_health.audio_ok = self.audio.is_healthy();
                            self.publish_snapshot();
                            if new_state == AudioState::Idle
                                && let Some(expected_tts_epoch) = self.pending_loop_rearm.take()
                                && expected_tts_epoch == self.tts_epoch
                                && self.state.voice_to_voice_loop
                                && self.active_tts_task.is_none()
                                && self.active_voice_task.is_none()
                            {
                                self.start_voice_command();
                            }
                        }
                    }
                }
                () = flush_fut => {
                    self.prefs_dirty_deadline = None;
                    self.flush_prefs_async();
                }
            }
        }
    }

    fn handle_command(&mut self, cmd: BackendCommand) {
        match cmd {
            BackendCommand::SetLanguage(lang) => {
                self.bump_nav_interrupt();
                if self.state.prefs.lang != lang {
                    self.state.prefs.lang = lang;
                    self.flush_prefs_async();
                    self.publish_snapshot();
                    let worker = self.ai_worker.clone();
                    let internal_tx = self.internal_tx.clone();
                    tokio::spawn(async move {
                        match worker.ensure_language(lang).await {
                            Ok(caps) => {
                                let _ =
                                    internal_tx.send(InternalMsg::SpeechCapsUpdated(caps)).await;
                            }
                            Err(err) => {
                                tracing::warn!(%err, %lang, "Failed to switch STT/TTS language");
                            }
                        }
                    });
                    if self.state.screen != AppScreen::Start {
                        self.trigger_auto_playback_for_current_content(false);
                    }
                }
            }
            BackendCommand::SetTheme(theme) if self.state.prefs.theme != theme => {
                self.state.prefs.theme = theme;
                self.flush_prefs_async();
                self.publish_snapshot();
            }
            BackendCommand::SetTheme(_) => {}
            BackendCommand::Navigate(screen) => {
                self.disarm_voice_loop();
                self.bump_nav_interrupt();
                self.state.modal = None;
                match screen {
                    AppScreen::Start => {
                        self.state.screen = AppScreen::Start;
                        self.active_content_key = None;
                        self.state.can_replay_tts = false;
                        self.state.playback_status = AudioPlaybackStatus::Idle;
                        self.flush_prefs_async();
                        self.publish_snapshot();
                    }
                    AppScreen::Kids => self.switch_age_screen(AgeGroup::Kids),
                    AppScreen::Youth => self.switch_age_screen(AgeGroup::Youth),
                    AppScreen::Adult => self.switch_age_screen(AgeGroup::Adult),
                }
            }
            BackendCommand::SelectAge(age) => {
                self.disarm_voice_loop();
                self.bump_nav_interrupt();
                self.state.modal = None;
                self.switch_age_screen(age);
            }
            BackendCommand::OpenModal(kind) => {
                if !matches!(kind, ModalKind::AiChat { .. }) {
                    self.disarm_voice_loop();
                }
                self.state.modal = Some(kind);
                self.publish_snapshot();
                if matches!(kind, ModalKind::KidsStory | ModalKind::YouthDeepDive) {
                    self.trigger_auto_playback_for_current_content(false);
                }
            }
            BackendCommand::CloseModal if self.state.modal.is_some() => {
                let prev = self.state.modal.take();
                if matches!(prev, Some(ModalKind::AiChat { .. })) {
                    self.disarm_voice_loop();
                }
                if matches!(prev, Some(ModalKind::KidsStory | ModalKind::AiChat { .. })) {
                    self.cancel_active_tts();
                    self.active_content_key = None;
                    self.state.can_replay_tts = false;
                    self.state.playback_status = if self.state.screen == AppScreen::Start {
                        AudioPlaybackStatus::Idle
                    } else {
                        AudioPlaybackStatus::FallbackReady
                    };
                }
                self.publish_snapshot();
            }
            BackendCommand::CloseModal => {}
            BackendCommand::EnterStartAutoListen => {
                if self.state.screen != AppScreen::Start
                    || self.active_voice_task.is_some()
                    || !self.audio.health().input_ok
                {
                    return;
                }
                self.start_voice_age_detect(true);
            }
            BackendCommand::StartVoiceAgeDetect => self.start_voice_age_detect(false),
            BackendCommand::StartVoiceCommand => self.start_voice_command(),
            BackendCommand::CancelVoice => {
                self.disarm_voice_loop();
                self.bump_nav_interrupt();
                self.publish_snapshot();
            }
            BackendCommand::SubmitTextSearch { query, division_id } => {
                self.run_text_search(query, division_id);
            }
            BackendCommand::OpenKural(num) => {
                if !(1..=1330).contains(&num) {
                    self.emit_event(UiTransientEvent::ErrorToast {
                        message_ta: VOICE_KURAL_RANGE_ERROR.ta.to_string(),
                        message_en: VOICE_KURAL_RANGE_ERROR.en.to_string(),
                    });
                    return;
                }
                self.bump_nav_interrupt();
                self.open_kural_with_pool_fixup(num, true);
            }
            BackendCommand::NextKural => {
                self.bump_nav_interrupt();
                if !self.pool.is_empty() {
                    let next_idx = (self.state.pool_idx + 1) % self.pool.len();
                    let num = self.pool[next_idx];
                    self.open_kural_in_current_pool(num, next_idx, true);
                }
            }
            BackendCommand::PrevKural => {
                self.bump_nav_interrupt();
                if !self.pool.is_empty() {
                    let prev_idx = (self.state.pool_idx + self.pool.len() - 1) % self.pool.len();
                    let num = self.pool[prev_idx];
                    self.open_kural_in_current_pool(num, prev_idx, true);
                }
            }
            BackendCommand::OpenAdhikaram(ad_id) => {
                let Some(ad) = self.catalog.adhikarams.iter().find(|a| a.id == ad_id) else {
                    self.emit_event(UiTransientEvent::ErrorToast {
                        message_ta: VOICE_ADHIKARAM_RANGE_ERROR.ta.to_string(),
                        message_en: VOICE_ADHIKARAM_RANGE_ERROR.en.to_string(),
                    });
                    return;
                };
                let start_num = ad.kural_start;
                self.bump_nav_interrupt();
                self.state.active_filter = None;
                if matches!(self.state.modal, Some(ModalKind::Search { .. })) {
                    self.state.modal = None;
                }
                self.restore_default_pool_and_open(start_num, true);
            }
            BackendCommand::FilterByDivision(div_opt) => match div_opt {
                Some(d) => self.apply_filter(ActiveFilter::Division(d)),
                None => self.clear_filter(),
            },
            BackendCommand::FilterByTag(tag_opt) => match tag_opt {
                Some(t) => self.apply_filter(ActiveFilter::Tag(t)),
                None => self.clear_filter(),
            },
            BackendCommand::FilterByResearchTheme(theme_opt) => match theme_opt {
                Some(rt) => self.apply_filter(ActiveFilter::ResearchTheme(rt)),
                None => self.clear_filter(),
            },
            BackendCommand::ClearFilter => self.clear_filter(),
            BackendCommand::SelectCommentary(scholar) => {
                let prev_effective = self.state.effective_scholar;
                self.state.requested_scholar = scholar;
                self.update_effective_scholar_and_notify(true);
                if self.state.effective_scholar != prev_effective {
                    self.cancel_active_tts();
                    self.active_content_key = None;
                }
                self.publish_snapshot();
                if self.state.screen == AppScreen::Adult {
                    self.trigger_auto_playback_for_current_content(false);
                }
            }
            BackendCommand::SpeakCurrent(target) => {
                // Child Mic -> Listen accidental collision guard (400 ms).
                if self.state.audio_state == AudioState::Listening
                    && let Some(entered) = self.listening_entered_at
                    && entered.elapsed()
                        < Duration::from_millis(self.config.audio.mic_to_listen_guard_ms)
                {
                    return;
                }
                if matches!(
                    self.state.playback_status,
                    AudioPlaybackStatus::Loading
                        | AudioPlaybackStatus::Playing
                        | AudioPlaybackStatus::Replaying
                ) {
                    return;
                }
                self.cancel_active_capture();
                self.cancel_active_tts();
                self.nav_epoch = self.nav_epoch.wrapping_add(1);
                let normalized_target = if self.state.screen == AppScreen::Adult
                    && let kural_core::SpeakTarget::ScholarCommentary(s) = target
                    && (s == self.state.effective_scholar || s == self.state.requested_scholar)
                {
                    kural_core::SpeakTarget::KuralAndMeaning
                } else {
                    target
                };
                let key_target =
                    if let kural_core::SpeakTarget::ScholarCommentary(s) = normalized_target {
                        kural_core::SpeakTarget::ScholarCommentary(resolve_effective_scholar(
                            &self.state.current_kural,
                            s,
                        ))
                    } else {
                        normalized_target
                    };
                self.active_content_key = Some(self.content_audio_key(key_target));
                self.start_speak_target(normalized_target);
            }
            BackendCommand::SpeakCustomText { text, lang, label } => {
                self.speak_custom_text(text, lang, label);
            }
            BackendCommand::ReplayAudio | BackendCommand::ReplayLastTts => {
                self.start_replay_last_tts();
            }
            BackendCommand::ReplayVoiceTurn { turn_id, mode } => {
                self.start_replay_voice_turn(turn_id, mode);
            }
            BackendCommand::SetAutoSpeakText(enabled) => {
                if self.state.prefs.auto_speak_text != enabled
                    || self.state.auto_speak_text != enabled
                {
                    self.state.prefs.auto_speak_text = enabled;
                    self.state.auto_speak_text = enabled;
                    self.flush_prefs_async();
                    self.publish_snapshot();
                }
                let notice = if enabled {
                    AUTO_SPEAK_ON_NOTICE
                } else {
                    AUTO_SPEAK_OFF_NOTICE
                };
                self.emit_event(UiTransientEvent::InfoToast {
                    message_ta: notice.ta.to_string(),
                    message_en: notice.en.to_string(),
                });
            }
            BackendCommand::SetVoiceToVoiceLoop(enabled) => {
                self.set_voice_to_voice_loop(enabled);
            }
            BackendCommand::StopAudio => {
                let was_loop = self.state.voice_to_voice_loop;
                self.disarm_voice_loop();
                self.bump_nav_interrupt();
                self.state.playback_status = if self.state.can_replay_tts {
                    AudioPlaybackStatus::Completed
                } else if self.state.screen != AppScreen::Start {
                    AudioPlaybackStatus::FallbackReady
                } else {
                    AudioPlaybackStatus::Idle
                };
                self.publish_snapshot();
                if was_loop {
                    self.emit_event(UiTransientEvent::InfoToast {
                        message_ta: VOICE_LOOP_OFF_NOTICE.ta.to_string(),
                        message_en: VOICE_LOOP_OFF_NOTICE.en.to_string(),
                    });
                }
            }
            BackendCommand::SetSpeechSpeed(speed) => {
                let snapped = snap_speech_speed(speed);
                if (self.state.prefs.speech_speed - snapped).abs() > 1e-4 {
                    self.state.prefs.speech_speed = snapped;
                    self.flush_prefs_async();
                    self.publish_snapshot();
                }
            }
            BackendCommand::SetMuted(muted) => {
                self.state.prefs.muted = muted;
                let audio = self.audio.clone();
                tokio::spawn(async move {
                    let _ = audio.set_muted(muted).await;
                });
                self.flush_prefs_async();
                self.publish_snapshot();
            }
            BackendCommand::AdjustVolume(delta) if delta.is_finite() => {
                let next_vol = (self.state.prefs.volume + delta).clamp(0.0, 1.0);
                self.state.prefs.volume = next_vol;
                let audio = self.audio.clone();
                tokio::spawn(async move {
                    let _ = audio.set_volume(next_vol).await;
                });
                self.flush_prefs_async();
                self.publish_snapshot();
            }
            BackendCommand::AdjustVolume(_) => {}
            BackendCommand::SubmitKidsGameAnswer {
                kural_number,
                choice_idx,
            } => self.handle_kids_game_answer(kural_number, choice_idx),
            BackendCommand::CompleteKidsStory(kural_number) => {
                self.handle_complete_kids_story(kural_number);
            }
            BackendCommand::SubmitYouthQuizAnswer {
                kural_number,
                choice_idx,
            } => self.handle_youth_quiz_answer(kural_number, choice_idx),
            BackendCommand::ToggleBookmark(kural_number) => {
                self.handle_toggle_bookmark(kural_number);
            }
            BackendCommand::AskAiQuestion { query, seed_kural } => {
                self.run_text_ai_question(query, seed_kural);
            }
            _ => {}
        }
    }

    fn handle_internal(&mut self, msg: InternalMsg) {
        match msg {
            InternalMsg::SpeechCapsUpdated(caps) => {
                self.apply_speech_caps(caps);
                self.publish_snapshot();
            }
            InternalMsg::StartVoiceAutoRouteDone {
                session_id,
                nav_epoch,
                lang,
                auto_triggered,
                age_timed_out,
                outcome,
                transcript,
                caps,
            } => {
                self.handle_start_voice_auto_route_done(StartVoiceAutoRouteData {
                    session_id,
                    nav_epoch,
                    lang,
                    auto_triggered,
                    age_timed_out,
                    outcome,
                    transcript,
                    caps,
                });
            }
            InternalMsg::VoiceCmdTranscribed {
                session_id,
                nav_epoch,
                lang,
                is_timeout,
                captured_user_16k,
                transcript,
                caps,
            } => {
                if let Some(c) = caps {
                    self.apply_speech_caps(c);
                }
                if session_id != self.voice_session_id || nav_epoch != self.nav_epoch {
                    return;
                }
                self.active_voice_task = None;
                match transcript {
                    Ok(raw) => self.route_recognized_voice(
                        raw,
                        lang,
                        captured_user_16k,
                        session_id,
                        nav_epoch,
                    ),
                    Err(err_msg) => {
                        if is_timeout && self.state.voice_to_voice_loop {
                            self.consecutive_silence_timeouts =
                                self.consecutive_silence_timeouts.saturating_add(1);
                            if self.consecutive_silence_timeouts
                                >= self.config.audio.voice_loop_max_silence_timeouts
                            {
                                self.disarm_voice_loop();
                                self.publish_snapshot();
                                self.emit_event(UiTransientEvent::InfoToast {
                                    message_ta: VOICE_LOOP_SILENCE_STOP_NOTICE.ta.to_string(),
                                    message_en: VOICE_LOOP_SILENCE_STOP_NOTICE.en.to_string(),
                                });
                            } else {
                                self.rearm_voice_loop_when_ready(self.tts_epoch);
                            }
                            return;
                        }
                        if self.state.voice_to_voice_loop {
                            self.disarm_voice_loop();
                            self.publish_snapshot();
                        }
                        let audio = self.audio.clone();
                        tokio::spawn(async move {
                            let _ = audio.play_earcon(EarconKind::RetryTone).await;
                        });
                        self.emit_event(UiTransientEvent::ErrorToast {
                            message_ta: err_msg.ta,
                            message_en: err_msg.en,
                        });
                    }
                }
            }
            InternalMsg::VoiceAiDone {
                session_id,
                nav_epoch,
                turn_id,
                seed_kural,
                lang,
                result,
            } => {
                if session_id != self.voice_session_id || nav_epoch != self.nav_epoch {
                    return;
                }
                match result {
                    Ok(ans) => {
                        let oos = ans.out_of_scope;
                        let spoken_text = if oos {
                            match lang {
                                kural_core::Language::Tamil => OOS_VOICE_TTS_REFUSAL.ta.to_string(),
                                kural_core::Language::English => {
                                    OOS_VOICE_TTS_REFUSAL.en.to_string()
                                }
                            }
                        } else {
                            let primary = match lang {
                                kural_core::Language::Tamil => ans.answer_ta.trim(),
                                kural_core::Language::English => ans.answer_en.trim(),
                            };
                            if !primary.is_empty() {
                                primary.to_string()
                            } else if !ans.answer_en.trim().is_empty() {
                                ans.answer_en.trim().to_string()
                            } else {
                                ans.answer_ta.trim().to_string()
                            }
                        };
                        let cited_nums: Vec<u16> =
                            ans.cited_kurals.iter().map(|k| k.kural.number).collect();
                        if let Some(tid) = turn_id
                            && let Some(turn) = self
                                .state
                                .recent_voice_turns
                                .iter_mut()
                                .find(|t| t.id == tid)
                        {
                            turn.ai_text = spoken_text.clone();
                            turn.cited_kurals = cited_nums;
                        }
                        self.last_ai_answer = Some(ans.clone());
                        self.state.modal = Some(ModalKind::AiChat { seed_kural });
                        self.publish_snapshot();
                        self.emit_event(UiTransientEvent::AiAnswerReady(ans));
                        let spoken_lang = detect_text_language(&spoken_text, lang);
                        self.speak_raw_chunks(
                            vec![spoken_text],
                            spoken_lang,
                            kural_core::SpeakTarget::AiAnswer,
                            4,
                            turn_id,
                            self.state.voice_to_voice_loop,
                        );
                    }
                    Err(err) => {
                        if self.state.voice_to_voice_loop {
                            self.disarm_voice_loop();
                            self.publish_snapshot();
                        }
                        self.emit_event(UiTransientEvent::ErrorToast {
                            message_ta: err.ta,
                            message_en: err.en,
                        });
                    }
                }
            }
            InternalMsg::TextAiDone { ai_epoch, result } => {
                if ai_epoch != self.ai_epoch {
                    return;
                }
                match result {
                    Ok(ans) => {
                        self.last_ai_answer = Some(ans.clone());
                        if self.state.prefs.auto_speak_text && !ans.out_of_scope {
                            let lang = self.state.prefs.lang;
                            let primary = match lang {
                                kural_core::Language::Tamil => ans.answer_ta.trim(),
                                kural_core::Language::English => ans.answer_en.trim(),
                            };
                            let text = if !primary.is_empty() {
                                primary.to_string()
                            } else if !ans.answer_en.trim().is_empty() {
                                ans.answer_en.trim().to_string()
                            } else {
                                ans.answer_ta.trim().to_string()
                            };
                            if !text.is_empty() {
                                let spoken_lang = detect_text_language(&text, lang);
                                self.speak_custom_text(text, spoken_lang, None);
                            } else {
                                self.state.can_replay_tts = false;
                                self.state.playback_status = AudioPlaybackStatus::FallbackReady;
                                self.publish_snapshot();
                            }
                        } else if !ans.out_of_scope {
                            self.state.can_replay_tts = false;
                            self.state.playback_status = AudioPlaybackStatus::FallbackReady;
                            self.publish_snapshot();
                        }
                        self.emit_event(UiTransientEvent::AiAnswerReady(ans));
                    }
                    Err(err) => {
                        self.emit_event(UiTransientEvent::ErrorToast {
                            message_ta: err.ta,
                            message_en: err.en,
                        });
                    }
                }
            }
            InternalMsg::SearchDone {
                search_epoch,
                result,
            } => {
                if search_epoch != self.search_epoch {
                    return;
                }
                match result {
                    Ok(outcome) => self.emit_event(UiTransientEvent::SearchResultsReady(outcome)),
                    Err(err) => self.emit_event(UiTransientEvent::ErrorToast {
                        message_ta: format!("தேடல் பிழை: {err}"),
                        message_en: format!("Search failed: {err}"),
                    }),
                }
            }
            InternalMsg::TtsFinished {
                tts_epoch,
                nav_epoch,
                target,
                kural_number,
                voice_turn_id,
                has_saved_audio,
                voice_loop_rearm,
                ok,
                error_msg,
            } => {
                if tts_epoch != self.tts_epoch || nav_epoch != self.nav_epoch {
                    return;
                }
                self.active_tts_task = None;
                self.state.active_replay_label = None;
                if ok {
                    if has_saved_audio {
                        self.state.has_replay_clip = true;
                        self.state.can_replay_tts = true;
                    }
                    if let Some(tid) = voice_turn_id
                        && has_saved_audio
                        && let Some(turn) = self
                            .state
                            .recent_voice_turns
                            .iter_mut()
                            .find(|t| t.id == tid)
                    {
                        turn.has_ai_audio = true;
                        self.state.can_replay_voice_turn = true;
                    }
                    self.state.playback_status = if self.state.can_replay_tts {
                        AudioPlaybackStatus::Completed
                    } else if self.state.screen != AppScreen::Start {
                        AudioPlaybackStatus::FallbackReady
                    } else {
                        AudioPlaybackStatus::Idle
                    };
                    self.publish_snapshot();
                    if matches!(target, kural_core::SpeakTarget::KuttyStory) {
                        self.handle_complete_kids_story(kural_number);
                    }
                    if voice_loop_rearm && self.state.voice_to_voice_loop {
                        self.rearm_voice_loop_when_ready(tts_epoch);
                    }
                } else {
                    self.state.playback_status = if self.state.can_replay_tts {
                        AudioPlaybackStatus::Completed
                    } else if self.state.screen != AppScreen::Start {
                        AudioPlaybackStatus::FallbackReady
                    } else {
                        AudioPlaybackStatus::Idle
                    };
                    self.publish_snapshot();
                    if let Some(msg) = error_msg {
                        self.emit_event(UiTransientEvent::ErrorToast {
                            message_ta: msg.ta,
                            message_en: msg.en,
                        });
                    }
                }
            }
        }
    }

    fn bump_nav_interrupt(&mut self) {
        self.nav_epoch = self.nav_epoch.wrapping_add(1);
        self.pending_loop_rearm = None;
        self.state.active_replay_label = None;
        self.cancel_active_capture();
        self.cancel_active_tts();
    }

    fn cancel_active_capture(&mut self) {
        self.voice_session_id = self.voice_session_id.wrapping_add(1);
        if let Some((handle, token)) = self.active_voice_task.take() {
            token.cancel();
            handle.abort();
        }
        self.audio.cancel_capture_nowait();
    }

    fn cancel_active_tts(&mut self) {
        self.tts_epoch = self.tts_epoch.wrapping_add(1);
        if let Some((handle, token)) = self.active_tts_task.take() {
            token.cancel();
            handle.abort();
        }
        self.audio.stop_playback_nowait();
    }

    fn switch_age_screen(&mut self, age: AgeGroup) {
        self.state.screen = AppScreen::from(age);
        self.state.modal = None;
        self.state.prefs.default_age = age;
        self.state.active_filter = None;

        let target_num = self.state.prefs.last_kural_for(age);
        let kids_only = age == AgeGroup::Kids;
        let pool = default_pool_or_fallback(&self.corpus, self.state.screen, target_num)
            .unwrap_or_else(|_| (1..=1330).collect());
        self.pool = pool;

        let open_num = if self.pool.contains(&target_num) {
            target_num
        } else {
            self.pool.first().copied().unwrap_or(1)
        };
        let _ = kids_only;
        if let Ok(full) = self.corpus.get_kural_full(open_num) {
            self.state.current_kural = full;
            self.state.pool_len = self.pool.len();
            self.state.pool_idx = find_kural_idx(&self.pool, open_num);
            self.state.prefs.set_last_kural_for(age, open_num);
            self.update_effective_scholar_and_notify(self.state.screen == AppScreen::Adult);
        }
        self.flush_prefs_async();
        self.publish_snapshot();
        self.trigger_auto_playback_for_current_content(false);
    }

    fn open_kural_with_pool_fixup(&mut self, num: u16, speak_after: bool) {
        if let Some(pos) = self.pool.iter().position(|&n| n == num) {
            self.open_kural_in_current_pool(num, pos, speak_after);
            return;
        }
        self.state.active_filter = None;
        self.restore_default_pool_and_open(num, speak_after);
    }

    fn restore_default_pool_and_open(&mut self, num: u16, speak_after: bool) {
        let mut pool = default_pool_or_fallback(&self.corpus, self.state.screen, num)
            .unwrap_or_else(|_| (1..=1330).collect());
        if !pool.contains(&num) {
            pool = (1..=1330).collect();
        }
        self.pool = pool;
        let idx = find_kural_idx(&self.pool, num);
        self.open_kural_in_current_pool(num, idx, speak_after);
    }

    fn open_kural_in_current_pool(&mut self, num: u16, idx: usize, speak_after: bool) {
        match self.corpus.get_kural_full(num) {
            Ok(full) => {
                self.state.current_kural = full;
                self.state.pool_len = self.pool.len();
                self.state.pool_idx = idx;
                let active_age =
                    active_age_for_screen(self.state.screen, self.state.prefs.default_age);
                self.state.prefs.set_last_kural_for(active_age, num);
                self.schedule_prefs_debounce();
                self.update_effective_scholar_and_notify(self.state.screen == AppScreen::Adult);
                self.publish_snapshot();
                if speak_after {
                    self.trigger_auto_playback_for_current_content(false);
                }
            }
            Err(err) => {
                self.emit_event(UiTransientEvent::ErrorToast {
                    message_ta: format!("குறள் #{num} ஏற்ற முடியவில்லை: {err}"),
                    message_en: format!("Unable to load Kural #{num}: {err}"),
                });
            }
        }
    }

    fn force_speak_kural_and_meaning(&mut self) {
        let target = kural_core::SpeakTarget::KuralAndMeaning;
        self.active_content_key = Some(self.content_audio_key(target));
        self.state.can_replay_tts = false;
        self.state.playback_status = AudioPlaybackStatus::Loading;
        self.publish_snapshot();
        self.start_speak_target(target);
    }

    fn apply_filter(&mut self, filter: ActiveFilter) {
        self.bump_nav_interrupt();
        let kids_only = self.state.screen == AppScreen::Kids;
        let mut pool = self
            .corpus
            .get_pool_numbers_for_filter(Some(&filter), kids_only)
            .unwrap_or_default();
        if pool.is_empty() && kids_only {
            pool = self
                .corpus
                .get_pool_numbers_for_filter(Some(&filter), false)
                .unwrap_or_default();
        }
        if pool.is_empty() {
            self.emit_event(UiTransientEvent::InfoToast {
                message_ta: "இந்த வடிகட்டியில் குறள்கள் இல்லை.".to_string(),
                message_en: "No Kurals matched this filter.".to_string(),
            });
            return;
        }
        self.pool = pool;
        self.state.active_filter = Some(filter);
        let first = self.pool[0];
        self.open_kural_in_current_pool(first, 0, true);
    }

    fn clear_filter(&mut self) {
        self.bump_nav_interrupt();
        self.state.active_filter = None;
        let current_num = self.state.current_kural.kural.number;
        let kids_only = self.state.screen == AppScreen::Kids;
        let mut pool = self
            .corpus
            .get_pool_numbers_for_filter(None, kids_only)
            .unwrap_or_else(|_| (1..=1330).collect());
        let open_num = if pool.contains(&current_num) {
            current_num
        } else if !kids_only {
            pool = (1..=1330).collect();
            current_num
        } else {
            pool.first().copied().unwrap_or(1)
        };
        self.pool = pool;
        let idx = find_kural_idx(&self.pool, open_num);
        if open_num == self.state.current_kural.kural.number {
            self.state.pool_len = self.pool.len();
            self.state.pool_idx = idx;
            self.publish_snapshot();
        } else {
            self.open_kural_in_current_pool(open_num, idx, true);
        }
    }

    fn update_effective_scholar_and_notify(&mut self, notify: bool) {
        let req = self.state.requested_scholar;
        let eff = resolve_effective_scholar(&self.state.current_kural, req);
        self.state.effective_scholar = eff;
        if notify && req == Scholar::Parimel && eff != Scholar::Parimel {
            self.emit_event(UiTransientEvent::CommentaryFallbackNotice {
                requested: req,
                effective: eff,
                notice_ta: PARIMEL_FALLBACK_NOTICE.ta.to_string(),
                notice_en: PARIMEL_FALLBACK_NOTICE.en.to_string(),
            });
        }
    }

    fn start_voice_age_detect(&mut self, auto_triggered: bool) {
        self.cancel_active_tts();
        self.cancel_active_capture();
        self.nav_epoch = self.nav_epoch.wrapping_add(1);
        let session_id = self.voice_session_id;
        let nav_epoch = self.nav_epoch;
        let lang = self.state.prefs.lang;

        let audio = self.audio.clone();
        let clf = self.age_classifier.clone();
        let age_ai_cfg = self.config.age_ai.clone();
        let max_duration_ms = self.config.audio.max_command_duration_ms;
        let inference_timeout = Duration::from_millis(self.config.ai.inference_timeout_ms);
        let ai_worker = self.ai_worker.clone();
        let internal_tx = self.internal_tx.clone();
        let cancel = CancellationToken::new();
        let task_cancel = cancel.clone();

        let handle = tokio::spawn(async move {
            let captured = match audio
                .start_capture(CaptureMode::VadCommand { max_duration_ms })
                .await
            {
                Ok(c) => c,
                Err(AudioError::Cancelled | AudioError::CaptureInProgress) => return,
                Err(AudioError::Timeout) => {
                    if task_cancel.is_cancelled() {
                        return;
                    }
                    let _ = internal_tx
                        .send(InternalMsg::StartVoiceAutoRouteDone {
                            session_id,
                            nav_epoch,
                            lang,
                            auto_triggered,
                            age_timed_out: false,
                            outcome: AgeOutcome::Retry(RetryReason::TooShort),
                            transcript: None,
                            caps: None,
                        })
                        .await;
                    return;
                }
                Err(AudioError::DeviceUnavailable(_)) => {
                    if auto_triggered || task_cancel.is_cancelled() {
                        return;
                    }
                    let _ = internal_tx
                        .send(InternalMsg::StartVoiceAutoRouteDone {
                            session_id,
                            nav_epoch,
                            lang,
                            auto_triggered,
                            age_timed_out: false,
                            outcome: AgeOutcome::Retry(RetryReason::HardwareUnavailable),
                            transcript: None,
                            caps: None,
                        })
                        .await;
                    return;
                }
                Err(err) => {
                    tracing::warn!(%err, "Voice age capture error");
                    if auto_triggered || task_cancel.is_cancelled() {
                        return;
                    }
                    let _ = internal_tx
                        .send(InternalMsg::StartVoiceAutoRouteDone {
                            session_id,
                            nav_epoch,
                            lang,
                            auto_triggered,
                            age_timed_out: false,
                            outcome: AgeOutcome::Retry(RetryReason::TooShort),
                            transcript: None,
                            caps: None,
                        })
                        .await;
                    return;
                }
            };

            if task_cancel.is_cancelled() {
                return;
            }

            let mut age_timed_out = false;
            let (outcome, samples_16k) = match tokio::time::timeout(
                inference_timeout,
                tokio::task::spawn_blocking(move || {
                    let mut outcome = clf.classify(&captured);
                    if matches!(outcome, AgeOutcome::Retry(RetryReason::TooShort))
                        && captured.voiced_ms >= 400
                        && captured.snr_db.is_finite()
                        && captured.snr_db >= age_ai_cfg.min_snr_db
                        && captured.clipping_ratio.is_finite()
                        && captured.clipping_ratio < age_ai_cfg.max_clipping_ratio
                        && captured.rms_level.is_finite()
                        && captured.rms_level >= 0.005
                    {
                        let mut relaxed_cfg = age_ai_cfg;
                        relaxed_cfg.min_voiced_ms = 400;
                        outcome = AgeClassifier::new(relaxed_cfg).classify(&captured);
                    }
                    (outcome, captured.samples_16k)
                }),
            )
            .await
            {
                Ok(Ok(pair)) => pair,
                Ok(Err(join_err)) => {
                    tracing::warn!(%join_err, "Voice age classifier task failed");
                    (AgeOutcome::Retry(RetryReason::TooShort), Vec::new())
                }
                Err(_) => {
                    age_timed_out = true;
                    tracing::warn!(
                        timeout_ms = inference_timeout.as_millis() as u64,
                        "Voice age classifier timed out"
                    );
                    (AgeOutcome::Retry(RetryReason::TooShort), Vec::new())
                }
            };

            if task_cancel.is_cancelled() {
                return;
            }

            let mut transcript: Option<String> = None;
            let mut caps: Option<SpeechCapabilities> = None;
            if !samples_16k.is_empty()
                && !task_cancel.is_cancelled()
                && let Ok((txt, c)) = ai_worker.transcribe(samples_16k, lang).await
            {
                let trimmed = txt.trim();
                if !trimmed.is_empty() {
                    transcript = Some(trimmed.to_string());
                }
                caps = Some(c);
            }

            if task_cancel.is_cancelled() {
                return;
            }
            let _ = internal_tx
                .send(InternalMsg::StartVoiceAutoRouteDone {
                    session_id,
                    nav_epoch,
                    lang,
                    auto_triggered,
                    age_timed_out,
                    outcome,
                    transcript,
                    caps,
                })
                .await;
        });

        self.active_voice_task = Some((handle, cancel));
    }

    fn handle_start_voice_auto_route_done(&mut self, data: StartVoiceAutoRouteData) {
        if data.session_id != self.voice_session_id
            || data.nav_epoch != self.nav_epoch
            || self.state.screen != AppScreen::Start
        {
            return;
        }
        if let Some(c) = data.caps {
            self.apply_speech_caps(c);
        }
        self.active_voice_task = None;

        let Some(target_age) =
            resolve_start_voice_age_route(&data.outcome, data.transcript.as_deref())
        else {
            if data.caps.is_some() {
                self.publish_snapshot();
            }
            if !data.auto_triggered {
                self.audio.play_earcon_nowait(EarconKind::RetryTone);
                if data.age_timed_out {
                    let msg = ai_inference_timeout_msg(self.config.ai.inference_timeout_ms);
                    self.emit_event(UiTransientEvent::ErrorToast {
                        message_ta: msg.ta,
                        message_en: msg.en,
                    });
                }
            }
            self.emit_event(UiTransientEvent::AgeResult(data.outcome));
            return;
        };

        let effective_outcome = match data.outcome {
            AgeOutcome::Detected(mut det) => {
                if det.age != target_age {
                    det.age = target_age;
                    det.confidence = det.confidence.max(0.95);
                    det.low_confidence = false;
                }
                AgeOutcome::Detected(det)
            }
            AgeOutcome::Retry(_) => AgeOutcome::Detected(AgeDetection {
                age: target_age,
                confidence: 1.0,
                low_confidence: false,
                f0_hz: 0.0,
                formant_dispersion_hz: 0.0,
                vtl_cm: 0.0,
                latency_ms: 0,
            }),
        };

        self.audio.play_earcon_nowait(EarconKind::CmdRecognized);
        self.emit_event(UiTransientEvent::AgeResult(effective_outcome));

        let opened_copy = match target_age {
            AgeGroup::Kids => AGE_MODE_OPENED_KIDS,
            AgeGroup::Youth => AGE_MODE_OPENED_YOUTH,
            AgeGroup::Adult => AGE_MODE_OPENED_ADULT,
        };
        self.emit_event(UiTransientEvent::InfoToast {
            message_ta: opened_copy.ta.to_string(),
            message_en: opened_copy.en.to_string(),
        });

        self.switch_age_screen(target_age);

        let ctx = RouterContext {
            screen: self.state.screen,
            modal: self.state.modal,
            active_kural: self.state.current_kural.kural.number,
            active_scholar: self.state.effective_scholar,
            speech_speed: self.state.prefs.speech_speed,
            muted: self.state.prefs.muted,
        };

        if let Some(cmd) =
            extract_opening_followup_command(data.transcript.as_deref(), data.lang, ctx)
        {
            match cmd {
                BackendCommand::OpenKural(_)
                | BackendCommand::OpenAdhikaram(_)
                | BackendCommand::OpenModal(ModalKind::AiChat { .. }) => {
                    self.handle_command(cmd);
                }
                _ => {}
            }
        }
    }

    fn start_voice_command(&mut self) {
        self.cancel_active_tts();
        self.cancel_active_capture();
        self.nav_epoch = self.nav_epoch.wrapping_add(1);
        let session_id = self.voice_session_id;
        let nav_epoch = self.nav_epoch;
        let lang = self.state.prefs.lang;

        let audio = self.audio.clone();
        let max_duration_ms = self.config.audio.max_command_duration_ms;
        let internal_tx = self.internal_tx.clone();
        let cancel = CancellationToken::new();
        let task_cancel = cancel.clone();
        let ai_worker = self.ai_worker.clone();

        let handle = tokio::spawn(async move {
            let captured = match audio
                .start_capture(CaptureMode::VadCommand { max_duration_ms })
                .await
            {
                Ok(c) => c,
                Err(AudioError::Cancelled) => return,
                Err(AudioError::Timeout) => {
                    let retry = retry_copy(RetryReason::TooShort);
                    let _ = audio.play_earcon(EarconKind::RetryTone).await;
                    let _ = internal_tx
                        .send(InternalMsg::VoiceCmdTranscribed {
                            session_id,
                            nav_epoch,
                            lang,
                            is_timeout: true,
                            captured_user_16k: None,
                            transcript: Err(BilingualMsg {
                                ta: retry.ta.to_string(),
                                en: retry.en.to_string(),
                            }),
                            caps: None,
                        })
                        .await;
                    return;
                }
                Err(AudioError::DeviceUnavailable(_)) => {
                    let retry = retry_copy(RetryReason::HardwareUnavailable);
                    let _ = internal_tx
                        .send(InternalMsg::VoiceCmdTranscribed {
                            session_id,
                            nav_epoch,
                            lang,
                            is_timeout: false,
                            captured_user_16k: None,
                            transcript: Err(BilingualMsg {
                                ta: retry.ta.to_string(),
                                en: retry.en.to_string(),
                            }),
                            caps: None,
                        })
                        .await;
                    return;
                }
                Err(e) => {
                    let _ = internal_tx
                        .send(InternalMsg::VoiceCmdTranscribed {
                            session_id,
                            nav_epoch,
                            lang,
                            is_timeout: false,
                            captured_user_16k: None,
                            transcript: Err(BilingualMsg {
                                ta: format!("குரல் உணர்வில் பிழை: {e}"),
                                en: format!("Voice recognition error: {e}"),
                            }),
                            caps: None,
                        })
                        .await;
                    return;
                }
            };

            if task_cancel.is_cancelled() {
                return;
            }

            // Pass continuous 16 kHz audio (including pre-roll, unvoiced consonants, and natural
            // inter-word pauses) to STT; only fall back to speech_only_16k if samples_16k is empty.
            let samples_for_stt = if captured.samples_16k.is_empty() {
                captured.speech_only_16k
            } else {
                captured.samples_16k
            };
            let captured_user_16k: Option<Arc<[f32]>> = if samples_for_stt.is_empty() {
                None
            } else {
                Some(Arc::from(samples_for_stt.as_slice()))
            };

            match ai_worker.transcribe(samples_for_stt, lang).await {
                Ok((txt, caps)) => {
                    let _ = internal_tx
                        .send(InternalMsg::VoiceCmdTranscribed {
                            session_id,
                            nav_epoch,
                            lang,
                            is_timeout: false,
                            captured_user_16k,
                            transcript: Ok(txt),
                            caps: Some(caps),
                        })
                        .await;
                }
                Err(AiError::Timeout { timeout_ms, .. }) => {
                    let _ = internal_tx
                        .send(InternalMsg::VoiceCmdTranscribed {
                            session_id,
                            nav_epoch,
                            lang,
                            is_timeout: false,
                            captured_user_16k: None,
                            transcript: Err(stt_timeout_msg(timeout_ms)),
                            caps: None,
                        })
                        .await;
                }
                Err(err) => {
                    let _ = internal_tx
                        .send(InternalMsg::VoiceCmdTranscribed {
                            session_id,
                            nav_epoch,
                            lang,
                            is_timeout: false,
                            captured_user_16k: None,
                            transcript: Err(BilingualMsg {
                                ta: format!("குரல் உணர்வில் பிழை: {err}"),
                                en: format!("Voice recognition error: {err}"),
                            }),
                            caps: None,
                        })
                        .await;
                }
            }
        });

        self.active_voice_task = Some((handle, cancel));
    }

    fn route_recognized_voice(
        &mut self,
        raw: String,
        lang: kural_core::Language,
        captured_user_16k: Option<Arc<[f32]>>,
        session_id: u64,
        nav_epoch: u64,
    ) {
        let trimmed = raw.trim();
        let valid_chars = trimmed.chars().filter(|c| c.is_alphanumeric()).count();
        let is_single_digit_kural = valid_chars == 1
            && trimmed
                .chars()
                .filter(|c| c.is_alphanumeric())
                .all(|c| matches!(c, '1'..='9' | '\u{0BE7}'..='\u{0BEF}'));
        let is_modal_quiz_choice = matches!(
            self.state.modal,
            Some(ModalKind::KidsWordGame | ModalKind::YouthQuiz)
        ) && parse_quiz_choice(trimmed).is_some();

        if trimmed.is_empty()
            || valid_chars == 0
            || (valid_chars < 2 && !is_single_digit_kural && !is_modal_quiz_choice)
        {
            let audio = self.audio.clone();
            tokio::spawn(async move {
                let _ = audio.play_earcon(EarconKind::RetryTone).await;
            });
            if self.state.voice_to_voice_loop {
                self.consecutive_silence_timeouts =
                    self.consecutive_silence_timeouts.saturating_add(1);
                if self.consecutive_silence_timeouts
                    >= self.config.audio.voice_loop_max_silence_timeouts
                {
                    self.disarm_voice_loop();
                    self.publish_snapshot();
                    self.emit_event(UiTransientEvent::InfoToast {
                        message_ta: VOICE_LOOP_SILENCE_STOP_NOTICE.ta.to_string(),
                        message_en: VOICE_LOOP_SILENCE_STOP_NOTICE.en.to_string(),
                    });
                } else {
                    self.rearm_voice_loop_when_ready(self.tts_epoch);
                }
                return;
            }
            self.emit_event(UiTransientEvent::InfoToast {
                message_ta: VOICE_UNCLEAR_RETRY_NOTICE.ta.to_string(),
                message_en: VOICE_UNCLEAR_RETRY_NOTICE.en.to_string(),
            });
            return;
        }

        self.consecutive_silence_timeouts = 0;

        let ctx = RouterContext {
            screen: self.state.screen,
            modal: self.state.modal,
            active_kural: self.state.current_kural.kural.number,
            active_scholar: self.state.effective_scholar,
            speech_speed: self.state.prefs.speech_speed,
            muted: self.state.prefs.muted,
        };

        match validate_voice_transcript(trimmed, lang, ctx) {
            Err(range_err) => {
                let audio = self.audio.clone();
                tokio::spawn(async move {
                    let _ = audio.play_earcon(EarconKind::RetryTone).await;
                });
                self.emit_event(UiTransientEvent::ErrorToast {
                    message_ta: range_err.ta.to_string(),
                    message_en: range_err.en.to_string(),
                });
            }
            Ok(outcome) => {
                let starts_audio_or_silence = matches!(
                    outcome.cmd,
                    BackendCommand::StopAudio
                        | BackendCommand::OpenKural(_)
                        | BackendCommand::SpeakCurrent(_)
                        | BackendCommand::SpeakCustomText { .. }
                        | BackendCommand::ReplayAudio
                        | BackendCommand::ReplayLastTts
                        | BackendCommand::ReplayVoiceTurn { .. }
                        | BackendCommand::SetVoiceToVoiceLoop(_)
                        | BackendCommand::StartVoiceCommand
                        | BackendCommand::StartVoiceAgeDetect
                        | BackendCommand::AskAiQuestion { .. }
                );
                if !starts_audio_or_silence {
                    self.audio.play_earcon_nowait(EarconKind::CmdRecognized);
                }
                self.emit_event(UiTransientEvent::VoiceCommandRecognized {
                    transcript: raw,
                    toast_ta: outcome.toast_ta,
                    toast_en: outcome.toast_en,
                    compact_tab: outcome.compact_tab,
                });

                match outcome.cmd {
                    BackendCommand::AskAiQuestion { query, seed_kural } => {
                        self.push_voice_turn_and_run_ai(
                            query,
                            seed_kural,
                            lang,
                            captured_user_16k,
                            session_id,
                            nav_epoch,
                        );
                    }
                    BackendCommand::OpenKural(num) => {
                        drop(captured_user_16k);
                        if !(1..=1330).contains(&num) {
                            self.emit_event(UiTransientEvent::ErrorToast {
                                message_ta: VOICE_KURAL_RANGE_ERROR.ta.to_string(),
                                message_en: VOICE_KURAL_RANGE_ERROR.en.to_string(),
                            });
                            return;
                        }
                        self.state.modal = None;
                        self.bump_nav_interrupt();
                        self.open_kural_with_pool_fixup(num, false);
                        if self.state.current_kural.kural.number == num {
                            self.force_speak_kural_and_meaning();
                        }
                    }
                    other => {
                        drop(captured_user_16k);
                        self.handle_command(other);
                    }
                }
            }
        }
    }

    fn push_voice_turn_and_run_ai(
        &mut self,
        query: String,
        seed_kural: Option<u16>,
        lang: kural_core::Language,
        captured_user_16k: Option<Arc<[f32]>>,
        session_id: u64,
        nav_epoch: u64,
    ) {
        self.cancel_active_tts();
        let turn_id = self.next_voice_turn_id;
        self.next_voice_turn_id = self.next_voice_turn_id.saturating_add(1);
        let summary = VoiceTurnSummary {
            id: turn_id,
            user_text: query.clone(),
            ai_text: String::new(),
            lang,
            has_user_audio: captured_user_16k.is_some(),
            has_ai_audio: false,
            cited_kurals: seed_kural.into_iter().collect(),
        };
        self.state.recent_voice_turns.push(summary);
        while self.state.recent_voice_turns.len() > MAX_VOICE_TURNS {
            self.state.recent_voice_turns.remove(0);
        }
        self.state.can_replay_voice_turn = self
            .state
            .recent_voice_turns
            .iter()
            .any(|t| t.has_user_audio || t.has_ai_audio);
        self.publish_snapshot();
        self.run_voice_ai_question(
            query,
            seed_kural,
            lang,
            Some(turn_id),
            captured_user_16k,
            session_id,
            nav_epoch,
        );
    }

    #[allow(clippy::too_many_arguments)]
    fn run_voice_ai_question(
        &mut self,
        query: String,
        seed_kural: Option<u16>,
        lang: kural_core::Language,
        turn_id: Option<u64>,
        captured_user_16k: Option<Arc<[f32]>>,
        session_id: u64,
        nav_epoch: u64,
    ) {
        let explainer = self.explainer.clone();
        let age = active_age_for_screen(self.state.screen, self.state.prefs.default_age);
        let audio = self.audio.clone();
        let internal_tx = self.internal_tx.clone();
        let inference_timeout = Duration::from_millis(self.config.ai.inference_timeout_ms);
        tokio::spawn(async move {
            if let (Some(tid), Some(pcm)) = (turn_id, captured_user_16k) {
                let _ = audio.record_voice_turn_user(tid, pcm).await;
            }
            let res = match tokio::time::timeout(
                inference_timeout,
                tokio::task::spawn_blocking(move || {
                    explainer.ask_question(&query, age, lang, seed_kural)
                }),
            )
            .await
            {
                Ok(Ok(Ok(ans))) => Ok(ans),
                Ok(Ok(Err(e))) => Err(BilingualMsg {
                    ta: format!("AI பதில் பிழை: {e}"),
                    en: format!("AI answer error: {e}"),
                }),
                Ok(Err(join_err)) => Err(BilingualMsg {
                    ta: format!("AI பதில் பிழை: {join_err}"),
                    en: format!("AI answer error: {join_err}"),
                }),
                Err(_) => {
                    let timeout_ms = inference_timeout.as_millis() as u64;
                    tracing::warn!(timeout_ms, "Voice AI question inference timed out");
                    Err(ai_inference_timeout_msg(timeout_ms))
                }
            };
            let _ = internal_tx
                .send(InternalMsg::VoiceAiDone {
                    session_id,
                    nav_epoch,
                    turn_id,
                    seed_kural,
                    lang,
                    result: res,
                })
                .await;
        });
    }

    fn run_text_ai_question(&mut self, query: String, seed_kural: Option<u16>) {
        self.ai_epoch = self.ai_epoch.wrapping_add(1);
        let ai_epoch = self.ai_epoch;
        let explainer = self.explainer.clone();
        let age = active_age_for_screen(self.state.screen, self.state.prefs.default_age);
        let lang = self.state.prefs.lang;
        let internal_tx = self.internal_tx.clone();
        let inference_timeout = Duration::from_millis(self.config.ai.inference_timeout_ms);
        tokio::spawn(async move {
            let res = match tokio::time::timeout(
                inference_timeout,
                tokio::task::spawn_blocking(move || {
                    explainer.ask_question(&query, age, lang, seed_kural)
                }),
            )
            .await
            {
                Ok(Ok(Ok(ans))) => Ok(ans),
                Ok(Ok(Err(e))) => Err(BilingualMsg {
                    ta: format!("AI தேடல் பிழை: {e}"),
                    en: format!("AI query failed: {e}"),
                }),
                Ok(Err(join_err)) => Err(BilingualMsg {
                    ta: format!("AI தேடல் பிழை: {join_err}"),
                    en: format!("AI query failed: {join_err}"),
                }),
                Err(_) => {
                    let timeout_ms = inference_timeout.as_millis() as u64;
                    tracing::warn!(timeout_ms, "Text AI question inference timed out");
                    Err(ai_inference_timeout_msg(timeout_ms))
                }
            };
            let _ = internal_tx
                .send(InternalMsg::TextAiDone {
                    ai_epoch,
                    result: res,
                })
                .await;
        });
    }

    fn run_text_search(&mut self, query: String, division_id: Option<u8>) {
        self.search_epoch = self.search_epoch.wrapping_add(1);
        let search_epoch = self.search_epoch;
        let corpus = self.corpus.clone();
        let min_cos = self.config.ai.min_vec_candidate_cosine;
        let internal_tx = self.internal_tx.clone();
        tokio::spawn(async move {
            let res = tokio::task::spawn_blocking(move || {
                corpus
                    .search_hybrid_rrf(&query, division_id, min_cos, RRF_K, TEXT_SEARCH_LIMIT)
                    .map(|h| h.outcome)
                    .map_err(|e| e.to_string())
            })
            .await
            .unwrap_or_else(|e| Err(e.to_string()));
            let _ = internal_tx
                .send(InternalMsg::SearchDone {
                    search_epoch,
                    result: res,
                })
                .await;
        });
    }

    fn default_content_target(&self) -> Option<kural_core::SpeakTarget> {
        if matches!(self.state.modal, Some(ModalKind::KidsStory)) {
            return Some(kural_core::SpeakTarget::KuttyStory);
        }
        if matches!(self.state.modal, Some(ModalKind::YouthDeepDive)) {
            return Some(kural_core::SpeakTarget::YouthBriefing);
        }
        match self.state.screen {
            AppScreen::Start => None,
            AppScreen::Kids => Some(kural_core::SpeakTarget::KuralAndMeaning),
            AppScreen::Youth => Some(kural_core::SpeakTarget::YouthBriefing),
            AppScreen::Adult => Some(kural_core::SpeakTarget::KuralAndMeaning),
        }
    }

    fn content_audio_key(&self, target: kural_core::SpeakTarget) -> ContentAudioKey {
        ContentAudioKey {
            screen: self.state.screen,
            kural_number: self.state.current_kural.kural.number,
            lang: self.state.prefs.lang,
            effective_scholar: self.state.effective_scholar,
            target,
        }
    }

    fn trigger_auto_playback_for_current_content(&mut self, force: bool) {
        let Some(target) = self.default_content_target() else {
            self.active_content_key = None;
            self.state.can_replay_tts = false;
            self.state.playback_status = AudioPlaybackStatus::Idle;
            self.publish_snapshot();
            return;
        };

        let key = self.content_audio_key(target);
        if !force
            && self.active_content_key == Some(key)
            && matches!(
                self.state.playback_status,
                AudioPlaybackStatus::Loading
                    | AudioPlaybackStatus::Playing
                    | AudioPlaybackStatus::Completed
                    | AudioPlaybackStatus::Replaying
            )
        {
            return;
        }

        self.active_content_key = Some(key);
        self.state.can_replay_tts = false;

        if !self.state.prefs.auto_speak_text || !self.audio.health().output_ok {
            self.state.playback_status = AudioPlaybackStatus::FallbackReady;
            self.publish_snapshot();
            return;
        }

        self.state.playback_status = AudioPlaybackStatus::Loading;
        self.publish_snapshot();
        self.start_speak_target(target);
    }

    fn start_speak_target(&mut self, target: kural_core::SpeakTarget) {
        let lang = self.state.prefs.lang;
        let Some(payload) = build_speech_payload(
            &self.state.current_kural,
            self.state.screen,
            target,
            lang,
            self.state.effective_scholar,
            self.state.prefs.speech_speed,
            self.last_ai_answer.as_ref(),
        ) else {
            return;
        };
        if let kural_core::SpeakTarget::ScholarCommentary(req) = target
            && req == Scholar::Parimel
            && self.state.current_kural.kural.meaning_parimel.is_none()
            && self.state.requested_scholar != Scholar::Parimel
        {
            self.emit_event(UiTransientEvent::CommentaryFallbackNotice {
                requested: Scholar::Parimel,
                effective: Scholar::MuVa,
                notice_ta: PARIMEL_FALLBACK_NOTICE.ta.to_string(),
                notice_en: PARIMEL_FALLBACK_NOTICE.en.to_string(),
            });
        }
        let persona = target_persona_code(self.state.screen, target);
        self.speak_raw_chunks(payload.chunks, lang, target, persona, None, false);
    }

    fn speak_custom_text(
        &mut self,
        text: String,
        lang: kural_core::Language,
        label: Option<String>,
    ) {
        let trimmed = text.trim();
        if trimmed.is_empty() {
            return;
        }
        self.pending_loop_rearm = None;
        self.cancel_active_capture();
        self.cancel_active_tts();
        self.nav_epoch = self.nav_epoch.wrapping_add(1);
        let spoken_lang = detect_text_language(trimmed, lang);
        self.state.active_replay_label = label;
        self.publish_snapshot();
        self.speak_raw_chunks(
            vec![trimmed.to_string()],
            spoken_lang,
            kural_core::SpeakTarget::AiAnswer,
            4,
            None,
            false,
        );
    }

    fn start_replay_last_tts(&mut self) {
        if matches!(
            self.state.playback_status,
            AudioPlaybackStatus::Loading
                | AudioPlaybackStatus::Playing
                | AudioPlaybackStatus::Replaying
        ) {
            return;
        }
        if !self.state.can_replay_tts {
            self.emit_event(UiTransientEvent::InfoToast {
                message_ta: NOTHING_TO_REPLAY_NOTICE.ta.to_string(),
                message_en: NOTHING_TO_REPLAY_NOTICE.en.to_string(),
            });
            return;
        }
        self.pending_loop_rearm = None;
        self.cancel_active_capture();
        self.cancel_active_tts();
        self.nav_epoch = self.nav_epoch.wrapping_add(1);

        let lang = self.state.prefs.lang;
        let speed = effective_tts_speed(self.state.prefs.speech_speed, self.state.screen);
        let notice = REPLAY_AUDIO_NOTICE;
        let label = match lang {
            kural_core::Language::Tamil => notice.ta,
            kural_core::Language::English => notice.en,
        };
        self.state.playback_status = AudioPlaybackStatus::Replaying;
        self.state.active_replay_label = Some(label.to_string());
        self.publish_snapshot();
        self.emit_event(UiTransientEvent::InfoToast {
            message_ta: notice.ta.to_string(),
            message_en: notice.en.to_string(),
        });

        let audio = self.audio.clone();
        let internal_tx = self.internal_tx.clone();
        let tts_epoch = self.tts_epoch;
        let nav_epoch = self.nav_epoch;
        let kural_num = self.state.current_kural.kural.number;
        let cancel = CancellationToken::new();
        let task_cancel = cancel.clone();
        let handle = tokio::spawn(async move {
            if task_cancel.is_cancelled() {
                return;
            }
            let res = audio.play_last_completed_tts(speed).await;
            if task_cancel.is_cancelled() {
                return;
            }
            let (ok, error_msg) = match res {
                Ok(()) => (true, None),
                Err(AudioError::Cancelled) => (false, None),
                Err(e) => (
                    false,
                    Some(BilingualMsg {
                        ta: format!("ஒலி இயக்கப் பிழை: {e}"),
                        en: format!("Audio playback error: {e}"),
                    }),
                ),
            };
            let _ = internal_tx
                .send(InternalMsg::TtsFinished {
                    tts_epoch,
                    nav_epoch,
                    target: kural_core::SpeakTarget::KuralOnly,
                    kural_number: kural_num,
                    voice_turn_id: None,
                    has_saved_audio: ok,
                    voice_loop_rearm: false,
                    ok,
                    error_msg,
                })
                .await;
        });
        self.active_tts_task = Some((handle, cancel));
    }

    fn start_replay_voice_turn(&mut self, turn_id: Option<u64>, mode: VoiceReplayMode) {
        let resolved = match turn_id {
            Some(id) => self
                .state
                .recent_voice_turns
                .iter()
                .find(|t| t.id == id)
                .cloned(),
            None => self.state.recent_voice_turns.last().cloned(),
        };
        let Some(turn) = resolved else {
            self.emit_event(UiTransientEvent::InfoToast {
                message_ta: NOTHING_TO_REPLAY_NOTICE.ta.to_string(),
                message_en: NOTHING_TO_REPLAY_NOTICE.en.to_string(),
            });
            return;
        };
        let available = match mode {
            VoiceReplayMode::UserVoiceOnly => turn.has_user_audio,
            VoiceReplayMode::AiReplyOnly => turn.has_ai_audio,
            VoiceReplayMode::FullExchange => turn.has_user_audio && turn.has_ai_audio,
        };
        if !available {
            self.emit_event(UiTransientEvent::InfoToast {
                message_ta: NOTHING_TO_REPLAY_NOTICE.ta.to_string(),
                message_en: NOTHING_TO_REPLAY_NOTICE.en.to_string(),
            });
            return;
        }

        self.pending_loop_rearm = None;
        self.cancel_active_capture();
        self.cancel_active_tts();
        self.nav_epoch = self.nav_epoch.wrapping_add(1);

        let lang = self.state.prefs.lang;
        let speed = effective_tts_speed(self.state.prefs.speech_speed, self.state.screen);
        let notice = match mode {
            VoiceReplayMode::UserVoiceOnly => REPLAY_USER_VOICE_NOTICE,
            VoiceReplayMode::AiReplyOnly => REPLAY_AUDIO_NOTICE,
            VoiceReplayMode::FullExchange => REPLAY_FULL_EXCHANGE_NOTICE,
        };
        let label = match lang {
            kural_core::Language::Tamil => notice.ta,
            kural_core::Language::English => notice.en,
        };
        self.state.active_replay_label = Some(label.to_string());
        self.publish_snapshot();
        self.emit_event(UiTransientEvent::InfoToast {
            message_ta: notice.ta.to_string(),
            message_en: notice.en.to_string(),
        });

        let audio = self.audio.clone();
        let internal_tx = self.internal_tx.clone();
        let tts_epoch = self.tts_epoch;
        let nav_epoch = self.nav_epoch;
        let kural_num = self.state.current_kural.kural.number;
        let resolved_id = turn.id;
        let cancel = CancellationToken::new();
        let task_cancel = cancel.clone();
        let handle = tokio::spawn(async move {
            if task_cancel.is_cancelled() {
                return;
            }
            let res = audio.play_voice_turn(Some(resolved_id), mode, speed).await;
            if task_cancel.is_cancelled() {
                return;
            }
            let (ok, error_msg) = match res {
                Ok(()) => (true, None),
                Err(AudioError::Cancelled) => (false, None),
                Err(e) => (
                    false,
                    Some(BilingualMsg {
                        ta: format!("ஒலி இயக்கப் பிழை: {e}"),
                        en: format!("Audio playback error: {e}"),
                    }),
                ),
            };
            let _ = internal_tx
                .send(InternalMsg::TtsFinished {
                    tts_epoch,
                    nav_epoch,
                    target: kural_core::SpeakTarget::AiAnswer,
                    kural_number: kural_num,
                    voice_turn_id: None,
                    has_saved_audio: false,
                    voice_loop_rearm: false,
                    ok,
                    error_msg,
                })
                .await;
        });
        self.active_tts_task = Some((handle, cancel));
    }

    fn set_voice_to_voice_loop(&mut self, enabled: bool) {
        self.pending_loop_rearm = None;
        self.consecutive_silence_timeouts = 0;
        self.state.voice_to_voice_loop = enabled;
        self.publish_snapshot();
        let notice = if enabled {
            VOICE_LOOP_ON_NOTICE
        } else {
            VOICE_LOOP_OFF_NOTICE
        };
        self.emit_event(UiTransientEvent::InfoToast {
            message_ta: notice.ta.to_string(),
            message_en: notice.en.to_string(),
        });
        if enabled {
            self.rearm_voice_loop_when_ready(self.tts_epoch);
        } else {
            self.cancel_active_capture();
        }
    }

    fn disarm_voice_loop(&mut self) {
        self.pending_loop_rearm = None;
        self.consecutive_silence_timeouts = 0;
        self.state.voice_to_voice_loop = false;
        self.state.active_replay_label = None;
    }

    fn rearm_voice_loop_when_ready(&mut self, expected_tts_epoch: u64) {
        if !self.state.voice_to_voice_loop {
            self.pending_loop_rearm = None;
            return;
        }
        let live_state = *self.audio.subscribe_state().borrow();
        if live_state == AudioState::Idle
            && self.active_tts_task.is_none()
            && self.active_voice_task.is_none()
        {
            self.pending_loop_rearm = None;
            self.start_voice_command();
        } else {
            self.pending_loop_rearm = Some(expected_tts_epoch);
        }
    }

    fn speak_raw_chunks(
        &mut self,
        chunks: Vec<String>,
        lang: kural_core::Language,
        target: kural_core::SpeakTarget,
        persona_code: u8,
        voice_turn_id: Option<u64>,
        voice_loop_rearm: bool,
    ) {
        self.cancel_active_tts();
        self.state.can_replay_tts = false;
        self.state.playback_status = AudioPlaybackStatus::Loading;
        self.publish_snapshot();
        let tts_epoch = self.tts_epoch;
        let nav_epoch = self.nav_epoch;
        let kural_number = self.state.current_kural.kural.number;
        let speed = effective_tts_speed(self.state.prefs.speech_speed, self.state.screen);
        let speed_pct = (snap_speech_speed(speed) * 100.0).round() as u32;
        let content = chunks.join("\n");
        let keyed_content = format!("s{speed_pct}:{content}");
        let u64_key = compute_tts_cache_key(
            hash_tts_content(&keyed_content),
            lang_code(lang),
            persona_code,
            self.active_tts_model_fingerprint,
        );

        let cancel = CancellationToken::new();
        let worker_cancel = cancel.clone();
        let task_cancel = cancel.clone();
        let (pcm_tx, pcm_rx) = mpsc::channel(8);
        let audio = self.audio.clone();
        let ai_worker = self.ai_worker.clone();
        let internal_tx = self.internal_tx.clone();

        let handle = tokio::spawn(async move {
            let hit = audio.play_cached_tts_by_key(u64_key, speed).await;
            let (ok, has_saved_audio, error_msg) = if task_cancel.is_cancelled() {
                (false, false, None)
            } else if let Ok(true) = hit {
                if let Some(tid) = voice_turn_id {
                    let _ = audio.attach_cached_tts_to_voice_turn(tid, u64_key).await;
                }
                (true, true, None)
            } else if matches!(hit, Err(AudioError::Cancelled)) {
                (false, false, None)
            } else {
                let synth_fut =
                    ai_worker.speak_collecting(chunks, lang, speed, worker_cancel, pcm_tx);
                let play_fut = audio.play_pcm_stream(pcm_rx, None);
                let (synth_res, play_res) = tokio::join!(synth_fut, play_fut);

                match (play_res, synth_res) {
                    (Ok(()), Ok((caps, maybe_pcm))) => {
                        let _ = internal_tx.send(InternalMsg::SpeechCapsUpdated(caps)).await;
                        let mut saved_audio = false;
                        if !task_cancel.is_cancelled()
                            && let Some((pcm, sr)) = maybe_pcm
                            && !pcm.is_empty()
                        {
                            let arc_pcm: Arc<[i16]> = Arc::from(pcm);
                            if audio
                                .commit_completed_tts_with_speed(
                                    u64_key,
                                    arc_pcm.clone(),
                                    sr,
                                    speed,
                                )
                                .await
                                .is_ok()
                            {
                                saved_audio = true;
                            }
                            if let Some(tid) = voice_turn_id {
                                let _ = audio
                                    .attach_voice_turn_ai_with_speed(tid, arc_pcm, sr, speed)
                                    .await;
                            }
                        }
                        (true, saved_audio, None)
                    }
                    (Err(AudioError::Cancelled), _) | (_, Err(kural_ai::AiError::Cancelled)) => {
                        (false, false, None)
                    }
                    (Err(e), _) => (
                        false,
                        false,
                        Some(BilingualMsg {
                            ta: format!("ஒலி இயக்கப் பிழை: {e}"),
                            en: format!("Audio playback error: {e}"),
                        }),
                    ),
                    (Ok(()), Err(AiError::Timeout { timeout_ms, .. })) => {
                        (false, false, Some(tts_timeout_msg(timeout_ms)))
                    }
                    (Ok(()), Err(e)) => (
                        false,
                        false,
                        Some(BilingualMsg {
                            ta: format!("ஒலி இயக்கப் பிழை: {e}"),
                            en: format!("Audio playback error: {e}"),
                        }),
                    ),
                }
            };

            let _ = internal_tx
                .send(InternalMsg::TtsFinished {
                    tts_epoch,
                    nav_epoch,
                    target,
                    kural_number,
                    voice_turn_id,
                    has_saved_audio,
                    voice_loop_rearm,
                    ok,
                    error_msg,
                })
                .await;
        });

        self.active_tts_task = Some((handle, cancel));
    }

    fn handle_kids_game_answer(&mut self, kural_number: u16, choice_idx: u8) {
        if !(1..=1330).contains(&kural_number) || choice_idx >= 4 {
            self.emit_event(UiTransientEvent::ErrorToast {
                message_ta: "தவறான விளையாட்டுத் தேர்வு.".to_string(),
                message_en: "Invalid Word Game selection.".to_string(),
            });
            return;
        }
        let full = if self.state.current_kural.kural.number == kural_number {
            Ok(self.state.current_kural.clone())
        } else {
            self.corpus.get_kural_full(kural_number)
        };
        let Ok(full) = full else {
            self.emit_event(UiTransientEvent::ErrorToast {
                message_ta: VOICE_KURAL_RANGE_ERROR.ta.to_string(),
                message_en: VOICE_KURAL_RANGE_ERROR.en.to_string(),
            });
            return;
        };
        let Some(child) = full.child.as_ref() else {
            self.emit_event(UiTransientEvent::ErrorToast {
                message_ta: "இந்தக் குறளுக்கு விளையாட்டு இல்லை.".to_string(),
                message_en: "Word Game unavailable for this Kural.".to_string(),
            });
            return;
        };
        let correct_idx = child
            .game_options_ta
            .iter()
            .position(|opt| opt == &child.game_blank_word_ta)
            .unwrap_or(0) as u8;
        let correct = choice_idx == correct_idx;

        match self
            .user_state
            .record_kids_game_attempt(kural_number, correct)
        {
            Ok(progress) => {
                let stars = progress.stars_earned;
                self.state.kids_progress = progress;
                self.publish_snapshot();
                self.emit_event(UiTransientEvent::KidsGameEvaluated {
                    kural_number,
                    correct,
                    correct_idx,
                    stars_earned: stars,
                });
            }
            Err(err) => self.emit_event(UiTransientEvent::ErrorToast {
                message_ta: format!("சேமிப்புப் பிழை: {err}"),
                message_en: format!("Progress save error: {err}"),
            }),
        }
    }

    fn handle_complete_kids_story(&mut self, kural_number: u16) {
        if !(1..=1330).contains(&kural_number) {
            return;
        }
        if let Ok(progress) = self.user_state.record_kids_story_completed(kural_number) {
            self.state.kids_progress = progress;
            self.publish_snapshot();
        }
    }

    fn handle_youth_quiz_answer(&mut self, kural_number: u16, choice_idx: u8) {
        if !(1..=1330).contains(&kural_number) || choice_idx >= 4 {
            self.emit_event(UiTransientEvent::ErrorToast {
                message_ta: "தவறான வினாத் தேர்வு.".to_string(),
                message_en: "Invalid Quiz choice.".to_string(),
            });
            return;
        }
        let full = if self.state.current_kural.kural.number == kural_number {
            Ok(self.state.current_kural.clone())
        } else {
            self.corpus.get_kural_full(kural_number)
        };
        let Ok(full) = full else {
            return;
        };
        let Some(youth) = full.youth.as_ref() else {
            return;
        };
        let correct_idx = youth.quiz_correct_idx.min(3);
        let correct = choice_idx == correct_idx;

        let now = Local::now();
        let today = now.format("%Y-%m-%d").to_string();
        let yesterday = now
            .checked_sub_days(Days::new(1))
            .map_or_else(|| today.clone(), |d| d.format("%Y-%m-%d").to_string());

        match self
            .user_state
            .record_youth_quiz_attempt(kural_number, correct, &today, &yesterday)
        {
            Ok(progress) => {
                let streak = progress.daily_streak;
                let xp = progress.quiz_xp;
                self.state.youth_progress = progress;
                self.publish_snapshot();
                self.emit_event(UiTransientEvent::YouthQuizEvaluated {
                    kural_number,
                    correct,
                    correct_idx,
                    daily_streak: streak,
                    quiz_xp: xp,
                });
            }
            Err(err) => self.emit_event(UiTransientEvent::ErrorToast {
                message_ta: format!("சேமிப்புப் பிழை: {err}"),
                message_en: format!("Quiz save error: {err}"),
            }),
        }
    }

    fn handle_toggle_bookmark(&mut self, kural_number: u16) {
        if !(1..=1330).contains(&kural_number) {
            return;
        }
        let now_rfc3339 = Local::now().to_rfc3339();
        match self.user_state.toggle_bookmark(
            kural_number,
            self.state.effective_scholar,
            &now_rfc3339,
            &self.corpus,
        ) {
            Ok(bookmarks) => {
                self.state.bookmarks = bookmarks;
                self.publish_snapshot();
            }
            Err(err) => self.emit_event(UiTransientEvent::ErrorToast {
                message_ta: format!("புத்தகக்குறிப் பிழை: {err}"),
                message_en: format!("Bookmark error: {err}"),
            }),
        }
    }

    fn schedule_prefs_debounce(&mut self) {
        self.prefs_dirty_deadline =
            Some(tokio::time::Instant::now() + Duration::from_secs(PREFS_DEBOUNCE_SECS));
    }

    fn flush_prefs_async(&mut self) {
        self.prefs_dirty_deadline = None;
        let store = self.user_state.clone();
        let prefs = self.state.prefs.clone();
        tokio::task::spawn_blocking(move || {
            let _ = store.save_prefs(&prefs);
        });
    }

    fn flush_prefs_now_sync(&mut self) {
        self.prefs_dirty_deadline = None;
        let _ = self.user_state.save_prefs(&self.state.prefs);
    }

    fn publish_snapshot(&self) {
        let _ = self.snapshot_tx.send(Arc::new(self.state.clone()));
    }

    fn emit_event(&mut self, evt: UiTransientEvent) {
        self.event_sinks
            .retain(|tx| match tx.try_send(evt.clone()) {
                Ok(()) | Err(mpsc::error::TrySendError::Full(_)) => true,
                Err(mpsc::error::TrySendError::Closed(_)) => false,
            });
    }
}

struct StartVoiceAutoRouteData {
    session_id: u64,
    nav_epoch: u64,
    lang: kural_core::Language,
    auto_triggered: bool,
    age_timed_out: bool,
    outcome: AgeOutcome,
    transcript: Option<String>,
    caps: Option<SpeechCapabilities>,
}

#[inline]
fn is_start_route_token_char(c: char) -> bool {
    c.is_alphanumeric() || matches!(c, '\u{0B80}'..='\u{0BFF}')
}

fn tokenize_start_utterance(raw: &str) -> Vec<String> {
    let nfc = normalize_nfc_trimmed(raw);
    let mut tokens = Vec::new();
    let mut current = String::new();
    for ch in nfc.chars().flat_map(char::to_lowercase) {
        if is_start_route_token_char(ch) {
            current.push(ch);
        } else if !current.is_empty() {
            tokens.push(std::mem::take(&mut current));
        }
    }
    if !current.is_empty() {
        tokens.push(current);
    }
    tokens
}

fn match_age_keyword_token(tok: &str) -> Option<AgeGroup> {
    if matches!(tok, "child" | "children" | "kids" | "kid")
        || tok.starts_with("சிறுவர்")
        || tok.starts_with("குழந்தை")
    {
        return Some(AgeGroup::Kids);
    }
    if matches!(
        tok,
        "teen" | "teens" | "teenager" | "teenagers" | "youth" | "student" | "students"
    ) || tok.starts_with("இளைஞர்")
        || tok.starts_with("மாணவர்")
    {
        return Some(AgeGroup::Youth);
    }
    if matches!(
        tok,
        "adult" | "adults" | "scholar" | "scholars" | "elder" | "elders"
    ) || tok.starts_with("பெரியவர்")
        || tok.starts_with("பெரியோர்")
        || tok.starts_with("அறிஞர்")
    {
        return Some(AgeGroup::Adult);
    }
    None
}

/// Resolve Start-screen voice age route: explicit spoken keywords override acoustic age.
fn resolve_start_voice_age_route(
    outcome: &AgeOutcome,
    transcript: Option<&str>,
) -> Option<AgeGroup> {
    if let Some(raw) = transcript {
        for tok in tokenize_start_utterance(raw) {
            if let Some(age) = match_age_keyword_token(&tok) {
                return Some(age);
            }
        }
    }
    match outcome {
        AgeOutcome::Detected(det) => Some(det.age),
        AgeOutcome::Retry(_) => None,
    }
}

/// Extract an explicit navigation or Discuss follow-up command (`OpenKural`, `OpenAdhikaram`, or
/// explicit `OpenModal(ModalKind::AiChat { .. })` when `"discuss"` / `"விவாதி"` is spoken) from
/// an opening Start-screen utterance. Never auto-triggers `AskAiQuestion` on landing.
fn extract_opening_followup_command(
    transcript: Option<&str>,
    lang: kural_core::Language,
    ctx: RouterContext,
) -> Option<BackendCommand> {
    let raw = transcript.map(normalize_nfc_trimmed)?;
    let valid_chars = raw
        .chars()
        .filter(|&c| is_start_route_token_char(c))
        .count();
    if valid_chars < 2 {
        return None;
    }

    let outcome = validate_voice_transcript(&raw, lang, ctx).ok()?;
    match outcome.cmd {
        BackendCommand::OpenKural(_) | BackendCommand::OpenAdhikaram(_) => Some(outcome.cmd),
        BackendCommand::OpenModal(ModalKind::AiChat { .. }) => {
            let tokens = tokenize_start_utterance(&raw);
            if tokens
                .iter()
                .any(|t| matches!(t.as_str(), "discuss" | "விவாதி"))
            {
                Some(outcome.cmd)
            } else {
                None
            }
        }
        _ => None,
    }
}

fn default_pool_or_fallback(
    corpus: &KuralDb,
    screen: AppScreen,
    must_include: u16,
) -> Result<Vec<u16>, BackendError> {
    let kids_only = screen == AppScreen::Kids;
    let mut pool = corpus.get_pool_numbers_for_filter(None, kids_only)?;
    if pool.is_empty() || (kids_only && !pool.contains(&must_include)) {
        pool = corpus.get_pool_numbers_for_filter(None, false)?;
    }
    Ok(pool)
}

fn find_kural_idx(pool: &[u16], target: u16) -> usize {
    pool.iter().position(|&n| n == target).unwrap_or(0)
}

fn active_age_for_screen(screen: AppScreen, default_age: AgeGroup) -> AgeGroup {
    match screen {
        AppScreen::Start => default_age,
        AppScreen::Kids => AgeGroup::Kids,
        AppScreen::Youth => AgeGroup::Youth,
        AppScreen::Adult => AgeGroup::Adult,
    }
}

const fn lang_code(lang: kural_core::Language) -> u8 {
    match lang {
        kural_core::Language::Tamil => 1,
        kural_core::Language::English => 2,
    }
}

const fn target_persona_code(screen: AppScreen, target: kural_core::SpeakTarget) -> u8 {
    let base = match screen {
        AppScreen::Start => 0,
        AppScreen::Kids => 10,
        AppScreen::Youth => 20,
        AppScreen::Adult => 30,
    };
    let sub = match target {
        kural_core::SpeakTarget::KuralOnly => 1,
        kural_core::SpeakTarget::KuralAndMeaning => 2,
        kural_core::SpeakTarget::KuttyStory => 3,
        kural_core::SpeakTarget::YouthBriefing => 4,
        kural_core::SpeakTarget::ScholarCommentary(s) => match s {
            Scholar::MuVa => 5,
            Scholar::Solomon => 6,
            Scholar::Kalaignar => 7,
            Scholar::Parimel => 8,
            Scholar::English => 9,
        },
        kural_core::SpeakTarget::AiAnswer => 0,
    };
    base + sub
}

#[cfg(test)]
mod tests {
    use super::*;
    use kural_core::{Language, ResearchTheme, SearchTab, Theme};
    use tempfile::TempDir;

    fn test_config(tmp: &TempDir) -> AppConfig {
        let mut cfg = AppConfig::load(None).expect("load workspace default config");
        cfg.paths.user_state_db = tmp.path().join("user_state.db");
        cfg.paths.models_dir = tmp.path().join("empty_models");
        cfg
    }

    async fn recv_until<F>(
        rx: &mut mpsc::Receiver<UiTransientEvent>,
        mut pred: F,
    ) -> Option<UiTransientEvent>
    where
        F: FnMut(&UiTransientEvent) -> bool,
    {
        let deadline = tokio::time::Instant::now() + Duration::from_secs(3);
        while tokio::time::Instant::now() < deadline {
            match tokio::time::timeout(Duration::from_millis(250), rx.recv()).await {
                Ok(Some(evt)) if pred(&evt) => return Some(evt),
                Ok(Some(_)) => continue,
                Ok(None) | Err(_) => continue,
            }
        }
        None
    }

    #[test]
    fn backend_controller_handles_navigation_parimel_fallback_games_quizzes_and_bookmarks() {
        let tmp = TempDir::new().expect("tempdir");
        let cfg = test_config(&tmp);
        let handle = BackendHandle::start(cfg.clone()).expect("BackendHandle::start");
        let mut events = handle.subscribe_events();
        let snap0 = handle.current_snapshot();

        assert_eq!(snap0.screen, AppScreen::Start);
        assert!(!handle.catalog().divisions.is_empty());
        assert_eq!(handle.catalog().adhikarams.len(), 133);
        assert!(snap0.system_health.db_ok);
        assert!(snap0.system_health.user_state_ok);

        // Select Adult, open Kural 395, request Parimel -> auto fallback to MuVa + notice
        handle.dispatch(BackendCommand::SelectAge(AgeGroup::Adult));
        handle.dispatch(BackendCommand::SelectCommentary(Scholar::Parimel));
        handle.dispatch(BackendCommand::OpenKural(395));

        let rt = tokio::runtime::Runtime::new().unwrap();
        let notice = rt.block_on(recv_until(&mut events, |e| {
            matches!(
                e,
                UiTransientEvent::CommentaryFallbackNotice {
                    requested: Scholar::Parimel,
                    effective: Scholar::MuVa,
                    ..
                }
            )
        }));
        assert!(notice.is_some(), "expected Parimel -> MuVa fallback notice");
        let snap_395 = handle.current_snapshot();
        assert_eq!(snap_395.current_kural.kural.number, 395);
        assert_eq!(snap_395.requested_scholar, Scholar::Parimel);
        assert_eq!(snap_395.effective_scholar, Scholar::MuVa);

        // Toggle bookmark -> captures MuVa (effective scholar)
        handle.dispatch(BackendCommand::ToggleBookmark(395));
        std::thread::sleep(Duration::from_millis(80));
        let snap_bm = handle.current_snapshot();
        assert!(snap_bm.bookmarks.iter().any(|b| b.kural_number == 395));

        // Open Kural #1 -> Parimel restores cleanly!
        handle.dispatch(BackendCommand::OpenKural(1));
        std::thread::sleep(Duration::from_millis(80));
        let snap_1 = handle.current_snapshot();
        assert_eq!(snap_1.effective_scholar, Scholar::Parimel);

        // Kids Game evaluation awards +1 star
        handle.dispatch(BackendCommand::SelectAge(AgeGroup::Kids));
        handle.dispatch(BackendCommand::OpenKural(1));
        std::thread::sleep(Duration::from_millis(80));
        let k1 = handle.current_snapshot().current_kural.clone();
        let child = k1.child.as_ref().expect("kural 1 child");
        let correct_slot = child
            .game_options_ta
            .iter()
            .position(|o| o == &child.game_blank_word_ta)
            .expect("valid game slot") as u8;
        handle.dispatch(BackendCommand::SubmitKidsGameAnswer {
            kural_number: 1,
            choice_idx: correct_slot,
        });
        let game_evt = rt.block_on(recv_until(&mut events, |e| {
            matches!(
                e,
                UiTransientEvent::KidsGameEvaluated {
                    correct: true,
                    stars_earned: 1,
                    ..
                }
            )
        }));
        assert!(game_evt.is_some());

        // Out-of-bounds Kids Game choice -> ErrorToast, zero panic
        handle.dispatch(BackendCommand::SubmitKidsGameAnswer {
            kural_number: 1,
            choice_idx: 9,
        });
        let err_evt = rt.block_on(recv_until(&mut events, |e| {
            matches!(e, UiTransientEvent::ErrorToast { .. })
        }));
        assert!(err_evt.is_some());

        // Youth Quiz & Research Theme & Adhikaram & Search
        handle.dispatch(BackendCommand::SelectAge(AgeGroup::Youth));
        handle.dispatch(BackendCommand::FilterByResearchTheme(Some(
            ResearchTheme::Polity,
        )));
        std::thread::sleep(Duration::from_millis(80));
        assert_eq!(
            handle.current_snapshot().active_filter,
            Some(ActiveFilter::ResearchTheme(ResearchTheme::Polity))
        );
        handle.dispatch(BackendCommand::ClearFilter);
        handle.dispatch(BackendCommand::OpenModal(ModalKind::Search {
            initial_tab: SearchTab::Divisions,
        }));
        handle.dispatch(BackendCommand::OpenAdhikaram(10));
        std::thread::sleep(Duration::from_millis(80));
        let snap_ad = handle.current_snapshot();
        assert_eq!(snap_ad.current_kural.adhikaram.id, 10);
        assert!(snap_ad.modal.is_none());

        let k_cur = handle.current_snapshot().current_kural.clone();
        let youth = k_cur.youth.as_ref().expect("kural youth");
        handle.dispatch(BackendCommand::SubmitYouthQuizAnswer {
            kural_number: k_cur.kural.number,
            choice_idx: youth.quiz_correct_idx,
        });
        let quiz_evt = rt.block_on(recv_until(&mut events, |e| {
            matches!(
                e,
                UiTransientEvent::YouthQuizEvaluated {
                    correct: true,
                    quiz_xp: 20,
                    ..
                }
            )
        }));
        assert!(quiz_evt.is_some());

        // Language, Theme, Speech speed persistence on shutdown
        handle.dispatch(BackendCommand::SetLanguage(Language::English));
        handle.dispatch(BackendCommand::SetTheme(Theme::Light));
        handle.dispatch(BackendCommand::SetSpeechSpeed(1.25));
        handle.dispatch(BackendCommand::AskAiQuestion {
            query: "What does Kural 1 mean?".to_string(),
            seed_kural: None,
        });
        let ai_evt = rt.block_on(recv_until(&mut events, |e| {
            matches!(e, UiTransientEvent::AiAnswerReady(_))
        }));
        assert!(ai_evt.is_some());

        handle.shutdown();

        // Reopen and verify persisted user_state.db survived cleanly
        let handle2 = BackendHandle::start(cfg).expect("restart BackendHandle");
        let reloaded = handle2.current_snapshot();
        assert_eq!(reloaded.prefs.lang, Language::English);
        assert_eq!(reloaded.prefs.theme, Theme::Light);
        assert_eq!(reloaded.prefs.speech_speed, 1.25);
        assert_eq!(reloaded.kids_progress.stars_earned, 1);
        assert_eq!(reloaded.youth_progress.quiz_xp, 20);
        assert!(reloaded.bookmarks.iter().any(|b| b.kural_number == 395));
        assert!(reloaded.auto_speak_text);
        handle2.shutdown();
    }

    #[test]
    fn backend_auto_speak_replay_and_voice_loop_behave_cleanly() {
        let tmp = TempDir::new().expect("tempdir");
        let cfg = test_config(&tmp);
        let handle = BackendHandle::start(cfg.clone()).expect("BackendHandle::start");
        let mut events = handle.subscribe_events();
        let rt = tokio::runtime::Runtime::new().unwrap();

        let snap0 = handle.current_snapshot();
        assert!(snap0.auto_speak_text);
        assert!(!snap0.can_replay_tts);
        assert!(!snap0.can_replay_voice_turn);
        assert!(!snap0.voice_to_voice_loop);
        assert!(snap0.recent_voice_turns.is_empty());

        // Empty replay requests emit friendly notice without crashing.
        handle.dispatch(BackendCommand::ReplayLastTts);
        let empty_evt = rt.block_on(recv_until(&mut events, |e| {
            matches!(
                e,
                UiTransientEvent::InfoToast { message_en, .. }
                if message_en == NOTHING_TO_REPLAY_NOTICE.en
            )
        }));
        assert!(empty_evt.is_some());

        handle.dispatch(BackendCommand::ReplayVoiceTurn {
            turn_id: None,
            mode: VoiceReplayMode::UserVoiceOnly,
        });
        let empty_voice_evt = rt.block_on(recv_until(&mut events, |e| {
            matches!(
                e,
                UiTransientEvent::InfoToast { message_en, .. }
                if message_en == NOTHING_TO_REPLAY_NOTICE.en
            )
        }));
        assert!(empty_voice_evt.is_some());

        // Auto-Speak Text toggles, emits toast, and persists across restart.
        handle.dispatch(BackendCommand::SetAutoSpeakText(false));
        let off_evt = rt.block_on(recv_until(&mut events, |e| {
            matches!(
                e,
                UiTransientEvent::InfoToast { message_en, .. }
                if message_en == AUTO_SPEAK_OFF_NOTICE.en
            )
        }));
        assert!(off_evt.is_some());
        assert!(!handle.current_snapshot().auto_speak_text);

        // Voice-to-Voice Loop arms cleanly and disarms on StopAudio & navigation.
        handle.dispatch(BackendCommand::SetVoiceToVoiceLoop(true));
        let loop_on_evt = rt.block_on(recv_until(&mut events, |e| {
            matches!(
                e,
                UiTransientEvent::InfoToast { message_en, .. }
                if message_en == VOICE_LOOP_ON_NOTICE.en
            )
        }));
        assert!(loop_on_evt.is_some());

        handle.dispatch(BackendCommand::StopAudio);
        let loop_off_evt = rt.block_on(recv_until(&mut events, |e| {
            matches!(
                e,
                UiTransientEvent::InfoToast { message_en, .. }
                if message_en == VOICE_LOOP_OFF_NOTICE.en
            )
        }));
        assert!(loop_off_evt.is_some());
        assert!(!handle.current_snapshot().voice_to_voice_loop);

        // SpeakCustomText with empty whitespace is a clean no-op; AskAiQuestion works with auto_speak off.
        handle.dispatch(BackendCommand::SpeakCustomText {
            text: "   ".to_string(),
            lang: Language::Tamil,
            label: None,
        });
        handle.dispatch(BackendCommand::AskAiQuestion {
            query: "Explain Kural 1".to_string(),
            seed_kural: Some(1),
        });
        let ai_evt = rt.block_on(recv_until(&mut events, |e| {
            matches!(e, UiTransientEvent::AiAnswerReady(_))
        }));
        assert!(ai_evt.is_some());

        handle.shutdown();

        let handle2 = BackendHandle::start(cfg).expect("restart BackendHandle");
        let reloaded = handle2.current_snapshot();
        assert!(!reloaded.prefs.auto_speak_text);
        assert!(!reloaded.auto_speak_text);
        // Verify UI-thread calls inside an active Tokio runtime do not panic.
        rt.block_on(async {
            let kurals = handle2
                .kurals_in_adhikaram(1)
                .expect("query kurals inside runtime");
            assert!(!kurals.is_empty());
            handle2.shutdown_ref();
        });
    }

    #[test]
    fn backend_automatic_initial_playback_and_persistent_replay_lifecycle() {
        let tmp = TempDir::new().expect("tempdir");
        let cfg = test_config(&tmp);
        let handle = BackendHandle::start(cfg).expect("BackendHandle::start");
        let mut events = handle.subscribe_events();
        let rt = tokio::runtime::Runtime::new().unwrap();

        // Start screen begins Idle without active content playback or Replay Audio.
        let snap0 = handle.current_snapshot();
        assert_eq!(snap0.screen, AppScreen::Start);
        assert_eq!(snap0.playback_status, AudioPlaybackStatus::Idle);
        assert!(!snap0.can_replay_tts);

        // When auto_speak_text is disabled (or autoplay blocked), entering content moves cleanly to FallbackReady.
        handle.dispatch(BackendCommand::SetAutoSpeakText(false));
        let off_evt = rt.block_on(recv_until(&mut events, |e| {
            matches!(
                e,
                UiTransientEvent::InfoToast { message_en, .. }
                if message_en == AUTO_SPEAK_OFF_NOTICE.en
            )
        }));
        assert!(off_evt.is_some());

        handle.dispatch(BackendCommand::SelectAge(AgeGroup::Kids));
        std::thread::sleep(Duration::from_millis(80));
        let fallback_snap = handle.current_snapshot();
        assert_eq!(fallback_snap.screen, AppScreen::Kids);
        assert_eq!(
            fallback_snap.playback_status,
            AudioPlaybackStatus::FallbackReady
        );
        assert!(!fallback_snap.can_replay_tts);

        // With auto_speak_text enabled, navigating to new content triggers Loading/Playing or clean FallbackReady.
        handle.dispatch(BackendCommand::SetAutoSpeakText(true));
        handle.dispatch(BackendCommand::OpenKural(2));
        std::thread::sleep(Duration::from_millis(80));
        let nav_snap = handle.current_snapshot();
        assert_eq!(nav_snap.current_kural.kural.number, 2);
        assert!(matches!(
            nav_snap.playback_status,
            AudioPlaybackStatus::Loading
                | AudioPlaybackStatus::Playing
                | AudioPlaybackStatus::Completed
                | AudioPlaybackStatus::FallbackReady
        ));

        // Returning to Start resets content playback state cleanly back to Idle.
        handle.dispatch(BackendCommand::Navigate(AppScreen::Start));
        std::thread::sleep(Duration::from_millis(80));
        let end_snap = handle.current_snapshot();
        assert_eq!(end_snap.screen, AppScreen::Start);
        assert_eq!(end_snap.playback_status, AudioPlaybackStatus::Idle);
        assert!(!end_snap.can_replay_tts);

        handle.shutdown();
    }

    fn make_detected_outcome(age: AgeGroup) -> AgeOutcome {
        AgeOutcome::Detected(AgeDetection {
            age,
            confidence: 0.88,
            low_confidence: false,
            f0_hz: match age {
                AgeGroup::Kids => 290.0,
                AgeGroup::Youth => 195.0,
                AgeGroup::Adult => 130.0,
            },
            formant_dispersion_hz: 1050.0,
            vtl_cm: 14.5,
            latency_ms: 12,
        })
    }

    fn spawn_test_controller(
        cfg: &AppConfig,
    ) -> (
        BackendController,
        watch::Receiver<Arc<UiSnapshot>>,
        mpsc::Receiver<UiTransientEvent>,
        AudioHandle,
        AiWorkerHandle,
    ) {
        let corpus = KuralDb::open_with_models_dir(&cfg.paths.kural_db, &cfg.paths.models_dir)
            .expect("open corpus");
        let catalog = corpus.get_catalog().expect("catalog");
        let user_state = UserStateStore::open(&cfg.paths.user_state_db, cfg.audio.master_volume)
            .expect("open user_state");
        let (prefs, kids_progress, youth_progress, bookmarks) =
            user_state.load_snapshot_state(&corpus).expect("load state");
        let current_kural = corpus.get_kural_full(1).expect("kural 1");
        let pool = default_pool_or_fallback(&corpus, AppScreen::Start, 1).expect("pool");
        let (ai_worker, speech_caps) =
            AiWorkerHandle::spawn(&cfg.paths.models_dir, prefs.lang, &cfg.ai).expect("ai worker");
        let audio = AudioHandle::spawn(&cfg.audio, None, cfg.ai.tts_cache_max_bytes);
        let age_classifier = Arc::new(AgeClassifier::new(cfg.age_ai.clone()));
        let explainer = Arc::new(SemanticExplainer::new(corpus.clone(), cfg.ai.clone()));
        let requested_scholar = Scholar::MuVa;
        let effective_scholar = resolve_effective_scholar(&current_kural, requested_scholar);
        let initial_snapshot = Arc::new(UiSnapshot {
            prefs: prefs.clone(),
            screen: AppScreen::Start,
            modal: None,
            current_kural,
            requested_scholar,
            effective_scholar,
            active_filter: None,
            pool_len: pool.len(),
            pool_idx: 0,
            kids_progress,
            youth_progress,
            bookmarks,
            audio_state: AudioState::Idle,
            playback_status: AudioPlaybackStatus::Idle,
            has_replay_clip: false,
            recent_voice_turns: Vec::new(),
            can_replay_tts: false,
            can_replay_voice_turn: false,
            auto_speak_text: prefs.auto_speak_text,
            auto_age_route_on_start: true,
            voice_to_voice_loop: false,
            active_replay_label: None,
            system_health: SystemHealth {
                db_ok: true,
                user_state_ok: true,
                audio_ok: audio.is_healthy(),
                embeddings_loaded: corpus.embeddings_loaded(),
                vad_loaded: false,
                active_stt_loaded: speech_caps.active_stt_loaded,
                active_tts_loaded: speech_caps.active_tts_loaded,
                degraded_reason: None,
            },
        });
        let (snapshot_tx, snapshot_rx) = watch::channel(initial_snapshot.clone());
        let (event_tx, event_rx) = mpsc::channel(EVENT_QUEUE_CAP);
        let ctrl = BackendController::new(
            cfg.clone(),
            corpus,
            catalog,
            user_state,
            audio.clone(),
            age_classifier,
            explainer,
            ai_worker.clone(),
            snapshot_tx,
            initial_snapshot,
            event_tx,
            pool,
            speech_caps.tts_model_fingerprint,
        );
        (ctrl, snapshot_rx, event_rx, audio, ai_worker)
    }

    #[test]
    fn resolve_start_voice_age_route_handles_acoustic_keywords_and_retry() {
        assert_eq!(
            resolve_start_voice_age_route(&make_detected_outcome(AgeGroup::Kids), None),
            Some(AgeGroup::Kids)
        );
        assert_eq!(
            resolve_start_voice_age_route(&make_detected_outcome(AgeGroup::Youth), Some("hello")),
            Some(AgeGroup::Youth)
        );
        assert_eq!(
            resolve_start_voice_age_route(&make_detected_outcome(AgeGroup::Adult), None),
            Some(AgeGroup::Adult)
        );

        // Explicit English & Tamil keywords override acoustic detection or Retry.
        let retry = AgeOutcome::Retry(RetryReason::TooShort);
        assert_eq!(resolve_start_voice_age_route(&retry, None), None);
        assert_eq!(
            resolve_start_voice_age_route(&retry, Some("I am a kid")),
            Some(AgeGroup::Kids)
        );
        assert_eq!(
            resolve_start_voice_age_route(
                &make_detected_outcome(AgeGroup::Adult),
                Some("சிறுவர் பகுதி")
            ),
            Some(AgeGroup::Kids)
        );
        assert_eq!(
            resolve_start_voice_age_route(&retry, Some("குழந்தை")),
            Some(AgeGroup::Kids)
        );
        assert_eq!(
            resolve_start_voice_age_route(&retry, Some("Open youth student mode")),
            Some(AgeGroup::Youth)
        );
        assert_eq!(
            resolve_start_voice_age_route(&retry, Some("இளைஞர் மாணவர்")),
            Some(AgeGroup::Youth)
        );
        assert_eq!(
            resolve_start_voice_age_route(
                &make_detected_outcome(AgeGroup::Kids),
                Some("elder scholar")
            ),
            Some(AgeGroup::Adult)
        );
        assert_eq!(
            resolve_start_voice_age_route(&retry, Some("பெரியவர் அறிஞர்")),
            Some(AgeGroup::Adult)
        );
        assert_eq!(
            resolve_start_voice_age_route(&retry, Some("பெரியோர்")),
            Some(AgeGroup::Adult)
        );
    }

    #[test]
    fn extract_opening_followup_command_ignores_greetings_and_routes_commands() {
        let ctx = RouterContext {
            screen: AppScreen::Youth,
            modal: None,
            active_kural: 481,
            active_scholar: Scholar::MuVa,
            speech_speed: 1.0,
            muted: false,
        };

        for ignored in [
            None,
            Some(""),
            Some("hello"),
            Some("hi"),
            Some("vanakkam"),
            Some("வணக்கம்"),
            Some("hello i want to learn thirukkural"),
            Some("tell me"),
            Some("Explain Kural 1"),
            Some("kids"),
            Some("open youth mode"),
            Some("i am an adult"),
            Some("சிறுவர் பகுதி"),
            Some("பெரியவர் பகுதி"),
        ] {
            assert_eq!(
                extract_opening_followup_command(ignored, Language::English, ctx),
                None,
                "expected greeting/age/non-nav utterance to be ignored: {ignored:?}"
            );
        }

        assert_eq!(
            extract_opening_followup_command(Some("Open Kural 42"), Language::English, ctx),
            Some(BackendCommand::OpenKural(42))
        );
        assert_eq!(
            extract_opening_followup_command(Some("Adhikaram 10"), Language::English, ctx),
            Some(BackendCommand::OpenAdhikaram(10))
        );
        assert_eq!(
            extract_opening_followup_command(Some("Discuss"), Language::English, ctx),
            Some(BackendCommand::OpenModal(ModalKind::AiChat {
                seed_kural: Some(481),
            }))
        );
        assert_eq!(
            extract_opening_followup_command(Some("விவாதி"), Language::Tamil, ctx),
            Some(BackendCommand::OpenModal(ModalKind::AiChat {
                seed_kural: Some(481),
            }))
        );
    }

    #[test]
    fn start_voice_auto_route_navigates_with_toast_and_guards_stale_results() {
        let tmp = TempDir::new().expect("tempdir");
        let cfg = test_config(&tmp);
        let rt = tokio::runtime::Runtime::new().unwrap();

        rt.block_on(async {
            for (age, expected_screen, expected_toast_en) in [
                (AgeGroup::Kids, AppScreen::Kids, AGE_MODE_OPENED_KIDS.en),
                (AgeGroup::Youth, AppScreen::Youth, AGE_MODE_OPENED_YOUTH.en),
                (AgeGroup::Adult, AppScreen::Adult, AGE_MODE_OPENED_ADULT.en),
            ] {
                let sub_dir = tmp.path().join(format!("case_{age}"));
                std::fs::create_dir_all(&sub_dir).unwrap();
                let mut sub_cfg = cfg.clone();
                sub_cfg.paths.user_state_db = sub_dir.join("user_state.db");

                let (mut ctrl, snap_rx, mut evt_rx, audio, ai_worker) =
                    spawn_test_controller(&sub_cfg);
                assert!(snap_rx.borrow().auto_age_route_on_start);

                ctrl.handle_start_voice_auto_route_done(StartVoiceAutoRouteData {
                    session_id: ctrl.voice_session_id,
                    nav_epoch: ctrl.nav_epoch,
                    lang: Language::English,
                    auto_triggered: true,
                    age_timed_out: false,
                    outcome: make_detected_outcome(age),
                    transcript: Some("hello i want to learn thirukkural".to_string()),
                    caps: None,
                });

                assert_eq!(ctrl.state.screen, expected_screen);
                assert_eq!(snap_rx.borrow().screen, expected_screen);

                let mut saw_toast = false;
                let mut saw_age_result = false;
                while let Ok(evt) = evt_rx.try_recv() {
                    match evt {
                        UiTransientEvent::InfoToast { message_en, .. }
                            if message_en == expected_toast_en =>
                        {
                            saw_toast = true;
                        }
                        UiTransientEvent::AgeResult(AgeOutcome::Detected(det))
                            if det.age == age =>
                        {
                            saw_age_result = true;
                        }
                        _ => {}
                    }
                }
                assert!(saw_toast, "expected bilingual opened toast for {age}");
                assert!(saw_age_result, "expected AgeResult for {age}");

                ctrl.cancel_active_tts();
                ctrl.cancel_active_capture();
                ai_worker.shutdown();
                audio.shutdown();
            }

            // Race guard: manual SelectAge increments nav_epoch and leaves Start; stale result is dropped.
            let stale_dir = tmp.path().join("case_stale");
            std::fs::create_dir_all(&stale_dir).unwrap();
            let mut stale_cfg = cfg.clone();
            stale_cfg.paths.user_state_db = stale_dir.join("user_state.db");
            let (mut ctrl, _snap_rx, mut evt_rx, audio, ai_worker) =
                spawn_test_controller(&stale_cfg);

            let stale_session = ctrl.voice_session_id;
            let stale_epoch = ctrl.nav_epoch;
            ctrl.handle_command(BackendCommand::SelectAge(AgeGroup::Adult));
            assert_eq!(ctrl.state.screen, AppScreen::Adult);
            assert_ne!(ctrl.nav_epoch, stale_epoch);

            ctrl.handle_start_voice_auto_route_done(StartVoiceAutoRouteData {
                session_id: stale_session,
                nav_epoch: stale_epoch,
                lang: Language::English,
                auto_triggered: true,
                age_timed_out: false,
                outcome: make_detected_outcome(AgeGroup::Kids),
                transcript: Some("Open Kural 42".to_string()),
                caps: None,
            });
            assert_eq!(ctrl.state.screen, AppScreen::Adult);
            assert_ne!(ctrl.state.current_kural.kural.number, 42);
            while let Ok(evt) = evt_rx.try_recv() {
                assert!(
                    !matches!(evt, UiTransientEvent::AgeResult(_)),
                    "stale result must not emit AgeResult"
                );
            }

            // Opening follow-up command & question routing.
            let cmd_dir = tmp.path().join("case_followup_cmd");
            std::fs::create_dir_all(&cmd_dir).unwrap();
            let mut cmd_cfg = cfg.clone();
            cmd_cfg.paths.user_state_db = cmd_dir.join("user_state.db");
            let (mut ctrl_cmd, _snap_rx2, _evt_rx2, audio2, ai_worker2) =
                spawn_test_controller(&cmd_cfg);
            ctrl_cmd.handle_start_voice_auto_route_done(StartVoiceAutoRouteData {
                session_id: ctrl_cmd.voice_session_id,
                nav_epoch: ctrl_cmd.nav_epoch,
                lang: Language::English,
                auto_triggered: true,
                age_timed_out: false,
                outcome: make_detected_outcome(AgeGroup::Youth),
                transcript: Some("Open Kural 42".to_string()),
                caps: None,
            });
            assert_eq!(ctrl_cmd.state.screen, AppScreen::Youth);
            assert_eq!(ctrl_cmd.state.current_kural.kural.number, 42);

            // Reset back to Start and verify multi-word Start speech NEVER auto-opens Discuss.
            ctrl_cmd.handle_command(BackendCommand::Navigate(AppScreen::Start));
            ctrl_cmd.handle_start_voice_auto_route_done(StartVoiceAutoRouteData {
                session_id: ctrl_cmd.voice_session_id,
                nav_epoch: ctrl_cmd.nav_epoch,
                lang: Language::English,
                auto_triggered: false,
                age_timed_out: false,
                outcome: make_detected_outcome(AgeGroup::Adult),
                transcript: Some("Explain Kural 1".to_string()),
                caps: None,
            });
            assert_eq!(ctrl_cmd.state.screen, AppScreen::Adult);
            assert!(ctrl_cmd.state.modal.is_none());
            assert!(ctrl_cmd.state.recent_voice_turns.is_empty());
            assert_eq!(
                ctrl_cmd.default_content_target(),
                Some(kural_core::SpeakTarget::KuralAndMeaning)
            );

            // Explicit "Discuss" on Start-screen voice detection opens ModalKind::AiChat.
            ctrl_cmd.handle_command(BackendCommand::Navigate(AppScreen::Start));
            ctrl_cmd.handle_start_voice_auto_route_done(StartVoiceAutoRouteData {
                session_id: ctrl_cmd.voice_session_id,
                nav_epoch: ctrl_cmd.nav_epoch,
                lang: Language::English,
                auto_triggered: false,
                age_timed_out: false,
                outcome: make_detected_outcome(AgeGroup::Adult),
                transcript: Some("Discuss".to_string()),
                caps: None,
            });
            assert_eq!(ctrl_cmd.state.screen, AppScreen::Adult);
            assert!(matches!(
                ctrl_cmd.state.modal,
                Some(ModalKind::AiChat { .. })
            ));

            ctrl_cmd.cancel_active_tts();
            ctrl_cmd.cancel_active_capture();
            ai_worker2.shutdown();
            audio2.shutdown();
            ctrl.cancel_active_tts();
            ctrl.cancel_active_capture();
            ai_worker.shutdown();
            audio.shutdown();
        });
    }

    #[test]
    fn adult_entry_default_target_and_manual_ai_chat_bilingual() {
        let tmp = TempDir::new().expect("tempdir");
        let cfg = test_config(&tmp);
        let handle = BackendHandle::start(cfg).expect("BackendHandle::start");
        let mut events = handle.subscribe_events();
        let rt = tokio::runtime::Runtime::new().unwrap();

        // Manual entry to Adult keeps Discuss closed and defaults to KuralAndMeaning.
        handle.dispatch(BackendCommand::SelectAge(AgeGroup::Adult));
        handle.dispatch(BackendCommand::OpenKural(395));
        handle.dispatch(BackendCommand::SelectCommentary(Scholar::Parimel));
        std::thread::sleep(Duration::from_millis(80));
        let adult_snap = handle.current_snapshot();
        assert_eq!(adult_snap.screen, AppScreen::Adult);
        assert!(adult_snap.modal.is_none());

        // Speaking current ScholarCommentary(Parimel) while requested_scholar is already Parimel
        // must not emit a duplicate CommentaryFallbackNotice.
        handle.dispatch(BackendCommand::SpeakCurrent(
            kural_core::SpeakTarget::ScholarCommentary(Scholar::Parimel),
        ));
        std::thread::sleep(Duration::from_millis(80));
        let mut fallback_notices = 0usize;
        while let Ok(evt) = events.try_recv() {
            if matches!(evt, UiTransientEvent::CommentaryFallbackNotice { .. }) {
                fallback_notices += 1;
            }
        }
        assert_eq!(fallback_notices, 1);

        // Explicitly opening Discuss (ModalKind::AiChat) and asking questions works in English & Tamil.
        handle.dispatch(BackendCommand::OpenModal(ModalKind::AiChat {
            seed_kural: Some(1),
        }));
        std::thread::sleep(Duration::from_millis(50));
        assert!(matches!(
            handle.current_snapshot().modal,
            Some(ModalKind::AiChat {
                seed_kural: Some(1)
            })
        ));

        handle.dispatch(BackendCommand::SetLanguage(Language::English));
        handle.dispatch(BackendCommand::AskAiQuestion {
            query: "What does Kural 1 teach?".to_string(),
            seed_kural: Some(1),
        });
        let en_evt = rt.block_on(recv_until(&mut events, |e| {
            matches!(e, UiTransientEvent::AiAnswerReady(_))
        }));
        let Some(UiTransientEvent::AiAnswerReady(en_ans)) = en_evt else {
            panic!("expected English AiAnswerReady");
        };
        assert!(!en_ans.answer_en.trim().is_empty());

        handle.dispatch(BackendCommand::SetLanguage(Language::Tamil));
        handle.dispatch(BackendCommand::AskAiQuestion {
            query: "குறள் 1 பொருள் என்ன?".to_string(),
            seed_kural: Some(1),
        });
        let ta_evt = rt.block_on(recv_until(&mut events, |e| {
            matches!(e, UiTransientEvent::AiAnswerReady(_))
        }));
        let Some(UiTransientEvent::AiAnswerReady(ta_ans)) = ta_evt else {
            panic!("expected Tamil AiAnswerReady");
        };
        assert!(!ta_ans.answer_ta.trim().is_empty());

        handle.shutdown();
    }

    #[test]
    fn voice_garbage_guard_single_digit_kural_and_range_errors() {
        let tmp = TempDir::new().expect("tempdir");
        let cfg = test_config(&tmp);
        let rt = tokio::runtime::Runtime::new().unwrap();

        rt.block_on(async {
            let (mut ctrl, _snap_rx, mut evt_rx, audio, ai_worker) = spawn_test_controller(&cfg);
            ctrl.handle_command(BackendCommand::SelectAge(AgeGroup::Youth));
            while evt_rx.try_recv().is_ok() {}

            // Empty, punctuation-only, and 1-char noise must emit VOICE_UNCLEAR_RETRY_NOTICE
            // and NEVER route into SemanticExplainer / AiChat.
            for noise in ["", "   ", "...", "?!", "a", "அ", "0"] {
                ctrl.route_recognized_voice(
                    noise.to_string(),
                    Language::Tamil,
                    None,
                    ctrl.voice_session_id,
                    ctrl.nav_epoch,
                );
                let mut saw_unclear = false;
                while let Ok(evt) = evt_rx.try_recv() {
                    if let UiTransientEvent::InfoToast { message_en, .. } = &evt
                        && message_en == VOICE_UNCLEAR_RETRY_NOTICE.en
                    {
                        saw_unclear = true;
                    }
                    assert!(
                        !matches!(evt, UiTransientEvent::AiAnswerReady(_)),
                        "noise {noise:?} must never reach AI Q&A"
                    );
                }
                assert!(
                    saw_unclear,
                    "expected VOICE_UNCLEAR_RETRY_NOTICE for {noise:?}"
                );
                assert!(ctrl.state.modal.is_none());
                assert!(ctrl.state.recent_voice_turns.is_empty());
            }

            // Out-of-range numbers emit VOICE_KURAL_RANGE_ERROR ErrorToast.
            for oor in ["1500", "kural 0", "குறள் 2000"] {
                ctrl.route_recognized_voice(
                    oor.to_string(),
                    Language::English,
                    None,
                    ctrl.voice_session_id,
                    ctrl.nav_epoch,
                );
                let mut saw_range_err = false;
                while let Ok(evt) = evt_rx.try_recv() {
                    if let UiTransientEvent::ErrorToast { message_en, .. } = &evt
                        && message_en == VOICE_KURAL_RANGE_ERROR.en
                    {
                        saw_range_err = true;
                    }
                }
                assert!(
                    saw_range_err,
                    "expected VOICE_KURAL_RANGE_ERROR for {oor:?}"
                );
            }

            // Single-digit Kural ("1" and Tamil "௪" = 4) opens Kural & forces KuralAndMeaning.
            ctrl.route_recognized_voice(
                "1".to_string(),
                Language::English,
                None,
                ctrl.voice_session_id,
                ctrl.nav_epoch,
            );
            assert_eq!(ctrl.state.current_kural.kural.number, 1);
            assert_eq!(
                ctrl.active_content_key.map(|k| k.target),
                Some(kural_core::SpeakTarget::KuralAndMeaning)
            );

            ctrl.route_recognized_voice(
                "௪".to_string(),
                Language::Tamil,
                None,
                ctrl.voice_session_id,
                ctrl.nav_epoch,
            );
            assert_eq!(ctrl.state.current_kural.kural.number, 4);
            assert_eq!(
                ctrl.active_content_key.map(|k| k.target),
                Some(kural_core::SpeakTarget::KuralAndMeaning)
            );

            // Configured voice_loop_max_silence_timeouts disarms loop cleanly.
            ctrl.config.audio.voice_loop_max_silence_timeouts = 3;
            ctrl.state.voice_to_voice_loop = true;
            for step in 1..=3 {
                ctrl.route_recognized_voice(
                    "...".to_string(),
                    Language::English,
                    None,
                    ctrl.voice_session_id,
                    ctrl.nav_epoch,
                );
                if step < 3 {
                    assert!(ctrl.state.voice_to_voice_loop);
                } else {
                    assert!(!ctrl.state.voice_to_voice_loop);
                }
            }

            ctrl.cancel_active_tts();
            ctrl.cancel_active_capture();
            ai_worker.shutdown();
            audio.shutdown();
        });
    }

    #[test]
    fn voice_open_kural_forces_kural_and_meaning_across_screens_even_with_auto_speak_off() {
        let tmp = TempDir::new().expect("tempdir");
        let cfg = test_config(&tmp);
        let rt = tokio::runtime::Runtime::new().unwrap();

        rt.block_on(async {
            let (mut ctrl, snap_rx, _evt_rx, audio, ai_worker) = spawn_test_controller(&cfg);
            ctrl.handle_command(BackendCommand::SetAutoSpeakText(false));

            for age in [AgeGroup::Kids, AgeGroup::Youth, AgeGroup::Adult] {
                ctrl.handle_command(BackendCommand::SelectAge(age));
                for lang in [Language::Tamil, Language::English] {
                    ctrl.state.prefs.lang = lang;
                    ctrl.state.modal = Some(ModalKind::Search {
                        initial_tab: SearchTab::Query,
                    });
                    ctrl.route_recognized_voice(
                        "read kural 42".to_string(),
                        lang,
                        None,
                        ctrl.voice_session_id,
                        ctrl.nav_epoch,
                    );
                    let snap = snap_rx.borrow().clone();
                    assert_eq!(snap.current_kural.kural.number, 42);
                    assert!(snap.modal.is_none());
                    assert_eq!(
                        ctrl.active_content_key,
                        Some(ContentAudioKey {
                            screen: AppScreen::from(age),
                            kural_number: 42,
                            lang,
                            effective_scholar: snap.effective_scholar,
                            target: kural_core::SpeakTarget::KuralAndMeaning,
                        })
                    );
                    assert_eq!(snap.playback_status, AudioPlaybackStatus::Loading);
                }
            }

            ctrl.cancel_active_tts();
            ctrl.cancel_active_capture();
            ai_worker.shutdown();
            audio.shutdown();
        });
    }

    #[test]
    fn degraded_reason_and_startup_info_toast_emit_once_when_models_missing() {
        let tmp = TempDir::new().expect("tempdir");
        let cfg = test_config(&tmp);
        let handle = BackendHandle::start(cfg).expect("BackendHandle::start");
        let mut events = handle.subscribe_events();
        let snap = handle.current_snapshot();
        assert!(snap.system_health.degraded_reason.is_some());

        let mut startup_toasts = 0usize;
        while let Ok(evt) = events.try_recv() {
            if let UiTransientEvent::InfoToast { message_en, .. } = evt
                && message_en.starts_with("Running in fallback mode")
            {
                startup_toasts += 1;
            }
        }
        assert_eq!(startup_toasts, 1);
        handle.shutdown();
    }

    #[test]
    fn ai_timeouts_emit_bilingual_error_toast_deterministically() {
        let tmp = TempDir::new().expect("tempdir");
        let cfg = test_config(&tmp);
        let rt = tokio::runtime::Runtime::new().unwrap();

        rt.block_on(async {
            let (mut ctrl, snap_rx, mut evt_rx, audio, ai_worker) = spawn_test_controller(&cfg);
            while evt_rx.try_recv().is_ok() {}

            // 1. Elapsed tokio::time::timeout -> TextAiDone -> ErrorToast (bilingual)
            let timed_out_text: Result<GroundedAnswer, BilingualMsg> =
                match tokio::time::timeout(Duration::from_millis(1), std::future::pending::<()>())
                    .await
                {
                    Ok(()) => unreachable!("pending future cannot complete"),
                    Err(_) => Err(ai_inference_timeout_msg(5000)),
                };
            ctrl.ai_epoch = ctrl.ai_epoch.wrapping_add(1);
            let cur_ai_epoch = ctrl.ai_epoch;
            ctrl.handle_internal(InternalMsg::TextAiDone {
                ai_epoch: cur_ai_epoch,
                result: timed_out_text,
            });
            let text_toast = evt_rx.try_recv().expect("TextAiDone ErrorToast");
            match text_toast {
                UiTransientEvent::ErrorToast {
                    message_ta,
                    message_en,
                } => {
                    assert!(!message_ta.trim().is_empty());
                    assert!(!message_en.trim().is_empty());
                    assert!(message_ta.contains("5000 ms"));
                    assert!(message_en.contains("5000 ms"));
                }
                other => panic!("expected ErrorToast for TextAiDone, got {other:?}"),
            }

            // 2. Elapsed tokio::time::timeout -> VoiceAiDone -> disarms voice loop + ErrorToast
            ctrl.state.voice_to_voice_loop = true;
            let timed_out_voice_ai: Result<GroundedAnswer, BilingualMsg> =
                match tokio::time::timeout(Duration::from_millis(1), std::future::pending::<()>())
                    .await
                {
                    Ok(()) => unreachable!("pending future cannot complete"),
                    Err(_) => Err(ai_inference_timeout_msg(4321)),
                };
            ctrl.handle_internal(InternalMsg::VoiceAiDone {
                session_id: ctrl.voice_session_id,
                nav_epoch: ctrl.nav_epoch,
                turn_id: None,
                seed_kural: Some(1),
                lang: Language::Tamil,
                result: timed_out_voice_ai,
            });
            assert!(!snap_rx.borrow().voice_to_voice_loop);
            let voice_ai_toast = evt_rx.try_recv().expect("VoiceAiDone ErrorToast");
            match voice_ai_toast {
                UiTransientEvent::ErrorToast {
                    message_ta,
                    message_en,
                } => {
                    assert!(!message_ta.trim().is_empty());
                    assert!(!message_en.trim().is_empty());
                    assert!(message_ta.contains("4321 ms"));
                    assert!(message_en.contains("4321 ms"));
                }
                other => panic!("expected ErrorToast for VoiceAiDone, got {other:?}"),
            }

            // 3. AiError::Timeout (STT) -> VoiceCmdTranscribed -> disarms loop + ErrorToast
            ctrl.state.voice_to_voice_loop = true;
            let stt_err = AiError::Timeout {
                operation: "stt_transcribe",
                timeout_ms: 8000,
            };
            let stt_msg = match stt_err {
                AiError::Timeout { timeout_ms, .. } => stt_timeout_msg(timeout_ms),
                other => unreachable!("unexpected error: {other}"),
            };
            ctrl.handle_internal(InternalMsg::VoiceCmdTranscribed {
                session_id: ctrl.voice_session_id,
                nav_epoch: ctrl.nav_epoch,
                lang: Language::English,
                is_timeout: false,
                captured_user_16k: None,
                transcript: Err(stt_msg),
                caps: None,
            });
            assert!(!snap_rx.borrow().voice_to_voice_loop);
            let stt_toast = evt_rx.try_recv().expect("VoiceCmdTranscribed ErrorToast");
            match stt_toast {
                UiTransientEvent::ErrorToast {
                    message_ta,
                    message_en,
                } => {
                    assert!(!message_ta.trim().is_empty());
                    assert!(!message_en.trim().is_empty());
                    assert!(message_ta.contains("8000 ms"));
                    assert!(message_en.contains("8000 ms"));
                }
                other => panic!("expected ErrorToast for VoiceCmdTranscribed, got {other:?}"),
            }

            // 4. AiError::Timeout (TTS) -> TtsFinished -> resets playback + ErrorToast
            ctrl.handle_command(BackendCommand::SelectAge(AgeGroup::Youth));
            ctrl.state.playback_status = AudioPlaybackStatus::Loading;
            let tts_err = AiError::Timeout {
                operation: "tts_synthesize",
                timeout_ms: 15000,
            };
            let tts_msg = match tts_err {
                AiError::Timeout { timeout_ms, .. } => tts_timeout_msg(timeout_ms),
                other => unreachable!("unexpected error: {other}"),
            };
            ctrl.handle_internal(InternalMsg::TtsFinished {
                tts_epoch: ctrl.tts_epoch,
                nav_epoch: ctrl.nav_epoch,
                target: kural_core::SpeakTarget::KuralAndMeaning,
                kural_number: ctrl.state.current_kural.kural.number,
                voice_turn_id: None,
                has_saved_audio: false,
                voice_loop_rearm: false,
                ok: false,
                error_msg: Some(tts_msg),
            });
            assert_eq!(
                snap_rx.borrow().playback_status,
                AudioPlaybackStatus::FallbackReady
            );
            let tts_toast = evt_rx.try_recv().expect("TtsFinished ErrorToast");
            match tts_toast {
                UiTransientEvent::ErrorToast {
                    message_ta,
                    message_en,
                } => {
                    assert!(!message_ta.trim().is_empty());
                    assert!(!message_en.trim().is_empty());
                    assert!(message_ta.contains("15000 ms"));
                    assert!(message_en.contains("15000 ms"));
                }
                other => panic!("expected ErrorToast for TtsFinished, got {other:?}"),
            }

            ctrl.cancel_active_tts();
            ctrl.cancel_active_capture();
            ai_worker.shutdown();
            audio.shutdown();
        });
    }
}
