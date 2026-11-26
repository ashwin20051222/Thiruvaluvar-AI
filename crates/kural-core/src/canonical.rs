use crate::text::{
    build_tts_kural_en, build_tts_kural_ta, check_transliteration_phonetic_sanity,
    normalize_nfc_trimmed, opening_grapheme_ta,
};
use crate::{Adhikaram, CoreError, Division, Iyal, KuralCore};
use serde::Deserialize;
use std::collections::{BTreeMap, HashMap, HashSet};
use std::fs;
use std::path::Path;

/// Verified Layer 1 canonical dataset loaded and audited from `seed_data/canonical/`.
#[derive(Debug, Clone)]
pub struct CanonicalDataset {
    pub divisions: Vec<Division>,
    pub iyals: Vec<Iyal>,
    pub adhikarams: Vec<Adhikaram>,
    pub kurals: Vec<KuralCore>,
}

#[derive(Debug, Deserialize)]
struct RawThirukkuralFile {
    kural: Vec<RawKural>,
}

#[derive(Debug, Deserialize)]
struct RawKural {
    #[serde(rename = "Number")]
    number: u16,
    #[serde(rename = "Line1")]
    line1: String,
    #[serde(rename = "Line2")]
    line2: String,
    #[serde(rename = "Translation")]
    translation: String,
    mv: String,
    sp: String,
    mk: String,
    explanation: String,
    couplet: String,
    transliteration1: String,
    transliteration2: String,
}

#[derive(Debug, Deserialize)]
struct RawDetailRoot {
    section: RawSectionWrapper,
}

#[derive(Debug, Deserialize)]
struct RawSectionWrapper {
    detail: Vec<RawDivision>,
}

#[derive(Debug, Deserialize)]
struct RawDivision {
    name: String,
    transliteration: String,
    number: u8,
    #[serde(rename = "chapterGroup")]
    chapter_group: RawChapterGroupWrapper,
}

#[derive(Debug, Deserialize)]
struct RawChapterGroupWrapper {
    detail: Vec<RawChapterGroup>,
}

#[derive(Debug, Deserialize)]
struct RawChapterGroup {
    chapters: RawChaptersWrapper,
}

#[derive(Debug, Deserialize)]
struct RawChaptersWrapper {
    detail: Vec<RawChapter>,
}

#[derive(Debug, Deserialize)]
struct RawChapter {
    name: String,
    translation: String,
    transliteration: String,
    number: u8,
    start: u16,
    end: u16,
}

#[derive(Debug, Deserialize)]
struct RawParimelEntry {
    #[serde(default)]
    pari_melakar: Option<String>,
}

struct CanonicalIyalDef {
    id: u8,
    division_id: u8,
    name_ta: &'static str,
    alt_name_ta: Option<&'static str>,
    name_en: &'static str,
    transliteration: &'static str,
    chapter_start: u8,
    chapter_end: u8,
}

const CANONICAL_IYALS: [CanonicalIyalDef; 9] = [
    CanonicalIyalDef {
        id: 1,
        division_id: 1,
        name_ta: "பாயிரவியல்",
        alt_name_ta: None,
        name_en: "Prologue",
        transliteration: "Paayiraviyal",
        chapter_start: 1,
        chapter_end: 4,
    },
    CanonicalIyalDef {
        id: 2,
        division_id: 1,
        name_ta: "இல்லறவியல்",
        alt_name_ta: None,
        name_en: "Domestic Virtue",
        transliteration: "Illaraviyal",
        chapter_start: 5,
        chapter_end: 24,
    },
    CanonicalIyalDef {
        id: 3,
        division_id: 1,
        name_ta: "துறவறவியல்",
        alt_name_ta: None,
        name_en: "Ascetic Virtue",
        transliteration: "Thuravaraviyal",
        chapter_start: 25,
        chapter_end: 37,
    },
    CanonicalIyalDef {
        id: 4,
        division_id: 1,
        name_ta: "ஊழியல்",
        alt_name_ta: None,
        name_en: "Fate",
        transliteration: "Oozhiyal",
        chapter_start: 38,
        chapter_end: 38,
    },
    CanonicalIyalDef {
        id: 5,
        division_id: 2,
        name_ta: "அரசியல்",
        alt_name_ta: None,
        name_en: "Royalty & Polity",
        transliteration: "Arasiyal",
        chapter_start: 39,
        chapter_end: 63,
    },
    CanonicalIyalDef {
        id: 6,
        division_id: 2,
        name_ta: "அங்கவியல்",
        alt_name_ta: None,
        name_en: "Ministers & Elements of State",
        transliteration: "Angaviyal",
        chapter_start: 64,
        chapter_end: 95,
    },
    CanonicalIyalDef {
        id: 7,
        division_id: 2,
        name_ta: "ஒழிபியல்",
        alt_name_ta: Some("குடியியல்"),
        name_en: "Citizenship & Miscellaneous",
        transliteration: "Ozhibiyal",
        chapter_start: 96,
        chapter_end: 108,
    },
    CanonicalIyalDef {
        id: 8,
        division_id: 3,
        name_ta: "களவியல்",
        alt_name_ta: None,
        name_en: "Pre-marital Love",
        transliteration: "Kalaviyal",
        chapter_start: 109,
        chapter_end: 116,
    },
    CanonicalIyalDef {
        id: 9,
        division_id: 3,
        name_ta: "கற்பியல்",
        alt_name_ta: None,
        name_en: "Post-marital Love",
        transliteration: "Karpiyal",
        chapter_start: 117,
        chapter_end: 133,
    },
];

fn read_json<T: for<'de> Deserialize<'de>>(path: &Path) -> Result<T, CoreError> {
    let raw = fs::read_to_string(path).map_err(|source| CoreError::Io {
        path: path.to_path_buf(),
        source,
    })?;
    serde_json::from_str(&raw).map_err(|source| CoreError::Json {
        path: path.to_path_buf(),
        source,
    })
}

fn require_non_empty(number: u16, field: &str, value: &str) -> Result<String, CoreError> {
    let norm = normalize_nfc_trimmed(value);
    if norm.is_empty() {
        return Err(CoreError::CanonicalAudit(format!(
            "kural #{number} has empty {field}"
        )));
    }
    Ok(norm)
}

fn clean_parimel_text(raw: &str) -> Option<String> {
    let norm = normalize_nfc_trimmed(raw);
    let stripped = norm
        .strip_prefix("பரிமேலழகர் உரை:")
        .or_else(|| norm.strip_prefix("பரிமேலழகர் உரை :"))
        .unwrap_or(&norm)
        .trim();
    if stripped.is_empty() {
        None
    } else {
        Some(normalize_nfc_trimmed(stripped))
    }
}

/// Load `thirukkural.json`, `detail.json`, and optional `parimel.json` from `canonical_dir`
/// and run a complete Layer 1 forensic audit.
pub fn load_and_audit_canonical(canonical_dir: &Path) -> Result<CanonicalDataset, CoreError> {
    let kural_path = canonical_dir.join("thirukkural.json");
    let detail_path = canonical_dir.join("detail.json");
    let parimel_path = canonical_dir.join("parimel.json");

    let raw_kurals: RawThirukkuralFile = read_json(&kural_path)?;
    let raw_details: Vec<RawDetailRoot> = read_json(&detail_path)?;
    let parimel_map: HashMap<String, RawParimelEntry> = if parimel_path.exists() {
        read_json(&parimel_path)?
    } else {
        HashMap::new()
    };

    let detail_root = raw_details
        .into_iter()
        .next()
        .ok_or_else(|| CoreError::CanonicalAudit("detail.json root array is empty".into()))?;

    if detail_root.section.detail.len() != 3 {
        return Err(CoreError::CanonicalAudit(format!(
            "expected 3 divisions in detail.json, found {}",
            detail_root.section.detail.len()
        )));
    }

    let mut divisions = Vec::with_capacity(3);
    let mut raw_chapters_by_id: BTreeMap<u8, (u8, RawChapter)> = BTreeMap::new();

    for raw_div in detail_root.section.detail {
        let div_id = raw_div.number;
        let (name_ta, alt_name_ta, name_en, transliteration, kural_start, kural_end) = match div_id
        {
            1 => (
                normalize_nfc_trimmed(&raw_div.name),
                None,
                "Book of Virtue".to_string(),
                "Arathuppal".to_string(),
                1,
                380,
            ),
            2 => (
                normalize_nfc_trimmed(&raw_div.name),
                None,
                "Book of Wealth & Polity".to_string(),
                "Porutpal".to_string(),
                381,
                1080,
            ),
            3 => (
                "இன்பத்துப்பால்".to_string(),
                Some(normalize_nfc_trimmed(&raw_div.name)),
                "Book of Love & Affection".to_string(),
                "Inbathuppal".to_string(),
                1081,
                1330,
            ),
            other => {
                return Err(CoreError::CanonicalAudit(format!(
                    "unexpected division id {other} (raw transliteration {})",
                    raw_div.transliteration
                )));
            }
        };

        divisions.push(Division {
            id: div_id,
            name_ta,
            alt_name_ta,
            name_en,
            transliteration,
            kural_start,
            kural_end,
        });

        for group in raw_div.chapter_group.detail {
            for ch in group.chapters.detail {
                if raw_chapters_by_id.insert(ch.number, (div_id, ch)).is_some() {
                    return Err(CoreError::CanonicalAudit(
                        "duplicate adhikaram number in detail.json".into(),
                    ));
                }
            }
        }
    }
    divisions.sort_by_key(|d| d.id);

    let iyals: Vec<Iyal> = CANONICAL_IYALS
        .iter()
        .map(|def| Iyal {
            id: def.id,
            division_id: def.division_id,
            name_ta: normalize_nfc_trimmed(def.name_ta),
            alt_name_ta: def.alt_name_ta.map(normalize_nfc_trimmed),
            name_en: def.name_en.to_string(),
            transliteration: def.transliteration.to_string(),
            chapter_start: def.chapter_start,
            chapter_end: def.chapter_end,
        })
        .collect();

    if raw_chapters_by_id.len() != 133 {
        return Err(CoreError::CanonicalAudit(format!(
            "expected 133 adhikarams, found {}",
            raw_chapters_by_id.len()
        )));
    }

    let mut adhikarams = Vec::with_capacity(133);
    for ch_id in 1u8..=133u8 {
        let (div_id, ch) = raw_chapters_by_id
            .remove(&ch_id)
            .ok_or_else(|| CoreError::CanonicalAudit(format!("missing adhikaram #{ch_id}")))?;
        let expected_start = u16::from(ch_id - 1) * 10 + 1;
        let expected_end = u16::from(ch_id) * 10;
        if ch.start != expected_start || ch.end != expected_end {
            return Err(CoreError::CanonicalAudit(format!(
                "adhikaram #{ch_id} range mismatch: got {}..={}, expected {expected_start}..={expected_end}",
                ch.start, ch.end
            )));
        }

        let iyal = CANONICAL_IYALS
            .iter()
            .find(|iy| ch_id >= iy.chapter_start && ch_id <= iy.chapter_end)
            .ok_or_else(|| {
                CoreError::CanonicalAudit(format!("adhikaram #{ch_id} did not map to any iyal"))
            })?;

        if iyal.division_id != div_id {
            return Err(CoreError::CanonicalAudit(format!(
                "adhikaram #{ch_id} division mismatch: detail.json has {div_id}, iyal expects {}",
                iyal.division_id
            )));
        }

        adhikarams.push(Adhikaram {
            id: ch_id,
            division_id: div_id,
            iyal_id: iyal.id,
            name_ta: normalize_nfc_trimmed(&ch.name),
            name_en: normalize_nfc_trimmed(&ch.translation),
            transliteration: normalize_nfc_trimmed(&ch.transliteration),
            kural_start: ch.start,
            kural_end: ch.end,
        });
    }

    if raw_kurals.kural.len() != 1330 {
        return Err(CoreError::CanonicalAudit(format!(
            "expected 1330 kurals, found {}",
            raw_kurals.kural.len()
        )));
    }

    let mut by_number: BTreeMap<u16, RawKural> = BTreeMap::new();
    let mut seen_lines = HashSet::with_capacity(1330);
    for k in raw_kurals.kural {
        if !(1..=1330).contains(&k.number) {
            return Err(CoreError::CanonicalAudit(format!(
                "kural number {} out of 1..=1330 range",
                k.number
            )));
        }
        if by_number.insert(k.number, k).is_some() {
            return Err(CoreError::CanonicalAudit(
                "duplicate kural number in thirukkural.json".into(),
            ));
        }
    }

    let mut kurals = Vec::with_capacity(1330);
    for number in 1u16..=1330u16 {
        let raw = by_number
            .remove(&number)
            .ok_or_else(|| CoreError::CanonicalAudit(format!("missing kural #{number}")))?;

        let line1_ta = require_non_empty(number, "line1_ta", &raw.line1)?;
        let line2_ta = require_non_empty(number, "line2_ta", &raw.line2)?;
        if !seen_lines.insert((line1_ta.clone(), line2_ta.clone())) {
            return Err(CoreError::CanonicalAudit(format!(
                "duplicate kural verse text at #{number}"
            )));
        }

        let opening_letter_ta = opening_grapheme_ta(&line1_ta).ok_or_else(|| {
            CoreError::CanonicalAudit(format!("failed to extract opening grapheme for #{number}"))
        })?;

        let transliteration1 =
            require_non_empty(number, "transliteration1", &raw.transliteration1)?;
        let transliteration2 =
            require_non_empty(number, "transliteration2", &raw.transliteration2)?;

        check_transliteration_phonetic_sanity(number, &line1_ta, &transliteration1)
            .map_err(CoreError::CanonicalAudit)?;

        let meaning_mu_va = require_non_empty(number, "mv", &raw.mv)?;
        let meaning_solomon = require_non_empty(number, "sp", &raw.sp)?;
        let meaning_kalaignar = require_non_empty(number, "mk", &raw.mk)?;
        let translation_en = require_non_empty(number, "Translation", &raw.translation)?;
        let explanation_en = require_non_empty(number, "explanation", &raw.explanation)?;
        let couplet_en = require_non_empty(number, "couplet", &raw.couplet)?;

        let meaning_parimel = parimel_map
            .get(&number.to_string())
            .and_then(|e| e.pari_melakar.as_deref())
            .and_then(clean_parimel_text);

        let tts_kural_ta = build_tts_kural_ta(&line1_ta, &line2_ta);
        let tts_kural_en = build_tts_kural_en(&translation_en, &couplet_en);

        let adhikaram_id = u8::try_from((number - 1) / 10 + 1).map_err(|_| {
            CoreError::CanonicalAudit(format!("adhikaram_id overflow for kural #{number}"))
        })?;
        let adhikaram = &adhikarams[usize::from(adhikaram_id - 1)];

        kurals.push(KuralCore {
            number,
            division_id: adhikaram.division_id,
            iyal_id: adhikaram.iyal_id,
            adhikaram_id,
            line1_ta,
            line2_ta,
            opening_letter_ta,
            transliteration1,
            transliteration2,
            meaning_mu_va,
            meaning_solomon,
            meaning_kalaignar,
            meaning_parimel,
            translation_en,
            explanation_en,
            couplet_en,
            tts_kural_ta,
            tts_kural_en,
        });
    }

    // Verify exact division splits: Aram 380, Porul 700, Inbam 250.
    let aram_count = kurals.iter().filter(|k| k.division_id == 1).count();
    let porul_count = kurals.iter().filter(|k| k.division_id == 2).count();
    let inbam_count = kurals.iter().filter(|k| k.division_id == 3).count();
    if (aram_count, porul_count, inbam_count) != (380, 700, 250) {
        return Err(CoreError::CanonicalAudit(format!(
            "division split mismatch: aram={aram_count}, porul={porul_count}, inbam={inbam_count}"
        )));
    }

    Ok(CanonicalDataset {
        divisions,
        iyals,
        adhikarams,
        kurals,
    })
}
