use thiserror::Error;

/// Errors produced by Voice Age AI, Semantic Explainer, Sherpa VAD, and bilingual STT/TTS.
#[derive(Error, Debug)]
pub enum AiError {
    #[error("database error: {0}")]
    Db(#[from] kural_db::DbError),

    #[error("audio error: {0}")]
    Audio(#[from] kural_audio::AudioError),

    #[error("invalid AI input: {0}")]
    InvalidInput(String),

    #[error("model loading error: {0}")]
    ModelLoad(String),

    #[error("inference failed: {0}")]
    Inference(String),

    #[error("offline fallback TTS unavailable: {0}")]
    FallbackTtsUnavailable(String),

    #[error("operation cancelled")]
    Cancelled,
}
