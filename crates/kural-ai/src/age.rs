//! Instant offline Vocal-Tract Voice Age AI (`< 30 ms` DSP + calibrated softmax classifier).
//!
//! Pipeline:
//! 1. **Stage 1 (`< 3 ms`)**: SNR-coupled speech quality gate + 512-pt Hann STFT pre-roll noise subtraction.
//! 2. **Stage 2 (`< 18 ms`)**: Top-40% steady-state vowel-nucleus ranking + 14 acoustic features
//!    (4 pYIN `F0` stats, 4 multi-order Burg LPC + Aberth-Ehrlich formant/dispersion features + `vtl_cm`,
//!    and 6 mel-cepstral/spectral features).
//! 3. **Stage 3 (`< 1 ms`)**: Auditable `const` linear-logistic temperature-scaled softmax scorer
//!    separating `Kids`, `Youth`, and `Adult` (including `240 Hz` Adult Female vs `285 Hz` Kid via VTL).

use std::f32::consts::PI as PI32;
use std::f64::consts::PI as PI64;
use std::sync::{Arc, OnceLock};
use std::time::Instant;

use kural_audio::CapturedAudio;
use kural_core::{AgeAiConfig, AgeDetection, AgeGroup, AgeOutcome, RetryReason};
use rustfft::num_complex::Complex;
use rustfft::{Fft, FftPlanner};

const SAMPLE_RATE_16K: f32 = 16_000.0;
const SAMPLE_RATE_10K: f32 = 10_000.0;
const SPEED_OF_SOUND_CM_PER_S: f32 = 35_000.0;

const STFT_FFT_LEN: usize = 512;
const STFT_HOP_LEN: usize = 256;
const NOISE_OVERSUB_ALPHA: f32 = 1.25;
const NOISE_FLOOR_BETA: f32 = 0.09;
const NOISE_MIN_GAIN: f32 = 0.30;

const FRAME_LEN_16K: usize = 400; // 25 ms @ 16 kHz
const FRAME_HOP_16K: usize = 160; // 10 ms @ 16 kHz
const MAX_ANALYSIS_FRAMES: usize = 250;
const MAX_ANALYSIS_SAMPLES_16K: usize = FRAME_LEN_16K + (MAX_ANALYSIS_FRAMES - 1) * FRAME_HOP_16K;
const MIN_FRAMES_REQUIRED: usize = 8;

const YIN_FFT_LEN: usize = 512;
const YIN_WINDOW_SAMPLES: usize = 256; // 16 ms integration window (256 + 229 = 485 < 512)
const F0_MIN_HZ: f32 = 70.0;
const F0_MAX_HZ: f32 = 500.0;
const YIN_TAU_MIN: usize = 32; // 16000 / 500 = 32
const YIN_TAU_MAX: usize = 229; // ceil(16000 / 70) = 229

const FRAME_LEN_10K: usize = 250; // 25 ms @ 10 kHz
const RESAMPLE_HALF_TAPS: usize = 10;
const RESAMPLE_TAPS: usize = RESAMPLE_HALF_TAPS * 2 + 1;
const PRE_EMPHASIS_COEFF: f32 = 0.97;

const MAX_LPC_ORDER: usize = 14;
const HIGH_PITCH_LPC_THRESHOLD_HZ: f32 = 240.0;
const POLE_MIN_HZ: f32 = 250.0;
const POLE_MAX_HZ: f32 = 4600.0;
const POLE_BW_SPLIT_HZ: f32 = 2200.0;
const POLE_MAX_BW_LOW_HZ: f32 = 450.0;
const POLE_MAX_BW_HIGH_HZ: f32 = 600.0;
const MIN_POLES_PER_FRAME: usize = 3;
const MIN_VALID_FORMANT_FRAMES: usize = 5;

const FITCH_DISPERSION_DENOM: f32 = 0.25 + 2.25 + 6.25; // 8.75

const MEL_BANDS: usize = 26;
const MEL_MIN_HZ: f32 = 50.0;
const MEL_MAX_HZ: f32 = 7600.0;

const SOFTMAX_TEMPERATURE: f32 = 1.0;

/// Auditable 14 acoustic vocal-tract features plus physical `vtl_cm` and `formant_frames_used`.
#[derive(Debug, Clone, PartialEq)]
pub struct VocalTractFeatures {
    pub f0_median: f32,
    pub f0_iqr: f32,
    pub f0_p10: f32,
    pub f0_p90: f32,
    pub f1_hz: f32,
    pub f2_hz: f32,
    pub f3_hz: f32,
    pub formant_dispersion_hz: f32,
    pub vtl_cm: f32,
    pub formant_frames_used: usize,
    pub mfcc_1: f32,
    pub mfcc_2: f32,
    pub mfcc_3: f32,
    pub spectral_centroid_hz: f32,
    pub spectral_tilt_db: f32,
    pub hnr_db: f32,
}

#[derive(Debug, Clone, Copy)]
struct ClassLinearWeights {
    vtl: f32,
    dispersion: f32,
    f0: f32,
    centroid: f32,
    tilt: f32,
    mfcc1: f32,
    bias: f32,
}

/// Primary formant-guided weights `[Kids, Youth, Adult]` driven by `vtl_cm`,
/// `formant_dispersion_hz`, `f0_median`, and spectral shape cues.
const FORMANT_CLASS_WEIGHTS: [ClassLinearWeights; 3] = [
    // Kids (6-12): short vocal tract (10.5..13.0 cm), wide formant dispersion, elevated F0.
    ClassLinearWeights {
        vtl: -3.60,
        dispersion: 1.50,
        f0: 0.90,
        centroid: 0.22,
        tilt: 0.12,
        mfcc1: 0.05,
        bias: -2.20,
    },
    // Youth (13-17): transitional VTL (13.1..14.9 cm) with mid/dropping pitch.
    ClassLinearWeights {
        vtl: 0.00,
        dispersion: 0.00,
        f0: -0.35,
        centroid: 0.00,
        tilt: 0.00,
        mfcc1: 0.00,
        bias: 1.05,
    },
    // Adult (18+): mature vocal tract (Female ~14.0..15.8 cm, Male ~16.5..18.5 cm).
    ClassLinearWeights {
        vtl: 3.60,
        dispersion: -1.50,
        f0: 0.45,
        centroid: -0.18,
        tilt: -0.10,
        mfcc1: -0.05,
        bias: 0.05,
    },
];

/// Graceful backup weights `[Kids, Youth, Adult]` when fewer than 5 formant frames survive.
const BACKUP_CLASS_WEIGHTS: [ClassLinearWeights; 3] = [
    ClassLinearWeights {
        vtl: 0.0,
        dispersion: 0.0,
        f0: 2.20,
        centroid: 0.10,
        tilt: 0.08,
        mfcc1: 0.05,
        bias: -1.35,
    },
    ClassLinearWeights {
        vtl: 0.0,
        dispersion: 0.0,
        f0: 0.00,
        centroid: 0.00,
        tilt: 0.00,
        mfcc1: 0.00,
        bias: 0.45,
    },
    ClassLinearWeights {
        vtl: 0.0,
        dispersion: 0.0,
        f0: -2.00,
        centroid: -0.10,
        tilt: -0.08,
        mfcc1: -0.05,
        bias: -0.55,
    },
];

/// Offline DSP + vocal-tract Voice Age AI classifier (`< 30 ms` target on Pi 4B).
#[derive(Debug, Clone)]
pub struct AgeClassifier {
    config: AgeAiConfig,
}

impl AgeClassifier {
    /// Construct an `AgeClassifier` using calibrated quality-gate and offset settings.
    #[must_use]
    pub fn new(config: AgeAiConfig) -> Self {
        Self { config }
    }

    /// Run Stage 1 quality gates + pre-roll noise subtraction, Stage 2 vowel-nucleus
    /// 14-feature extraction, and Stage 3 calibrated softmax classification.
    #[must_use]
    pub fn classify(&self, audio: &CapturedAudio) -> AgeOutcome {
        let started = Instant::now();

        if audio.voiced_ms < self.config.min_voiced_ms {
            return AgeOutcome::Retry(RetryReason::TooShort);
        }
        if !audio.clipping_ratio.is_finite()
            || audio.clipping_ratio >= self.config.max_clipping_ratio
        {
            return AgeOutcome::Retry(RetryReason::Clipped);
        }
        if !audio.snr_db.is_finite() || audio.snr_db < self.config.min_snr_db {
            return AgeOutcome::Retry(RetryReason::TooNoisy);
        }
        if !audio.rms_level.is_finite()
            || audio.rms_level < 0.005
            || (audio.rms_level < self.config.min_rms_level && audio.snr_db < 12.0)
        {
            return AgeOutcome::Retry(RetryReason::TooQuiet);
        }

        let speech = if audio.speech_only_16k.is_empty() {
            audio.samples_16k.as_slice()
        } else {
            audio.speech_only_16k.as_slice()
        };

        let Some(features) =
            extract_vocal_tract_features(speech, &audio.pre_roll_noise_16k, &self.config)
        else {
            let reason = if speech.len() < FRAME_LEN_16K + (MIN_FRAMES_REQUIRED - 1) * FRAME_HOP_16K
            {
                RetryReason::TooShort
            } else {
                RetryReason::TooQuiet
            };
            return AgeOutcome::Retry(reason);
        };

        let (age, confidence) = classify_features(&features);
        let latency_ms = started.elapsed().as_millis().min(u128::from(u32::MAX)) as u32;

        AgeOutcome::Detected(AgeDetection {
            age,
            confidence,
            low_confidence: confidence < self.config.low_confidence_threshold,
            f0_hz: features.f0_median,
            formant_dispersion_hz: features.formant_dispersion_hz,
            vtl_cm: features.vtl_cm,
            latency_ms,
        })
    }
}

struct SharedFfts {
    fft_512_fwd: Arc<dyn Fft<f32>>,
    fft_512_inv: Arc<dyn Fft<f32>>,
}

fn shared_ffts() -> &'static SharedFfts {
    static CACHE: OnceLock<SharedFfts> = OnceLock::new();
    CACHE.get_or_init(|| {
        let mut planner = FftPlanner::<f32>::new();
        SharedFfts {
            fft_512_fwd: planner.plan_fft_forward(STFT_FFT_LEN),
            fft_512_inv: planner.plan_fft_inverse(STFT_FFT_LEN),
        }
    })
}

/// Extract the 14 vocal-tract acoustic features + `vtl_cm` + `formant_frames_used` from `16 kHz` PCM.
///
/// Returns `None` if `speech_16k` has too few frames (`< 8`) or lacks detectable signal energy.
#[must_use]
pub fn extract_vocal_tract_features(
    speech_16k: &[f32],
    pre_roll_noise_16k: &[f32],
    config: &AgeAiConfig,
) -> Option<VocalTractFeatures> {
    let min_samples = FRAME_LEN_16K + (MIN_FRAMES_REQUIRED - 1) * FRAME_HOP_16K;
    if speech_16k.len() < min_samples {
        return None;
    }

    let ffts = shared_ffts();
    let active_speech = select_active_window(speech_16k);
    let clean = subtract_noise_stft(active_speech, pre_roll_noise_16k, ffts);

    if clean.len() < min_samples {
        return None;
    }
    let total_energy: f32 = clean.iter().map(|&s| s * s).sum();
    if !total_energy.is_finite() || (total_energy / clean.len() as f32).sqrt() < 1e-6 {
        return None;
    }

    let num_frames = ((clean.len() - FRAME_LEN_16K) / FRAME_HOP_16K + 1).min(MAX_ANALYSIS_FRAMES);
    if num_frames < MIN_FRAMES_REQUIRED {
        return None;
    }

    // Prefix sum of squared samples enables O(1) window energy queries for YIN.
    let mut prefix_sq = Vec::with_capacity(clean.len() + 1);
    prefix_sq.push(0.0_f64);
    let mut acc_sq = 0.0_f64;
    for &s in &clean {
        let d = f64::from(s);
        acc_sq += d * d;
        prefix_sq.push(acc_sq);
    }

    let stft_fft = &ffts.fft_512_fwd;
    let yin_fwd = &ffts.fft_512_fwd;
    let yin_inv = &ffts.fft_512_inv;

    let hann_400 = make_periodic_hann(FRAME_LEN_16K);
    let mel_fb = MelFilterbank::new(SAMPLE_RATE_16K, STFT_FFT_LEN);

    let mut stft_buf = vec![Complex::new(0.0_f32, 0.0_f32); STFT_FFT_LEN];
    let mut stft_scratch = vec![Complex::new(0.0_f32, 0.0_f32); stft_fft.get_inplace_scratch_len()];
    let mut yin_buf = vec![Complex::new(0.0_f32, 0.0_f32); YIN_FFT_LEN];
    let mut yin_spec_scratch =
        vec![Complex::new(0.0_f32, 0.0_f32); yin_fwd.get_inplace_scratch_len()];
    let mut yin_inv_scratch =
        vec![Complex::new(0.0_f32, 0.0_f32); yin_inv.get_inplace_scratch_len()];

    let mut frame_metrics = Vec::with_capacity(num_frames);
    let mut prev_norm_mag = vec![0.0_f32; STFT_FFT_LEN / 2 + 1];
    let mut cur_norm_mag = vec![0.0_f32; STFT_FFT_LEN / 2 + 1];

    // Process STFT power spectra two real frames per complex FFT for 2x throughput.
    let nyq = STFT_FFT_LEN / 2;
    let mut pair_idx = 0_usize;
    while pair_idx < num_frames {
        let start_a = pair_idx * FRAME_HOP_16K;
        let frame_a = &clean[start_a..start_a + FRAME_LEN_16K];
        let has_b = pair_idx + 1 < num_frames;
        let start_b = if has_b {
            (pair_idx + 1) * FRAME_HOP_16K
        } else {
            start_a
        };
        let frame_b = &clean[start_b..start_b + FRAME_LEN_16K];

        let mut rms_sum_a = 0.0_f32;
        let mut rms_sum_b = 0.0_f32;
        for i in 0..FRAME_LEN_16K {
            let sa = frame_a[i];
            rms_sum_a += sa * sa;
            let sb = if has_b { frame_b[i] } else { 0.0 };
            rms_sum_b += sb * sb;
            let w = hann_400[i];
            stft_buf[i] = Complex::new(sa * w, sb * w);
        }
        for slot in &mut stft_buf[FRAME_LEN_16K..STFT_FFT_LEN] {
            *slot = Complex::new(0.0, 0.0);
        }
        stft_fft.process_with_scratch(&mut stft_buf, &mut stft_scratch);

        let mut power_a = [0.0_f32; STFT_FFT_LEN / 2 + 1];
        let mut power_b = [0.0_f32; STFT_FFT_LEN / 2 + 1];
        let z0 = stft_buf[0];
        power_a[0] = z0.re * z0.re;
        power_b[0] = z0.im * z0.im;
        let z_nyq = stft_buf[nyq];
        power_a[nyq] = z_nyq.re * z_nyq.re;
        power_b[nyq] = z_nyq.im * z_nyq.im;

        for k in 1..nyq {
            let zk = stft_buf[k];
            let znk_conj = stft_buf[STFT_FFT_LEN - k].conj();
            let ak = (zk + znk_conj) * 0.5;
            let bk = Complex::new(0.0, -0.5) * (zk - znk_conj);
            power_a[k] = ak.norm_sqr();
            power_b[k] = bk.norm_sqr();
        }

        let pair_frames: [(usize, f32, [f32; STFT_FFT_LEN / 2 + 1]); 2] = [
            (start_a, (rms_sum_a / FRAME_LEN_16K as f32).sqrt(), power_a),
            (start_b, (rms_sum_b / FRAME_LEN_16K as f32).sqrt(), power_b),
        ];
        let count_in_pair = if has_b { 2 } else { 1 };
        for (local_offset, &(start, frame_rms, power)) in
            pair_frames[..count_in_pair].iter().enumerate()
        {
            let cur_idx = pair_idx + local_offset;
            let mut mag_sq_sum = 0.0_f32;
            for (k, &p) in power.iter().enumerate() {
                cur_norm_mag[k] = p.sqrt();
                mag_sq_sum += p;
            }
            let mag_norm = mag_sq_sum.sqrt().max(1e-9);
            for mag in &mut cur_norm_mag {
                *mag /= mag_norm;
            }

            let spectral_flux = if cur_idx == 0 {
                0.15
            } else {
                let flux_sq: f32 = cur_norm_mag
                    .iter()
                    .zip(prev_norm_mag.iter())
                    .map(|(a, b)| {
                        let d = a - b;
                        d * d
                    })
                    .sum();
                flux_sq.sqrt()
            };
            prev_norm_mag.copy_from_slice(&cur_norm_mag);

            // Compute YIN on every even frame (20 ms grid) for ranking; odd selected frames are evaluated below.
            let yin_res = if cur_idx % 2 == 0 {
                let mut yin_buffers = YinBuffers {
                    fft_fwd: yin_fwd,
                    fft_inv: yin_inv,
                    buf: &mut yin_buf,
                    fwd_scratch: &mut yin_spec_scratch,
                    inv_scratch: &mut yin_inv_scratch,
                };
                Some(compute_frame_yin_hnr(
                    &clean,
                    &prefix_sq,
                    start,
                    &mut yin_buffers,
                ))
            } else {
                None
            };

            let (hnr_db, yin_candidates, yin_computed) = match yin_res {
                Some(r) => (r.hnr_db, r.candidates, true),
                None => (0.0, Vec::new(), false),
            };

            frame_metrics.push(FrameAnalysis {
                start_sample: start,
                frame_rms,
                spectral_flux,
                nucleus_score: 0.0,
                hnr_db,
                yin_candidates,
                yin_computed,
                power,
            });
        }

        pair_idx += 2;
    }

    // Interpolate even-frame HNR to odd frames for steady-state ranking, then compute exact ranking scores.
    for idx in 0..num_frames {
        if !frame_metrics[idx].yin_computed {
            let left = frame_metrics[idx - 1].hnr_db;
            let right = if idx + 1 < num_frames {
                frame_metrics[idx + 1].hnr_db
            } else {
                left
            };
            frame_metrics[idx].hnr_db = 0.5 * (left + right);
        }
        let fm = &mut frame_metrics[idx];
        let hnr_score = fm.hnr_db.clamp(0.05, 35.0);
        fm.nucleus_score = fm.frame_rms * hnr_score / (1.0 + fm.spectral_flux);
    }

    // Rank frames and keep the top 40% steady-state vowel nuclei (>= 8 frames).
    let mut ranked_indices: Vec<usize> = (0..num_frames).collect();
    ranked_indices.sort_by(|&a, &b| {
        frame_metrics[b]
            .nucleus_score
            .total_cmp(&frame_metrics[a].nucleus_score)
    });
    let top_k = ((num_frames * 40).div_ceil(100))
        .max(MIN_FRAMES_REQUIRED)
        .min(num_frames);
    let mut selected_indices = ranked_indices[..top_k].to_vec();
    selected_indices.sort_unstable();

    // Evaluate exact YIN pitch + HNR on any selected odd frames.
    for &idx in &selected_indices {
        if !frame_metrics[idx].yin_computed {
            let start = frame_metrics[idx].start_sample;
            let mut yin_buffers = YinBuffers {
                fft_fwd: yin_fwd,
                fft_inv: yin_inv,
                buf: &mut yin_buf,
                fwd_scratch: &mut yin_spec_scratch,
                inv_scratch: &mut yin_inv_scratch,
            };
            let r = compute_frame_yin_hnr(&clean, &prefix_sq, start, &mut yin_buffers);
            frame_metrics[idx].hnr_db = r.hnr_db;
            frame_metrics[idx].yin_candidates = r.candidates;
            frame_metrics[idx].yin_computed = true;
        }
    }

    let f0_offset = if config.f0_offset_hz.is_finite() {
        config.f0_offset_hz
    } else {
        0.0
    };
    let vtl_offset = if config.vtl_offset_cm.is_finite() {
        config.vtl_offset_cm
    } else {
        0.0
    };

    // Viterbi-smoothed F0 tracking across selected vowel-nucleus frames.
    let selected_candidates: Vec<&Vec<PitchCandidate>> = selected_indices
        .iter()
        .map(|&idx| &frame_metrics[idx].yin_candidates)
        .collect();
    let frame_f0_track = viterbi_f0_track(&selected_candidates);

    let mut f0_values: Vec<f32> = frame_f0_track
        .iter()
        .flatten()
        .map(|&f| (f + f0_offset).clamp(50.0, 600.0))
        .collect();

    if f0_values.is_empty() {
        // Fallback to raw best candidate per selected frame if strict voicing missed all frames.
        for &idx in &selected_indices {
            if let Some(best) = frame_metrics[idx].yin_candidates.first() {
                f0_values.push((best.freq_hz + f0_offset).clamp(50.0, 600.0));
            }
        }
    }
    if f0_values.is_empty() {
        f0_values.push((180.0 + f0_offset).clamp(50.0, 600.0));
    }
    f0_values.sort_by(f32::total_cmp);

    let f0_p10 = quantile_sorted(&f0_values, 0.10);
    let f0_p25 = quantile_sorted(&f0_values, 0.25);
    let f0_median = quantile_sorted(&f0_values, 0.50);
    let f0_p75 = quantile_sorted(&f0_values, 0.75);
    let f0_p90 = quantile_sorted(&f0_values, 0.90);
    let f0_iqr = (f0_p75 - f0_p25).max(0.0);

    // Precompute resampling kernels and Hamming window for 10 kHz Burg LPC formant analysis.
    let resampler = Resampler16To10::new();
    let hamming_250 = make_hamming(FRAME_LEN_10K);

    let mut f1_samples = Vec::with_capacity(selected_indices.len());
    let mut f2_samples = Vec::with_capacity(selected_indices.len());
    let mut f3_samples = Vec::with_capacity(selected_indices.len());

    let mut mfcc1_sum = 0.0_f32;
    let mut mfcc2_sum = 0.0_f32;
    let mut mfcc3_sum = 0.0_f32;
    let mut centroid_sum = 0.0_f32;
    let mut tilt_sum = 0.0_f32;
    let mut hnr_sum = 0.0_f32;

    let mut frame_10k = [0.0_f32; FRAME_LEN_10K];
    let mut frame_lpc_in = [0.0_f64; FRAME_LEN_10K];

    for (pos, &frame_idx) in selected_indices.iter().enumerate() {
        let fm = &frame_metrics[frame_idx];
        let spec = mel_fb.extract_spectral_features(&fm.power);
        mfcc1_sum += spec.mfcc_1;
        mfcc2_sum += spec.mfcc_2;
        mfcc3_sum += spec.mfcc_3;
        centroid_sum += spec.centroid_hz;
        tilt_sum += spec.tilt_db;
        hnr_sum += fm.hnr_db;

        let local_f0 = frame_f0_track[pos].unwrap_or(f0_median);
        resampler.resample_frame(&clean, fm.start_sample, &mut frame_10k);
        apply_pre_emphasis_and_window(&frame_10k, &hamming_250, &mut frame_lpc_in);

        let formant_opt = if local_f0 > HIGH_PITCH_LPC_THRESHOLD_HZ {
            extract_formants_single_order(&frame_lpc_in, 10, &fm.power)
        } else {
            extract_formants_consensus(&frame_lpc_in, &fm.power)
        };

        if let Some([f1, f2, f3]) = formant_opt {
            f1_samples.push(f1);
            f2_samples.push(f2);
            f3_samples.push(f3);
        }
    }

    let inv_selected = 1.0 / selected_indices.len() as f32;
    let mfcc_1 = mfcc1_sum * inv_selected;
    let mfcc_2 = mfcc2_sum * inv_selected;
    let mfcc_3 = mfcc3_sum * inv_selected;
    let spectral_centroid_hz = centroid_sum * inv_selected;
    let spectral_tilt_db = tilt_sum * inv_selected;
    let hnr_db = hnr_sum * inv_selected;

    let formant_frames_used = f1_samples.len();
    let (f1_hz, f2_hz, f3_hz, formant_dispersion_hz, vtl_cm) =
        if formant_frames_used >= MIN_VALID_FORMANT_FRAMES {
            let f1 = median_f32(&mut f1_samples);
            let f2 = median_f32(&mut f2_samples);
            let f3 = median_f32(&mut f3_samples);
            let delta_f = (0.5 * f1 + 1.5 * f2 + 2.5 * f3) / FITCH_DISPERSION_DENOM;
            let vtl = if delta_f > 100.0 {
                (SPEED_OF_SOUND_CM_PER_S / (2.0 * delta_f)) + vtl_offset
            } else {
                0.0
            };
            (f1, f2, f3, delta_f, vtl.max(0.0))
        } else {
            (0.0, 0.0, 0.0, 0.0, 0.0)
        };

    Some(VocalTractFeatures {
        f0_median,
        f0_iqr,
        f0_p10,
        f0_p90,
        f1_hz,
        f2_hz,
        f3_hz,
        formant_dispersion_hz,
        vtl_cm,
        formant_frames_used,
        mfcc_1,
        mfcc_2,
        mfcc_3,
        spectral_centroid_hz,
        spectral_tilt_db,
        hnr_db,
    })
}

// ============================================================================
// Stage 1: Active Windowing & STFT Pre-Roll Spectral Noise Subtraction
// ============================================================================

fn select_active_window(speech_16k: &[f32]) -> &[f32] {
    if speech_16k.len() <= MAX_ANALYSIS_SAMPLES_16K {
        return speech_16k;
    }

    // Select the highest-energy contiguous 250-frame segment when audio exceeds 2.515 s.
    let block = FRAME_HOP_16K;
    let blocks_in_win = MAX_ANALYSIS_SAMPLES_16K / block;
    let total_blocks = speech_16k.len() / block;
    if total_blocks <= blocks_in_win {
        return &speech_16k[..MAX_ANALYSIS_SAMPLES_16K];
    }

    let mut block_energy = Vec::with_capacity(total_blocks);
    for b in 0..total_blocks {
        let slice = &speech_16k[b * block..(b + 1) * block];
        let e: f32 = slice
            .iter()
            .map(|&s| if s.is_finite() { s * s } else { 0.0 })
            .sum();
        block_energy.push(e);
    }

    let mut window_sum: f32 = block_energy[..blocks_in_win].iter().sum();
    let mut best_sum = window_sum;
    let mut best_start_block = 0;

    for start_b in 1..=(total_blocks - blocks_in_win) {
        window_sum += block_energy[start_b + blocks_in_win - 1] - block_energy[start_b - 1];
        if window_sum > best_sum {
            best_sum = window_sum;
            best_start_block = start_b;
        }
    }

    let start_idx = best_start_block * block;
    &speech_16k[start_idx..start_idx + MAX_ANALYSIS_SAMPLES_16K]
}

fn subtract_noise_stft(
    speech_16k: &[f32],
    pre_roll_noise_16k: &[f32],
    ffts: &SharedFfts,
) -> Vec<f32> {
    let mut clean: Vec<f32> = speech_16k
        .iter()
        .map(|&s| {
            if s.is_finite() {
                s.clamp(-4.0, 4.0)
            } else {
                0.0
            }
        })
        .collect();

    if pre_roll_noise_16k.len() < 16 {
        return clean;
    }

    let noise_energy: f32 = pre_roll_noise_16k
        .iter()
        .map(|&s| if s.is_finite() { s * s } else { 0.0 })
        .sum();
    let noise_rms = if noise_energy.is_finite() {
        (noise_energy / pre_roll_noise_16k.len() as f32).sqrt()
    } else {
        0.0
    };
    let speech_rms = (clean.iter().map(|&s| s * s).sum::<f32>() / clean.len().max(1) as f32).sqrt();
    if noise_rms < 1e-6 || (speech_rms > 0.0 && noise_rms < 0.04 * speech_rms) {
        return clean;
    }

    let hann = make_periodic_hann(STFT_FFT_LEN);
    let sqrt_hann: Vec<f32> = hann.iter().map(|&w| w.sqrt()).collect();
    let full_w_sq: f32 = hann.iter().sum();

    let fft_fwd = &ffts.fft_512_fwd;
    let fft_inv = &ffts.fft_512_inv;
    let mut buf = vec![Complex::new(0.0_f32, 0.0_f32); STFT_FFT_LEN];
    let mut fwd_scratch = vec![
        Complex::new(0.0_f32, 0.0_f32);
        fft_fwd
            .get_inplace_scratch_len()
            .max(fft_inv.get_inplace_scratch_len())
    ];

    // Estimate ambient noise power spectrum from pre-roll frames.
    let mut noise_pow = [0.0_f32; STFT_FFT_LEN / 2 + 1];
    let mut noise_frames = 0_usize;

    if pre_roll_noise_16k.len() < STFT_FFT_LEN {
        let mut used_w_sq = 0.0_f32;
        for (i, (&s, &w)) in pre_roll_noise_16k.iter().zip(sqrt_hann.iter()).enumerate() {
            let v = if s.is_finite() {
                s.clamp(-4.0, 4.0)
            } else {
                0.0
            };
            buf[i] = Complex::new(v * w, 0.0);
            used_w_sq += w * w;
        }
        for slot in &mut buf[pre_roll_noise_16k.len()..STFT_FFT_LEN] {
            *slot = Complex::new(0.0, 0.0);
        }
        fft_fwd.process_with_scratch(&mut buf, &mut fwd_scratch);
        let scale = if used_w_sq > 1e-6 {
            full_w_sq / used_w_sq
        } else {
            1.0
        };
        for (k, c) in buf.iter().take(STFT_FFT_LEN / 2 + 1).enumerate() {
            noise_pow[k] = c.norm_sqr() * scale;
        }
        noise_frames = 1;
    } else {
        let mut offset = 0;
        while offset + STFT_FFT_LEN <= pre_roll_noise_16k.len() {
            for (i, (&s, &w)) in pre_roll_noise_16k[offset..offset + STFT_FFT_LEN]
                .iter()
                .zip(sqrt_hann.iter())
                .enumerate()
            {
                let v = if s.is_finite() {
                    s.clamp(-4.0, 4.0)
                } else {
                    0.0
                };
                buf[i] = Complex::new(v * w, 0.0);
            }
            fft_fwd.process_with_scratch(&mut buf, &mut fwd_scratch);
            for (k, c) in buf.iter().take(STFT_FFT_LEN / 2 + 1).enumerate() {
                noise_pow[k] += c.norm_sqr();
            }
            noise_frames += 1;
            offset += STFT_HOP_LEN;
        }
    }

    if noise_frames == 0 {
        return clean;
    }
    let inv_nf = 1.0 / noise_frames as f32;
    for np in &mut noise_pow {
        *np *= inv_nf;
    }
    let smooth_noise = smooth_11bin(&noise_pow);

    // Pad both ends by STFT_HOP_LEN so every sample in clean experiences exact COLA = 1.0.
    let pad_left = STFT_HOP_LEN;
    let total_needed = pad_left + clean.len() + STFT_FFT_LEN;
    let num_hops = (total_needed - STFT_FFT_LEN).div_ceil(STFT_HOP_LEN) + 1;
    let padded_len = STFT_FFT_LEN + (num_hops - 1) * STFT_HOP_LEN;

    let mut padded = vec![0.0_f32; padded_len];
    padded[pad_left..pad_left + clean.len()].copy_from_slice(&clean);
    let mut out = vec![0.0_f32; padded_len];

    let nyq = STFT_FFT_LEN / 2;
    let inv_fft_len = 1.0 / STFT_FFT_LEN as f32;
    let mut raw_pow_a = [0.0_f32; STFT_FFT_LEN / 2 + 1];
    let mut raw_pow_b = [0.0_f32; STFT_FFT_LEN / 2 + 1];
    let mut spec_a = [Complex::new(0.0_f32, 0.0_f32); STFT_FFT_LEN / 2 + 1];
    let mut spec_b = [Complex::new(0.0_f32, 0.0_f32); STFT_FFT_LEN / 2 + 1];

    // Pack two STFT frames per complex FFT/IFFT pair to halve FFT cost while keeping smooth spectral envelope gain.
    let mut hop_idx = 0_usize;
    while hop_idx < num_hops {
        let off_a = hop_idx * STFT_HOP_LEN;
        let has_b = hop_idx + 1 < num_hops;
        let off_b = if has_b {
            (hop_idx + 1) * STFT_HOP_LEN
        } else {
            off_a
        };

        for (i, &w) in sqrt_hann.iter().enumerate() {
            let sa = padded[off_a + i];
            let sb = if has_b { padded[off_b + i] } else { 0.0 };
            buf[i] = Complex::new(sa * w, sb * w);
        }
        fft_fwd.process_with_scratch(&mut buf, &mut fwd_scratch);

        let z0 = buf[0];
        spec_a[0] = Complex::new(z0.re, 0.0);
        spec_b[0] = Complex::new(z0.im, 0.0);
        raw_pow_a[0] = z0.re * z0.re;
        raw_pow_b[0] = z0.im * z0.im;

        let z_nyq = buf[nyq];
        spec_a[nyq] = Complex::new(z_nyq.re, 0.0);
        spec_b[nyq] = Complex::new(z_nyq.im, 0.0);
        raw_pow_a[nyq] = z_nyq.re * z_nyq.re;
        raw_pow_b[nyq] = z_nyq.im * z_nyq.im;

        for k in 1..nyq {
            let zk = buf[k];
            let znk_conj = buf[STFT_FFT_LEN - k].conj();
            let ak = (zk + znk_conj) * 0.5;
            let bk = Complex::new(0.0, -0.5) * (zk - znk_conj);
            spec_a[k] = ak;
            spec_b[k] = bk;
            raw_pow_a[k] = ak.norm_sqr();
            raw_pow_b[k] = bk.norm_sqr();
        }

        let env_a = smooth_11bin(&raw_pow_a);
        let env_b = smooth_11bin(&raw_pow_b);

        let ga0 = envelope_gain(env_a[0], smooth_noise[0]);
        let gb0 = envelope_gain(env_b[0], smooth_noise[0]);
        buf[0] = Complex::new(spec_a[0].re * ga0, spec_b[0].re * gb0);

        let ganyq = envelope_gain(env_a[nyq], smooth_noise[nyq]);
        let gbnyq = envelope_gain(env_b[nyq], smooth_noise[nyq]);
        buf[nyq] = Complex::new(spec_a[nyq].re * ganyq, spec_b[nyq].re * gbnyq);

        for k in 1..nyq {
            let ga = envelope_gain(env_a[k], smooth_noise[k]);
            let gb = envelope_gain(env_b[k], smooth_noise[k]);
            let ac = spec_a[k] * ga;
            let bc = spec_b[k] * gb;
            buf[k] = ac + Complex::new(-bc.im, bc.re);
            buf[STFT_FFT_LEN - k] = ac.conj() + Complex::new(bc.im, bc.re);
        }

        fft_inv.process_with_scratch(&mut buf, &mut fwd_scratch);
        for (i, (c, &w)) in buf.iter().zip(sqrt_hann.iter()).enumerate() {
            out[off_a + i] += c.re * inv_fft_len * w;
            if has_b {
                out[off_b + i] += c.im * inv_fft_len * w;
            }
        }

        hop_idx += 2;
    }

    let clean_len = clean.len();
    clean.copy_from_slice(&out[pad_left..pad_left + clean_len]);
    clean
}

fn smooth_11bin(power: &[f32; STFT_FFT_LEN / 2 + 1]) -> [f32; STFT_FFT_LEN / 2 + 1] {
    // 11-bin (~344 Hz) triangular spectral envelope smoother spans > 1 pitch harmonic up to 340 Hz.
    const WEIGHTS: [f32; 11] = [
        1.0 / 36.0,
        2.0 / 36.0,
        3.0 / 36.0,
        4.0 / 36.0,
        5.0 / 36.0,
        6.0 / 36.0,
        5.0 / 36.0,
        4.0 / 36.0,
        3.0 / 36.0,
        2.0 / 36.0,
        1.0 / 36.0,
    ];
    let mut out = [0.0_f32; STFT_FFT_LEN / 2 + 1];
    let last = (power.len() - 1) as isize;
    for (k, slot) in out.iter_mut().enumerate() {
        let ki = k as isize;
        let mut acc = 0.0_f32;
        for (offset, &w) in (-5_isize..=5_isize).zip(WEIGHTS.iter()) {
            let idx = (ki + offset).clamp(0, last) as usize;
            acc += w * power[idx];
        }
        *slot = acc;
    }
    out
}

fn envelope_gain(env_pow: f32, noise_pow: f32) -> f32 {
    let clean_pow = (env_pow - NOISE_OVERSUB_ALPHA * noise_pow).max(NOISE_FLOOR_BETA * env_pow);
    (clean_pow / (env_pow + 1e-12))
        .sqrt()
        .clamp(NOISE_MIN_GAIN, 1.0)
}

// ============================================================================
// Stage 2A: YIN/pYIN CMNDF via FFT Cross-Correlation + Viterbi Pitch Tracking
// ============================================================================

#[derive(Debug, Clone, Copy)]
struct PitchCandidate {
    freq_hz: f32,
    weight: f32,
}

struct FrameAnalysis {
    start_sample: usize,
    frame_rms: f32,
    spectral_flux: f32,
    nucleus_score: f32,
    hnr_db: f32,
    yin_candidates: Vec<PitchCandidate>,
    yin_computed: bool,
    power: [f32; STFT_FFT_LEN / 2 + 1],
}

struct YinFrameResult {
    hnr_db: f32,
    candidates: Vec<PitchCandidate>,
}

struct YinBuffers<'a> {
    fft_fwd: &'a Arc<dyn Fft<f32>>,
    fft_inv: &'a Arc<dyn Fft<f32>>,
    buf: &'a mut [Complex<f32>],
    fwd_scratch: &'a mut [Complex<f32>],
    inv_scratch: &'a mut [Complex<f32>],
}

fn compute_frame_yin_hnr(
    samples: &[f32],
    prefix_sq: &[f64],
    frame_start: usize,
    yin: &mut YinBuffers<'_>,
) -> YinFrameResult {
    let needed = YIN_WINDOW_SAMPLES + YIN_TAU_MAX;
    let start = if frame_start + needed <= samples.len() {
        frame_start
    } else {
        samples.len().saturating_sub(needed)
    };
    let w_len = YIN_WINDOW_SAMPLES.min(samples.len().saturating_sub(start + 1));
    let tau_limit = YIN_TAU_MAX.min(samples.len().saturating_sub(start + w_len));

    if w_len < 64 || tau_limit <= YIN_TAU_MIN {
        return YinFrameResult {
            hnr_db: 0.0,
            candidates: Vec::new(),
        };
    }

    // Pack a (template length W) and b (search window length W + tau_limit) into one complex FFT.
    for (i, slot) in yin.buf.iter_mut().enumerate() {
        let a = if i < w_len { samples[start + i] } else { 0.0 };
        let b = if i < w_len + tau_limit {
            samples[start + i]
        } else {
            0.0
        };
        *slot = Complex::new(a, b);
    }

    yin.fft_fwd.process_with_scratch(yin.buf, yin.fwd_scratch);

    // Extract C[k] = conj(A[k]) * B[k] from packed Z[k] = A[k] + i*B[k].
    let z0 = yin.buf[0];
    let c0 = Complex::new(z0.re * z0.im, 0.0);
    let nyq = YIN_FFT_LEN / 2;
    let z_nyq = yin.buf[nyq];
    let c_nyq = Complex::new(z_nyq.re * z_nyq.im, 0.0);

    for k in 1..nyq {
        let zk = yin.buf[k];
        let znk_conj = yin.buf[YIN_FFT_LEN - k].conj();
        let ak = (zk + znk_conj) * 0.5;
        let bk = Complex::new(0.0, -0.5) * (zk - znk_conj);
        let ck = ak.conj() * bk;
        let cnk = ck.conj();
        yin.buf[k] = ck;
        yin.buf[YIN_FFT_LEN - k] = cnk;
    }
    yin.buf[0] = c0;
    yin.buf[nyq] = c_nyq;

    yin.fft_inv.process_with_scratch(yin.buf, yin.inv_scratch);

    let inv_n = 1.0_f64 / YIN_FFT_LEN as f64;
    let e0 = prefix_sq[start + w_len] - prefix_sq[start];

    let mut diff = [0.0_f64; YIN_TAU_MAX + 1];
    let mut cmndf = [1.0_f64; YIN_TAU_MAX + 1];
    let mut running_sum = 0.0_f64;
    let mut r_max = 0.01_f64;

    for tau in 1..=tau_limit {
        let r_tau = f64::from(yin.buf[tau].re) * inv_n;
        let e_tau = prefix_sq[start + tau + w_len] - prefix_sq[start + tau];
        let d = (e0 + e_tau - 2.0 * r_tau).max(0.0);
        diff[tau] = d;
        running_sum += d;
        cmndf[tau] = if running_sum > 1e-12 {
            d * (tau as f64) / running_sum
        } else {
            1.0
        };

        if (YIN_TAU_MIN..=tau_limit).contains(&tau) {
            let denom = (e0 * e_tau).sqrt();
            if denom > 1e-12 {
                let norm_r = (r_tau / denom).clamp(-1.0, 0.9995);
                if norm_r > r_max {
                    r_max = norm_r;
                }
            }
        }
    }

    let hnr_db = (10.0 * (r_max / (1.0 - r_max)).log10()) as f32;

    // Collect local CMNDF minima (< 0.45) as pYIN pitch candidates with parabolic sub-sample refinement.
    let mut candidates = Vec::with_capacity(6);
    let max_tau_check = tau_limit.min(YIN_TAU_MAX);
    let first_tau = YIN_TAU_MIN.max(2);
    let last_tau = max_tau_check.saturating_sub(1);

    if last_tau > first_tau {
        for tau in first_tau..=last_tau {
            let v = cmndf[tau];
            if v < 0.45 && v <= cmndf[tau - 1] && v <= cmndf[tau + 1] {
                let refined_tau = parabolic_min_offset(cmndf[tau - 1], v, cmndf[tau + 1], tau);
                let freq = SAMPLE_RATE_16K / refined_tau as f32;
                if (F0_MIN_HZ..=F0_MAX_HZ).contains(&freq) {
                    // Prioritize deep dips and the fundamental (first deep dip below 0.16) to resist subharmonics.
                    let dip_quality = ((1.0 - v).clamp(0.0, 1.0) as f32).powi(3);
                    let fundamental_bonus = if v < 0.16 && candidates.is_empty() {
                        1.45
                    } else {
                        1.0
                    };
                    candidates.push(PitchCandidate {
                        freq_hz: freq,
                        weight: dip_quality * fundamental_bonus,
                    });
                }
            }
        }
    }

    // Order strongest candidates first; if none < 0.45, retain global minimum if < 0.65.
    if candidates.is_empty() && last_tau > first_tau {
        let mut best_tau = first_tau;
        let mut best_val = cmndf[first_tau];
        for (offset, &val) in cmndf[first_tau..=last_tau].iter().enumerate() {
            if val < best_val {
                best_val = val;
                best_tau = first_tau + offset;
            }
        }
        if best_val < 0.65 {
            let refined_tau =
                parabolic_min_offset(cmndf[best_tau - 1], best_val, cmndf[best_tau + 1], best_tau);
            let freq = SAMPLE_RATE_16K / refined_tau as f32;
            if (F0_MIN_HZ..=F0_MAX_HZ).contains(&freq) {
                candidates.push(PitchCandidate {
                    freq_hz: freq,
                    weight: ((1.0 - best_val).clamp(0.05, 1.0) as f32).powi(2),
                });
            }
        }
    } else {
        candidates.sort_by(|a, b| b.weight.total_cmp(&a.weight));
        candidates.truncate(4);
    }

    YinFrameResult {
        hnr_db: hnr_db.clamp(-10.0, 35.0),
        candidates,
    }
}

fn parabolic_min_offset(left: f64, center: f64, right: f64, idx: usize) -> f64 {
    let denom = 2.0 * (left - 2.0 * center + right);
    let delta = if denom.abs() > 1e-12 {
        ((left - right) / denom).clamp(-0.5, 0.5)
    } else {
        0.0
    };
    idx as f64 + delta
}

fn viterbi_f0_track(frames: &[&Vec<PitchCandidate>]) -> Vec<Option<f32>> {
    if frames.is_empty() {
        return Vec::new();
    }

    let mut dp_scores: Vec<Vec<f32>> = Vec::with_capacity(frames.len());
    let mut backptrs: Vec<Vec<usize>> = Vec::with_capacity(frames.len());
    let mut active_indices: Vec<usize> = Vec::with_capacity(frames.len());

    for (t, cands) in frames.iter().enumerate() {
        if cands.is_empty() {
            continue;
        }
        let mut scores = vec![f32::NEG_INFINITY; cands.len()];
        let mut ptrs = vec![0_usize; cands.len()];

        if active_indices.is_empty() {
            for (k, c) in cands.iter().enumerate() {
                scores[k] = (c.weight.max(1e-4)).ln();
            }
        } else {
            let prev_pos = dp_scores.len() - 1;
            let prev_t = active_indices[prev_pos];
            let prev_cands = frames[prev_t];
            let prev_scores = &dp_scores[prev_pos];
            let gap_scale = ((t - prev_t) as f32).sqrt().max(1.0);

            for (k, c) in cands.iter().enumerate() {
                let emit = (c.weight.max(1e-4)).ln();
                let mut best_val = f32::NEG_INFINITY;
                let mut best_j = 0_usize;
                for (j, pc) in prev_cands.iter().enumerate() {
                    let oct_jump = (c.freq_hz / pc.freq_hz).log2().abs();
                    let trans_penalty = 3.5 * oct_jump / gap_scale;
                    let total = prev_scores[j] - trans_penalty + emit;
                    if total > best_val {
                        best_val = total;
                        best_j = j;
                    }
                }
                scores[k] = best_val;
                ptrs[k] = best_j;
            }
        }

        dp_scores.push(scores);
        backptrs.push(ptrs);
        active_indices.push(t);
    }

    let mut result = vec![None; frames.len()];
    if active_indices.is_empty() {
        return result;
    }

    let last_pos = dp_scores.len() - 1;
    let mut best_k = 0_usize;
    let mut best_score = f32::NEG_INFINITY;
    for (k, &s) in dp_scores[last_pos].iter().enumerate() {
        if s > best_score {
            best_score = s;
            best_k = k;
        }
    }

    for pos in (0..=last_pos).rev() {
        let t = active_indices[pos];
        result[t] = Some(frames[t][best_k].freq_hz);
        if pos > 0 {
            best_k = backptrs[pos][best_k];
        }
    }

    result
}

// ============================================================================
// Stage 2B: 16 kHz -> 10 kHz Windowed-Sinc Resampler + Multi-Order Burg LPC
// ============================================================================

struct Resampler16To10 {
    kernels: [[f32; RESAMPLE_TAPS]; 5],
}

impl Resampler16To10 {
    fn new() -> Self {
        // Anti-aliasing low-pass cutoff at 4.75 kHz (Nyquist of 10 kHz is 5.0 kHz).
        let cutoff_ratio = 4750.0_f64 / 8000.0_f64;
        let half = RESAMPLE_HALF_TAPS as i32;
        let mut kernels = [[0.0_f32; RESAMPLE_TAPS]; 5];

        for (phase_idx, kernel) in kernels.iter_mut().enumerate() {
            // Exact fractional positions for m * 1.6 mod 1.0: {0.0, 0.6, 0.2, 0.8, 0.4}.
            let frac = ((phase_idx * 3) % 5) as f64 * 0.2;
            let mut sum = 0.0_f64;
            let mut raw = [0.0_f64; RESAMPLE_TAPS];

            for (tap_idx, k_offset) in (-half..=half).enumerate() {
                let u = (k_offset as f64) - frac;
                let sinc = if u.abs() < 1e-9 {
                    cutoff_ratio
                } else {
                    (PI64 * cutoff_ratio * u).sin() / (PI64 * u)
                };
                // Blackman window over [-RESAMPLE_HALF_TAPS - 0.5, +RESAMPLE_HALF_TAPS + 0.5].
                let win_pos =
                    (u + (RESAMPLE_HALF_TAPS as f64) + 0.5) / ((2 * RESAMPLE_HALF_TAPS + 1) as f64);
                let win = if (0.0..=1.0).contains(&win_pos) {
                    0.42 - 0.50 * (2.0 * PI64 * win_pos).cos() + 0.08 * (4.0 * PI64 * win_pos).cos()
                } else {
                    0.0
                };
                let val = sinc * win;
                raw[tap_idx] = val;
                sum += val;
            }

            let inv_sum = if sum.abs() > 1e-12 { 1.0 / sum } else { 1.0 };
            for (dst, &src) in kernel.iter_mut().zip(raw.iter()) {
                *dst = (src * inv_sum) as f32;
            }
        }

        Self { kernels }
    }

    fn resample_frame(
        &self,
        signal_16k: &[f32],
        frame_start: usize,
        out_10k: &mut [f32; FRAME_LEN_10K],
    ) {
        let len_isize = signal_16k.len() as isize;
        let half = RESAMPLE_HALF_TAPS as isize;

        for (m, out) in out_10k.iter_mut().enumerate() {
            // m * 1.6 = (m * 8) / 5
            let num = m * 8;
            let int_offset = (num / 5) as isize;
            let phase_idx = m % 5;
            let kernel = &self.kernels[phase_idx];
            let base = (frame_start as isize) + int_offset;

            let mut acc = 0.0_f32;
            for (tap_idx, &coeff) in kernel.iter().enumerate() {
                let idx = (base + (tap_idx as isize) - half).clamp(0, len_isize - 1) as usize;
                acc += signal_16k[idx] * coeff;
            }
            *out = acc;
        }
    }
}

fn apply_pre_emphasis_and_window(
    frame_10k: &[f32; FRAME_LEN_10K],
    hamming: &[f32],
    out_lpc: &mut [f64; FRAME_LEN_10K],
) {
    out_lpc[0] = f64::from(frame_10k[0] * (1.0 - PRE_EMPHASIS_COEFF) * hamming[0]);
    for i in 1..FRAME_LEN_10K {
        let pre = frame_10k[i] - PRE_EMPHASIS_COEFF * frame_10k[i - 1];
        out_lpc[i] = f64::from(pre * hamming[i]);
    }
}

struct BurgMultiOrder {
    order10: [f64; 11],
    order12: [f64; 13],
    order14: [f64; 15],
    max_order_reached: usize,
}

fn burg_lpc_up_to_14(frame: &[f64; FRAME_LEN_10K], max_order: usize) -> BurgMultiOrder {
    let n = frame.len();
    let mut ef = *frame;
    let mut eb = *frame;
    let mut a = [0.0_f64; MAX_LPC_ORDER + 1];
    a[0] = 1.0;

    let mut res = BurgMultiOrder {
        order10: [0.0; 11],
        order12: [0.0; 13],
        order14: [0.0; 15],
        max_order_reached: 0,
    };
    res.order10[0] = 1.0;
    res.order12[0] = 1.0;
    res.order14[0] = 1.0;

    let initial_den: f64 = 2.0 * frame.iter().map(|&v| v * v).sum::<f64>();
    if !initial_den.is_finite() || initial_den <= 1e-18 {
        return res;
    }

    let target = max_order.min(MAX_LPC_ORDER);
    for m in 1..=target {
        let mut num = 0.0_f64;
        let mut den = 0.0_f64;
        for idx in m..n {
            let f = ef[idx];
            let b = eb[idx - 1];
            num += f * b;
            den += f * f + b * b;
        }
        // Stop cleanly once prediction residual vanishes (e.g., pure 1- or 2-tone sinusoid).
        if !den.is_finite() || den <= 1e-18 || den <= 5e-4 * initial_den {
            break;
        }
        let k_m = (-2.0 * num / den).clamp(-0.9999, 0.9999);

        for j in 1..=(m / 2) {
            let aj = a[j];
            let amj = a[m - j];
            if j == m - j {
                a[j] = aj + k_m * aj;
            } else {
                a[j] = aj + k_m * amj;
                a[m - j] = amj + k_m * aj;
            }
        }
        a[m] = k_m;

        for idx in (m..n).rev() {
            let f_prev = ef[idx];
            let b_prev = eb[idx - 1];
            ef[idx] = f_prev + k_m * b_prev;
            eb[idx] = b_prev + k_m * f_prev;
        }

        res.max_order_reached = m;
        if m == 10 {
            res.order10.copy_from_slice(&a[..11]);
        } else if m == 12 {
            res.order12.copy_from_slice(&a[..13]);
        } else if m == 14 {
            res.order14.copy_from_slice(&a[..15]);
        }
    }

    res
}

fn extract_formants_single_order(
    frame: &[f64; FRAME_LEN_10K],
    order: usize,
    power_16k: &[f32; STFT_FFT_LEN / 2 + 1],
) -> Option<[f32; 3]> {
    let burg = burg_lpc_up_to_14(frame, order);
    if burg.max_order_reached < order {
        return None;
    }
    match order {
        10 => extract_formants_from_lpc(&burg.order10, Some(power_16k)),
        12 => extract_formants_from_lpc(&burg.order12, Some(power_16k)),
        14 => extract_formants_from_lpc(&burg.order14, Some(power_16k)),
        _ => None,
    }
}

fn extract_formants_consensus(
    frame: &[f64; FRAME_LEN_10K],
    power_16k: &[f32; STFT_FFT_LEN / 2 + 1],
) -> Option<[f32; 3]> {
    let burg = burg_lpc_up_to_14(frame, 14);
    let mut f1s = [0.0_f32; 3];
    let mut f2s = [0.0_f32; 3];
    let mut f3s = [0.0_f32; 3];
    let mut count = 0_usize;

    let candidates: [(usize, &[f64]); 3] = [
        (10, &burg.order10),
        (12, &burg.order12),
        (14, &burg.order14),
    ];

    for (ord, coeffs) in candidates {
        if burg.max_order_reached >= ord
            && let Some([f1, f2, f3]) = extract_formants_from_lpc(coeffs, Some(power_16k))
        {
            f1s[count] = f1;
            f2s[count] = f2;
            f3s[count] = f3;
            count += 1;
        }
    }

    if count < 2 {
        return None;
    }
    Some([
        median_f32(&mut f1s[..count]),
        median_f32(&mut f2s[..count]),
        median_f32(&mut f3s[..count]),
    ])
}

/// Find all roots of monic LP polynomial `A(z) = z^P + a[1] z^(P-1) + ... + a[P]`
/// via Aberth-Ehrlich iteration and extract the lowest 3 valid vocal-tract formants.
fn extract_formants_from_lpc(
    monic_coeffs: &[f64],
    power_16k: Option<&[f32; STFT_FFT_LEN / 2 + 1]>,
) -> Option<[f32; 3]> {
    let degree = monic_coeffs.len().saturating_sub(1);
    if degree == 0 || degree > MAX_LPC_ORDER {
        return None;
    }
    let roots = aberth_ehrlich_roots(monic_coeffs);
    let mut poles = [0.0_f32; MAX_LPC_ORDER];
    let mut pole_count = 0_usize;

    let max_power = power_16k
        .map(|p| p.iter().copied().fold(0.0_f32, f32::max))
        .unwrap_or(0.0);
    let bin_hz = SAMPLE_RATE_16K / STFT_FFT_LEN as f32;

    for &(zr, zi) in &roots[..degree] {
        if !zr.is_finite() || !zi.is_finite() || zi <= 1e-6 {
            continue;
        }
        let r = zr.hypot(zi);
        if r <= 0.30 || r >= 1.0 {
            continue;
        }
        let theta = zi.atan2(zr);
        let freq_hz = (theta * f64::from(SAMPLE_RATE_10K) / (2.0 * PI64)) as f32;
        if !(POLE_MIN_HZ..=POLE_MAX_HZ).contains(&freq_hz) {
            continue;
        }
        let bandwidth_hz = (-r.ln() * f64::from(SAMPLE_RATE_10K) / PI64) as f32;
        let max_bw = if freq_hz < POLE_BW_SPLIT_HZ {
            POLE_MAX_BW_LOW_HZ
        } else {
            POLE_MAX_BW_HIGH_HZ
        };
        if bandwidth_hz <= 0.0 || bandwidth_hz >= max_bw {
            continue;
        }

        // Verify the pole corresponds to real acoustic energy (suppresses pure-tone leakage poles).
        if let Some(power) = power_16k
            && max_power > 0.0
        {
            let center_bin = (freq_hz / bin_hz).round() as usize;
            let lo = center_bin.saturating_sub(6).max(1);
            let hi = (center_bin + 6).min(power.len() - 1);
            let local_peak = power[lo..=hi].iter().copied().fold(0.0_f32, f32::max);
            if local_peak < max_power * 1e-5 {
                continue;
            }
        }

        poles[pole_count] = freq_hz;
        pole_count += 1;
    }

    if pole_count < MIN_POLES_PER_FRAME {
        return None;
    }
    let valid_poles = &mut poles[..pole_count];
    valid_poles.sort_by(f32::total_cmp);
    Some([valid_poles[0], valid_poles[1], valid_poles[2]])
}

fn aberth_ehrlich_roots(monic_coeffs: &[f64]) -> [(f64, f64); MAX_LPC_ORDER] {
    let mut roots = [(0.0_f64, 0.0_f64); MAX_LPC_ORDER];
    let degree = monic_coeffs.len().saturating_sub(1).min(MAX_LPC_ORDER);
    if degree == 0 {
        return roots;
    }

    // Asymmetric circle initialization near the unit circle breaks real-axis & conjugate traps.
    for (k, slot) in roots.iter_mut().take(degree).enumerate() {
        let angle = 2.0 * PI64 * (k as f64) / (degree as f64) + 0.137;
        let radius = 0.86 + 0.04 * ((k % 3) as f64);
        let (sin_a, cos_a) = angle.sin_cos();
        *slot = (radius * cos_a, radius * sin_a);
    }

    const MAX_ITERS: usize = 32;
    for _ in 0..MAX_ITERS {
        let mut max_step_sq = 0.0_f64;
        for i in 0..degree {
            let (zr, zi) = roots[i];
            let (pr, pi, dpr, dpi) = eval_monic_horner(monic_coeffs, zr, zi);
            let p_sq = pr * pr + pi * pi;
            if p_sq < 1e-24 {
                continue;
            }
            let dp_sq = dpr * dpr + dpi * dpi;
            if dp_sq < 1e-28 {
                roots[i] = (zr + 1e-5, zi + 1e-5);
                continue;
            }
            // w = p / dp
            let wr = (pr * dpr + pi * dpi) / dp_sq;
            let wi = (pi * dpr - pr * dpi) / dp_sq;

            let mut sr = 0.0_f64;
            let mut si = 0.0_f64;
            for (j, &(zjr, zji)) in roots.iter().take(degree).enumerate() {
                if j != i {
                    let dr = zr - zjr;
                    let di = zi - zji;
                    let d_sq = dr * dr + di * di;
                    if d_sq > 1e-24 {
                        sr += dr / d_sq;
                        si -= di / d_sq;
                    }
                }
            }

            // denom = 1 - w * s
            let den_r = 1.0 - (wr * sr - wi * si);
            let den_i = -(wr * si + wi * sr);
            let den_sq = den_r * den_r + den_i * den_i;
            let (delta_r, delta_i) = if den_sq > 1e-24 {
                (
                    (wr * den_r + wi * den_i) / den_sq,
                    (wi * den_r - wr * den_i) / den_sq,
                )
            } else {
                (wr * 0.5, wi * 0.5)
            };

            if delta_r.is_finite() && delta_i.is_finite() {
                roots[i] = (zr - delta_r, zi - delta_i);
                let step_sq = delta_r * delta_r + delta_i * delta_i;
                if step_sq > max_step_sq {
                    max_step_sq = step_sq;
                }
            }
        }
        if max_step_sq < 1e-16 {
            break;
        }
    }

    roots
}

fn eval_monic_horner(monic_coeffs: &[f64], zr: f64, zi: f64) -> (f64, f64, f64, f64) {
    let mut pr = 1.0_f64;
    let mut pi = 0.0_f64;
    let mut dpr = 0.0_f64;
    let mut dpi = 0.0_f64;
    for &a in &monic_coeffs[1..] {
        let next_dpr = dpr * zr - dpi * zi + pr;
        let next_dpi = dpr * zi + dpi * zr + pi;
        dpr = next_dpr;
        dpi = next_dpi;

        let next_pr = pr * zr - pi * zi + a;
        let next_pi = pr * zi + pi * zr;
        pr = next_pr;
        pi = next_pi;
    }
    (pr, pi, dpr, dpi)
}

// ============================================================================
// Stage 2C: 26-Band Mel Filterbank, DCT-II MFCCs & Spectral Shape Features
// ============================================================================

struct SpectralFrameStats {
    mfcc_1: f32,
    mfcc_2: f32,
    mfcc_3: f32,
    centroid_hz: f32,
    tilt_db: f32,
}

struct MelFilterbank {
    bin_hz: f32,
    num_bins: usize,
    band_weights: Vec<Vec<(usize, f32)>>,
    dct_cos: [[f32; MEL_BANDS]; 3],
    tilt_band_indices: Vec<(usize, f32)>,
    tilt_mean_log2_f: f32,
    tilt_var_log2_f: f32,
}

impl MelFilterbank {
    fn new(sample_rate_hz: f32, fft_len: usize) -> Self {
        let num_bins = fft_len / 2 + 1;
        let bin_hz = sample_rate_hz / fft_len as f32;
        let mel_min = hz_to_mel(MEL_MIN_HZ);
        let mel_max = hz_to_mel(MEL_MAX_HZ.min(sample_rate_hz * 0.49));
        let step = (mel_max - mel_min) / (MEL_BANDS + 1) as f32;

        let mut mel_hz_pts = Vec::with_capacity(MEL_BANDS + 2);
        let mut mel_bin_pts = Vec::with_capacity(MEL_BANDS + 2);
        for i in 0..=(MEL_BANDS + 1) {
            let hz = mel_to_hz(mel_min + (i as f32) * step);
            mel_hz_pts.push(hz);
            mel_bin_pts.push(hz / bin_hz);
        }

        let mut band_weights = Vec::with_capacity(MEL_BANDS);
        let mut tilt_band_indices = Vec::new();
        for m in 0..MEL_BANDS {
            let left = mel_bin_pts[m];
            let center = mel_bin_pts[m + 1];
            let right = mel_bin_pts[m + 2];

            let k_start = (left.floor() as isize).max(1) as usize;
            let k_end = (right.ceil() as usize).min(num_bins - 1);
            let mut weights = Vec::new();

            for k in k_start..=k_end {
                let kf = k as f32;
                let w = if kf >= left && kf <= center && (center - left) > 1e-6 {
                    (kf - left) / (center - left)
                } else if kf > center && kf <= right && (right - center) > 1e-6 {
                    (right - kf) / (right - center)
                } else {
                    0.0
                };
                if w > 0.0 {
                    weights.push((k, w));
                }
            }
            // Guarantee even narrow low-freq bands retain their nearest FFT bin.
            if weights.is_empty() {
                let nearest = (center.round() as usize).clamp(1, num_bins - 1);
                weights.push((nearest, 1.0));
            }
            band_weights.push(weights);

            let center_hz = mel_hz_pts[m + 1];
            if (150.0..=4500.0).contains(&center_hz) {
                tilt_band_indices.push((m, center_hz.log2()));
            }
        }

        let dct_scale = (2.0 / MEL_BANDS as f32).sqrt();
        let mut dct_cos = [[0.0_f32; MEL_BANDS]; 3];
        for (coeff_idx, row) in dct_cos.iter_mut().enumerate() {
            let c = (coeff_idx + 1) as f32;
            for (m, slot) in row.iter_mut().enumerate() {
                *slot = dct_scale * (PI32 * c * (m as f32 + 0.5) / MEL_BANDS as f32).cos();
            }
        }

        let n_tilt = tilt_band_indices.len().max(1) as f32;
        let tilt_mean_log2_f = tilt_band_indices.iter().map(|&(_, x)| x).sum::<f32>() / n_tilt;
        let tilt_var_log2_f = tilt_band_indices
            .iter()
            .map(|&(_, x)| {
                let dx = x - tilt_mean_log2_f;
                dx * dx
            })
            .sum::<f32>()
            .max(1e-6);

        Self {
            bin_hz,
            num_bins,
            band_weights,
            dct_cos,
            tilt_band_indices,
            tilt_mean_log2_f,
            tilt_var_log2_f,
        }
    }

    fn extract_spectral_features(&self, power: &[f32]) -> SpectralFrameStats {
        let mut log_mel = [0.0_f32; MEL_BANDS];
        for (m, weights) in self.band_weights.iter().enumerate() {
            let energy: f32 = weights.iter().map(|&(k, w)| w * power[k]).sum();
            log_mel[m] = energy.max(1e-12).ln();
        }

        let mfcc_1 = dot_f32(&log_mel, &self.dct_cos[0]);
        let mfcc_2 = dot_f32(&log_mel, &self.dct_cos[1]);
        let mfcc_3 = dot_f32(&log_mel, &self.dct_cos[2]);

        let mut num = 0.0_f32;
        let mut den = 0.0_f32;
        for (k, &p) in power.iter().enumerate().take(self.num_bins).skip(1) {
            let mag = p.sqrt();
            num += (k as f32 * self.bin_hz) * mag;
            den += mag;
        }
        let centroid_hz = if den > 1e-9 { num / den } else { 0.0 };

        // Spectral tilt in dB/octave across mel bands in [150, 4500] Hz.
        let ln_to_db = 10.0 / std::f32::consts::LN_10;
        let n_tilt = self.tilt_band_indices.len().max(1) as f32;
        let mean_db = self
            .tilt_band_indices
            .iter()
            .map(|&(m, _)| log_mel[m] * ln_to_db)
            .sum::<f32>()
            / n_tilt;
        let cov_xy: f32 = self
            .tilt_band_indices
            .iter()
            .map(|&(m, x)| (x - self.tilt_mean_log2_f) * (log_mel[m] * ln_to_db - mean_db))
            .sum();
        let tilt_db = cov_xy / self.tilt_var_log2_f;

        SpectralFrameStats {
            mfcc_1,
            mfcc_2,
            mfcc_3,
            centroid_hz,
            tilt_db,
        }
    }
}

fn hz_to_mel(hz: f32) -> f32 {
    2595.0 * (1.0 + hz / 700.0).log10()
}

fn mel_to_hz(mel: f32) -> f32 {
    700.0 * (10.0_f32.powf(mel / 2595.0) - 1.0)
}

fn dot_f32(a: &[f32; MEL_BANDS], b: &[f32; MEL_BANDS]) -> f32 {
    a.iter().zip(b.iter()).map(|(&x, &y)| x * y).sum()
}

// ============================================================================
// Stage 3: Calibrated Temperature-Scaled Softmax Classifier
// ============================================================================

fn classify_features(features: &VocalTractFeatures) -> (AgeGroup, f32) {
    let formants_valid = features.formant_frames_used >= MIN_VALID_FORMANT_FRAMES
        && features.vtl_cm > 5.0
        && features.formant_dispersion_hz > 200.0;

    let z_vtl = if formants_valid {
        (features.vtl_cm - 14.0) / 1.5
    } else {
        0.0
    };
    let z_disp = if formants_valid {
        (features.formant_dispersion_hz - 1250.0) / 180.0
    } else {
        0.0
    };
    let z_f0 = (features.f0_median - 195.0) / 60.0;
    let z_cent = (features.spectral_centroid_hz - 1400.0) / 500.0;
    let z_tilt = (features.spectral_tilt_db + 6.0) / 4.0;
    let z_mfcc1 = features.mfcc_1 / 12.0;

    let weights = if formants_valid {
        &FORMANT_CLASS_WEIGHTS
    } else {
        &BACKUP_CLASS_WEIGHTS
    };

    let mut logits = [0.0_f32; 3];
    for (idx, w) in weights.iter().enumerate() {
        logits[idx] = w.vtl * z_vtl
            + w.dispersion * z_disp
            + w.f0 * z_f0
            + w.centroid * z_cent
            + w.tilt * z_tilt
            + w.mfcc1 * z_mfcc1
            + w.bias;
    }

    let max_logit = logits.iter().copied().fold(f32::NEG_INFINITY, f32::max);
    let mut exps = [0.0_f32; 3];
    let mut exp_sum = 0.0_f32;
    for (idx, &l) in logits.iter().enumerate() {
        let e = ((l - max_logit) / SOFTMAX_TEMPERATURE).exp();
        exps[idx] = e;
        exp_sum += e;
    }

    let inv_sum = if exp_sum > 0.0 && exp_sum.is_finite() {
        1.0 / exp_sum
    } else {
        1.0 / 3.0
    };
    let probs = [exps[0] * inv_sum, exps[1] * inv_sum, exps[2] * inv_sum];

    let classes = [AgeGroup::Kids, AgeGroup::Youth, AgeGroup::Adult];
    let mut best_idx = 0_usize;
    for idx in 1..3 {
        if probs[idx] > probs[best_idx] {
            best_idx = idx;
        }
    }

    (classes[best_idx], probs[best_idx].clamp(0.0, 1.0))
}

// ============================================================================
// Shared Window & Order-Statistics Helpers
// ============================================================================

fn make_periodic_hann(len: usize) -> Vec<f32> {
    let denom = len as f32;
    (0..len)
        .map(|n| 0.5 * (1.0 - (2.0 * PI32 * (n as f32) / denom).cos()))
        .collect()
}

fn make_hamming(len: usize) -> Vec<f32> {
    let denom = (len.saturating_sub(1)).max(1) as f32;
    (0..len)
        .map(|n| 0.54 - 0.46 * (2.0 * PI32 * (n as f32) / denom).cos())
        .collect()
}

fn median_f32(values: &mut [f32]) -> f32 {
    if values.is_empty() {
        return 0.0;
    }
    values.sort_by(f32::total_cmp);
    quantile_sorted(values, 0.5)
}

fn quantile_sorted(sorted: &[f32], q: f32) -> f32 {
    if sorted.is_empty() {
        return 0.0;
    }
    if sorted.len() == 1 {
        return sorted[0];
    }
    let pos = q.clamp(0.0, 1.0) * (sorted.len() - 1) as f32;
    let lo = pos.floor() as usize;
    let hi = pos.ceil() as usize;
    let frac = pos - (lo as f32);
    sorted[lo] * (1.0 - frac) + sorted[hi] * frac
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Synthesize a realistic vowel utterance as a glottal pulse train filtered through a
    /// `-6 dB/oct` glottal-radiation filter and 3 cascaded 2nd-order all-pole formant resonators.
    fn synthesize_vowel_speech(
        f0_hz: f32,
        formants_hz: [f32; 3],
        duration_ms: u32,
        amplitude: f32,
        aspiration_noise_amp: f32,
    ) -> Vec<f32> {
        let total_samples = (duration_ms as usize * 16_000) / 1000;
        let fs = 16_000.0_f64;

        // Bandwidths typical of sustained oral vowels: B1=70 Hz, B2=100 Hz, B3=140 Hz.
        let bandwidths = [70.0_f64, 100.0_f64, 140.0_f64];
        let mut r = [0.0_f64; 3];
        let mut two_r_cos = [0.0_f64; 3];
        let mut gain = [0.0_f64; 3];
        for i in 0..3 {
            let rad = (-PI64 * bandwidths[i] / fs).exp();
            let theta = 2.0 * PI64 * f64::from(formants_hz[i]) / fs;
            let a1 = 2.0 * rad * theta.cos();
            let a2 = rad * rad;
            r[i] = a2;
            two_r_cos[i] = a1;
            gain[i] = 1.0 - a1 + a2;
        }

        let mut glottal_state = 0.0_f64;
        let mut res_y1 = [0.0_f64; 3];
        let mut res_y2 = [0.0_f64; 3];

        let mut phase = 0.0_f64;
        let mut rng_state = 0x9E37_79B9_7F4A_7C15_u64;
        let mut out = Vec::with_capacity(total_samples);

        for n in 0..total_samples {
            // Subtle natural vibrato (+/- 1.2%) avoids synthetic zero-jitter degeneracies.
            let t = (n as f64) / fs;
            let inst_f0 = f64::from(f0_hz) * (1.0 + 0.012 * (2.0 * PI64 * 4.5 * t).sin());
            phase += inst_f0 / fs;

            let pulse = if phase >= 1.0 {
                phase -= 1.0;
                1.0_f64
            } else {
                0.0_f64
            };

            // Deterministic xorshift64 pseudo-random aspiration noise.
            rng_state ^= rng_state << 13;
            rng_state ^= rng_state >> 7;
            rng_state ^= rng_state << 17;
            let rand_unit = ((rng_state >> 11) as f64) / ((1_u64 << 53) as f64) * 2.0 - 1.0;

            let exc = pulse + f64::from(aspiration_noise_amp) * rand_unit;
            // Mild 1-pole glottal spectral tilt (-6 dB/oct above ~130 Hz).
            glottal_state = 0.95 * glottal_state + 0.05 * exc;

            let mut y = glottal_state;
            for i in 0..3 {
                let next = gain[i] * y + two_r_cos[i] * res_y1[i] - r[i] * res_y2[i];
                res_y2[i] = res_y1[i];
                res_y1[i] = next;
                y = next;
            }
            out.push(y as f32);
        }

        let rms = (out.iter().map(|&s| s * s).sum::<f32>() / out.len().max(1) as f32)
            .sqrt()
            .max(1e-6);
        let scale = amplitude / rms;
        for s in &mut out {
            *s *= scale;
        }
        out
    }

    fn make_noise(len: usize, rms: f32, seed: u64) -> Vec<f32> {
        let mut state = seed;
        let mut out = Vec::with_capacity(len);
        for _ in 0..len {
            state ^= state << 13;
            state ^= state >> 7;
            state ^= state << 17;
            let u = ((state >> 11) as f32) / ((1_u64 << 53) as f32) * 2.0 - 1.0;
            out.push(u);
        }
        let cur_rms = (out.iter().map(|&s| s * s).sum::<f32>() / out.len().max(1) as f32)
            .sqrt()
            .max(1e-6);
        let scale = rms / cur_rms;
        for s in &mut out {
            *s *= scale;
        }
        out
    }

    fn make_capture(
        speech: Vec<f32>,
        pre_roll: Vec<f32>,
        voiced_ms: u32,
        snr_db: f32,
        clipping_ratio: f32,
    ) -> CapturedAudio {
        let rms = (speech.iter().map(|&s| s * s).sum::<f32>() / speech.len().max(1) as f32).sqrt();
        CapturedAudio {
            samples_16k: speech.clone(),
            speech_only_16k: speech,
            pre_roll_noise_16k: pre_roll,
            voiced_ms,
            snr_db,
            clipping_ratio,
            rms_level: rms,
        }
    }

    #[test]
    fn quality_gates_reject_short_clipped_noisy_and_quiet_audio() {
        let clf = AgeClassifier::new(AgeAiConfig::default());
        let good_speech = synthesize_vowel_speech(175.0, [640.0, 1920.0, 3200.0], 1600, 0.08, 0.01);

        // 1. TooShort (< 1200 ms)
        let short = make_capture(good_speech.clone(), vec![0.0; 3200], 1100, 22.0, 0.0);
        assert_eq!(
            clf.classify(&short),
            AgeOutcome::Retry(RetryReason::TooShort)
        );

        // 2. Clipped (>= 0.02)
        let clipped = make_capture(good_speech.clone(), vec![0.0; 3200], 1600, 22.0, 0.025);
        assert_eq!(
            clf.classify(&clipped),
            AgeOutcome::Retry(RetryReason::Clipped)
        );

        // 3. TooNoisy (< 8.0 dB)
        let noisy = make_capture(good_speech.clone(), vec![0.0; 3200], 1600, 7.5, 0.0);
        assert_eq!(
            clf.classify(&noisy),
            AgeOutcome::Retry(RetryReason::TooNoisy)
        );

        // 4. TooQuiet absolute floor (< 0.005 even with high SNR)
        let ultra_quiet =
            synthesize_vowel_speech(285.0, [760.0, 2280.0, 3800.0], 1600, 0.004, 0.01);
        let cap_ultra_quiet = make_capture(ultra_quiet, vec![0.0; 3200], 1600, 24.0, 0.0);
        assert_eq!(
            clf.classify(&cap_ultra_quiet),
            AgeOutcome::Retry(RetryReason::TooQuiet)
        );

        // 5. TooQuiet SNR-coupled (< min_rms_level=0.015 AND snr < 12.0 dB)
        let dim_noisy = synthesize_vowel_speech(285.0, [760.0, 2280.0, 3800.0], 1600, 0.010, 0.01);
        let cap_dim_noisy = make_capture(dim_noisy.clone(), vec![0.0; 3200], 1600, 10.5, 0.0);
        assert_eq!(
            clf.classify(&cap_dim_noisy),
            AgeOutcome::Retry(RetryReason::TooQuiet)
        );

        // 6. Quiet room soft off-axis child (rms=0.010 < 0.015, but snr=16.0 >= 12.0) PASSES!
        let cap_soft_child = make_capture(dim_noisy, vec![0.0; 3200], 1600, 16.0, 0.0);
        match clf.classify(&cap_soft_child) {
            AgeOutcome::Detected(det) => assert_eq!(det.age, AgeGroup::Kids),
            AgeOutcome::Retry(r) => panic!("soft quiet-room child should pass, got {r:?}"),
        }
    }

    #[test]
    fn falls_back_to_samples_16k_when_speech_only_is_empty() {
        let clf = AgeClassifier::new(AgeAiConfig::default());
        let speech = synthesize_vowel_speech(115.0, [500.0, 1500.0, 2500.0], 1600, 0.08, 0.01);
        let audio = CapturedAudio {
            samples_16k: speech,
            speech_only_16k: Vec::new(),
            pre_roll_noise_16k: vec![0.0; 3200],
            voiced_ms: 1600,
            snr_db: 24.0,
            clipping_ratio: 0.0,
            rms_level: 0.08,
        };
        match clf.classify(&audio) {
            AgeOutcome::Detected(det) => assert_eq!(det.age, AgeGroup::Adult),
            AgeOutcome::Retry(r) => panic!("expected detection fallback to samples_16k, got {r:?}"),
        }
    }

    #[test]
    fn classifies_kid_youth_adult_female_and_adult_male_accurately() {
        let clf = AgeClassifier::new(AgeAiConfig::default());

        // 1. Kid: F0 = 285 Hz, F1/F2/F3 = 760/2280/3800 Hz -> VTL ~ 11.51 cm
        let kid_pcm = synthesize_vowel_speech(285.0, [760.0, 2280.0, 3800.0], 1800, 0.08, 0.01);
        let kid_cap = make_capture(kid_pcm, vec![0.0; 3200], 1800, 28.0, 0.0);
        let kid_det = match clf.classify(&kid_cap) {
            AgeOutcome::Detected(d) => d,
            AgeOutcome::Retry(r) => panic!("kid failed: {r:?}"),
        };
        eprintln!("KID: {kid_det:?}");
        assert_eq!(kid_det.age, AgeGroup::Kids, "kid detection: {kid_det:?}");
        assert!(
            (kid_det.f0_hz - 285.0).abs() < 8.0,
            "kid f0={}",
            kid_det.f0_hz
        );
        assert!(
            (kid_det.vtl_cm - 11.5).abs() < 0.8,
            "kid vtl={}",
            kid_det.vtl_cm
        );

        // 2. Youth: F0 = 175 Hz, F1/F2/F3 = 640/1920/3200 Hz -> VTL ~ 13.67 cm
        let youth_pcm = synthesize_vowel_speech(175.0, [640.0, 1920.0, 3200.0], 1800, 0.08, 0.01);
        let youth_cap = make_capture(youth_pcm, vec![0.0; 3200], 1800, 28.0, 0.0);
        let youth_det = match clf.classify(&youth_cap) {
            AgeOutcome::Detected(d) => d,
            AgeOutcome::Retry(r) => panic!("youth failed: {r:?}"),
        };
        eprintln!("YOUTH: {youth_det:?}");
        assert_eq!(
            youth_det.age,
            AgeGroup::Youth,
            "youth detection: {youth_det:?}"
        );
        assert!(
            (youth_det.f0_hz - 175.0).abs() < 8.0,
            "youth f0={}",
            youth_det.f0_hz
        );
        assert!(
            (youth_det.vtl_cm - 13.67).abs() < 0.8,
            "youth vtl={}",
            youth_det.vtl_cm
        );

        // 3. Adult Female: F0 = 240 Hz, F1/F2/F3 = 590/1770/2960 Hz -> VTL ~ 14.80 cm
        // Crucial test: must classify as Adult, NOT Kids or Youth despite high 240 Hz pitch!
        let female_pcm = synthesize_vowel_speech(240.0, [590.0, 1770.0, 2960.0], 1800, 0.08, 0.01);
        let female_cap = make_capture(female_pcm, vec![0.0; 3200], 1800, 28.0, 0.0);
        let female_det = match clf.classify(&female_cap) {
            AgeOutcome::Detected(d) => d,
            AgeOutcome::Retry(r) => panic!("adult female failed: {r:?}"),
        };
        eprintln!("FEMALE: {female_det:?}");
        assert_eq!(
            female_det.age,
            AgeGroup::Adult,
            "adult female detection: {female_det:?}"
        );
        assert!(
            (female_det.f0_hz - 240.0).abs() < 8.0,
            "female f0={}",
            female_det.f0_hz
        );
        assert!(
            (female_det.vtl_cm - 14.8).abs() < 0.8,
            "female vtl={}",
            female_det.vtl_cm
        );

        // 4. Adult Male: F0 = 115 Hz, F1/F2/F3 = 500/1500/2500 Hz -> VTL ~ 17.50 cm
        let male_pcm = synthesize_vowel_speech(115.0, [500.0, 1500.0, 2500.0], 1800, 0.08, 0.01);
        let male_cap = make_capture(male_pcm, vec![0.0; 3200], 1800, 28.0, 0.0);
        let male_det = match clf.classify(&male_cap) {
            AgeOutcome::Detected(d) => d,
            AgeOutcome::Retry(r) => panic!("adult male failed: {r:?}"),
        };
        eprintln!("MALE: {male_det:?}");
        assert_eq!(
            male_det.age,
            AgeGroup::Adult,
            "adult male detection: {male_det:?}"
        );
        assert!(
            (male_det.f0_hz - 115.0).abs() < 6.0,
            "male f0={}",
            male_det.f0_hz
        );
        assert!(
            (male_det.vtl_cm - 17.5).abs() < 0.9,
            "male vtl={}",
            male_det.vtl_cm
        );
    }

    #[test]
    fn pre_roll_noise_subtraction_preserves_classification_under_10db_noise() {
        let clf = AgeClassifier::new(AgeAiConfig::default());
        let clean = synthesize_vowel_speech(240.0, [590.0, 1770.0, 2960.0], 1800, 0.10, 0.005);
        // Add 10 dB ambient white noise (noise RMS = 0.0316 for speech RMS = 0.10).
        let noise_rms = 0.0316_f32;
        let pre_roll = make_noise(3200, noise_rms, 0x1234_5678_9ABC_DEF0);
        let speech_noise = make_noise(clean.len(), noise_rms, 0xCAFE_BABE_DEAD_BEEF);
        let noisy_speech: Vec<f32> = clean
            .iter()
            .zip(speech_noise.iter())
            .map(|(s, n)| s + n)
            .collect();

        let cap = make_capture(noisy_speech, pre_roll, 1800, 10.0, 0.0);
        match clf.classify(&cap) {
            AgeOutcome::Detected(det) => {
                eprintln!("NOISY_FEMALE: {det:?}");
                assert_eq!(det.age, AgeGroup::Adult, "noisy adult female: {det:?}");
                assert!((det.f0_hz - 240.0).abs() < 12.0, "f0={}", det.f0_hz);
            }
            AgeOutcome::Retry(r) => panic!("expected detection after noise subtraction, got {r:?}"),
        }
    }

    #[test]
    fn calibration_offsets_shift_f0_and_vtl_linearly() {
        let speech = synthesize_vowel_speech(175.0, [640.0, 1920.0, 3200.0], 1600, 0.08, 0.01);
        let base_cfg = AgeAiConfig::default();
        let cal_cfg = AgeAiConfig {
            f0_offset_hz: 12.5,
            vtl_offset_cm: 1.25,
            ..AgeAiConfig::default()
        };

        let base_feats = extract_vocal_tract_features(&speech, &[], &base_cfg).unwrap();
        let cal_feats = extract_vocal_tract_features(&speech, &[], &cal_cfg).unwrap();

        assert!(base_feats.formant_frames_used >= MIN_VALID_FORMANT_FRAMES);
        assert!((cal_feats.f0_median - base_feats.f0_median - 12.5).abs() < 1e-3);
        assert!((cal_feats.f0_p10 - base_feats.f0_p10 - 12.5).abs() < 1e-3);
        assert!((cal_feats.f0_p90 - base_feats.f0_p90 - 12.5).abs() < 1e-3);
        assert!((cal_feats.f0_iqr - base_feats.f0_iqr).abs() < 1e-3);
        assert!((cal_feats.vtl_cm - base_feats.vtl_cm - 1.25).abs() < 1e-3);
    }

    #[test]
    fn degrades_gracefully_when_fewer_than_five_formant_frames_survive() {
        // A pure 285 Hz sine wave has only 1 spectral peak (F0), so < 3 valid formant poles exist.
        let total = 1600 * 16; // 1.6 s @ 16 kHz
        let pure_tone: Vec<f32> = (0..total)
            .map(|n| 0.08 * (2.0 * PI32 * 285.0 * (n as f32) / 16_000.0).sin())
            .collect();

        let feats = extract_vocal_tract_features(&pure_tone, &[], &AgeAiConfig::default())
            .expect("feature extraction succeeds on pure tone");
        assert!(feats.formant_frames_used < MIN_VALID_FORMANT_FRAMES);
        assert!((feats.vtl_cm - 0.0).abs() < 1e-6);
        assert!((feats.formant_dispersion_hz - 0.0).abs() < 1e-6);
        assert!((feats.f0_median - 285.0).abs() < 5.0);

        let clf = AgeClassifier::new(AgeAiConfig::default());
        let cap = make_capture(pure_tone, Vec::new(), 1600, 30.0, 0.0);
        match clf.classify(&cap) {
            AgeOutcome::Detected(det) => {
                assert_eq!(det.age, AgeGroup::Kids);
                assert!(det.confidence.is_finite());
            }
            AgeOutcome::Retry(r) => panic!("expected graceful backup detection, got {r:?}"),
        }
    }

    #[test]
    fn aberth_ehrlich_recovers_exact_known_biquad_resonator_poles() {
        // Two known poles at 600 Hz (bw=80 Hz) and 1800 Hz (bw=120 Hz) at fs = 10 kHz.
        let fs = 10_000.0_f64;
        let poles_spec = [
            (600.0_f64, 80.0_f64),
            (1800.0_f64, 120.0_f64),
            (3200.0_f64, 150.0_f64),
        ];
        // Multiply quadratic factors (1 - 2r cos(theta) z^-1 + r^2 z^-2).
        let mut poly = vec![1.0_f64];
        for &(freq, bw) in &poles_spec {
            let r = (-PI64 * bw / fs).exp();
            let theta = 2.0 * PI64 * freq / fs;
            let q = [1.0, -2.0 * r * theta.cos(), r * r];
            let mut next = vec![0.0_f64; poly.len() + 2];
            for (i, &pi) in poly.iter().enumerate() {
                for (j, &qj) in q.iter().enumerate() {
                    next[i + j] += pi * qj;
                }
            }
            poly = next;
        }

        let extracted = extract_formants_from_lpc(&poly, None).expect("3 poles recovered");
        assert!((extracted[0] - 600.0).abs() < 1.0, "F1={}", extracted[0]);
        assert!((extracted[1] - 1800.0).abs() < 1.0, "F2={}", extracted[1]);
        assert!((extracted[2] - 3200.0).abs() < 1.0, "F3={}", extracted[2]);
    }

    #[test]
    fn fft_yin_matches_direct_difference_function() {
        let speech = synthesize_vowel_speech(210.0, [620.0, 1860.0, 3100.0], 200, 0.08, 0.005);
        let mut prefix_sq = Vec::with_capacity(speech.len() + 1);
        prefix_sq.push(0.0_f64);
        let mut acc = 0.0_f64;
        for &s in &speech {
            acc += f64::from(s) * f64::from(s);
            prefix_sq.push(acc);
        }

        let mut planner = FftPlanner::<f32>::new();
        let fwd = planner.plan_fft_forward(YIN_FFT_LEN);
        let inv = planner.plan_fft_inverse(YIN_FFT_LEN);
        let mut buf = vec![Complex::new(0.0_f32, 0.0_f32); YIN_FFT_LEN];
        let mut s1 = vec![Complex::new(0.0_f32, 0.0_f32); fwd.get_inplace_scratch_len()];
        let mut s2 = vec![Complex::new(0.0_f32, 0.0_f32); inv.get_inplace_scratch_len()];
        let mut yin = YinBuffers {
            fft_fwd: &fwd,
            fft_inv: &inv,
            buf: &mut buf,
            fwd_scratch: &mut s1,
            inv_scratch: &mut s2,
        };

        let res = compute_frame_yin_hnr(&speech, &prefix_sq, 0, &mut yin);
        let best_f0 = res.candidates[0].freq_hz;
        assert!((best_f0 - 210.0).abs() < 5.0, "got best_f0={best_f0}");
    }

    #[test]
    fn classification_finishes_within_latency_budget_for_four_second_audio() {
        let clf = AgeClassifier::new(AgeAiConfig::default());
        let speech = synthesize_vowel_speech(240.0, [590.0, 1770.0, 2960.0], 4000, 0.08, 0.01);
        let pre_roll = make_noise(3200, 0.005, 0x5555_AAAA_1111_2222);
        let cap = make_capture(speech, pre_roll, 4000, 24.0, 0.0);

        let feats = extract_vocal_tract_features(
            &cap.speech_only_16k,
            &cap.pre_roll_noise_16k,
            &AgeAiConfig::default(),
        );
        let feats_no_noise =
            extract_vocal_tract_features(&cap.speech_only_16k, &[], &AgeAiConfig::default());
        eprintln!("4s feats (with pre_roll): {feats:?}");
        eprintln!("4s feats (no pre_roll): {feats_no_noise:?}");
        let outcome = clf.classify(&cap);
        match outcome {
            AgeOutcome::Detected(det) => {
                eprintln!("4s detection: {det:?}");
                assert_eq!(det.age, AgeGroup::Adult);
                let budget_ms = if cfg!(debug_assertions) { 350 } else { 30 };
                assert!(
                    det.latency_ms <= budget_ms,
                    "latency_ms {} exceeded {budget_ms} ms budget",
                    det.latency_ms
                );
            }
            AgeOutcome::Retry(r) => panic!("unexpected retry: {r:?}"),
        }
    }
}
