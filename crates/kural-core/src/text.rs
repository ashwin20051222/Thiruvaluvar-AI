use std::collections::HashSet;
use unicode_normalization::{IsNormalized, UnicodeNormalization, is_nfc_quick};
use unicode_segmentation::UnicodeSegmentation;

/// Convert a string into Unicode NFC normalized form.
pub fn normalize_nfc(input: &str) -> String {
    input.nfc().collect()
}

/// Trim surrounding whitespace and convert to Unicode NFC normalized form.
pub fn normalize_nfc_trimmed(input: &str) -> String {
    input.trim().nfc().collect()
}

/// Return true if `input` is already in Unicode NFC normalized form.
pub fn is_nfc(input: &str) -> bool {
    matches!(is_nfc_quick(input.chars()), IsNormalized::Yes)
        || input.nfc().eq(input.chars())
}

/// Return the first extended Unicode grapheme cluster of `text` (trimmed),
/// preserving compound Tamil letters like `கெ`, `கு`, `எ`, `கொ`, or `க்ஷ`.
pub fn opening_grapheme_ta(text: &str) -> Option<String> {
    let normalized = normalize_nfc_trimmed(text);
    normalized
        .graphemes(true)
        .next()
        .map(ToString::to_string)
}

fn trim_edge_punctuation(token: &str) -> &str {
    token.trim_matches(|c: char| !c.is_alphabetic() && !is_tamil_combining_mark(c))
}

fn is_tamil_combining_mark(c: char) -> bool {
    ('\u{0B82}'..='\u{0BCD}').contains(&c) || c == '\u{0BD7}'
}

/// Extract ordered unique Tamil word tokens from a Kural's two lines (NFC normalized, punctuation-trimmed).
pub fn tokenize_kural_words(line1_ta: &str, line2_ta: &str) -> Vec<String> {
    let mut seen = HashSet::new();
    let mut out = Vec::new();
    for line in [line1_ta, line2_ta] {
        let norm_line = normalize_nfc_trimmed(line);
        for raw in norm_line.split_whitespace() {
            let trimmed = trim_edge_punctuation(raw);
            if !trimmed.is_empty() && seen.insert(trimmed.to_string()) {
                out.push(trimmed.to_string());
            }
        }
    }
    out
}

/// Check that `candidate` is a single non-empty word occurring verbatim in `line1_ta` or `line2_ta`
/// (matching either the exact whitespace-separated token or its punctuation-trimmed form).
pub fn contains_exact_kural_word(line1_ta: &str, line2_ta: &str, candidate: &str) -> bool {
    let norm_cand = normalize_nfc_trimmed(candidate);
    if norm_cand.is_empty() || norm_cand.split_whitespace().count() != 1 {
        return false;
    }

    for line in [line1_ta, line2_ta] {
        let norm_line = normalize_nfc_trimmed(line);
        for raw in norm_line.split_whitespace() {
            if raw == norm_cand || trim_edge_punctuation(raw) == norm_cand {
                return true;
            }
        }
    }
    false
}

/// Build a TTS-ready Tamil script with natural poetic pause between line 1 and line 2.
pub fn build_tts_kural_ta(line1_ta: &str, line2_ta: &str) -> String {
    let l1 = normalize_nfc_trimmed(line1_ta);
    let l2 = normalize_nfc_trimmed(line2_ta);
    let l2_clean = if l2.ends_with('.') || l2.ends_with('!') || l2.ends_with('?') {
        l2
    } else {
        format!("{l2}.")
    };
    format!("{l1}, ... {l2_clean}")
}

/// Build a TTS-ready English script with clean punctuation and poetic pause.
pub fn build_tts_kural_en(translation_en: &str, couplet_en: &str) -> String {
    let trans = normalize_nfc_trimmed(translation_en);
    let couplet = normalize_nfc_trimmed(couplet_en);
    let primary = if !trans.is_empty() { trans } else { couplet };
    if primary.ends_with('.') || primary.ends_with('!') || primary.ends_with('?') {
        primary
    } else {
        format!("{primary}.")
    }
}

/// Verify phonetic alignment between Tamil opening character of `line1_ta` and `transliteration1`,
/// plus explicit assertions for Kural #1 (`Akara...`) and Kural #2 (`Katradhanaal...`).
pub fn check_transliteration_phonetic_sanity(
    number: u16,
    line1_ta: &str,
    transliteration1: &str,
) -> Result<(), String> {
    let l1 = normalize_nfc_trimmed(line1_ta);
    let tr1 = normalize_nfc_trimmed(transliteration1);
    if tr1.is_empty() {
        return Err(format!("kural #{number} has empty transliteration1"));
    }

    if number == 1 && !tr1.starts_with("Akara") {
        return Err(format!(
            "kural #1 transliteration1 must start with 'Akara', found '{tr1}'"
        ));
    }
    if number == 2 && !tr1.starts_with("Katradhanaal") {
        return Err(format!(
            "kural #2 transliteration1 must start with 'Katradhanaal', found '{tr1}'"
        ));
    }

    let first_tamil_char = l1
        .chars()
        .next()
        .ok_or_else(|| format!("kural #{number} has empty line1_ta"))?;
    let first_latin_char = tr1
        .chars()
        .find(char::is_ascii_alphabetic)
        .map(|c| c.to_ascii_lowercase())
        .ok_or_else(|| format!("kural #{number} transliteration1 lacks ASCII letters: '{tr1}'"))?;

    let allowed_latin: &[char] = match first_tamil_char {
        'அ' | 'ஆ' | 'ஐ' => &['a'],
        'இ' => &['i'],
        'ஈ' | 'எ' | 'ஏ' => &['e'],
        'உ' => &['u'],
        'ஊ' | 'ஒ' | 'ஓ' => &['o'],
        'க' => &['k', 'g', 'c'],
        'ச' => &['s', 'c'],
        'ஞ' => &['g', 'n'],
        'த' => &['t', 'd'],
        'ந' => &['n'],
        'ப' => &['p', 'b'],
        'ம' => &['m'],
        'ய' => &['y'],
        'வ' => &['v', 'w'],
        'ஔ' => &['o', 'a'],
        other => {
            return Err(format!(
                "kural #{number} starts with unexpected Tamil character '{other}'"
            ));
        }
    };

    if !allowed_latin.contains(&first_latin_char) {
        return Err(format!(
            "kural #{number} phonetic mismatch: line1 starts with '{first_tamil_char}', transliteration1 starts with '{first_latin_char}' ('{tr1}')"
        ));
    }

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_opening_grapheme_preserves_compound_tamil_letters() {
        assert_eq!(opening_grapheme_ta("கெடுப்பது").as_deref(), Some("கெ"));
        assert_eq!(opening_grapheme_ta("குணம்").as_deref(), Some("கு"));
        assert_eq!(opening_grapheme_ta("எப்பொருள்").as_deref(), Some("எ"));
        assert_eq!(opening_grapheme_ta("கொக்கொக்க").as_deref(), Some("கொ"));
        assert_eq!(opening_grapheme_ta("  அகர முதல").as_deref(), Some("அ"));
    }

    #[test]
    fn test_nfc_normalization_and_check() {
        // Decomposed 'கெ' = 'க' + U+0BC6 (already NFC in Tamil for ெ, whereas கொ = க + U+0BCA vs க + ெ + ா)
        let decomposed = "க\u{0BC6}\u{0BBE}டு";
        assert!(!is_nfc(decomposed));
        let normalized = normalize_nfc(decomposed);
        assert!(is_nfc(&normalized));
        assert_eq!(normalized, "கொடு");
        assert_eq!(opening_grapheme_ta(decomposed).as_deref(), Some("கொ"));
    }

    #[test]
    fn test_contains_exact_kural_word() {
        let l1 = "அகர முதல எழுத்தெல்லாம் ஆதி";
        let l2 = "பகவன் முதற்றே உலகு.";
        assert!(contains_exact_kural_word(l1, l2, "அகர"));
        assert!(contains_exact_kural_word(l1, l2, "உலகு"));
        assert!(contains_exact_kural_word(l1, l2, "உலகு."));
        assert!(!contains_exact_kural_word(l1, l2, "உல"));
        assert!(!contains_exact_kural_word(l1, l2, "அகர முதல"));
        assert!(!contains_exact_kural_word(l1, l2, ""));
    }

    #[test]
    fn test_transliteration_sanity_catches_kural_2_carrying_kural_1() {
        assert!(
            check_transliteration_phonetic_sanity(
                1,
                "அகர முதல எழுத்தெல்லாம் ஆதி",
                "Akara Mudhala Ezhuththellaam Aadhi"
            )
            .is_ok()
        );
        assert!(
            check_transliteration_phonetic_sanity(
                2,
                "கற்றதனால் ஆய பயனென்கொல் வாலறிவன்",
                "Katradhanaal Aaya Payanenkol Vaalarivan"
            )
            .is_ok()
        );
        assert!(
            check_transliteration_phonetic_sanity(
                2,
                "கற்றதனால் ஆய பயனென்கொல் வாலறிவன்",
                "Akara Mudhala Ezhuththellaam Aadhi"
            )
            .is_err()
        );
    }
}

