use crate::error::AudioError;
use crate::types::{CapturedAudio, VadBackend};
use kural_core::CaptureMode;
use std::collections::VecDeque;

/// Number of `16 kHz` mono samples per VAD analysis frame (`32 ms`).
pub const FRAME_SAMPLES_16K: usize = 512;

/// Number of `16 kHz` mono samples retained in the pre-roll circular buffer (`200 ms`).
pub const PRE_ROLL_SAMPLES_16K: usize = 3200;

const FRAME_DURATION_MS: u32 = 32;
const DEFAULT_THRESHOLD_RATIO: f32 = 2.5;
const INITIAL_NOISE_FLOOR_RMS: f32 = 0.003;
const MIN_NOISE_FLOOR_RMS: f32 = 0.0015;
const MAX_NOISE_FLOOR_RMS: f32 = 0.15;
const ZCR_MIN_SPEECH: f32 = 0.02;
const ZCR_MAX_SPEECH: f32 = 0.45;
const CLIPPING_THRESHOLD: f32 = 0.99;
const MAX_NOISE_HISTORY_SAMPLES: usize = 160_000;

/// Pure-Rust adaptive energy and zero-crossing-rate fallback VAD.
#[derive(Debug, Clone)]
pub struct EnergyZcrVad {
    threshold_ratio: f32,
    noise_floor_rms: f32,
}

impl EnergyZcrVad {
    /// Construct an energy + ZCR VAD with the given energy-to-noise threshold ratio.
    #[must_use]
    pub fn new(threshold_ratio: f32) -> Self {
        let valid_ratio = if threshold_ratio.is_finite() && threshold_ratio > 0.0 {
            threshold_ratio
        } else {
            DEFAULT_THRESHOLD_RATIO
        };
        Self {
            threshold_ratio: valid_ratio,
            noise_floor_rms: INITIAL_NOISE_FLOOR_RMS,
        }
    }

    /// Return the current adaptive ambient noise floor RMS estimate.
    #[must_use]
    pub fn noise_floor_rms(&self) -> f32 {
        self.noise_floor_rms
    }

    fn adapt_noise_floor(&mut self, rms: f32) {
        if !rms.is_finite() {
            return;
        }
        let updated = if rms < self.noise_floor_rms {
            0.85 * self.noise_floor_rms + 0.15 * rms
        } else {
            let capped_target = rms.min(self.noise_floor_rms * 2.0).min(MAX_NOISE_FLOOR_RMS);
            0.92 * self.noise_floor_rms + 0.08 * capped_target
        };
        if updated.is_finite() {
            self.noise_floor_rms = updated.clamp(MIN_NOISE_FLOOR_RMS, MAX_NOISE_FLOOR_RMS);
        }
    }
}

impl Default for EnergyZcrVad {
    fn default() -> Self {
        Self::new(DEFAULT_THRESHOLD_RATIO)
    }
}

impl VadBackend for EnergyZcrVad {
    fn reset(&mut self) {
        self.noise_floor_rms = INITIAL_NOISE_FLOOR_RMS;
    }

    fn speech_probability(&mut self, frame_512_16k: &[f32]) -> Result<f32, AudioError> {
        if frame_512_16k.len() != FRAME_SAMPLES_16K {
            return Err(AudioError::StreamError(format!(
                "expected {FRAME_SAMPLES_16K} samples per VAD frame, got {}",
                frame_512_16k.len()
            )));
        }

        let rms = compute_rms(frame_512_16k);
        let zcr = compute_zcr(frame_512_16k);
        let zcr_in_speech_band = (ZCR_MIN_SPEECH..=ZCR_MAX_SPEECH).contains(&zcr);

        let floor = self.noise_floor_rms.max(MIN_NOISE_FLOOR_RMS);
        let ratio = if rms.is_finite() { rms / floor } else { 0.0 };

        let energy_score = if ratio >= self.threshold_ratio {
            let excess = (ratio - self.threshold_ratio) / self.threshold_ratio;
            (0.55 + 0.45 * (excess / (1.0 + excess))).clamp(0.55, 1.0)
        } else {
            (0.45 * (ratio / self.threshold_ratio)).clamp(0.0, 0.45)
        };

        // Adapt ambient noise floor only when quiescent (below energy gate or outside speech ZCR).
        if !zcr_in_speech_band || ratio < self.threshold_ratio {
            self.adapt_noise_floor(rms);
        }

        let prob = if zcr_in_speech_band {
            energy_score
        } else {
            energy_score * 0.25
        };

        Ok(if prob.is_finite() {
            prob.clamp(0.0, 1.0)
        } else {
            0.0
        })
    }
}

/// Incremental outcome emitted after feeding a `512`-sample frame into [`UtteranceFramer`].
#[derive(Debug, Clone, PartialEq)]
pub enum FrameAction {
    KeepWaiting,
    CaptureComplete(CapturedAudio),
    TimedOut,
}

/// Stateful `16 kHz` utterance accumulator supporting fixed-window age sampling and VAD-gated commands.
#[derive(Debug, Clone)]
pub struct UtteranceFramer {
    mode: CaptureMode,
    vad_threshold: f32,
    listening_timeout_ms: u32,
    trailing_silence_ms: u32,
    pre_roll: VecDeque<f32>,
    samples_16k: Vec<f32>,
    speech_only_16k: Vec<f32>,
    pre_roll_noise_16k: Vec<f32>,
    in_speech: bool,
    total_wait_ms: u32,
    voiced_ms: u32,
    capture_elapsed_ms: u32,
    silence_after_speech_ms: u32,
    finished: bool,
}

impl UtteranceFramer {
    /// Create a new utterance framer for `mode` with VAD threshold and timing limits.
    #[must_use]
    pub fn new(
        mode: CaptureMode,
        vad_threshold: f32,
        listening_timeout_ms: u32,
        trailing_silence_ms: u32,
    ) -> Self {
        let valid_threshold = if vad_threshold.is_finite() {
            vad_threshold.clamp(0.0, 1.0)
        } else {
            0.5
        };
        Self {
            mode,
            vad_threshold: valid_threshold,
            listening_timeout_ms,
            trailing_silence_ms,
            pre_roll: VecDeque::with_capacity(PRE_ROLL_SAMPLES_16K),
            samples_16k: Vec::new(),
            speech_only_16k: Vec::new(),
            pre_roll_noise_16k: Vec::new(),
            in_speech: false,
            total_wait_ms: 0,
            voiced_ms: 0,
            capture_elapsed_ms: 0,
            silence_after_speech_ms: 0,
            finished: false,
        }
    }

    /// Return `true` once voiced speech onset has been detected in the current capture.
    #[must_use]
    pub fn is_capturing_speech(&self) -> bool {
        self.in_speech
    }

    /// Process a single `512`-sample (`32 ms`) `16 kHz` frame through `vad`.
    pub fn feed_512_frame(
        &mut self,
        frame: &[f32],
        vad: &mut dyn VadBackend,
    ) -> Result<FrameAction, AudioError> {
        if self.finished {
            return Err(AudioError::StreamError(
                "utterance framer already finished".to_string(),
            ));
        }
        if frame.len() != FRAME_SAMPLES_16K {
            return Err(AudioError::StreamError(format!(
                "expected {FRAME_SAMPLES_16K} samples per VAD frame, got {}",
                frame.len()
            )));
        }

        let mut sanitized = [0.0_f32; FRAME_SAMPLES_16K];
        let clean_frame: &[f32] = if frame.iter().all(|s| s.is_finite()) {
            frame
        } else {
            for (dst, &src) in sanitized.iter_mut().zip(frame.iter()) {
                *dst = sanitize_sample(src);
            }
            &sanitized
        };

        let raw_prob = vad.speech_probability(clean_frame)?;
        let prob = if raw_prob.is_finite() {
            raw_prob.clamp(0.0, 1.0)
        } else {
            0.0
        };
        let is_voiced = prob >= self.vad_threshold;

        match self.mode {
            CaptureMode::FixedAgeSample { duration_ms } => {
                self.feed_fixed_age_sample(clean_frame, is_voiced, duration_ms)
            }
            CaptureMode::VadCommand { max_duration_ms } => {
                self.feed_vad_command(clean_frame, is_voiced, max_duration_ms)
            }
        }
    }

    fn feed_fixed_age_sample(
        &mut self,
        frame: &[f32],
        is_voiced: bool,
        duration_ms: u32,
    ) -> Result<FrameAction, AudioError> {
        self.samples_16k.extend_from_slice(frame);
        self.capture_elapsed_ms = self.capture_elapsed_ms.saturating_add(FRAME_DURATION_MS);

        if is_voiced {
            self.in_speech = true;
            self.voiced_ms = self.voiced_ms.saturating_add(FRAME_DURATION_MS);
            self.speech_only_16k.extend_from_slice(frame);
        } else {
            self.push_noise_frame(frame);
        }

        if self.capture_elapsed_ms >= duration_ms {
            self.finished = true;
            Ok(FrameAction::CaptureComplete(self.finalize_capture()))
        } else {
            Ok(FrameAction::KeepWaiting)
        }
    }

    fn feed_vad_command(
        &mut self,
        frame: &[f32],
        is_voiced: bool,
        max_duration_ms: u32,
    ) -> Result<FrameAction, AudioError> {
        if !self.in_speech {
            if is_voiced {
                self.in_speech = true;
                self.samples_16k.reserve(self.pre_roll.len() + frame.len());
                self.samples_16k.extend(self.pre_roll.drain(..));
                self.samples_16k.extend_from_slice(frame);
                self.speech_only_16k.extend_from_slice(frame);
                self.voiced_ms = self.voiced_ms.saturating_add(FRAME_DURATION_MS);
                self.capture_elapsed_ms = self.capture_elapsed_ms.saturating_add(FRAME_DURATION_MS);
                self.silence_after_speech_ms = 0;

                if self.capture_elapsed_ms >= max_duration_ms {
                    self.finished = true;
                    return Ok(FrameAction::CaptureComplete(self.finalize_capture()));
                }
                return Ok(FrameAction::KeepWaiting);
            }

            self.push_pre_roll(frame);
            self.push_noise_frame(frame);
            self.total_wait_ms = self.total_wait_ms.saturating_add(FRAME_DURATION_MS);

            if self.total_wait_ms >= self.listening_timeout_ms {
                self.finished = true;
                return Ok(FrameAction::TimedOut);
            }
            return Ok(FrameAction::KeepWaiting);
        }

        self.samples_16k.extend_from_slice(frame);
        self.capture_elapsed_ms = self.capture_elapsed_ms.saturating_add(FRAME_DURATION_MS);

        if is_voiced {
            self.voiced_ms = self.voiced_ms.saturating_add(FRAME_DURATION_MS);
            self.silence_after_speech_ms = 0;
            self.speech_only_16k.extend_from_slice(frame);
        } else {
            self.silence_after_speech_ms = self
                .silence_after_speech_ms
                .saturating_add(FRAME_DURATION_MS);
        }

        if self.silence_after_speech_ms >= self.trailing_silence_ms
            || self.capture_elapsed_ms >= max_duration_ms
        {
            self.finished = true;
            Ok(FrameAction::CaptureComplete(self.finalize_capture()))
        } else {
            Ok(FrameAction::KeepWaiting)
        }
    }

    fn push_pre_roll(&mut self, frame: &[f32]) {
        for &s in frame {
            if self.pre_roll.len() == PRE_ROLL_SAMPLES_16K {
                self.pre_roll.pop_front();
            }
            self.pre_roll.push_back(s);
        }
    }

    fn push_noise_frame(&mut self, frame: &[f32]) {
        let remaining = MAX_NOISE_HISTORY_SAMPLES.saturating_sub(self.pre_roll_noise_16k.len());
        if remaining >= frame.len() {
            self.pre_roll_noise_16k.extend_from_slice(frame);
        } else if remaining > 0 {
            self.pre_roll_noise_16k
                .extend_from_slice(&frame[..remaining]);
        }
    }

    fn finalize_capture(&mut self) -> CapturedAudio {
        let samples_16k = std::mem::take(&mut self.samples_16k);
        let speech_only_16k = std::mem::take(&mut self.speech_only_16k);
        let pre_roll_noise_16k = std::mem::take(&mut self.pre_roll_noise_16k);

        let active_speech = if !speech_only_16k.is_empty() {
            speech_only_16k.as_slice()
        } else {
            samples_16k.as_slice()
        };
        let rms_level = compute_rms(active_speech);
        let clipping_ratio = compute_clipping_ratio(&samples_16k);
        let snr_db = compute_snr_db(rms_level, active_speech, &pre_roll_noise_16k);

        CapturedAudio {
            samples_16k,
            speech_only_16k,
            pre_roll_noise_16k,
            voiced_ms: self.voiced_ms,
            snr_db,
            clipping_ratio,
            rms_level,
        }
    }
}

fn sanitize_sample(sample: f32) -> f32 {
    if sample.is_finite() { sample } else { 0.0 }
}

fn compute_rms(samples: &[f32]) -> f32 {
    if samples.is_empty() {
        return 0.0;
    }
    let sum_sq: f64 = samples
        .iter()
        .map(|&s| {
            let v = f64::from(sanitize_sample(s));
            v * v
        })
        .sum();
    let rms = (sum_sq / (samples.len() as f64)).sqrt() as f32;
    if rms.is_finite() { rms } else { 0.0 }
}

fn compute_zcr(samples: &[f32]) -> f32 {
    if samples.len() < 2 {
        return 0.0;
    }
    let mut crossings = 0usize;
    let mut prev_nonneg = sanitize_sample(samples[0]) >= 0.0;
    for &raw in &samples[1..] {
        let cur_nonneg = sanitize_sample(raw) >= 0.0;
        if cur_nonneg != prev_nonneg {
            crossings += 1;
        }
        prev_nonneg = cur_nonneg;
    }
    crossings as f32 / (samples.len() - 1) as f32
}

fn compute_clipping_ratio(samples: &[f32]) -> f32 {
    if samples.is_empty() {
        return 0.0;
    }
    let clipped = samples
        .iter()
        .filter(|&&s| !s.is_finite() || s.abs() >= CLIPPING_THRESHOLD)
        .count();
    clipped as f32 / samples.len() as f32
}

fn frame_rms_quantile(samples: &[f32], numerator: usize, denominator: usize) -> Option<f32> {
    if samples.len() < FRAME_SAMPLES_16K || denominator == 0 {
        return None;
    }
    let mut frame_rms: Vec<f32> = samples
        .chunks_exact(FRAME_SAMPLES_16K)
        .map(compute_rms)
        .filter(|r| r.is_finite())
        .collect();
    if frame_rms.is_empty() {
        return None;
    }
    frame_rms.sort_by(f32::total_cmp);
    let idx = ((frame_rms.len() - 1) * numerator) / denominator;
    Some(frame_rms[idx])
}

fn compute_snr_db(signal_rms: f32, active_speech: &[f32], pre_roll_noise: &[f32]) -> f32 {
    let speech_rms = signal_rms.max(frame_rms_quantile(active_speech, 3, 4).unwrap_or(0.0));
    if !speech_rms.is_finite() || speech_rms <= 1e-6 {
        return 0.0;
    }
    let noise_rms = if pre_roll_noise.is_empty() {
        1e-3_f32
    } else {
        frame_rms_quantile(pre_roll_noise, 1, 4)
            .unwrap_or_else(|| compute_rms(pre_roll_noise))
            .max(1e-5)
    };
    let snr = 20.0 * (speech_rms / noise_rms).log10();
    if snr.is_finite() {
        snr.clamp(-60.0, 80.0)
    } else {
        0.0
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::f32::consts::PI;

    fn sine_frame(freq_hz: f32, amplitude: f32, frame_idx: usize) -> Vec<f32> {
        let sample_offset = frame_idx * FRAME_SAMPLES_16K;
        (0..FRAME_SAMPLES_16K)
            .map(|i| {
                let t = (sample_offset + i) as f32 / 16_000.0;
                amplitude * (2.0 * PI * freq_hz * t).sin()
            })
            .collect()
    }

    #[test]
    fn listening_timeout_triggers_timed_out_on_silence() {
        let mut vad = EnergyZcrVad::default();
        let mut framer = UtteranceFramer::new(
            CaptureMode::VadCommand {
                max_duration_ms: 5000,
            },
            0.5,
            320,
            850,
        );
        let silence = vec![0.0_f32; FRAME_SAMPLES_16K];

        // 9 frames = 288 ms (< 320 ms)
        for _ in 0..9 {
            let action = framer.feed_512_frame(&silence, &mut vad).expect("feed");
            assert_eq!(action, FrameAction::KeepWaiting);
            assert!(!framer.is_capturing_speech());
        }

        // 10th frame = 320 ms (>= 320 ms)
        let action = framer.feed_512_frame(&silence, &mut vad).expect("feed");
        assert_eq!(action, FrameAction::TimedOut);
        assert!(!framer.is_capturing_speech());
    }

    #[test]
    fn pre_roll_200ms_is_preserved_ahead_of_speech_onset() {
        let mut vad = EnergyZcrVad::default();
        let mut framer = UtteranceFramer::new(
            CaptureMode::VadCommand {
                max_duration_ms: 5000,
            },
            0.5,
            5000,
            850,
        );

        // Feed 10 distinct silent/quiet frames (> 3200 samples total) so circular buffer wraps.
        // Tag each waiting frame with tiny distinguishable DC-free micro-marker well below VAD floor.
        for frame_idx in 0..10 {
            let marker = (frame_idx as f32 + 1.0) * 1e-5;
            let quiet_frame = vec![marker; FRAME_SAMPLES_16K];
            let action = framer
                .feed_512_frame(&quiet_frame, &mut vad)
                .expect("quiet frame");
            assert_eq!(action, FrameAction::KeepWaiting);
        }

        // Feed 3 voiced frames (350 Hz vowel-like sine at 0.15 amplitude).
        for idx in 0..3 {
            let speech = sine_frame(350.0, 0.15, idx);
            let action = framer
                .feed_512_frame(&speech, &mut vad)
                .expect("speech frame");
            assert_eq!(action, FrameAction::KeepWaiting);
            assert!(framer.is_capturing_speech());
        }

        // Feed 27 silent frames (27 * 32 = 864 ms >= 850 ms trailing silence).
        let silence = vec![0.0_f32; FRAME_SAMPLES_16K];
        let mut final_capture = None;
        for _ in 0..27 {
            match framer.feed_512_frame(&silence, &mut vad).expect("trailing") {
                FrameAction::KeepWaiting => {}
                FrameAction::CaptureComplete(c) => {
                    final_capture = Some(c);
                    break;
                }
                FrameAction::TimedOut => panic!("unexpected timeout after speech"),
            }
        }

        let captured = final_capture.expect("capture should complete");
        // Expected samples: 3200 pre-roll + (3 speech + 27 trailing) * 512 = 18560 samples.
        assert_eq!(
            captured.samples_16k.len(),
            PRE_ROLL_SAMPLES_16K + 30 * FRAME_SAMPLES_16K
        );
        // The first sample of pre-roll comes from frame_idx 3 (last 3200 of 5120 = offset 1920 -> frame 3).
        assert!((captured.samples_16k[0] - 4.0e-5).abs() < 1e-7);
        assert_eq!(captured.pre_roll_noise_16k.len(), 10 * FRAME_SAMPLES_16K);
        assert_eq!(captured.speech_only_16k.len(), 3 * FRAME_SAMPLES_16K);
        assert_eq!(captured.voiced_ms, 96);
        assert!(captured.snr_db > 20.0);
    }

    #[test]
    fn trailing_silence_850ms_auto_stops_vad_command() {
        let mut vad = EnergyZcrVad::default();
        let mut framer = UtteranceFramer::new(
            CaptureMode::VadCommand {
                max_duration_ms: 7000,
            },
            0.5,
            5000,
            850,
        );

        // Start directly with speech (4 frames = 128 ms).
        for idx in 0..4 {
            let speech = sine_frame(440.0, 0.2, idx);
            assert_eq!(
                framer.feed_512_frame(&speech, &mut vad).unwrap(),
                FrameAction::KeepWaiting
            );
        }

        // Brief pause of 10 frames (320 ms < 850 ms) followed by resumed speech must not stop early.
        let silence = vec![0.0_f32; FRAME_SAMPLES_16K];
        for _ in 0..10 {
            assert_eq!(
                framer.feed_512_frame(&silence, &mut vad).unwrap(),
                FrameAction::KeepWaiting
            );
        }
        let resumed = sine_frame(520.0, 0.2, 0);
        assert_eq!(
            framer.feed_512_frame(&resumed, &mut vad).unwrap(),
            FrameAction::KeepWaiting
        );

        // Now 26 frames (832 ms) still keep waiting; 27th frame (864 ms >= 850 ms) completes.
        for _ in 0..26 {
            assert_eq!(
                framer.feed_512_frame(&silence, &mut vad).unwrap(),
                FrameAction::KeepWaiting
            );
        }
        match framer.feed_512_frame(&silence, &mut vad).unwrap() {
            FrameAction::CaptureComplete(captured) => {
                assert_eq!(captured.voiced_ms, 5 * 32);
                assert_eq!(captured.speech_only_16k.len(), 5 * FRAME_SAMPLES_16K);
                assert!(captured.rms_level > 0.1);
                assert_eq!(captured.clipping_ratio, 0.0);
            }
            other => panic!("expected CaptureComplete, got {other:?}"),
        }
    }

    #[test]
    fn vad_command_stops_at_max_duration_ms() {
        let mut vad = EnergyZcrVad::default();
        let mut framer = UtteranceFramer::new(
            CaptureMode::VadCommand {
                max_duration_ms: 160,
            },
            0.5,
            5000,
            850,
        );

        // 4 voiced frames = 128 ms (< 160 ms), 5th voiced frame = 160 ms (>= 160 ms).
        for idx in 0..4 {
            let speech = sine_frame(300.0, 0.15, idx);
            assert_eq!(
                framer.feed_512_frame(&speech, &mut vad).unwrap(),
                FrameAction::KeepWaiting
            );
        }
        let speech = sine_frame(300.0, 0.15, 4);
        match framer.feed_512_frame(&speech, &mut vad).unwrap() {
            FrameAction::CaptureComplete(captured) => {
                assert_eq!(captured.voiced_ms, 160);
                assert_eq!(captured.samples_16k.len(), 5 * FRAME_SAMPLES_16K);
            }
            other => panic!("expected CaptureComplete at max_duration_ms, got {other:?}"),
        }
    }

    #[test]
    fn fixed_age_sample_captures_full_duration_and_speech_only_frames() {
        let mut vad = EnergyZcrVad::default();
        let mut framer = UtteranceFramer::new(
            CaptureMode::FixedAgeSample { duration_ms: 640 },
            0.5,
            500,
            850,
        );

        let silence = vec![0.0_f32; FRAME_SAMPLES_16K];
        // 5 silent + 10 voiced + 5 silent = 20 frames = 640 ms.
        for _ in 0..5 {
            assert_eq!(
                framer.feed_512_frame(&silence, &mut vad).unwrap(),
                FrameAction::KeepWaiting
            );
            assert!(!framer.is_capturing_speech());
        }
        for idx in 0..10 {
            let speech = sine_frame(250.0, 0.18, idx);
            assert_eq!(
                framer.feed_512_frame(&speech, &mut vad).unwrap(),
                FrameAction::KeepWaiting
            );
            assert!(framer.is_capturing_speech());
        }
        let mut completed = None;
        for _ in 0..5 {
            match framer.feed_512_frame(&silence, &mut vad).unwrap() {
                FrameAction::KeepWaiting => {}
                FrameAction::CaptureComplete(c) => {
                    completed = Some(c);
                }
                FrameAction::TimedOut => panic!("FixedAgeSample must never time out"),
            }
        }

        let captured = completed.expect("FixedAgeSample should complete at 640 ms");
        assert_eq!(captured.samples_16k.len(), 20 * FRAME_SAMPLES_16K);
        assert_eq!(captured.speech_only_16k.len(), 10 * FRAME_SAMPLES_16K);
        assert_eq!(captured.pre_roll_noise_16k.len(), 10 * FRAME_SAMPLES_16K);
        assert_eq!(captured.voiced_ms, 320);
        assert!(captured.snr_db > 30.0);
    }

    #[test]
    fn energy_zcr_vad_rejects_out_of_band_zcr_and_adapts_noise_floor() {
        let mut vad = EnergyZcrVad::default();

        // Sub-speech 50 Hz rumble has ZCR ~0.006 (< 0.02), must stay below 0.5 even at high amplitude.
        let rumble = sine_frame(50.0, 0.3, 0);
        let rumble_prob = vad.speech_probability(&rumble).unwrap();
        assert!(rumble_prob < 0.5, "50 Hz rumble prob was {rumble_prob}");

        // High-frequency 7000 Hz hiss has ZCR ~0.875 (> 0.45), must stay below 0.5.
        let hiss = sine_frame(7000.0, 0.3, 0);
        let hiss_prob = vad.speech_probability(&hiss).unwrap();
        assert!(hiss_prob < 0.5, "7 kHz hiss prob was {hiss_prob}");

        // Reset and check downward noise-floor adaptation on digital silence.
        vad.reset();
        let initial_floor = vad.noise_floor_rms();
        let silence = vec![0.0_f32; FRAME_SAMPLES_16K];
        for _ in 0..10 {
            let p = vad.speech_probability(&silence).unwrap();
            assert_eq!(p, 0.0);
        }
        assert!(vad.noise_floor_rms() < initial_floor);
        assert!(vad.noise_floor_rms() >= MIN_NOISE_FLOOR_RMS);

        // Valid vowel-band 440 Hz tone triggers speech probability >= 0.5.
        let vowel = sine_frame(440.0, 0.12, 0);
        let vowel_prob = vad.speech_probability(&vowel).unwrap();
        assert!(vowel_prob >= 0.5, "440 Hz vowel prob was {vowel_prob}");
    }

    #[test]
    fn non_finite_samples_are_guarded_in_vad_and_framer() {
        let mut vad = EnergyZcrVad::new(f32::NAN);
        let mut toxic = vec![f32::NAN; FRAME_SAMPLES_16K];
        toxic[0] = f32::INFINITY;
        toxic[1] = f32::NEG_INFINITY;

        let prob = vad.speech_probability(&toxic).unwrap();
        assert!(prob.is_finite());
        assert_eq!(prob, 0.0);
        assert!(vad.noise_floor_rms().is_finite());

        let mut framer = UtteranceFramer::new(
            CaptureMode::FixedAgeSample { duration_ms: 32 },
            f32::NAN,
            500,
            850,
        );
        match framer.feed_512_frame(&toxic, &mut vad).unwrap() {
            FrameAction::CaptureComplete(captured) => {
                assert!(captured.samples_16k.iter().all(|s| s.is_finite()));
                assert!(captured.rms_level.is_finite());
                assert!(captured.snr_db.is_finite());
                assert!(captured.clipping_ratio.is_finite());
            }
            other => panic!("expected CaptureComplete, got {other:?}"),
        }
    }

    #[test]
    fn invalid_frame_length_returns_stream_error() {
        let mut vad = EnergyZcrVad::default();
        let short = vec![0.0_f32; 256];
        assert!(matches!(
            vad.speech_probability(&short),
            Err(AudioError::StreamError(_))
        ));

        let mut framer = UtteranceFramer::new(
            CaptureMode::VadCommand {
                max_duration_ms: 1000,
            },
            0.5,
            500,
            850,
        );
        assert!(matches!(
            framer.feed_512_frame(&short, &mut vad),
            Err(AudioError::StreamError(_))
        ));
    }

    #[test]
    fn fixed_age_sample_snr_resists_vad_onset_lag_and_transient_spikes() {
        struct LaggyVad {
            lag_frames: usize,
        }
        impl VadBackend for LaggyVad {
            fn reset(&mut self) {}
            fn speech_probability(&mut self, frame_512_16k: &[f32]) -> Result<f32, AudioError> {
                if compute_rms(frame_512_16k) < 0.05 {
                    return Ok(0.0);
                }
                if self.lag_frames > 0 {
                    self.lag_frames -= 1;
                    return Ok(0.0);
                }
                Ok(0.95)
            }
        }

        let mut vad = LaggyVad { lag_frames: 4 };
        let mut framer = UtteranceFramer::new(
            CaptureMode::FixedAgeSample {
                duration_ms: 56 * FRAME_DURATION_MS,
            },
            0.5,
            500,
            850,
        );

        // 12 quiet ambient frames + 4 loud speech-onset frames missed due to VAD lag + 40 speech frames.
        for idx in 0..12 {
            let ambient = sine_frame(120.0, 0.005, idx);
            assert_eq!(
                framer.feed_512_frame(&ambient, &mut vad).unwrap(),
                FrameAction::KeepWaiting
            );
        }
        let mut completed = None;
        for idx in 0..44 {
            let speech = sine_frame(240.0, 0.15, idx);
            match framer.feed_512_frame(&speech, &mut vad).unwrap() {
                FrameAction::KeepWaiting => {}
                FrameAction::CaptureComplete(c) => {
                    completed = Some(c);
                }
                FrameAction::TimedOut => panic!("FixedAgeSample must never time out"),
            }
        }

        let captured = completed.expect("FixedAgeSample should complete");
        assert!(
            captured.snr_db > 18.0,
            "expected quantile SNR > 18 dB despite 4 onset lag frames, got {:.2} dB",
            captured.snr_db
        );
    }

    #[test]
    fn fixed_age_sample_snr_still_rejects_steady_loud_background_noise() {
        struct ThresholdVad;
        impl VadBackend for ThresholdVad {
            fn reset(&mut self) {}
            fn speech_probability(&mut self, frame_512_16k: &[f32]) -> Result<f32, AudioError> {
                Ok(if compute_rms(frame_512_16k) >= 0.10 {
                    0.95
                } else {
                    0.0
                })
            }
        }

        let mut vad = ThresholdVad;
        let mut framer = UtteranceFramer::new(
            CaptureMode::FixedAgeSample {
                duration_ms: 40 * FRAME_DURATION_MS,
            },
            0.5,
            500,
            850,
        );

        for idx in 0..15 {
            let loud_noise = sine_frame(300.0, 0.09, idx);
            assert_eq!(
                framer.feed_512_frame(&loud_noise, &mut vad).unwrap(),
                FrameAction::KeepWaiting
            );
        }
        let mut completed = None;
        for idx in 0..25 {
            let speech_over_noise = sine_frame(300.0, 0.15, idx);
            match framer.feed_512_frame(&speech_over_noise, &mut vad).unwrap() {
                FrameAction::KeepWaiting => {}
                FrameAction::CaptureComplete(c) => {
                    completed = Some(c);
                }
                FrameAction::TimedOut => panic!("FixedAgeSample must never time out"),
            }
        }

        let captured = completed.expect("FixedAgeSample should complete");
        assert!(
            captured.snr_db < 8.0,
            "expected steady loud background noise to stay below 8 dB, got {:.2} dB",
            captured.snr_db
        );
    }
}
