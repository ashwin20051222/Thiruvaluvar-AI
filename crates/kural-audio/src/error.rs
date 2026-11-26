use thiserror::Error;

/// Errors produced by ALSA capture/playback, DSP conditioning, VAD, or Echo Guard actor commands.
#[derive(Error, Debug, Clone, PartialEq, Eq)]
pub enum AudioError {
    #[error("audio hardware unavailable: {0}")]
    DeviceUnavailable(String),

    #[error("capture already active")]
    CaptureInProgress,

    #[error("listening timed out with no speech")]
    Timeout,

    #[error("capture cancelled")]
    Cancelled,

    #[error("audio stream error: {0}")]
    StreamError(String),
}
