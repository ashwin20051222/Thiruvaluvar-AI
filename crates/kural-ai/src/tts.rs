use crate::AiError;
use crate::stt::{SttEngine, load_stt_for_language};
use kural_audio::PcmChunk;
use kural_core::{AiConfig, Language, normalize_nfc_trimmed};
use serde_json::Value as JsonValue;
use sherpa_onnx::{
    GenerationConfig, OfflineTts, OfflineTtsConfig, OfflineTtsModelConfig,
    OfflineTtsVitsModelConfig,
};
use std::collections::{HashMap, HashSet};
use std::ffi::CString;
use std::fmt;
use std::fs;
use std::io::ErrorKind;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::time::{Duration, Instant, UNIX_EPOCH};
use tokio::sync::mpsc;
use tokio_util::sync::CancellationToken;

/// Silent pause duration inserted between consecutive top-level chunks (e.g., couplet and explanation).
pub const COUPLET_PAUSE_MS: u32 = 350;
/// Silent pause duration inserted between consecutive newline-separated lines within a chunk.
pub const LINE_PAUSE_MS: u32 = 150;

const DEFAULT_FALLBACK_SAMPLE_RATE_HZ: u32 = 22_050;
const BASE_ESPEAK_WPM: f32 = 165.0;
const SEND_BACKOFF_MS: u64 = 2;
const VITS_PEAK_CEILING: f32 = 0.8595;

/// Normalize raw text into NFC-normalized, TTS-friendly prose.
///
/// Converts `#481` style Kural number markers into spoken numbers, expands ASCII/Tamil digits
/// into natural Tamil words when `lang == Language::Tamil`, strips emojis/brackets/markdown
/// symbols without harming Tamil vowel signs or pulli, and collapses repeated whitespace and
/// punctuation.
#[must_use]
pub fn normalize_spoken_text(raw: &str, lang: Language) -> String {
    let nfc = normalize_nfc_trimmed(raw);
    if nfc.is_empty() {
        return String::new();
    }

    let chars: Vec<char> = nfc.chars().collect();
    let mut pre = String::with_capacity(nfc.len());
    let mut idx = 0usize;

    while idx < chars.len() {
        let ch = chars[idx];
        let prev_alnum = idx > 0 && chars[idx - 1].is_alphanumeric();
        let next_alnum = idx + 1 < chars.len() && chars[idx + 1].is_alphanumeric();

        match ch {
            // Preserve all Tamil script letters, vowel signs, pulli, and aytham; map Tamil numerals to ASCII for expansion.
            '\u{0BE6}'..='\u{0BEF}' => {
                let d = char::from_u32(u32::from('0') + (u32::from(ch) - 0x0BE6)).unwrap_or('0');
                pre.push(d);
            }
            '\u{0B80}'..='\u{0BFF}' => pre.push(ch),
            '#' => {
                // Convert `#481` -> `481`, drop stray `#` so TTS never reads "hash".
                if idx + 1 < chars.len() && chars[idx + 1].is_ascii_digit() {
                    // Omit `#`, keep following digits intact.
                } else {
                    pre.push(' ');
                }
            }
            '%' => {
                if idx > 0 && chars[idx - 1].is_ascii_digit() {
                    pre.push_str(match lang {
                        Language::Tamil => " சதவீதம் ",
                        Language::English => " percent ",
                    });
                } else {
                    pre.push(' ');
                }
            }
            '&' => {
                pre.push_str(match lang {
                    Language::Tamil => " மற்றும் ",
                    Language::English => " and ",
                });
            }
            '…' => pre.push('.'),
            '—' | '–' => {
                pre.push(',');
                pre.push(' ');
            }
            '’' | '‘' => pre.push('\''),
            '।' | '॥' => pre.push('.'),
            '-' => {
                if prev_alnum && next_alnum {
                    pre.push('-');
                } else {
                    pre.push(' ');
                }
            }
            // Strip zero-width joiners and emoji variation selectors cleanly without splitting words.
            '\u{200B}'..='\u{200F}' | '\u{FE00}'..='\u{FE0F}' | '\u{FEFF}' => {}
            // Strip brackets, quotes, markdown symbols, and decorative glyphs.
            '“' | '”' | '"' | '«' | '»' | '(' | ')' | '[' | ']' | '{' | '}' | '<' | '>' | '*'
            | '_' | '`' | '~' | '^' | '|' | '\\' | '/' | '=' | '+' | '@' | '$' => pre.push(' '),
            c if c.is_whitespace() => pre.push(' '),
            c if c.is_ascii_alphanumeric()
                || c.is_alphabetic()
                || matches!(c, '.' | ',' | '!' | '?' | ';' | ':' | '\'') =>
            {
                pre.push(c);
            }
            _ => pre.push(' '),
        }
        idx += 1;
    }

    let expanded = if lang == Language::Tamil {
        expand_tamil_numbers(&pre)
    } else {
        pre
    };

    collapse_punctuation_and_whitespace(&expanded)
}

fn expand_tamil_numbers(input: &str) -> String {
    let chars: Vec<char> = input.chars().collect();
    let mut out = String::with_capacity(input.len());
    let mut i = 0usize;

    while i < chars.len() {
        if !chars[i].is_ascii_digit() {
            out.push(chars[i]);
            i += 1;
            continue;
        }

        let prev_ascii_alpha = i > 0 && chars[i - 1].is_ascii_alphabetic();
        let start = i;
        while i < chars.len() && chars[i].is_ascii_digit() {
            i += 1;
        }
        let int_str: String = chars[start..i].iter().collect();

        // Check for decimal `A.B` (e.g. `1.25`).
        let mut frac_str: Option<String> = None;
        if i + 1 < chars.len() && chars[i] == '.' && chars[i + 1].is_ascii_digit() {
            let f_start = i + 1;
            let mut f_end = f_start;
            while f_end < chars.len() && chars[f_end].is_ascii_digit() {
                f_end += 1;
            }
            frac_str = Some(chars[f_start..f_end].iter().collect());
            i = f_end;
        }

        let next_ascii_alpha = i < chars.len() && chars[i].is_ascii_alphabetic();
        if prev_ascii_alpha || next_ascii_alpha {
            out.push_str(&int_str);
            if let Some(frac) = frac_str {
                out.push('.');
                out.push_str(&frac);
            }
            continue;
        }

        if let Some(prev) = out.chars().last()
            && !prev.is_whitespace()
            && !matches!(prev, '.' | ',' | '!' | '?' | ';' | ':' | '\'')
        {
            out.push(' ');
        }

        out.push_str(&digits_to_tamil_words(&int_str));
        if let Some(frac) = frac_str {
            out.push_str(" புள்ளி ");
            out.push_str(&digits_each_to_tamil(&frac));
        }

        if i < chars.len()
            && !chars[i].is_whitespace()
            && !matches!(chars[i], '.' | ',' | '!' | '?' | ';' | ':')
        {
            out.push(' ');
        }
    }

    out
}

fn digits_to_tamil_words(digits: &str) -> String {
    if digits.len() > 1 && digits.starts_with('0') {
        return digits_each_to_tamil(digits);
    }
    let Ok(num) = digits.parse::<u64>() else {
        return digits_each_to_tamil(digits);
    };
    if num > 99_999_999 {
        return digits_each_to_tamil(digits);
    }
    u64_to_tamil_words(num)
}

fn digits_each_to_tamil(digits: &str) -> String {
    let mut parts = Vec::with_capacity(digits.len());
    for ch in digits.chars() {
        let word = match ch {
            '0' => "பூஜ்ஜியம்",
            '1' => "ஒன்று",
            '2' => "இரண்டு",
            '3' => "மூன்று",
            '4' => "நான்கு",
            '5' => "ஐந்து",
            '6' => "ஆறு",
            '7' => "ஏழு",
            '8' => "எட்டு",
            '9' => "ஒன்பது",
            _ => "",
        };
        if !word.is_empty() {
            parts.push(word);
        }
    }
    parts.join(" ")
}

fn u64_to_tamil_words(n: u64) -> String {
    const ONES: [&str; 10] = [
        "பூஜ்ஜியம்",
        "ஒன்று",
        "இரண்டு",
        "மூன்று",
        "நான்கு",
        "ஐந்து",
        "ஆறு",
        "ஏழு",
        "எட்டு",
        "ஒன்பது",
    ];
    const TEENS: [&str; 10] = [
        "பத்து",
        "பதினொன்று",
        "பன்னிரண்டு",
        "பதின்மூன்று",
        "பதினான்கு",
        "பதினைந்து",
        "பதினாறு",
        "பதினேழு",
        "பதினெட்டு",
        "பத்தொன்பது",
    ];
    const TENS_EXACT: [&str; 10] = [
        "",
        "",
        "இருபது",
        "முப்பது",
        "நாற்பது",
        "ஐம்பது",
        "அறுபது",
        "எழுபது",
        "எண்பது",
        "தொண்ணூறு",
    ];
    const TENS_JOIN: [&str; 10] = [
        "",
        "",
        "இருபத்து",
        "முப்பத்து",
        "நாற்பத்து",
        "ஐம்பத்து",
        "அறுபத்து",
        "எழுபத்து",
        "எண்பத்து",
        "தொண்ணூற்று",
    ];
    const HUNDREDS_EXACT: [&str; 10] = [
        "",
        "நூறு",
        "இருநூறு",
        "முன்னூறு",
        "நானூறு",
        "ஐந்நூறு",
        "அறுநூறு",
        "எழுநூறு",
        "எண்ணூறு",
        "தொள்ளாயிரம்",
    ];
    const HUNDREDS_JOIN: [&str; 10] = [
        "",
        "நூற்று",
        "இருநூற்று",
        "முன்னூற்று",
        "நானூற்று",
        "ஐந்நூற்று",
        "அறுநூற்று",
        "எழுநூற்று",
        "எண்ணூற்று",
        "தொள்ளாயிரத்து",
    ];

    if n < 10 {
        return ONES[n as usize].to_string();
    }
    if n < 20 {
        return TEENS[(n - 10) as usize].to_string();
    }
    if n < 100 {
        let t = (n / 10) as usize;
        let rem = n % 10;
        return if rem == 0 {
            TENS_EXACT[t].to_string()
        } else {
            format!("{} {}", TENS_JOIN[t], ONES[rem as usize])
        };
    }
    if n < 1_000 {
        let h = (n / 100) as usize;
        let rem = n % 100;
        return if rem == 0 {
            HUNDREDS_EXACT[h].to_string()
        } else {
            format!("{} {}", HUNDREDS_JOIN[h], u64_to_tamil_words(rem))
        };
    }
    if n < 100_000 {
        let k = n / 1_000;
        let rem = n % 1_000;
        let k_word = match (k, rem == 0) {
            (1, true) => "ஆயிரம்".to_string(),
            (1, false) => "ஆயிரத்து".to_string(),
            (2, true) => "இரண்டாயிரம்".to_string(),
            (2, false) => "இரண்டாயிரத்து".to_string(),
            (3, true) => "மூவாயிரம்".to_string(),
            (3, false) => "மூவாயிரத்து".to_string(),
            (4, true) => "நான்காயிரம்".to_string(),
            (4, false) => "நான்காயிரத்து".to_string(),
            (5, true) => "ஐந்தாயிரம்".to_string(),
            (5, false) => "ஐந்தாயிரத்து".to_string(),
            (6, true) => "ஆறாயிரம்".to_string(),
            (6, false) => "ஆறாயிரத்து".to_string(),
            (7, true) => "ஏழாயிரம்".to_string(),
            (7, false) => "ஏழாயிரத்து".to_string(),
            (8, true) => "எட்டாயிரம்".to_string(),
            (8, false) => "எட்டாயிரத்து".to_string(),
            (9, true) => "ஒன்பதாயிரம்".to_string(),
            (9, false) => "ஒன்பதாயிரத்து".to_string(),
            (_, true) => format!("{} ஆயிரம்", u64_to_tamil_words(k)),
            (_, false) => format!("{} ஆயிரத்து", u64_to_tamil_words(k)),
        };
        return if rem == 0 {
            k_word
        } else {
            format!("{k_word} {}", u64_to_tamil_words(rem))
        };
    }
    if n < 10_000_000 {
        let lakh = n / 100_000;
        let rem = n % 100_000;
        let prefix = if lakh == 1 {
            "ஒரு".to_string()
        } else {
            u64_to_tamil_words(lakh)
        };
        return if rem == 0 {
            format!("{prefix} லட்சம்")
        } else {
            format!("{prefix} லட்சத்து {}", u64_to_tamil_words(rem))
        };
    }
    let crore = n / 10_000_000;
    let rem = n % 10_000_000;
    let prefix = if crore == 1 {
        "ஒரு".to_string()
    } else {
        u64_to_tamil_words(crore)
    };
    if rem == 0 {
        format!("{prefix} கோடி")
    } else {
        format!("{prefix} கோடியே {}", u64_to_tamil_words(rem))
    }
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
    loaded_speed: f32,
}

impl fmt::Debug for PiperTts {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("PiperTts")
            .field("sample_rate_hz", &self.sample_rate_hz)
            .field("has_lexicon", &self.has_lexicon)
            .field("espeak_voice", &self.espeak_voice)
            .field("loaded_speed", &self.loaded_speed)
            .finish_non_exhaustive()
    }
}

/// Alias for [`SpeechEngine`] when referenced as the TTS engine.
pub type TtsEngine = SpeechEngine;

/// Timed speech-to-text transcription output.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SttTranscription {
    pub text: String,
    pub latency_ms: u64,
}

/// Latency telemetry collected during streaming TTS synthesis.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct TtsSynthMetrics {
    pub first_chunk_ms: u64,
    pub total_ms: u64,
}

/// Stitched mono `i16` PCM samples alongside playback sample rate in Hz.
pub type CollectedTtsAudio = (Vec<i16>, u32);

/// Stitched optional TTS PCM output paired with streaming latency metrics.
pub type TimedTtsCollection = (Option<CollectedTtsAudio>, TtsSynthMetrics);

fn fnv1a_update(mut hash: u64, bytes: &[u8]) -> u64 {
    for &b in bytes {
        hash ^= u64::from(b);
        hash = hash.wrapping_mul(0x0100_0000_01b3);
    }
    hash
}

fn lang_fingerprint_code(lang: Language) -> u8 {
    match lang {
        Language::Tamil => 1,
        Language::English => 2,
    }
}

/// Compute an FNV-1a fingerprint identifying the active TTS model asset for `lang`.
#[must_use]
pub fn compute_tts_model_fingerprint(models_dir: &Path, lang: Language) -> u64 {
    let mut hash: u64 = 0xcbf2_9ce4_8422_2325;
    let code = [lang_fingerprint_code(lang)];
    if let Some(discovery) = discover_piper_files(models_dir, lang) {
        let name = discovery
            .onnx_path
            .file_name()
            .and_then(|n| n.to_str())
            .unwrap_or_default();
        hash = fnv1a_update(hash, name.as_bytes());
        if let Ok(meta) = fs::metadata(&discovery.onnx_path) {
            hash = fnv1a_update(hash, &meta.len().to_le_bytes());
            let (secs, nanos) = meta
                .modified()
                .ok()
                .and_then(|t| t.duration_since(UNIX_EPOCH).ok())
                .map_or((0_u64, 0_u32), |d| (d.as_secs(), d.subsec_nanos()));
            hash = fnv1a_update(hash, &secs.to_le_bytes());
            hash = fnv1a_update(hash, &nanos.to_le_bytes());
        }
        hash = fnv1a_update(hash, &code);
    } else {
        hash = fnv1a_update(hash, b"espeak-ng-or-none");
        hash = fnv1a_update(hash, &code);
    }
    hash
}

/// Single-active-language bilingual STT + streaming TTS orchestrator.
///
/// Enforces **drop-before-load** when switching between Tamil and English so only one heavy
/// language STT/TTS pair resides in memory at any point in time.
pub struct SpeechEngine {
    models_dir: PathBuf,
    ai_cfg: AiConfig,
    active_lang: Language,
    loaded_stt_lang: Option<Language>,
    loaded_tts_lang: Option<Language>,
    stt: Option<SttEngine>,
    tts: Option<PiperTts>,
    espeak_bin: Option<PathBuf>,
    active_tts_model_fingerprint: u64,
}

impl fmt::Debug for SpeechEngine {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("SpeechEngine")
            .field("models_dir", &self.models_dir)
            .field("active_lang", &self.active_lang)
            .field("stt_loaded", &self.stt_loaded())
            .field("tts_loaded", &self.tts_loaded())
            .field("tts_backend", &self.tts_backend_name())
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
            loaded_stt_lang: None,
            loaded_tts_lang: None,
            stt: None,
            tts: None,
            espeak_bin,
            active_tts_model_fingerprint: 0,
        };
        engine.ensure_language(initial_lang)?;
        Ok(engine)
    }

    fn compute_active_tts_fingerprint(&self, lang: Language) -> u64 {
        let model_fp = compute_tts_model_fingerprint(&self.models_dir, lang);
        let mut hash: u64 = 0xcbf2_9ce4_8422_2325;
        hash = fnv1a_update(hash, self.tts_backend_name().as_bytes());
        fnv1a_update(hash, &model_fp.to_le_bytes())
    }

    /// Return the fingerprint identifying the currently loaded TTS backend + model for the active language.
    #[must_use]
    pub fn active_tts_model_fingerprint(&self) -> u64 {
        self.active_tts_model_fingerprint
    }

    /// Ensure `lang` is the sole resident language pair, dropping the previous language first.
    pub fn ensure_language(&mut self, lang: Language) -> Result<(), AiError> {
        let stt_ok = self.loaded_stt_lang == Some(lang);
        let tts_ok = self.loaded_tts_lang == Some(lang);
        if stt_ok && tts_ok {
            self.active_lang = lang;
            return Ok(());
        }

        // Drop old heavy models strictly BEFORE loading the newly requested language.
        if !stt_ok {
            self.stt = None;
            self.loaded_stt_lang = None;
        }
        if !tts_ok {
            self.tts = None;
            self.loaded_tts_lang = None;
        }
        self.active_lang = lang;

        if !stt_ok {
            self.stt = load_stt_for_language(&self.models_dir, lang, &self.ai_cfg)?;
            self.loaded_stt_lang = Some(lang);
        }
        if !tts_ok {
            self.tts = load_tts_for_language(&self.models_dir, lang, &self.ai_cfg, 1.0)?;
            self.loaded_tts_lang = Some(lang);
            if !self.tts_loaded() {
                tracing::warn!(%lang, "Neither Piper VITS nor espeak-ng TTS is available");
            }
            self.active_tts_model_fingerprint = self.compute_active_tts_fingerprint(lang);
        }
        Ok(())
    }

    fn ensure_tts_for_run(&mut self, lang: Language, speed: f32) -> Result<(), AiError> {
        let needs_reload = match &self.tts {
            Some(piper) => {
                self.loaded_tts_lang != Some(lang) || (piper.loaded_speed - speed).abs() > 0.005
            }
            None => self.loaded_tts_lang != Some(lang),
        };
        if !needs_reload {
            return Ok(());
        }
        if self.loaded_tts_lang != Some(lang)
            && discover_piper_files(&self.models_dir, lang).is_none()
        {
            return Ok(());
        }

        self.tts = None;
        self.loaded_tts_lang = None;
        self.tts = load_tts_for_language(&self.models_dir, lang, &self.ai_cfg, speed)?;
        self.loaded_tts_lang = Some(lang);
        self.active_tts_model_fingerprint = self.compute_active_tts_fingerprint(lang);
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

    /// Return the active TTS backend name (`"sherpa-vits"`, `"espeak-ng"`, or `"unavailable"`).
    #[must_use]
    pub fn tts_backend_name(&self) -> &'static str {
        if self.tts.is_some() {
            "sherpa-vits"
        } else if self.espeak_bin.is_some() {
            "espeak-ng"
        } else {
            "unavailable"
        }
    }

    /// Transcribe `16 kHz` mono PCM speech in `lang`, returning normalized text and measured latency.
    pub fn transcribe_timed(
        &mut self,
        samples_16k: &[f32],
        lang: Language,
    ) -> Result<SttTranscription, AiError> {
        let t0 = Instant::now();
        self.ensure_language(lang)?;
        let Some(stt) = self.stt.as_mut() else {
            return Err(AiError::ModelLoad(format!(
                "STT model is not available for language '{lang}'"
            )));
        };
        let text = stt.transcribe(samples_16k)?;
        let latency_ms = t0.elapsed().as_millis() as u64;
        tracing::debug!(
            %lang,
            stt_latency_ms = latency_ms,
            chars = text.len(),
            "STT transcription completed"
        );
        Ok(SttTranscription { text, latency_ms })
    }

    /// Transcribe `16 kHz` mono PCM speech in `lang`.
    pub fn transcribe(&mut self, samples_16k: &[f32], lang: Language) -> Result<String, AiError> {
        self.transcribe_timed(samples_16k, lang).map(|r| r.text)
    }

    /// Stream sentence-chunked `i16` mono PCM audio over `pcm_tx` with mid-stream cancellation.
    ///
    /// Inserts a `350 ms` ([`COUPLET_PAUSE_MS`]) zero-PCM silent [`PcmChunk`] between consecutive
    /// non-empty entries in `text_chunks` and `150 ms` between newline-separated lines inside a chunk.
    /// Stops immediately with [`AiError::Cancelled`] if `cancel` is triggered or `pcm_tx` closes,
    /// never emitting `is_final: true` on abort.
    pub fn synthesize_streaming(
        &mut self,
        text_chunks: &[&str],
        lang: Language,
        speed: f32,
        cancel: &CancellationToken,
        pcm_tx: &mpsc::Sender<PcmChunk>,
    ) -> Result<(), AiError> {
        self.synthesize_streaming_inner(text_chunks, lang, speed, cancel, pcm_tx, false)
            .map(|_| ())
    }

    /// Stream sentence-chunked `i16` mono PCM audio over `pcm_tx` while also returning the full
    /// stitched mono buffer `(samples, sample_rate_hz)` **only** if synthesis completes without
    /// cancellation or error (so interrupted playback is never committed into the RAM replay cache).
    pub fn synthesize_streaming_collecting(
        &mut self,
        text_chunks: &[&str],
        lang: Language,
        speed: f32,
        cancel: &CancellationToken,
        pcm_tx: &mpsc::Sender<PcmChunk>,
    ) -> Result<Option<(Vec<i16>, u32)>, AiError> {
        self.synthesize_streaming_collecting_timed(text_chunks, lang, speed, cancel, pcm_tx)
            .map(|(pcm, _metrics)| pcm)
    }

    /// Stream sentence-chunked `i16` mono PCM audio over `pcm_tx`, returning both the stitched
    /// mono buffer (on clean completion) and synthesis latency metrics.
    pub fn synthesize_streaming_collecting_timed(
        &mut self,
        text_chunks: &[&str],
        lang: Language,
        speed: f32,
        cancel: &CancellationToken,
        pcm_tx: &mpsc::Sender<PcmChunk>,
    ) -> Result<TimedTtsCollection, AiError> {
        self.synthesize_streaming_inner(text_chunks, lang, speed, cancel, pcm_tx, true)
    }

    fn synthesize_streaming_inner(
        &mut self,
        text_chunks: &[&str],
        lang: Language,
        speed: f32,
        cancel: &CancellationToken,
        pcm_tx: &mpsc::Sender<PcmChunk>,
        collect: bool,
    ) -> Result<TimedTtsCollection, AiError> {
        let t0 = Instant::now();
        if cancel.is_cancelled() || pcm_tx.is_closed() {
            return Err(AiError::Cancelled);
        }

        let clamped_speed = if speed.is_finite() {
            speed.clamp(0.5, 2.0)
        } else {
            1.0
        };

        self.ensure_language(lang)?;

        if !self.tts_loaded() {
            return Err(AiError::FallbackTtsUnavailable(
                "neither Piper TTS model nor espeak-ng binary is available".to_string(),
            ));
        }

        let plan = build_synthesis_plan(text_chunks, lang);
        if plan.is_empty() {
            let elapsed = t0.elapsed().as_millis() as u64;
            return Ok((
                None,
                TtsSynthMetrics {
                    first_chunk_ms: elapsed,
                    total_ms: elapsed,
                },
            ));
        }

        let mut last_sample_rate_hz: Option<u32> = None;
        let mut collected_sample_rate_hz: Option<u32> = None;
        let mut collected = Vec::<i16>::new();
        let mut first_chunk_ms: Option<u64> = None;
        let total_steps = plan.len();

        for (idx, step) in plan.into_iter().enumerate() {
            if cancel.is_cancelled() || pcm_tx.is_closed() {
                return Err(AiError::Cancelled);
            }

            let is_final = idx + 1 == total_steps;
            let chunk = match step {
                SynthStep::PauseMs(pause_ms) => {
                    let sr = last_sample_rate_hz.unwrap_or_else(|| self.active_sample_rate_hz());
                    let zero_samples = ((u64::from(sr) * u64::from(pause_ms)) / 1000) as usize;
                    PcmChunk {
                        samples: vec![0_i16; zero_samples],
                        sample_rate_hz: sr,
                        is_final,
                    }
                }
                SynthStep::Speak(runs) => {
                    let (samples, sr) = self.synthesize_bilingual_runs(&runs, clamped_speed)?;
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

            let copy_for_cache = if collect {
                Some((chunk.samples.clone(), chunk.sample_rate_hz))
            } else {
                None
            };

            send_chunk_cancellable(pcm_tx, chunk, cancel)?;
            if first_chunk_ms.is_none() {
                first_chunk_ms = Some(t0.elapsed().as_millis() as u64);
            }

            if let Some((samples, chunk_sr)) = copy_for_cache {
                let dst_sr = *collected_sample_rate_hz.get_or_insert(chunk_sr);
                append_resampled_i16(&mut collected, &samples, chunk_sr, dst_sr);
            }
        }

        let total_ms = t0.elapsed().as_millis() as u64;
        let metrics = TtsSynthMetrics {
            first_chunk_ms: first_chunk_ms.unwrap_or(total_ms),
            total_ms,
        };
        tracing::debug!(
            %lang,
            tts_first_chunk_ms = metrics.first_chunk_ms,
            tts_total_ms = metrics.total_ms,
            backend = self.tts_backend_name(),
            "TTS synthesis completed"
        );

        if !collect || collected.is_empty() || cancel.is_cancelled() {
            return Ok((None, metrics));
        }

        Ok((collected_sample_rate_hz.map(|sr| (collected, sr)), metrics))
    }

    fn active_sample_rate_hz(&self) -> u32 {
        self.tts
            .as_ref()
            .map_or(DEFAULT_FALLBACK_SAMPLE_RATE_HZ, |p| p.sample_rate_hz)
    }

    fn synthesize_bilingual_runs(
        &mut self,
        runs: &[BilingualRun],
        speed: f32,
    ) -> Result<(Vec<i16>, u32), AiError> {
        let mut combined = Vec::new();
        let mut target_sr: Option<u32> = None;

        for run in runs {
            let (samples, sr) = self.synthesize_single_run(&run.text, run.lang, speed)?;
            if samples.is_empty() {
                continue;
            }
            let dst_sr = *target_sr.get_or_insert(sr);
            append_resampled_i16(&mut combined, &samples, sr, dst_sr);
        }

        Ok((
            combined,
            target_sr.unwrap_or_else(|| self.active_sample_rate_hz()),
        ))
    }

    fn synthesize_single_run(
        &mut self,
        sentence: &str,
        lang: Language,
        speed: f32,
    ) -> Result<(Vec<i16>, u32), AiError> {
        let _ = self.ensure_tts_for_run(lang, speed);

        if self.loaded_tts_lang == Some(lang)
            && let Some(piper) = &self.tts
            && let Some(audio) = try_synthesize_piper(piper, sentence, self.espeak_bin.as_deref())
        {
            return Ok(audio);
        }

        if let Some(espeak_path) = self.espeak_bin.as_deref() {
            return synthesize_with_espeak(espeak_path, sentence, lang, speed);
        }

        if let Some(piper) = &self.tts
            && let Some(audio) = try_synthesize_piper(piper, sentence, None)
        {
            return Ok(audio);
        }

        Err(AiError::FallbackTtsUnavailable(
            "espeak-ng binary not found on system".to_string(),
        ))
    }
}

/// Script-tagged segment produced by [`segment_bilingual_runs`] for mixed Tamil/English speech.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BilingualRun {
    pub lang: Language,
    pub text: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum ScriptKind {
    Tamil,
    English,
}

fn char_script(ch: char) -> Option<ScriptKind> {
    if ('\u{0B80}'..='\u{0BFF}').contains(&ch) && !('\u{0BE6}'..='\u{0BEF}').contains(&ch) {
        Some(ScriptKind::Tamil)
    } else if ch.is_ascii_alphabetic() {
        Some(ScriptKind::English)
    } else {
        None
    }
}

/// Split mixed Tamil and English text on script boundaries so each segment uses its native voice.
///
/// Neutral characters (whitespace, digits, punctuation) remain attached to the surrounding
/// script run so phrases like `"Chapter 12"` stay English while `"குறள் 481"` stays Tamil.
#[must_use]
pub fn segment_bilingual_runs(text: &str, default_lang: Language) -> Vec<BilingualRun> {
    let trimmed = text.trim();
    if trimmed.is_empty() {
        return Vec::new();
    }

    let mut runs: Vec<(Language, String)> = Vec::new();
    let mut pending_neutral = String::new();
    let mut active_lang: Option<Language> = None;

    for word in trimmed.split_whitespace() {
        let mut sub_start = 0usize;
        let mut word_script: Option<ScriptKind> = None;
        let mut saw_any_script = false;

        for (byte_idx, ch) in word.char_indices() {
            if let Some(sc) = char_script(ch) {
                if let Some(prev_sc) = word_script
                    && prev_sc != sc
                {
                    let part = &word[sub_start..byte_idx];
                    push_script_token(
                        &mut runs,
                        &mut active_lang,
                        &mut pending_neutral,
                        script_to_lang(prev_sc),
                        part,
                    );
                    sub_start = byte_idx;
                }
                word_script = Some(sc);
                saw_any_script = true;
            }
        }

        let tail = &word[sub_start..];
        if saw_any_script && let Some(sc) = word_script {
            push_script_token(
                &mut runs,
                &mut active_lang,
                &mut pending_neutral,
                script_to_lang(sc),
                tail,
            );
        } else {
            if !pending_neutral.is_empty() {
                pending_neutral.push(' ');
            }
            pending_neutral.push_str(word);
        }
    }

    if !pending_neutral.is_empty() {
        if let Some((_, last_text)) = runs.last_mut() {
            last_text.push(' ');
            last_text.push_str(&pending_neutral);
        } else {
            runs.push((default_lang, pending_neutral));
        }
    }

    runs.into_iter()
        .filter_map(|(lang, seg)| {
            let t = seg.trim();
            if t.is_empty() {
                None
            } else {
                Some(BilingualRun {
                    lang,
                    text: t.to_string(),
                })
            }
        })
        .collect()
}

fn script_to_lang(sc: ScriptKind) -> Language {
    match sc {
        ScriptKind::Tamil => Language::Tamil,
        ScriptKind::English => Language::English,
    }
}

fn push_script_token(
    runs: &mut Vec<(Language, String)>,
    active_lang: &mut Option<Language>,
    pending_neutral: &mut String,
    lang: Language,
    token: &str,
) {
    if token.is_empty() {
        return;
    }
    if *active_lang == Some(lang)
        && let Some((_, buf)) = runs.last_mut()
    {
        if !pending_neutral.is_empty() {
            buf.push(' ');
            buf.push_str(pending_neutral);
            pending_neutral.clear();
        }
        buf.push(' ');
        buf.push_str(token);
        return;
    }

    let mut text = String::new();
    if !pending_neutral.is_empty() {
        if *active_lang == Some(lang) || active_lang.is_none() {
            text.push_str(pending_neutral);
            text.push(' ');
            pending_neutral.clear();
        } else if let Some((_, prev_buf)) = runs.last_mut() {
            prev_buf.push(' ');
            prev_buf.push_str(pending_neutral);
            pending_neutral.clear();
        }
    }
    text.push_str(token);
    *active_lang = Some(lang);
    runs.push((lang, text));
}

#[derive(Debug, Clone, PartialEq, Eq)]
enum SynthStep {
    Speak(Vec<BilingualRun>),
    PauseMs(u32),
}

fn build_synthesis_plan(text_chunks: &[&str], lang: Language) -> Vec<SynthStep> {
    let mut grouped_lines: Vec<Vec<Vec<Vec<BilingualRun>>>> = Vec::new();
    for &raw_chunk in text_chunks {
        let mut lines_in_chunk = Vec::new();
        for raw_line in raw_chunk.lines() {
            let mut sentences_in_line = Vec::new();
            for raw_sentence in split_tts_sentences(raw_line) {
                let runs = normalize_bilingual_sentence(&raw_sentence, lang);
                if !runs.is_empty() {
                    sentences_in_line.push(runs);
                }
            }
            if !sentences_in_line.is_empty() {
                lines_in_chunk.push(sentences_in_line);
            }
        }
        if !lines_in_chunk.is_empty() {
            grouped_lines.push(lines_in_chunk);
        }
    }

    let mut plan = Vec::new();
    let group_count = grouped_lines.len();
    for (group_idx, lines) in grouped_lines.into_iter().enumerate() {
        let line_count = lines.len();
        for (line_idx, sentences) in lines.into_iter().enumerate() {
            for runs in sentences {
                plan.push(SynthStep::Speak(runs));
            }
            if line_idx + 1 < line_count {
                plan.push(SynthStep::PauseMs(LINE_PAUSE_MS));
            }
        }
        if group_idx + 1 < group_count {
            plan.push(SynthStep::PauseMs(COUPLET_PAUSE_MS));
        }
    }
    plan
}

fn normalize_bilingual_sentence(raw_sentence: &str, default_lang: Language) -> Vec<BilingualRun> {
    let runs = segment_bilingual_runs(raw_sentence, default_lang);
    let mut out: Vec<BilingualRun> = Vec::with_capacity(runs.len());
    for run in runs {
        let norm = normalize_spoken_text(&run.text, run.lang);
        if norm.is_empty() || !norm.chars().any(char::is_alphanumeric) {
            continue;
        }
        if let Some(prev) = out.last_mut()
            && prev.lang == run.lang
        {
            prev.text.push(' ');
            prev.text.push_str(&norm);
        } else {
            out.push(BilingualRun {
                lang: run.lang,
                text: norm,
            });
        }
    }
    out
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
    espeak_bin: Option<&Path>,
) -> Option<(Vec<i16>, u32)> {
    // Speed is applied via `OfflineTtsVitsModelConfig.length_scale`; keep GenerationConfig.speed = 1.0
    // so pitch stays natural and downstream playback stays 1.0x.
    let generated = if piper.has_lexicon {
        let gen_cfg = GenerationConfig {
            speed: 1.0,
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
            speed: 1.0,
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

    let pcm = scale_vits_samples_to_i16(raw_samples);
    Some((pcm, sample_rate as u32))
}

/// Apply anti-clipping peak ceiling (`<= 0.86`) and convert finite VITS `f32` samples to `i16`.
fn scale_vits_samples_to_i16(raw_samples: &[f32]) -> Vec<i16> {
    let max_abs = raw_samples
        .iter()
        .copied()
        .filter(|s| s.is_finite())
        .map(f32::abs)
        .fold(0.0_f32, f32::max);
    let gain = if max_abs > VITS_PEAK_CEILING {
        VITS_PEAK_CEILING / max_abs
    } else {
        1.0
    };
    raw_samples
        .iter()
        .copied()
        .map(|s| f32_to_i16(s * gain))
        .collect()
}

fn f32_to_i16(sample: f32) -> i16 {
    if !sample.is_finite() {
        return 0;
    }
    let scaled = (sample.clamp(-VITS_PEAK_CEILING, VITS_PEAK_CEILING) * 32767.0).round();
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

/// Return `true` if a Piper VITS `.onnx` model is present for `lang` under `models_dir/tts`.
#[must_use]
pub fn has_piper_vits_model(models_dir: &Path, lang: Language) -> bool {
    discover_piper_files(models_dir, lang).is_some()
}

/// Patch all Piper `.onnx` models and generate missing `tokens.txt` files in-place under `dir`.
///
/// Walks `dir` recursively for `.onnx` files with sibling `.onnx.json` (or `<stem>.json`)
/// configs, appends missing `sherpa-onnx` `ModelProto.metadata_props` entries (`model_type`,
/// `comment`, `language`, `voice`, `has_espeak`, `n_speakers`, `sample_rate`), and writes
/// `tokens.txt` atomically. Returns the `.onnx` paths that were modified or had tokens created.
pub fn patch_piper_onnx_dir(dir: &Path) -> Result<Vec<PathBuf>, AiError> {
    if !dir.is_dir() {
        return Err(AiError::ModelLoad(format!(
            "Piper model directory does not exist: '{}'",
            dir.display()
        )));
    }

    let mut onnx_files = Vec::new();
    collect_onnx_files_recursive(dir, &mut onnx_files)?;
    onnx_files.sort();

    let mut patched = Vec::new();
    for onnx_path in onnx_files {
        let Some(json_path) = find_sibling_piper_json(&onnx_path) else {
            continue;
        };
        let raw_json = fs::read_to_string(&json_path).map_err(|err| {
            AiError::ModelLoad(format!(
                "reading Piper config '{}': {err}",
                json_path.display()
            ))
        })?;
        let meta = parse_piper_onnx_json(&raw_json).ok_or_else(|| {
            AiError::ModelLoad(format!(
                "invalid Piper JSON config '{}'",
                json_path.display()
            ))
        })?;
        let inferred_lang = infer_language_from_piper(&onnx_path, &meta);

        let mut modified = false;
        let onnx_bytes = fs::read(&onnx_path).map_err(|err| {
            AiError::ModelLoad(format!("reading ONNX '{}': {err}", onnx_path.display()))
        })?;
        let existing_keys = scan_onnx_metadata_keys(&onnx_bytes)?;
        let missing_props =
            build_missing_piper_metadata(&existing_keys, Some(&meta), inferred_lang);
        if !missing_props.is_empty() {
            let patched_bytes = append_onnx_metadata_props(&onnx_bytes, &missing_props);
            write_file_atomic(&onnx_path, &patched_bytes).map_err(|err| {
                AiError::ModelLoad(format!(
                    "patching ONNX metadata in '{}': {err}",
                    onnx_path.display()
                ))
            })?;
            modified = true;
        }

        if let Some(tokens_body) = build_tokens_txt_body(&meta.phoneme_id_map)
            && let Some(target_tokens) = preferred_tokens_path(&onnx_path)
            && !target_tokens.is_file()
        {
            write_file_atomic(&target_tokens, tokens_body.as_bytes()).map_err(|err| {
                AiError::ModelLoad(format!(
                    "writing tokens file '{}': {err}",
                    target_tokens.display()
                ))
            })?;
            modified = true;
        }

        if modified {
            patched.push(onnx_path);
        }
    }

    Ok(patched)
}

fn collect_onnx_files_recursive(dir: &Path, out: &mut Vec<PathBuf>) -> Result<(), AiError> {
    let entries = fs::read_dir(dir).map_err(|err| {
        AiError::ModelLoad(format!("reading directory '{}': {err}", dir.display()))
    })?;
    for entry in entries.filter_map(Result::ok) {
        let path = entry.path();
        let fname = entry.file_name();
        let name = fname.to_string_lossy();
        if name.starts_with('.') || name == "espeak-ng-data" {
            continue;
        }
        if path.is_dir() {
            collect_onnx_files_recursive(&path, out)?;
        } else if path.is_file()
            && path
                .extension()
                .and_then(|e| e.to_str())
                .is_some_and(|e| e.eq_ignore_ascii_case("onnx"))
        {
            out.push(path);
        }
    }
    Ok(())
}

fn find_sibling_piper_json(onnx_path: &Path) -> Option<PathBuf> {
    let parent = onnx_path.parent()?;
    let stem = onnx_path
        .file_stem()
        .and_then(|s| s.to_str())
        .unwrap_or("model");
    let json_onnx = PathBuf::from(format!("{}.json", onnx_path.display()));
    let json_stem = parent.join(format!("{stem}.json"));
    [json_onnx, json_stem].into_iter().find(|p| p.is_file())
}

fn infer_language_from_piper(onnx_path: &Path, meta: &PiperJsonMeta) -> Language {
    let code = meta
        .language_code
        .as_deref()
        .or(meta.espeak_voice.as_deref())
        .unwrap_or_default()
        .to_ascii_lowercase();
    if code.starts_with("ta") {
        return Language::Tamil;
    }
    if code.starts_with("en") {
        return Language::English;
    }
    let name = onnx_path
        .file_name()
        .and_then(|n| n.to_str())
        .unwrap_or_default()
        .to_ascii_lowercase();
    if name.starts_with("ta_") || name.starts_with("ta-") || name.contains("valluva") {
        Language::Tamil
    } else {
        Language::English
    }
}

fn load_tts_for_language(
    models_dir: &Path,
    lang: Language,
    ai_cfg: &AiConfig,
    speed: f32,
) -> Result<Option<PiperTts>, AiError> {
    let Some(discovery) = discover_piper_files(models_dir, lang) else {
        tracing::warn!(
            %lang,
            models_dir = %models_dir.display(),
            "Piper VITS TTS model absent; falling back to espeak-ng"
        );
        return Ok(None);
    };

    let parsed_meta = discovery
        .json_path
        .as_deref()
        .and_then(|p| fs::read_to_string(p).ok())
        .and_then(|raw| parse_piper_onnx_json(&raw));

    let (compatible_onnx, tokens_path) =
        match ensure_piper_sherpa_compatible(&discovery, parsed_meta.as_ref(), lang) {
            Ok(pair) => pair,
            Err(err) => {
                tracing::warn!(
                    %lang,
                    onnx_path = %discovery.onnx_path.display(),
                    error = %err,
                    "Piper VITS compatibility setup failed; falling back to espeak-ng"
                );
                return Ok(None);
            }
        };

    let valid_phoneme_chars = read_tokens_char_set(&tokens_path);
    let espeak_voice = parsed_meta
        .as_ref()
        .and_then(|m| m.espeak_voice.clone())
        .unwrap_or_else(|| default_espeak_voice(lang).to_string());

    let clamped_speed = if speed.is_finite() {
        speed.clamp(0.5, 2.0)
    } else {
        1.0
    };
    let noise_scale = parsed_meta
        .as_ref()
        .and_then(|m| m.noise_scale)
        .filter(|v| v.is_finite() && *v > 0.0)
        .unwrap_or(0.667);
    let noise_scale_w = parsed_meta
        .as_ref()
        .and_then(|m| m.noise_w)
        .filter(|v| v.is_finite() && *v > 0.0)
        .unwrap_or(0.8);
    let base_length_scale = parsed_meta
        .as_ref()
        .and_then(|m| m.length_scale)
        .filter(|v| v.is_finite() && *v > 0.0)
        .unwrap_or(1.0);
    let effective_length_scale = base_length_scale / clamped_speed;

    let onnx_dir = discovery
        .onnx_path
        .parent()
        .unwrap_or_else(|| Path::new("."));
    let data_dir =
        discover_espeak_data_dir(onnx_dir, models_dir).and_then(|p| path_to_c_safe_string(&p).ok());

    let Ok(model_str) = path_to_c_safe_string(&compatible_onnx) else {
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
                data_dir,
                noise_scale,
                noise_scale_w,
                length_scale: effective_length_scale,
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
        tracing::warn!(
            %lang,
            onnx_path = %discovery.onnx_path.display(),
            "OfflineTts::create returned None for Piper VITS; falling back to espeak-ng"
        );
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
        loaded_speed: clamped_speed,
    }))
}

fn ensure_piper_sherpa_compatible(
    discovery: &PiperDiscovery,
    meta: Option<&PiperJsonMeta>,
    lang: Language,
) -> Result<(PathBuf, PathBuf), AiError> {
    ensure_piper_sherpa_compatible_in(discovery, meta, lang, &default_cache_root())
}

fn ensure_piper_sherpa_compatible_in(
    discovery: &PiperDiscovery,
    meta: Option<&PiperJsonMeta>,
    lang: Language,
    cache_root: &Path,
) -> Result<(PathBuf, PathBuf), AiError> {
    let tokens_body = meta.and_then(|m| build_tokens_txt_body(&m.phoneme_id_map));

    // Ensure `tokens.txt` is generated first if missing (preserves tokens extraction even when
    // testing with synthetic/corrupt ONNX bytes in writable temp directories).
    let mut use_cache_fallback = false;
    let resolved_tokens = if let Some(existing) = &discovery.tokens_path {
        existing.clone()
    } else if let Some(body) = tokens_body.as_deref() {
        let preferred = preferred_tokens_path(&discovery.onnx_path).ok_or_else(|| {
            AiError::ModelLoad("Piper ONNX path has no parent directory".to_string())
        })?;
        match write_file_atomic(&preferred, body.as_bytes()) {
            Ok(()) => preferred,
            Err(err) if is_read_only_fs_error(&err) => {
                use_cache_fallback = true;
                let cache_dir = piper_compat_cache_dir_in(cache_root, &discovery.onnx_path);
                fs::create_dir_all(&cache_dir).map_err(|ce| {
                    AiError::ModelLoad(format!(
                        "creating Piper compat cache '{}': {ce}",
                        cache_dir.display()
                    ))
                })?;
                let cached_tokens = cache_dir.join("tokens.txt");
                ensure_cached_bytes(&cached_tokens, body.as_bytes())?;
                cached_tokens
            }
            Err(err) => {
                return Err(AiError::ModelLoad(format!(
                    "writing Piper tokens '{}': {err}",
                    preferred.display()
                )));
            }
        }
    } else {
        return Err(AiError::ModelLoad(format!(
            "missing tokens.txt and phoneme_id_map for '{}'",
            discovery.onnx_path.display()
        )));
    };

    let onnx_bytes = fs::read(&discovery.onnx_path).map_err(|err| {
        AiError::ModelLoad(format!(
            "reading Piper ONNX '{}': {err}",
            discovery.onnx_path.display()
        ))
    })?;
    let existing_keys = scan_onnx_metadata_keys(&onnx_bytes)?;
    let missing_props = build_missing_piper_metadata(&existing_keys, meta, lang);
    if missing_props.is_empty() {
        return Ok((discovery.onnx_path.clone(), resolved_tokens));
    }

    if !use_cache_fallback {
        let patched_bytes = append_onnx_metadata_props(&onnx_bytes, &missing_props);
        match write_file_atomic(&discovery.onnx_path, &patched_bytes) {
            Ok(()) => return Ok((discovery.onnx_path.clone(), resolved_tokens)),
            Err(err) if is_read_only_fs_error(&err) => {
                // Fall through to cache fallback below.
            }
            Err(err) => {
                return Err(AiError::ModelLoad(format!(
                    "patching Piper ONNX '{}': {err}",
                    discovery.onnx_path.display()
                )));
            }
        }
    }

    let cache_dir = piper_compat_cache_dir_in(cache_root, &discovery.onnx_path);
    fs::create_dir_all(&cache_dir).map_err(|err| {
        AiError::ModelLoad(format!(
            "creating Piper compat cache '{}': {err}",
            cache_dir.display()
        ))
    })?;

    let onnx_name = discovery
        .onnx_path
        .file_name()
        .unwrap_or_else(|| std::ffi::OsStr::new("model.onnx"));
    let cached_onnx = cache_dir.join(onnx_name);

    let needs_cache_write = if cached_onnx.is_file() {
        fs::read(&cached_onnx)
            .ok()
            .and_then(|bytes| {
                if bytes.len() < onnx_bytes.len() {
                    return None;
                }
                scan_onnx_metadata_keys(&bytes).ok()
            })
            .is_none_or(|keys| !keys.contains("sample_rate") || !keys.contains("model_type"))
    } else {
        true
    };

    if needs_cache_write {
        let patched_bytes = append_onnx_metadata_props(&onnx_bytes, &missing_props);
        write_file_atomic(&cached_onnx, &patched_bytes).map_err(|err| {
            AiError::ModelLoad(format!(
                "writing cached Piper ONNX '{}': {err}",
                cached_onnx.display()
            ))
        })?;
    }

    Ok((cached_onnx, resolved_tokens))
}

fn ensure_cached_bytes(target: &Path, expected: &[u8]) -> Result<(), AiError> {
    if let Ok(existing) = fs::read(target)
        && existing == expected
    {
        return Ok(());
    }
    write_file_atomic(target, expected).map_err(|err| {
        AiError::ModelLoad(format!("writing cache file '{}': {err}", target.display()))
    })
}

fn is_read_only_fs_error(err: &std::io::Error) -> bool {
    err.kind() == ErrorKind::PermissionDenied || matches!(err.raw_os_error(), Some(1 | 13 | 30))
}

fn write_file_atomic(target: &Path, data: &[u8]) -> std::io::Result<()> {
    let parent = target.parent().unwrap_or_else(|| Path::new("."));
    let fname = target
        .file_name()
        .and_then(|n| n.to_str())
        .unwrap_or("file");
    let tmp_path = parent.join(format!(".kural-tmp-{}-{fname}", std::process::id()));
    if let Err(err) = fs::write(&tmp_path, data) {
        let _ = fs::remove_file(&tmp_path);
        return Err(err);
    }
    if let Err(err) = fs::rename(&tmp_path, target) {
        let _ = fs::remove_file(&tmp_path);
        return Err(err);
    }
    Ok(())
}

fn default_cache_root() -> PathBuf {
    std::env::var_os("XDG_CACHE_HOME")
        .filter(|v| !v.is_empty())
        .map(PathBuf::from)
        .or_else(|| {
            std::env::var_os("HOME")
                .filter(|v| !v.is_empty())
                .map(|h| PathBuf::from(h).join(".cache"))
        })
        .unwrap_or_else(std::env::temp_dir)
}

fn piper_compat_cache_dir_in(cache_root: &Path, onnx_path: &Path) -> PathBuf {
    let canonical = fs::canonicalize(onnx_path).unwrap_or_else(|_| onnx_path.to_path_buf());
    let mut hash: u64 = 0xcbf2_9ce4_8422_2325;
    for &b in canonical.to_string_lossy().as_bytes() {
        hash ^= u64::from(b);
        hash = hash.wrapping_mul(0x0100_0000_01b3);
    }
    let stem = onnx_path
        .file_stem()
        .and_then(|s| s.to_str())
        .unwrap_or("model");
    let sanitized_stem: String = stem
        .chars()
        .map(|c| {
            if c.is_ascii_alphanumeric() || c == '-' || c == '_' {
                c.to_ascii_lowercase()
            } else {
                '-'
            }
        })
        .collect();
    let slug = format!("{sanitized_stem}-{hash:016x}");
    cache_root
        .join("thiruvalluvar-ai")
        .join("piper-compat")
        .join(slug)
}

fn discover_espeak_data_dir(onnx_dir: &Path, models_dir: &Path) -> Option<PathBuf> {
    let mut candidates = vec![onnx_dir.join("espeak-ng-data")];
    if let Some(parent) = onnx_dir.parent() {
        candidates.push(parent.join("espeak-ng-data"));
    }
    candidates.push(models_dir.join("tts").join("espeak-ng-data"));
    candidates.push(models_dir.join("espeak-ng-data"));
    candidates.extend(
        [
            "/usr/share/espeak-ng-data",
            "/usr/local/share/espeak-ng-data",
            "/usr/lib/x86_64-linux-gnu/espeak-ng-data",
            "/usr/lib/aarch64-linux-gnu/espeak-ng-data",
            "/usr/lib/espeak-ng-data",
            "/usr/lib64/espeak-ng-data",
            "/opt/homebrew/share/espeak-ng-data",
        ]
        .into_iter()
        .map(PathBuf::from),
    );

    candidates.into_iter().find(|dir| {
        dir.is_dir() && (dir.join("phontab").is_file() || dir.join("phonindex").is_file())
    })
}

// --- Pure-Rust ONNX ModelProto.metadata_props (field 14) scanner & patcher ---

fn read_proto_varint(buf: &[u8], pos: &mut usize) -> Result<u64, AiError> {
    let mut val = 0_u64;
    let mut shift = 0_u32;
    loop {
        let b = *buf.get(*pos).ok_or_else(|| {
            AiError::ModelLoad("truncated protobuf varint in ONNX model".to_string())
        })?;
        *pos += 1;
        if shift >= 64 {
            return Err(AiError::ModelLoad(
                "overflowing protobuf varint in ONNX model".to_string(),
            ));
        }
        val |= u64::from(b & 0x7F) << shift;
        if (b & 0x80) == 0 {
            return Ok(val);
        }
        shift += 7;
    }
}

fn write_proto_varint(out: &mut Vec<u8>, mut val: u64) {
    loop {
        let byte = (val & 0x7F) as u8;
        val >>= 7;
        if val == 0 {
            out.push(byte);
            break;
        }
        out.push(byte | 0x80);
    }
}

fn skip_proto_field(buf: &[u8], pos: &mut usize, wire_type: u64) -> Result<(), AiError> {
    match wire_type {
        0 => {
            let _ = read_proto_varint(buf, pos)?;
            Ok(())
        }
        1 => {
            if buf.len().saturating_sub(*pos) < 8 {
                return Err(AiError::ModelLoad(
                    "truncated 64-bit protobuf field in ONNX".to_string(),
                ));
            }
            *pos += 8;
            Ok(())
        }
        2 => {
            let len_u64 = read_proto_varint(buf, pos)?;
            let len = usize::try_from(len_u64)
                .map_err(|_| AiError::ModelLoad("protobuf length overflow in ONNX".to_string()))?;
            let end = pos.checked_add(len).ok_or_else(|| {
                AiError::ModelLoad("protobuf length overflow in ONNX".to_string())
            })?;
            if end > buf.len() {
                return Err(AiError::ModelLoad(
                    "truncated length-delimited protobuf field in ONNX".to_string(),
                ));
            }
            *pos = end;
            Ok(())
        }
        5 => {
            if buf.len().saturating_sub(*pos) < 4 {
                return Err(AiError::ModelLoad(
                    "truncated 32-bit protobuf field in ONNX".to_string(),
                ));
            }
            *pos += 4;
            Ok(())
        }
        other => Err(AiError::ModelLoad(format!(
            "unsupported protobuf wire type {other} in ONNX model"
        ))),
    }
}

fn parse_string_string_entry_key(entry: &[u8]) -> Result<Option<String>, AiError> {
    let mut pos = 0_usize;
    let mut key: Option<String> = None;
    while pos < entry.len() {
        let tag = read_proto_varint(entry, &mut pos)?;
        let field_num = tag >> 3;
        let wire_type = tag & 0x07;
        if field_num == 0 {
            return Err(AiError::ModelLoad(
                "invalid protobuf field number 0 in ONNX StringStringEntryProto".to_string(),
            ));
        }
        if field_num == 1 && wire_type == 2 {
            let len_u64 = read_proto_varint(entry, &mut pos)?;
            let len = usize::try_from(len_u64).map_err(|_| {
                AiError::ModelLoad("protobuf string length overflow in ONNX".to_string())
            })?;
            let end = pos.checked_add(len).ok_or_else(|| {
                AiError::ModelLoad("protobuf string length overflow in ONNX".to_string())
            })?;
            let raw = entry
                .get(pos..end)
                .ok_or_else(|| AiError::ModelLoad("truncated metadata key in ONNX".to_string()))?;
            key = Some(String::from_utf8_lossy(raw).into_owned());
            pos = end;
        } else {
            skip_proto_field(entry, &mut pos, wire_type)?;
        }
    }
    Ok(key)
}

fn scan_onnx_metadata_keys(bytes: &[u8]) -> Result<HashSet<String>, AiError> {
    if bytes.is_empty() {
        return Err(AiError::ModelLoad("empty ONNX model file".to_string()));
    }
    let mut keys = HashSet::new();
    let mut pos = 0_usize;
    while pos < bytes.len() {
        let tag = read_proto_varint(bytes, &mut pos)?;
        let field_num = tag >> 3;
        let wire_type = tag & 0x07;
        if field_num == 0 {
            return Err(AiError::ModelLoad(
                "invalid protobuf field number 0 in ONNX ModelProto".to_string(),
            ));
        }
        if field_num == 14 && wire_type == 2 {
            let len_u64 = read_proto_varint(bytes, &mut pos)?;
            let len = usize::try_from(len_u64).map_err(|_| {
                AiError::ModelLoad("protobuf metadata_props length overflow".to_string())
            })?;
            let end = pos.checked_add(len).ok_or_else(|| {
                AiError::ModelLoad("protobuf metadata_props length overflow".to_string())
            })?;
            let payload = bytes.get(pos..end).ok_or_else(|| {
                AiError::ModelLoad("truncated metadata_props entry in ONNX".to_string())
            })?;
            if let Some(key) = parse_string_string_entry_key(payload)? {
                keys.insert(key);
            }
            pos = end;
        } else {
            skip_proto_field(bytes, &mut pos, wire_type)?;
        }
    }
    Ok(keys)
}

fn build_missing_piper_metadata(
    existing_keys: &HashSet<String>,
    meta: Option<&PiperJsonMeta>,
    lang: Language,
) -> Vec<(String, String)> {
    // Only consider already complete if both core sherpa-onnx VITS metadata keys exist.
    if existing_keys.contains("sample_rate") && existing_keys.contains("model_type") {
        return Vec::new();
    }

    let sample_rate = meta
        .and_then(|m| m.sample_rate)
        .filter(|&sr| sr > 0)
        .unwrap_or(DEFAULT_FALLBACK_SAMPLE_RATE_HZ);
    let n_speakers = meta
        .and_then(|m| m.num_speakers)
        .filter(|&n| n > 0)
        .unwrap_or(1);
    let voice = meta
        .and_then(|m| m.espeak_voice.as_deref())
        .or_else(|| meta.and_then(|m| m.language_code.as_deref()))
        .filter(|s| !s.is_empty())
        .unwrap_or_else(|| default_espeak_voice(lang));
    let language = meta
        .and_then(|m| m.language_name_english.as_deref())
        .filter(|s| !s.is_empty())
        .map(str::to_string)
        .unwrap_or_else(|| {
            let code = meta
                .and_then(|m| m.language_code.as_deref())
                .unwrap_or(voice)
                .to_ascii_lowercase();
            if code.starts_with("ta") {
                "Tamil".to_string()
            } else if code.starts_with("en") {
                "English".to_string()
            } else {
                code
            }
        });
    let has_espeak = match meta.and_then(|m| m.phoneme_type.as_deref()) {
        Some(pt) if pt.eq_ignore_ascii_case("text") => "0",
        _ => "1",
    };

    let desired = [
        ("model_type", "vits".to_string()),
        ("comment", "piper".to_string()),
        ("language", language),
        ("voice", voice.to_string()),
        ("has_espeak", has_espeak.to_string()),
        ("n_speakers", n_speakers.to_string()),
        ("sample_rate", sample_rate.to_string()),
    ];

    desired
        .into_iter()
        .filter(|(k, _)| !existing_keys.contains(*k))
        .map(|(k, v)| (k.to_string(), v))
        .collect()
}

fn append_onnx_metadata_props(onnx_bytes: &[u8], props: &[(String, String)]) -> Vec<u8> {
    let mut out = Vec::with_capacity(onnx_bytes.len() + props.len() * 40);
    out.extend_from_slice(onnx_bytes);
    for (key, val) in props {
        let mut entry = Vec::with_capacity(4 + key.len() + val.len());
        // field 1: key (tag = (1 << 3) | 2 = 0x0A)
        entry.push(0x0A);
        write_proto_varint(&mut entry, key.len() as u64);
        entry.extend_from_slice(key.as_bytes());
        // field 2: value (tag = (2 << 3) | 2 = 0x12)
        entry.push(0x12);
        write_proto_varint(&mut entry, val.len() as u64);
        entry.extend_from_slice(val.as_bytes());

        // ModelProto field 14: metadata_props (tag = (14 << 3) | 2 = 0x72)
        out.push(0x72);
        write_proto_varint(&mut out, entry.len() as u64);
        out.extend_from_slice(&entry);
    }
    out
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
        if let Ok(nested) = fs::read_dir(&dir) {
            let mut subdirs: Vec<PathBuf> = nested
                .filter_map(Result::ok)
                .map(|e| e.path())
                .filter(|p| {
                    p.is_dir()
                        && p.file_name()
                            .and_then(|n| n.to_str())
                            .is_some_and(|n| n != "espeak-ng-data" && !n.starts_with('.'))
                })
                .collect();
            subdirs.sort();
            for sd in subdirs {
                if let Some(found) = find_piper_in_dir(&sd, lang) {
                    return Some(found);
                }
            }
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
            dir_name == "en"
                || dir_name.starts_with("en_")
                || dir_name.starts_with("en-")
                || dir_name.contains("en_us")
        }
        Language::Tamil => {
            dir_name == "ta"
                || dir_name.starts_with("ta_")
                || dir_name.starts_with("ta-")
                || dir_name.contains("ta_in")
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

fn build_tokens_txt_body(phoneme_id_map: &HashMap<String, i32>) -> Option<String> {
    let mut pairs: Vec<(&str, i32)> = phoneme_id_map
        .iter()
        .filter(|(sym, _)| !sym.contains('\n') && !sym.contains('\r') && !sym.is_empty())
        .map(|(sym, &id)| (sym.as_str(), id))
        .collect();
    if pairs.is_empty() {
        return None;
    }
    pairs.sort_by(|a, b| a.1.cmp(&b.1).then_with(|| a.0.cmp(b.0)));

    let mut body = String::new();
    for (sym, id) in pairs {
        body.push_str(sym);
        body.push(' ');
        body.push_str(&id.to_string());
        body.push('\n');
    }
    Some(body)
}

fn preferred_tokens_path(onnx_path: &Path) -> Option<PathBuf> {
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

    if onnx_count <= 1 {
        Some(parent.join("tokens.txt"))
    } else {
        Some(parent.join(format!("{stem}.tokens.txt")))
    }
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

// --- Piper `.onnx.json` parser via `serde_json` ---

struct PiperJsonMeta {
    sample_rate: Option<u32>,
    num_speakers: Option<u32>,
    espeak_voice: Option<String>,
    language_code: Option<String>,
    language_name_english: Option<String>,
    phoneme_type: Option<String>,
    noise_scale: Option<f32>,
    length_scale: Option<f32>,
    noise_w: Option<f32>,
    phoneme_id_map: HashMap<String, i32>,
}

fn parse_piper_onnx_json(raw: &str) -> Option<PiperJsonMeta> {
    let root: JsonValue = serde_json::from_str(raw).ok()?;
    let obj = root.as_object()?;

    let sample_rate = obj
        .get("audio")
        .and_then(|v| v.get("sample_rate"))
        .and_then(JsonValue::as_u64)
        .and_then(|v| u32::try_from(v).ok());
    let num_speakers = obj
        .get("num_speakers")
        .and_then(JsonValue::as_u64)
        .and_then(|v| u32::try_from(v).ok());
    let espeak_voice = obj
        .get("espeak")
        .and_then(|v| v.get("voice"))
        .and_then(JsonValue::as_str)
        .map(str::to_string);
    let language_code = obj
        .get("language")
        .and_then(|v| v.get("code"))
        .and_then(JsonValue::as_str)
        .map(str::to_string);
    let language_name_english = obj
        .get("language")
        .and_then(|v| v.get("name_english"))
        .and_then(JsonValue::as_str)
        .map(str::to_string);
    let phoneme_type = obj
        .get("phoneme_type")
        .and_then(JsonValue::as_str)
        .map(str::to_string);

    let inference = obj.get("inference");
    let noise_scale = inference
        .and_then(|v| v.get("noise_scale"))
        .and_then(JsonValue::as_f64)
        .map(|v| v as f32);
    let length_scale = inference
        .and_then(|v| v.get("length_scale"))
        .and_then(JsonValue::as_f64)
        .map(|v| v as f32);
    let noise_w = inference
        .and_then(|v| v.get("noise_w"))
        .and_then(JsonValue::as_f64)
        .map(|v| v as f32);

    let mut phoneme_id_map = HashMap::new();
    if let Some(map_obj) = obj.get("phoneme_id_map").and_then(JsonValue::as_object) {
        for (sym, ids_val) in map_obj {
            if let Some(id) = ids_val
                .as_array()
                .and_then(|arr| arr.first())
                .and_then(JsonValue::as_i64)
                .and_then(|v| i32::try_from(v).ok())
            {
                phoneme_id_map.insert(sym.clone(), id);
            }
        }
    }

    Some(PiperJsonMeta {
        sample_rate,
        num_speakers,
        espeak_voice,
        language_code,
        language_name_english,
        phoneme_type,
        noise_scale,
        length_scale,
        noise_w,
        phoneme_id_map,
    })
}

/// Detect whether arbitrary user-facing text should be spoken in Tamil or English.
///
/// Counts Tamil Unicode block letters (`U+0B80..=U+0BFF`) against ASCII letters so short
/// bilingual strings such as `"திருக்குறள் AI பற்றி"` stay on Tamil TTS instead of flipping to
/// English on a single acronym.
pub fn detect_text_language(text: &str, fallback: Language) -> Language {
    let mut tamil_chars = 0_usize;
    let mut ascii_alpha = 0_usize;
    for ch in text.chars() {
        if ('\u{0B80}'..='\u{0BFF}').contains(&ch) {
            tamil_chars += 1;
        } else if ch.is_ascii_alphabetic() {
            ascii_alpha += 1;
        }
    }
    if tamil_chars == 0 && ascii_alpha == 0 {
        fallback
    } else if tamil_chars > 0 && (ascii_alpha == 0 || tamil_chars * 2 >= ascii_alpha) {
        Language::Tamil
    } else {
        Language::English
    }
}

fn append_resampled_i16(out: &mut Vec<i16>, samples: &[i16], src_sr: u32, dst_sr: u32) {
    if samples.is_empty() || src_sr == 0 || dst_sr == 0 {
        return;
    }
    if src_sr == dst_sr {
        out.extend_from_slice(samples);
        return;
    }
    let out_len = (((samples.len() as u64) * u64::from(dst_sr)) / u64::from(src_sr)) as usize;
    if out_len == 0 {
        return;
    }
    let last_src = samples.len() - 1;
    for i in 0..out_len {
        let src_pos = (i as f64) * f64::from(src_sr) / f64::from(dst_sr);
        let idx0 = (src_pos.floor() as usize).min(last_src);
        let idx1 = (idx0 + 1).min(last_src);
        let frac = src_pos - (idx0 as f64);
        let s0 = f64::from(samples[idx0]);
        let s1 = f64::from(samples[idx1]);
        let interp = s0 + (s1 - s0) * frac;
        out.push(
            interp
                .round()
                .clamp(f64::from(i16::MIN), f64::from(i16::MAX)) as i16,
        );
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use kural_core::is_nfc;
    use tempfile::TempDir;

    #[test]
    fn normalize_spoken_text_handles_nfc_hashes_numerals_and_punctuation() {
        let en = normalize_spoken_text(
            "  Kural #481:   Virtue's **power**... [note] 🔥 \n\n  Speak!!  ",
            Language::English,
        );
        assert_eq!(en, "Kural 481: Virtue's power. note Speak!");

        // Decomposed Tamil 'கொ' (`க` + `ெ` + `ா`) must normalize to NFC `'கொ'`, preserve vowel signs & pulli,
        // and expand Tamil numbers naturally.
        let ta_raw = "  குறள் #1: க\u{0BC6}\u{0BBE}டு, ...  உலகு!! **சிறப்பு** 🙏 \n";
        let ta = normalize_spoken_text(ta_raw, Language::Tamil);
        assert!(is_nfc(&ta));
        assert_eq!(ta, "குறள் ஒன்று: கொடு, உலகு! சிறப்பு");

        // Tamil numeral expansion across key ranges.
        assert_eq!(
            normalize_spoken_text("குறள் #1330", Language::Tamil),
            "குறள் ஆயிரத்து முன்னூற்று முப்பது"
        );
        assert_eq!(
            normalize_spoken_text("0 10 19 25 100 481 1.25", Language::Tamil),
            "பூஜ்ஜியம் பத்து பத்தொன்பது இருபத்து ஐந்து நூறு நானூற்று எண்பத்து ஒன்று ஒன்று புள்ளி இரண்டு ஐந்து"
        );

        // Preserve decimal dots and numeric separators in English.
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
        let loaded = load_tts_for_language(dir.path(), Language::Tamil, &AiConfig::default(), 1.0)
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
            load_tts_for_language(dir.path(), Language::English, &AiConfig::default(), 1.0)
                .unwrap()
                .is_none()
        );
        assert!(
            load_tts_for_language(dir.path(), Language::Tamil, &AiConfig::default(), 1.0)
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

    #[test]
    fn detect_text_language_handles_tamil_acronyms_and_english() {
        assert_eq!(
            detect_text_language("திருக்குறள் AI பற்றி", Language::English),
            Language::Tamil
        );
        assert_eq!(
            detect_text_language("AI தமிழ் விளக்கம்", Language::English),
            Language::Tamil
        );
        assert_eq!(
            detect_text_language("What does அறம் mean in Kural?", Language::Tamil),
            Language::English
        );
        assert_eq!(
            detect_text_language("Virtue brings honour and wisdom.", Language::Tamil),
            Language::English
        );
        assert_eq!(
            detect_text_language("1330", Language::Tamil),
            Language::Tamil
        );
    }

    #[test]
    fn synthesize_streaming_collecting_caches_only_uninterrupted_audio() {
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

        let (tx, mut rx) = mpsc::channel::<PcmChunk>(8);
        let cancel = CancellationToken::new();
        let drain_task = std::thread::spawn(move || {
            let mut count = 0usize;
            while let Some(chunk) = rx.blocking_recv() {
                count += chunk.samples.len();
            }
            count
        });
        let collected = engine
            .synthesize_streaming_collecting(
                &["Virtue.", "Wisdom."],
                Language::English,
                1.0,
                &cancel,
                &tx,
            )
            .expect("collect")
            .expect("some collected pcm");
        drop(tx);
        let streamed_len = drain_task.join().expect("join drain");
        assert!(!collected.0.is_empty());
        assert_eq!(collected.0.len(), streamed_len);
        assert!(collected.1 >= 16_000);

        let (tx2, rx2) = mpsc::channel(1);
        let cancel2 = CancellationToken::new();
        cancel2.cancel();
        drop(rx2);
        let err = engine
            .synthesize_streaming_collecting(
                &["Aborted text."],
                Language::English,
                1.0,
                &cancel2,
                &tx2,
            )
            .unwrap_err();
        assert!(matches!(err, AiError::Cancelled));
    }

    fn build_synthetic_model_proto() -> Vec<u8> {
        let mut out = vec![
            // field 1: ir_version = 8 (varint)
            0x08, 0x08, // field 2: producer_name = "pytorch" (length-delimited)
            0x12, 7,
        ];
        out.extend_from_slice(b"pytorch");
        // field 7: graph = empty GraphProto (length-delimited)
        out.push(0x3A);
        out.push(0x00);
        // field 9: model_version = 1 (64-bit wire type 1 to test skip_proto_field)
        out.push((9 << 3) | 1);
        out.extend_from_slice(&1_u64.to_le_bytes());
        // field 10: dummy 32-bit wire type 5 to test skip_proto_field
        out.push((10 << 3) | 5);
        out.extend_from_slice(&0_u32.to_le_bytes());
        out
    }

    #[test]
    fn onnx_metadata_props_patcher_is_valid_and_idempotent() {
        let dir = TempDir::new().expect("tempdir");
        let ta_dir = dir.path().join("tts/ta");
        fs::create_dir_all(&ta_dir).expect("mkdir");

        let onnx_path = ta_dir.join("ta_IN-ValluvarNeural-medium.onnx");
        let raw_proto = build_synthetic_model_proto();
        fs::write(&onnx_path, &raw_proto).expect("write proto");
        fs::write(
            ta_dir.join("ta_IN-ValluvarNeural-medium.onnx.json"),
            r#"{
                "audio": { "sample_rate": 22050 },
                "espeak": { "voice": "ta" },
                "language": { "code": "ta" },
                "inference": { "noise_scale": 0.667, "length_scale": 1, "noise_w": 0.8 },
                "num_speakers": 1,
                "phoneme_type": "espeak",
                "phoneme_id_map": { "_": [0], "^": [1], "$": [2], " ": [3], "a": [14] }
            }"#,
        )
        .expect("write json");

        let initial_keys = scan_onnx_metadata_keys(&raw_proto).expect("scan empty");
        assert!(initial_keys.is_empty());

        let patched1 = patch_piper_onnx_dir(dir.path()).expect("first patch");
        assert_eq!(patched1, vec![onnx_path.clone()]);
        assert!(ta_dir.join("tokens.txt").is_file());

        let patched_bytes = fs::read(&onnx_path).expect("read patched");
        let keys = scan_onnx_metadata_keys(&patched_bytes).expect("scan patched");
        assert!(keys.contains("model_type"));
        assert!(keys.contains("comment"));
        assert!(keys.contains("language"));
        assert!(keys.contains("voice"));
        assert!(keys.contains("has_espeak"));
        assert!(keys.contains("n_speakers"));
        assert!(keys.contains("sample_rate"));

        // Second pass must be idempotent (zero files modified, bytes unchanged).
        let patched2 = patch_piper_onnx_dir(dir.path()).expect("second patch");
        assert!(patched2.is_empty());
        assert_eq!(fs::read(&onnx_path).expect("reread"), patched_bytes);
    }

    #[cfg(unix)]
    #[test]
    fn readonly_model_dir_falls_back_to_cache_dir_for_onnx_and_tokens() {
        use std::os::unix::fs::PermissionsExt;

        let dir = TempDir::new().expect("tempdir");
        let ro_dir = dir.path().join("readonly_ta");
        let cache_root = dir.path().join("xdg_cache");
        fs::create_dir_all(&ro_dir).expect("mkdir ro");
        fs::create_dir_all(&cache_root).expect("mkdir cache");

        let onnx_path = ro_dir.join("ta_IN-ValluvarNeural-medium.onnx");
        let json_path = ro_dir.join("ta_IN-ValluvarNeural-medium.onnx.json");
        fs::write(&onnx_path, build_synthetic_model_proto()).expect("write onnx");
        fs::write(
            &json_path,
            r#"{
                "audio": { "sample_rate": 22050 },
                "espeak": { "voice": "ta" },
                "language": { "code": "ta" },
                "num_speakers": 1,
                "phoneme_id_map": { "_": [0], " ": [3], "a": [14] }
            }"#,
        )
        .expect("write json");

        let orig_perms = fs::metadata(&ro_dir).expect("meta").permissions();
        let mut ro_perms = orig_perms.clone();
        ro_perms.set_mode(0o555);
        fs::set_permissions(&ro_dir, ro_perms).expect("chmod 0555");

        let discovery = PiperDiscovery {
            onnx_path: onnx_path.clone(),
            tokens_path: None,
            lexicon_path: None,
            json_path: Some(json_path),
        };
        let raw_json = fs::read_to_string(discovery.json_path.as_ref().unwrap()).unwrap();
        let parsed = parse_piper_onnx_json(&raw_json).unwrap();

        let res = ensure_piper_sherpa_compatible_in(
            &discovery,
            Some(&parsed),
            Language::Tamil,
            &cache_root,
        );

        // Restore writable permissions so TempDir cleanup succeeds cleanly.
        let _ = fs::set_permissions(&ro_dir, orig_perms);

        let (cached_onnx, cached_tokens) = res.expect("cache fallback should succeed");
        assert!(cached_onnx.starts_with(&cache_root));
        assert!(cached_tokens.starts_with(&cache_root));
        assert!(cached_onnx.is_file());
        assert!(cached_tokens.is_file());

        let cached_keys = scan_onnx_metadata_keys(&fs::read(&cached_onnx).unwrap()).unwrap();
        assert!(cached_keys.contains("sample_rate"));
        assert!(cached_keys.contains("model_type"));
    }

    #[test]
    fn segment_bilingual_runs_splits_scripts_and_preserves_neutral_context() {
        let runs = segment_bilingual_runs("திருக்குறள் AI பற்றி", Language::Tamil);
        assert_eq!(
            runs,
            vec![
                BilingualRun {
                    lang: Language::Tamil,
                    text: "திருக்குறள்".to_string()
                },
                BilingualRun {
                    lang: Language::English,
                    text: "AI".to_string()
                },
                BilingualRun {
                    lang: Language::Tamil,
                    text: "பற்றி".to_string()
                },
            ]
        );

        let runs2 = segment_bilingual_runs("Chapter 12 பற்றி", Language::Tamil);
        assert_eq!(
            runs2,
            vec![
                BilingualRun {
                    lang: Language::English,
                    text: "Chapter 12".to_string()
                },
                BilingualRun {
                    lang: Language::Tamil,
                    text: "பற்றி".to_string()
                },
            ]
        );
    }

    #[test]
    fn vits_peak_ceiling_clamps_hot_samples_and_guards_non_finite() {
        let raw = [
            0.0_f32,
            1.5,
            -1.2,
            f32::NAN,
            f32::INFINITY,
            f32::NEG_INFINITY,
            0.3,
        ];
        let pcm = scale_vits_samples_to_i16(&raw);
        assert_eq!(pcm.len(), raw.len());
        assert_eq!(pcm[3], 0);
        assert_eq!(pcm[4], 0);
        assert_eq!(pcm[5], 0);
        let peak = pcm
            .iter()
            .map(|&s| (f32::from(s) / 32767.0).abs())
            .fold(0.0_f32, f32::max);
        assert!(peak <= 0.86, "expected peak <= 0.86, got {peak}");
        assert!(
            peak > 0.84,
            "expected normalized peak near ceiling, got {peak}"
        );
    }

    #[test]
    fn couplet_and_line_pauses_use_expected_durations() {
        assert_eq!(COUPLET_PAUSE_MS, 350);
        assert_eq!(LINE_PAUSE_MS, 150);
        let plan =
            build_synthesis_plan(&["அகர முதல\nபகவன் முதற்றே", "பொருள் விளக்கம்"], Language::Tamil);
        assert_eq!(plan.len(), 5);
        assert_eq!(plan[1], SynthStep::PauseMs(LINE_PAUSE_MS));
        assert_eq!(plan[3], SynthStep::PauseMs(COUPLET_PAUSE_MS));
    }
}
