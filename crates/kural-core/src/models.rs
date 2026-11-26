use crate::CoreError;
use serde::{Deserialize, Serialize};
use std::fmt;
use std::str::FromStr;

/// Canonical fixed list of 20 bilingual topic tags `(slug, name_ta, name_en)`.
pub const CANONICAL_TAGS: [(&str, &str, &str); 20] = [
    ("leadership", "தலைமைத்துவம்", "Leadership"),
    ("ethics", "அறநெறி", "Ethics"),
    ("friendship", "நட்பு", "Friendship"),
    ("wisdom", "அறிவு", "Wisdom"),
    ("time", "காலம்", "Time"),
    ("education", "கல்வி", "Education"),
    ("self-mastery", "தன்னடக்கம்", "Self-Mastery"),
    ("gratitude", "நன்றி", "Gratitude"),
    ("truth", "வாய்மை", "Truth"),
    ("courage", "வீரம்", "Courage"),
    ("kindness", "அருள்", "Kindness"),
    ("discipline", "ஒழுக்கம்", "Discipline"),
    ("humility", "பணிவு", "Humility"),
    ("justice", "நீதி", "Justice"),
    ("family", "குடும்பம்", "Family"),
    ("speech", "இன்சொல்", "Speech"),
    ("perseverance", "முயற்சி", "Perseverance"),
    ("wealth", "பொருள்", "Wealth"),
    ("love", "அன்பு", "Love"),
    ("harmony", "நல்லிணக்கம்", "Harmony"),
];

/// Return true if `slug` is in the fixed canonical 20-tag vocabulary.
pub fn is_canonical_tag_slug(slug: &str) -> bool {
    CANONICAL_TAGS.iter().any(|(s, _, _)| *s == slug)
}

/// Look up `(slug, name_ta, name_en)` for a canonical tag slug.
pub fn canonical_tag_by_slug(slug: &str) -> Option<(&'static str, &'static str, &'static str)> {
    CANONICAL_TAGS
        .iter()
        .copied()
        .find(|(s, _, _)| *s == slug)
}

/// Target age mode for pedagogical delivery.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum AgeGroup {
    Kids,
    Youth,
    Adult,
}

impl fmt::Display for AgeGroup {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Kids => write!(f, "kids"),
            Self::Youth => write!(f, "youth"),
            Self::Adult => write!(f, "adult"),
        }
    }
}

impl FromStr for AgeGroup {
    type Err = CoreError;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s.trim().to_ascii_lowercase().as_str() {
            "kids" | "child" | "children" => Ok(Self::Kids),
            "youth" | "teen" | "teenager" => Ok(Self::Youth),
            "adult" | "scholar" => Ok(Self::Adult),
            other => Err(CoreError::InvalidAgeGroup(other.to_string())),
        }
    }
}

/// Supported display/speech languages.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Language {
    Tamil,
    English,
}

/// Short alias for [`Language`] used across speech and replay contracts.
pub type Lang = Language;

impl Language {
    /// Canonical lowercase identifier stored in SQLite and config files.
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Tamil => "tamil",
            Self::English => "english",
        }
    }
}

impl fmt::Display for Language {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}

impl FromStr for Language {
    type Err = CoreError;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s.trim().to_ascii_lowercase().as_str() {
            "ta" | "tamil" => Ok(Self::Tamil),
            "en" | "english" => Ok(Self::English),
            other => Err(CoreError::InvalidLanguage(other.to_string())),
        }
    }
}

/// Adult/scholar research taxonomy.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ResearchTheme {
    Virtue,
    Philosophy,
    Polity,
    LoveEthics,
}

impl ResearchTheme {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Virtue => "virtue",
            Self::Philosophy => "philosophy",
            Self::Polity => "polity",
            Self::LoveEthics => "love_ethics",
        }
    }
}

impl fmt::Display for ResearchTheme {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}

impl FromStr for ResearchTheme {
    type Err = CoreError;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s.trim().to_ascii_lowercase().as_str() {
            "virtue" => Ok(Self::Virtue),
            "philosophy" => Ok(Self::Philosophy),
            "polity" => Ok(Self::Polity),
            "love_ethics" => Ok(Self::LoveEthics),
            other => Err(CoreError::InvalidResearchTheme(other.to_string())),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Division {
    pub id: u8,
    pub name_ta: String,
    pub alt_name_ta: Option<String>,
    pub name_en: String,
    pub transliteration: String,
    pub kural_start: u16,
    pub kural_end: u16,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Iyal {
    pub id: u8,
    pub division_id: u8,
    pub name_ta: String,
    pub alt_name_ta: Option<String>,
    pub name_en: String,
    pub transliteration: String,
    pub chapter_start: u8,
    pub chapter_end: u8,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Adhikaram {
    pub id: u8,
    pub division_id: u8,
    pub iyal_id: u8,
    pub name_ta: String,
    pub name_en: String,
    pub transliteration: String,
    pub kural_start: u16,
    pub kural_end: u16,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct KuralCore {
    pub number: u16,
    pub division_id: u8,
    pub iyal_id: u8,
    pub adhikaram_id: u8,
    pub line1_ta: String,
    pub line2_ta: String,
    pub opening_letter_ta: String,
    pub transliteration1: String,
    pub transliteration2: String,
    pub meaning_mu_va: String,
    pub meaning_solomon: String,
    pub meaning_kalaignar: String,
    pub meaning_parimel: Option<String>,
    pub translation_en: String,
    pub explanation_en: String,
    pub couplet_en: String,
    pub tts_kural_ta: String,
    pub tts_kural_en: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct KuralChild {
    pub kural_number: u16,
    pub kids_featured: bool,
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
pub struct KuralYouth {
    pub kural_number: u16,
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
pub struct KuralAdult {
    pub kural_number: u16,
    pub reflection_ta: String,
    pub reflection_en: String,
    pub research_theme: ResearchTheme,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Tag {
    pub id: u16,
    pub slug: String,
    pub name_ta: String,
    pub name_en: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct FullKural {
    pub kural: KuralCore,
    pub division: Division,
    pub iyal: Iyal,
    pub adhikaram: Adhikaram,
    pub child: Option<KuralChild>,
    pub youth: Option<KuralYouth>,
    pub adult: Option<KuralAdult>,
    pub tags: Vec<Tag>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct KuralSummary {
    pub number: u16,
    pub division_id: u8,
    pub adhikaram_id: u8,
    pub adhikaram_ta: String,
    pub adhikaram_en: String,
    pub line1_ta: String,
    pub line2_ta: String,
    pub transliteration1: String,
    pub transliteration2: String,
    pub translation_en: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SearchMatchKind {
    ExactNumber,
    AdhikaramMatch,
    FullText,
    FuzzyFallback,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct SearchHit {
    pub kural: KuralSummary,
    pub match_kind: SearchMatchKind,
    pub score: f64,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct SearchOutcome {
    pub query: String,
    pub did_you_mean: Option<String>,
    pub hits: Vec<SearchHit>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct KnowledgeEntry {
    pub id: i64,
    pub category: String,
    pub question_ta: String,
    pub question_en: String,
    pub answer_ta: String,
    pub answer_en: String,
    pub keywords: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct DbStats {
    pub divisions: usize,
    pub iyals: usize,
    pub adhikarams: usize,
    pub kurals: usize,
    pub kural_child: usize,
    pub kural_youth: usize,
    pub kural_adult: usize,
    pub tags: usize,
    pub kural_tags: usize,
    pub knowledge_base: usize,
    pub kural_embeddings: usize,
    pub knowledge_embeddings: usize,
    pub build_mode: String,
    pub integrity_check: String,
    pub file_size_bytes: u64,
}

