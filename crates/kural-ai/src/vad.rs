use crate::AiError;
use kural_audio::{AudioError, EnergyZcrVad, FRAME_SAMPLES_16K, VadBackend};
use kural_core::{AiConfig, AudioConfig};
use sherpa_onnx::{SileroVadModelConfig, VadModelConfig, VoiceActivityDetector};
use std::ffi::CString;
use std::fmt;
use std::path::{Path, PathBuf};

const VAD_MODEL_CANDIDATES: &[&str] = &[
    "silero_vad.onnx",
    "silero_vad.int8.onnx",
    "silero_vad_v5.onnx",
];
const VAD_SAMPLE_RATE_HZ: i32 = 16_000;
const VAD_BUFFER_SECONDS: f32 = 30.0;

/// Neural Voice Activity Detector backed by `sherpa-onnx` Silero VAD.
pub struct SherpaSileroVad {
    inner: VoiceActivityDetector,
    active_prob: f32,
}

impl fmt::Debug for SherpaSileroVad {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("SherpaSileroVad")
            .field("active_prob", &self.active_prob)
            .finish_non_exhaustive()
    }
}

impl SherpaSileroVad {
    /// Locate a Silero VAD ONNX file under `models_dir/vad/` and load it via `sherpa-onnx`.
    ///
    /// Returns `Ok(None)` when no Silero VAD model file is present on disk.
    pub fn try_load(
        models_dir: &Path,
        audio_cfg: &AudioConfig,
        ai_cfg: &AiConfig,
    ) -> Result<Option<Self>, AiError> {
        let Some(model_path) = discover_vad_model_path(models_dir) else {
            return Ok(None);
        };

        let model_str = path_to_c_safe_string(&model_path)?;
        let threshold = sanitize_threshold(audio_cfg.vad_threshold);
        let num_threads = ai_cfg.onnx_intra_threads.max(1);

        let cfg = VadModelConfig {
            silero_vad: SileroVadModelConfig {
                model: Some(model_str),
                threshold,
                min_silence_duration: 0.25,
                min_speech_duration: 0.12,
                window_size: FRAME_SAMPLES_16K as i32,
                max_speech_duration: 15.0,
            },
            sample_rate: VAD_SAMPLE_RATE_HZ,
            num_threads,
            provider: Some("cpu".into()),
            debug: false,
            ..Default::default()
        };

        let inner = VoiceActivityDetector::create(&cfg, VAD_BUFFER_SECONDS).ok_or_else(|| {
            AiError::ModelLoad(format!(
                "failed to initialize Silero VAD from '{}'",
                model_path.display()
            ))
        })?;

        Ok(Some(Self {
            inner,
            active_prob: compute_active_prob(threshold),
        }))
    }

    /// Load [`SherpaSileroVad`] when available on disk, otherwise fall back cleanly to [`EnergyZcrVad`].
    #[must_use]
    pub fn load_or_fallback(
        models_dir: &Path,
        audio_cfg: &AudioConfig,
        ai_cfg: &AiConfig,
    ) -> Box<dyn VadBackend> {
        match Self::try_load(models_dir, audio_cfg, ai_cfg) {
            Ok(Some(vad)) => Box::new(vad),
            Ok(None) | Err(_) => Box::new(EnergyZcrVad::default()),
        }
    }
}

impl VadBackend for SherpaSileroVad {
    fn reset(&mut self) {
        self.inner.reset();
        self.inner.clear();
    }

    fn speech_probability(&mut self, frame_512_16k: &[f32]) -> Result<f32, AudioError> {
        if frame_512_16k.len() != FRAME_SAMPLES_16K {
            return Err(AudioError::StreamError(format!(
                "expected {FRAME_SAMPLES_16K} samples per VAD frame, got {}",
                frame_512_16k.len()
            )));
        }

        let mut clean_buf = [0.0_f32; FRAME_SAMPLES_16K];
        let clean_frame: &[f32] = if frame_512_16k.iter().all(|s| s.is_finite()) {
            frame_512_16k
        } else {
            for (dst, &src) in clean_buf.iter_mut().zip(frame_512_16k.iter()) {
                *dst = if src.is_finite() { src } else { 0.0 };
            }
            &clean_buf
        };

        self.inner.accept_waveform(clean_frame);
        while !self.inner.is_empty() {
            self.inner.pop();
        }

        Ok(if self.inner.detected() {
            self.active_prob
        } else {
            0.0
        })
    }
}

fn discover_vad_model_path(models_dir: &Path) -> Option<PathBuf> {
    let vad_dir = models_dir.join("vad");
    if !vad_dir.is_dir() {
        return None;
    }
    VAD_MODEL_CANDIDATES
        .iter()
        .map(|name| vad_dir.join(name))
        .find(|candidate| candidate.is_file())
}

fn sanitize_threshold(raw: f32) -> f32 {
    if raw.is_finite() {
        raw.clamp(0.0, 1.0)
    } else {
        0.5
    }
}

fn compute_active_prob(threshold: f32) -> f32 {
    0.95_f32.max(threshold + 0.10).min(1.0)
}

fn path_to_c_safe_string(path: &Path) -> Result<String, AiError> {
    let s = path
        .to_str()
        .ok_or_else(|| AiError::ModelLoad(format!("non-UTF-8 model path: '{}'", path.display())))?;
    if CString::new(s).is_err() {
        return Err(AiError::ModelLoad(format!(
            "model path contains interior NUL byte: '{}'",
            path.display()
        )));
    }
    Ok(s.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    use tempfile::TempDir;

    #[test]
    fn missing_vad_model_returns_ok_none_and_falls_back_gracefully() {
        let dir = TempDir::new().expect("tempdir");
        let audio_cfg = AudioConfig::default();
        let ai_cfg = AiConfig::default();

        let loaded = SherpaSileroVad::try_load(dir.path(), &audio_cfg, &ai_cfg).expect("try_load");
        assert!(loaded.is_none());

        let mut backend = SherpaSileroVad::load_or_fallback(dir.path(), &audio_cfg, &ai_cfg);
        let silence = vec![0.0_f32; FRAME_SAMPLES_16K];
        let prob = backend.speech_probability(&silence).expect("silence frame");
        assert_eq!(prob, 0.0);

        let wrong_len = vec![0.0_f32; 256];
        assert!(matches!(
            backend.speech_probability(&wrong_len),
            Err(AudioError::StreamError(_))
        ));
    }

    #[test]
    fn corrupted_vad_model_returns_model_load_error_and_fallback_recovers() {
        let dir = TempDir::new().expect("tempdir");
        let vad_dir = dir.path().join("vad");
        fs::create_dir_all(&vad_dir).expect("mkdir vad");
        fs::write(vad_dir.join("silero_vad.onnx"), b"not-a-valid-onnx-model")
            .expect("write dummy onnx");

        let audio_cfg = AudioConfig::default();
        let ai_cfg = AiConfig::default();

        let err = SherpaSileroVad::try_load(dir.path(), &audio_cfg, &ai_cfg).unwrap_err();
        assert!(matches!(err, AiError::ModelLoad(_)));

        let mut backend = SherpaSileroVad::load_or_fallback(dir.path(), &audio_cfg, &ai_cfg);
        backend.reset();
        let silence = vec![0.0_f32; FRAME_SAMPLES_16K];
        assert_eq!(backend.speech_probability(&silence).unwrap(), 0.0);
    }

    #[test]
    fn discovers_candidate_priority_and_computes_active_prob() {
        let dir = TempDir::new().expect("tempdir");
        let vad_dir = dir.path().join("vad");
        fs::create_dir_all(&vad_dir).expect("mkdir vad");
        fs::write(vad_dir.join("silero_vad_v5.onnx"), b"v5").expect("write v5");
        assert_eq!(
            discover_vad_model_path(dir.path()),
            Some(vad_dir.join("silero_vad_v5.onnx"))
        );

        fs::write(vad_dir.join("silero_vad.int8.onnx"), b"int8").expect("write int8");
        assert_eq!(
            discover_vad_model_path(dir.path()),
            Some(vad_dir.join("silero_vad.int8.onnx"))
        );

        fs::write(vad_dir.join("silero_vad.onnx"), b"fp32").expect("write fp32");
        assert_eq!(
            discover_vad_model_path(dir.path()),
            Some(vad_dir.join("silero_vad.onnx"))
        );

        assert!((compute_active_prob(0.50) - 0.95).abs() < 1e-6);
        assert!((compute_active_prob(0.92) - 1.0).abs() < 1e-6);
        assert!((sanitize_threshold(f32::NAN) - 0.5).abs() < 1e-6);
    }
}
