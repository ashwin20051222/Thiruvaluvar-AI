use crate::AudioError;

/// Streaming TTS PCM audio chunk (`i16` mono at `sample_rate_hz`).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PcmChunk {
    pub samples: Vec<i16>,
    pub sample_rate_hz: u32,
    pub is_final: bool,
}

/// Conditioned `16 kHz` mono microphone capture returned from `AudioHandle::start_capture`.
#[derive(Debug, Clone, PartialEq)]
pub struct CapturedAudio {
    pub samples_16k: Vec<f32>,
    pub speech_only_16k: Vec<f32>,
    pub pre_roll_noise_16k: Vec<f32>,
    pub voiced_ms: u32,
    pub snr_db: f32,
    pub clipping_ratio: f32,
    pub rms_level: f32,
}

/// Synthesized system audio chimes.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum EarconKind {
    ListenStart,
    CmdRecognized,
    RetryTone,
}

/// Voice Activity Detection backend pluggable across pure-Rust fallback and ONNX Silero VAD.
pub trait VadBackend: Send {
    /// Reset internal stateful RNN/energy accumulators between utterances.
    fn reset(&mut self);

    /// Score a single `512`-sample (`32 ms`) `16 kHz` mono frame, returning probability in `[0.0, 1.0]`.
    fn speech_probability(&mut self, frame_512_16k: &[f32]) -> Result<f32, AudioError>;
}

/// Enumerated ALSA / system audio endpoint metadata.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AudioDeviceInfo {
    pub name: String,
    pub driver: Option<String>,
    pub is_input: bool,
    pub is_output: bool,
    pub is_default: bool,
    pub default_sample_rate_hz: Option<u32>,
    pub default_channels: Option<u16>,
}

/// Hardware health snapshot returned by `AudioHandle` and `audio_test`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct AudioHealthStatus {
    pub input_ok: bool,
    pub output_ok: bool,
    pub sample_rate_hz: u32,
}
