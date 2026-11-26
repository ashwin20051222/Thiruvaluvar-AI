//! Shared domain models, configuration, backend contracts, NFC/grapheme helpers, and validators.

mod canonical;
mod config;
mod contract;
mod copy;
mod enrichment;
mod error;
mod models;
mod text;

pub use canonical::{CanonicalDataset, load_and_audit_canonical};
pub use config::{AgeAiConfig, AiConfig, AppConfig, AudioConfig, PathsConfig, UiConfig};
pub use contract::{
    ActiveFilter, AgeDetection, AgeOutcome, AppScreen, AudioPlaybackStatus, AudioState,
    BackendCommand, BookmarkEntry, CaptureMode, CompactTabHint, CorpusCatalog, GroundedAnswer,
    KidsProgress, ModalKind, RetryReason, SPEECH_SPEED_PRESETS, Scholar, SearchTab, SpeakTarget,
    SystemHealth, Theme, UiSnapshot, UiTransientEvent, UserPrefs, VoiceReplayMode,
    VoiceTurnSummary, YouthProgress, age_cadence_factor, effective_tts_speed, snap_speech_speed,
    step_speech_speed,
};
pub use copy::{
    AGE_MODE_OPENED_ADULT, AGE_MODE_OPENED_KIDS, AGE_MODE_OPENED_YOUTH, AUTO_SPEAK_OFF_NOTICE,
    AUTO_SPEAK_ON_NOTICE, BilingualCopy, NOTHING_TO_REPLAY_NOTICE, OOS_CHAT_REFUSAL,
    OOS_VOICE_TTS_REFUSAL, PARIMEL_FALLBACK_NOTICE, REPLAY_AUDIO_NOTICE,
    REPLAY_FULL_EXCHANGE_NOTICE, REPLAY_USER_VOICE_NOTICE, VOICE_ADHIKARAM_RANGE_ERROR,
    VOICE_KURAL_RANGE_ERROR, VOICE_LOOP_OFF_NOTICE, VOICE_LOOP_ON_NOTICE,
    VOICE_LOOP_SILENCE_STOP_NOTICE, VOICE_UNCLEAR_RETRY_NOTICE, format_voice_toast, retry_copy,
};
pub use enrichment::{
    EnrichedAdultPayload, EnrichedChapterFile, EnrichedChildPayload, EnrichedKuralRecord,
    EnrichedYouthPayload, normalize_enriched_chapter, validate_enriched_chapter,
};
pub use error::CoreError;
pub use models::{
    Adhikaram, AgeGroup, CANONICAL_TAGS, DbStats, Division, FullKural, Iyal, KnowledgeEntry,
    KuralAdult, KuralChild, KuralCore, KuralSummary, KuralYouth, Lang, Language, ResearchTheme,
    SearchHit, SearchMatchKind, SearchOutcome, Tag, canonical_tag_by_slug, is_canonical_tag_slug,
};
pub use text::{
    build_tts_kural_en, build_tts_kural_ta, check_transliteration_phonetic_sanity,
    contains_exact_kural_word, is_nfc, normalize_nfc, normalize_nfc_trimmed, opening_grapheme_ta,
    tokenize_kural_words,
};
