use crate::types::EarconKind;
use std::f32::consts::PI;

/// Sample rate (`22,050 Hz` mono) for all pre-synthesized system earcons.
pub const EARCON_SAMPLE_RATE_HZ: u32 = 22_050;

const FADE_MS: u32 = 8;

#[derive(Debug, Clone)]
struct EarconClip {
    f32_samples: Vec<f32>,
    i16_samples: Vec<i16>,
}

impl EarconClip {
    fn from_f32(f32_samples: Vec<f32>) -> Self {
        let i16_samples = f32_samples.iter().copied().map(f32_to_i16).collect();
        Self {
            f32_samples,
            i16_samples,
        }
    }
}

/// Pre-synthesized pure-Rust system audio earcons (`ListenStart`, `CmdRecognized`, `RetryTone`).
#[derive(Debug, Clone)]
pub struct EarconBank {
    listen_start: EarconClip,
    cmd_recognized: EarconClip,
    retry_tone: EarconClip,
}

impl EarconBank {
    /// Synthesize all three earcons once with raised-cosine fade-in/out envelopes.
    #[must_use]
    pub fn new() -> Self {
        // ListenStart: 90 ms soft ascending chime (523.25 Hz 45 ms + 659.25 Hz 45 ms, gain 0.25).
        let mut listen_f32 = synthesize_tone_segment(523.25, ms_to_samples(45), 0.25, FADE_MS);
        listen_f32.extend(synthesize_tone_segment(
            659.25,
            ms_to_samples(45),
            0.25,
            FADE_MS,
        ));

        // CmdRecognized: 80 ms warm two-tone confirmation (659.25 Hz + 783.99 Hz, gain 0.22).
        let cmd_f32 = synthesize_dyad_segment(659.25, 783.99, ms_to_samples(80), 0.22, FADE_MS);

        // RetryTone: 140 ms gentle low double-pulse (330.0 Hz 60 ms + 20 ms gap + 261.63 Hz 60 ms, gain 0.22).
        let mut retry_f32 = synthesize_tone_segment(330.0, ms_to_samples(60), 0.22, FADE_MS);
        retry_f32.resize(retry_f32.len() + ms_to_samples(20), 0.0);
        retry_f32.extend(synthesize_tone_segment(
            261.63,
            ms_to_samples(60),
            0.22,
            FADE_MS,
        ));

        Self {
            listen_start: EarconClip::from_f32(listen_f32),
            cmd_recognized: EarconClip::from_f32(cmd_f32),
            retry_tone: EarconClip::from_f32(retry_f32),
        }
    }

    /// Return pre-synthesized `22,050 Hz` mono `f32` samples for `kind`.
    #[must_use]
    pub fn samples_f32(&self, kind: EarconKind) -> &[f32] {
        &self.clip(kind).f32_samples
    }

    /// Return pre-synthesized `22,050 Hz` mono `i16` samples for `kind`.
    #[must_use]
    pub fn samples_i16(&self, kind: EarconKind) -> &[i16] {
        &self.clip(kind).i16_samples
    }

    /// Return nominal playback duration in milliseconds for `kind`.
    #[must_use]
    pub fn duration_ms(&self, kind: EarconKind) -> u64 {
        let len = self.samples_f32(kind).len() as f64;
        (len * 1000.0 / f64::from(EARCON_SAMPLE_RATE_HZ)).round() as u64
    }

    fn clip(&self, kind: EarconKind) -> &EarconClip {
        match kind {
            EarconKind::ListenStart => &self.listen_start,
            EarconKind::CmdRecognized => &self.cmd_recognized,
            EarconKind::RetryTone => &self.retry_tone,
        }
    }
}

impl Default for EarconBank {
    fn default() -> Self {
        Self::new()
    }
}

fn ms_to_samples(ms: u32) -> usize {
    (EARCON_SAMPLE_RATE_HZ as usize * ms as usize) / 1000
}

fn raised_cosine_envelope(idx: usize, total: usize, fade_samples: usize) -> f32 {
    if total == 0 {
        return 0.0;
    }
    let fade = fade_samples.min(total / 2).max(1);
    if idx < fade {
        let phase = idx as f32 / (fade - 1).max(1) as f32;
        0.5 * (1.0 - (PI * phase).cos())
    } else if idx >= total - fade {
        let rem = total - 1 - idx;
        let phase = rem as f32 / (fade - 1).max(1) as f32;
        0.5 * (1.0 - (PI * phase).cos())
    } else {
        1.0
    }
}

fn synthesize_tone_segment(freq_hz: f32, len: usize, gain: f32, fade_ms: u32) -> Vec<f32> {
    let fade_samples = ms_to_samples(fade_ms);
    let sr = EARCON_SAMPLE_RATE_HZ as f32;
    (0..len)
        .map(|i| {
            let env = raised_cosine_envelope(i, len, fade_samples);
            let t = i as f32 / sr;
            gain * env * (2.0 * PI * freq_hz * t).sin()
        })
        .collect()
}

fn synthesize_dyad_segment(
    freq_a_hz: f32,
    freq_b_hz: f32,
    len: usize,
    gain: f32,
    fade_ms: u32,
) -> Vec<f32> {
    let fade_samples = ms_to_samples(fade_ms);
    let sr = EARCON_SAMPLE_RATE_HZ as f32;
    (0..len)
        .map(|i| {
            let env = raised_cosine_envelope(i, len, fade_samples);
            let t = i as f32 / sr;
            let wave = 0.5 * ((2.0 * PI * freq_a_hz * t).sin() + (2.0 * PI * freq_b_hz * t).sin());
            gain * env * wave
        })
        .collect()
}

fn f32_to_i16(sample: f32) -> i16 {
    (sample.clamp(-1.0, 1.0) * f32::from(i16::MAX)).round() as i16
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn exact_sample_lengths_and_durations_at_22050hz() {
        let bank = EarconBank::new();

        assert_eq!(
            bank.samples_f32(EarconKind::ListenStart).len(),
            ms_to_samples(45) * 2
        );
        assert_eq!(bank.samples_f32(EarconKind::ListenStart).len(), 1984);
        assert_eq!(bank.duration_ms(EarconKind::ListenStart), 90);

        assert_eq!(
            bank.samples_f32(EarconKind::CmdRecognized).len(),
            ms_to_samples(80)
        );
        assert_eq!(bank.samples_f32(EarconKind::CmdRecognized).len(), 1764);
        assert_eq!(bank.duration_ms(EarconKind::CmdRecognized), 80);

        assert_eq!(
            bank.samples_f32(EarconKind::RetryTone).len(),
            ms_to_samples(140)
        );
        assert_eq!(bank.samples_f32(EarconKind::RetryTone).len(), 3087);
        assert_eq!(bank.duration_ms(EarconKind::RetryTone), 140);
    }

    #[test]
    fn non_clipping_amplitude_within_safe_bounds() {
        let bank = EarconBank::default();
        for kind in [
            EarconKind::ListenStart,
            EarconKind::CmdRecognized,
            EarconKind::RetryTone,
        ] {
            let samples = bank.samples_f32(kind);
            assert!(!samples.is_empty());
            for &x in samples {
                assert!(x.is_finite());
                assert!(
                    x.abs() <= 0.35,
                    "sample amplitude {x} exceeded 0.35 for {kind:?}"
                );
            }
        }
    }

    #[test]
    fn smooth_start_and_end_tapering_prevents_clicks() {
        let bank = EarconBank::new();
        for kind in [
            EarconKind::ListenStart,
            EarconKind::CmdRecognized,
            EarconKind::RetryTone,
        ] {
            let samples = bank.samples_f32(kind);
            let first = samples.first().copied().unwrap();
            let last = samples.last().copied().unwrap();
            assert!(
                first.abs() < 0.01,
                "first sample {first} not tapered for {kind:?}"
            );
            assert!(
                last.abs() < 0.01,
                "last sample {last} not tapered for {kind:?}"
            );
        }

        // Verify midpoint transition in ListenStart is also click-free.
        let listen = bank.samples_f32(EarconKind::ListenStart);
        let mid = ms_to_samples(45);
        assert!(listen[mid - 1].abs() < 0.01);
        assert!(listen[mid].abs() < 0.01);
    }

    #[test]
    fn i16_samples_match_f32_samples_without_overflow() {
        let bank = EarconBank::new();
        for kind in [
            EarconKind::ListenStart,
            EarconKind::CmdRecognized,
            EarconKind::RetryTone,
        ] {
            let f32s = bank.samples_f32(kind);
            let i16s = bank.samples_i16(kind);
            assert_eq!(f32s.len(), i16s.len());
            assert!(i16s.iter().all(|&s| s.unsigned_abs() <= 11_500));
        }
    }

    #[test]
    fn retry_tone_has_silent_middle_gap() {
        let bank = EarconBank::new();
        let retry = bank.samples_f32(EarconKind::RetryTone);
        let gap_start = ms_to_samples(60);
        let gap_end = gap_start + ms_to_samples(20);
        assert!(retry[gap_start..gap_end].iter().all(|&s| s == 0.0));
    }
}
