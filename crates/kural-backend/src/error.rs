use kural_ai::AiError;
use kural_audio::AudioError;
use kural_core::CoreError;
use kural_db::DbError;
use thiserror::Error;

/// Errors returned during backend bootstrap, dispatch, or synchronous shutdown.
#[derive(Error, Debug)]
pub enum BackendError {
    #[error("configuration/core error: {0}")]
    Core(#[from] CoreError),

    #[error("database error: {0}")]
    Db(#[from] DbError),

    #[error("audio subsystem error: {0}")]
    Audio(#[from] AudioError),

    #[error("AI subsystem error: {0}")]
    Ai(#[from] AiError),

    #[error("backend initialization failed: {0}")]
    Init(String),

    #[error("backend controller channel closed")]
    Closed,
}
