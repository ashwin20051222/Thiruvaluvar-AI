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
pub use cache::{
    CachedClip, MAX_VOICE_TURN_BYTES, MAX_VOICE_TURNS, TtsPcmCache, VoiceTurnAudio,
    VoiceTurnAudioStore, compute_tts_cache_key, f32_pcm_to_i16_arc, hash_tts_content,
};
pub use devices::{
    DEFAULT_OUTPUT_CHANNELS, DEFAULT_OUTPUT_SAMPLE_RATE_HZ, find_input_device, find_output_device,
    list_audio_devices, query_output_device_format,
};
pub use dsp::{
    AudioConditioner, BiquadFilter, DEFAULT_CLIPPING_THRESHOLD, OUTPUT_FADE_MS,
    OUTPUT_SOFT_LIMIT_CEILING, OutputPcmConditioner, Resampler16k, TARGET_SAMPLE_RATE_HZ,
    compute_clipping_ratio, compute_rms, compute_snr_db, condition_output_pcm, pcm_i16_to_f32,
};
pub use earcon::{EARCON_SAMPLE_RATE_HZ, EarconBank};
pub use error::AudioError;
pub use types::{
    AudioDeviceInfo, AudioHealthStatus, CapturedAudio, EarconKind, PcmChunk, VadBackend,
};
pub use vad::{
    EnergyZcrVad, FRAME_SAMPLES_16K, FrameAction, PRE_ROLL_SAMPLES_16K, UtteranceFramer,
};
