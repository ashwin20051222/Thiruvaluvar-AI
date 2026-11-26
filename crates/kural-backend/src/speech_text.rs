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

/// Spoken payload chunks for a [`SpeakTarget`].
pub struct SpeechPayload {
    pub chunks: Vec<String>,
}

fn normalize_english_tts_text(text: &str) -> String {
    text.trim().replace("'''", "'").replace("''", "'")
}

fn non_empty_english_couplet(full: &FullKural) -> String {
    for candidate in [
        full.kural.tts_kural_en.as_str(),
        full.kural.translation_en.as_str(),
        full.kural.couplet_en.as_str(),
    ] {
        let norm = normalize_english_tts_text(candidate);
        if !norm.is_empty() {
            return norm;
        }
    }
    String::new()
}

fn non_empty_english_meaning(full: &FullKural) -> String {
    for candidate in [
        full.kural.explanation_en.as_str(),
        full.kural.translation_en.as_str(),
    ] {
        let norm = normalize_english_tts_text(candidate);
        if !norm.is_empty() {
            return norm;
        }
    }
    String::new()
}

fn non_empty_or<'a>(primary: &'a str, fallback: &'a str) -> &'a str {
    let trimmed = primary.trim();
    if trimmed.is_empty() {
        fallback
    } else {
        trimmed
    }
}

fn tamil_scholar_meaning(full: &FullKural, scholar: Scholar) -> &str {
    match resolve_effective_scholar(full, scholar) {
        Scholar::MuVa | Scholar::English => full.kural.meaning_mu_va.as_str(),
        Scholar::Solomon => non_empty_or(&full.kural.meaning_solomon, &full.kural.meaning_mu_va),
        Scholar::Kalaignar => {
            non_empty_or(&full.kural.meaning_kalaignar, &full.kural.meaning_mu_va)
        }
        Scholar::Parimel => full
            .kural
            .meaning_parimel
            .as_deref()
            .map(str::trim)
            .filter(|s| !s.is_empty())
            .unwrap_or(&full.kural.meaning_mu_va),
    }
}

fn push_non_empty(dst: &mut Vec<String>, text: &str) {
    let t = text.trim();
    if !t.is_empty() {
        dst.push(t.to_string());
    }
}

fn push_kural_and_meaning(
    chunks: &mut Vec<String>,
    full: &FullKural,
    screen: AppScreen,
    lang: Language,
    effective_scholar: Scholar,
) {
    match lang {
        Language::Tamil => {
            push_non_empty(chunks, &full.kural.line1_ta);
            push_non_empty(chunks, &full.kural.line2_ta);
            let meaning = match screen {
                AppScreen::Kids => full
                    .child
                    .as_ref()
                    .map(|c| c.simple_meaning_ta.as_str())
                    .filter(|s| !s.trim().is_empty())
                    .unwrap_or_else(|| tamil_scholar_meaning(full, effective_scholar)),
                AppScreen::Youth => non_empty_or(
                    &full.kural.meaning_mu_va,
                    non_empty_or(&full.kural.meaning_solomon, ""),
                ),
                AppScreen::Adult | AppScreen::Start => {
                    tamil_scholar_meaning(full, effective_scholar)
                }
            };
            push_non_empty(chunks, meaning);
        }
        Language::English => {
            push_non_empty(chunks, &non_empty_english_couplet(full));
            let meaning = match screen {
                AppScreen::Kids => full
                    .child
                    .as_ref()
                    .map(|c| normalize_english_tts_text(&c.simple_meaning_en))
                    .filter(|s| !s.is_empty())
                    .unwrap_or_else(|| non_empty_english_meaning(full)),
                AppScreen::Youth | AppScreen::Adult | AppScreen::Start => {
                    non_empty_english_meaning(full)
                }
            };
            push_non_empty(chunks, &meaning);
        }
    }
}

/// Map `(full, screen, target, lang, effective_scholar, user_speed, last_ai_answer)` into TTS
/// chunks following Section 4.5 of `backend_master_approved.md`.
///
/// Crucially, when `Language::English` is active, only English text is emitted so `en_US-lessac`
/// never attempts to speak raw Tamil script.
#[must_use]
pub fn build_speech_payload(
    full: &FullKural,
    screen: AppScreen,
    target: SpeakTarget,
    lang: Language,
    effective_scholar: Scholar,
    user_speed: f32,
    last_ai_answer: Option<&GroundedAnswer>,
) -> Option<SpeechPayload> {
    let mut chunks = Vec::new();

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
        SpeakTarget::KuralAndMeaning => {
            push_kural_and_meaning(&mut chunks, full, screen, lang, effective_scholar);
        }
        SpeakTarget::KuttyStory => {
            if let Some(child) = &full.child {
                match lang {
                    Language::Tamil => {
                        push_non_empty(&mut chunks, &child.story_title_ta);
                        push_non_empty(&mut chunks, &child.kutty_story_ta);
                        push_non_empty(&mut chunks, &child.moral_ta);
                    }
                    Language::English => {
                        push_non_empty(
                            &mut chunks,
                            &normalize_english_tts_text(&child.story_title_en),
                        );
                        push_non_empty(
                            &mut chunks,
                            &normalize_english_tts_text(&child.kutty_story_en),
                        );
                        push_non_empty(&mut chunks, &normalize_english_tts_text(&child.moral_en));
                    }
                }
            } else {
                return build_speech_payload(
                    full,
                    screen,
                    SpeakTarget::KuralAndMeaning,
                    lang,
                    effective_scholar,
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
                    push_non_empty(&mut chunks, &normalize_english_tts_text(&youth.analysis_en));
                    push_non_empty(
                        &mut chunks,
                        &normalize_english_tts_text(&youth.action_tip_en),
                    );
                } else {
                    push_non_empty(&mut chunks, &non_empty_english_meaning(full));
                }
            }
        },
        SpeakTarget::ScholarCommentary(eff) => {
            if screen == AppScreen::Adult {
                push_kural_and_meaning(&mut chunks, full, screen, lang, eff);
            } else {
                match lang {
                    Language::Tamil => {
                        push_non_empty(&mut chunks, tamil_scholar_meaning(full, eff));
                    }
                    Language::English => {
                        push_non_empty(&mut chunks, &non_empty_english_couplet(full));
                        push_non_empty(&mut chunks, &non_empty_english_meaning(full));
                    }
                }
            }
        }
        SpeakTarget::AiAnswer => {
            if let Some(ans) = last_ai_answer {
                let text = match lang {
                    Language::Tamil => ans.answer_ta.trim().to_string(),
                    Language::English => normalize_english_tts_text(&ans.answer_en),
                };
                push_non_empty(&mut chunks, &text);
            }
        }
    }

    if chunks.is_empty() {
        return None;
    }

    let _ = snap_speech_speed(user_speed);
    Some(SpeechPayload { chunks })
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
                Scholar::MuVa,
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
            Scholar::MuVa,
            1.0,
            None,
        )
        .expect("tamil fallback payload");
        assert_eq!(
            ta_parimel.chunks,
            vec![
                full.kural.line1_ta.clone(),
                full.kural.line2_ta.clone(),
                "மு.வ. உரை மாதிரி".to_string(),
            ]
        );
    }

    #[test]
    fn adult_kural_and_meaning_and_scholar_commentary_speak_kural_then_meaning() {
        let mut full = sample_full_kural(Some("பரிமேல் உரை மாதிரி"));
        full.kural.tts_kural_en = "'''Stand''' eager to learn.".to_string();

        // Tamil Adult: KuralAndMeaning respects effective_scholar (line1, line2, scholar meaning).
        for (scholar, expected_meaning) in [
            (Scholar::MuVa, "மு.வ. உரை மாதிரி"),
            (Scholar::Solomon, "சாலமன் உரை மாதிரி"),
            (Scholar::Kalaignar, "கலைஞர் உரை மாதிரி"),
            (Scholar::Parimel, "பரிமேல் உரை மாதிரி"),
        ] {
            let p_km = build_speech_payload(
                &full,
                AppScreen::Adult,
                SpeakTarget::KuralAndMeaning,
                Language::Tamil,
                scholar,
                1.0,
                None,
            )
            .expect("tamil kural and meaning");
            assert_eq!(
                p_km.chunks,
                vec![
                    full.kural.line1_ta.clone(),
                    full.kural.line2_ta.clone(),
                    expected_meaning.to_string(),
                ]
            );

            let p_sc = build_speech_payload(
                &full,
                AppScreen::Adult,
                SpeakTarget::ScholarCommentary(scholar),
                Language::Tamil,
                Scholar::MuVa,
                1.0,
                None,
            )
            .expect("tamil scholar commentary");
            assert_eq!(p_sc.chunks, p_km.chunks);
        }

        // Parimel == None falls back to MuVa for both KuralAndMeaning and ScholarCommentary.
        let full_no_parimel = sample_full_kural(None);
        let p_km_fb = build_speech_payload(
            &full_no_parimel,
            AppScreen::Adult,
            SpeakTarget::KuralAndMeaning,
            Language::Tamil,
            Scholar::Parimel,
            1.0,
            None,
        )
        .expect("tamil kural and meaning parimel fallback");
        assert_eq!(
            p_km_fb.chunks,
            vec![
                full_no_parimel.kural.line1_ta.clone(),
                full_no_parimel.kural.line2_ta.clone(),
                "மு.வ. உரை மாதிரி".to_string(),
            ]
        );

        // English Adult: Kural first (with ''' normalized to '), Meaning second, zero Tamil chars.
        for target in [
            SpeakTarget::KuralAndMeaning,
            SpeakTarget::ScholarCommentary(Scholar::Solomon),
        ] {
            let en_payload = build_speech_payload(
                &full,
                AppScreen::Adult,
                target,
                Language::English,
                Scholar::Solomon,
                1.0,
                None,
            )
            .expect("english adult payload");
            assert_eq!(
                en_payload.chunks,
                vec![
                    "'Stand' eager to learn.".to_string(),
                    "Sample English explanation.".to_string(),
                ]
            );
            assert!(
                en_payload
                    .chunks
                    .iter()
                    .all(|c| !c.chars().any(|ch| ('\u{0B80}'..='\u{0BFF}').contains(&ch)))
            );
        }

        // English fallback to translation_en when explanation_en is empty.
        let mut full_no_exp = sample_full_kural(None);
        full_no_exp.kural.explanation_en.clear();
        let en_fb = build_speech_payload(
            &full_no_exp,
            AppScreen::Adult,
            SpeakTarget::KuralAndMeaning,
            Language::English,
            Scholar::MuVa,
            1.0,
            None,
        )
        .expect("english fallback to translation_en");
        assert_eq!(
            en_fb.chunks,
            vec![
                "Stand eager to learn.".to_string(),
                "Sample English translation.".to_string(),
            ]
        );
    }

    #[test]
    fn kids_and_youth_kural_and_meaning_produce_age_appropriate_bilingual_chunks() {
        let full = sample_full_kural(Some("பரிமேல் உரை மாதிரி"));

        // Kids Tamil -> line1, line2, simple_meaning_ta
        let kids_ta = build_speech_payload(
            &full,
            AppScreen::Kids,
            SpeakTarget::KuralAndMeaning,
            Language::Tamil,
            Scholar::MuVa,
            1.0,
            None,
        )
        .expect("kids tamil");
        assert_eq!(
            kids_ta.chunks,
            vec![
                full.kural.line1_ta.clone(),
                full.kural.line2_ta.clone(),
                "எளிய தமிழ் பொருள்".to_string(),
            ]
        );

        // Kids English -> english couplet, simple_meaning_en
        let kids_en = build_speech_payload(
            &full,
            AppScreen::Kids,
            SpeakTarget::KuralAndMeaning,
            Language::English,
            Scholar::MuVa,
            1.0,
            None,
        )
        .expect("kids english");
        assert_eq!(
            kids_en.chunks,
            vec![
                "Stand eager to learn.".to_string(),
                "Simple kids meaning.".to_string(),
            ]
        );

        // Youth Tamil -> line1, line2, meaning_mu_va
        let youth_ta = build_speech_payload(
            &full,
            AppScreen::Youth,
            SpeakTarget::KuralAndMeaning,
            Language::Tamil,
            Scholar::MuVa,
            1.0,
            None,
        )
        .expect("youth tamil");
        assert_eq!(
            youth_ta.chunks,
            vec![
                full.kural.line1_ta.clone(),
                full.kural.line2_ta.clone(),
                "மு.வ. உரை மாதிரி".to_string(),
            ]
        );

        // Youth English -> english couplet, explanation_en (zero Tamil script)
        let youth_en = build_speech_payload(
            &full,
            AppScreen::Youth,
            SpeakTarget::KuralAndMeaning,
            Language::English,
            Scholar::MuVa,
            1.0,
            None,
        )
        .expect("youth english");
        assert_eq!(
            youth_en.chunks,
            vec![
                "Stand eager to learn.".to_string(),
                "Sample English explanation.".to_string(),
            ]
        );
        assert!(
            youth_en
                .chunks
                .iter()
                .all(|c| !c.chars().any(|ch| ('\u{0B80}'..='\u{0BFF}').contains(&ch)))
        );

        // Fallbacks when child/youth metadata or mu_va are absent.
        let mut bare = sample_full_kural(None);
        bare.child = None;
        bare.youth = None;
        let kids_fb = build_speech_payload(
            &bare,
            AppScreen::Kids,
            SpeakTarget::KuralAndMeaning,
            Language::Tamil,
            Scholar::MuVa,
            1.0,
            None,
        )
        .unwrap();
        assert_eq!(kids_fb.chunks[2], "மு.வ. உரை மாதிரி");

        bare.kural.meaning_mu_va.clear();
        let youth_fb = build_speech_payload(
            &bare,
            AppScreen::Youth,
            SpeakTarget::KuralAndMeaning,
            Language::Tamil,
            Scholar::MuVa,
            1.0,
            None,
        )
        .unwrap();
        assert_eq!(youth_fb.chunks[2], "சாலமன் உரை மாதிரி");
    }
}
