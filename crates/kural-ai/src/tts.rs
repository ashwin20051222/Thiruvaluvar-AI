use crate::AiError;
use crate::stt::{SttEngine, load_stt_for_language};
use kural_audio::PcmChunk;
use kural_core::{AiConfig, Language, normalize_nfc_trimmed};
use sherpa_onnx::{
    GenerationConfig, OfflineTts, OfflineTtsConfig, OfflineTtsModelConfig,
    OfflineTtsVitsModelConfig,
};
use std::collections::{HashMap, HashSet};
use std::ffi::CString;
use std::fmt;
use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::time::Duration;
use tokio::sync::mpsc;
use tokio_util::sync::CancellationToken;

/// Silent pause duration inserted between consecutive top-level chunks (e.g., Tamil couplet lines).
pub const COUPLET_PAUSE_MS: u32 = 220;

const DEFAULT_FALLBACK_SAMPLE_RATE_HZ: u32 = 22_050;
const BASE_ESPEAK_WPM: f32 = 165.0;
const SEND_BACKOFF_MS: u64 = 2;

/// Normalize raw text into NFC-normalized, TTS-friendly prose.
///
/// Converts `#481` style Kural number markers into spoken `481`, expands dashes/ellipses
/// cleanly, and collapses repeated whitespace and punctuation.
#[must_use]
pub fn normalize_spoken_text(raw: &str, _lang: Language) -> String {
    let nfc = normalize_nfc_trimmed(raw);
    if nfc.is_empty() {
        return String::new();
    }

    let chars: Vec<char> = nfc.chars().collect();
    let mut pre = String::with_capacity(nfc.len());
    let mut idx = 0usize;

    while idx < chars.len() {
        let ch = chars[idx];
        match ch {
            '#' => {
                // Convert `#481` -> `481`, drop stray `#` so TTS never reads "hash".
                if idx + 1 < chars.len() && chars[idx + 1].is_ascii_digit() {
                    // Omit `#`, keep following digits intact.
                } else {
                    pre.push(' ');
                }
            }
            '…' => pre.push('.'),
            '—' | '–' => {
                pre.push(',');
                pre.push(' ');
            }
            '’' | '‘' => pre.push('\''),
            '“' | '”' | '"' | '«' | '»' => pre.push(' '),
            '।' | '॥' => pre.push('.'),
            c if c.is_whitespace() => pre.push(' '),
            c => pre.push(c),
        }
        idx += 1;
    }

    collapse_punctuation_and_whitespace(&pre)
}

fn collapse_punctuation_and_whitespace(input: &str) -> String {
    let chars: Vec<char> = input.chars().collect();
    let mut out = String::with_capacity(input.len());
    let mut i = 0usize;

    while i < chars.len() {
        let ch = chars[i];
        if ch == ' ' {
            if !out.is_empty() && !out.ends_with(' ') {
                out.push(' ');
            }
            i += 1;
            continue;
        }

        if matches!(ch, '.' | ',' | '!' | '?' | ';' | ':') {
            // Look ahead over spaces if we have `, ...` (from poetic pause templates).
            if ch == ',' {
                let mut look = i + 1;
                while look < chars.len() && chars[look] == ' ' {
                    look += 1;
                }
                if look < chars.len() && chars[look] == '.' {
                    let mut dots = look;
                    while dots < chars.len() && (chars[dots] == '.' || chars[dots] == ' ') {
                        dots += 1;
                    }
                    trim_trailing_space(&mut out);
                    out.push(',');
                    i = dots;
                    if i < chars.len() {
                        out.push(' ');
                    }
                    continue;
                }
            }

            let is_numeric_sep = i > 0
                && i + 1 < chars.len()
                && chars[i - 1].is_ascii_digit()
                && chars[i + 1].is_ascii_digit()
                && matches!(ch, '.' | ',' | ':');

            if is_numeric_sep {
                out.push(ch);
                i += 1;
                continue;
            }

            // Strip any space immediately before punctuation.
            trim_trailing_space(&mut out);

            // Collapse repeated identical punctuation (`...`, `!!`, `??`, `,,`).
            while i + 1 < chars.len() && chars[i + 1] == ch {
                i += 1;
            }

            // Avoid duplicating identical punctuation already at tail.
            if !out.ends_with(ch) {
                out.push(ch);
            }

            // Ensure single space after punctuation when followed by non-space/non-punct text.
            if i + 1 < chars.len()
                && chars[i + 1] != ' '
                && !matches!(chars[i + 1], '.' | ',' | '!' | '?' | ';' | ':')
            {
                out.push(' ');
            }

            i += 1;
            continue;
        }

        out.push(ch);
        i += 1;
    }

    out.trim().to_string()
}

fn trim_trailing_space(s: &mut String) {
    while s.ends_with(' ') {
        s.pop();
    }
}

/// Split prose into trimmed, non-empty speakable sentences on `.`, `!`, `?`, and newlines.
#[must_use]
pub fn split_tts_sentences(text: &str) -> Vec<String> {
    let chars: Vec<char> = text.chars().collect();
    let mut sentences = Vec::new();
    let mut current = String::new();
    let mut i = 0usize;

    while i < chars.len() {
        let ch = chars[i];

        if ch == '\n' || ch == '\r' {
            push_if_speakable(&mut sentences, &mut current);
            i += 1;
            continue;
        }

        if matches!(ch, '.' | '!' | '?' | '।' | '॥') {
            let is_decimal = ch == '.'
                && i > 0
                && i + 1 < chars.len()
                && chars[i - 1].is_ascii_digit()
                && chars[i + 1].is_ascii_digit();

            if is_decimal {
                current.push(ch);
                i += 1;
                continue;
            }

            let terminal = match ch {
                '।' | '॥' => '.',
                other => other,
            };
            while i + 1 < chars.len() && matches!(chars[i + 1], '.' | '!' | '?' | '।' | '॥') {
                i += 1;
            }
            if !current.trim().is_empty() {
                let trimmed_len = current.trim_end().len();
                current.truncate(trimmed_len);
                current.push(terminal);
                push_if_speakable(&mut sentences, &mut current);
            } else {
                current.clear();
            }
            i += 1;
            continue;
        }

        current.push(ch);
        i += 1;
    }

    push_if_speakable(&mut sentences, &mut current);
    sentences
}

fn push_if_speakable(out: &mut Vec<String>, current: &mut String) {
    let trimmed = current.trim();
    if !trimmed.is_empty() && trimmed.chars().any(char::is_alphanumeric) {
        out.push(trimmed.to_string());
    }
    current.clear();
}

#[derive(Debug, Clone, Copy)]
struct WavFormat {
    channels: u16,
    sample_rate_hz: u32,
}

/// Parse a RIFF/WAVE 16-bit PCM byte stream into mono `i16` samples and sample rate in Hz.
///
/// Dynamically scans subchunks (`fmt `, metadata chunks such as `LIST`/`JUNK`/`fact`, and `data`)
/// rather than hardcoding a 44-byte header offset, and handles streaming WAV lengths (e.g.
/// `espeak-ng --stdout` placeholder `0x7ffff000` chunk sizes). Multi-channel 16-bit PCM is mixed
/// cleanly down to mono.
pub fn parse_wav_pcm_mono_i16(wav_bytes: &[u8]) -> Result<(Vec<i16>, u32), AiError> {
    if wav_bytes.len() < 12 || &wav_bytes[0..4] != b"RIFF" || &wav_bytes[8..12] != b"WAVE" {
        return Err(AiError::Inference(
            "invalid WAV header: expected RIFF/WAVE signature".to_string(),
        ));
    }

    let mut offset = 12usize;
    let mut fmt_info: Option<WavFormat> = None;
    let mut data_slice: Option<&[u8]> = None;

    while offset + 8 <= wav_bytes.len() {
        let chunk_id = &wav_bytes[offset..offset + 4];
        let chunk_size = u32::from_le_bytes([
            wav_bytes[offset + 4],
            wav_bytes[offset + 5],
            wav_bytes[offset + 6],
            wav_bytes[offset + 7],
        ]) as usize;
        let payload_start = offset + 8;
        let remaining = wav_bytes.len() - payload_start;

        if chunk_id == b"fmt " {
            if chunk_size < 16 || chunk_size > remaining {
                return Err(AiError::Inference(
                    "invalid or truncated WAV 'fmt ' subchunk".to_string(),
                ));
            }
            let fmt = &wav_bytes[payload_start..payload_start + chunk_size];
            let audio_format = u16::from_le_bytes([fmt[0], fmt[1]]);
            let channels = u16::from_le_bytes([fmt[2], fmt[3]]);
            let sample_rate_hz = u32::from_le_bytes([fmt[4], fmt[5], fmt[6], fmt[7]]);
            let bits_per_sample = u16::from_le_bytes([fmt[14], fmt[15]]);

            let is_pcm = audio_format == 1
                || (audio_format == 0xFFFE && chunk_size >= 40 && fmt[24] == 1 && fmt[25] == 0);
            if !is_pcm {
                return Err(AiError::Inference(format!(
                    "unsupported WAV audio format {audio_format}; expected PCM (1)"
                )));
            }
            if channels == 0 || sample_rate_hz == 0 {
                return Err(AiError::Inference(format!(
                    "invalid WAV geometry: channels={channels}, sample_rate={sample_rate_hz}"
                )));
            }
            if bits_per_sample != 16 {
                return Err(AiError::Inference(format!(
                    "unsupported WAV bits_per_sample={bits_per_sample}; expected 16"
                )));
            }

            fmt_info = Some(WavFormat {
                channels,
                sample_rate_hz,
            });
            let padded = chunk_size + (chunk_size & 1);
            offset = payload_start.saturating_add(padded).min(wav_bytes.len());
        } else if chunk_id == b"data" {
            // `espeak-ng --stdout` emits placeholder 0x7ffff000 data size; clamp to remaining EOF.
            let actual_len = remaining.min(chunk_size);
            data_slice = Some(&wav_bytes[payload_start..payload_start + actual_len]);
            if chunk_size > remaining {
                break;
            }
            let padded = chunk_size + (chunk_size & 1);
            offset = payload_start.saturating_add(padded).min(wav_bytes.len());
        } else {
            if chunk_size > remaining {
                return Err(AiError::Inference(
                    "truncated WAV metadata subchunk before 'data'".to_string(),
                ));
            }
            let padded = chunk_size + (chunk_size & 1);
            offset = payload_start.saturating_add(padded).min(wav_bytes.len());
        }
    }

    let fmt = fmt_info
        .ok_or_else(|| AiError::Inference("WAV stream missing 'fmt ' subchunk".to_string()))?;
    let data = data_slice
        .ok_or_else(|| AiError::Inference("WAV stream missing 'data' subchunk".to_string()))?;

    let channels = usize::from(fmt.channels);
    let frame_bytes = channels.saturating_mul(2);
    let usable_bytes = (data.len() / frame_bytes) * frame_bytes;
    let payload = &data[..usable_bytes];

    let samples = if channels == 1 {
        payload
            .chunks_exact(2)
            .map(|b| i16::from_le_bytes([b[0], b[1]]))
            .collect()
    } else {
        payload
            .chunks_exact(frame_bytes)
            .map(|frame| {
                let sum: i32 = frame
                    .chunks_exact(2)
                    .map(|b| i32::from(i16::from_le_bytes([b[0], b[1]])))
                    .sum();
                let avg = sum / (channels as i32);
                avg.clamp(i32::from(i16::MIN), i32::from(i16::MAX)) as i16
            })
            .collect()
    };

    Ok((samples, fmt.sample_rate_hz))
}

struct PiperTts {
    engine: OfflineTts,
    sample_rate_hz: u32,
    has_lexicon: bool,
    valid_phoneme_chars: HashSet<char>,
    espeak_voice: String,
}

impl fmt::Debug for PiperTts {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("PiperTts")
            .field("sample_rate_hz", &self.sample_rate_hz)
            .field("has_lexicon", &self.has_lexicon)
            .field("espeak_voice", &self.espeak_voice)
            .finish_non_exhaustive()
    }
}

/// Single-active-language bilingual STT + streaming TTS orchestrator.
///
/// Enforces **drop-before-load** when switching between Tamil and English so only one heavy
/// language STT/TTS pair resides in memory at any point in time.
pub struct SpeechEngine {
    models_dir: PathBuf,
    ai_cfg: AiConfig,
    active_lang: Language,
    loaded_lang: Option<Language>,
    stt: Option<SttEngine>,
    tts: Option<PiperTts>,
    espeak_bin: Option<PathBuf>,
}

impl fmt::Debug for SpeechEngine {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("SpeechEngine")
            .field("models_dir", &self.models_dir)
            .field("active_lang", &self.active_lang)
            .field("stt_loaded", &self.stt_loaded())
            .field("tts_loaded", &self.tts_loaded())
            .finish_non_exhaustive()
    }
}

impl SpeechEngine {
    /// Construct a `SpeechEngine` and load the active STT/TTS assets for `initial_lang`.
    ///
    /// Missing model files on disk do not cause an error; `stt_loaded()` reflects ONNX STT
    /// availability and `tts_loaded()` reflects Piper ONNX TTS or `espeak-ng` availability.
    pub fn new(
        models_dir: &Path,
        initial_lang: Language,
        ai_cfg: &AiConfig,
    ) -> Result<Self, AiError> {
        Self::with_espeak_bin(models_dir, initial_lang, ai_cfg, find_espeak_ng_binary())
    }

    fn with_espeak_bin(
        models_dir: &Path,
        initial_lang: Language,
        ai_cfg: &AiConfig,
        espeak_bin: Option<PathBuf>,
    ) -> Result<Self, AiError> {
        let mut engine = Self {
            models_dir: models_dir.to_path_buf(),
            ai_cfg: ai_cfg.clone(),
            active_lang: initial_lang,
            loaded_lang: None,
            stt: None,
            tts: None,
            espeak_bin,
        };
        engine.ensure_language(initial_lang)?;
        Ok(engine)
    }

    /// Ensure `lang` is the sole resident language pair, dropping the previous language first.
    pub fn ensure_language(&mut self, lang: Language) -> Result<(), AiError> {
        if self.loaded_lang == Some(lang) {
            return Ok(());
        }

        // Drop old heavy models strictly BEFORE loading the newly requested language.
        self.stt = None;
        self.tts = None;
        self.loaded_lang = None;
        self.active_lang = lang;

        self.stt = load_stt_for_language(&self.models_dir, lang, &self.ai_cfg)?;
        self.tts = load_tts_for_language(&self.models_dir, lang, &self.ai_cfg)?;
        self.loaded_lang = Some(lang);
        Ok(())
    }

    /// Return the currently configured language.
    #[must_use]
    pub fn active_language(&self) -> Language {
        self.active_lang
    }

    /// Return `true` when an ONNX STT recognizer is currently loaded in memory.
    #[must_use]
    pub fn stt_loaded(&self) -> bool {
        self.stt.is_some()
    }

    /// Return `true` when either Piper ONNX TTS or local `espeak-ng` fallback is available.
    #[must_use]
    pub fn tts_loaded(&self) -> bool {
        self.tts.is_some() || self.espeak_bin.is_some()
    }

    /// Transcribe `16 kHz` mono PCM speech in `lang`.
    pub fn transcribe(&mut self, samples_16k: &[f32], lang: Language) -> Result<String, AiError> {
        self.ensure_language(lang)?;
        let Some(stt) = self.stt.as_mut() else {
            return Err(AiError::ModelLoad(format!(
                "STT model is not available for language '{lang}'"
            )));
        };
        stt.transcribe(samples_16k)
    }

    /// Stream sentence-chunked `i16` mono PCM audio over `pcm_tx` with mid-stream cancellation.
    ///
    /// Inserts a `220 ms` zero-PCM silent [`PcmChunk`] between consecutive non-empty entries in
    /// `text_chunks` (e.g., `[line1_ta, line2_ta]`). Stops immediately with [`AiError::Cancelled`]
    /// if `cancel` is triggered or `pcm_tx` closes, never emitting `is_final: true` on abort.
    pub fn synthesize_streaming(
        &mut self,
        text_chunks: &[&str],
        lang: Language,
        speed: f32,
        cancel: &CancellationToken,
        pcm_tx: &mpsc::Sender<PcmChunk>,
    ) -> Result<(), AiError> {
        if cancel.is_cancelled() || pcm_tx.is_closed() {
            return Err(AiError::Cancelled);
        }

        self.ensure_language(lang)?;

        if !self.tts_loaded() {
            return Err(AiError::FallbackTtsUnavailable(
                "neither Piper TTS model nor espeak-ng binary is available".to_string(),
            ));
        }

        let plan = build_synthesis_plan(text_chunks, lang);
        if plan.is_empty() {
            return Ok(());
        }

        let clamped_speed = if speed.is_finite() {
            speed.clamp(0.5, 2.0)
        } else {
            1.0
        };

        let mut last_sample_rate_hz: Option<u32> = None;
        let total_steps = plan.len();

        for (idx, step) in plan.into_iter().enumerate() {
            if cancel.is_cancelled() || pcm_tx.is_closed() {
                return Err(AiError::Cancelled);
            }

            let is_final = idx + 1 == total_steps;
            let chunk = match step {
                SynthStep::Pause220ms => {
                    let sr = last_sample_rate_hz.unwrap_or_else(|| self.active_sample_rate_hz());
                    let zero_samples =
                        ((u64::from(sr) * u64::from(COUPLET_PAUSE_MS)) / 1000) as usize;
                    PcmChunk {
                        samples: vec![0_i16; zero_samples],
                        sample_rate_hz: sr,
                        is_final,
                    }
                }
                SynthStep::Speak(sentence) => {
                    let (samples, sr) = self.synthesize_sentence(&sentence, lang, clamped_speed)?;
                    last_sample_rate_hz = Some(sr);
                    PcmChunk {
                        samples,
                        sample_rate_hz: sr,
                        is_final,
                    }
                }
            };

            if cancel.is_cancelled() || pcm_tx.is_closed() {
                return Err(AiError::Cancelled);
            }

            send_chunk_cancellable(pcm_tx, chunk, cancel)?;
        }

        Ok(())
    }

    fn active_sample_rate_hz(&self) -> u32 {
        self.tts
            .as_ref()
            .map_or(DEFAULT_FALLBACK_SAMPLE_RATE_HZ, |p| p.sample_rate_hz)
    }

    fn synthesize_sentence(
        &self,
        sentence: &str,
        lang: Language,
        speed: f32,
    ) -> Result<(Vec<i16>, u32), AiError> {
        if let Some(piper) = &self.tts
            && let Some(audio) =
                try_synthesize_piper(piper, sentence, speed, self.espeak_bin.as_deref())
        {
            return Ok(audio);
        }

        let Some(espeak_path) = self.espeak_bin.as_deref() else {
            return Err(AiError::FallbackTtsUnavailable(
                "espeak-ng binary not found on system".to_string(),
            ));
        };

        synthesize_with_espeak(espeak_path, sentence, lang, speed)
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
enum SynthStep {
    Speak(String),
    Pause220ms,
}

fn build_synthesis_plan(text_chunks: &[&str], lang: Language) -> Vec<SynthStep> {
    let mut grouped: Vec<Vec<String>> = Vec::new();
    for &raw_chunk in text_chunks {
        let mut chunk_sentences = Vec::new();
        for raw_sentence in split_tts_sentences(raw_chunk) {
            let norm = normalize_spoken_text(&raw_sentence, lang);
            if !norm.is_empty() && norm.chars().any(char::is_alphanumeric) {
                chunk_sentences.push(norm);
            }
        }
        if !chunk_sentences.is_empty() {
            grouped.push(chunk_sentences);
        }
    }

    let mut plan = Vec::new();
    let group_count = grouped.len();
    for (group_idx, sentences) in grouped.into_iter().enumerate() {
        for sentence in sentences {
            plan.push(SynthStep::Speak(sentence));
        }
        if group_idx + 1 < group_count {
            plan.push(SynthStep::Pause220ms);
        }
    }
    plan
}

fn send_chunk_cancellable(
    pcm_tx: &mpsc::Sender<PcmChunk>,
    mut chunk: PcmChunk,
    cancel: &CancellationToken,
) -> Result<(), AiError> {
    loop {
        if cancel.is_cancelled() || pcm_tx.is_closed() {
            return Err(AiError::Cancelled);
        }
        match pcm_tx.try_send(chunk) {
            Ok(()) => return Ok(()),
            Err(mpsc::error::TrySendError::Closed(_)) => return Err(AiError::Cancelled),
            Err(mpsc::error::TrySendError::Full(returned)) => {
                chunk = returned;
                std::thread::sleep(Duration::from_millis(SEND_BACKOFF_MS));
            }
        }
    }
}

fn try_synthesize_piper(
    piper: &PiperTts,
    sentence: &str,
    speed: f32,
    espeak_bin: Option<&Path>,
) -> Option<(Vec<i16>, u32)> {
    let generated = if piper.has_lexicon {
        let gen_cfg = GenerationConfig {
            speed,
            ..Default::default()
        };
        piper
            .engine
            .generate_with_config(sentence, &gen_cfg, None::<fn(&[f32], f32) -> bool>)
    } else {
        let espeak_path = espeak_bin?;
        let codepoints = phonemize_sentence_to_codepoints(
            espeak_path,
            &piper.espeak_voice,
            sentence,
            &piper.valid_phoneme_chars,
        )?;
        if codepoints.is_empty() {
            return None;
        }
        let gen_cfg = GenerationConfig {
            speed,
            phoneme_codepoints: Some(vec![codepoints]),
            ..Default::default()
        };
        piper
            .engine
            .generate_with_config("", &gen_cfg, None::<fn(&[f32], f32) -> bool>)
    }?;

    let raw_samples = generated.samples();
    let sample_rate = generated.sample_rate();
    if raw_samples.is_empty() || sample_rate <= 0 {
        return None;
    }

    let pcm: Vec<i16> = raw_samples.iter().copied().map(f32_to_i16).collect();
    Some((pcm, sample_rate as u32))
}

fn f32_to_i16(sample: f32) -> i16 {
    if !sample.is_finite() {
        return 0;
    }
    let scaled = (sample.clamp(-1.0, 1.0) * 32767.0).round();
    scaled.clamp(f32::from(i16::MIN), f32::from(i16::MAX)) as i16
}

fn phonemize_sentence_to_codepoints(
    espeak_path: &Path,
    voice: &str,
    sentence: &str,
    valid_chars: &HashSet<char>,
) -> Option<Vec<i32>> {
    let output = Command::new(espeak_path)
        .args(["--ipa", "-q", "-v", voice, "--", sentence])
        .output()
        .ok()?;
    if !output.status.success() {
        return None;
    }

    let raw_ipa = String::from_utf8_lossy(&output.stdout);
    let mut cps = Vec::with_capacity(raw_ipa.len());
    let mut prev_space = true;

    for ch in raw_ipa.chars() {
        let mapped = if ch == '\n' || ch == '\r' || ch == '\t' {
            ' '
        } else {
            ch
        };
        if mapped == ' ' {
            if !prev_space && (valid_chars.is_empty() || valid_chars.contains(&' ')) {
                cps.push(' ' as i32);
                prev_space = true;
            }
            continue;
        }
        if valid_chars.is_empty() || valid_chars.contains(&mapped) {
            cps.push(mapped as i32);
            prev_space = false;
        }
    }

    while cps.last() == Some(&(' ' as i32)) {
        cps.pop();
    }

    if let Some(last_punct) = sentence.trim_end().chars().last()
        && matches!(last_punct, '.' | '!' | '?' | ',')
        && (valid_chars.is_empty() || valid_chars.contains(&last_punct))
        && cps.last() != Some(&(last_punct as i32))
    {
        cps.push(last_punct as i32);
    }

    Some(cps)
}

fn synthesize_with_espeak(
    espeak_path: &Path,
    sentence: &str,
    lang: Language,
    speed: f32,
) -> Result<(Vec<i16>, u32), AiError> {
    let voice = match lang {
        Language::Tamil => "ta",
        Language::English => "en",
    };
    let wpm = ((BASE_ESPEAK_WPM * speed).round() as u32).clamp(80, 400);
    let wpm_str = wpm.to_string();

    let output = Command::new(espeak_path)
        .args(["-v", voice, "-s", &wpm_str, "--stdout", "--", sentence])
        .output()
        .map_err(|err| AiError::FallbackTtsUnavailable(format!("espeak-ng spawn failed: {err}")))?;

    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        return Err(AiError::Inference(format!(
            "espeak-ng failed with status {}: {}",
            output.status,
            stderr.trim()
        )));
    }

    parse_wav_pcm_mono_i16(&output.stdout)
}

fn find_espeak_ng_binary() -> Option<PathBuf> {
    let fixed = [
        Path::new("/usr/bin/espeak-ng"),
        Path::new("/usr/local/bin/espeak-ng"),
        Path::new("/bin/espeak-ng"),
    ];
    for candidate in fixed {
        if candidate.is_file() {
            return Some(candidate.to_path_buf());
        }
    }

    let path_var = std::env::var_os("PATH")?;
    for dir in std::env::split_paths(&path_var) {
        let candidate = dir.join("espeak-ng");
        if candidate.is_file() {
            return Some(candidate);
        }
    }
    None
}

struct PiperDiscovery {
    onnx_path: PathBuf,
    tokens_path: Option<PathBuf>,
    lexicon_path: Option<PathBuf>,
    json_path: Option<PathBuf>,
}

fn load_tts_for_language(
    models_dir: &Path,
    lang: Language,
    ai_cfg: &AiConfig,
) -> Result<Option<PiperTts>, AiError> {
    let Some(discovery) = discover_piper_files(models_dir, lang) else {
        return Ok(None);
    };

    let mut parsed_meta = discovery
        .json_path
        .as_deref()
        .and_then(|p| fs::read_to_string(p).ok())
        .and_then(|raw| parse_piper_onnx_json(&raw));

    let tokens_path = if let Some(existing) = discovery.tokens_path {
        existing
    } else if let Some(meta) = parsed_meta.as_ref()
        && !meta.phoneme_id_map.is_empty()
    {
        match write_piper_tokens_file(&discovery.onnx_path, &meta.phoneme_id_map) {
            Some(path) => path,
            None => return Ok(None),
        }
    } else {
        return Ok(None);
    };

    let valid_phoneme_chars = read_tokens_char_set(&tokens_path);
    let espeak_voice = parsed_meta
        .take()
        .and_then(|m| m.espeak_voice)
        .unwrap_or_else(|| default_espeak_voice(lang).to_string());

    let Ok(model_str) = path_to_c_safe_string(&discovery.onnx_path) else {
        return Ok(None);
    };
    let Ok(tokens_str) = path_to_c_safe_string(&tokens_path) else {
        return Ok(None);
    };
    let lexicon_str = match discovery.lexicon_path.as_deref() {
        Some(p) => path_to_c_safe_string(p).ok(),
        None => None,
    };

    let has_lexicon = lexicon_str.is_some();
    let cfg = OfflineTtsConfig {
        model: OfflineTtsModelConfig {
            vits: OfflineTtsVitsModelConfig {
                model: Some(model_str),
                lexicon: lexicon_str,
                tokens: Some(tokens_str),
                ..Default::default()
            },
            num_threads: ai_cfg.onnx_intra_threads.max(1),
            provider: Some("cpu".into()),
            debug: false,
            ..Default::default()
        },
        max_num_sentences: 1,
        ..Default::default()
    };

    // If `OfflineTts::create` returns `None`, seamlessly fall back to `espeak-ng`.
    let Some(engine) = OfflineTts::create(&cfg) else {
        return Ok(None);
    };
    let sr = engine.sample_rate();
    let sample_rate_hz = if sr > 0 {
        sr as u32
    } else {
        DEFAULT_FALLBACK_SAMPLE_RATE_HZ
    };

    Ok(Some(PiperTts {
        engine,
        sample_rate_hz,
        has_lexicon,
        valid_phoneme_chars,
        espeak_voice,
    }))
}

fn default_espeak_voice(lang: Language) -> &'static str {
    match lang {
        Language::Tamil => "ta",
        Language::English => "en-us",
    }
}

fn discover_piper_files(models_dir: &Path, lang: Language) -> Option<PiperDiscovery> {
    let tts_root = models_dir.join("tts");
    if !tts_root.is_dir() {
        return None;
    }

    let mut search_dirs = Vec::new();
    match lang {
        Language::English => {
            search_dirs.push(tts_root.join("en"));
            search_dirs.push(tts_root.join("en_US"));
        }
        Language::Tamil => {
            search_dirs.push(tts_root.join("ta"));
            search_dirs.push(tts_root.join("ta_IN"));
        }
    }
    search_dirs.push(tts_root.clone());

    if let Ok(entries) = fs::read_dir(&tts_root) {
        let mut subdirs: Vec<PathBuf> = entries
            .filter_map(Result::ok)
            .map(|e| e.path())
            .filter(|p| p.is_dir())
            .collect();
        subdirs.sort();
        for sd in subdirs {
            if !search_dirs.contains(&sd) {
                search_dirs.push(sd);
            }
        }
    }

    for dir in search_dirs {
        if !dir.is_dir() {
            continue;
        }
        if let Some(found) = find_piper_in_dir(&dir, lang) {
            return Some(found);
        }
    }
    None
}

fn find_piper_in_dir(dir: &Path, lang: Language) -> Option<PiperDiscovery> {
    let Ok(entries) = fs::read_dir(dir) else {
        return None;
    };
    let mut onnx_files: Vec<PathBuf> = entries
        .filter_map(Result::ok)
        .map(|e| e.path())
        .filter(|p| {
            p.is_file()
                && p.extension()
                    .and_then(|e| e.to_str())
                    .is_some_and(|e| e.eq_ignore_ascii_case("onnx"))
        })
        .collect();
    onnx_files.sort();

    let dir_name = dir
        .file_name()
        .and_then(|n| n.to_str())
        .unwrap_or_default()
        .to_ascii_lowercase();
    let in_lang_subdir = match lang {
        Language::English => {
            dir_name == "en" || dir_name.starts_with("en_") || dir_name.starts_with("en-")
        }
        Language::Tamil => {
            dir_name == "ta" || dir_name.starts_with("ta_") || dir_name.starts_with("ta-")
        }
    };

    let onnx_path = onnx_files.iter().find(|p| {
        let fname = p
            .file_name()
            .and_then(|n| n.to_str())
            .unwrap_or_default()
            .to_ascii_lowercase();
        match lang {
            Language::English => {
                fname.starts_with("en_")
                    || fname.starts_with("en-")
                    || fname.contains("lessac")
                    || in_lang_subdir
            }
            Language::Tamil => {
                fname.starts_with("ta_")
                    || fname.starts_with("ta-")
                    || fname.contains("valluva")
                    || in_lang_subdir
            }
        }
    })?;

    let stem = onnx_path
        .file_stem()
        .and_then(|s| s.to_str())
        .unwrap_or("model");
    let json_onnx = PathBuf::from(format!("{}.json", onnx_path.display()));
    let json_stem = dir.join(format!("{stem}.json"));
    let json_path = [json_onnx, json_stem].into_iter().find(|p| p.is_file());

    let lang_prefix = match lang {
        Language::English => "en",
        Language::Tamil => "ta",
    };

    let stem_tokens = dir.join(format!("{stem}.tokens.txt"));
    let lang_tokens = dir.join(format!("{lang_prefix}_tokens.txt"));
    let shared_tokens = dir.join("tokens.txt");
    let tokens_path = if stem_tokens.is_file() {
        Some(stem_tokens)
    } else if lang_tokens.is_file() {
        Some(lang_tokens)
    } else if shared_tokens.is_file() && (json_path.is_none() || onnx_files.len() == 1) {
        Some(shared_tokens)
    } else {
        None
    };

    let stem_lex = dir.join(format!("{stem}.lexicon.txt"));
    let lang_lex = dir.join(format!("{lang_prefix}_lexicon.txt"));
    let shared_lex = dir.join("lexicon.txt");
    let lexicon_path = [stem_lex, lang_lex, shared_lex]
        .into_iter()
        .find(|p| p.is_file());

    Some(PiperDiscovery {
        onnx_path: onnx_path.clone(),
        tokens_path,
        lexicon_path,
        json_path,
    })
}

fn write_piper_tokens_file(
    onnx_path: &Path,
    phoneme_id_map: &HashMap<String, i32>,
) -> Option<PathBuf> {
    let mut pairs: Vec<(&str, i32)> = phoneme_id_map
        .iter()
        .filter(|(sym, _)| !sym.contains('\n') && !sym.contains('\r') && !sym.is_empty())
        .map(|(sym, &id)| (sym.as_str(), id))
        .collect();
    pairs.sort_by(|a, b| a.1.cmp(&b.1).then_with(|| a.0.cmp(b.0)));

    let mut body = String::new();
    for (sym, id) in pairs {
        body.push_str(sym);
        body.push(' ');
        body.push_str(&id.to_string());
        body.push('\n');
    }

    let parent = onnx_path.parent()?;
    let stem = onnx_path
        .file_stem()
        .and_then(|s| s.to_str())
        .unwrap_or("model");

    let onnx_count = fs::read_dir(parent)
        .ok()
        .into_iter()
        .flatten()
        .filter_map(Result::ok)
        .filter(|e| {
            e.path()
                .extension()
                .and_then(|ext| ext.to_str())
                .is_some_and(|ext| ext.eq_ignore_ascii_case("onnx"))
        })
        .count();

    let preferred = if onnx_count <= 1 && !parent.join("tokens.txt").exists() {
        parent.join("tokens.txt")
    } else {
        parent.join(format!("{stem}.tokens.txt"))
    };

    if fs::write(&preferred, body.as_bytes()).is_ok() {
        return Some(preferred);
    }

    let tmp_path = std::env::temp_dir().join(format!("kural_{stem}.tokens.txt"));
    fs::write(&tmp_path, body.as_bytes()).ok()?;
    Some(tmp_path)
}

fn read_tokens_char_set(tokens_path: &Path) -> HashSet<char> {
    let Ok(raw) = fs::read_to_string(tokens_path) else {
        return HashSet::new();
    };
    let mut chars = HashSet::new();
    for line in raw.lines() {
        let trimmed = line.trim_end_matches('\r');
        if let Some((symbol, _id)) = trimmed.rsplit_once(' ') {
            let mut it = symbol.chars();
            if let (Some(ch), None) = (it.next(), it.next()) {
                chars.insert(ch);
            }
        }
    }
    chars
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

// --- Pure-Rust Piper `.onnx.json` parser (no `serde_json` dependency required) ---

struct PiperJsonMeta {
    phoneme_id_map: HashMap<String, i32>,
    espeak_voice: Option<String>,
}

fn parse_piper_onnx_json(raw: &str) -> Option<PiperJsonMeta> {
    let bytes = raw.as_bytes();
    let mut parser = JsonParser { bytes, pos: 0 };
    let JsonVal::Object(entries) = parser.parse_value()? else {
        return None;
    };

    let mut phoneme_id_map = HashMap::new();
    let mut espeak_voice = None;

    for (key, val) in entries {
        if key == "phoneme_id_map"
            && let JsonVal::Object(map_entries) = val
        {
            for (sym, ids_val) in map_entries {
                if let JsonVal::Array(arr) = ids_val
                    && let Some(JsonVal::Int(id)) = arr.first()
                {
                    phoneme_id_map.insert(sym, *id);
                }
            }
        } else if key == "espeak"
            && let JsonVal::Object(espeak_entries) = val
        {
            for (ek, ev) in espeak_entries {
                if ek == "voice"
                    && let JsonVal::Str(v) = ev
                {
                    espeak_voice = Some(v);
                }
            }
        }
    }

    Some(PiperJsonMeta {
        phoneme_id_map,
        espeak_voice,
    })
}

enum JsonVal {
    Object(Vec<(String, JsonVal)>),
    Array(Vec<JsonVal>),
    Str(String),
    Int(i32),
    Other,
}

struct JsonParser<'a> {
    bytes: &'a [u8],
    pos: usize,
}

impl<'a> JsonParser<'a> {
    fn skip_ws(&mut self) {
        while self.pos < self.bytes.len() && self.bytes[self.pos].is_ascii_whitespace() {
            self.pos += 1;
        }
    }

    fn peek(&mut self) -> Option<u8> {
        self.skip_ws();
        self.bytes.get(self.pos).copied()
    }

    fn parse_value(&mut self) -> Option<JsonVal> {
        match self.peek()? {
            b'{' => self.parse_object(),
            b'[' => self.parse_array(),
            b'"' => self.parse_string().map(JsonVal::Str),
            b'-' | b'0'..=b'9' => self.parse_number(),
            b't' => self.consume_literal(b"true").map(|()| JsonVal::Other),
            b'f' => self.consume_literal(b"false").map(|()| JsonVal::Other),
            b'n' => self.consume_literal(b"null").map(|()| JsonVal::Other),
            _ => None,
        }
    }

    fn consume_literal(&mut self, lit: &[u8]) -> Option<()> {
        if self.bytes.get(self.pos..self.pos + lit.len()) == Some(lit) {
            self.pos += lit.len();
            Some(())
        } else {
            None
        }
    }

    fn parse_object(&mut self) -> Option<JsonVal> {
        self.pos += 1; // '{'
        let mut out = Vec::new();
        if self.peek() == Some(b'}') {
            self.pos += 1;
            return Some(JsonVal::Object(out));
        }
        loop {
            if self.peek() != Some(b'"') {
                return None;
            }
            let key = self.parse_string()?;
            if self.peek() != Some(b':') {
                return None;
            }
            self.pos += 1;
            let val = self.parse_value()?;
            out.push((key, val));
            match self.peek()? {
                b',' => self.pos += 1,
                b'}' => {
                    self.pos += 1;
                    return Some(JsonVal::Object(out));
                }
                _ => return None,
            }
        }
    }

    fn parse_array(&mut self) -> Option<JsonVal> {
        self.pos += 1; // '['
        let mut out = Vec::new();
        if self.peek() == Some(b']') {
            self.pos += 1;
            return Some(JsonVal::Array(out));
        }
        loop {
            let val = self.parse_value()?;
            out.push(val);
            match self.peek()? {
                b',' => self.pos += 1,
                b']' => {
                    self.pos += 1;
                    return Some(JsonVal::Array(out));
                }
                _ => return None,
            }
        }
    }

    fn parse_number(&mut self) -> Option<JsonVal> {
        let start = self.pos;
        if self.bytes[self.pos] == b'-' {
            self.pos += 1;
        }
        let mut is_float = false;
        while self.pos < self.bytes.len() {
            match self.bytes[self.pos] {
                b'0'..=b'9' => self.pos += 1,
                b'.' | b'e' | b'E' | b'+' | b'-' => {
                    is_float = true;
                    self.pos += 1;
                }
                _ => break,
            }
        }
        let slice = std::str::from_utf8(&self.bytes[start..self.pos]).ok()?;
        if !is_float && let Ok(v) = slice.parse::<i32>() {
            return Some(JsonVal::Int(v));
        }
        Some(JsonVal::Other)
    }

    fn parse_string(&mut self) -> Option<String> {
        self.pos += 1; // '"'
        let mut out = String::new();
        while self.pos < self.bytes.len() {
            let b = self.bytes[self.pos];
            if b == b'"' {
                self.pos += 1;
                return Some(out);
            }
            if b == b'\\' {
                self.pos += 1;
                let esc = *self.bytes.get(self.pos)?;
                self.pos += 1;
                match esc {
                    b'"' => out.push('"'),
                    b'\\' => out.push('\\'),
                    b'/' => out.push('/'),
                    b'b' => out.push('\u{0008}'),
                    b'f' => out.push('\u{000C}'),
                    b'n' => out.push('\n'),
                    b'r' => out.push('\r'),
                    b't' => out.push('\t'),
                    b'u' => {
                        let code = self.parse_hex4()?;
                        if (0xD800..=0xDBFF).contains(&code)
                            && self.bytes.get(self.pos..self.pos + 2) == Some(b"\\u")
                        {
                            self.pos += 2;
                            let low = self.parse_hex4()?;
                            let combined = 0x10000
                                + (((u32::from(code) - 0xD800) << 10) | (u32::from(low) - 0xDC00));
                            out.push(char::from_u32(combined)?);
                        } else {
                            out.push(char::from_u32(u32::from(code))?);
                        }
                    }
                    _ => return None,
                }
            } else if b < 0x80 {
                out.push(char::from(b));
                self.pos += 1;
            } else {
                let rest = std::str::from_utf8(&self.bytes[self.pos..]).ok()?;
                let ch = rest.chars().next()?;
                out.push(ch);
                self.pos += ch.len_utf8();
            }
        }
        None
    }

    fn parse_hex4(&mut self) -> Option<u16> {
        let slice = self.bytes.get(self.pos..self.pos + 4)?;
        let s = std::str::from_utf8(slice).ok()?;
        let v = u16::from_str_radix(s, 16).ok()?;
        self.pos += 4;
        Some(v)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use kural_core::is_nfc;
    use tempfile::TempDir;

    #[test]
    fn normalize_spoken_text_handles_nfc_hashes_and_punctuation() {
        let en = normalize_spoken_text(
            "  Kural #481:   Virtue's  power... \n\n  Speak!!  ",
            Language::English,
        );
        assert_eq!(en, "Kural 481: Virtue's power. Speak!");

        // Decomposed Tamil 'கொ' (`க` + `ெ` + `ா`) must normalize to NFC `'கொ'`.
        let ta_raw = "  குறள் #1: க\u{0BC6}\u{0BBE}டு, ...  உலகு!! \n";
        let ta = normalize_spoken_text(ta_raw, Language::Tamil);
        assert!(is_nfc(&ta));
        assert_eq!(ta, "குறள் 1: கொடு, உலகு!");

        // Preserve decimal dots and numeric separators.
        let num = normalize_spoken_text("Speed 1.25x at 10:30.", Language::English);
        assert_eq!(num, "Speed 1.25x at 10:30.");
    }

    #[test]
    fn split_tts_sentences_splits_cleanly_and_drops_empty_segments() {
        let s = split_tts_sentences("First line.\nSecond line! Third?\n\n  ... \nFourth 1.25x\n");
        assert_eq!(
            s,
            vec!["First line.", "Second line!", "Third?", "Fourth 1.25x"]
        );

        assert!(split_tts_sentences("  \n\n . ! ? ").is_empty());
    }

    #[test]
    fn parse_wav_pcm_mono_i16_parses_extra_metadata_chunks_and_stereo_mixdown() {
        // Build a synthetic RIFF/WAVE with JUNK (odd length 5) and LIST/INFO chunks before `data`.
        let mut wav = Vec::new();
        wav.extend_from_slice(b"RIFF");
        wav.extend_from_slice(&0x7FFF_F000_u32.to_le_bytes()); // Streaming placeholder size
        wav.extend_from_slice(b"WAVE");

        // JUNK subchunk with odd payload size (5 bytes + 1 padding byte)
        wav.extend_from_slice(b"JUNK");
        wav.extend_from_slice(&5_u32.to_le_bytes());
        wav.extend_from_slice(b"abcde\0");

        // fmt subchunk: stereo, 16000 Hz, 16-bit PCM
        wav.extend_from_slice(b"fmt ");
        wav.extend_from_slice(&16_u32.to_le_bytes());
        wav.extend_from_slice(&1_u16.to_le_bytes()); // PCM
        wav.extend_from_slice(&2_u16.to_le_bytes()); // stereo
        wav.extend_from_slice(&16_000_u32.to_le_bytes());
        wav.extend_from_slice(&(16_000_u32 * 4).to_le_bytes());
        wav.extend_from_slice(&4_u16.to_le_bytes());
        wav.extend_from_slice(&16_u16.to_le_bytes());

        // LIST metadata subchunk before data
        wav.extend_from_slice(b"LIST");
        wav.extend_from_slice(&8_u32.to_le_bytes());
        wav.extend_from_slice(b"INFOtest");

        // data subchunk with streaming placeholder size (larger than actual payload)
        wav.extend_from_slice(b"data");
        wav.extend_from_slice(&0x7FFF_F000_u32.to_le_bytes());
        wav.extend_from_slice(&1000_i16.to_le_bytes());
        wav.extend_from_slice(&3000_i16.to_le_bytes());
        wav.extend_from_slice(&(-400_i16).to_le_bytes());
        wav.extend_from_slice(&(-800_i16).to_le_bytes());

        let (samples, sr) = parse_wav_pcm_mono_i16(&wav).expect("parse stereo wav with metadata");
        assert_eq!(sr, 16_000);
        assert_eq!(samples, vec![2000, -600]);
    }

    #[test]
    fn parse_wav_rejects_invalid_headers_and_non_16bit() {
        assert!(parse_wav_pcm_mono_i16(b"NOTWAV").is_err());

        let mut bad_bits = Vec::new();
        bad_bits.extend_from_slice(b"RIFF\x24\0\0\0WAVEfmt ");
        bad_bits.extend_from_slice(&16_u32.to_le_bytes());
        bad_bits.extend_from_slice(&1_u16.to_le_bytes());
        bad_bits.extend_from_slice(&1_u16.to_le_bytes());
        bad_bits.extend_from_slice(&16_000_u32.to_le_bytes());
        bad_bits.extend_from_slice(&32_000_u32.to_le_bytes());
        bad_bits.extend_from_slice(&2_u16.to_le_bytes());
        bad_bits.extend_from_slice(&8_u16.to_le_bytes()); // 8-bit invalid
        bad_bits.extend_from_slice(b"data\x04\0\0\0\x01\x02\x03\x04");
        assert!(parse_wav_pcm_mono_i16(&bad_bits).is_err());
    }

    #[test]
    fn piper_onnx_json_generates_tokens_txt_without_serde_json() {
        let dir = TempDir::new().expect("tempdir");
        let tts_dir = dir.path().join("tts");
        fs::create_dir_all(&tts_dir).expect("mkdir tts");

        let onnx_path = tts_dir.join("ta_IN-ValluvarNeural-medium.onnx");
        fs::write(&onnx_path, b"corrupt-onnx-triggers-espeak-fallback").expect("onnx");
        let json_body = r#"{
            "audio": { "sample_rate": 22050 },
            "espeak": { "voice": "ta" },
            "phoneme_id_map": {
                "_": [0],
                "^": [1],
                "$": [2],
                " ": [3],
                ",": [4],
                "\u0259": [19],
                "a": [14]
            }
        }"#;
        fs::write(
            tts_dir.join("ta_IN-ValluvarNeural-medium.onnx.json"),
            json_body,
        )
        .expect("json");

        // `load_tts_for_language` should extract tokens.txt and then gracefully return `Ok(None)`
        // when `OfflineTts::create` rejects the corrupt ONNX payload.
        let loaded = load_tts_for_language(dir.path(), Language::Tamil, &AiConfig::default())
            .expect("graceful fallback");
        assert!(loaded.is_none());

        let generated_tokens = fs::read_to_string(tts_dir.join("tokens.txt")).expect("tokens.txt");
        assert!(generated_tokens.contains("_ 0\n"));
        assert!(generated_tokens.contains("^ 1\n"));
        assert!(generated_tokens.contains("$ 2\n"));
        assert!(generated_tokens.contains("  3\n"));
        assert!(generated_tokens.contains(", 4\n"));
        assert!(generated_tokens.contains("ə 19\n"));
    }

    #[test]
    fn speech_engine_drop_before_load_and_missing_models_behave_cleanly() {
        let dir = TempDir::new().expect("tempdir");
        let ai_cfg = AiConfig::default();

        let mut engine = SpeechEngine::new(dir.path(), Language::Tamil, &ai_cfg).expect("init");
        assert_eq!(engine.active_language(), Language::Tamil);
        assert!(!engine.stt_loaded());

        // Switching language updates active_language cleanly even with empty models dir.
        engine
            .ensure_language(Language::English)
            .expect("switch en");
        assert_eq!(engine.active_language(), Language::English);
        assert!(!engine.stt_loaded());

        // Transcribe returns ModelLoad error when STT model is absent.
        let err = engine
            .transcribe(&[0.0_f32; 1600], Language::Tamil)
            .unwrap_err();
        assert!(matches!(err, AiError::ModelLoad(_)));
        assert_eq!(engine.active_language(), Language::Tamil);
    }

    #[test]
    fn unavailable_fallback_tts_returns_fallback_tts_unavailable_error() {
        let dir = TempDir::new().expect("tempdir");
        let mut engine = SpeechEngine::with_espeak_bin(
            dir.path(),
            Language::English,
            &AiConfig::default(),
            None,
        )
        .expect("init without espeak");

        assert!(!engine.tts_loaded());
        let (tx, _rx) = mpsc::channel(8);
        let cancel = CancellationToken::new();
        let err = engine
            .synthesize_streaming(&["Hello world"], Language::English, 1.0, &cancel, &tx)
            .unwrap_err();
        assert!(matches!(err, AiError::FallbackTtsUnavailable(_)));
    }

    #[test]
    fn couplet_streaming_inserts_220ms_silence_and_cancellation_aborts_without_is_final() {
        let Some(espeak) = find_espeak_ng_binary() else {
            return;
        };
        let dir = TempDir::new().expect("tempdir");
        let mut engine = SpeechEngine::with_espeak_bin(
            dir.path(),
            Language::Tamil,
            &AiConfig::default(),
            Some(espeak),
        )
        .expect("init with espeak");
        assert!(engine.tts_loaded());

        // 1. Tamil couplet [line1_ta, line2_ta] emits speech + 220ms zero-PCM silence + speech (is_final=true).
        let (tx, mut rx) = mpsc::channel(16);
        let cancel = CancellationToken::new();
        engine
            .synthesize_streaming(
                &["அகர முதல எழுத்தெல்லாம் ஆதி", "பகவன் முதற்றே உலகு."],
                Language::Tamil,
                1.0,
                &cancel,
                &tx,
            )
            .expect("couplet synth");
        drop(tx);

        let mut chunks = Vec::new();
        while let Some(c) = rx.blocking_recv() {
            chunks.push(c);
        }

        assert_eq!(chunks.len(), 3, "expected line1, 220ms silence, line2");
        assert!(!chunks[0].samples.is_empty());
        assert!(!chunks[0].is_final);

        let sr = chunks[0].sample_rate_hz;
        let expected_silence_len = ((u64::from(sr) * u64::from(COUPLET_PAUSE_MS)) / 1000) as usize;
        assert_eq!(chunks[1].sample_rate_hz, sr);
        assert_eq!(chunks[1].samples.len(), expected_silence_len);
        assert!(chunks[1].samples.iter().all(|&s| s == 0));
        assert!(!chunks[1].is_final);

        assert!(!chunks[2].samples.is_empty());
        assert!(chunks[2].is_final);

        // 2. Mid-stream cancellation when receiver drops or cancel token fires never emits is_final: true.
        let (tx2, mut rx2) = mpsc::channel(1);
        let cancel2 = CancellationToken::new();
        let cancel2_worker = cancel2.clone();

        let worker = std::thread::spawn(move || {
            engine.synthesize_streaming(
                &["Sentence one.", "Sentence two.", "Sentence three."],
                Language::English,
                1.0,
                &cancel2_worker,
                &tx2,
            )
        });

        let first = rx2.blocking_recv().expect("first chunk");
        assert!(!first.is_final);
        cancel2.cancel();
        let res = worker.join().expect("join synth worker");
        assert!(matches!(res, Err(AiError::Cancelled)));
        while let Some(rem) = rx2.blocking_recv() {
            assert!(
                !rem.is_final,
                "cancelled stream must never emit is_final: true"
            );
        }
    }

    #[test]
    fn bilingual_piper_json_keeps_separate_tokens_and_phonemizer_filters_symbols() {
        let dir = TempDir::new().expect("tempdir");
        let tts_dir = dir.path().join("tts");
        fs::create_dir_all(&tts_dir).expect("mkdir tts");

        fs::write(tts_dir.join("en_US-lessac-low.onnx"), b"en-corrupt").expect("en onnx");
        fs::write(
            tts_dir.join("en_US-lessac-low.onnx.json"),
            r#"{"espeak":{"voice":"en-us"},"phoneme_id_map":{"_": [0], "^": [1], "$": [2], " ": [3], "h": [10]}}"#,
        )
        .expect("en json");

        fs::write(
            tts_dir.join("ta_IN-ValluvarNeural-medium.onnx"),
            b"ta-corrupt",
        )
        .expect("ta onnx");
        fs::write(
            tts_dir.join("ta_IN-ValluvarNeural-medium.onnx.json"),
            r#"{"espeak":{"voice":"ta"},"phoneme_id_map":{"_": [0], "^": [1], "$": [2], " ": [3], "a": [11]}}"#,
        )
        .expect("ta json");

        assert!(
            load_tts_for_language(dir.path(), Language::English, &AiConfig::default())
                .unwrap()
                .is_none()
        );
        assert!(
            load_tts_for_language(dir.path(), Language::Tamil, &AiConfig::default())
                .unwrap()
                .is_none()
        );

        assert!(tts_dir.join("en_US-lessac-low.tokens.txt").is_file());
        assert!(
            tts_dir
                .join("ta_IN-ValluvarNeural-medium.tokens.txt")
                .is_file()
        );

        if let Some(espeak) = find_espeak_ng_binary() {
            let valid: HashSet<char> = ['h', 'ə', 'l', 'ˈ', 'o', 'ʊ', ' ', '!']
                .into_iter()
                .collect();
            let cps = phonemize_sentence_to_codepoints(&espeak, "en-us", "Hello!", &valid)
                .expect("ipa codepoints");
            assert!(!cps.is_empty());
            assert!(
                cps.iter()
                    .all(|&cp| { char::from_u32(cp as u32).is_some_and(|c| valid.contains(&c)) })
            );
            assert_eq!(cps.last().copied(), Some('!' as i32));
        }
    }

    #[test]
    fn closed_receiver_and_precancelled_token_abort_immediately() {
        let Some(espeak) = find_espeak_ng_binary() else {
            return;
        };
        let dir = TempDir::new().expect("tempdir");
        let mut engine = SpeechEngine::with_espeak_bin(
            dir.path(),
            Language::English,
            &AiConfig::default(),
            Some(espeak),
        )
        .expect("init");

        // Pre-cancelled token aborts before emitting anything.
        let (tx, mut rx) = mpsc::channel(4);
        let cancel = CancellationToken::new();
        cancel.cancel();
        let err = engine
            .synthesize_streaming(&["Hello world."], Language::English, 1.0, &cancel, &tx)
            .unwrap_err();
        assert!(matches!(err, AiError::Cancelled));
        drop(tx);
        assert!(rx.blocking_recv().is_none());

        // Dropped receiver causes Cancelled before sending.
        let (tx2, rx2) = mpsc::channel(4);
        drop(rx2);
        let cancel2 = CancellationToken::new();
        let err2 = engine
            .synthesize_streaming(&["Hello world."], Language::English, 1.0, &cancel2, &tx2)
            .unwrap_err();
        assert!(matches!(err2, AiError::Cancelled));

        // Empty/punctuation-only chunks return Ok(()) without hanging.
        let (tx3, mut rx3) = mpsc::channel(4);
        engine
            .synthesize_streaming(&["   ", "\n..."], Language::English, 1.0, &cancel2, &tx3)
            .expect("empty chunks");
        drop(tx3);
        assert!(rx3.blocking_recv().is_none());
    }
}
