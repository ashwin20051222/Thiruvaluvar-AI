use kural_core::{
    AppScreen, FullKural, GroundedAnswer, Language, Scholar, SpeakTarget, snap_speech_speed,
};

/// Resolve the effective commentary scholar for `full`, falling back from `Parimel` to `MuVa`
/// whenever `meaning_parimel` is `None` (Kurals `#395` and `#648`).
#[must_use]
pub fn resolve_effective_scholar(full: &FullKural, requested: Scholar) -> Scholar {
    if requested == Scholar::Parimel && full.kural.meaning_parimel.is_none() {
        Scholar::MuVa
    } else {
        requested
    }
}

/// Spoken payload and deterministic PCM LRU cache key for a [`SpeakTarget`].
pub struct SpeechPayload {
    pub chunks: Vec<String>,
    pub cache_key: Option<String>,
}

fn non_empty_english_couplet(full: &FullKural) -> String {
    let tts = full.kural.tts_kural_en.trim();
    if !tts.is_empty() {
        return tts.to_string();
    }
    let tr = full.kural.translation_en.trim();
    if !tr.is_empty() {
        return tr.to_string();
    }
    full.kural.couplet_en.trim().to_string()
}

fn push_non_empty(dst: &mut Vec<String>, text: &str) {
    let t = text.trim();
    if !t.is_empty() {
        dst.push(t.to_string());
    }
}

/// Map `(full, screen, target, lang, user_speed, last_ai_answer)` into TTS chunks and cache key
/// following Section 4.5 of `backend_master_approved.md`.
///
/// Crucially, when `Language::English` is active, only English text is emitted so `en_US-lessac`
/// never attempts to speak raw Tamil script.
#[must_use]
pub fn build_speech_payload(
    full: &FullKural,
    screen: AppScreen,
    target: SpeakTarget,
    lang: Language,
    user_speed: f32,
    last_ai_answer: Option<&GroundedAnswer>,
) -> Option<SpeechPayload> {
    let mut chunks = Vec::new();
    let speed_pct = (snap_speech_speed(user_speed) * 100.0).round() as u32;

    let resolved_target = match target {
        SpeakTarget::ScholarCommentary(s) => {
            SpeakTarget::ScholarCommentary(resolve_effective_scholar(full, s))
        }
        other => other,
    };

    match resolved_target {
        SpeakTarget::KuralOnly => match lang {
            Language::Tamil => {
                push_non_empty(&mut chunks, &full.kural.line1_ta);
                push_non_empty(&mut chunks, &full.kural.line2_ta);
            }
            Language::English => {
                push_non_empty(&mut chunks, &non_empty_english_couplet(full));
            }
        },
        SpeakTarget::KuralAndMeaning => match lang {
            Language::Tamil => {
                push_non_empty(&mut chunks, &full.kural.line1_ta);
                push_non_empty(&mut chunks, &full.kural.line2_ta);
                let meaning = if screen == AppScreen::Kids {
                    full.child
                        .as_ref()
                        .map(|c| c.simple_meaning_ta.as_str())
                        .unwrap_or(&full.kural.meaning_mu_va)
                } else {
                    full.kural.meaning_mu_va.as_str()
                };
                push_non_empty(&mut chunks, meaning);
            }
            Language::English => {
                push_non_empty(&mut chunks, &non_empty_english_couplet(full));
                let meaning = if screen == AppScreen::Kids {
                    full.child
                        .as_ref()
                        .map(|c| c.simple_meaning_en.as_str())
                        .unwrap_or(&full.kural.explanation_en)
                } else {
                    full.kural.explanation_en.as_str()
                };
                push_non_empty(&mut chunks, meaning);
            }
        },
        SpeakTarget::KuttyStory => {
            if let Some(child) = &full.child {
                match lang {
                    Language::Tamil => {
                        push_non_empty(&mut chunks, &child.story_title_ta);
                        push_non_empty(&mut chunks, &child.kutty_story_ta);
                        push_non_empty(&mut chunks, &child.moral_ta);
                    }
                    Language::English => {
                        push_non_empty(&mut chunks, &child.story_title_en);
                        push_non_empty(&mut chunks, &child.kutty_story_en);
                        push_non_empty(&mut chunks, &child.moral_en);
                    }
                }
            } else {
                return build_speech_payload(
                    full,
                    screen,
                    SpeakTarget::KuralAndMeaning,
                    lang,
                    user_speed,
                    None,
                );
            }
        }
        SpeakTarget::YouthBriefing => match lang {
            Language::Tamil => {
                push_non_empty(&mut chunks, &full.kural.line1_ta);
                push_non_empty(&mut chunks, &full.kural.line2_ta);
                if let Some(youth) = &full.youth {
                    push_non_empty(&mut chunks, &youth.analysis_ta);
                    push_non_empty(&mut chunks, &youth.action_tip_ta);
                } else {
                    push_non_empty(&mut chunks, &full.kural.meaning_solomon);
                }
            }
            Language::English => {
                push_non_empty(&mut chunks, &non_empty_english_couplet(full));
                if let Some(youth) = &full.youth {
                    push_non_empty(&mut chunks, &youth.analysis_en);
                    push_non_empty(&mut chunks, &youth.action_tip_en);
                } else {
                    push_non_empty(&mut chunks, &full.kural.explanation_en);
                }
            }
        },
        SpeakTarget::ScholarCommentary(eff) => match lang {
            Language::Tamil => {
                let text = match eff {
                    Scholar::MuVa | Scholar::English => full.kural.meaning_mu_va.as_str(),
                    Scholar::Solomon => full.kural.meaning_solomon.as_str(),
                    Scholar::Kalaignar => full.kural.meaning_kalaignar.as_str(),
                    Scholar::Parimel => full
                        .kural
                        .meaning_parimel
                        .as_deref()
                        .unwrap_or(&full.kural.meaning_mu_va),
                };
                push_non_empty(&mut chunks, text);
            }
            Language::English => {
                push_non_empty(&mut chunks, &full.kural.translation_en);
                push_non_empty(&mut chunks, &full.kural.explanation_en);
            }
        },
        SpeakTarget::AiAnswer => {
            if let Some(ans) = last_ai_answer {
                let text = match lang {
                    Language::Tamil => &ans.answer_ta,
                    Language::English => &ans.answer_en,
                };
                push_non_empty(&mut chunks, text);
            }
        }
    }

    if chunks.is_empty() {
        return None;
    }

    let cache_key = if matches!(resolved_target, SpeakTarget::AiAnswer) {
        None
    } else {
        Some(format!(
            "k{}:{}:{}:s{}:{}",
            full.kural.number,
            lang.as_str(),
            screen.as_str(),
            speed_pct,
            target_cache_slug(resolved_target),
        ))
    };

    Some(SpeechPayload { chunks, cache_key })
}

fn target_cache_slug(target: SpeakTarget) -> String {
    match target {
        SpeakTarget::KuralOnly => "kural".to_string(),
        SpeakTarget::KuralAndMeaning => "kural_meaning".to_string(),
        SpeakTarget::KuttyStory => "story".to_string(),
        SpeakTarget::YouthBriefing => "youth".to_string(),
        SpeakTarget::ScholarCommentary(s) => format!("scholar_{}", s.as_str()),
        SpeakTarget::AiAnswer => "ai".to_string(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use kural_core::{Adhikaram, Division, Iyal, KuralChild, KuralCore, KuralYouth};

    fn sample_full_kural(parimel: Option<&str>) -> FullKural {
        FullKural {
            kural: KuralCore {
                number: 395,
                division_id: 2,
                iyal_id: 4,
                adhikaram_id: 40,
                line1_ta: "உடையார்முன் இல்லார்போல் ஏக்கற்றுங் கற்றார்".to_string(),
                line2_ta: "கடையரே கல்லா தவர்.".to_string(),
                opening_letter_ta: "உ".to_string(),
                transliteration1: "Udaiyaarmun Illaarpol".to_string(),
                transliteration2: "Kadaiyarae Kallaathavar.".to_string(),
                meaning_mu_va: "மு.வ. உரை மாதிரி".to_string(),
                meaning_solomon: "சாலமன் உரை மாதிரி".to_string(),
                meaning_kalaignar: "கலைஞர் உரை மாதிரி".to_string(),
                meaning_parimel: parimel.map(ToString::to_string),
                translation_en: "Sample English translation.".to_string(),
                explanation_en: "Sample English explanation.".to_string(),
                couplet_en: "Sample English couplet.".to_string(),
                tts_kural_ta: "உடையார்முன், கடையரே.".to_string(),
                tts_kural_en: "Stand eager to learn.".to_string(),
            },
            division: Division {
                id: 2,
                name_ta: "பொருட்பால்".to_string(),
                alt_name_ta: None,
                name_en: "Wealth".to_string(),
                transliteration: "Porutpal".to_string(),
                kural_start: 381,
                kural_end: 1080,
            },
            iyal: Iyal {
                id: 4,
                division_id: 2,
                name_ta: "அரசியல்".to_string(),
                alt_name_ta: None,
                name_en: "Royalty".to_string(),
                transliteration: "Arasiyal".to_string(),
                chapter_start: 39,
                chapter_end: 63,
            },
            adhikaram: Adhikaram {
                id: 40,
                division_id: 2,
                iyal_id: 4,
                name_ta: "கல்வி".to_string(),
                name_en: "Learning".to_string(),
                transliteration: "Kalvi".to_string(),
                kural_start: 391,
                kural_end: 400,
            },
            child: Some(KuralChild {
                kural_number: 395,
                kids_featured: true,
                simple_meaning_ta: "எளிய தமிழ் பொருள்".to_string(),
                simple_meaning_en: "Simple kids meaning.".to_string(),
                story_title_ta: "கதை தலைப்பு".to_string(),
                story_title_en: "Story Title".to_string(),
                kutty_story_ta: "குட்டி கதை".to_string(),
                kutty_story_en: "Short story body.".to_string(),
                moral_ta: "நல்ல நீதி".to_string(),
                moral_en: "Good moral.".to_string(),
                game_blank_word_ta: "கற்றார்".to_string(),
                game_options_ta: ["கற்றார்".into(), "ஆ".into(), "இ".into(), "ஈ".into()],
                game_options_en: ["learned".into(), "a".into(), "b".into(), "c".into()],
            }),
            youth: Some(KuralYouth {
                kural_number: 395,
                analysis_ta: "இளைஞர் பகுப்பாய்வு".to_string(),
                analysis_en: "Youth analysis.".to_string(),
                action_tip_ta: "செயல் குறிப்பு".to_string(),
                action_tip_en: "Action tip.".to_string(),
                quiz_question_ta: "வினா?".to_string(),
                quiz_question_en: "Question?".to_string(),
                quiz_options_ta: ["அ".into(), "ஆ".into(), "இ".into(), "ஈ".into()],
                quiz_options_en: ["A".into(), "B".into(), "C".into(), "D".into()],
                quiz_correct_idx: 0,
            }),
            adult: None,
            tags: Vec::new(),
        }
    }

    #[test]
    fn english_tts_never_emits_tamil_and_parimel_null_falls_back_to_mu_va() {
        let full = sample_full_kural(None);
        assert_eq!(
            resolve_effective_scholar(&full, Scholar::Parimel),
            Scholar::MuVa
        );

        for target in [
            SpeakTarget::KuralOnly,
            SpeakTarget::KuralAndMeaning,
            SpeakTarget::KuttyStory,
            SpeakTarget::YouthBriefing,
            SpeakTarget::ScholarCommentary(Scholar::Parimel),
        ] {
            let payload = build_speech_payload(
                &full,
                AppScreen::Adult,
                target,
                Language::English,
                1.0,
                None,
            )
            .expect("english payload");
            for chunk in &payload.chunks {
                assert!(
                    !chunk
                        .chars()
                        .any(|c| ('\u{0B80}'..='\u{0BFF}').contains(&c)),
                    "English TTS leaked Tamil script in {target:?}: {chunk}"
                );
            }
        }

        let ta_parimel = build_speech_payload(
            &full,
            AppScreen::Adult,
            SpeakTarget::ScholarCommentary(Scholar::Parimel),
            Language::Tamil,
            1.0,
            None,
        )
        .expect("tamil fallback payload");
        assert_eq!(ta_parimel.chunks, vec!["மு.வ. உரை மாதிரி".to_string()]);
    }
}
