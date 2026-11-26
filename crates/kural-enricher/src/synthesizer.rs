use crate::lexicon::{determine_research_theme, match_tags};
use anyhow::{Result, bail};
use chrono::Utc;
use kural_core::{
    Adhikaram, CanonicalDataset, Division, EnrichedAdultPayload, EnrichedChapterFile,
    EnrichedChildPayload, EnrichedKuralRecord, EnrichedYouthPayload, Iyal, KuralCore,
    normalize_enriched_chapter, tokenize_kural_words, validate_enriched_chapter,
};
use std::collections::{BTreeMap, HashMap, HashSet};
use unicode_segmentation::UnicodeSegmentation;

const PROTAGONISTS: [(&str, &str); 6] = [
    ("கவின்", "Kavin"),
    ("நிலா", "Nila"),
    ("அன்பு", "Anbu"),
    ("இனியா", "Iniya"),
    ("முகில்", "Mugil"),
    ("தமிழினி", "Tamizhini"),
];

const FALLBACK_TAMIL_WORDS: [&str; 6] = ["அறிவு", "பண்பு", "உண்மை", "முயற்சி", "கருணை", "நட்பு"];

/// Truncate `text` cleanly on a whitespace boundary to at most `max_chars` scalar values.
fn excerpt_clean(text: &str, max_chars: usize) -> String {
    let trimmed = text.trim();
    if trimmed.chars().count() <= max_chars {
        return trimmed.to_string();
    }
    let mut last_space_byte = 0usize;
    let mut end_byte = 0usize;
    for (count, (byte_idx, ch)) in trimmed.char_indices().enumerate() {
        if count >= max_chars {
            break;
        }
        end_byte = byte_idx + ch.len_utf8();
        if ch.is_whitespace() {
            last_space_byte = byte_idx;
        }
    }
    let slice_end = if last_space_byte > end_byte / 2 {
        last_space_byte
    } else {
        end_byte
    };
    let head = trimmed[..slice_end].trim_end_matches(|c: char| !c.is_alphanumeric());
    format!("{head}...")
}

fn ensure_sentence_end(s: &str) -> String {
    let t = s.trim();
    if t.ends_with('.') || t.ends_with('!') || t.ends_with('?') {
        t.to_string()
    } else {
        format!("{t}.")
    }
}

fn select_game_blank_word(kural: &KuralCore) -> String {
    let tokens = tokenize_kural_words(&kural.line1_ta, &kural.line2_ta);
    let long_tokens: Vec<&String> = tokens
        .iter()
        .filter(|w| w.graphemes(true).count() >= 3)
        .collect();
    if !long_tokens.is_empty() {
        let idx = usize::from(kural.number) % long_tokens.len();
        return long_tokens[idx].clone();
    }
    tokens
        .into_iter()
        .max_by_key(|w| w.graphemes(true).count())
        .unwrap_or_else(|| "உலகு".to_string())
}

fn build_game_options(
    kural: &KuralCore,
    blank_word: &str,
    chapter_kurals: &[&KuralCore],
    all_kurals: &[KuralCore],
) -> ([String; 4], [String; 4]) {
    let mut ta_distractors = Vec::with_capacity(3);
    let mut seen_ta = HashSet::new();
    seen_ta.insert(blank_word.to_string());

    for neighbor in chapter_kurals
        .iter()
        .copied()
        .filter(|other| other.number != kural.number)
    {
        for tok in tokenize_kural_words(&neighbor.line1_ta, &neighbor.line2_ta) {
            if tok.graphemes(true).count() >= 3 && seen_ta.insert(tok.clone()) {
                ta_distractors.push(tok);
                if ta_distractors.len() == 3 {
                    break;
                }
            }
        }
        if ta_distractors.len() == 3 {
            break;
        }
    }
    for fb in FALLBACK_TAMIL_WORDS {
        if ta_distractors.len() == 3 {
            break;
        }
        if seen_ta.insert(fb.to_string()) {
            ta_distractors.push(fb.to_string());
        }
    }

    let slot = usize::from(kural.number) % 4;
    let mut ta_arr = [String::new(), String::new(), String::new(), String::new()];
    let mut d_idx = 0usize;
    for (i, out) in ta_arr.iter_mut().enumerate() {
        if i == slot {
            *out = blank_word.to_string();
        } else {
            *out = ta_distractors[d_idx].clone();
            d_idx += 1;
        }
    }

    let mut en_distractors = Vec::with_capacity(3);
    let mut seen_en = HashSet::new();
    seen_en.insert(kural.translation_en.clone());

    for neighbor in chapter_kurals
        .iter()
        .copied()
        .filter(|other| other.number != kural.number)
    {
        if seen_en.insert(neighbor.translation_en.clone()) {
            en_distractors.push(neighbor.translation_en.clone());
            if en_distractors.len() == 3 {
                break;
            }
        }
    }
    if en_distractors.len() < 3 {
        let total = all_kurals.len();
        let mut step = 1usize;
        while en_distractors.len() < 3 && step < total {
            let cand = &all_kurals[(usize::from(kural.number) + step * 17) % total];
            if seen_en.insert(cand.translation_en.clone()) {
                en_distractors.push(cand.translation_en.clone());
            }
            step += 1;
        }
    }

    let mut en_arr = [String::new(), String::new(), String::new(), String::new()];
    let mut e_idx = 0usize;
    for (i, out) in en_arr.iter_mut().enumerate() {
        if i == slot {
            *out = kural.translation_en.clone();
        } else {
            *out = en_distractors[e_idx].clone();
            e_idx += 1;
        }
    }

    (ta_arr, en_arr)
}

fn build_child_payload(
    kural: &KuralCore,
    adhikaram: &Adhikaram,
    chapter_kurals: &[&KuralCore],
    all_kurals: &[KuralCore],
) -> EnrichedChildPayload {
    let (name_ta, name_en) = PROTAGONISTS[usize::from(kural.number) % PROTAGONISTS.len()];
    let mv_sentence = ensure_sentence_end(&kural.meaning_mu_va);
    let sp_sentence = ensure_sentence_end(&kural.meaning_solomon);
    let exp_sentence = ensure_sentence_end(&kural.explanation_en);

    let simple_meaning_ta = match kural.number % 4 {
        0 => format!("குழந்தைகளே, தாத்தா வள்ளுவர் அன்போடு சொல்கிறார்: {mv_sentence}"),
        1 => format!("குழந்தைகளே, இக்குறளின் எளிய பொருள்: {mv_sentence}"),
        2 => format!("அன்புக் குழந்தைகளே, நினைவில் கொள்ளுங்கள்: {mv_sentence}"),
        _ => format!("சின்னஞ்சிறு நண்பரே, வள்ளுவர் தரும் பாடல்: {mv_sentence}"),
    };

    let simple_meaning_en = match kural.number % 4 {
        0 => format!("Little friend, Grandpa Valluvar gently reminds us: {exp_sentence}"),
        1 => format!("Little friend, here is the simple wisdom of this Kural: {exp_sentence}"),
        2 => format!("Dear child, remember this golden lesson: {exp_sentence}"),
        _ => format!("Young explorer, Grandpa Valluvar teaches us: {exp_sentence}"),
    };

    let blank_word = select_game_blank_word(kural);
    let story_title_ta = format!("{} — {} கற்ற பாடம்", adhikaram.name_ta, name_ta);
    let story_title_en = format!("{} — {}'s Lesson", adhikaram.name_en, name_en);

    let (kutty_story_ta, kutty_story_en) = if kural.division_id == 1 {
        let ta = format!(
            "{name_ta} பள்ளி முடிந்து வீட்டுக்கு வந்ததும் தன் தாத்தாவுடன் பூங்காவில் அமர்ந்திருந்தான்/அமர்ந்திருந்தாள். \
            அன்று '{}' என்ற அதிகாரத்தில் உள்ள திருக்குறள் {} பற்றி தாத்தா ஒரு சுவையான கதை கூறினார். \
            '{}' என்று திருவள்ளுவர் ஏன் கூறினார் என்பதைத் தன் நண்பர்களுக்குச் சிறு செயலால் {} காட்டியது. \
            கூட இருந்த அனைவரும் அந்த நல்ல குணத்தைப் பாராட்டினார்கள். \
            நல்லொழுக்கமும் அன்பும் கொண்ட வாழ்க்கையே உண்மையான மகிழ்ச்சி தரும் என {} மகிழ்ச்சியுடன் புரிந்துகொண்டாள்.",
            adhikaram.name_ta, kural.number, sp_sentence, name_ta, name_ta
        );
        let en = format!(
            "After school, {name_en} sat under the mango tree with Grandpa to hear a story on '{}'. \
            Grandpa shared Kural {} and explained in simple words: {exp_sentence} \
            The very next morning, {name_en} put that virtue into action at school by helping a classmate with patience and warmth. \
            The teacher smiled and praised how one small good habit brightened the entire classroom. \
            From that day on, {name_en} remembered that living truthfully and kindly brings lasting joy.",
            adhikaram.name_en, kural.number
        );
        (ta, en)
    } else if kural.division_id == 2 {
        let ta = format!(
            "பள்ளி அறிவியல் கண்காட்சிக்குத் தயாராகிக் கொண்டிருந்த {} குழுவுக்குச் சிறு சவால் வந்தது. \
            அப்போது ஆசிரியர் '{}' என்ற அதிகாரத்திலிருந்து குறள் {} பற்றிக் கதை சொன்னார். \
            '{}' என்பதே வெற்றிக்கான ரகசியம் என்பதை உணர்ந்த {} தன் நண்பர்களுடன் திட்டமிட்டுச் செயல்பட்டது. \
            சரியான நேரமும் நேர்மையும் இருந்தால் எந்தப் பணியையும் சிறப்பாக முடிக்கலாம் என்பது அவர்களுக்குத் தெளிவானது. \
            முடிவில் குழுவின் ஒற்றுமையையும் முயற்சியையும் பள்ளி முழுவதும் பாராட்டியது.",
            name_ta, adhikaram.name_ta, kural.number, sp_sentence, name_ta
        );
        let en = format!(
            "While preparing for the school science fair, {name_en} and the team hit a tricky problem. \
            Their teacher paused and told a story from Chapter {}, '{}', centered on Kural {}: {exp_sentence} \
            Guided by this wisdom, {name_en} organized the team calmly, listened to every friend's idea, and worked step by step. \
            By working together at the right moment with honesty and effort, the group finished their project cleanly. \
            Everyone learned that smart planning and true friendship overcome tough challenges.",
            adhikaram.id, adhikaram.name_en, kural.number
        );
        (ta, en)
    } else {
        // Inbathuppal (1081..=1330): 100% child-safe family affection, care, and harmony framing.
        let ta = format!(
            "விடுமுறை நாளில் {} வீட்டில் தாத்தா பாட்டியும் பெற்றோரும் அன்போடு கூடியிருந்தனர். \
            '{}' என்ற அதிகாரத்தில் வரும் குறள் {} சொல்லும் குடும்பப் பாசத்தையும் அக்கறையையும் பாட்டி எளிய கதையாகக் கூறினார். \
            வீட்டில் உள்ளவர்கள் ஒருவரையொருவர் புரிந்துகொண்டு அன்போடும் மரியாதையோடும் பேசும்போது வீடே மகிழ்ச்சியால் நிறையும் என்று விளக்கினார். \
            அதைக் கேட்ட {} தன் தம்பிக்கும் பெற்றோருக்கும் சிறு உதவிகளைச் செய்து அன்பைக் காட்டியது. \
            குடும்ப அன்பும் கருணையும் நல்லிணக்கமும் தான் வாழ்வின் இனிமை என {} உணர்ந்தாள்.",
            name_ta, adhikaram.name_ta, kural.number, name_ta, name_ta
        );
        let en = format!(
            "On a sunny weekend, {name_en}'s grandparents visited home, filling the house with warmth and laughter. \
            Grandma shared a gentle story inspired by Kural {} from '{}', highlighting how caring hearts and kind words strengthen family harmony. \
            She explained that noticing how loved ones feel and responding with patience keeps every home cheerful and safe. \
            Inspired by the story, {name_en} helped prepare tea and shared favorite books with younger cousins. \
            The whole family smiled, seeing how kindness, respect, and affection turn ordinary moments into lasting memories.",
            kural.number, adhikaram.name_en
        );
        (ta, en)
    };

    let moral_ta = format!("நீதி: {}", ensure_sentence_end(&kural.meaning_kalaignar));
    let moral_en = format!("Moral: {}", ensure_sentence_end(&kural.couplet_en));

    let (game_options_ta, game_options_en) =
        build_game_options(kural, &blank_word, chapter_kurals, all_kurals);

    EnrichedChildPayload {
        simple_meaning_ta,
        simple_meaning_en,
        story_title_ta,
        story_title_en,
        kutty_story_ta,
        kutty_story_en,
        moral_ta,
        moral_en,
        game_blank_word_ta: blank_word,
        game_options_ta,
        game_options_en,
    }
}

fn build_youth_payload(
    kural: &KuralCore,
    iyal: &Iyal,
    adhikaram: &Adhikaram,
    all_kurals: &[KuralCore],
) -> EnrichedYouthPayload {
    let sp_sentence = ensure_sentence_end(&kural.meaning_solomon);
    let mk_sentence = ensure_sentence_end(&kural.meaning_kalaignar);
    let exp_sentence = ensure_sentence_end(&kural.explanation_en);

    let analysis_ta = format!(
        "{} ({}) பிரிவில் அமைந்த குறள் {}, மாணவர்களுக்கும் இளையோருக்கும் முக்கியமான வாழ்வியல் சிந்தனையைத் தருகிறது. \
        சாலமன் பாப்பையா உரைப்படி: {} கலைஞர் உரை கூறுவது: {} \
        இன்றைய படிப்பு, நட்பு, இணைய ஒழுக்கம் மற்றும் இலக்கு நோக்கிய முயற்சியில் இது ஒரு தெளிவான வழிகாட்டியாகும்.",
        adhikaram.name_ta, iyal.name_ta, kural.number, sp_sentence, mk_sentence
    );

    let analysis_en = format!(
        "From {} ({}), Kural {} speaks directly to teens navigating studies, friendships, leadership, and digital choices: {exp_sentence} \
        Instead of reacting impulsively or following crowd noise, applying this principle builds quiet resilience, sharper focus, and genuine credibility among peers.",
        adhikaram.name_en, iyal.name_en, kural.number
    );

    let (action_tip_ta, action_tip_en) = match iyal.id {
        1 | 4 => (
            format!(
                "இன்று '{}' குறள் {} நெறியை நினைவில் கொண்டு 15 நிமிடம் கவனச் சிதறலின்றிப் படி அல்லது சிந்தி.",
                adhikaram.name_ta, kural.number
            ),
            format!(
                "Spend 15 distraction-free minutes today reflecting on Kural {} ({}) and writing one clear priority.",
                kural.number, adhikaram.name_en
            ),
        ),
        2 | 3 => (
            format!(
                "இன்று ஒருவரிடம் கனிவாகப் பேசி '{}' பண்பைச் செயலில் காட்டு.",
                adhikaram.name_ta
            ),
            format!(
                "Practice '{}' today by speaking kindly and doing one unprompted helpful act at home or school.",
                adhikaram.name_en
            ),
        ),
        5..=7 => (
            format!(
                "இன்று ஒரு முடிவெடுக்கும் முன் அவசரப்படாமல் '{}' கூறும் கோணத்தில் 2 நிமிடம் ஆராய்ந்து செயல்படு.",
                adhikaram.name_ta
            ),
            format!(
                "Before making a group or study decision today, pause for two minutes to apply '{}' thoughtfully.",
                adhikaram.name_en
            ),
        ),
        _ => (
            format!(
                "இன்று குடும்பத்தினர் அல்லது நண்பர்களின் உணர்வுகளை மதித்து '{}' பண்போடு நடந்துகொள்.",
                adhikaram.name_ta
            ),
            format!(
                "Listen empathetically to a family member or friend today, honoring the spirit of '{}'.",
                adhikaram.name_en
            ),
        ),
    };

    let quiz_question_ta = format!(
        "'{}' அதிகாரத்தில் வரும் திருக்குறள் {} உணர்த்தும் மையக் கருத்து என்ன?",
        adhikaram.name_ta, kural.number
    );
    let quiz_question_en = format!(
        "What is the core meaning of Kural {} from Chapter '{}'?",
        kural.number, adhikaram.name_en
    );

    let correct_idx = u8::try_from(kural.number % 4).unwrap_or(0);
    let mut dist_ta = Vec::with_capacity(3);
    let mut dist_en = Vec::with_capacity(3);
    let mut seen_ta = HashSet::new();
    let mut seen_en = HashSet::new();
    seen_ta.insert(kural.meaning_mu_va.clone());
    seen_en.insert(kural.translation_en.clone());

    let total = all_kurals.len();
    let mut offset = 1usize;
    while (dist_ta.len() < 3 || dist_en.len() < 3) && offset < total {
        let cand = &all_kurals[(usize::from(kural.number) + offset * 43) % total];
        if cand.adhikaram_id != kural.adhikaram_id {
            if dist_ta.len() < 3 && seen_ta.insert(cand.meaning_mu_va.clone()) {
                dist_ta.push(cand.meaning_mu_va.clone());
            }
            if dist_en.len() < 3 && seen_en.insert(cand.translation_en.clone()) {
                dist_en.push(cand.translation_en.clone());
            }
        }
        offset += 1;
    }

    let mut quiz_options_ta = [String::new(), String::new(), String::new(), String::new()];
    let mut quiz_options_en = [String::new(), String::new(), String::new(), String::new()];
    let mut t_i = 0usize;
    let mut e_i = 0usize;
    for idx in 0..4usize {
        if idx == usize::from(correct_idx) {
            quiz_options_ta[idx] = kural.meaning_mu_va.clone();
            quiz_options_en[idx] = kural.translation_en.clone();
        } else {
            quiz_options_ta[idx] = dist_ta[t_i].clone();
            quiz_options_en[idx] = dist_en[e_i].clone();
            t_i += 1;
            e_i += 1;
        }
    }

    EnrichedYouthPayload {
        analysis_ta,
        analysis_en,
        action_tip_ta,
        action_tip_en,
        quiz_question_ta,
        quiz_question_en,
        quiz_options_ta,
        quiz_options_en,
        quiz_correct_idx: correct_idx,
    }
}

fn build_adult_payload(
    kural: &KuralCore,
    division: &Division,
    iyal: &Iyal,
    adhikaram: &Adhikaram,
) -> EnrichedAdultPayload {
    let parimel_part = match &kural.meaning_parimel {
        Some(p) if !p.trim().is_empty() => {
            format!(" பரிமேலழகர் உரை நுட்பம்: {}", excerpt_clean(p, 260))
        }
        _ => String::new(),
    };

    let reflection_ta = format!(
        "{} — {} ({}) பகுதியில் குறள் {} மனித வாழ்வின் ஆழமான தத்துவ ஒழுங்கை முன்வைக்கிறது. \
        மு.வ. விளக்கத்தின்படி: {}{}",
        division.name_ta,
        iyal.name_ta,
        adhikaram.name_ta,
        kural.number,
        ensure_sentence_end(&kural.meaning_mu_va),
        parimel_part
    );

    let reflection_en = format!(
        "Within {} ({} — Chapter {}: '{}'), Kural {} distills a foundational ethical insight: \"{}\" \
        Philosophically, {}",
        division.name_en,
        iyal.name_en,
        adhikaram.id,
        adhikaram.name_en,
        kural.number,
        kural.couplet_en.trim(),
        ensure_sentence_end(&kural.explanation_en)
    );

    EnrichedAdultPayload {
        reflection_ta,
        reflection_en,
        research_theme: determine_research_theme(adhikaram.id, division.id),
    }
}

/// Synthesize and strictly validate one Adhikaram (10 Kurals) completely offline.
pub fn synthesize_chapter(
    canonical: &CanonicalDataset,
    adhikaram_id: u8,
) -> Result<EnrichedChapterFile> {
    let adhikaram = canonical
        .adhikarams
        .iter()
        .find(|a| a.id == adhikaram_id)
        .ok_or_else(|| anyhow::anyhow!("adhikaram #{adhikaram_id} not found"))?;
    let division = canonical
        .divisions
        .iter()
        .find(|d| d.id == adhikaram.division_id)
        .ok_or_else(|| anyhow::anyhow!("division #{} not found", adhikaram.division_id))?;
    let iyal = canonical
        .iyals
        .iter()
        .find(|i| i.id == adhikaram.iyal_id)
        .ok_or_else(|| anyhow::anyhow!("iyal #{} not found", adhikaram.iyal_id))?;

    let chapter_kurals: Vec<&KuralCore> = canonical
        .kurals
        .iter()
        .filter(|k| k.adhikaram_id == adhikaram_id)
        .collect();

    let mut records = Vec::with_capacity(chapter_kurals.len());
    for &kural in &chapter_kurals {
        let child = build_child_payload(kural, adhikaram, &chapter_kurals, &canonical.kurals);
        let youth = build_youth_payload(kural, iyal, adhikaram, &canonical.kurals);
        let adult = build_adult_payload(kural, division, iyal, adhikaram);
        let tags = match_tags(kural, adhikaram, iyal);

        records.push(EnrichedKuralRecord {
            kural_number: kural.number,
            child,
            youth,
            adult,
            tags,
        });
    }

    let mut chapter = EnrichedChapterFile {
        adhikaram_id,
        provider: "offline-rust-synthesizer".to_string(),
        model: "canonical-deterministic-v1".to_string(),
        generated_at: Utc::now().to_rfc3339(),
        kurals: records,
    };
    normalize_enriched_chapter(&mut chapter);

    if let Err(errs) = validate_enriched_chapter(&chapter, &chapter_kurals) {
        bail!(
            "synthesized adhikaram #{adhikaram_id} failed validation: {}",
            errs.join("; ")
        );
    }

    Ok(chapter)
}

/// Synthesize and strictly validate all 133 Adhikarams (1,330 Kurals) completely offline.
pub fn synthesize_all_chapters(
    canonical: &CanonicalDataset,
) -> Result<BTreeMap<u8, EnrichedChapterFile>> {
    let mut map = BTreeMap::new();
    let div_map: HashMap<u8, &Division> = canonical.divisions.iter().map(|d| (d.id, d)).collect();
    let iyal_map: HashMap<u8, &Iyal> = canonical.iyals.iter().map(|i| (i.id, i)).collect();
    let _ = (div_map, iyal_map);

    for ad in &canonical.adhikarams {
        let ch = synthesize_chapter(canonical, ad.id)?;
        map.insert(ad.id, ch);
    }
    Ok(map)
}

#[cfg(test)]
mod tests {
    use super::*;
    use kural_core::load_and_audit_canonical;
    use std::path::PathBuf;

    #[test]
    fn offline_synthesizer_produces_valid_133_chapters_and_tags() {
        let root = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .parent()
            .and_then(|p| p.parent())
            .expect("workspace root")
            .to_path_buf();
        let canonical =
            load_and_audit_canonical(&root.join("seed_data/canonical")).expect("canonical audit");

        let chapters = synthesize_all_chapters(&canonical).expect("synthesize all 133");
        assert_eq!(chapters.len(), 133);

        let total_kurals: usize = chapters.values().map(|c| c.kurals.len()).sum();
        let total_tags: usize = chapters
            .values()
            .flat_map(|c| c.kurals.iter())
            .map(|r| r.tags.len())
            .sum();
        assert_eq!(total_kurals, 1330);
        assert!(total_tags >= 2660);
    }

    #[test]
    fn synthesize_chapter_is_deterministic_across_divisions() {
        let root = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .parent()
            .and_then(|p| p.parent())
            .expect("workspace root")
            .to_path_buf();
        let canonical =
            load_and_audit_canonical(&root.join("seed_data/canonical")).expect("canonical audit");

        for ad_id in [1u8, 4, 39, 43, 109, 133] {
            let mut first = synthesize_chapter(&canonical, ad_id).expect("first synthesis");
            let mut second = synthesize_chapter(&canonical, ad_id).expect("second synthesis");
            assert!(!first.generated_at.is_empty());
            assert!(!second.generated_at.is_empty());
            first.generated_at.clear();
            second.generated_at.clear();
            assert_eq!(
                first, second,
                "synthesize_chapter must be deterministic for adhikaram #{ad_id}"
            );
        }
    }

    #[test]
    fn synthesized_records_preserve_nfc_blank_words_and_quiz_bounds() {
        let root = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .parent()
            .and_then(|p| p.parent())
            .expect("workspace root")
            .to_path_buf();
        let canonical =
            load_and_audit_canonical(&root.join("seed_data/canonical")).expect("canonical audit");
        let kural_by_num: HashMap<u16, &KuralCore> =
            canonical.kurals.iter().map(|k| (k.number, k)).collect();

        let chapters = synthesize_all_chapters(&canonical).expect("synthesize all 133");
        for chapter in chapters.values() {
            for rec in &chapter.kurals {
                let kural = kural_by_num
                    .get(&rec.kural_number)
                    .expect("canonical kural present");

                for (label, text) in [
                    (
                        "child.simple_meaning_ta",
                        rec.child.simple_meaning_ta.as_str(),
                    ),
                    ("child.story_title_ta", rec.child.story_title_ta.as_str()),
                    ("child.kutty_story_ta", rec.child.kutty_story_ta.as_str()),
                    ("child.moral_ta", rec.child.moral_ta.as_str()),
                    (
                        "child.game_blank_word_ta",
                        rec.child.game_blank_word_ta.as_str(),
                    ),
                    ("youth.analysis_ta", rec.youth.analysis_ta.as_str()),
                    ("youth.action_tip_ta", rec.youth.action_tip_ta.as_str()),
                    (
                        "youth.quiz_question_ta",
                        rec.youth.quiz_question_ta.as_str(),
                    ),
                    ("adult.reflection_ta", rec.adult.reflection_ta.as_str()),
                ] {
                    assert!(
                        kural_core::is_nfc(text),
                        "kural #{}: {label} is not Unicode NFC",
                        rec.kural_number
                    );
                }

                for opt in &rec.child.game_options_ta {
                    assert!(
                        kural_core::is_nfc(opt),
                        "kural #{}: child.game_options_ta entry is not NFC",
                        rec.kural_number
                    );
                }
                for opt in &rec.youth.quiz_options_ta {
                    assert!(
                        kural_core::is_nfc(opt),
                        "kural #{}: youth.quiz_options_ta entry is not NFC",
                        rec.kural_number
                    );
                }

                let combined_ta = format!("{} {}", kural.line1_ta, kural.line2_ta);
                assert!(
                    combined_ta.contains(&rec.child.game_blank_word_ta)
                        && kural_core::contains_exact_kural_word(
                            &kural.line1_ta,
                            &kural.line2_ta,
                            &rec.child.game_blank_word_ta,
                        ),
                    "kural #{}: game_blank_word_ta '{}' not verbatim in Kural text",
                    rec.kural_number,
                    rec.child.game_blank_word_ta
                );
                assert!(
                    rec.child
                        .game_options_ta
                        .iter()
                        .any(|opt| opt == &rec.child.game_blank_word_ta),
                    "kural #{}: game_options_ta missing game_blank_word_ta",
                    rec.kural_number
                );
                assert!(
                    rec.youth.quiz_correct_idx < 4,
                    "kural #{}: quiz_correct_idx {} out of bounds",
                    rec.kural_number,
                    rec.youth.quiz_correct_idx
                );
            }
        }
    }
}
