//! Single-owner echo-safe ALSA/CPAL audio capture, DSP conditioning, VAD, Earcons, and Rodio playback.

mod actor;
mod cache;
mod devices;
mod dsp;
mod earcon;
mod error;
mod types;
mod vad;

pub use actor::{
    AudioHandle, CaptureStartAction, EchoGuardPhase, EchoGuardState, GuardTransition,
    StopPlaybackAction, reopen_backoff_ms,
};
pub use cache::{CachedClip, TtsPcmCache};
pub use devices::{find_input_device, find_output_device, list_audio_devices};
pub use dsp::{
    AudioConditioner, BiquadFilter, DEFAULT_CLIPPING_THRESHOLD, Resampler16k,
    TARGET_SAMPLE_RATE_HZ, compute_clipping_ratio, compute_rms, compute_snr_db, pcm_i16_to_f32,
};
pub use earcon::{EARCON_SAMPLE_RATE_HZ, EarconBank};
pub use error::AudioError;
pub use types::{
    AudioDeviceInfo, AudioHealthStatus, CapturedAudio, EarconKind, PcmChunk, VadBackend,
};
pub use vad::{EnergyZcrVad, FRAME_SAMPLES_16K, FrameAction, PRE_ROLL_SAMPLES_16K, UtteranceFramer};

