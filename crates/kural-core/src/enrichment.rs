use crate::text::{contains_exact_kural_word, normalize_nfc_trimmed};
use crate::{KuralCore, ResearchTheme, is_canonical_tag_slug};
use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, HashSet};

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct EnrichedChapterFile {
    pub adhikaram_id: u8,
    pub provider: String,
    pub model: String,
    pub generated_at: String,
    pub kurals: Vec<EnrichedKuralRecord>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct EnrichedKuralRecord {
    pub kural_number: u16,
    pub child: EnrichedChildPayload,
    pub youth: EnrichedYouthPayload,
    pub adult: EnrichedAdultPayload,
    pub tags: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct EnrichedChildPayload {
    pub simple_meaning_ta: String,
    pub simple_meaning_en: String,
    pub story_title_ta: String,
    pub story_title_en: String,
    pub kutty_story_ta: String,
    pub kutty_story_en: String,
    pub moral_ta: String,
    pub moral_en: String,
    pub game_blank_word_ta: String,
    pub game_options_ta: [String; 4],
    pub game_options_en: [String; 4],
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct EnrichedYouthPayload {
    pub analysis_ta: String,
    pub analysis_en: String,
    pub action_tip_ta: String,
    pub action_tip_en: String,
    pub quiz_question_ta: String,
    pub quiz_question_en: String,
    pub quiz_options_ta: [String; 4],
    pub quiz_options_en: [String; 4],
    pub quiz_correct_idx: u8,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct EnrichedAdultPayload {
    pub reflection_ta: String,
    pub reflection_en: String,
    pub research_theme: ResearchTheme,
}

/// Normalize and trim all strings inside an enriched chapter checkpoint to Unicode NFC.
pub fn normalize_enriched_chapter(chapter: &mut EnrichedChapterFile) {
    chapter.provider = normalize_nfc_trimmed(&chapter.provider);
    chapter.model = normalize_nfc_trimmed(&chapter.model);
    chapter.generated_at = normalize_nfc_trimmed(&chapter.generated_at);

    for rec in &mut chapter.kurals {
        rec.child.simple_meaning_ta = normalize_nfc_trimmed(&rec.child.simple_meaning_ta);
        rec.child.simple_meaning_en = normalize_nfc_trimmed(&rec.child.simple_meaning_en);
        rec.child.story_title_ta = normalize_nfc_trimmed(&rec.child.story_title_ta);
        rec.child.story_title_en = normalize_nfc_trimmed(&rec.child.story_title_en);
        rec.child.kutty_story_ta = normalize_nfc_trimmed(&rec.child.kutty_story_ta);
        rec.child.kutty_story_en = normalize_nfc_trimmed(&rec.child.kutty_story_en);
        rec.child.moral_ta = normalize_nfc_trimmed(&rec.child.moral_ta);
        rec.child.moral_en = normalize_nfc_trimmed(&rec.child.moral_en);
        rec.child.game_blank_word_ta = normalize_nfc_trimmed(&rec.child.game_blank_word_ta);
        for opt in &mut rec.child.game_options_ta {
            *opt = normalize_nfc_trimmed(opt);
        }
        for opt in &mut rec.child.game_options_en {
            *opt = normalize_nfc_trimmed(opt);
        }

        rec.youth.analysis_ta = normalize_nfc_trimmed(&rec.youth.analysis_ta);
        rec.youth.analysis_en = normalize_nfc_trimmed(&rec.youth.analysis_en);
        rec.youth.action_tip_ta = normalize_nfc_trimmed(&rec.youth.action_tip_ta);
        rec.youth.action_tip_en = normalize_nfc_trimmed(&rec.youth.action_tip_en);
        rec.youth.quiz_question_ta = normalize_nfc_trimmed(&rec.youth.quiz_question_ta);
        rec.youth.quiz_question_en = normalize_nfc_trimmed(&rec.youth.quiz_question_en);
        for opt in &mut rec.youth.quiz_options_ta {
            *opt = normalize_nfc_trimmed(opt);
        }
        for opt in &mut rec.youth.quiz_options_en {
            *opt = normalize_nfc_trimmed(opt);
        }

        rec.adult.reflection_ta = normalize_nfc_trimmed(&rec.adult.reflection_ta);
        rec.adult.reflection_en = normalize_nfc_trimmed(&rec.adult.reflection_en);

        for tag in &mut rec.tags {
            *tag = tag.trim().to_ascii_lowercase();
        }
    }
}

fn validate_four_options(
    label: &str,
    kural_number: u16,
    opts: &[String; 4],
    errors: &mut Vec<String>,
) {
    let mut seen = HashSet::new();
    for (idx, opt) in opts.iter().enumerate() {
        let norm = normalize_nfc_trimmed(opt);
        if norm.is_empty() {
            errors.push(format!("kural #{kural_number}: {label}[{idx}] is empty"));
        } else if !seen.insert(norm) {
            errors.push(format!(
                "kural #{kural_number}: {label} contains duplicate option at index {idx}"
            ));
        }
    }
}

fn has_tamil_script(value: &str) -> bool {
    value
        .chars()
        .any(|c| ('\u{0B80}'..='\u{0BFF}').contains(&c))
}

fn check_non_empty(kural_number: u16, field: &str, value: &str, errors: &mut Vec<String>) {
    if normalize_nfc_trimmed(value).is_empty() {
        errors.push(format!("kural #{kural_number}: {field} is empty"));
    }
}

fn check_tamil_non_empty(kural_number: u16, field: &str, value: &str, errors: &mut Vec<String>) {
    let norm = normalize_nfc_trimmed(value);
    if norm.is_empty() {
        errors.push(format!("kural #{kural_number}: {field} is empty"));
    } else if !has_tamil_script(&norm) {
        errors.push(format!(
            "kural #{kural_number}: {field} must contain Tamil script (found '{norm}')"
        ));
    }
}

fn validate_four_tamil_options(
    label: &str,
    kural_number: u16,
    opts: &[String; 4],
    errors: &mut Vec<String>,
) {
    validate_four_options(label, kural_number, opts, errors);
    for (idx, opt) in opts.iter().enumerate() {
        let norm = normalize_nfc_trimmed(opt);
        if !norm.is_empty() && !has_tamil_script(&norm) {
            errors.push(format!(
                "kural #{kural_number}: {label}[{idx}] must contain Tamil script (found '{norm}')"
            ));
        }
    }
}

/// Validate an enriched chapter checkpoint against its 10 canonical Kurals.
/// Returns all discovered validation violations so retries can report exact diagnostics.
pub fn validate_enriched_chapter(
    chapter: &EnrichedChapterFile,
    canonical_kurals: &[&KuralCore],
) -> Result<(), Vec<String>> {
    let mut errors = Vec::new();
    if !(1..=133).contains(&chapter.adhikaram_id) {
        errors.push(format!(
            "adhikaram_id {} is out of range 1..=133",
            chapter.adhikaram_id
        ));
        return Err(errors);
    }

    let expected_start = u16::from(chapter.adhikaram_id - 1) * 10 + 1;
    let expected_end = u16::from(chapter.adhikaram_id) * 10;

    let canonical_by_num: BTreeMap<u16, &KuralCore> = canonical_kurals
        .iter()
        .copied()
        .map(|k| (k.number, k))
        .collect();

    if chapter.kurals.len() != 10 {
        errors.push(format!(
            "adhikaram #{}: expected 10 kurals, found {}",
            chapter.adhikaram_id,
            chapter.kurals.len()
        ));
    }

    let mut seen_numbers = HashSet::new();
    for rec in &chapter.kurals {
        let num = rec.kural_number;
        if !(expected_start..=expected_end).contains(&num) {
            errors.push(format!(
                "kural #{num} does not belong to adhikaram #{} ({expected_start}..={expected_end})",
                chapter.adhikaram_id
            ));
            continue;
        }
        if !seen_numbers.insert(num) {
            errors.push(format!("duplicate enriched kural #{num}"));
            continue;
        }

        let Some(canon) = canonical_by_num.get(&num) else {
            errors.push(format!("missing canonical reference for kural #{num}"));
            continue;
        };

        // Child fields
        check_tamil_non_empty(
            num,
            "child.simple_meaning_ta",
            &rec.child.simple_meaning_ta,
            &mut errors,
        );
        check_non_empty(
            num,
            "child.simple_meaning_en",
            &rec.child.simple_meaning_en,
            &mut errors,
        );
        check_tamil_non_empty(
            num,
            "child.story_title_ta",
            &rec.child.story_title_ta,
            &mut errors,
        );
        check_non_empty(
            num,
            "child.story_title_en",
            &rec.child.story_title_en,
            &mut errors,
        );
        check_tamil_non_empty(
            num,
            "child.kutty_story_ta",
            &rec.child.kutty_story_ta,
            &mut errors,
        );
        check_non_empty(
            num,
            "child.kutty_story_en",
            &rec.child.kutty_story_en,
            &mut errors,
        );
        check_tamil_non_empty(num, "child.moral_ta", &rec.child.moral_ta, &mut errors);
        check_non_empty(num, "child.moral_en", &rec.child.moral_en, &mut errors);
        check_tamil_non_empty(
            num,
            "child.game_blank_word_ta",
            &rec.child.game_blank_word_ta,
            &mut errors,
        );

        validate_four_tamil_options(
            "child.game_options_ta",
            num,
            &rec.child.game_options_ta,
            &mut errors,
        );
        validate_four_options(
            "child.game_options_en",
            num,
            &rec.child.game_options_en,
            &mut errors,
        );

        let blank_norm = normalize_nfc_trimmed(&rec.child.game_blank_word_ta);
        if !contains_exact_kural_word(&canon.line1_ta, &canon.line2_ta, &blank_norm) {
            errors.push(format!(
                "kural #{num}: game_blank_word_ta '{blank_norm}' is not an exact word in '{}' / '{}'",
                canon.line1_ta, canon.line2_ta
            ));
        }
        if !rec
            .child
            .game_options_ta
            .iter()
            .any(|opt| normalize_nfc_trimmed(opt) == blank_norm)
        {
            errors.push(format!(
                "kural #{num}: game_options_ta does not contain game_blank_word_ta '{blank_norm}'"
            ));
        }

        // Youth fields
        check_tamil_non_empty(
            num,
            "youth.analysis_ta",
            &rec.youth.analysis_ta,
            &mut errors,
        );
        check_non_empty(
            num,
            "youth.analysis_en",
            &rec.youth.analysis_en,
            &mut errors,
        );
        check_tamil_non_empty(
            num,
            "youth.action_tip_ta",
            &rec.youth.action_tip_ta,
            &mut errors,
        );
        check_non_empty(
            num,
            "youth.action_tip_en",
            &rec.youth.action_tip_en,
            &mut errors,
        );
        check_tamil_non_empty(
            num,
            "youth.quiz_question_ta",
            &rec.youth.quiz_question_ta,
            &mut errors,
        );
        check_non_empty(
            num,
            "youth.quiz_question_en",
            &rec.youth.quiz_question_en,
            &mut errors,
        );
        validate_four_tamil_options(
            "youth.quiz_options_ta",
            num,
            &rec.youth.quiz_options_ta,
            &mut errors,
        );
        validate_four_options(
            "youth.quiz_options_en",
            num,
            &rec.youth.quiz_options_en,
            &mut errors,
        );
        if rec.youth.quiz_correct_idx > 3 {
            errors.push(format!(
                "kural #{num}: quiz_correct_idx {} is outside 0..=3",
                rec.youth.quiz_correct_idx
            ));
        }

        // Adult fields
        check_tamil_non_empty(
            num,
            "adult.reflection_ta",
            &rec.adult.reflection_ta,
            &mut errors,
        );
        check_non_empty(
            num,
            "adult.reflection_en",
            &rec.adult.reflection_en,
            &mut errors,
        );

        // Tags (2..=4 canonical unique slugs)
        if !(2..=4).contains(&rec.tags.len()) {
            errors.push(format!(
                "kural #{num}: expected 2..=4 tags, found {}",
                rec.tags.len()
            ));
        }
        let mut seen_tags = HashSet::new();
        for slug in &rec.tags {
            if !is_canonical_tag_slug(slug) {
                errors.push(format!("kural #{num}: non-canonical tag slug '{slug}'"));
            } else if !seen_tags.insert(slug.as_str()) {
                errors.push(format!("kural #{num}: duplicate tag slug '{slug}'"));
            }
        }
    }

    for num in expected_start..=expected_end {
        if !seen_numbers.contains(&num) {
            errors.push(format!(
                "adhikaram #{}: missing enriched kural #{num}",
                chapter.adhikaram_id
            ));
        }
    }

    if errors.is_empty() {
        Ok(())
    } else {
        Err(errors)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::text::build_tts_kural_en;
    use crate::text::build_tts_kural_ta;

    fn sample_core(number: u16) -> KuralCore {
        KuralCore {
            number,
            division_id: 1,
            iyal_id: 1,
            adhikaram_id: 1,
            line1_ta: "அகர முதல எழுத்தெல்லாம் ஆதி".into(),
            line2_ta: "பகவன் முதற்றே உலகு.".into(),
            opening_letter_ta: "அ".into(),
            transliteration1: "Akara Mudhala Ezhuththellaam Aadhi".into(),
            transliteration2: "Pakavan Mudhatre Ulaku".into(),
            meaning_mu_va: "விளக்கம்".into(),
            meaning_solomon: "விளக்கம்".into(),
            meaning_kalaignar: "விளக்கம்".into(),
            meaning_parimel: None,
            translation_en: "Meaning".into(),
            explanation_en: "Explanation".into(),
            couplet_en: "Couplet".into(),
            tts_kural_ta: build_tts_kural_ta("அகர முதல எழுத்தெல்லாம் ஆதி", "பகவன் முதற்றே உலகு."),
            tts_kural_en: build_tts_kural_en("Meaning", "Couplet"),
        }
    }

    fn sample_record(number: u16) -> EnrichedKuralRecord {
        EnrichedKuralRecord {
            kural_number: number,
            child: EnrichedChildPayload {
                simple_meaning_ta: "எல்லா எழுத்துக்கும் அ முதல் போல உலகிற்கு இறைவன் முதல்.".into(),
                simple_meaning_en: "Just like A begins every letter, goodness begins our world.".into(),
                story_title_ta: "முதல் எழுத்து".into(),
                story_title_en: "The First Letter".into(),
                kutty_story_ta: "அன்பு என்ற சிறுமி பள்ளிக்குச் சென்றாள். அவள் முதலில் அ என்ற எழுத்தைக் கற்றாள். எல்லாச் சொற்களுக்கும் அதுவே அடிப்படை என்றாள் ஆசிரியை. நல்ல எண்ணமே வாழ்வின் அடிப்படை எனப் புரிந்துகொண்டாள்.".into(),
                kutty_story_en: "Little Anbu went to school for the first time. She learned the very first letter A. Her teacher explained that every book starts from one small first letter. Anbu smiled and remembered to always begin her day with kindness.".into(),
                moral_ta: "நல்ல தொடக்கமே உயர்வின் அடிப்படை.".into(),
                moral_en: "A good beginning shapes the whole journey.".into(),
                game_blank_word_ta: "உலகு".into(),
                game_options_ta: ["உலகு".into(), "மலர்".into(), "கடல்".into(), "மழை".into()],
                game_options_en: ["World".into(), "Flower".into(), "Ocean".into(), "Rain".into()],
            },
            youth: EnrichedYouthPayload {
                analysis_ta: "அடிப்படை வலுவாக இருந்தால் கல்வியும் வாழ்க்கையும் சிறக்கும்.".into(),
                analysis_en: "Strong fundamentals in study and ethics anchor long-term focus.".into(),
                action_tip_ta: "இன்று படிக்கத் தொடங்கும் முன் 5 நிமிடம் குறிக்கோளை எழுது.".into(),
                action_tip_en: "Write one clear study goal before opening your phone today.".into(),
                quiz_question_ta: "இக்குறள் எதை அடிப்படையாகக் கூறுகிறது?".into(),
                quiz_question_en: "What foundational idea does this couplet highlight?".into(),
                quiz_options_ta: [
                    "அடிப்படை மூலமும் இறைவனும்".into(),
                    "வீண் பெருமை".into(),
                    "கோபம்".into(),
                    "சோம்பல்".into(),
                ],
                quiz_options_en: [
                    "First principles and the Primal Source".into(),
                    "Boasting loudly".into(),
                    "Anger".into(),
                    "Laziness".into(),
                ],
                quiz_correct_idx: 0,
            },
            adult: EnrichedAdultPayload {
                reflection_ta: "முதற் காரணத்தை உணர்வதே மெய்யறிவின் தொடக்கம்.".into(),
                reflection_en: "Recognizing first principles establishes ethical and philosophical coherence.".into(),
                research_theme: ResearchTheme::Philosophy,
            },
            tags: vec!["wisdom".into(), "humility".into()],
        }
    }

    #[test]
    fn test_validate_enriched_chapter_accepts_valid_and_rejects_bad_blank_word() {
        let cores: Vec<KuralCore> = (1..=10).map(sample_core).collect();
        let core_refs: Vec<&KuralCore> = cores.iter().collect();
        let mut chapter = EnrichedChapterFile {
            adhikaram_id: 1,
            provider: "test".into(),
            model: "test".into(),
            generated_at: "2026-11-24T00:00:00Z".into(),
            kurals: (1..=10).map(sample_record).collect(),
        };
        normalize_enriched_chapter(&mut chapter);
        assert!(validate_enriched_chapter(&chapter, &core_refs).is_ok());

        chapter.kurals[0].child.game_blank_word_ta = "இல்லாதசொல்".into();
        let errs = validate_enriched_chapter(&chapter, &core_refs).unwrap_err();
        assert!(errs.iter().any(|e| e.contains("game_blank_word_ta")));

        chapter.kurals[0].child.simple_meaning_ta = "English text inside Tamil field".into();
        let errs = validate_enriched_chapter(&chapter, &core_refs).unwrap_err();
        assert!(
            errs.iter()
                .any(|e| e.contains("child.simple_meaning_ta must contain Tamil script"))
        );
    }
}
