//! Bilingual copy helpers, sentence-boundary pagination (`480x320` zero-scroll), and
//! scholar label helpers shared across all screens and modals.

use kural_core::{AgeGroup, FullKural, Language, ResearchTheme, Scholar};

/// Return `ta` or `en` matching `lang`.
#[must_use]
pub fn tr<'a>(lang: Language, ta: &'a str, en: &'a str) -> &'a str {
    match lang {
        Language::Tamil => ta,
        Language::English => en,
    }
}

/// Format bilingual Scholar pill label.
#[must_use]
pub fn scholar_label(scholar: Scholar, lang: Language) -> &'static str {
    match (scholar, lang) {
        (Scholar::MuVa, Language::Tamil) => "மு.வ.",
        (Scholar::MuVa, Language::English) => "Mu. Va.",
        (Scholar::Solomon, Language::Tamil) => "சாலமன்",
        (Scholar::Solomon, Language::English) => "Solomon",
        (Scholar::Kalaignar, Language::Tamil) => "கலைஞர்",
        (Scholar::Kalaignar, Language::English) => "Kalaignar",
        (Scholar::Parimel, Language::Tamil) => "பரிமேலழகர்",
        (Scholar::Parimel, Language::English) => "Parimel",
        (Scholar::English, _) => "English",
    }
}

/// Ordered 5 scholars for Adult Mode tabs & Pi cycle pill.
pub const ALL_SCHOLARS: [Scholar; 5] = [
    Scholar::MuVa,
    Scholar::Solomon,
    Scholar::Kalaignar,
    Scholar::Parimel,
    Scholar::English,
];

/// Next scholar in cycle order (wrapping cleanly).
#[must_use]
pub fn next_scholar(cur: Scholar) -> Scholar {
    let idx = ALL_SCHOLARS.iter().position(|&s| s == cur).unwrap_or(0);
    ALL_SCHOLARS[(idx + 1) % ALL_SCHOLARS.len()]
}

/// Return English meaning (`explanation_en` if non-empty, else `translation_en`).
fn english_meaning(full: &FullKural) -> &str {
    if full.kural.explanation_en.trim().is_empty() {
        &full.kural.translation_en
    } else {
        &full.kural.explanation_en
    }
}

/// Return active commentary body text for `full` respecting `effective_scholar` + `lang`.
#[must_use]
pub fn commentary_text(full: &FullKural, effective: Scholar, lang: Language) -> &str {
    if lang == Language::English {
        return english_meaning(full);
    }
    match effective {
        Scholar::MuVa => &full.kural.meaning_mu_va,
        Scholar::Solomon => &full.kural.meaning_solomon,
        Scholar::Kalaignar => &full.kural.meaning_kalaignar,
        Scholar::Parimel => full
            .kural
            .meaning_parimel
            .as_deref()
            .unwrap_or(&full.kural.meaning_mu_va),
        Scholar::English => english_meaning(full),
    }
}

/// Return English couplet display text (`translation_en` if non-empty, else `couplet_en`),
/// normalizing redundant triple-quotes (`'''` -> `'`) without mutating DB data.
#[must_use]
pub fn english_couplet_display(full: &FullKural) -> String {
    let raw = if full.kural.translation_en.trim().is_empty() {
        full.kural.couplet_en.trim()
    } else {
        full.kural.translation_en.trim()
    };
    raw.replace("'''", "'")
}

/// Return age badge title.
#[must_use]
pub fn age_label(age: AgeGroup, lang: Language) -> &'static str {
    match (age, lang) {
        (AgeGroup::Kids, Language::Tamil) => "சிறுவர் (6–12)",
        (AgeGroup::Kids, Language::English) => "Child (6–12)",
        (AgeGroup::Youth, Language::Tamil) => "இளைஞர் (13–17)",
        (AgeGroup::Youth, Language::English) => "Teen (13–17)",
        (AgeGroup::Adult, Language::Tamil) => "பெரியோர் (18+)",
        (AgeGroup::Adult, Language::English) => "Adult (18+)",
    }
}

/// Research Theme bilingual label & short summary.
#[must_use]
pub fn research_theme_label(theme: ResearchTheme, lang: Language) -> (&'static str, &'static str) {
    match (theme, lang) {
        (ResearchTheme::Virtue, Language::Tamil) => ("அறவியல்", "அறத்துப்பால் விழுமியங்கள்"),
        (ResearchTheme::Virtue, Language::English) => ("Virtue Ethics", "Foundations of Aram"),
        (ResearchTheme::Philosophy, Language::Tamil) => ("மெய்யியல்", "உண்மை & அறிவுடைமை"),
        (ResearchTheme::Philosophy, Language::English) => ("Philosophy", "Discernment & Mind"),
        (ResearchTheme::Polity, Language::Tamil) => ("அரசியல் & மேலாண்மை", "ஆட்சி, காலம், வினைத்திறன்"),
        (ResearchTheme::Polity, Language::English) => ("Statecraft", "Leadership & Timing"),
        (ResearchTheme::LoveEthics, Language::Tamil) => ("காதல் & இல்லறம்", "அன்பின் ஆழமும் பண்பாடும்"),
        (ResearchTheme::LoveEthics, Language::English) => ("Love & Grace", "Inbam & Human Bond"),
    }
}

/// Mask `game_blank_word_ta` cleanly with `_____` in Kids Word Game display without touching DB.
#[must_use]
pub fn mask_game_blank(line1_ta: &str, line2_ta: &str, blank_word: &str) -> (String, String) {
    let target = blank_word.trim();
    if target.is_empty() {
        return (line1_ta.to_string(), line2_ta.to_string());
    }
    let mask_line = |line: &str| -> String {
        line.split_whitespace()
            .map(|tok| {
                let clean =
                    tok.trim_matches(|c: char| matches!(c, '.' | ',' | ';' | ':' | '!' | '?'));
                if clean == target {
                    tok.replacen(clean, "_____", 1)
                } else {
                    tok.to_string()
                }
            })
            .collect::<Vec<_>>()
            .join(" ")
    };
    (mask_line(line1_ta), mask_line(line2_ta))
}

/// Sentence-boundary paginator guaranteeing whole sentences & non-clipped cards on `480x320`.
#[must_use]
pub fn paginate_sentences(text: &str, max_chars: usize) -> Vec<String> {
    let trimmed = text.trim();
    if trimmed.is_empty() {
        return vec![String::new()];
    }
    let cap = max_chars.max(40);
    let mut sentences = Vec::new();
    let mut buf = String::new();
    for ch in trimmed.chars() {
        buf.push(ch);
        if matches!(ch, '.' | '!' | '?' | '।' | '॥' | '\n') && !buf.trim().is_empty() {
            sentences.push(buf.trim().to_string());
            buf.clear();
        }
    }
    if !buf.trim().is_empty() {
        sentences.push(buf.trim().to_string());
    }
    if sentences.is_empty() {
        return vec![trimmed.to_string()];
    }

    let mut pages = Vec::new();
    let mut current = String::new();
    for s in sentences {
        let cur_chars = current.chars().count();
        let next_chars = s.chars().count();
        if !current.is_empty() && cur_chars + 1 + next_chars > cap {
            pages.push(current);
            current = s;
        } else {
            if !current.is_empty() {
                current.push(' ');
            }
            current.push_str(&s);
        }
    }
    if !current.is_empty() {
        pages.push(current);
    }
    pages
}

#[cfg(test)]
mod tests {
    use super::*;
    use kural_core::{Adhikaram, Division, Iyal, KuralCore};

    fn sample_full_kural() -> FullKural {
        FullKural {
            kural: KuralCore {
                number: 1,
                division_id: 1,
                iyal_id: 1,
                adhikaram_id: 1,
                line1_ta: "அகர முதல எழுத்தெல்லாம் ஆதி".to_string(),
                line2_ta: "பகவன் முதற்றே உலகு.".to_string(),
                opening_letter_ta: "அ".to_string(),
                transliteration1: "Agara mudhala ezhuthellaam aadhi".to_string(),
                transliteration2: "Bhagavan mudhatre ulagu".to_string(),
                meaning_mu_va: "மு.வ. தமிழ் உரை".to_string(),
                meaning_solomon: "சாலமன் தமிழ் உரை".to_string(),
                meaning_kalaignar: "கலைஞர் தமிழ் உரை".to_string(),
                meaning_parimel: None,
                translation_en: "As letters all begin with '''A''', the world begins with God."
                    .to_string(),
                explanation_en: "God is the primal source of the universe.".to_string(),
                couplet_en: "A is the first of letters.".to_string(),
                tts_kural_ta: String::new(),
                tts_kural_en: String::new(),
            },
            division: Division {
                id: 1,
                name_ta: "அறத்துப்பால்".to_string(),
                alt_name_ta: None,
                name_en: "Virtue".to_string(),
                transliteration: "Arathuppal".to_string(),
                kural_start: 1,
                kural_end: 380,
            },
            iyal: Iyal {
                id: 1,
                division_id: 1,
                name_ta: "பாயிரவியல்".to_string(),
                alt_name_ta: None,
                name_en: "Prologue".to_string(),
                transliteration: "Payiraviyal".to_string(),
                chapter_start: 1,
                chapter_end: 4,
            },
            adhikaram: Adhikaram {
                id: 1,
                division_id: 1,
                iyal_id: 1,
                name_ta: "கடவுள் வாழ்த்து".to_string(),
                name_en: "Praise of God".to_string(),
                transliteration: "Kadavul Vaazhthu".to_string(),
                kural_start: 1,
                kural_end: 10,
            },
            child: None,
            youth: None,
            adult: None,
            tags: Vec::new(),
        }
    }

    #[test]
    fn sentence_paginator_and_word_game_mask_preserve_integrity() {
        let pages = paginate_sentences(
            "First calm sentence. Second helpful thought about kindness. Third closing reflection.",
            55,
        );
        assert!(pages.len() >= 2);
        assert!(pages.iter().all(|p| !p.is_empty()));

        let (l1, l2) = mask_game_blank("அகர முதல எழுத்தெல்லாம் ஆதி", "பகவன் முதற்றே உலகு.", "உலகு");
        assert!(l1.contains("அகர"));
        assert!(l2.contains("_____."));
        assert!(!l2.contains("உலகு"));
    }

    #[test]
    fn english_mode_returns_english_commentary_and_clean_couplet() {
        let mut full = sample_full_kural();

        // Even when effective scholar is MuVa, English mode returns English explanation.
        assert_eq!(
            commentary_text(&full, Scholar::MuVa, Language::English),
            "God is the primal source of the universe."
        );
        // Tamil mode preserves MuVa commentary.
        assert_eq!(
            commentary_text(&full, Scholar::MuVa, Language::Tamil),
            "மு.வ. தமிழ் உரை"
        );

        // Cleans ''' -> ' without mutating DB data.
        assert_eq!(
            english_couplet_display(&full),
            "As letters all begin with 'A', the world begins with God."
        );

        // Fallback to translation_en in commentary_text when explanation_en is blank,
        // and fallback to couplet_en in english_couplet_display when translation_en is blank.
        full.kural.explanation_en = "   ".to_string();
        assert_eq!(
            commentary_text(&full, Scholar::MuVa, Language::English),
            "As letters all begin with '''A''', the world begins with God."
        );
        full.kural.translation_en.clear();
        full.kural.couplet_en = "'''A''' leads all letters.".to_string();
        assert_eq!(english_couplet_display(&full), "'A' leads all letters.");
    }
}
