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

/// Return active commentary body text for `full` respecting `effective_scholar` + `lang`.
#[must_use]
pub fn commentary_text(full: &FullKural, effective: Scholar, _lang: Language) -> &str {
    match effective {
        Scholar::MuVa => &full.kural.meaning_mu_va,
        Scholar::Solomon => &full.kural.meaning_solomon,
        Scholar::Kalaignar => &full.kural.meaning_kalaignar,
        Scholar::Parimel => full
            .kural
            .meaning_parimel
            .as_deref()
            .unwrap_or(&full.kural.meaning_mu_va),
        Scholar::English => {
            if full.kural.explanation_en.trim().is_empty() {
                &full.kural.translation_en
            } else {
                &full.kural.explanation_en
            }
        }
    }
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
}
