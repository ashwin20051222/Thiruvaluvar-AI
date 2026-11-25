//! Offline vocal-tract Voice Age AI, 3-level Semantic Kural Explainer, Sherpa VAD, and bilingual STT/TTS.
//!
//! # Crate layout
//!
//! | Module | Public export | Latency |
//! |--------|---------------|---------|
//! | [`age`] | [`AgeClassifier`], [`VocalTractFeatures`], [`extract_vocal_tract_features`] | `< 30 ms` |
//! | [`explainer`] | [`SemanticExplainer`] | `< 15 ms` |
//! | [`vad`] | [`SherpaSileroVad`] | streaming |
//! | [`stt`] | [`EnSttFiles`], [`TaSttFiles`], file discovery helpers | offline |
//! | [`tts`] | [`SpeechEngine`], [`COUPLET_PAUSE_MS`], WAV/prose helpers | offline |
//! | [`error`] | [`AiError`] | — |
//!
//! All modules are **100 % offline** (zero runtime network calls, zero Python).
#![forbid(unsafe_code)]

mod age;
mod error;
mod explainer;
mod stt;
mod tts;
mod vad;

pub use age::{AgeClassifier, VocalTractFeatures, extract_vocal_tract_features};
pub use error::AiError;
pub use explainer::SemanticExplainer;
pub use stt::{EnSttFiles, TaSttFiles, discover_en_stt_files, discover_ta_stt_files};
pub use tts::{
    COUPLET_PAUSE_MS, SpeechEngine, normalize_spoken_text, parse_wav_pcm_mono_i16,
    split_tts_sentences,
};
pub use vad::SherpaSileroVad;
