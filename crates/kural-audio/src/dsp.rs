//! Pure-Rust audio conditioning, resampling (`16 kHz` mono `f32`), and signal quality helpers.

use std::collections::VecDeque;
use std::f64::consts::PI;

use audioadapter_buffers::direct::InterleavedSlice;
use rubato::{
    Async, FixedAsync, Indexing, Resampler, SincInterpolationParameters, SincInterpolationType,
    WindowFunction,
};

use crate::AudioError;

/// Target sample rate (`16 kHz`) used throughout the voice pipeline.
pub const TARGET_SAMPLE_RATE_HZ: u32 = 16_000;

/// Smooth soft-limiter ceiling applied to conditioned output PCM so DAC peaks never hard-clip.
pub const OUTPUT_SOFT_LIMIT_CEILING: f32 = 0.84;

/// Raised-cosine boundary fade duration (`10 ms`) for output PCM click/pop elimination.
pub const OUTPUT_FADE_MS: u32 = 10;

/// Linear threshold below which the output soft-limiter is unity gain (`<= 0.70` untouched).
const OUTPUT_SOFT_LIMIT_KNEE: f32 = 0.70;

/// Chunk size used for asynchronous output sinc resampling.
const OUTPUT_RESAMPLER_CHUNK_FRAMES: usize = 256;

/// Default peak amplitude threshold for clipping detection.
pub const DEFAULT_CLIPPING_THRESHOLD: f32 = 0.99;

/// Minimum supported input sample rate for [`Resampler16k`].
const MIN_INPUT_SAMPLE_RATE_HZ: u32 = 8_000;

/// Fixed input chunk size (10 ms at 48 kHz) used for asynchronous sinc resampling.
const RESAMPLER_CHUNK_FRAMES: usize = 480;

/// Pole coefficient for the continuous 1-pole DC-offset blocker (`y[n] = x[n] - x[n-1] + 0.995 * y[n-1]`).
const DC_BLOCKER_POLE: f64 = 0.995;

/// High-pass cutoff frequency (`80 Hz`) for 4th-order Butterworth conditioning.
const HPF_CUTOFF_HZ: f32 = 80.0;

/// Quality factors for a cascaded 4th-order Butterworth high-pass filter (`24 dB/oct`).
const BUTTERWORTH_4TH_Q1: f32 = 0.541_196_1;
const BUTTERWORTH_4TH_Q2: f32 = 1.306_563;

/// Mains hum notch frequencies (`50 Hz` and `60 Hz`) and notch `Q`.
const NOTCH_50_HZ: f32 = 50.0;
const NOTCH_60_HZ: f32 = 60.0;
const NOTCH_Q: f32 = 8.0;

/// Lower bound on noise RMS when computing SNR so silence does not produce `inf`/`NaN`.
const MIN_NOISE_RMS: f32 = 1e-7;
const MIN_SNR_DB: f32 = -60.0;
const MAX_SNR_DB: f32 = 96.0;

/// Direct-Form II Transposed biquad IIR filter.
///
/// Internal state and coefficients use `f64` to preserve numerical accuracy for low-frequency
/// high-pass and high-Q notch filters across `16 kHz`–`48 kHz` sample rates.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct BiquadFilter {
    b0: f64,
    b1: f64,
    b2: f64,
    a1: f64,
    a2: f64,
    z1: f64,
    z2: f64,
}

impl BiquadFilter {
    /// Identity passthrough biquad (`y[n] = x[n]`).
    fn identity() -> Self {
        Self {
            b0: 1.0,
            b1: 0.0,
            b2: 0.0,
            a1: 0.0,
            a2: 0.0,
            z1: 0.0,
            z2: 0.0,
        }
    }

    /// Construct a 2nd-order Butterworth high-pass biquad via the standard RBJ bilinear transform.
    #[must_use]
    pub fn highpass_butterworth_2nd(sample_rate_hz: f32, cutoff_hz: f32, q: f32) -> Self {
        if !sample_rate_hz.is_finite()
            || sample_rate_hz <= 0.0
            || !cutoff_hz.is_finite()
            || cutoff_hz <= 0.0
            || !q.is_finite()
            || q <= 0.0
        {
            return Self::identity();
        }

        let fs = f64::from(sample_rate_hz);
        let f0 = f64::from(cutoff_hz).min(fs * 0.499);
        let q = f64::from(q);

        let omega = 2.0 * PI * f0 / fs;
        let (sin_w, cos_w) = omega.sin_cos();
        let alpha = sin_w / (2.0 * q);
        let a0 = 1.0 + alpha;
        let one_plus_cos = 1.0 + cos_w;
        let half_one_plus_cos = 0.5 * one_plus_cos;

        Self {
            b0: half_one_plus_cos / a0,
            b1: -one_plus_cos / a0,
            b2: half_one_plus_cos / a0,
            a1: (-2.0 * cos_w) / a0,
            a2: (1.0 - alpha) / a0,
            z1: 0.0,
            z2: 0.0,
        }
    }

    /// Construct a biquad notch filter (`omega = 2*pi*f0/fs`, `alpha = sin(omega)/(2*Q)`, `a0 = 1 + alpha`).
    #[must_use]
    pub fn notch(sample_rate_hz: f32, center_hz: f32, q: f32) -> Self {
        if !sample_rate_hz.is_finite()
            || sample_rate_hz <= 0.0
            || !center_hz.is_finite()
            || center_hz <= 0.0
            || !q.is_finite()
            || q <= 0.0
        {
            return Self::identity();
        }

        let fs = f64::from(sample_rate_hz);
        let f0 = f64::from(center_hz);
        if f0 >= fs * 0.5 {
            return Self::identity();
        }
        let q = f64::from(q);

        let omega = 2.0 * PI * f0 / fs;
        let (sin_w, cos_w) = omega.sin_cos();
        let alpha = sin_w / (2.0 * q);
        let a0 = 1.0 + alpha;
        let b0 = 1.0 / a0;
        let b1 = (-2.0 * cos_w) / a0;

        Self {
            b0,
            b1,
            b2: b0,
            a1: b1,
            a2: (1.0 - alpha) / a0,
            z1: 0.0,
            z2: 0.0,
        }
    }

    /// Process a single sample using Direct-Form II Transposed topology.
    ///
    /// Any non-finite input (`NaN` or `±inf`) or non-finite intermediate state resets the filter
    /// delay registers cleanly to `0.0` and outputs `0.0`.
    pub fn process_sample(&mut self, x: f32) -> f32 {
        if !x.is_finite() {
            self.reset();
            return 0.0;
        }

        let x = f64::from(x);
        let y = self.b0 * x + self.z1;
        let z1_next = self.b1 * x - self.a1 * y + self.z2;
        let z2_next = self.b2 * x - self.a2 * y;

        if !y.is_finite() || !z1_next.is_finite() || !z2_next.is_finite() {
            self.reset();
            return 0.0;
        }

        self.z1 = z1_next;
        self.z2 = z2_next;
        y as f32
    }

    /// Clear internal filter delay registers to `0.0`.
    pub fn reset(&mut self) {
        self.z1 = 0.0;
        self.z2 = 0.0;
    }
}

#[derive(Debug, Clone, Copy, Default)]
struct DcBlocker {
    x_prev: f64,
    y_prev: f64,
}

impl DcBlocker {
    fn process_sample(&mut self, x: f32) -> f32 {
        if !x.is_finite() {
            self.reset();
            return 0.0;
        }
        let x_f64 = f64::from(x);
        let y = x_f64 - self.x_prev + DC_BLOCKER_POLE * self.y_prev;
        if !y.is_finite() {
            self.reset();
            return 0.0;
        }
        self.x_prev = x_f64;
        self.y_prev = y;
        y as f32
    }

    fn reset(&mut self) {
        self.x_prev = 0.0;
        self.y_prev = 0.0;
    }
}

/// Continuous frame-by-frame audio conditioner for microphone capture.
///
/// Applies in cascade:
/// 1. 1-pole DC-offset blocker (`y[n] = x[n] - x[n-1] + 0.995 * y[n-1]`),
/// 2. 4th-order `80 Hz` Butterworth high-pass filter (`24 dB/oct` slope),
/// 3. `50 Hz` (`Q = 8.0`) and `60 Hz` (`Q = 8.0`) mains hum biquad notch filters.
#[derive(Debug, Clone)]
pub struct AudioConditioner {
    dc_blocker: DcBlocker,
    hpf_stage1: BiquadFilter,
    hpf_stage2: BiquadFilter,
    notch_50: BiquadFilter,
    notch_60: BiquadFilter,
}

impl AudioConditioner {
    /// Construct a conditioner configured for `sample_rate_hz` (typically `16_000 Hz`).
    #[must_use]
    pub fn new(sample_rate_hz: u32) -> Self {
        let fs = sample_rate_hz as f32;
        Self {
            dc_blocker: DcBlocker::default(),
            hpf_stage1: BiquadFilter::highpass_butterworth_2nd(
                fs,
                HPF_CUTOFF_HZ,
                BUTTERWORTH_4TH_Q1,
            ),
            hpf_stage2: BiquadFilter::highpass_butterworth_2nd(
                fs,
                HPF_CUTOFF_HZ,
                BUTTERWORTH_4TH_Q2,
            ),
            notch_50: BiquadFilter::notch(fs, NOTCH_50_HZ, NOTCH_Q),
            notch_60: BiquadFilter::notch(fs, NOTCH_60_HZ, NOTCH_Q),
        }
    }

    /// Process a single sample through the conditioning chain.
    pub fn process_sample(&mut self, x: f32) -> f32 {
        if !x.is_finite() {
            self.reset();
            return 0.0;
        }

        let dc_y = self.dc_blocker.process_sample(x);
        let s1 = self.hpf_stage1.process_sample(dc_y);
        let s2 = self.hpf_stage2.process_sample(s1);
        let s3 = self.notch_50.process_sample(s2);
        self.notch_60.process_sample(s3)
    }

    /// Condition a frame of samples in place while preserving continuous filter state across calls.
    pub fn process_in_place(&mut self, samples: &mut [f32]) {
        for sample in samples.iter_mut() {
            *sample = self.process_sample(*sample);
        }
    }

    /// Reset DC tracker and all biquad delay registers to `0.0`.
    pub fn reset(&mut self) {
        self.dc_blocker.reset();
        self.hpf_stage1.reset();
        self.hpf_stage2.reset();
        self.notch_50.reset();
        self.notch_60.reset();
    }
}

/// Streaming downmixer and high-quality asynchronous sinc resampler targeting `16 kHz` mono `f32`.
///
/// When `src_rate_hz == 16_000`, `rubato` is bypassed and incoming interleaved frames are
/// downmixed directly to mono with zero resampling delay.
#[derive(Debug)]
pub struct Resampler16k {
    src_rate_hz: u32,
    channels: usize,
    resampler: Option<Async<f32>>,
    pending_mono: Vec<f32>,
    scratch_out: Vec<f32>,
}

impl Resampler16k {
    /// Construct a resampler for interleaved audio at `src_rate_hz` (`>= 8000`) and `channels` (`>= 1`).
    pub fn new(src_rate_hz: u32, channels: u16) -> Result<Self, AudioError> {
        if src_rate_hz < MIN_INPUT_SAMPLE_RATE_HZ {
            return Err(AudioError::StreamError(format!(
                "unsupported input sample rate: {src_rate_hz} Hz (expected >= {MIN_INPUT_SAMPLE_RATE_HZ} Hz)"
            )));
        }
        if channels == 0 {
            return Err(AudioError::StreamError(
                "input channel count must be >= 1".to_string(),
            ));
        }

        let channels = usize::from(channels);
        if src_rate_hz == TARGET_SAMPLE_RATE_HZ {
            return Ok(Self {
                src_rate_hz,
                channels,
                resampler: None,
                pending_mono: Vec::new(),
                scratch_out: Vec::new(),
            });
        }

        let params = SincInterpolationParameters::new(128, WindowFunction::Blackman2)
            .oversampling_factor(128)
            .interpolation(SincInterpolationType::Linear);

        let resampler = Async::<f32>::new_sinc(
            f64::from(TARGET_SAMPLE_RATE_HZ) / f64::from(src_rate_hz),
            1.1,
            &params,
            RESAMPLER_CHUNK_FRAMES,
            1,
            FixedAsync::Input,
        )
        .map_err(|err| AudioError::StreamError(format!("failed to create resampler: {err}")))?;

        if resampler.output_frames_next() == 0 {
            return Err(AudioError::StreamError(format!(
                "unsupported sample rate ratio for {src_rate_hz} Hz -> {TARGET_SAMPLE_RATE_HZ} Hz"
            )));
        }

        let scratch_len = resampler.output_frames_max();
        Ok(Self {
            src_rate_hz,
            channels,
            resampler: Some(resampler),
            pending_mono: Vec::with_capacity(RESAMPLER_CHUNK_FRAMES * 2),
            scratch_out: vec![0.0; scratch_len],
        })
    }

    /// Native input sample rate in Hz.
    #[must_use]
    pub fn src_rate_hz(&self) -> u32 {
        self.src_rate_hz
    }

    /// Configured input channel count.
    #[must_use]
    pub fn channels(&self) -> usize {
        self.channels
    }

    /// Number of downmixed mono input frames currently buffered awaiting the next resampler chunk.
    #[must_use]
    pub fn pending_frames(&self) -> usize {
        self.pending_mono.len()
    }

    /// Downmix interleaved input samples to mono, buffer until full chunks are ready, and return
    /// accumulated `16 kHz` mono `f32` samples.
    pub fn push_interleaved(&mut self, interleaved: &[f32]) -> Result<Vec<f32>, AudioError> {
        if interleaved.len() % self.channels != 0 {
            return Err(AudioError::StreamError(format!(
                "interleaved sample count ({}) is not divisible by channel count ({})",
                interleaved.len(),
                self.channels
            )));
        }
        if interleaved.is_empty() {
            return Ok(Vec::new());
        }

        let Some(resampler) = &mut self.resampler else {
            let mut mono = Vec::with_capacity(interleaved.len() / self.channels);
            downmix_interleaved_into(interleaved, self.channels, &mut mono);
            return Ok(mono);
        };

        downmix_interleaved_into(interleaved, self.channels, &mut self.pending_mono);

        let ratio = f64::from(TARGET_SAMPLE_RATE_HZ) / f64::from(self.src_rate_hz);
        let estimated_out = ((self.pending_mono.len() as f64) * ratio).ceil() as usize;
        let mut output = Vec::with_capacity(estimated_out);
        let mut consumed = 0usize;

        while self.pending_mono.len() - consumed >= resampler.input_frames_next() {
            let in_frames = resampler.input_frames_next();
            let out_frames_cap = resampler.output_frames_next();
            if out_frames_cap == 0 {
                break;
            }
            if self.scratch_out.len() < out_frames_cap {
                self.scratch_out.resize(out_frames_cap, 0.0);
            }

            let in_slice = &self.pending_mono[consumed..consumed + in_frames];
            let in_adapter = InterleavedSlice::new(in_slice, 1, in_frames).map_err(|err| {
                AudioError::StreamError(format!("resampler input buffer error: {err}"))
            })?;
            let mut out_adapter = InterleavedSlice::new_mut(
                &mut self.scratch_out[..out_frames_cap],
                1,
                out_frames_cap,
            )
            .map_err(|err| {
                AudioError::StreamError(format!("resampler output buffer error: {err}"))
            })?;

            let (frames_in, frames_out) = resampler
                .process_into_buffer(&in_adapter, &mut out_adapter, None)
                .map_err(|err| AudioError::StreamError(format!("resampling error: {err}")))?;

            consumed += frames_in;
            output.extend_from_slice(&self.scratch_out[..frames_out]);
        }

        if consumed > 0 {
            self.pending_mono.drain(..consumed);
        }

        Ok(output)
    }

    /// Clear buffered mono frames and reset internal resampler filter state.
    pub fn reset(&mut self) {
        self.pending_mono.clear();
        if let Some(resampler) = &mut self.resampler {
            resampler.reset();
        }
    }
}

fn downmix_interleaved_into(interleaved: &[f32], channels: usize, dst: &mut Vec<f32>) {
    if channels == 1 {
        dst.extend_from_slice(interleaved);
        return;
    }
    let ch_f32 = channels as f32;
    dst.reserve(interleaved.len() / channels);
    for frame in interleaved.chunks_exact(channels) {
        let sum: f32 = frame.iter().copied().sum();
        dst.push(sum / ch_f32);
    }
}

/// Compute root-mean-square level of `samples`, returning `0.0` for empty or non-finite input.
#[must_use]
pub fn compute_rms(samples: &[f32]) -> f32 {
    if samples.is_empty() {
        return 0.0;
    }
    let sum_sq: f64 = samples
        .iter()
        .map(|&s| {
            let v = f64::from(s);
            v * v
        })
        .sum();
    let rms = (sum_sq / samples.len() as f64).sqrt() as f32;
    if rms.is_finite() { rms } else { 0.0 }
}

/// Compute the ratio of samples whose absolute amplitude meets or exceeds `threshold` (default `0.99`).
#[must_use]
pub fn compute_clipping_ratio(samples: &[f32], threshold: f32) -> f32 {
    if samples.is_empty() || !threshold.is_finite() {
        return 0.0;
    }
    let clipped = samples.iter().filter(|&&s| s.abs() >= threshold).count();
    clipped as f32 / samples.len() as f32
}

/// Estimate signal-to-noise ratio in `dB` from voiced `speech` and pre-roll `noise_floor_samples`.
///
/// Returns `0.0` when `speech` is empty or silent, and clamps the result to `[-60.0, 96.0] dB`
/// when `noise_floor_samples` is near zero.
#[must_use]
pub fn compute_snr_db(speech: &[f32], noise_floor_samples: &[f32]) -> f32 {
    let speech_rms = compute_rms(speech);
    if speech_rms <= f32::EPSILON {
        return 0.0;
    }
    let noise_rms = compute_rms(noise_floor_samples).max(MIN_NOISE_RMS);
    let snr = 20.0 * (speech_rms / noise_rms).log10();
    if snr.is_finite() {
        snr.clamp(MIN_SNR_DB, MAX_SNR_DB)
    } else {
        0.0
    }
}

/// Convert signed 16-bit PCM samples to normalized `f32` samples in `[-1.0, 1.0)`.
#[must_use]
pub fn pcm_i16_to_f32(samples: &[i16]) -> Vec<f32> {
    samples.iter().map(|&s| f32::from(s) / 32_768.0).collect()
}

/// Stateful pure-Rust output PCM conditioner for continuous streaming or single-utterance playback.
///
/// Applies in order:
/// 1. `NaN`/`±Inf` sanitization to `0.0`,
/// 2. 1-pole DC-blocking high-pass filter (`DC_BLOCKER_POLE = 0.995`),
/// 3. High-quality `rubato::Async::<f32>::new_sinc` resampling from `src_rate_hz` to `dst_rate_hz`
///    (bypassed when `src_rate_hz == dst_rate_hz`),
/// 4. Smooth `tanh` knee soft-limiter capping output amplitude strictly at `0.84`,
/// 5. `10 ms` raised-cosine (`0.5 * (1 - cos(pi * t))`) fade-in and fade-out at utterance boundaries,
/// 6. Clean mono-to-interleaved channel adaptation for `dst_channels`.
#[derive(Debug)]
pub struct OutputPcmConditioner {
    src_rate_hz: u32,
    dst_rate_hz: u32,
    dst_channels: usize,
    dc_blocker: DcBlocker,
    resampler: Option<Async<f32>>,
    pending_in: Vec<f32>,
    scratch_out: Vec<f32>,
    trim_delay_remaining: usize,
    total_in_frames: usize,
    emitted_mono_frames: usize,
    tail_buf: VecDeque<f32>,
    fade_samples: usize,
}

impl OutputPcmConditioner {
    /// Construct an output conditioner targeting exact hardware `(dst_rate_hz, dst_channels)`.
    pub fn new(src_rate_hz: u32, dst_rate_hz: u32, dst_channels: u16) -> Result<Self, AudioError> {
        if src_rate_hz == 0 || dst_rate_hz == 0 {
            return Err(AudioError::StreamError(format!(
                "output conditioning sample rates must be > 0 Hz (got {src_rate_hz} -> {dst_rate_hz})"
            )));
        }
        if dst_channels == 0 {
            return Err(AudioError::StreamError(
                "output channel count must be >= 1".to_string(),
            ));
        }

        let (resampler, trim_delay_remaining, scratch_len) = if src_rate_hz == dst_rate_hz {
            (None, 0, 0)
        } else {
            let params = SincInterpolationParameters::new(32, WindowFunction::Blackman2)
                .oversampling_factor(32)
                .interpolation(SincInterpolationType::Linear);
            let resampler = Async::<f32>::new_sinc(
                f64::from(dst_rate_hz) / f64::from(src_rate_hz),
                1.1,
                &params,
                OUTPUT_RESAMPLER_CHUNK_FRAMES,
                1,
                FixedAsync::Input,
            )
            .map_err(|err| {
                AudioError::StreamError(format!("failed to create output sinc resampler: {err}"))
            })?;
            if resampler.output_frames_next() == 0 {
                return Err(AudioError::StreamError(format!(
                    "unsupported output sample rate ratio for {src_rate_hz} Hz -> {dst_rate_hz} Hz"
                )));
            }
            let delay = resampler.output_delay();
            let max_out = resampler.output_frames_max();
            (Some(resampler), delay, max_out)
        };

        let fade_samples = ((dst_rate_hz as usize) * (OUTPUT_FADE_MS as usize)) / 1000;
        Ok(Self {
            src_rate_hz,
            dst_rate_hz,
            dst_channels: usize::from(dst_channels),
            dc_blocker: DcBlocker::default(),
            resampler,
            pending_in: Vec::with_capacity(OUTPUT_RESAMPLER_CHUNK_FRAMES * 2),
            scratch_out: vec![0.0_f32; scratch_len],
            trim_delay_remaining,
            total_in_frames: 0,
            emitted_mono_frames: 0,
            tail_buf: VecDeque::with_capacity(fade_samples * 2 + 64),
            fade_samples,
        })
    }

    /// Source sample rate in Hz.
    #[must_use]
    pub fn src_rate_hz(&self) -> u32 {
        self.src_rate_hz
    }

    /// Push a chunk of mono `f32` samples and emit ready conditioned interleaved frames.
    pub fn push_mono(&mut self, mono_samples: &[f32]) -> Result<Vec<f32>, AudioError> {
        if mono_samples.is_empty() {
            return Ok(Vec::new());
        }
        self.total_in_frames = self.total_in_frames.saturating_add(mono_samples.len());

        if self.resampler.is_none() {
            for &sample in mono_samples {
                let clean = sanitize_output_sample(sample);
                let dc_blocked = self.dc_blocker.process_sample(clean);
                self.tail_buf.push_back(soft_limit_output(dc_blocked));
            }
            return Ok(self.drain_ready_interleaved());
        }

        self.pending_in.reserve(mono_samples.len());
        for &sample in mono_samples {
            let clean = sanitize_output_sample(sample);
            self.pending_in.push(self.dc_blocker.process_sample(clean));
        }

        let mut consumed = 0usize;
        if let Some(resampler) = &mut self.resampler {
            while self.pending_in.len() - consumed >= resampler.input_frames_next() {
                let in_frames = resampler.input_frames_next();
                let out_frames_cap = resampler.output_frames_next();
                if out_frames_cap == 0 {
                    break;
                }
                if self.scratch_out.len() < out_frames_cap {
                    self.scratch_out.resize(out_frames_cap, 0.0);
                }

                let in_slice = &self.pending_in[consumed..consumed + in_frames];
                let in_adapter = InterleavedSlice::new(in_slice, 1, in_frames).map_err(|err| {
                    AudioError::StreamError(format!("output resampler input error: {err}"))
                })?;
                let mut out_adapter = InterleavedSlice::new_mut(
                    &mut self.scratch_out[..out_frames_cap],
                    1,
                    out_frames_cap,
                )
                .map_err(|err| {
                    AudioError::StreamError(format!("output resampler output error: {err}"))
                })?;

                let (frames_in, frames_out) = resampler
                    .process_into_buffer(&in_adapter, &mut out_adapter, None)
                    .map_err(|err| {
                        AudioError::StreamError(format!("output resampling error: {err}"))
                    })?;

                consumed += frames_in;
                push_trimmed_soft_limited(
                    &self.scratch_out[..frames_out],
                    &mut self.trim_delay_remaining,
                    &mut self.tail_buf,
                );
            }
        }
        if consumed > 0 {
            self.pending_in.drain(..consumed);
        }

        Ok(self.drain_ready_interleaved())
    }

    /// Flush all buffered resampler frames, apply the `10 ms` raised-cosine fade-out, and return
    /// the remaining conditioned interleaved frames.
    pub fn finish(mut self) -> Result<Vec<f32>, AudioError> {
        if self.total_in_frames == 0 {
            return Ok(Vec::new());
        }

        let expected_total_out = if self.src_rate_hz == self.dst_rate_hz {
            self.total_in_frames
        } else {
            (((self.total_in_frames as f64) * f64::from(self.dst_rate_hz)
                / f64::from(self.src_rate_hz))
            .round() as usize)
                .max(1)
        };

        if let Some(resampler) = &mut self.resampler {
            if !self.pending_in.is_empty() {
                let rem = self.pending_in.len();
                let out_frames_cap = resampler.output_frames_next();
                if out_frames_cap > 0 {
                    if self.scratch_out.len() < out_frames_cap {
                        self.scratch_out.resize(out_frames_cap, 0.0);
                    }
                    let in_adapter =
                        InterleavedSlice::new(&self.pending_in, 1, rem).map_err(|err| {
                            AudioError::StreamError(format!(
                                "output resampler final input error: {err}"
                            ))
                        })?;
                    let mut out_adapter = InterleavedSlice::new_mut(
                        &mut self.scratch_out[..out_frames_cap],
                        1,
                        out_frames_cap,
                    )
                    .map_err(|err| {
                        AudioError::StreamError(format!(
                            "output resampler final output error: {err}"
                        ))
                    })?;
                    let indexing = Indexing {
                        input_offset: 0,
                        output_offset: 0,
                        active_channels_mask: None,
                        partial_len: Some(rem),
                    };
                    let (_frames_in, frames_out) = resampler
                        .process_into_buffer(&in_adapter, &mut out_adapter, Some(&indexing))
                        .map_err(|err| {
                            AudioError::StreamError(format!("output resampling tail error: {err}"))
                        })?;
                    push_trimmed_soft_limited(
                        &self.scratch_out[..frames_out],
                        &mut self.trim_delay_remaining,
                        &mut self.tail_buf,
                    );
                }
                self.pending_in.clear();
            }

            let empty_in = InterleavedSlice::new(&[], 1, 0).map_err(|err| {
                AudioError::StreamError(format!("output resampler flush input error: {err}"))
            })?;
            while self.emitted_mono_frames + self.tail_buf.len() < expected_total_out {
                let out_frames_cap = resampler.output_frames_next();
                if out_frames_cap == 0 {
                    break;
                }
                if self.scratch_out.len() < out_frames_cap {
                    self.scratch_out.resize(out_frames_cap, 0.0);
                }
                let mut out_adapter = InterleavedSlice::new_mut(
                    &mut self.scratch_out[..out_frames_cap],
                    1,
                    out_frames_cap,
                )
                .map_err(|err| {
                    AudioError::StreamError(format!("output resampler flush output error: {err}"))
                })?;
                let indexing = Indexing {
                    input_offset: 0,
                    output_offset: 0,
                    active_channels_mask: None,
                    partial_len: Some(0),
                };
                let (_frames_in, frames_out) = resampler
                    .process_into_buffer(&empty_in, &mut out_adapter, Some(&indexing))
                    .map_err(|err| {
                        AudioError::StreamError(format!("output resampling flush error: {err}"))
                    })?;
                if frames_out == 0 {
                    break;
                }
                push_trimmed_soft_limited(
                    &self.scratch_out[..frames_out],
                    &mut self.trim_delay_remaining,
                    &mut self.tail_buf,
                );
            }
        }

        let total_now = self.emitted_mono_frames + self.tail_buf.len();
        if total_now > expected_total_out {
            let excess = total_now - expected_total_out;
            let keep = self.tail_buf.len().saturating_sub(excess);
            self.tail_buf.truncate(keep);
        }

        let remaining = self.tail_buf.len();
        if remaining == 0 {
            return Ok(Vec::new());
        }

        let mut tail: Vec<f32> = self.tail_buf.drain(..).collect();
        if self.emitted_mono_frames == 0 {
            if remaining == 1 {
                if self.fade_samples > 0 {
                    tail[0] = 0.0;
                }
            } else if self.fade_samples > 0 {
                let fade = self.fade_samples.min(remaining / 2);
                if fade > 0 {
                    for (i, sample) in tail[..fade].iter_mut().enumerate() {
                        *sample *= raised_cosine_fade_gain(i, fade);
                    }
                    let out_start = remaining - fade;
                    for (i, sample) in tail[out_start..].iter_mut().enumerate() {
                        let rem = fade - 1 - i;
                        *sample *= raised_cosine_fade_gain(rem, fade);
                    }
                }
            }
        } else if self.fade_samples > 0 {
            for (i, sample) in tail.iter_mut().enumerate() {
                let global_idx = self.emitted_mono_frames + i;
                if global_idx < self.fade_samples {
                    *sample *= raised_cosine_fade_gain(global_idx, self.fade_samples);
                }
            }
            let fade_out_len = self.fade_samples.min(remaining);
            if fade_out_len > 0 {
                let out_start = remaining - fade_out_len;
                for (i, sample) in tail[out_start..].iter_mut().enumerate() {
                    let rem = fade_out_len - 1 - i;
                    *sample *= raised_cosine_fade_gain(rem, fade_out_len);
                }
            }
        }

        Ok(interleave_mono_to_channels(&tail, self.dst_channels))
    }

    fn drain_ready_interleaved(&mut self) -> Vec<f32> {
        let hold = self.fade_samples.saturating_mul(2);
        let ready_len = self.tail_buf.len().saturating_sub(hold);
        if ready_len == 0 {
            return Vec::new();
        }
        let mut mono = Vec::with_capacity(ready_len);
        for _ in 0..ready_len {
            if let Some(mut sample) = self.tail_buf.pop_front() {
                let idx = self.emitted_mono_frames;
                self.emitted_mono_frames += 1;
                if idx < self.fade_samples {
                    sample *= raised_cosine_fade_gain(idx, self.fade_samples);
                }
                mono.push(sample);
            }
        }
        interleave_mono_to_channels(&mono, self.dst_channels)
    }
}

/// Condition mono `f32` PCM for hardware speaker playback:
/// 1. Sanitize `NaN`/`±Inf` to `0.0`,
/// 2. Apply DC-blocking high-pass filter (`DC_BLOCKER_POLE = 0.995`),
/// 3. Sinc-resample from `src_rate_hz` to `dst_rate_hz` via `rubato::Async::<f32>::new_sinc`,
/// 4. Smoothly soft-limit peaks to `<= 0.84` (`OUTPUT_SOFT_LIMIT_CEILING`),
/// 5. Apply `10 ms` raised-cosine (`0.5 * (1 - cos(pi * t))`) fade-in and fade-out,
/// 6. Adapt mono to interleaved `dst_channels` (e.g. stereo).
pub fn condition_output_pcm(
    mono_samples: &[f32],
    src_rate_hz: u32,
    dst_rate_hz: u32,
    dst_channels: u16,
) -> Result<Vec<f32>, AudioError> {
    if mono_samples.is_empty() {
        if src_rate_hz == 0 || dst_rate_hz == 0 || dst_channels == 0 {
            return Err(AudioError::StreamError(
                "output sample rates and channel count must be > 0".to_string(),
            ));
        }
        return Ok(Vec::new());
    }
    let mut conditioner = OutputPcmConditioner::new(src_rate_hz, dst_rate_hz, dst_channels)?;
    let mut out = conditioner.push_mono(mono_samples)?;
    out.extend(conditioner.finish()?);
    Ok(out)
}

fn sanitize_output_sample(sample: f32) -> f32 {
    if sample.is_finite() { sample } else { 0.0 }
}

fn soft_limit_output(sample: f32) -> f32 {
    if !sample.is_finite() {
        return 0.0;
    }
    let abs = sample.abs();
    if abs <= OUTPUT_SOFT_LIMIT_KNEE {
        return sample;
    }
    let headroom = OUTPUT_SOFT_LIMIT_CEILING - OUTPUT_SOFT_LIMIT_KNEE;
    let excess = (abs - OUTPUT_SOFT_LIMIT_KNEE) / headroom;
    let limited = OUTPUT_SOFT_LIMIT_KNEE + headroom * excess.tanh();
    (sample.signum() * limited).clamp(-OUTPUT_SOFT_LIMIT_CEILING, OUTPUT_SOFT_LIMIT_CEILING)
}

fn raised_cosine_fade_gain(pos: usize, fade_len: usize) -> f32 {
    if fade_len <= 1 {
        return 0.0;
    }
    let t = (pos as f64 / (fade_len - 1) as f64).clamp(0.0, 1.0);
    (0.5 * (1.0 - (PI * t).cos())) as f32
}

fn push_trimmed_soft_limited(
    samples: &[f32],
    trim_delay_remaining: &mut usize,
    tail_buf: &mut VecDeque<f32>,
) {
    let skip = (*trim_delay_remaining).min(samples.len());
    *trim_delay_remaining -= skip;
    for &sample in &samples[skip..] {
        tail_buf.push_back(soft_limit_output(sample));
    }
}

fn interleave_mono_to_channels(mono: &[f32], channels: usize) -> Vec<f32> {
    if channels == 1 {
        return mono.to_vec();
    }
    let mut interleaved = Vec::with_capacity(mono.len() * channels);
    for &sample in mono {
        for _ in 0..channels {
            interleaved.push(sample);
        }
    }
    interleaved
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::f64::consts::PI;

    fn dft_magnitude_at_hz(samples: &[f32], sample_rate_hz: f64, freq_hz: f64) -> f64 {
        let n = samples.len();
        if n == 0 {
            return 0.0;
        }
        let mut re = 0.0_f64;
        let mut im = 0.0_f64;
        let omega = 2.0 * PI * freq_hz / sample_rate_hz;
        for (i, &s) in samples.iter().enumerate() {
            let angle = omega * (i as f64);
            let val = f64::from(s);
            re += val * angle.cos();
            im -= val * angle.sin();
        }
        (re * re + im * im).sqrt() * 2.0 / (n as f64)
    }

    fn zero_crossing_frequency_hz(samples: &[f32], sample_rate_hz: f64) -> f64 {
        let mut rising_crossings = Vec::new();
        for i in 1..samples.len() {
            let prev = samples[i - 1];
            let curr = samples[i];
            if prev <= 0.0 && curr > 0.0 {
                let frac = f64::from(-prev) / f64::from(curr - prev);
                rising_crossings.push((i - 1) as f64 + frac);
            }
        }
        assert!(
            rising_crossings.len() >= 2,
            "need at least two rising zero crossings"
        );
        let periods = (rising_crossings.len() - 1) as f64;
        let span_samples = rising_crossings[rising_crossings.len() - 1] - rising_crossings[0];
        sample_rate_hz * periods / span_samples
    }

    #[test]
    fn resampler_48k_to_16k_preserves_1khz_tone() {
        let src_rate = 48_000_u32;
        let total_frames = 24_000_usize; // 0.5 s at 48 kHz
        let tone_hz = 1_000.0_f64;

        let mut stereo = Vec::with_capacity(total_frames * 2);
        for i in 0..total_frames {
            let t = i as f64 / f64::from(src_rate);
            let sample = (2.0 * PI * tone_hz * t).sin() as f32;
            stereo.push(sample);
            stereo.push(sample);
        }

        let mut resampler = Resampler16k::new(src_rate, 2).expect("valid 48k stereo resampler");
        // Push in realistic callback-sized blocks (e.g. 512 stereo frames at a time)
        let mut output = Vec::new();
        for chunk in stereo.chunks(512 * 2) {
            output.extend(resampler.push_interleaved(chunk).expect("resample chunk"));
        }

        assert!(
            (7_800..=8_100).contains(&output.len()),
            "expected ~8000 samples from 0.5 s at 48 kHz, got {}",
            output.len()
        );

        let steady = &output[256..];
        let zc_freq = zero_crossing_frequency_hz(steady, 16_000.0);
        assert!(
            (zc_freq - tone_hz).abs() <= 15.0,
            "zero-crossing frequency {zc_freq:.2} Hz outside 1000 ± 15 Hz"
        );

        // Coarse DFT sweep to confirm dominant peak is within ±15 Hz of 1000 Hz
        let mut best_freq = 0.0_f64;
        let mut best_mag = -1.0_f64;
        for f in (50..=7_900).step_by(10) {
            let freq = f as f64;
            let mag = dft_magnitude_at_hz(steady, 16_000.0, freq);
            if mag > best_mag {
                best_mag = mag;
                best_freq = freq;
            }
        }
        assert!(
            (best_freq - tone_hz).abs() <= 15.0,
            "DFT peak frequency {best_freq:.2} Hz outside 1000 ± 15 Hz"
        );
    }

    fn measure_tone_attenuation_db(
        fs: u32,
        freq_hz: f64,
        settle_len: usize,
        measure_len: usize,
    ) -> f32 {
        let total_len = settle_len + measure_len;
        let orig: Vec<f32> = (0..total_len)
            .map(|i| (2.0 * PI * freq_hz * (i as f64) / f64::from(fs)).sin() as f32 * 0.5)
            .collect();
        let mut processed = orig.clone();
        let mut cond = AudioConditioner::new(fs);
        cond.process_in_place(&mut processed);
        let in_rms = compute_rms(&orig[settle_len..]);
        let out_rms = compute_rms(&processed[settle_len..]).max(1e-9);
        20.0 * (in_rms / out_rms).log10()
    }

    #[test]
    fn conditioner_attenuates_dc_and_50_60hz_hum_by_18db_while_preserving_speech() {
        let fs = 16_000_u32;
        let settle_len = 6_400_usize; // 0.4 s settling
        let measure_len = 6_400_usize; // 0.4 s measurement tail
        let total_len = settle_len + measure_len;

        // 1. DC offset (0.5)
        let mut dc = vec![0.5_f32; total_len];
        let mut cond = AudioConditioner::new(fs);
        cond.process_in_place(&mut dc);
        let dc_mean =
            dc[settle_len..].iter().map(|&s| f64::from(s)).sum::<f64>() / (measure_len as f64);
        assert!(
            dc_mean.abs() < 1e-3,
            "expected DC mean < 1e-3, got {dc_mean}"
        );

        // 2. Individual 50 Hz, 60 Hz hum attenuation and 1000 Hz passband preservation
        let atten_50_db = measure_tone_attenuation_db(fs, 50.0, settle_len, measure_len);
        assert!(
            atten_50_db >= 18.0,
            "expected 50 Hz attenuation >= 18.0 dB, got {atten_50_db:.2} dB"
        );

        let atten_60_db = measure_tone_attenuation_db(fs, 60.0, settle_len, measure_len);
        assert!(
            atten_60_db >= 18.0,
            "expected 60 Hz attenuation >= 18.0 dB, got {atten_60_db:.2} dB"
        );

        let loss_1k_db = measure_tone_attenuation_db(fs, 1_000.0, settle_len, measure_len);
        assert!(
            loss_1k_db.abs() <= 1.0,
            "expected 1000 Hz passband loss <= 1.0 dB, got {loss_1k_db:.4} dB"
        );

        // 3. Combined superposition test: DC + 50 Hz + 60 Hz + 1000 Hz simultaneously
        let combined_orig: Vec<f32> = (0..total_len)
            .map(|i| {
                let t = i as f64 / f64::from(fs);
                (0.5 + 0.25 * (2.0 * PI * 50.0 * t).sin()
                    + 0.25 * (2.0 * PI * 60.0 * t).sin()
                    + 0.25 * (2.0 * PI * 1_000.0 * t).sin()) as f32
            })
            .collect();
        let mut combined = combined_orig.clone();
        let mut cond = AudioConditioner::new(fs);
        cond.process_in_place(&mut combined);
        let tail_in = &combined_orig[settle_len..];
        let tail_out = &combined[settle_len..];

        let combined_dc_mean =
            tail_out.iter().map(|&s| f64::from(s)).sum::<f64>() / (measure_len as f64);
        assert!(combined_dc_mean.abs() < 1e-3);

        let dft_50_in = dft_magnitude_at_hz(tail_in, f64::from(fs), 50.0);
        let dft_50_out = dft_magnitude_at_hz(tail_out, f64::from(fs), 50.0).max(1e-12);
        assert!(20.0 * (dft_50_in / dft_50_out).log10() >= 18.0);

        let dft_60_in = dft_magnitude_at_hz(tail_in, f64::from(fs), 60.0);
        let dft_60_out = dft_magnitude_at_hz(tail_out, f64::from(fs), 60.0).max(1e-12);
        assert!(20.0 * (dft_60_in / dft_60_out).log10() >= 18.0);

        let dft_1k_in = dft_magnitude_at_hz(tail_in, f64::from(fs), 1_000.0);
        let dft_1k_out = dft_magnitude_at_hz(tail_out, f64::from(fs), 1_000.0);
        assert!((20.0 * (dft_1k_in / dft_1k_out).log10()).abs() <= 1.0);
    }

    #[test]
    fn conditioner_chunk_continuity_and_nan_reset() {
        let fs = 16_000_u32;
        let input: Vec<f32> = (0..1_024)
            .map(|i| (2.0 * PI * 440.0 * (i as f64) / f64::from(fs)).sin() as f32 * 0.4)
            .collect();

        let mut single_pass = input.clone();
        let mut cond_a = AudioConditioner::new(fs);
        cond_a.process_in_place(&mut single_pass);

        let mut chunked = input.clone();
        let mut cond_b = AudioConditioner::new(fs);
        for chunk in chunked.chunks_mut(137) {
            cond_b.process_in_place(chunk);
        }
        assert_eq!(single_pass, chunked);

        assert_eq!(cond_b.process_sample(f32::NAN), 0.0);
        let mut fresh = AudioConditioner::new(fs);
        assert_eq!(cond_b.process_sample(0.25), fresh.process_sample(0.25));
    }

    #[test]
    fn rms_clipping_and_snr_edge_cases() {
        // Empty slices
        assert_eq!(compute_rms(&[]), 0.0);
        assert_eq!(compute_clipping_ratio(&[], DEFAULT_CLIPPING_THRESHOLD), 0.0);
        assert_eq!(compute_snr_db(&[], &[]), 0.0);

        // Zero slices
        let zeros = [0.0_f32; 64];
        assert_eq!(compute_rms(&zeros), 0.0);
        assert_eq!(
            compute_clipping_ratio(&zeros, DEFAULT_CLIPPING_THRESHOLD),
            0.0
        );
        assert_eq!(compute_snr_db(&zeros, &zeros), 0.0);

        // Clipped slice [1.0, -1.0, 0.1]
        let clipped = [1.0_f32, -1.0, 0.1];
        let ratio = compute_clipping_ratio(&clipped, DEFAULT_CLIPPING_THRESHOLD);
        assert!((ratio - (2.0 / 3.0)).abs() < 1e-6);

        let expected_rms = ((1.0_f64 + 1.0 + 0.01) / 3.0).sqrt() as f32;
        assert!((compute_rms(&clipped) - expected_rms).abs() < 1e-6);

        // Zero noise floor clamps SNR cleanly without Inf/NaN
        let snr_zero_noise = compute_snr_db(&[0.5, -0.5], &zeros);
        assert!(snr_zero_noise.is_finite());
        assert_eq!(snr_zero_noise, MAX_SNR_DB);

        // Known +20 dB SNR
        let snr_20db = compute_snr_db(&[0.5, -0.5], &[0.05, -0.05]);
        assert!((snr_20db - 20.0).abs() < 1e-4);

        // PCM i16 -> f32 normalization
        assert!(pcm_i16_to_f32(&[]).is_empty());
        let pcm = pcm_i16_to_f32(&[i16::MIN, 0, i16::MAX]);
        assert_eq!(pcm[0], -1.0);
        assert_eq!(pcm[1], 0.0);
        assert!((pcm[2] - (32_767.0 / 32_768.0)).abs() < 1e-7);
    }

    #[test]
    fn biquad_nan_guard_resets_state_cleanly() {
        let mut filter = BiquadFilter::highpass_butterworth_2nd(16_000.0, 80.0, 0.707_106_77);
        for _ in 0..32 {
            let _ = filter.process_sample(0.5);
        }
        assert_eq!(filter.process_sample(f32::NAN), 0.0);
        assert_eq!(filter.process_sample(f32::INFINITY), 0.0);
        assert_eq!(filter.process_sample(f32::NEG_INFINITY), 0.0);

        let mut fresh = BiquadFilter::highpass_butterworth_2nd(16_000.0, 80.0, 0.707_106_77);
        assert_eq!(filter.process_sample(1.0), fresh.process_sample(1.0));
        assert_eq!(filter.process_sample(0.0), fresh.process_sample(0.0));
    }

    #[test]
    fn resampler_fast_path_and_validation() {
        assert!(Resampler16k::new(7_999, 1).is_err());
        assert!(Resampler16k::new(16_000, 0).is_err());

        let mut fast = Resampler16k::new(16_000, 2).expect("16k stereo fast-path");
        assert_eq!(fast.src_rate_hz(), 16_000);
        assert_eq!(fast.channels(), 2);
        assert!(fast.push_interleaved(&[0.1, 0.2, 0.3]).is_err());

        let mono = fast
            .push_interleaved(&[0.2, 0.6, -0.4, 0.0])
            .expect("stereo downmix at 16k");
        assert_eq!(mono.len(), 2);
        assert!((mono[0] - 0.4).abs() < 1e-6);
        assert!((mono[1] - (-0.2)).abs() < 1e-6);

        let mut r44 = Resampler16k::new(44_100, 1).expect("44.1k mono resampler");
        let partial = r44
            .push_interleaved(&vec![0.1; 100])
            .expect("buffer partial");
        assert!(partial.is_empty());
        assert_eq!(r44.pending_frames(), 100);
        r44.reset();
        assert_eq!(r44.pending_frames(), 0);
    }

    #[test]
    fn resampler_44100_to_16k_preserves_1khz_tone() {
        let src_rate = 44_100_u32;
        let total_frames = 22_050_usize; // 0.5 s at 44.1 kHz
        let tone_hz = 1_000.0_f64;

        let mono: Vec<f32> = (0..total_frames)
            .map(|i| (2.0 * PI * tone_hz * (i as f64) / f64::from(src_rate)).sin() as f32)
            .collect();

        let mut resampler = Resampler16k::new(src_rate, 1).expect("44.1k mono resampler");
        let output = resampler.push_interleaved(&mono).expect("resample 44.1k");
        assert!(
            (7_600..=8_100).contains(&output.len()),
            "expected ~7837 samples from 0.5 s at 44.1 kHz, got {}",
            output.len()
        );
        let zc_freq = zero_crossing_frequency_hz(&output[256..], 16_000.0);
        assert!(
            (zc_freq - tone_hz).abs() <= 15.0,
            "44.1k -> 16k zero-crossing frequency {zc_freq:.2} Hz outside 1000 ± 15 Hz"
        );
    }

    #[test]
    fn condition_output_pcm_sanitizes_blocks_dc_soft_limits_fades_resamples_and_interleaves() {
        let src_rate = 22_050_u32;
        let dst_rate = 48_000_u32;
        let dst_channels = 2_u16;
        let total_src = 11_025_usize; // 0.5 s at 22.05 kHz
        let tone_hz = 1_000.0_f64;

        // Combine DC offset (+0.45), hot transient tone (amplitude 1.5 > 0.84), and NaN/Inf spikes.
        let mut input: Vec<f32> = (0..total_src)
            .map(|i| {
                let t = i as f64 / f64::from(src_rate);
                0.45 + 1.5 * (2.0 * PI * tone_hz * t).sin() as f32
            })
            .collect();
        input[120] = f32::NAN;
        input[500] = f32::INFINITY;
        input[900] = f32::NEG_INFINITY;

        let out = condition_output_pcm(&input, src_rate, dst_rate, dst_channels)
            .expect("condition 22.05k -> 48k stereo");

        // 0.5 s at 48 kHz stereo = 24_000 frames * 2 channels = 48_000 samples.
        assert_eq!(out.len() % 2, 0);
        let out_frames = out.len() / 2;
        assert!(
            (23_990..=24_010).contains(&out_frames),
            "expected ~24000 stereo frames at 48 kHz, got {out_frames}"
        );

        // 1. All output samples finite, strictly <= 0.84 ceiling, and stereo L == R.
        for pair in out.chunks_exact(2) {
            assert!(pair[0].is_finite() && pair[1].is_finite());
            assert_eq!(pair[0], pair[1]);
            assert!(
                pair[0].abs() <= OUTPUT_SOFT_LIMIT_CEILING,
                "sample {} exceeded soft-limit ceiling {}",
                pair[0],
                OUTPUT_SOFT_LIMIT_CEILING
            );
        }

        // 2. 10 ms raised-cosine fade-in & fade-out edge values are 0.0 (click/pop free).
        assert!(out[0].abs() < 1e-6 && out[1].abs() < 1e-6);
        assert!(out[out.len() - 2].abs() < 1e-6 && out[out.len() - 1].abs() < 1e-6);

        // Extract left channel to inspect DC suppression and 1 kHz tone frequency.
        let left: Vec<f32> = out.chunks_exact(2).map(|p| p[0]).collect();
        // After DC blocker settling (~150 ms = 7200 frames at 48 kHz), steady DC mean < 0.01.
        let steady = &left[7_200..left.len() - 480];
        let dc_mean = steady.iter().map(|&s| f64::from(s)).sum::<f64>() / steady.len() as f64;
        assert!(
            dc_mean.abs() < 0.01,
            "expected DC mean < 0.01 after settling, got {dc_mean}"
        );

        // Sinc-resampled 1 kHz tone at 48 kHz preserves frequency cleanly.
        let zc_freq = zero_crossing_frequency_hz(steady, f64::from(dst_rate));
        assert!(
            (zc_freq - tone_hz).abs() <= 15.0,
            "22.05k -> 48k zero-crossing frequency {zc_freq:.2} Hz outside 1000 ± 15 Hz"
        );
    }

    #[test]
    fn condition_output_pcm_validates_params_and_streaming_matches_fade_edges() {
        assert!(condition_output_pcm(&[0.1], 0, 48_000, 2).is_err());
        assert!(condition_output_pcm(&[0.1], 22_050, 0, 2).is_err());
        assert!(condition_output_pcm(&[0.1], 22_050, 48_000, 0).is_err());
        assert!(
            condition_output_pcm(&[], 22_050, 48_000, 2)
                .expect("empty input")
                .is_empty()
        );

        // Verify 10 ms raised-cosine envelope shape on a constant-amplitude 48 kHz tone
        // and verify streaming OutputPcmConditioner matches boundary tapering.
        let src_rate = 16_000_u32;
        let dst_rate = 48_000_u32;
        let samples: Vec<f32> = (0..4_800) // 300 ms at 16 kHz
            .map(|i| (2.0 * PI * 440.0 * (i as f64) / f64::from(src_rate)).sin() as f32 * 0.5)
            .collect();

        let mut cond = OutputPcmConditioner::new(src_rate, dst_rate, 2).expect("stream cond");
        let mut streamed = cond.push_mono(&samples[..2_400]).expect("chunk 1");
        assert!(
            !streamed.is_empty(),
            "150 ms chunk should emit conditioned frames immediately"
        );
        streamed.extend(cond.push_mono(&samples[2_400..]).expect("chunk 2"));
        streamed.extend(cond.finish().expect("finish"));

        assert_eq!(streamed.len(), 14_400 * 2); // 300 ms at 48 kHz stereo
        assert!(streamed[0].abs() < 1e-6);
        assert!(streamed[streamed.len() - 1].abs() < 1e-6);
    }
}
