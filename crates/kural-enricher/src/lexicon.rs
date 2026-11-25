use kural_core::{Adhikaram, Iyal, KuralCore, ResearchTheme};

struct TagLexicon {
    slug: &'static str,
    tamil_keywords: &'static [&'static str],
    english_keywords: &'static [&'static str],
}

const TAG_LEXICONS: [TagLexicon; 20] = [
    TagLexicon {
        slug: "leadership",
        tamil_keywords: &["அரச", "வேந்த", "தலைமை", "இறைமாட்சி", "ஆட்சி", "அமைச்ச", "படை"],
        english_keywords: &["king", "ruler", "minister", "leader", "sovereign", "govern", "state", "army"],
    },
    TagLexicon {
        slug: "ethics",
        tamil_keywords: &["அறம்", "அறன்", "நெறி", "பாவம்", "புண்ணியம்", "நன்மை", "தீமை", "கடமை"],
        english_keywords: &["virtue", "righteous", "moral", "duty", "ethic", "goodness", "vice", "sin"],
    },
    TagLexicon {
        slug: "friendship",
        tamil_keywords: &["நட்பு", "நண்ப", "கேண்மை", "பழக", "உதவி", "கூடாநட்பு"],
        english_keywords: &["friend", "companionship", "ally", "associate", "camaraderie"],
    },
    TagLexicon {
        slug: "wisdom",
        tamil_keywords: &["அறிவு", "மெய்", "உணர்வு", "தெளிவு", "ஞானம்", "கடவுள்", "வாலறிவன்"],
        english_keywords: &["wisdom", "wise", "discern", "insight", "truth-knowledge", "sagacity", "god", "primal"],
    },
    TagLexicon {
        slug: "time",
        tamil_keywords: &["காலம்", "பொழுது", "நேரம்", "பகல்", "விரைந்து", "தக்க"],
        english_keywords: &["time", "season", "moment", "hour", "opportunity", "delay", "timely"],
    },
    TagLexicon {
        slug: "education",
        tamil_keywords: &["கல்வி", "கற்க", "கேள்வி", "நூல்", "எழுத்து", "எண்", "படி"],
        english_keywords: &["learn", "study", "education", "lore", "letter", "book", "scholar", "listen"],
    },
    TagLexicon {
        slug: "self-mastery",
        tamil_keywords: &["அடக்கம்", "சினம்", "பொறை", "புலன்", "துறவு", "தவம்", "மனம்", "ஆசை"],
        english_keywords: &["control", "restraint", "anger", "patience", "desire", "ascetic", "temper", "mind"],
    },
    TagLexicon {
        slug: "gratitude",
        tamil_keywords: &["நன்றி", "உதவி", "செய்ந்நன்றி", "பயன்", "மறவ"],
        english_keywords: &["gratitude", "grateful", "thank", "benefit", "favour", "kindness done"],
    },
    TagLexicon {
        slug: "truth",
        tamil_keywords: &["வாய்மை", "உண்மை", "பொய்", "மெய்ப்பொருள்", "வஞ்சம்"],
        english_keywords: &["truth", "honest", "falsehood", "lie", "sincere", "deceit", "real"],
    },
    TagLexicon {
        slug: "courage",
        tamil_keywords: &["வீரம்", "அஞ்சா", "ஊக்கம்", "மறம்", "துணிவு", "பகை"],
        english_keywords: &["courage", "brave", "fearless", "valour", "valor", "foe", "enemy", "bold"],
    },
    TagLexicon {
        slug: "kindness",
        tamil_keywords: &["அருள்", "ஈகை", "கருணை", "இன்னா", "கொல்லாமை", "பசி", "வறிய"],
        english_keywords: &["kind", "compassion", "mercy", "charity", "give", "poor", "harm", "grace"],
    },
    TagLexicon {
        slug: "discipline",
        tamil_keywords: &["ஒழுக்கம்", "நெறி", "கட்டுப்பாடு", "கள்ளுண்ணாமை", "சூது", "வழி"],
        english_keywords: &["discipline", "conduct", "decorum", "proper", "habit", "temperance", "order"],
    },
    TagLexicon {
        slug: "humility",
        tamil_keywords: &["பணிவு", "எளிமை", "பெருமை", "அகந்தை", "நாண்", "செருக்கு"],
        english_keywords: &["humble", "humility", "modest", "pride", "arrogance", "meek", "vanity"],
    },
    TagLexicon {
        slug: "justice",
        tamil_keywords: &["நடுவு", "நீதி", "செங்கோல்", "கொடுங்கோல்", "முறை", "தண்டம்"],
        english_keywords: &["justice", "just", "impartial", "fair", "equity", "sceptre", "law", "judge"],
    },
    TagLexicon {
        slug: "family",
        tamil_keywords: &["இல்வாழ்", "மனை", "மக்கள்", "புதல்வ", "விருந்து", "சுற்றம்", "குடி"],
        english_keywords: &["family", "home", "household", "child", "children", "son", "guest", "kin"],
    },
    TagLexicon {
        slug: "speech",
        tamil_keywords: &["சொல்", "இன்சொல்", "வாய்", "நா", "பேச", "புறங்கூறாமை", "பயனில"],
        english_keywords: &["speech", "word", "speak", "tongue", "sweet", "harsh", "slander", "talk"],
    },
    TagLexicon {
        slug: "perseverance",
        tamil_keywords: &["முயற்சி", "ஊக்கம்", "ஆள்வினை", "மடி", "சோம்பல்", "தளரா"],
        english_keywords: &["effort", "persever", "energy", "diligen", "sloth", "idleness", "strive", "work"],
    },
    TagLexicon {
        slug: "wealth",
        tamil_keywords: &["பொருள்", "செல்வம்", "பயன்", "வளம்", "உழவு", "நல்குரவு", "ஈட்ட"],
        english_keywords: &["wealth", "prosper", "rich", "poverty", "farming", "plough", "treasure", "resource"],
    },
    TagLexicon {
        slug: "love",
        tamil_keywords: &["அன்பு", "காதல்", "கண்ணோட்டம்", "நெஞ்சு", "கண்", "பிரிவு", "கற்பு"],
        english_keywords: &["love", "affection", "beloved", "heart", "devotion", "tenderness", "fond"],
    },
    TagLexicon {
        slug: "harmony",
        tamil_keywords: &["நல்லிணக்கம்", "ஒற்றுமை", "பண்பு", "இனிது", "ஊடல்", "சமன்", "உலகம்"],
        english_keywords: &["harmony", "peace", "world", "unity", "gentle", "grace", "accord", "reconcil"],
    },
];

/// Map a Kural + chapter context to 2..=4 canonical topic tag slugs using bilingual lexicons.
pub fn match_tags(kural: &KuralCore, adhikaram: &Adhikaram, iyal: &Iyal) -> Vec<String> {
    let chapter_text_ta = &adhikaram.name_ta;
    let chapter_text_en = adhikaram.name_en.to_ascii_lowercase();

    let verse_text_ta = format!("{} {}", kural.line1_ta, kural.line2_ta);
    let verse_text_en = kural.translation_en.to_ascii_lowercase();

    let comm_text_ta = format!(
        "{} {} {}",
        kural.meaning_mu_va, kural.meaning_solomon, kural.meaning_kalaignar
    );
    let comm_text_en = format!(
        "{} {}",
        kural.explanation_en.to_ascii_lowercase(),
        kural.couplet_en.to_ascii_lowercase()
    );

    let mut scored: Vec<(usize, u32, &'static str)> = TAG_LEXICONS
        .iter()
        .enumerate()
        .map(|(idx, lex)| {
            let mut score = 0u32;
            for kw in lex.tamil_keywords {
                if chapter_text_ta.contains(kw) {
                    score += 3;
                }
                if verse_text_ta.contains(kw) {
                    score += 2;
                }
                if comm_text_ta.contains(kw) {
                    score += 1;
                }
            }
            for kw in lex.english_keywords {
                if chapter_text_en.contains(kw) {
                    score += 3;
                }
                if verse_text_en.contains(kw) {
                    score += 2;
                }
                if comm_text_en.contains(kw) {
                    score += 1;
                }
            }
            (idx, score, lex.slug)
        })
        .filter(|&(_, score, _)| score > 0)
        .collect();

    // Sort highest score first, breaking ties deterministically by rotated index so tags stay varied.
    let offset = usize::from(kural.number % 20);
    scored.sort_by(|a, b| {
        b.1.cmp(&a.1)
            .then_with(|| ((a.0 + 20 - offset) % 20).cmp(&((b.0 + 20 - offset) % 20)))
    });

    let mut tags: Vec<String> = scored
        .into_iter()
        .take(4)
        .map(|(_, _, slug)| slug.to_string())
        .collect();

    let fallbacks: &[&str] = match iyal.id {
        1 => &["wisdom", "ethics", "gratitude"],
        2 => &["family", "kindness", "gratitude"],
        3 => &["self-mastery", "truth", "kindness"],
        4 => &["wisdom", "perseverance"],
        5 => &["leadership", "justice", "time"],
        6 => &["leadership", "friendship", "wealth"],
        7 => &["ethics", "humility", "perseverance"],
        _ => &["love", "harmony", "family"],
    };

    for &fb in fallbacks {
        if tags.len() >= 2 {
            break;
        }
        if !tags.iter().any(|t| t == fb) {
            tags.push(fb.to_string());
        }
    }

    tags
}

/// Deterministic ResearchTheme assignment by Division and Adhikaram.
/// Chapters 25..=43 (ascetic wisdom, fate, education, listening, wisdom) map to `Philosophy`.
pub fn determine_research_theme(adhikaram_id: u8, division_id: u8) -> ResearchTheme {
    if (25..=43).contains(&adhikaram_id) || adhikaram_id == 1 {
        return ResearchTheme::Philosophy;
    }
    match division_id {
        1 => ResearchTheme::Virtue,
        2 => ResearchTheme::Polity,
        _ => ResearchTheme::LoveEthics,
    }
}

