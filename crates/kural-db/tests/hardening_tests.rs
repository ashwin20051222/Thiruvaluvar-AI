use kural_core::{
    AgeGroup, EnrichedAdultPayload, EnrichedChapterFile, EnrichedChildPayload, EnrichedKuralRecord,
    EnrichedYouthPayload, Language, ResearchTheme, Scholar, SearchMatchKind, Theme,
    load_and_audit_canonical, tokenize_kural_words,
};
use kural_db::{
    KuralDb, USER_STATE_SCHEMA_VERSION, UserStateStore, compile_database, load_knowledge_base,
};
use rusqlite::Connection;
use std::collections::{BTreeMap, BTreeSet};
use std::path::PathBuf;
use tempfile::TempDir;

fn workspace_root() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .and_then(|p| p.parent())
        .expect("workspace root")
        .to_path_buf()
}

fn build_synthetic_full_enrichment(
    canonical: &kural_core::CanonicalDataset,
) -> BTreeMap<u8, EnrichedChapterFile> {
    let mut map = BTreeMap::new();
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
                    .expect("canonical kural has at least one Tamil token");
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
                        kutty_story_ta: format!(
                            "குறள் {} சொல்லும் நற்பண்பை நிலா மகிழ்ச்சியுடன் கடைப்பிடித்தாள்.",
                            k.number
                        ),
                        kutty_story_en: format!(
                            "Little Nila practiced Kural {} with kindness and joy.",
                            k.number
                        ),
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
                        analysis_ta: format!("இளையோர் சிந்தனை {}: அறிவுடைமை மற்றும் ஒழுக்கம்.", k.number),
                        analysis_en: format!(
                            "Youth reflection {}: apply wisdom in daily study.",
                            k.number
                        ),
                        action_tip_ta: "இன்று ஒரு நல்ல செயலைச் செய்.".to_string(),
                        action_tip_en: "Practice one mindful habit today.".to_string(),
                        quiz_question_ta: format!("குறள் {} எதை வலியுறுத்துகிறது?", k.number),
                        quiz_question_en: format!("What does Kural {} emphasize?", k.number),
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
                        reflection_ta: format!("குறள் {} தத்துவச் சிந்தனை.", k.number),
                        reflection_en: format!("Philosophical reflection on Kural {}.", k.number),
                        research_theme: theme,
                    },
                    tags,
                }
            })
            .collect();

        map.insert(
            ad.id,
            EnrichedChapterFile {
                adhikaram_id: ad.id,
                provider: "hardening-test".to_string(),
                model: "synthetic-v1".to_string(),
                generated_at: "2026-11-26T00:00:00Z".to_string(),
                kurals: records,
            },
        );
    }
    map
}

fn compile_temp_synthetic_db(dir: &TempDir) -> PathBuf {
    let root = workspace_root();
    let canonical =
        load_and_audit_canonical(&root.join("seed_data/canonical")).expect("canonical audit");
    let knowledge =
        load_knowledge_base(&root.join("seed_data/knowledge_base.json")).expect("knowledge base");
    let enriched = build_synthetic_full_enrichment(&canonical);
    let db_path = dir.path().join("thirukkural.db");
    compile_database(&canonical, &enriched, &knowledge, &db_path, true).expect("compile db");
    db_path
}

#[test]
fn fts5_preserves_tamil_diacritics_without_stripping() {
    let dir = TempDir::new().expect("tempdir");
    let db_path = compile_temp_synthetic_db(&dir);
    let db = KuralDb::open(&db_path).expect("open compiled db");

    // Exact Tamil words containing virama (்) and vowel signs (ா, ி, ு, ை) must match cleanly.
    for (query, expected_kural) in [
        ("எழுத்தெல்லாம்", 1u16), // ு, ், ா
        ("முதற்றே", 1u16),    // ு, ்
        ("வாலறிவன்", 2u16),    // ா, ி
        ("மலர்மிசை", 3u16),   // ், ி, ை
    ] {
        let outcome = db
            .search_hybrid(query, None, 10)
            .expect("search_hybrid with Tamil diacritics");
        assert!(
            outcome
                .hits
                .iter()
                .any(|h| h.kural.number == expected_kural
                    && h.match_kind == SearchMatchKind::FullText),
            "expected FullText match for '{query}' hitting Kural #{expected_kural}, got: {:?}",
            outcome.hits
        );
    }

    // Stripped-diacritic equivalents must NOT match via FTS5 (proves `remove_diacritics 0` is active).
    for stripped in ["எழததெலலாம", "மதறறே", "வலறவன", "மலரமச"]
    {
        let rrf = db
            .search_hybrid_rrf(stripped, None, 0.25, 60.0, 10)
            .expect("search_hybrid_rrf stripped");
        assert!(
            !rrf.has_exact_or_fts_match,
            "stripped form '{stripped}' should not match FTS5 when remove_diacritics 0 is enforced"
        );
    }
}

#[test]
fn legacy_user_state_db_full_migration_and_idempotent_reopen_preserves_all_data() {
    let dir = TempDir::new().expect("tempdir");
    let corpus_path = compile_temp_synthetic_db(&dir);
    let corpus = KuralDb::open(&corpus_path).expect("open corpus");

    let state_path = dir.path().join("legacy_user_state.db");
    {
        let conn = Connection::open(&state_path).expect("create legacy user_state.db");
        conn.execute_batch(
            "CREATE TABLE preferences (
                id               INTEGER PRIMARY KEY CHECK (id = 1),
                lang             TEXT NOT NULL DEFAULT 'tamil',
                theme            TEXT NOT NULL DEFAULT 'dark',
                speech_speed_pct INTEGER NOT NULL DEFAULT 100,
                volume           REAL NOT NULL DEFAULT 0.85,
                muted            INTEGER NOT NULL DEFAULT 0,
                default_age      TEXT NOT NULL DEFAULT 'youth',
                last_kural_kids  INTEGER NOT NULL DEFAULT 1,
                last_kural_youth INTEGER NOT NULL DEFAULT 481,
                last_kural_adult INTEGER NOT NULL DEFAULT 423
            );
            CREATE TABLE kids_progress (
                id           INTEGER PRIMARY KEY CHECK (id = 1),
                stars_earned INTEGER NOT NULL DEFAULT 0
            );
            CREATE TABLE kids_completed_stories (
                kural_number INTEGER PRIMARY KEY
            );
            CREATE TABLE kids_completed_games (
                kural_number INTEGER PRIMARY KEY
            );
            CREATE TABLE youth_progress (
                id               INTEGER PRIMARY KEY CHECK (id = 1),
                daily_streak     INTEGER NOT NULL DEFAULT 0,
                last_active_date TEXT NOT NULL DEFAULT '',
                quiz_xp          INTEGER NOT NULL DEFAULT 0
            );
            CREATE TABLE youth_answered_quizzes (
                kural_number  INTEGER PRIMARY KEY,
                first_correct INTEGER NOT NULL
            );
            CREATE TABLE adult_bookmarks (
                kural_number      INTEGER PRIMARY KEY,
                preferred_scholar TEXT NOT NULL,
                created_at        TEXT NOT NULL
            );

            INSERT INTO preferences (
                id, lang, theme, speech_speed_pct, volume, muted, default_age,
                last_kural_kids, last_kural_youth, last_kural_adult
            ) VALUES (1, 'english', 'light', 125, 0.72, 1, 'adult', 12, 481, 423);

            INSERT INTO kids_progress (id, stars_earned) VALUES (1, 4);
            INSERT INTO kids_completed_stories (kural_number) VALUES (1), (2);
            INSERT INTO kids_completed_games (kural_number) VALUES (1);

            INSERT INTO youth_progress (id, daily_streak, last_active_date, quiz_xp)
            VALUES (1, 5, '2026-11-25', 80);
            INSERT INTO youth_answered_quizzes (kural_number, first_correct)
            VALUES (481, 1);

            INSERT INTO adult_bookmarks (kural_number, preferred_scholar, created_at)
            VALUES (423, 'mu_va', '2026-11-25T09:00:00Z');",
        )
        .expect("seed legacy pre-v1 user_state.db");
    }

    // First open runs v0 -> v1 migration.
    let store1 = UserStateStore::open(&state_path, 0.85).expect("migrate legacy user_state.db");
    assert_eq!(
        store1.schema_version().expect("schema_version 1"),
        USER_STATE_SCHEMA_VERSION
    );
    drop(store1);

    // Second open verifies idempotency and zero data loss across all tables.
    let store2 = UserStateStore::open(&state_path, 0.85).expect("reopen migrated user_state.db");
    assert_eq!(
        store2.schema_version().expect("schema_version 2"),
        USER_STATE_SCHEMA_VERSION
    );

    let (prefs, kids, youth, bookmarks) = store2
        .load_snapshot_state(&corpus)
        .expect("load migrated snapshot state");

    assert_eq!(prefs.lang, Language::English);
    assert_eq!(prefs.theme, Theme::Light);
    assert_eq!(prefs.speech_speed, 1.25);
    assert!((prefs.volume - 0.72).abs() < 1e-5);
    assert!(prefs.muted);
    assert_eq!(prefs.default_age, AgeGroup::Adult);
    assert_eq!(prefs.last_kural_kids, 12);
    assert_eq!(prefs.last_kural_youth, 481);
    assert_eq!(prefs.last_kural_adult, 423);
    assert!(prefs.auto_speak_text);

    assert_eq!(kids.stars_earned, 4);
    assert_eq!(kids.completed_stories, BTreeSet::from([1u16, 2u16]));
    assert_eq!(kids.completed_games, BTreeSet::from([1u16]));

    assert_eq!(youth.daily_streak, 5);
    assert_eq!(youth.last_active_date, "2026-11-25");
    assert_eq!(youth.quiz_xp, 80);
    assert_eq!(youth.answered_quizzes, BTreeSet::from([481u16]));

    assert_eq!(bookmarks.len(), 1);
    assert_eq!(bookmarks[0].kural_number, 423);
    assert_eq!(bookmarks[0].preferred_scholar, Scholar::MuVa);
    assert_eq!(bookmarks[0].created_at, "2026-11-25T09:00:00Z");
    assert!(!bookmarks[0].line1_ta.is_empty());

    // Confirm legacy answered quiz #481 retained first_correct=1 (repeat correct awards 0 extra XP).
    let after_repeat = store2
        .record_youth_quiz_attempt(481, true, "2026-11-25", "2026-11-24")
        .expect("repeat quiz attempt after migration");
    assert_eq!(after_repeat.quiz_xp, 80);
    assert_eq!(after_repeat.daily_streak, 5);
    assert_eq!(after_repeat.answered_quizzes, BTreeSet::from([481u16]));
}
