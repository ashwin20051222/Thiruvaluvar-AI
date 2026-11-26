use crate::{
    Adhikaram, AgeGroup, CoreError, Division, FullKural, Iyal, KnowledgeEntry, KuralSummary, Lang,
    Language, ResearchTheme, SearchOutcome,
};
use serde::{Deserialize, Serialize};
use std::collections::BTreeSet;
use std::fmt;
use std::str::FromStr;

/// Canonical discrete user TTS speed presets (`0.75x`, `1.0x`, `1.25x`).
pub const SPEECH_SPEED_PRESETS: [f32; 3] = [0.75, 1.0, 1.25];

/// Snap any raw `f32` speed value onto the nearest supported preset `{0.75, 1.0, 1.25}`.
pub fn snap_speech_speed(raw: f32) -> f32 {
    if !raw.is_finite() {
        return 1.0;
    }
    SPEECH_SPEED_PRESETS
        .iter()
        .copied()
        .min_by(|a, b| (*a - raw).abs().total_cmp(&(*b - raw).abs()))
        .unwrap_or(1.0)
}

/// Step the user's speed preset by `delta` (`-1` slower, `+1` faster), clamped to `0..=2`.
pub fn step_speech_speed(current: f32, delta: i8) -> f32 {
    let snapped = snap_speech_speed(current);
    let cur_idx = SPEECH_SPEED_PRESETS
        .iter()
        .position(|&p| (p - snapped).abs() < 0.01)
        .unwrap_or(1) as isize;
    let next_idx = (cur_idx + isize::from(delta)).clamp(0, 2) as usize;
    SPEECH_SPEED_PRESETS[next_idx]
}

/// Subtle persona cadence multiplier applied on top of user-chosen speed.
pub const fn age_cadence_factor(screen: AppScreen) -> f32 {
    match screen {
        AppScreen::Kids => 0.95,
        AppScreen::Youth => 1.02,
        AppScreen::Adult | AppScreen::Start => 1.00,
    }
}

/// Final runtime TTS rate combining user speed pill and age-appropriate cadence.
pub fn effective_tts_speed(user_speed: f32, screen: AppScreen) -> f32 {
    (snap_speech_speed(user_speed) * age_cadence_factor(screen)).clamp(0.65, 1.40)
}

/// UI color theme.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Theme {
    Dark,
    Light,
}

impl Theme {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Dark => "dark",
            Self::Light => "light",
        }
    }
}

impl fmt::Display for Theme {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}

impl FromStr for Theme {
    type Err = CoreError;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s.trim().to_ascii_lowercase().as_str() {
            "dark" => Ok(Self::Dark),
            "light" => Ok(Self::Light),
            other => Err(CoreError::InvalidTheme(other.to_string())),
        }
    }
}

/// Active screen route in `kural-app`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum AppScreen {
    Start,
    Kids,
    Youth,
    Adult,
}

impl AppScreen {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Start => "start",
            Self::Kids => "kids",
            Self::Youth => "youth",
            Self::Adult => "adult",
        }
    }
}

impl fmt::Display for AppScreen {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}

impl FromStr for AppScreen {
    type Err = CoreError;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s.trim().to_ascii_lowercase().as_str() {
            "start" | "home" => Ok(Self::Start),
            "kids" | "child" => Ok(Self::Kids),
            "youth" | "teen" => Ok(Self::Youth),
            "adult" | "scholar" => Ok(Self::Adult),
            other => Err(CoreError::InvalidAppScreen(other.to_string())),
        }
    }
}

impl From<AgeGroup> for AppScreen {
    fn from(age: AgeGroup) -> Self {
        match age {
            AgeGroup::Kids => Self::Kids,
            AgeGroup::Youth => Self::Youth,
            AgeGroup::Adult => Self::Adult,
        }
    }
}

/// Active tab inside the Search modal.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SearchTab {
    Query,
    Divisions,
}

impl SearchTab {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Query => "query",
            Self::Divisions => "divisions",
        }
    }
}

impl fmt::Display for SearchTab {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}

impl FromStr for SearchTab {
    type Err = CoreError;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s.trim().to_ascii_lowercase().as_str() {
            "query" | "search" => Ok(Self::Query),
            "divisions" | "paals" => Ok(Self::Divisions),
            other => Err(CoreError::InvalidSearchTab(other.to_string())),
        }
    }
}

/// Zero-scroll overlay modals supported across desktop and `480×320` Pi LCD.
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

/// Commentary scholar tab in Adult mode (`S4`) and TTS target.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Scholar {
    MuVa,
    Solomon,
    Kalaignar,
    Parimel,
    English,
}

impl Scholar {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::MuVa => "mu_va",
            Self::Solomon => "solomon",
            Self::Kalaignar => "kalaignar",
            Self::Parimel => "parimel",
            Self::English => "english",
        }
    }
}

impl fmt::Display for Scholar {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}

impl FromStr for Scholar {
    type Err = CoreError;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s.trim().to_ascii_lowercase().as_str() {
            "mu_va" | "muva" => Ok(Self::MuVa),
            "solomon" => Ok(Self::Solomon),
            "kalaignar" => Ok(Self::Kalaignar),
            "parimel" | "parimelazhagar" => Ok(Self::Parimel),
            "english" => Ok(Self::English),
            other => Err(CoreError::InvalidScholar(other.to_string())),
        }
    }
}

/// Target content scope for TTS synthesis and playback.
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

/// Compact UI panel hint for Raspberry Pi `480×320` `#pi-mode-strip` synchronization.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum CompactTabHint {
    Verse,
    Analysis,
    Quiz,
    Commentary,
    Study,
}

/// Half-duplex audio engine state shared with UI HUD.
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

/// Lifecycle state for automatic initial content playback and persistent Replay Audio.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum AudioPlaybackStatus {
    /// Start screen or no active content audio.
    #[default]
    Idle,
    /// Initial automatic playback triggered and synthesizing/buffering.
    Loading,
    /// Initial automatic playback actively playing.
    Playing,
    /// Initial playback finished cleanly; persistent Replay Audio is available.
    Completed,
    /// Manual Replay Audio currently playing from the beginning.
    Replaying,
    /// Auto-playback blocked/unavailable/disabled/stopped before completion; show one-click Play Audio fallback.
    FallbackReady,
}

/// Microphone capture mode requested from `AudioHandle`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum CaptureMode {
    FixedAgeSample { duration_ms: u32 },
    VadCommand { max_duration_ms: u32 },
}

/// Speech quality gate rejection reason during Voice Age Detection.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum RetryReason {
    TooQuiet,
    TooShort,
    TooNoisy,
    Clipped,
    HardwareUnavailable,
}

/// Acoustic Age AI prediction telemetry and confidence result.
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

/// Outcome returned by Voice Age AI.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum AgeOutcome {
    Detected(AgeDetection),
    Retry(RetryReason),
}

/// Active browsing filter applied to `PrevKural` / `NextKural` stepping.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ActiveFilter {
    Division(u8),
    Tag(String),
    ResearchTheme(ResearchTheme),
}

/// Complete hierarchical structure cached once in memory for `SearchTab::Divisions`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct CorpusCatalog {
    pub divisions: Vec<Division>,
    pub iyals: Vec<Iyal>,
    pub adhikarams: Vec<Adhikaram>,
}

/// Grounded offline AI response with verbatim cited SQLite records.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct GroundedAnswer {
    pub answer_ta: String,
    pub answer_en: String,
    pub cited_kurals: Vec<FullKural>,
    pub kb_hits: Vec<KnowledgeEntry>,
    pub out_of_scope: bool,
    pub latency_ms: u32,
}

/// Offline subsystem health flags shown in UI status indicators.
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

/// Replay target mode for a captured voice-to-voice turn.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum VoiceReplayMode {
    AiReplyOnly,
    UserVoiceOnly,
    FullExchange,
}

/// UI metadata summary of a recent voice-to-voice turn (RAM-bounded, last 5 turns).
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

const fn default_auto_speak_text() -> bool {
    true
}

/// Persisted user preferences stored in `data/user_state.db`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct UserPrefs {
    pub lang: Language,
    pub theme: Theme,
    pub speech_speed: f32,
    pub volume: f32,
    pub muted: bool,
    pub default_age: AgeGroup,
    pub last_kural_kids: u16,
    pub last_kural_youth: u16,
    pub last_kural_adult: u16,
    #[serde(default = "default_auto_speak_text")]
    pub auto_speak_text: bool,
}

impl Default for UserPrefs {
    fn default() -> Self {
        Self {
            lang: Language::Tamil,
            theme: Theme::Dark,
            speech_speed: 1.0,
            volume: 0.85,
            muted: false,
            default_age: AgeGroup::Youth,
            last_kural_kids: 1,
            last_kural_youth: 481,
            last_kural_adult: 423,
            auto_speak_text: true,
        }
    }
}

impl UserPrefs {
    /// Return persisted cursor Kural number for `age`.
    pub const fn last_kural_for(&self, age: AgeGroup) -> u16 {
        match age {
            AgeGroup::Kids => self.last_kural_kids,
            AgeGroup::Youth => self.last_kural_youth,
            AgeGroup::Adult => self.last_kural_adult,
        }
    }

    /// Update persisted cursor Kural number for `age`, ignoring out-of-range values.
    pub fn set_last_kural_for(&mut self, age: AgeGroup, number: u16) {
        if !(1..=1330).contains(&number) {
            return;
        }
        match age {
            AgeGroup::Kids => self.last_kural_kids = number,
            AgeGroup::Youth => self.last_kural_youth = number,
            AgeGroup::Adult => self.last_kural_adult = number,
        }
    }
}

/// Kids mode learning progress persisted across restarts.
#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize)]
pub struct KidsProgress {
    pub stars_earned: u32,
    pub completed_stories: BTreeSet<u16>,
    pub completed_games: BTreeSet<u16>,
}

/// Youth mode streak and quiz XP progress persisted across restarts.
#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize)]
pub struct YouthProgress {
    pub daily_streak: u32,
    pub last_active_date: String,
    pub quiz_xp: u32,
    pub answered_quizzes: BTreeSet<u16>,
}

/// Adult mode scholarly bookmark persisted in `data/user_state.db`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct BookmarkEntry {
    pub kural_number: u16,
    pub preferred_scholar: Scholar,
    pub adhikaram_ta: String,
    pub adhikaram_en: String,
    pub line1_ta: String,
    pub created_at: String,
}

/// Unified command channel fed identically by UI touch clicks and voice commands.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[non_exhaustive]
pub enum BackendCommand {
    SetLanguage(Language),
    SetTheme(Theme),
    Navigate(AppScreen),
    SelectAge(AgeGroup),
    OpenModal(ModalKind),
    CloseModal,

    EnterStartAutoListen,
    StartVoiceAgeDetect,
    StartVoiceCommand,
    CancelVoice,

    SubmitTextSearch {
        query: String,
        division_id: Option<u8>,
    },
    OpenKural(u16),
    NextKural,
    PrevKural,
    OpenAdhikaram(u8),
    FilterByDivision(Option<u8>),
    FilterByTag(Option<String>),
    FilterByResearchTheme(Option<ResearchTheme>),
    ClearFilter,

    SelectCommentary(Scholar),
    SpeakCurrent(SpeakTarget),
    SpeakCustomText {
        text: String,
        lang: Lang,
        label: Option<String>,
    },
    ReplayAudio,
    ReplayLastTts,
    ReplayVoiceTurn {
        turn_id: Option<u64>,
        mode: VoiceReplayMode,
    },
    SetAutoSpeakText(bool),
    SetVoiceToVoiceLoop(bool),
    StopAudio,
    SetSpeechSpeed(f32),
    SetMuted(bool),
    AdjustVolume(f32),

    SubmitKidsGameAnswer {
        kural_number: u16,
        choice_idx: u8,
    },
    CompleteKidsStory(u16),
    SubmitYouthQuizAnswer {
        kural_number: u16,
        choice_idx: u8,
    },
    ToggleBookmark(u16),

    AskAiQuestion {
        query: String,
        seed_kural: Option<u16>,
    },
}

const fn default_auto_age_route_on_start() -> bool {
    true
}

/// Complete coherent UI state snapshot pushed via `watch::Sender<Arc<UiSnapshot>>`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct UiSnapshot {
    pub prefs: UserPrefs,
    pub screen: AppScreen,
    pub modal: Option<ModalKind>,
    pub current_kural: FullKural,
    pub requested_scholar: Scholar,
    pub effective_scholar: Scholar,
    pub active_filter: Option<ActiveFilter>,
    pub pool_len: usize,
    pub pool_idx: usize,
    pub kids_progress: KidsProgress,
    pub youth_progress: YouthProgress,
    pub bookmarks: Vec<BookmarkEntry>,
    pub audio_state: AudioState,
    #[serde(default)]
    pub playback_status: AudioPlaybackStatus,
    pub has_replay_clip: bool,
    pub recent_voice_turns: Vec<VoiceTurnSummary>,
    pub can_replay_tts: bool,
    pub can_replay_voice_turn: bool,
    pub auto_speak_text: bool,
    #[serde(default = "default_auto_age_route_on_start")]
    pub auto_age_route_on_start: bool,
    pub voice_to_voice_loop: bool,
    pub active_replay_label: Option<String>,
    pub system_health: SystemHealth,
}

/// One-shot transient events delivered via bounded `mpsc::Receiver<UiTransientEvent>`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[non_exhaustive]
pub enum UiTransientEvent {
    SearchResultsReady(SearchOutcome),
    AdhikaramPreviewReady {
        adhikaram_id: u8,
        kurals: Vec<KuralSummary>,
    },
    AgeResult(AgeOutcome),
    VoiceCommandRecognized {
        transcript: String,
        toast_ta: String,
        toast_en: String,
        compact_tab: Option<CompactTabHint>,
    },
    AiAnswerReady(GroundedAnswer),
    KidsGameEvaluated {
        kural_number: u16,
        correct: bool,
        correct_idx: u8,
        stars_earned: u32,
    },
    YouthQuizEvaluated {
        kural_number: u16,
        correct: bool,
        correct_idx: u8,
        daily_streak: u32,
        quiz_xp: u32,
    },
    CommentaryFallbackNotice {
        requested: Scholar,
        effective: Scholar,
        notice_ta: String,
        notice_en: String,
    },
    InfoToast {
        message_ta: String,
        message_en: String,
    },
    ErrorToast {
        message_ta: String,
        message_en: String,
    },
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn speech_speed_snapping_stepping_and_cadence_behave_deterministically() {
        assert_eq!(snap_speech_speed(0.70), 0.75);
        assert_eq!(snap_speech_speed(1.10), 1.0);
        assert_eq!(snap_speech_speed(1.40), 1.25);
        assert_eq!(snap_speech_speed(f32::NAN), 1.0);

        assert_eq!(step_speech_speed(1.0, -1), 0.75);
        assert_eq!(step_speech_speed(0.75, -1), 0.75);
        assert_eq!(step_speech_speed(1.0, 1), 1.25);
        assert_eq!(step_speech_speed(1.25, 1), 1.25);

        assert!((effective_tts_speed(1.0, AppScreen::Kids) - 0.95).abs() < 1e-5);
        assert!((effective_tts_speed(1.0, AppScreen::Youth) - 1.02).abs() < 1e-5);
        assert!((effective_tts_speed(1.0, AppScreen::Adult) - 1.00).abs() < 1e-5);
    }

    #[test]
    fn enum_display_and_from_str_round_trip() {
        for lang in [Language::Tamil, Language::English] {
            assert_eq!(lang.to_string().parse::<Language>().unwrap(), lang);
        }
        for theme in [Theme::Dark, Theme::Light] {
            assert_eq!(theme.to_string().parse::<Theme>().unwrap(), theme);
        }
        for scholar in [
            Scholar::MuVa,
            Scholar::Solomon,
            Scholar::Kalaignar,
            Scholar::Parimel,
            Scholar::English,
        ] {
            assert_eq!(scholar.to_string().parse::<Scholar>().unwrap(), scholar);
        }
        for screen in [
            AppScreen::Start,
            AppScreen::Kids,
            AppScreen::Youth,
            AppScreen::Adult,
        ] {
            assert_eq!(screen.to_string().parse::<AppScreen>().unwrap(), screen);
        }
    }

    #[test]
    fn user_prefs_backward_compat_and_replay_contracts_round_trip() {
        let legacy_json = r#"{
            "lang": "tamil",
            "theme": "dark",
            "speech_speed": 1.0,
            "volume": 0.85,
            "muted": false,
            "default_age": "youth",
            "last_kural_kids": 1,
            "last_kural_youth": 481,
            "last_kural_adult": 423
        }"#;
        let prefs: UserPrefs = serde_json::from_str(legacy_json).expect("legacy prefs");
        assert!(prefs.auto_speak_text);

        for mode in [
            VoiceReplayMode::AiReplyOnly,
            VoiceReplayMode::UserVoiceOnly,
            VoiceReplayMode::FullExchange,
        ] {
            let enc = serde_json::to_string(&mode).unwrap();
            let dec: VoiceReplayMode = serde_json::from_str(&enc).unwrap();
            assert_eq!(dec, mode);
        }

        let summary = VoiceTurnSummary {
            id: 7,
            user_text: "What is virtue?".into(),
            ai_text: "Virtue is spotless mind.".into(),
            lang: Lang::English,
            has_user_audio: true,
            has_ai_audio: true,
            cited_kurals: vec![34],
        };
        let enc = serde_json::to_string(&summary).unwrap();
        let dec: VoiceTurnSummary = serde_json::from_str(&enc).unwrap();
        assert_eq!(dec, summary);
    }
}

