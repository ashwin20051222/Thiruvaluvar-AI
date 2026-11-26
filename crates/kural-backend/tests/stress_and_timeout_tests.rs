use kural_backend::BackendHandle;
use kural_core::{
    AgeGroup, AppConfig, BackendCommand, EnrichedAdultPayload, EnrichedChapterFile,
    EnrichedChildPayload, EnrichedKuralRecord, EnrichedYouthPayload, Language, ResearchTheme,
    SpeakTarget, load_and_audit_canonical, tokenize_kural_words,
};
use kural_db::{compile_database, load_knowledge_base};
use std::collections::BTreeMap;
use std::path::PathBuf;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use tempfile::TempDir;

fn workspace_root() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .and_then(|p| p.parent())
        .expect("workspace root")
        .to_path_buf()
}

fn build_synthetic_db(dir: &TempDir) -> PathBuf {
    let root = workspace_root();
    let canonical =
        load_and_audit_canonical(&root.join("seed_data/canonical")).expect("canonical audit");
    let knowledge =
        load_knowledge_base(&root.join("seed_data/knowledge_base.json")).expect("knowledge base");

    let mut enriched = BTreeMap::new();
    for ad in &canonical.adhikarams {
        let records = canonical
            .kurals
            .iter()
            .filter(|k| k.adhikaram_id == ad.id)
            .map(|k| {
                let words = tokenize_kural_words(&k.line1_ta, &k.line2_ta);
                let blank_word = words
                    .first()
                    .cloned()
                    .expect("canonical kural has at least one token");
                let distractors = ["மலர்", "கடல்", "மழை", "நிலம்", "வான்"];
                let mut opts_ta = vec![blank_word.clone()];
                for d in distractors {
                    if opts_ta.len() == 4 {
                        break;
                    }
                    if !opts_ta.iter().any(|existing| existing == d) {
                        opts_ta.push(d.to_string());
                    }
                }
                let game_options_ta: [String; 4] = opts_ta.try_into().expect("4 tamil options");
                let (theme, tags) = match k.division_id {
                    1 => (
                        ResearchTheme::Virtue,
                        vec!["ethics".to_string(), "wisdom".to_string()],
                    ),
                    2 => (
                        ResearchTheme::Polity,
                        vec!["leadership".to_string(), "friendship".to_string()],
                    ),
                    _ => (
                        ResearchTheme::LoveEthics,
                        vec!["love".to_string(), "harmony".to_string()],
                    ),
                };

                EnrichedKuralRecord {
                    kural_number: k.number,
                    child: EnrichedChildPayload {
                        simple_meaning_ta: format!("குறள் {} எளிய விளக்கம்.", k.number),
                        simple_meaning_en: format!("Simple meaning for Kural {}.", k.number),
                        story_title_ta: format!("கதை {}", k.number),
                        story_title_en: format!("Story {}", k.number),
                        kutty_story_ta: format!("குறள் {} கதை.", k.number),
                        kutty_story_en: format!("Story for Kural {}.", k.number),
                        moral_ta: "நல்ல பண்பே உயர்வு தரும்.".to_string(),
                        moral_en: "Good character elevates everyone.".to_string(),
                        game_blank_word_ta: blank_word,
                        game_options_ta,
                        game_options_en: [
                            "Key Tamil word".to_string(),
                            "Flower".to_string(),
                            "Ocean".to_string(),
                            "Rain".to_string(),
                        ],
                    },
                    youth: EnrichedYouthPayload {
                        analysis_ta: format!("இளையோர் சிந்தனை {}.", k.number),
                        analysis_en: format!("Youth reflection {}.", k.number),
                        action_tip_ta: "நல்ல செயல் செய்.".to_string(),
                        action_tip_en: "Practice one mindful habit.".to_string(),
                        quiz_question_ta: format!("குறள் {} கேள்வி?", k.number),
                        quiz_question_en: format!("Question for Kural {}?", k.number),
                        quiz_options_ta: [
                            "நற்பண்பு".to_string(),
                            "சோம்பல்".to_string(),
                            "கோபம்".to_string(),
                            "பொறாமை".to_string(),
                        ],
                        quiz_options_en: [
                            "Ethical wisdom".to_string(),
                            "Laziness".to_string(),
                            "Anger".to_string(),
                            "Envy".to_string(),
                        ],
                        quiz_correct_idx: 0,
                    },
                    adult: EnrichedAdultPayload {
                        reflection_ta: format!("குறள் {} சிந்தனை.", k.number),
                        reflection_en: format!("Reflection on Kural {}.", k.number),
                        research_theme: theme,
                    },
                    tags,
                }
            })
            .collect();

        enriched.insert(
            ad.id,
            EnrichedChapterFile {
                adhikaram_id: ad.id,
                provider: "stress-test".to_string(),
                model: "synthetic-v1".to_string(),
                generated_at: "2026-11-26T00:00:00Z".to_string(),
                kurals: records,
            },
        );
    }

    let db_path = dir.path().join("thirukkural_stress.db");
    compile_database(&canonical, &enriched, &knowledge, &db_path, true).expect("compile temp db");
    db_path
}

#[test]
fn concurrent_command_burst_preserves_invariants_and_shuts_down_cleanly() {
    let dir = TempDir::new().expect("tempdir");
    let db_path = build_synthetic_db(&dir);

    let mut cfg = AppConfig::default();
    cfg.paths.kural_db = db_path;
    cfg.paths.user_state_db = dir.path().join("user_state.db");
    cfg.paths.models_dir = dir.path().join("missing_models");

    let handle = BackendHandle::start(cfg).expect("BackendHandle::start");
    let mut events = handle.subscribe_events();

    let stop_reader = Arc::new(AtomicBool::new(false));
    let reader_flag = Arc::clone(&stop_reader);
    let reader_handle = handle.clone();
    let reader_thread = std::thread::spawn(move || {
        while !reader_flag.load(Ordering::Acquire) {
            let snap = reader_handle.current_snapshot();
            assert!((1..=1330).contains(&snap.current_kural.kural.number));
            assert!(snap.pool_len > 0);
            assert!(snap.pool_idx < snap.pool_len);
        }
    });

    let mut workers = Vec::new();
    for worker_id in 0..6u16 {
        let h = handle.clone();
        workers.push(std::thread::spawn(move || {
            let ages = [AgeGroup::Kids, AgeGroup::Youth, AgeGroup::Adult];
            let kurals = [1u16, 42, 395, 481, 1081];
            let langs = [Language::Tamil, Language::English];

            for step in 0..35u16 {
                let idx = usize::from(worker_id + step);
                match idx % 9 {
                    0 => h.dispatch(BackendCommand::SelectAge(ages[idx % ages.len()])),
                    1 => h.dispatch(BackendCommand::OpenKural(kurals[idx % kurals.len()])),
                    2 => h.dispatch(BackendCommand::NextKural),
                    3 => h.dispatch(BackendCommand::PrevKural),
                    4 => h.dispatch(BackendCommand::SubmitTextSearch {
                        query: if idx.is_multiple_of(2) {
                            "friendship".to_string()
                        } else {
                            "நட்பு".to_string()
                        },
                        division_id: None,
                    }),
                    5 => h.dispatch(BackendCommand::SpeakCurrent(SpeakTarget::KuralAndMeaning)),
                    6 => h.dispatch(BackendCommand::StopAudio),
                    7 => h.dispatch(BackendCommand::ToggleBookmark(423)),
                    _ => h.dispatch(BackendCommand::SetLanguage(langs[idx % langs.len()])),
                }
            }
        }));
    }

    for w in workers {
        w.join().expect("worker thread joined cleanly");
    }
    stop_reader.store(true, Ordering::Release);
    reader_thread.join().expect("reader thread joined cleanly");

    // Allow the controller loop a moment to process any queued commands, then inspect invariants.
    std::thread::sleep(std::time::Duration::from_millis(80));
    let final_snap = handle.current_snapshot();
    assert!((1..=1330).contains(&final_snap.current_kural.kural.number));
    assert!(final_snap.pool_len > 0);
    assert!(final_snap.pool_idx < final_snap.pool_len);

    while events.try_recv().is_ok() {}
    handle.shutdown();
}
