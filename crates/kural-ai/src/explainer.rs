use std::collections::HashSet;
use std::time::Instant;

use kural_core::{
    AgeGroup, AiConfig, FullKural, GroundedAnswer, KnowledgeEntry, Language, OOS_CHAT_REFUSAL,
    PARIMEL_FALLBACK_NOTICE, Scholar, SearchMatchKind, SearchOutcome, VOICE_ADHIKARAM_RANGE_ERROR,
    VOICE_KURAL_RANGE_ERROR, normalize_nfc_trimmed,
};
use kural_db::{HybridRrfOutcome, KnowledgeRrfResult, KuralDb};

use crate::AiError;

const KURAL_MIN: u32 = 1;
const KURAL_MAX: u32 = 1330;
const ADHIKARAM_MIN: u32 = 1;
const ADHIKARAM_MAX: u32 = 133;
const RRF_K: f32 = 60.0;
const KURAL_SEARCH_LIMIT: usize = 3;
const KB_SEARCH_LIMIT: usize = 2;

const STOPWORDS: &[&str] = &[
    // Prompt-mandated stopwords
    "what",
    "is",
    "are",
    "the",
    "a",
    "an",
    "in",
    "on",
    "of",
    "for",
    "to",
    "how",
    "why",
    "who",
    "does",
    "mean",
    "tell",
    "me",
    "about",
    "write",
    "code",
    "என்ன",
    "எப்படி",
    "யார்",
    "பற்றி",
    // Additional grammatical & question-framing fillers
    "was",
    "were",
    "be",
    "been",
    "being",
    "do",
    "did",
    "can",
    "could",
    "would",
    "should",
    "will",
    "shall",
    "may",
    "might",
    "must",
    "have",
    "has",
    "had",
    "am",
    "when",
    "where",
    "which",
    "whom",
    "whose",
    "many",
    "much",
    "some",
    "any",
    "all",
    "this",
    "that",
    "these",
    "those",
    "it",
    "its",
    "my",
    "your",
    "our",
    "their",
    "us",
    "we",
    "they",
    "he",
    "she",
    "there",
    "here",
    "with",
    "from",
    "by",
    "at",
    "as",
    "into",
    "and",
    "or",
    "not",
    "no",
    "yes",
    "please",
    "say",
    "says",
    "said",
    "teach",
    "teaches",
    "taught",
    "give",
    "show",
    "explain",
    "define",
    "describe",
    "means",
    "meaning",
    "எது",
    "எவை",
    "யாவை",
    "எங்கே",
    "எப்போது",
    "ஏன்",
    "எத்தனை",
    "பற்றிய",
    "என்று",
    "கூறு",
    "கூறுக",
    "சொல்",
    "சொல்லு",
    "சொல்லுங்கள்",
    "விளக்கு",
    "விளக்கம்",
    "விளக்குக",
    "இந்த",
    "இந்தக்",
    "அந்த",
    "இதன்",
    "அதன்",
    "ஒரு",
    "மற்றும்",
    "உள்ள",
    "ஆகும்",
    "உள்ளதா",
    "ஆகுமா",
];

const CORPUS_META_WORDS: &[&str] = &[
    "thirukkural",
    "kural",
    "kurals",
    "valluvar",
    "thiruvalluvar",
    "திருக்குறள்",
    "திருக்குறளின்",
    "திருக்குறளில்",
    "திருக்குறளை",
    "திருக்குறளுக்கு",
    "குறள்",
    "குறள்கள்",
    "குறளின்",
    "குறளில்",
    "குறளை",
    "வள்ளுவர்",
    "வள்ளுவரின்",
    "வள்ளுவரை",
    "வள்ளுவருக்கு",
    "திருவள்ளுவர்",
    "திருவள்ளுவரின்",
    "திருவள்ளுவரை",
    "திருவள்ளுவருக்கு",
];

const GENERIC_TEMPORAL_OR_UNIT_WORDS: &[&str] = &[
    "year",
    "years",
    "month",
    "months",
    "week",
    "weeks",
    "day",
    "days",
    "today",
    "tomorrow",
    "yesterday",
    "rupee",
    "rupees",
    "dollar",
    "dollars",
    "percent",
    "article",
    "section",
    "page",
    "version",
    "windows",
    "http",
];

const ANAPHORIC_PHRASES: &[&str] = &[
    "explain this",
    "explain it",
    "explain the kural",
    "explain current",
    "what does this mean",
    "what does it mean",
    "simple meaning",
    "this kural",
    "this couplet",
    "current kural",
    "kutty story",
    "இந்தக் குறள்",
    "இந்த குறள்",
    "இக்குறள்",
    "இதன் பொருள்",
];

const ANAPHORIC_INTENT_TOKENS: &[&str] = &[
    "explain",
    "meaning",
    "mean",
    "story",
    "moral",
    "commentary",
    "explanation",
    "விளக்கு",
    "விளக்குக",
    "விளக்கம்",
    "விளக்கவும்",
    "பொருள்",
    "கதை",
    "கருத்து",
    "நீதி",
    "உரை",
];

const ANAPHORIC_COMPANION_TOKENS: &[&str] = &[
    "what",
    "is",
    "are",
    "does",
    "do",
    "did",
    "can",
    "could",
    "would",
    "will",
    "please",
    "tell",
    "give",
    "show",
    "read",
    "say",
    "me",
    "us",
    "the",
    "a",
    "an",
    "of",
    "for",
    "in",
    "on",
    "about",
    "to",
    "with",
    "this",
    "that",
    "it",
    "its",
    "here",
    "kural",
    "couplet",
    "verse",
    "current",
    "above",
    "simple",
    "easy",
    "short",
    "brief",
    "full",
    "detailed",
    "more",
    "words",
    "lesson",
    "translation",
    "tamil",
    "english",
    "kids",
    "youth",
    "adult",
    "parimel",
    "parimelazhagar",
    "muva",
    "solomon",
    "kalaignar",
    "explain",
    "meaning",
    "mean",
    "story",
    "kutty",
    "moral",
    "commentary",
    "explanation",
    "இந்தக்",
    "இந்த",
    "குறள்",
    "குறளின்",
    "குறளுக்கு",
    "இக்குறள்",
    "இக்குறளின்",
    "இதன்",
    "இதை",
    "இது",
    "பொருள்",
    "எளிய",
    "விளக்கு",
    "விளக்குக",
    "விளக்கம்",
    "விளக்கவும்",
    "கதை",
    "குட்டி",
    "கருத்து",
    "நீதி",
    "உரை",
    "என்ன",
    "கூறு",
    "கூறுக",
    "சொல்",
    "சொல்லு",
    "சொல்லுங்கள்",
    "பற்றி",
    "தரவும்",
];

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum AnchoredTarget {
    Kural(u16),
    KuralOutOfRange,
    Adhikaram(u8),
    AdhikaramOutOfRange,
}

/// Offline, zero-hallucination 3-level Kural Explainer & 4-Point OOS Guardrail (`< 15 ms`).
#[derive(Debug, Clone)]
pub struct SemanticExplainer {
    db: KuralDb,
    ai: AiConfig,
}

impl SemanticExplainer {
    /// Construct a new [`SemanticExplainer`] backed by read-only `KuralDb` and `AiConfig` thresholds.
    pub fn new(db: KuralDb, ai: AiConfig) -> Self {
        Self { db, ai }
    }

    /// Directly explain Kural `number` (`1..=1330`) tailored for `age` and `scholar`,
    /// returning verbatim SQLite fields and automatic `#395`/`#648` `Parimel -> MuVa` fallback.
    pub fn explain_kural(
        &self,
        number: u16,
        age: AgeGroup,
        lang: Language,
        scholar: Scholar,
    ) -> Result<GroundedAnswer, AiError> {
        let _ = lang;
        let started = Instant::now();
        let full = self.db.get_kural_full(number)?;
        let (answer_ta, answer_en) = compose_kural_answer(&full, age, scholar, None);
        Ok(GroundedAnswer {
            answer_ta,
            answer_en,
            cited_kurals: vec![full],
            kb_hits: Vec::new(),
            out_of_scope: false,
            latency_ms: elapsed_ms(started),
        })
    }

    /// Answer a bilingual question through the 4-Point OOS Guardrail and 3-level persona composer.
    pub fn ask_question(
        &self,
        query: &str,
        age: AgeGroup,
        lang: Language,
        seed_kural: Option<u16>,
    ) -> Result<GroundedAnswer, AiError> {
        let _ = lang;
        let started = Instant::now();
        let norm_query = normalize_nfc_trimmed(query);
        if norm_query.is_empty() {
            return Ok(oos_refusal(started));
        }

        let scholar_hint = detect_scholar_hint(&norm_query);

        // Point 1: Anchored Structural/Number Router.
        if let Some(target) = detect_anchored_target(&norm_query) {
            return match target {
                AnchoredTarget::Kural(num) => {
                    let full = self.db.get_kural_full(num)?;
                    let (answer_ta, answer_en) =
                        compose_kural_answer(&full, age, scholar_hint, None);
                    Ok(GroundedAnswer {
                        answer_ta,
                        answer_en,
                        cited_kurals: vec![full],
                        kb_hits: Vec::new(),
                        out_of_scope: false,
                        latency_ms: elapsed_ms(started),
                    })
                }
                AnchoredTarget::KuralOutOfRange => Ok(GroundedAnswer {
                    answer_ta: VOICE_KURAL_RANGE_ERROR.ta.to_string(),
                    answer_en: VOICE_KURAL_RANGE_ERROR.en.to_string(),
                    cited_kurals: Vec::new(),
                    kb_hits: Vec::new(),
                    out_of_scope: true,
                    latency_ms: elapsed_ms(started),
                }),
                AnchoredTarget::Adhikaram(ad_id) => {
                    let kurals = self.db.get_kurals_by_adhikaram(ad_id)?;
                    let Some(first) = kurals.first() else {
                        return Ok(GroundedAnswer {
                            answer_ta: VOICE_ADHIKARAM_RANGE_ERROR.ta.to_string(),
                            answer_en: VOICE_ADHIKARAM_RANGE_ERROR.en.to_string(),
                            cited_kurals: Vec::new(),
                            kb_hits: Vec::new(),
                            out_of_scope: true,
                            latency_ms: elapsed_ms(started),
                        });
                    };
                    let full = self.db.get_kural_full(first.number)?;
                    let (answer_ta, answer_en) =
                        compose_kural_answer(&full, age, scholar_hint, Some(ad_id));
                    Ok(GroundedAnswer {
                        answer_ta,
                        answer_en,
                        cited_kurals: vec![full],
                        kb_hits: Vec::new(),
                        out_of_scope: false,
                        latency_ms: elapsed_ms(started),
                    })
                }
                AnchoredTarget::AdhikaramOutOfRange => Ok(GroundedAnswer {
                    answer_ta: VOICE_ADHIKARAM_RANGE_ERROR.ta.to_string(),
                    answer_en: VOICE_ADHIKARAM_RANGE_ERROR.en.to_string(),
                    cited_kurals: Vec::new(),
                    kb_hits: Vec::new(),
                    out_of_scope: true,
                    latency_ms: elapsed_ms(started),
                }),
            };
        }

        // Point 2: Anaphoric `seed_kural` Gate.
        if let Some(seed_num) = seed_kural
            && (1..=1330).contains(&seed_num)
            && is_anaphoric_seed_query(&norm_query)
        {
            let full = self.db.get_kural_full(seed_num)?;
            let (answer_ta, answer_en) = compose_kural_answer(&full, age, scholar_hint, None);
            return Ok(GroundedAnswer {
                answer_ta,
                answer_en,
                cited_kurals: vec![full],
                kb_hits: Vec::new(),
                out_of_scope: false,
                latency_ms: elapsed_ms(started),
            });
        }

        // Point 3: Stopword-Cleaned Cosine-Filtered RRF Search.
        let Some(prepared) = prepare_search_query(&norm_query) else {
            return Ok(oos_refusal(started));
        };

        let kural_hits = if prepared.substantive_tokens.is_empty() {
            HybridRrfOutcome {
                outcome: SearchOutcome {
                    query: prepared.kural_q.clone(),
                    did_you_mean: None,
                    hits: Vec::new(),
                },
                max_vec_cosine: 0.0,
                max_rrf_score: 0.0,
                has_exact_or_fts_match: false,
            }
        } else {
            self.db.search_hybrid_rrf_no_fuzzy(
                &prepared.kural_q,
                None,
                self.ai.min_vec_candidate_cosine,
                RRF_K,
                KURAL_SEARCH_LIMIT,
            )?
        };
        let kb_results = self.db.search_knowledge_rrf(
            &prepared.kb_q,
            self.ai.min_vec_candidate_cosine,
            RRF_K,
            KB_SEARCH_LIMIT,
        )?;

        // Point 4: Strict Dual Confidence Threshold (with degraded FTS coverage guards).
        let embeddings_loaded = self.db.embeddings_loaded();
        let (kural_confident, prefetched_top_kural) =
            self.evaluate_kural_confidence(&kural_hits, &prepared, embeddings_loaded)?;
        let confident_kb: Vec<KnowledgeRrfResult> = kb_results
            .into_iter()
            .filter(|k| self.is_kb_hit_confident(k, &prepared, embeddings_loaded))
            .collect();

        if !kural_confident && confident_kb.is_empty() {
            return Ok(oos_refusal(started));
        }

        let mut cited_kurals = Vec::new();
        if kural_confident {
            if let Some(top_full) = prefetched_top_kural {
                cited_kurals.push(top_full);
                for hit in kural_hits.outcome.hits.iter().skip(1) {
                    cited_kurals.push(self.db.get_kural_full(hit.kural.number)?);
                }
            } else {
                for hit in &kural_hits.outcome.hits {
                    cited_kurals.push(self.db.get_kural_full(hit.kural.number)?);
                }
            }
        }

        let kb_primary = is_kb_primary(&kural_hits, &confident_kb, &cited_kurals);
        let kb_entries: Vec<KnowledgeEntry> = confident_kb.into_iter().map(|r| r.entry).collect();

        let (answer_ta, answer_en) =
            compose_search_answer(&cited_kurals, &kb_entries, kb_primary, age, scholar_hint);

        Ok(GroundedAnswer {
            answer_ta,
            answer_en,
            cited_kurals,
            kb_hits: kb_entries,
            out_of_scope: false,
            latency_ms: elapsed_ms(started),
        })
    }

    fn evaluate_kural_confidence(
        &self,
        kural_hits: &HybridRrfOutcome,
        prepared: &PreparedQuery,
        embeddings_loaded: bool,
    ) -> Result<(bool, Option<FullKural>), AiError> {
        if kural_hits.outcome.hits.is_empty() {
            return Ok((false, None));
        }

        let top_kind = kural_hits.outcome.hits[0].match_kind;
        let is_exact_or_adhikaram = matches!(
            top_kind,
            SearchMatchKind::ExactNumber | SearchMatchKind::AdhikaramMatch
        );

        // If the query only consisted of corpus meta-words (e.g. "Who is Thiruvalluvar?"),
        // avoid citing an arbitrary Kural matched only via boilerplate unless vector/RRF strongly matched.
        if prepared.substantive_tokens.is_empty() && !is_exact_or_adhikaram {
            return Ok((false, None));
        }

        if embeddings_loaded {
            let has_tamil = prepared
                .kural_q
                .chars()
                .any(|c| ('\u{0B80}'..='\u{0BFF}').contains(&c));
            // Multilingual static subword embeddings (model2vec) cluster arbitrary Tamil strings
            // into a 0.40-0.60 cosine cone; require higher cosine when zero lexical/FTS overlap exists.
            let min_vec_cos = if !kural_hits.has_exact_or_fts_match && has_tamil {
                self.ai.oos_min_vector_cosine.max(0.68)
            } else {
                self.ai.oos_min_vector_cosine
            };
            let confident = kural_hits.max_vec_cosine >= min_vec_cos
                || (kural_hits.has_exact_or_fts_match
                    && kural_hits.max_rrf_score >= self.ai.oos_min_rrf_score);
            return Ok((confident, None));
        }

        // Graceful degraded mode when embeddings are not loaded.
        if !kural_hits.has_exact_or_fts_match {
            return Ok((false, None));
        }
        if is_exact_or_adhikaram {
            return Ok((true, None));
        }

        let top_num = kural_hits.outcome.hits[0].kural.number;
        let top_full = self.db.get_kural_full(top_num)?;
        if prepared.substantive_tokens.len() >= 2
            && !full_kural_matches_multi_tokens(&top_full, &prepared.substantive_tokens)
        {
            return Ok((false, None));
        }
        Ok((true, Some(top_full)))
    }

    fn is_kb_hit_confident(
        &self,
        hit: &KnowledgeRrfResult,
        prepared: &PreparedQuery,
        embeddings_loaded: bool,
    ) -> bool {
        let passes_score = if embeddings_loaded {
            hit.vec_cosine >= self.ai.kb_min_vector_cosine
                || hit.rrf_score >= self.ai.kb_min_rrf_score
        } else {
            hit.has_fts_match
        };
        if !passes_score {
            return false;
        }

        // Prevent a corpus meta-word (e.g. "thirukkural") from validating an OOS query
        // such as "What does Thirukkural say about quantum physics?" via OR fallback.
        if !prepared.substantive_tokens.is_empty() {
            let haystack = knowledge_haystack(&hit.entry);
            let matched = count_matched_tokens(&haystack, &prepared.substantive_tokens);
            if matched == 0 {
                return false;
            }
            if !embeddings_loaded && prepared.substantive_tokens.len() >= 2 && matched < 2 {
                return false;
            }
        }
        true
    }
}

struct PreparedQuery {
    kural_q: String,
    kb_q: String,
    substantive_tokens: Vec<String>,
}

fn elapsed_ms(started: Instant) -> u32 {
    u32::try_from(started.elapsed().as_millis()).unwrap_or(u32::MAX)
}

fn oos_refusal(started: Instant) -> GroundedAnswer {
    GroundedAnswer {
        answer_ta: OOS_CHAT_REFUSAL.ta.to_string(),
        answer_en: OOS_CHAT_REFUSAL.en.to_string(),
        cited_kurals: Vec::new(),
        kb_hits: Vec::new(),
        out_of_scope: true,
        latency_ms: elapsed_ms(started),
    }
}

fn is_tamil_combining_mark(c: char) -> bool {
    ('\u{0B82}'..='\u{0BCD}').contains(&c) || c == '\u{0BD7}'
}

fn is_token_char(c: char) -> bool {
    c.is_alphanumeric() || is_tamil_combining_mark(c)
}

fn tokenize_normalized(norm: &str) -> Vec<String> {
    let mut out = Vec::new();
    for raw in norm.split(|c: char| !is_token_char(c) && c != '\'' && c != '’') {
        let trimmed = raw
            .trim_matches(|c: char| !is_token_char(c))
            .to_ascii_lowercase();
        let clean = trimmed
            .strip_suffix("'s")
            .or_else(|| trimmed.strip_suffix("’s"))
            .unwrap_or(&trimmed)
            .trim_matches(|c: char| !is_token_char(c));
        if !clean.is_empty() {
            out.push(clean.to_string());
        }
    }
    out
}

fn has_arithmetic_expression(norm_query: &str) -> bool {
    if norm_query.contains('*') || norm_query.contains('=') || norm_query.contains('%') {
        return true;
    }
    let chars: Vec<char> = norm_query.chars().collect();
    for (idx, &ch) in chars.iter().enumerate() {
        if matches!(ch, '+' | '/') {
            let left_digit = chars[..idx]
                .iter()
                .rev()
                .find(|c| !c.is_whitespace())
                .is_some_and(|c| c.is_ascii_digit());
            let right_digit = chars[idx + 1..]
                .iter()
                .find(|c| !c.is_whitespace())
                .is_some_and(|c| c.is_ascii_digit());
            if left_digit && right_digit {
                return true;
            }
        }
    }
    false
}

fn prepare_search_query(norm_query: &str) -> Option<PreparedQuery> {
    if has_arithmetic_expression(norm_query) {
        return None;
    }
    let stopword_set: HashSet<&str> = STOPWORDS.iter().copied().collect();
    let meta_set: HashSet<&str> = CORPUS_META_WORDS.iter().copied().collect();

    let tokens = tokenize_normalized(norm_query);
    let non_stop: Vec<String> = tokens
        .into_iter()
        .filter(|t| !stopword_set.contains(t.as_str()))
        .collect();

    if non_stop.is_empty()
        || non_stop
            .iter()
            .all(|t| t.chars().all(|c| c.is_ascii_digit()))
    {
        return None;
    }

    let had_unanchored_number = non_stop
        .iter()
        .any(|t| t.chars().all(|c| c.is_ascii_digit()));
    let non_numeric: Vec<String> = non_stop
        .into_iter()
        .filter(|t| !t.chars().all(|c| c.is_ascii_digit()) && t.chars().count() >= 2)
        .collect();

    if non_numeric.is_empty() {
        return None;
    }
    if non_numeric.len() == 1 {
        let tok = non_numeric[0].as_str();
        if matches!(tok, "today" | "tomorrow" | "yesterday") {
            return None;
        }
        if had_unanchored_number && GENERIC_TEMPORAL_OR_UNIT_WORDS.contains(&tok) {
            return None;
        }
    }

    let substantive_tokens: Vec<String> = non_numeric
        .iter()
        .filter(|t| !meta_set.contains(t.as_str()))
        .cloned()
        .collect();

    let kb_q = non_numeric.join(" ");
    let kural_q = if substantive_tokens.is_empty() {
        kb_q.clone()
    } else {
        substantive_tokens.join(" ")
    };

    Some(PreparedQuery {
        kural_q,
        kb_q,
        substantive_tokens,
    })
}

fn detect_anchored_target(norm_query: &str) -> Option<AnchoredTarget> {
    // Case 1: Entire trimmed query is optional `#` + ASCII digits (allowing trailing ?, !, .).
    let whole = norm_query.trim().trim_end_matches(['?', '!', '.']).trim();
    if !whole.is_empty() {
        let digit_part = whole.strip_prefix('#').unwrap_or(whole);
        if !digit_part.is_empty() && digit_part.chars().all(|c| c.is_ascii_digit()) {
            let val = digit_part.parse::<u32>().unwrap_or(u32::MAX);
            return Some(if (KURAL_MIN..=KURAL_MAX).contains(&val) {
                AnchoredTarget::Kural(val as u16)
            } else {
                AnchoredTarget::KuralOutOfRange
            });
        }
    }

    // Case 2: Anchored prefix/phrase followed by digits anywhere on token boundaries.
    let lower = norm_query.to_ascii_lowercase();
    let anchors: &[(&str, bool)] = &[
        ("அதிகாரம் எண்", true),
        ("அதிகாரம்", true),
        ("adhikaram number", true),
        ("adhikaram no.", true),
        ("adhikaram no", true),
        ("adhikaram", true),
        ("chapter number", true),
        ("chapter no.", true),
        ("chapter no", true),
        ("chapter", true),
        ("திருக்குறள் எண்", false),
        ("திருக்குறள்", false),
        ("குறள் எண்", false),
        ("குறள்", false),
        ("thirukkural number", false),
        ("thirukkural no.", false),
        ("thirukkural no", false),
        ("thirukkural", false),
        ("kural number", false),
        ("kural no.", false),
        ("kural no", false),
        ("kural", false),
        ("couplet number", false),
        ("couplet no.", false),
        ("couplet no", false),
        ("couplet", false),
        ("#", false),
    ];

    let mut best: Option<(usize, AnchoredTarget)> = None;
    for &(anchor, is_adhikaram) in anchors {
        let mut search_start = 0usize;
        while let Some(rel_idx) = lower[search_start..].find(anchor) {
            let idx = search_start + rel_idx;
            let end_idx = idx + anchor.len();
            search_start = end_idx;

            let left_ok = idx == 0
                || lower[..idx]
                    .chars()
                    .next_back()
                    .is_some_and(|c| !is_token_char(c));
            if !left_ok {
                continue;
            }

            let Some(val) = parse_digits_after_anchor(&lower[end_idx..], anchor == "#") else {
                continue;
            };

            let target = if is_adhikaram {
                if (ADHIKARAM_MIN..=ADHIKARAM_MAX).contains(&val) {
                    AnchoredTarget::Adhikaram(val as u8)
                } else {
                    AnchoredTarget::AdhikaramOutOfRange
                }
            } else if (KURAL_MIN..=KURAL_MAX).contains(&val) {
                AnchoredTarget::Kural(val as u16)
            } else {
                AnchoredTarget::KuralOutOfRange
            };

            if best.is_none_or(|(best_idx, _)| idx < best_idx) {
                best = Some((idx, target));
            }
            break;
        }
    }

    if let Some((_, target)) = best {
        return Some(target);
    }

    // Case 3: Spoken Tamil or English Kural / Adhikaram numbers (e.g. "குறள் நூற்று இருபத்தெட்டு",
    // "kural one twenty eight", "நூற்று இருபத்தெட்டு").
    match kural_voice_cmd::extract_anchored_number(norm_query)? {
        kural_voice_cmd::AnchoredNumber::Kural(val)
        | kural_voice_cmd::AnchoredNumber::Bare(val) => {
            Some(if (KURAL_MIN..=KURAL_MAX).contains(&val) {
                AnchoredTarget::Kural(val as u16)
            } else {
                AnchoredTarget::KuralOutOfRange
            })
        }
        kural_voice_cmd::AnchoredNumber::Adhikaram(val) => {
            Some(if (ADHIKARAM_MIN..=ADHIKARAM_MAX).contains(&val) {
                AnchoredTarget::Adhikaram(val as u8)
            } else {
                AnchoredTarget::AdhikaramOutOfRange
            })
        }
    }
}

fn parse_digits_after_anchor(rest: &str, hash_only: bool) -> Option<u32> {
    if hash_only {
        let digits: String = rest.chars().take_while(|c| c.is_ascii_digit()).collect();
        if digits.is_empty() {
            return None;
        }
        let after = &rest[digits.len()..];
        if !valid_post_digit_boundary(after) {
            return None;
        }
        return Some(digits.parse::<u32>().unwrap_or(u32::MAX));
    }

    // Named anchors require at least one separator char (whitespace or #, :, -, .) before digits,
    // preventing false positives like "kurals 10" or "chapters 133".
    let mut byte_pos = 0usize;
    let mut saw_sep = false;
    for ch in rest.chars() {
        if ch.is_whitespace() || matches!(ch, '#' | ':' | '-' | '.') {
            saw_sep = true;
            byte_pos += ch.len_utf8();
        } else {
            break;
        }
    }
    if !saw_sep {
        return None;
    }

    let digit_slice = &rest[byte_pos..];
    let digits: String = digit_slice
        .chars()
        .take_while(|c| c.is_ascii_digit())
        .collect();
    if digits.is_empty() {
        return None;
    }
    let after = &digit_slice[digits.len()..];
    if !valid_post_digit_boundary(after) {
        return None;
    }
    Some(digits.parse::<u32>().unwrap_or(u32::MAX))
}

fn valid_post_digit_boundary(after: &str) -> bool {
    if let Some(next_ch) = after.chars().next()
        && is_token_char(next_ch)
    {
        return false;
    }
    let next_non_ws = after.chars().find(|c| !c.is_whitespace());
    !matches!(next_non_ws, Some('*' | '/' | '+' | '=' | '%'))
}

fn is_anaphoric_seed_query(norm_query: &str) -> bool {
    let lower = norm_query.to_ascii_lowercase();
    let tokens = tokenize_normalized(&lower);
    if tokens.is_empty() {
        return false;
    }

    // Reject explicit non-anaphoric targets like "story about trains" or "meaning of friendship".
    let non_anaphoric_targets = [
        "story about ",
        "story of ",
        "meaning of ",
        "moral of ",
        "commentary on ",
    ];
    for pat in non_anaphoric_targets {
        if let Some(pos) = lower.find(pat) {
            let tail = lower[pos + pat.len()..].trim_start();
            let starts_anaphoric = ["this", "it", "that", "the kural", "kural", "current"]
                .iter()
                .any(|p| tail.starts_with(p));
            if !starts_anaphoric {
                return false;
            }
        }
    }

    let companion_set: HashSet<&str> = ANAPHORIC_COMPANION_TOKENS.iter().copied().collect();
    let all_companion = tokens.iter().all(|t| companion_set.contains(t.as_str()));
    if !all_companion {
        return false;
    }

    if ANAPHORIC_PHRASES.iter().any(|p| lower.contains(p)) {
        return true;
    }
    let intent_set: HashSet<&str> = ANAPHORIC_INTENT_TOKENS.iter().copied().collect();
    tokens.iter().any(|t| intent_set.contains(t.as_str()))
}

fn detect_scholar_hint(norm_query: &str) -> Scholar {
    let lower = norm_query.to_ascii_lowercase();
    if lower.contains("parimel") || lower.contains("பரிமேலழகர்") {
        Scholar::Parimel
    } else if lower.contains("solomon") || lower.contains("சாலமன்") {
        Scholar::Solomon
    } else if lower.contains("kalaignar") || lower.contains("கலைஞர்") {
        Scholar::Kalaignar
    } else {
        Scholar::MuVa
    }
}

fn kural_haystack(full: &FullKural) -> String {
    let mut parts = vec![
        full.division.name_ta.as_str(),
        full.division.name_en.as_str(),
        full.adhikaram.name_ta.as_str(),
        full.adhikaram.name_en.as_str(),
        full.adhikaram.transliteration.as_str(),
        full.kural.line1_ta.as_str(),
        full.kural.line2_ta.as_str(),
        full.kural.transliteration1.as_str(),
        full.kural.transliteration2.as_str(),
        full.kural.meaning_mu_va.as_str(),
        full.kural.meaning_solomon.as_str(),
        full.kural.meaning_kalaignar.as_str(),
        full.kural.translation_en.as_str(),
        full.kural.explanation_en.as_str(),
        full.kural.couplet_en.as_str(),
    ];
    if let Some(child) = &full.child {
        parts.push(child.simple_meaning_ta.as_str());
        parts.push(child.simple_meaning_en.as_str());
        parts.push(child.kutty_story_ta.as_str());
        parts.push(child.kutty_story_en.as_str());
    }
    if let Some(youth) = &full.youth {
        parts.push(youth.analysis_ta.as_str());
        parts.push(youth.analysis_en.as_str());
    }
    let mut out = parts.join(" ");
    for tag in &full.tags {
        out.push(' ');
        out.push_str(&tag.slug);
        out.push(' ');
        out.push_str(&tag.name_ta);
        out.push(' ');
        out.push_str(&tag.name_en);
    }
    out
}

fn knowledge_haystack(entry: &KnowledgeEntry) -> String {
    format!(
        "{} {} {} {} {}",
        entry.question_ta, entry.question_en, entry.answer_ta, entry.answer_en, entry.keywords
    )
}

fn count_matched_tokens(haystack: &str, query_tokens: &[String]) -> usize {
    let hay_tokens = tokenize_normalized(&normalize_nfc_trimmed(haystack).to_ascii_lowercase());
    query_tokens
        .iter()
        .filter(|q| token_matches_hay_list(q, &hay_tokens))
        .count()
}

fn token_matches_hay_list(q: &str, hay_tokens: &[String]) -> bool {
    let q_chars = q.chars().count();
    let q_stem: String = q.chars().take(5).collect();
    hay_tokens.iter().any(|h| {
        if h == q {
            return true;
        }
        if q_chars >= 3 && h.starts_with(q) {
            return true;
        }
        if q_chars >= 5 && h.chars().count() >= 5 && h.starts_with(&q_stem) {
            return true;
        }
        false
    })
}

fn full_kural_matches_multi_tokens(full: &FullKural, tokens: &[String]) -> bool {
    if tokens.len() < 2 {
        return true;
    }
    let haystack = kural_haystack(full);
    count_matched_tokens(&haystack, tokens) >= 2
}

fn is_kb_primary(
    kural_hits: &HybridRrfOutcome,
    confident_kb: &[KnowledgeRrfResult],
    cited_kurals: &[FullKural],
) -> bool {
    let Some(top_kb) = confident_kb.first() else {
        return false;
    };
    if cited_kurals.is_empty() {
        return true;
    }
    if let Some(top_kural) = kural_hits.outcome.hits.first()
        && matches!(
            top_kural.match_kind,
            SearchMatchKind::ExactNumber | SearchMatchKind::AdhikaramMatch
        )
    {
        return false;
    }
    top_kb.rrf_score > kural_hits.max_rrf_score
        || top_kb.vec_cosine > kural_hits.max_vec_cosine
        || matches!(
            top_kb.entry.category.as_str(),
            "biography"
                | "monuments"
                | "structure"
                | "literary"
                | "translations"
                | "commentaries"
                | "faq"
        )
}

fn compose_kural_answer(
    full: &FullKural,
    age: AgeGroup,
    scholar: Scholar,
    adhikaram_id: Option<u8>,
) -> (String, String) {
    let num = full.kural.number;
    let header_ta = if let Some(ad_id) = adhikaram_id {
        format!(
            "அதிகாரம் {ad_id}: {} • குறள் {num} ({})\n{}\n{}",
            full.adhikaram.name_ta, full.division.name_ta, full.kural.line1_ta, full.kural.line2_ta
        )
    } else {
        format!(
            "குறள் {num} ({} • {})\n{}\n{}",
            full.division.name_ta, full.adhikaram.name_ta, full.kural.line1_ta, full.kural.line2_ta
        )
    };
    let header_en = if let Some(ad_id) = adhikaram_id {
        format!(
            "Chapter {ad_id}: {} - Kural {num} ({})\n{} / {}\nTranslation: {}",
            full.adhikaram.name_en,
            full.division.name_en,
            full.kural.transliteration1,
            full.kural.transliteration2,
            full.kural.translation_en
        )
    } else {
        format!(
            "Kural {num} ({} - {})\n{} / {}\nTranslation: {}",
            full.division.name_en,
            full.adhikaram.name_en,
            full.kural.transliteration1,
            full.kural.transliteration2,
            full.kural.translation_en
        )
    };

    match age {
        AgeGroup::Kids => {
            let (body_ta, body_en) = if let Some(child) = &full.child {
                let moral_ta = if child.moral_ta.starts_with("நீதி") {
                    child.moral_ta.clone()
                } else {
                    format!("நீதி: {}", child.moral_ta)
                };
                let moral_en = if child.moral_en.starts_with("Moral") {
                    child.moral_en.clone()
                } else {
                    format!("Moral: {}", child.moral_en)
                };
                (
                    format!(
                        "{}\nகதை: {}\n{}",
                        child.simple_meaning_ta, child.story_title_ta, moral_ta
                    ),
                    format!(
                        "{}\nStory: {}\n{}",
                        child.simple_meaning_en, child.story_title_en, moral_en
                    ),
                )
            } else {
                (
                    format!("பொருள்: {}", full.kural.meaning_mu_va),
                    format!("Explanation: {}", full.kural.explanation_en),
                )
            };
            (
                format!("{header_ta}\n\n{body_ta}"),
                format!("{header_en}\n\n{body_en}"),
            )
        }
        AgeGroup::Youth => {
            let (body_ta, body_en) = if let Some(youth) = &full.youth {
                (
                    format!(
                        "{}\nசெயல் குறிப்பு: {}",
                        youth.analysis_ta, youth.action_tip_ta
                    ),
                    format!("{}\nAction tip: {}", youth.analysis_en, youth.action_tip_en),
                )
            } else {
                (
                    format!("பொருள்: {}", full.kural.meaning_mu_va),
                    format!("Explanation: {}", full.kural.explanation_en),
                )
            };
            (
                format!("{header_ta}\n\n{body_ta}"),
                format!("{header_en}\n\n{body_en}"),
            )
        }
        AgeGroup::Adult => {
            let (effective_scholar, fallback_used) = match scholar {
                Scholar::Parimel if full.kural.meaning_parimel.is_none() => (Scholar::MuVa, true),
                other => (other, false),
            };
            let commentary_ta = match effective_scholar {
                Scholar::MuVa | Scholar::English => {
                    format!("மு.வ. உரை: {}", full.kural.meaning_mu_va)
                }
                Scholar::Solomon => {
                    format!("சாலமன் பாப்பையா உரை: {}", full.kural.meaning_solomon)
                }
                Scholar::Kalaignar => {
                    format!("கலைஞர் உரை: {}", full.kural.meaning_kalaignar)
                }
                Scholar::Parimel => match &full.kural.meaning_parimel {
                    Some(parimel) => format!("பரிமேலழகர் உரை: {parimel}"),
                    None => format!("மு.வ. உரை: {}", full.kural.meaning_mu_va),
                },
            };

            let mut ta_sections = vec![header_ta];
            if fallback_used {
                ta_sections.push(PARIMEL_FALLBACK_NOTICE.ta.to_string());
            }
            ta_sections.push(commentary_ta);
            if let Some(adult) = &full.adult {
                ta_sections.push(adult.reflection_ta.clone());
            }

            let mut en_sections = vec![header_en];
            if fallback_used {
                en_sections.push(PARIMEL_FALLBACK_NOTICE.en.to_string());
            }
            en_sections.push(format!("Explanation: {}", full.kural.explanation_en));
            if let Some(adult) = &full.adult {
                en_sections.push(format!(
                    "{} [Theme: {}]",
                    adult.reflection_en, adult.research_theme
                ));
            }

            (ta_sections.join("\n\n"), en_sections.join("\n\n"))
        }
    }
}

fn compose_kb_sections(kb_hits: &[KnowledgeEntry]) -> (Vec<String>, Vec<String>) {
    let mut ta = Vec::with_capacity(kb_hits.len());
    let mut en = Vec::with_capacity(kb_hits.len());
    for entry in kb_hits {
        ta.push(format!("{}\n{}", entry.question_ta, entry.answer_ta));
        en.push(format!("{}\n{}", entry.question_en, entry.answer_en));
    }
    (ta, en)
}

fn compose_additional_kurals(extras: &[FullKural]) -> (Vec<String>, Vec<String>) {
    let mut ta = Vec::with_capacity(extras.len());
    let mut en = Vec::with_capacity(extras.len());
    for full in extras {
        ta.push(format!(
            "தொடர்புடைய குறள் {} ({}):\n{}\n{}",
            full.kural.number, full.adhikaram.name_ta, full.kural.line1_ta, full.kural.line2_ta
        ));
        en.push(format!(
            "Related Kural {} ({}): {}",
            full.kural.number, full.adhikaram.name_en, full.kural.translation_en
        ));
    }
    (ta, en)
}

fn compose_search_answer(
    cited_kurals: &[FullKural],
    kb_hits: &[KnowledgeEntry],
    kb_primary: bool,
    age: AgeGroup,
    scholar: Scholar,
) -> (String, String) {
    let mut ta_parts = Vec::new();
    let mut en_parts = Vec::new();

    if kb_primary {
        let (kb_ta, kb_en) = compose_kb_sections(kb_hits);
        ta_parts.extend(kb_ta);
        en_parts.extend(kb_en);
        if let Some(primary) = cited_kurals.first() {
            let (k_ta, k_en) = compose_kural_answer(primary, age, scholar, None);
            ta_parts.push(k_ta);
            en_parts.push(k_en);
            let (extra_ta, extra_en) = compose_additional_kurals(&cited_kurals[1..]);
            ta_parts.extend(extra_ta);
            en_parts.extend(extra_en);
        }
    } else {
        if let Some(primary) = cited_kurals.first() {
            let (k_ta, k_en) = compose_kural_answer(primary, age, scholar, None);
            ta_parts.push(k_ta);
            en_parts.push(k_en);
            let (extra_ta, extra_en) = compose_additional_kurals(&cited_kurals[1..]);
            ta_parts.extend(extra_ta);
            en_parts.extend(extra_en);
        }
        if let Some(top_kb) = kb_hits.first() {
            ta_parts.push(format!("{}\n{}", top_kb.question_ta, top_kb.answer_ta));
            en_parts.push(format!("{}\n{}", top_kb.question_en, top_kb.answer_en));
        }
    }

    (ta_parts.join("\n\n"), en_parts.join("\n\n"))
}

#[cfg(test)]
mod tests {
    use super::*;
    use kural_core::{is_nfc, load_and_audit_canonical};
    use kural_db::{
        DbError, TestHashEmbedder, TextEmbedder, compile_database, index_database_embeddings,
        load_enriched_dir, load_knowledge_base, resolve_db_path,
    };
    use std::path::PathBuf;
    use std::sync::Arc;
    use tempfile::TempDir;

    fn workspace_root() -> PathBuf {
        PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../..")
    }

    fn open_or_compile_test_db() -> (Option<TempDir>, PathBuf) {
        if let Ok(path) = resolve_db_path(None) {
            return (None, path);
        }
        let root = workspace_root();
        let canonical =
            load_and_audit_canonical(&root.join("seed_data/canonical")).expect("canonical");
        let enriched =
            load_enriched_dir(&root.join("seed_data/enriched"), &canonical).expect("enriched");
        let knowledge =
            load_knowledge_base(&root.join("seed_data/knowledge_base.json")).expect("kb");
        let tmp = TempDir::new().expect("tempdir");
        let db_path = tmp.path().join("thirukkural.db");
        compile_database(&canonical, &enriched, &knowledge, &db_path, true)
            .expect("compile test db");
        (Some(tmp), db_path)
    }

    fn build_vector_indexed_db() -> (TempDir, KuralDb) {
        let (_maybe_tmp, src_db) = open_or_compile_test_db();
        let dir = TempDir::new().expect("tempdir");
        let vec_db_path = dir.path().join("thirukkural_vec.db");
        std::fs::copy(&src_db, &vec_db_path).expect("copy db");

        let embedder = TestHashEmbedder;
        index_database_embeddings(&vec_db_path, &embedder, "test_hash")
            .expect("index hash embeddings");
        let db = KuralDb::open(&vec_db_path)
            .expect("open vec db")
            .with_embedder(Arc::new(embedder));
        assert!(db.embeddings_loaded());
        (dir, db)
    }

    /// Controlled deterministic embedder for testing exact rank-100 low-cosine noise rejection.
    struct ControlledNoiseEmbedder;

    impl TextEmbedder for ControlledNoiseEmbedder {
        fn dim(&self) -> usize {
            8
        }

        fn embed_f32(&self, text: &str) -> Result<Vec<f32>, DbError> {
            let lower = text.to_ascii_lowercase();
            if lower.contains("noiseprobelow") {
                // Cosine with [1, 0, ...] is 0.20 (< min_vec_candidate_cosine 0.25).
                return Ok(vec![0.20, 0.979_795_9, 0.0, 0.0, 0.0, 0.0, 0.0, 0.0]);
            }
            if lower.contains("noiseprobeborderline") {
                // Cosine with [1, 0, ...] is 0.30 (>= 0.25, < oos_min_vector_cosine 0.36).
                // Rank-1 vector-only RRF = 1/61 ≈ 0.01639 (< 0.022) -> must still be refused!
                return Ok(vec![0.30, 0.953_939_2, 0.0, 0.0, 0.0, 0.0, 0.0, 0.0]);
            }
            if text.contains("அஆஇஈ") {
                // Simulates model2vec multilingual Tamil subword cone (~0.55 cosine with 0 FTS hits).
                return Ok(vec![0.55, 0.835_164_7, 0.0, 0.0, 0.0, 0.0, 0.0, 0.0]);
            }
            if lower.contains("semanticprobestrong") {
                // Cosine with [1, 0, ...] is 0.95 (>= 0.36) -> accepted even with 0 FTS hits!
                return Ok(vec![0.95, 0.312_249_9, 0.0, 0.0, 0.0, 0.0, 0.0, 0.0]);
            }
            Ok(vec![1.0, 0.0, 0.0, 0.0, 0.0, 0.0, 0.0, 0.0])
        }
    }

    #[test]
    fn explain_kural_delivers_3_levels_and_parimel_fallback() {
        let (_tmp, db_path) = open_or_compile_test_db();
        let db = KuralDb::open(&db_path).expect("open db");
        let explainer = SemanticExplainer::new(db, AiConfig::default());

        // Kids mode #1
        let kids = explainer
            .explain_kural(1, AgeGroup::Kids, Language::Tamil, Scholar::MuVa)
            .expect("explain kural 1 kids");
        assert!(!kids.out_of_scope);
        assert_eq!(kids.cited_kurals.len(), 1);
        assert_eq!(kids.cited_kurals[0].kural.number, 1);
        assert!(kids.answer_ta.contains("அகர முதல எழுத்தெல்லாம்"));
        assert!(
            kids.answer_ta
                .contains(&kids.cited_kurals[0].child.as_ref().unwrap().story_title_ta)
        );
        assert!(
            kids.answer_ta
                .contains(&kids.cited_kurals[0].child.as_ref().unwrap().moral_ta)
        );
        assert!(
            kids.answer_en.contains(
                &kids.cited_kurals[0]
                    .child
                    .as_ref()
                    .unwrap()
                    .simple_meaning_en
            )
        );
        assert!(
            kids.answer_en
                .contains(&kids.cited_kurals[0].child.as_ref().unwrap().story_title_en)
        );
        assert!(
            kids.answer_en
                .chars()
                .all(|c| !('\u{0B80}'..='\u{0BFF}').contains(&c))
        );
        assert!(is_nfc(&kids.answer_ta));
        let budget_ms = if cfg!(debug_assertions) { 80 } else { 20 };
        assert!(
            kids.latency_ms <= budget_ms,
            "kids latency {} ms",
            kids.latency_ms
        );

        // Youth mode #481
        let youth = explainer
            .explain_kural(481, AgeGroup::Youth, Language::English, Scholar::MuVa)
            .expect("explain kural 481 youth");
        assert!(!youth.out_of_scope);
        assert!(
            youth
                .answer_ta
                .contains(&youth.cited_kurals[0].youth.as_ref().unwrap().action_tip_ta)
        );
        assert!(
            youth
                .answer_en
                .contains(&youth.cited_kurals[0].youth.as_ref().unwrap().analysis_en)
        );
        assert!(
            youth
                .answer_en
                .contains(&youth.cited_kurals[0].youth.as_ref().unwrap().action_tip_en)
        );
        assert!(
            youth.latency_ms <= budget_ms,
            "youth latency {} ms",
            youth.latency_ms
        );

        // Adult mode #1 with Solomon, Kalaignar, English, and Parimel present (no fallback notice)
        let solomon = explainer
            .explain_kural(1, AgeGroup::Adult, Language::Tamil, Scholar::Solomon)
            .expect("explain kural 1 solomon");
        assert!(
            solomon
                .answer_ta
                .contains(&solomon.cited_kurals[0].kural.meaning_solomon)
        );
        let kalaignar = explainer
            .explain_kural(1, AgeGroup::Adult, Language::Tamil, Scholar::Kalaignar)
            .expect("explain kural 1 kalaignar");
        assert!(
            kalaignar
                .answer_ta
                .contains(&kalaignar.cited_kurals[0].kural.meaning_kalaignar)
        );
        let english_scholar = explainer
            .explain_kural(1, AgeGroup::Adult, Language::English, Scholar::English)
            .expect("explain kural 1 english scholar");
        assert!(
            english_scholar
                .answer_ta
                .contains(&english_scholar.cited_kurals[0].kural.meaning_mu_va)
        );
        assert!(
            english_scholar
                .answer_en
                .contains(&english_scholar.cited_kurals[0].kural.explanation_en)
        );

        let adult_1 = explainer
            .explain_kural(1, AgeGroup::Adult, Language::Tamil, Scholar::Parimel)
            .expect("explain kural 1 adult parimel");
        assert!(!adult_1.answer_ta.contains(PARIMEL_FALLBACK_NOTICE.ta));
        assert!(adult_1.answer_ta.contains("பரிமேலழகர் உரை:"));

        // Adult mode #395 and #648 with Parimel NULL -> fallback notice + Mu.Va.
        for num in [395u16, 648u16] {
            let adult = explainer
                .explain_kural(num, AgeGroup::Adult, Language::Tamil, Scholar::Parimel)
                .expect("explain parimel fallback");
            assert!(!adult.out_of_scope);
            assert!(adult.cited_kurals[0].kural.meaning_parimel.is_none());
            assert!(
                adult.answer_ta.contains(PARIMEL_FALLBACK_NOTICE.ta),
                "missing ta fallback notice for #{num}"
            );
            assert!(
                adult.answer_en.contains(PARIMEL_FALLBACK_NOTICE.en),
                "missing en fallback notice for #{num}"
            );
            assert!(
                adult
                    .answer_ta
                    .contains(&adult.cited_kurals[0].kural.meaning_mu_va)
            );
        }

        // Invalid kural numbers fail cleanly
        assert!(matches!(
            explainer.explain_kural(0, AgeGroup::Adult, Language::Tamil, Scholar::MuVa),
            Err(AiError::Db(DbError::KuralNotFound(0)))
        ));
        assert!(matches!(
            explainer.explain_kural(1331, AgeGroup::Adult, Language::Tamil, Scholar::MuVa),
            Err(AiError::Db(DbError::KuralNotFound(1331)))
        ));
    }

    #[test]
    fn point1_anchored_numbers_and_adhikaram_route_cleanly() {
        let (_tmp, db_path) = open_or_compile_test_db();
        let db = KuralDb::open(&db_path).expect("open db");
        let explainer = SemanticExplainer::new(db, AiConfig::default());

        let cases = [
            ("குறள் 481", 481u16),
            ("குறள் எண் 10", 10u16),
            ("kural #1", 1u16),
            ("kural 481", 481u16),
            ("couplet 42", 42u16),
            ("thirukkural 1", 1u16),
            ("#481", 481u16),
            ("481", 481u16),
            ("adhikaram 1", 1u16),
            ("adhikaram 10", 91u16),
            ("chapter 49", 481u16),
            ("அதிகாரம் 10", 91u16),
            ("குறள் நூற்று இருபத்தெட்டு", 128u16),
            ("நூற்று இருபத்தெட்டு", 128u16),
            ("kural one twenty eight", 128u16),
            ("one hundred twenty eight", 128u16),
        ];

        for (query, expected_kural) in cases {
            let ans = explainer
                .ask_question(query, AgeGroup::Youth, Language::English, None)
                .expect(query);
            assert!(!ans.out_of_scope, "expected in-scope for '{query}'");
            assert_eq!(ans.cited_kurals.len(), 1, "expected 1 kural for '{query}'");
            assert_eq!(
                ans.cited_kurals[0].kural.number, expected_kural,
                "wrong kural for '{query}'"
            );
            let budget_ms = if cfg!(debug_assertions) { 80 } else { 20 };
            assert!(
                ans.latency_ms <= budget_ms,
                "latency {} ms for '{query}'",
                ans.latency_ms
            );
        }

        // Point 1 overrides seed_kural when explicit anchored kural is given,
        // and scholar hint ("parimel") triggers #395 fallback cleanly.
        let explicit_over_seed = explainer
            .ask_question("kural 481", AgeGroup::Youth, Language::English, Some(1))
            .expect("explicit over seed");
        assert_eq!(explicit_over_seed.cited_kurals[0].kural.number, 481);

        let parimel_hint = explainer
            .ask_question(
                "Explain kural 395 Parimel commentary",
                AgeGroup::Adult,
                Language::Tamil,
                None,
            )
            .expect("parimel hint on 395");
        assert!(!parimel_hint.out_of_scope);
        assert!(parimel_hint.answer_ta.contains(PARIMEL_FALLBACK_NOTICE.ta));
        assert!(parimel_hint.answer_en.contains(PARIMEL_FALLBACK_NOTICE.en));

        // Out-of-range explicit anchored numbers return locked range messages with out_of_scope: true
        for bad_kural in [
            "kural 0",
            "kural 1331",
            "#0",
            "#1500",
            "குறள் 2000",
            "0",
            "1331",
        ] {
            let ans = explainer
                .ask_question(bad_kural, AgeGroup::Adult, Language::Tamil, None)
                .expect(bad_kural);
            assert!(ans.out_of_scope, "expected OOS for '{bad_kural}'");
            assert!(ans.cited_kurals.is_empty());
            assert_eq!(ans.answer_ta, VOICE_KURAL_RANGE_ERROR.ta);
            assert_eq!(ans.answer_en, VOICE_KURAL_RANGE_ERROR.en);
        }
        for bad_ad in ["adhikaram 0", "chapter 134", "அதிகாரம் 999"] {
            let ans = explainer
                .ask_question(bad_ad, AgeGroup::Adult, Language::Tamil, None)
                .expect(bad_ad);
            assert!(ans.out_of_scope, "expected OOS for '{bad_ad}'");
            assert!(ans.cited_kurals.is_empty());
            assert_eq!(ans.answer_ta, VOICE_ADHIKARAM_RANGE_ERROR.ta);
            assert_eq!(ans.answer_en, VOICE_ADHIKARAM_RANGE_ERROR.en);
        }
    }

    #[test]
    fn point2_anaphoric_seed_kural_gate_protects_against_unrelated_queries() {
        let (_tmp, db_path) = open_or_compile_test_db();
        let db = KuralDb::open(&db_path).expect("open db");
        let explainer = SemanticExplainer::new(db, AiConfig::default());

        let anaphoric_queries = [
            "explain this",
            "meaning",
            "what does this mean",
            "story",
            "moral",
            "commentary",
            "simple meaning",
            "இந்தக் குறள்",
            "இதன் பொருள்",
            "பொருள்",
            "விளக்கு",
            "விளக்கம்",
            "கதை",
            "கருத்து",
        ];
        for q in anaphoric_queries {
            let ans = explainer
                .ask_question(q, AgeGroup::Kids, Language::Tamil, Some(1))
                .expect(q);
            assert!(!ans.out_of_scope, "expected seed resolution for '{q}'");
            assert_eq!(ans.cited_kurals.len(), 1);
            assert_eq!(ans.cited_kurals[0].kural.number, 1);
        }

        // Unrelated queries with seed_kural: Some(1) MUST ignore seed_kural and refuse as OOS.
        for oos in [
            "What is 42 * 10?",
            "Tell me a joke about trains",
            "Tell me Python quicksort",
            "Tell me a story about trains",
            "What is the meaning of quantum physics?",
        ] {
            let ans = explainer
                .ask_question(oos, AgeGroup::Youth, Language::English, Some(1))
                .expect(oos);
            assert!(ans.out_of_scope, "seed_kural bypassed OOS for '{oos}'");
            assert!(ans.cited_kurals.is_empty());
            assert!(ans.kb_hits.is_empty());
            assert_eq!(ans.answer_en, OOS_CHAT_REFUSAL.en);
        }

        // Anaphoric without seed_kural must also refuse cleanly.
        let no_seed = explainer
            .ask_question("explain this", AgeGroup::Youth, Language::English, None)
            .expect("explain this without seed");
        assert!(no_seed.out_of_scope);
    }

    #[test]
    fn point3_and_4_reject_oos_and_accept_grounded_queries_degraded_and_vector_modes() {
        let (_tmp, db_path) = open_or_compile_test_db();
        let degraded_db = KuralDb::open(&db_path).expect("open degraded db");
        let degraded = SemanticExplainer::new(degraded_db, AiConfig::default());

        let (_vec_dir, vec_db) = build_vector_indexed_db();
        let vectorized = SemanticExplainer::new(vec_db, AiConfig::default());

        let early_oos_queries = [
            "",
            "   ",
            "வ எ",
            "What is 42 * 10?",
            "Article 370",
            "year 2026",
            "500 rupees",
            "weather today",
        ];
        let fuzzy_miss_oos_queries = [
            "Tell me Python quicksort",
            "Tell me a joke about trains",
            "Write Rust code",
            "What does Thirukkural say about quantum physics?",
        ];

        for explainer in [&degraded, &vectorized] {
            for q in early_oos_queries {
                let ans = explainer
                    .ask_question(q, AgeGroup::Youth, Language::English, None)
                    .expect(q);
                assert!(ans.out_of_scope, "expected OOS refusal for '{q}'");
                assert!(
                    ans.cited_kurals.is_empty(),
                    "cited_kurals non-empty for '{q}'"
                );
                assert!(ans.kb_hits.is_empty(), "kb_hits non-empty for '{q}'");
                assert_eq!(ans.answer_ta, OOS_CHAT_REFUSAL.ta);
                assert_eq!(ans.answer_en, OOS_CHAT_REFUSAL.en);
                let budget_ms = if cfg!(debug_assertions) { 80 } else { 20 };
                assert!(
                    ans.latency_ms <= budget_ms,
                    "OOS latency {} ms for '{q}'",
                    ans.latency_ms
                );
            }
            // Note: queries with zero FTS and zero vector hits enter kural_db's internal
            // `run_fuzzy_fallback` scan inside `search_hybrid_rrf`; verify exact refusal invariants.
            for q in fuzzy_miss_oos_queries {
                let ans = explainer
                    .ask_question(q, AgeGroup::Youth, Language::English, None)
                    .expect(q);
                assert!(ans.out_of_scope, "expected OOS refusal for '{q}'");
                assert!(
                    ans.cited_kurals.is_empty(),
                    "cited_kurals non-empty for '{q}'"
                );
                assert!(ans.kb_hits.is_empty(), "kb_hits non-empty for '{q}'");
                assert_eq!(ans.answer_ta, OOS_CHAT_REFUSAL.ta);
                assert_eq!(ans.answer_en, OOS_CHAT_REFUSAL.en);
            }
        }

        // Grounded Kurals & KB queries in degraded mode
        for q in [
            "friendship",
            "What does Thirukkural say about controlling anger?",
            "கல்வி",
            "நட்பு பற்றி கூறு",
            "Who is Thiruvalluvar?",
            "Who was Thiruvalluvar's wife?",
            "What is the Kanyakumari statue?",
            "திருவள்ளுவர் யார்?",
        ] {
            let ans = degraded
                .ask_question(q, AgeGroup::Adult, Language::English, None)
                .expect(q);
            assert!(!ans.out_of_scope, "expected grounded answer for '{q}'");
            assert!(
                !ans.cited_kurals.is_empty() || !ans.kb_hits.is_empty(),
                "expected citations or kb_hits for '{q}'"
            );
            if !ans.kb_hits.is_empty() && ans.cited_kurals.is_empty() {
                assert!(ans.answer_ta.contains(&ans.kb_hits[0].question_ta));
                assert!(ans.answer_ta.contains(&ans.kb_hits[0].answer_ta));
                assert!(ans.answer_en.contains(&ans.kb_hits[0].question_en));
                assert!(ans.answer_en.contains(&ans.kb_hits[0].answer_en));
            }
            let budget_ms = if cfg!(debug_assertions) { 80 } else { 20 };
            assert!(
                ans.latency_ms <= budget_ms,
                "latency {} ms for '{q}'",
                ans.latency_ms
            );
        }

        // Grounded Adhikaram + KB queries in vectorized mode
        for q in [
            "friendship",
            "gratitude",
            "கல்வி",
            "Kanyakumari 133 feet statue memorial monument",
            "Who is Thiruvalluvar?",
        ] {
            let ans = vectorized
                .ask_question(q, AgeGroup::Youth, Language::Tamil, None)
                .expect(q);
            assert!(
                !ans.out_of_scope,
                "expected vectorized grounded answer for '{q}'"
            );
            let budget_ms = if cfg!(debug_assertions) { 80 } else { 20 };
            assert!(
                ans.latency_ms <= budget_ms,
                "vec latency {} ms for '{q}'",
                ans.latency_ms
            );
        }
    }

    #[test]
    fn low_cosine_rank100_noise_vector_is_rejected_while_strong_vector_passes() {
        let (_maybe_tmp, src_db) = open_or_compile_test_db();
        let dir = TempDir::new().expect("tempdir");
        let db_path = dir.path().join("controlled_vec.db");
        std::fs::copy(&src_db, &db_path).expect("copy db");

        let embedder = ControlledNoiseEmbedder;
        index_database_embeddings(&db_path, &embedder, "controlled_noise")
            .expect("index controlled vectors");
        let db = KuralDb::open(&db_path)
            .expect("open controlled db")
            .with_embedder(Arc::new(embedder));
        let explainer = SemanticExplainer::new(db, AiConfig::default());

        // vec_cosine = 0.20 (< min_vec_candidate_cosine 0.25) -> excluded pre-RRF -> OOS
        let low = explainer
            .ask_question("noiseprobelow", AgeGroup::Youth, Language::English, None)
            .expect("noiseprobelow");
        assert!(low.out_of_scope);
        assert!(low.cited_kurals.is_empty());
        assert!(low.kb_hits.is_empty());
        assert_eq!(low.answer_en, OOS_CHAT_REFUSAL.en);

        // vec_cosine = 0.30 (>= 0.25, < 0.36; rank-1 vector-only RRF = 1/61 < 0.022) -> OOS
        let borderline = explainer
            .ask_question(
                "noiseprobeborderline",
                AgeGroup::Youth,
                Language::English,
                None,
            )
            .expect("noiseprobeborderline");
        assert!(borderline.out_of_scope);
        assert!(borderline.cited_kurals.is_empty());
        assert!(borderline.kb_hits.is_empty());

        // Tamil gibberish with model2vec-like 0.55 cosine and 0 FTS hits -> refused as OOS!
        let tamil_noise = explainer
            .ask_question("அஆஇஈ", AgeGroup::Youth, Language::Tamil, None)
            .expect("tamil_noise");
        assert!(tamil_noise.out_of_scope);
        assert!(tamil_noise.cited_kurals.is_empty());
        assert!(tamil_noise.kb_hits.is_empty());
        assert_eq!(tamil_noise.answer_ta, OOS_CHAT_REFUSAL.ta);

        // vec_cosine = 0.95 (>= 0.36) -> passes cleanly
        let strong = explainer
            .ask_question(
                "semanticprobestrong",
                AgeGroup::Adult,
                Language::English,
                None,
            )
            .expect("semanticprobestrong");
        assert!(!strong.out_of_scope);
        assert!(!strong.cited_kurals.is_empty() || !strong.kb_hits.is_empty());
        let budget_ms = if cfg!(debug_assertions) { 80 } else { 20 };
        assert!(strong.latency_ms <= budget_ms);
    }
}
