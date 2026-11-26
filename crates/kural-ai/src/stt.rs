use crate::AiError;
use kural_core::{AiConfig, Language, normalize_nfc_trimmed};
use sherpa_onnx::{
    OfflineNemoEncDecCtcModelConfig, OfflineRecognizer, OfflineRecognizerConfig, OnlineRecognizer,
    OnlineRecognizerConfig, OnlineTransducerModelConfig,
};
use std::ffi::CString;
use std::fmt;
use std::fs;
use std::path::{Path, PathBuf};

const STT_SAMPLE_RATE_HZ: i32 = 16_000;
const STT_FEATURE_DIM: i32 = 80;
/// Leading silence prepended to offline Tamil STT buffers (250 ms at 16 kHz) so initial syllables
/// are never clipped on abrupt speech onset.
pub(crate) const OFFLINE_STT_LEADING_SILENCE_SAMPLES: usize = 4_000;
/// Trailing silence appended to offline Tamil STT buffers (500 ms at 16 kHz) so final syllables
/// flush cleanly.
pub(crate) const OFFLINE_STT_TRAILING_SILENCE_SAMPLES: usize = 8_000;
/// Leading silence prepended to streaming English Zipformer buffers (900 ms at 16 kHz) so the
/// encoder's initial left-context warmup never swallows short commands (e.g. `"one twenty eight"`).
pub(crate) const ONLINE_EN_STT_LEADING_SILENCE_SAMPLES: usize = 14_400;
/// Trailing silence appended to streaming English Zipformer buffers (800 ms at 16 kHz) so the
/// transducer's lookahead chunk flushes the final word completely.
pub(crate) const ONLINE_EN_STT_TRAILING_SILENCE_SAMPLES: usize = 12_800;

/// Discovered ONNX transducer artifacts for streaming English ASR.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct EnSttFiles {
    pub encoder: PathBuf,
    pub decoder: PathBuf,
    pub joiner: PathBuf,
    pub tokens: PathBuf,
}

/// Discovered ONNX NeMo CTC artifacts for offline Tamil ASR.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TaSttFiles {
    pub model: PathBuf,
    pub tokens: PathBuf,
}

/// Active language-specific `sherpa-onnx` speech-to-text recognizer.
pub(crate) enum SttEngine {
    English(OnlineRecognizer),
    Tamil(OfflineRecognizer),
}

impl fmt::Debug for SttEngine {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::English(_) => f.write_str("SttEngine::English(..)"),
            Self::Tamil(_) => f.write_str("SttEngine::Tamil(..)"),
        }
    }
}

impl SttEngine {
    /// Transcribe a `16 kHz` mono PCM utterance into NFC-normalized trimmed text.
    pub(crate) fn transcribe(&mut self, samples_16k: &[f32]) -> Result<String, AiError> {
        if samples_16k.is_empty() {
            return Ok(String::new());
        }

        let raw_text = match self {
            Self::English(rec) => {
                let padded = prepare_online_en_stt_samples(samples_16k);
                let stream = rec.create_stream();
                stream.accept_waveform(STT_SAMPLE_RATE_HZ, &padded);
                stream.input_finished();
                while rec.is_ready(&stream) {
                    rec.decode(&stream);
                }
                rec.get_result(&stream).map(|r| r.text).unwrap_or_default()
            }
            Self::Tamil(rec) => {
                let padded = prepare_offline_stt_samples(samples_16k);
                let stream = rec.create_stream();
                stream.accept_waveform(STT_SAMPLE_RATE_HZ, &padded);
                rec.decode(&stream);
                stream.get_result().map(|r| r.text).unwrap_or_default()
            }
        };

        Ok(normalize_nfc_trimmed(&raw_text))
    }
}

fn pad_and_sanitize_stt_samples(
    samples_16k: &[f32],
    leading_silence: usize,
    trailing_silence: usize,
) -> Vec<f32> {
    let total_len = leading_silence + samples_16k.len() + trailing_silence;
    let mut padded = Vec::with_capacity(total_len);
    padded.resize(leading_silence, 0.0);
    padded.extend(
        samples_16k
            .iter()
            .map(|&s| if s.is_finite() { s } else { 0.0 }),
    );
    padded.resize(total_len, 0.0);
    padded
}

/// Sanitize non-finite samples and pad with `250 ms` leading (`4_000` samples) and `500 ms`
/// trailing (`8_000` samples) zero-silence for offline Tamil STT decoding.
pub(crate) fn prepare_offline_stt_samples(samples_16k: &[f32]) -> Vec<f32> {
    pad_and_sanitize_stt_samples(
        samples_16k,
        OFFLINE_STT_LEADING_SILENCE_SAMPLES,
        OFFLINE_STT_TRAILING_SILENCE_SAMPLES,
    )
}

/// Sanitize non-finite samples and pad with `900 ms` leading (`14_400` samples) and `800 ms`
/// trailing (`12_800` samples) zero-silence for streaming English Zipformer decoding.
pub(crate) fn prepare_online_en_stt_samples(samples_16k: &[f32]) -> Vec<f32> {
    pad_and_sanitize_stt_samples(
        samples_16k,
        ONLINE_EN_STT_LEADING_SILENCE_SAMPLES,
        ONLINE_EN_STT_TRAILING_SILENCE_SAMPLES,
    )
}

/// Discover streaming English Zipformer transducer files under `models_dir/stt_en/` or `models_dir/stt/en/`.
#[must_use]
pub fn discover_en_stt_files(models_dir: &Path) -> Option<EnSttFiles> {
    [models_dir.join("stt_en"), models_dir.join("stt").join("en")]
        .into_iter()
        .flat_map(|base| candidate_dirs(&base))
        .find_map(|dir| find_en_stt_in_dir(&dir))
}

/// Discover offline Tamil NeMo CTC files under `models_dir/stt_ta/` or `models_dir/stt/ta/`.
#[must_use]
pub fn discover_ta_stt_files(models_dir: &Path) -> Option<TaSttFiles> {
    [models_dir.join("stt_ta"), models_dir.join("stt").join("ta")]
        .into_iter()
        .flat_map(|base| candidate_dirs(&base))
        .find_map(|dir| find_ta_stt_in_dir(&dir))
}

/// Load the STT engine for `lang`, returning `Ok(None)` cleanly when model artifacts are absent.
pub(crate) fn load_stt_for_language(
    models_dir: &Path,
    lang: Language,
    ai_cfg: &AiConfig,
) -> Result<Option<SttEngine>, AiError> {
    let num_threads = ai_cfg.onnx_intra_threads.max(1);

    match lang {
        Language::English => {
            let Some(files) = discover_en_stt_files(models_dir) else {
                tracing::warn!(
                    %lang,
                    models_dir = %models_dir.display(),
                    "English STT model absent; offline voice transcription unavailable for language"
                );
                return Ok(None);
            };

            let mut cfg = OnlineRecognizerConfig::default();
            cfg.feat_config.sample_rate = STT_SAMPLE_RATE_HZ;
            cfg.feat_config.feature_dim = STT_FEATURE_DIM;
            cfg.model_config.transducer = OnlineTransducerModelConfig {
                encoder: Some(path_to_c_safe_string(&files.encoder)?),
                decoder: Some(path_to_c_safe_string(&files.decoder)?),
                joiner: Some(path_to_c_safe_string(&files.joiner)?),
            };
            cfg.model_config.tokens = Some(path_to_c_safe_string(&files.tokens)?);
            cfg.model_config.num_threads = num_threads;
            cfg.model_config.provider = Some("cpu".into());
            cfg.model_config.debug = false;
            cfg.decoding_method = Some("modified_beam_search".into());
            cfg.max_active_paths = 4;
            cfg.enable_endpoint = false;

            let rec = OnlineRecognizer::create(&cfg).ok_or_else(|| {
                tracing::error!(
                    %lang,
                    encoder = %files.encoder.display(),
                    decoder = %files.decoder.display(),
                    joiner = %files.joiner.display(),
                    tokens = %files.tokens.display(),
                    "English Zipformer STT model failed to initialize"
                );
                AiError::ModelLoad(format!(
                    "failed to load English Zipformer STT from '{}'",
                    files.encoder.display()
                ))
            })?;
            Ok(Some(SttEngine::English(rec)))
        }
        Language::Tamil => {
            let Some(files) = discover_ta_stt_files(models_dir) else {
                tracing::warn!(
                    %lang,
                    models_dir = %models_dir.display(),
                    "Tamil STT model absent; offline voice transcription unavailable for language"
                );
                return Ok(None);
            };

            let mut cfg = OfflineRecognizerConfig::default();
            cfg.feat_config.sample_rate = STT_SAMPLE_RATE_HZ;
            cfg.feat_config.feature_dim = STT_FEATURE_DIM;
            cfg.model_config.nemo_ctc = OfflineNemoEncDecCtcModelConfig {
                model: Some(path_to_c_safe_string(&files.model)?),
            };
            cfg.model_config.tokens = Some(path_to_c_safe_string(&files.tokens)?);
            cfg.model_config.num_threads = num_threads;
            cfg.model_config.provider = Some("cpu".into());
            cfg.model_config.debug = false;
            cfg.decoding_method = Some("greedy_search".into());

            let rec = OfflineRecognizer::create(&cfg).ok_or_else(|| {
                tracing::error!(
                    %lang,
                    model = %files.model.display(),
                    tokens = %files.tokens.display(),
                    "Tamil NeMo CTC STT model failed to initialize"
                );
                AiError::ModelLoad(format!(
                    "failed to load Tamil NeMo CTC STT from '{}'",
                    files.model.display()
                ))
            })?;
            Ok(Some(SttEngine::Tamil(rec)))
        }
    }
}

fn candidate_dirs(base: &Path) -> Vec<PathBuf> {
    if !base.is_dir() {
        return Vec::new();
    }
    let mut dirs = vec![base.to_path_buf()];
    if let Ok(entries) = fs::read_dir(base) {
        let mut subdirs: Vec<PathBuf> = entries
            .filter_map(Result::ok)
            .map(|e| e.path())
            .filter(|p| p.is_dir())
            .collect();
        subdirs.sort();
        dirs.extend(subdirs);
    }
    dirs
}

fn sorted_files_in_dir(dir: &Path) -> Vec<PathBuf> {
    let Ok(entries) = fs::read_dir(dir) else {
        return Vec::new();
    };
    let mut files: Vec<PathBuf> = entries
        .filter_map(Result::ok)
        .map(|e| e.path())
        .filter(|p| p.is_file())
        .collect();
    files.sort();
    files
}

fn find_en_stt_in_dir(dir: &Path) -> Option<EnSttFiles> {
    let tokens = dir.join("tokens.txt");
    if !tokens.is_file() {
        return None;
    }
    let files = sorted_files_in_dir(dir);
    let encoder = find_role_onnx(&files, "encoder")?;
    let decoder = find_role_onnx(&files, "decoder")?;
    let joiner = find_role_onnx(&files, "joiner")?;
    Some(EnSttFiles {
        encoder,
        decoder,
        joiner,
        tokens,
    })
}

fn find_role_onnx(files: &[PathBuf], role: &str) -> Option<PathBuf> {
    let mut fallback = None;
    for path in files {
        let Some(name) = path.file_name().and_then(|n| n.to_str()) else {
            continue;
        };
        let lower = name.to_ascii_lowercase();
        if !lower.contains(role) || !lower.ends_with(".onnx") {
            continue;
        }
        if lower.ends_with(".int8.onnx") {
            return Some(path.clone());
        }
        if fallback.is_none() {
            fallback = Some(path.clone());
        }
    }
    fallback
}

fn find_ta_stt_in_dir(dir: &Path) -> Option<TaSttFiles> {
    let tokens = dir.join("tokens.txt");
    if !tokens.is_file() {
        return None;
    }

    let exact_int8 = dir.join("model.int8.onnx");
    if exact_int8.is_file() {
        return Some(TaSttFiles {
            model: exact_int8,
            tokens,
        });
    }

    let files = sorted_files_in_dir(dir);
    let mut exact_fp32 = None;
    let mut any_int8 = None;
    let mut any_onnx = None;

    for path in files {
        let Some(name) = path.file_name().and_then(|n| n.to_str()) else {
            continue;
        };
        let lower = name.to_ascii_lowercase();
        if !lower.ends_with(".onnx") {
            continue;
        }
        if lower == "model.onnx" && exact_fp32.is_none() {
            exact_fp32 = Some(path.clone());
        } else if (lower.ends_with(".int8.onnx") || lower.contains("int8")) && any_int8.is_none() {
            any_int8 = Some(path.clone());
        } else if any_onnx.is_none() {
            any_onnx = Some(path.clone());
        }
    }

    let model = any_int8.or(exact_fp32).or(any_onnx)?;
    Some(TaSttFiles { model, tokens })
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
    use tempfile::TempDir;

    #[test]
    fn missing_stt_models_return_ok_none_without_panicking() {
        let dir = TempDir::new().expect("tempdir");
        let ai_cfg = AiConfig::default();

        assert!(discover_en_stt_files(dir.path()).is_none());
        assert!(discover_ta_stt_files(dir.path()).is_none());

        assert!(
            load_stt_for_language(dir.path(), Language::English, &ai_cfg)
                .expect("en missing")
                .is_none()
        );
        assert!(
            load_stt_for_language(dir.path(), Language::Tamil, &ai_cfg)
                .expect("ta missing")
                .is_none()
        );
    }

    #[test]
    fn discovers_en_int8_priority_and_nested_subdir() {
        let dir = TempDir::new().expect("tempdir");
        let nested = dir.path().join("stt_en/zipformer-en-20M");
        fs::create_dir_all(&nested).expect("mkdir");

        fs::write(nested.join("tokens.txt"), b"<blk> 0\n").expect("tokens");
        fs::write(nested.join("encoder-epoch-99.onnx"), b"fp32").expect("enc fp32");
        fs::write(nested.join("encoder-epoch-99.int8.onnx"), b"int8").expect("enc int8");
        fs::write(nested.join("decoder-epoch-99.onnx"), b"dec").expect("dec");
        fs::write(nested.join("joiner-epoch-99.int8.onnx"), b"join").expect("join");

        let found = discover_en_stt_files(dir.path()).expect("en files");
        assert_eq!(found.encoder, nested.join("encoder-epoch-99.int8.onnx"));
        assert_eq!(found.decoder, nested.join("decoder-epoch-99.onnx"));
        assert_eq!(found.joiner, nested.join("joiner-epoch-99.int8.onnx"));
        assert_eq!(found.tokens, nested.join("tokens.txt"));
    }

    #[test]
    fn discovers_ta_model_priority_and_errors_on_corrupt_onnx() {
        let dir = TempDir::new().expect("tempdir");
        let ta_dir = dir.path().join("stt_ta");
        fs::create_dir_all(&ta_dir).expect("mkdir");

        fs::write(ta_dir.join("tokens.txt"), b"<blk> 0\n").expect("tokens");
        fs::write(ta_dir.join("indicconformer-ta-ctc-int8.onnx"), b"int8").expect("ta int8");

        let found = discover_ta_stt_files(dir.path()).expect("ta files");
        assert_eq!(found.model, ta_dir.join("indicconformer-ta-ctc-int8.onnx"));
        assert_eq!(found.tokens, ta_dir.join("tokens.txt"));

        fs::write(ta_dir.join("model.int8.onnx"), b"corrupt").expect("model.int8");
        let found2 = discover_ta_stt_files(dir.path()).expect("ta files2");
        assert_eq!(found2.model, ta_dir.join("model.int8.onnx"));

        let err =
            load_stt_for_language(dir.path(), Language::Tamil, &AiConfig::default()).unwrap_err();
        assert!(matches!(err, AiError::ModelLoad(_)));
    }

    #[test]
    fn partial_en_and_corrupt_en_models_behave_cleanly() {
        let dir = TempDir::new().expect("tempdir");
        let en_dir = dir.path().join("stt_en");
        fs::create_dir_all(&en_dir).expect("mkdir");

        // Missing joiner -> Ok(None), no panic.
        fs::write(en_dir.join("tokens.txt"), b"<blk> 0\n").expect("tokens");
        fs::write(en_dir.join("encoder.int8.onnx"), b"enc").expect("enc");
        fs::write(en_dir.join("decoder.int8.onnx"), b"dec").expect("dec");
        assert!(discover_en_stt_files(dir.path()).is_none());
        assert!(
            load_stt_for_language(dir.path(), Language::English, &AiConfig::default())
                .expect("incomplete en")
                .is_none()
        );

        // Once joiner is present with corrupt ONNX bytes -> Err(AiError::ModelLoad).
        fs::write(en_dir.join("joiner.int8.onnx"), b"corrupt").expect("join");
        let err =
            load_stt_for_language(dir.path(), Language::English, &AiConfig::default()).unwrap_err();
        assert!(matches!(err, AiError::ModelLoad(_)));
    }

    #[test]
    fn offline_stt_pads_leading_and_trailing_silence_and_sanitizes_non_finite() {
        let input = [0.25_f32, f32::NAN, -0.5, f32::INFINITY];
        let padded = prepare_offline_stt_samples(&input);
        assert_eq!(OFFLINE_STT_LEADING_SILENCE_SAMPLES, 4_000);
        assert_eq!(OFFLINE_STT_TRAILING_SILENCE_SAMPLES, 8_000);
        assert_eq!(padded.len(), 4_000 + input.len() + 8_000);
        assert!(padded[..4_000].iter().all(|&s| s == 0.0));
        assert_eq!(padded[4_000], 0.25);
        assert_eq!(padded[4_001], 0.0);
        assert_eq!(padded[4_002], -0.5);
        assert_eq!(padded[4_003], 0.0);
        assert!(padded[4_004..].iter().all(|&s| s == 0.0));

        let en_padded = prepare_online_en_stt_samples(&input);
        assert_eq!(ONLINE_EN_STT_LEADING_SILENCE_SAMPLES, 14_400);
        assert_eq!(ONLINE_EN_STT_TRAILING_SILENCE_SAMPLES, 12_800);
        assert_eq!(en_padded.len(), 14_400 + input.len() + 12_800);
        assert!(en_padded[..14_400].iter().all(|&s| s == 0.0));
        assert_eq!(en_padded[14_400], 0.25);
        assert_eq!(en_padded[14_402], -0.5);
        assert!(en_padded[14_404..].iter().all(|&s| s == 0.0));
    }

    #[test]
    fn discovers_en_and_ta_stt_files_under_nested_stt_lang_layout() {
        let dir = TempDir::new().expect("tempdir");

        let en_nested = dir.path().join("stt/en/zipformer");
        fs::create_dir_all(&en_nested).expect("mkdir en");
        fs::write(en_nested.join("tokens.txt"), b"<blk> 0\n").expect("en tokens");
        fs::write(en_nested.join("encoder.int8.onnx"), b"enc").expect("en enc");
        fs::write(en_nested.join("decoder.int8.onnx"), b"dec").expect("en dec");
        fs::write(en_nested.join("joiner.int8.onnx"), b"join").expect("en join");

        let en_found = discover_en_stt_files(dir.path()).expect("en under stt/en");
        assert_eq!(en_found.encoder, en_nested.join("encoder.int8.onnx"));
        assert_eq!(en_found.decoder, en_nested.join("decoder.int8.onnx"));
        assert_eq!(en_found.joiner, en_nested.join("joiner.int8.onnx"));
        assert_eq!(en_found.tokens, en_nested.join("tokens.txt"));

        let ta_nested = dir.path().join("stt/ta/indicconformer");
        fs::create_dir_all(&ta_nested).expect("mkdir ta");
        fs::write(ta_nested.join("tokens.txt"), b"<blk> 0\n").expect("ta tokens");
        fs::write(ta_nested.join("model.int8.onnx"), b"ta").expect("ta model");

        let ta_found = discover_ta_stt_files(dir.path()).expect("ta under stt/ta");
        assert_eq!(ta_found.model, ta_nested.join("model.int8.onnx"));
        assert_eq!(ta_found.tokens, ta_nested.join("tokens.txt"));
    }
}
