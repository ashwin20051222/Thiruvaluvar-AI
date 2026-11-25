use kural_core::{
    ActiveFilter, AgeGroup, CANONICAL_TAGS, EnrichedAdultPayload, EnrichedChapterFile,
    EnrichedChildPayload, EnrichedKuralRecord, EnrichedYouthPayload, ResearchTheme, Scholar,
    SearchMatchKind, contains_exact_kural_word, is_nfc, load_and_audit_canonical,
    opening_grapheme_ta, tokenize_kural_words,
};
use kural_db::{
    KuralDb, TextEmbedder, UserStateStore, compile_database, index_database_embeddings,
    load_knowledge_base,
};
use std::collections::BTreeMap;
use std::path::PathBuf;
use std::sync::Arc;
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
        let ch_kurals: Vec<&kural_core::KuralCore> = canonical
            .kurals
            .iter()
            .filter(|k| k.adhikaram_id == ad.id)
            .collect();

        let records = ch_kurals
            .iter()
            .map(|k| {
                let words = tokenize_kural_words(&k.line1_ta, &k.line2_ta);
                let blank_word = words
                    .first()
                    .cloned()
                    .expect("canonical kural must have at least 1 Tamil token");
                let distractor_pool = ["மலர்", "கடல்", "மழை", "நிலம்", "வான்", "காற்று"];
                let mut opts_ta = vec![blank_word.clone()];
                for d in distractor_pool {
                    if opts_ta.len() == 4 {
                        break;
                    }
                    if !opts_ta.iter().any(|existing| existing == d) {
                        opts_ta.push(d.to_string());
                    }
                }
                let game_options_ta: [String; 4] = opts_ta.try_into().expect("4 tamil options");
                let game_options_en = [
                    "Key Tamil word".to_string(),
                    "Flower".to_string(),
                    "Ocean".to_string(),
                    "Rain".to_string(),
                ];

                let (theme, tags) = match k.division_id {
                    1 => (
                        if k.adhikaram_id == 1 {
                            ResearchTheme::Philosophy
                        } else {
                            ResearchTheme::Virtue
                        },
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

                let (story_ta, story_en) = if k.number <= 1080 {
                    (
                        format!(
                            "குறள் {} சொல்லும் நல்லொழுக்கத்தைப் பள்ளிச் சிறுமி நிலா மகிழ்ச்சியுடன் கடைப்பிடித்தாள்.",
                            k.number
                        ),
                        format!(
                            "Little Nila practiced the virtue taught in Kural {} with kindness and joy.",
                            k.number
                        ),
                    )
                } else {
                    (
                        format!(
                            "குறள் {} குடும்ப அன்பும் நட்பும் நல்லிணக்கமும் வளர்க்கும் கதையாகப் பாட்டி கூறினார்.",
                            k.number
                        ),
                        format!(
                            "Grandma shared Kural {} as a gentle story of family affection, kindness, and harmony.",
                            k.number
                        ),
                    )
                };

                EnrichedKuralRecord {
                    kural_number: k.number,
                    child: EnrichedChildPayload {
                        simple_meaning_ta: format!("குறள் {} எளிய விளக்கம்.", k.number),
                        simple_meaning_en: format!("Simple meaning for Kural {}.", k.number),
                        story_title_ta: format!("கதை {}", k.number),
                        story_title_en: format!("Story {}", k.number),
                        kutty_story_ta: story_ta,
                        kutty_story_en: story_en,
                        moral_ta: "நல்ல பண்பே உயர்வு தரும்.".to_string(),
                        moral_en: "Good character elevates everyone.".to_string(),
                        game_blank_word_ta: blank_word,
                        game_options_ta,
                        game_options_en,
                    },
                    youth: EnrichedYouthPayload {
                        analysis_ta: format!("இளையோர் சிந்தனை {}: அறிவுடைமை மற்றும் ஒழுக்கம்.", k.number),
                        analysis_en: format!(
                            "Youth reflection {}: apply wisdom and friendship in daily study.",
                            k.number
                        ),
                        action_tip_ta: "இன்று ஒரு நல்ல செயலைச் செய்.".to_string(),
                        action_tip_en: "Practice one mindful habit today.".to_string(),
                        quiz_question_ta: format!("குறள் {} எதை வலியுறுத்துகிறது?", k.number),
                        quiz_question_en: format!("What principle does Kural {} emphasize?", k.number),
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
                provider: "test-fixture".to_string(),
                model: "deterministic-v1".to_string(),
                generated_at: "2026-11-24T00:00:00Z".to_string(),
                kurals: records,
            },
        );
    }
    map
}

fn assert_all_invariants(db: &KuralDb) {
    let stats = db.stats().expect("stats");
    assert_eq!(stats.divisions, 3);
    assert_eq!(stats.iyals, 9);
    assert_eq!(stats.adhikarams, 133);
    assert_eq!(stats.kurals, 1330);
    assert_eq!(stats.kural_child, 1330);
    assert_eq!(stats.kural_youth, 1330);
    assert_eq!(stats.kural_adult, 1330);
    assert_eq!(stats.tags, 20);
    assert!(stats.kural_tags >= 2660);
    assert!(stats.knowledge_base >= 40);
    assert_eq!(stats.integrity_check, "ok");
    assert!(
        stats.file_size_bytes < 35 * 1024 * 1024,
        "DB size {} exceeds 35 MB Raspberry Pi budget",
        stats.file_size_bytes
    );

    let divisions = db.list_divisions().expect("divisions");
    assert_eq!(divisions.len(), 3);
    assert_eq!(divisions[0].name_ta, "அறத்துப்பால்");
    assert_eq!(divisions[0].kural_start, 1);
    assert_eq!(divisions[0].kural_end, 380);
    assert_eq!(divisions[1].name_ta, "பொருட்பால்");
    assert_eq!(divisions[1].kural_start, 381);
    assert_eq!(divisions[1].kural_end, 1080);
    assert_eq!(divisions[2].name_ta, "இன்பத்துப்பால்");
    assert_eq!(divisions[2].kural_start, 1081);
    assert_eq!(divisions[2].kural_end, 1330);

    let iyals = db.list_iyals(None).expect("iyals");
    assert_eq!(iyals.len(), 9);
    let adhikarams = db.list_adhikarams(None).expect("adhikarams");
    assert_eq!(adhikarams.len(), 133);

    // Exact division splits & per-kural invariants
    let mut aram = 0usize;
    let mut porul = 0usize;
    let mut inbam = 0usize;
    let mut kids_featured_count = 0usize;

    for num in 1u16..=1330u16 {
        let full = db.get_kural_full(num).expect("full kural");
        match num {
            1..=380 => {
                assert_eq!(full.kural.division_id, 1);
                aram += 1;
            }
            381..=1080 => {
                assert_eq!(full.kural.division_id, 2);
                porul += 1;
            }
            1081..=1330 => {
                assert_eq!(full.kural.division_id, 3);
                inbam += 1;
            }
            _ => unreachable!(),
        }

        // NFC & intact opening grapheme cluster
        assert!(is_nfc(&full.kural.line1_ta));
        assert!(is_nfc(&full.kural.line2_ta));
        assert!(is_nfc(&full.kural.opening_letter_ta));
        assert_eq!(
            Some(full.kural.opening_letter_ta.clone()),
            opening_grapheme_ta(&full.kural.line1_ta)
        );
        assert!(is_nfc(&full.kural.meaning_mu_va));
        assert!(is_nfc(&full.kural.meaning_solomon));
        assert!(is_nfc(&full.kural.meaning_kalaignar));

        // Transliteration sanity check explicitly verified for Kural #1 vs #2
        if num == 1 {
            assert!(full.kural.transliteration1.starts_with("Akara"));
        } else if num == 2 {
            assert!(full.kural.transliteration1.starts_with("Katradhanaal"));
        }

        let child = full.child.expect("child row must exist");
        if num <= 1080 {
            assert!(child.kids_featured);
            kids_featured_count += 1;
        } else {
            assert!(!child.kids_featured);
        }
        assert!(is_nfc(&child.kutty_story_ta));
        assert!(is_nfc(&child.game_blank_word_ta));
        assert!(contains_exact_kural_word(
            &full.kural.line1_ta,
            &full.kural.line2_ta,
            &child.game_blank_word_ta
        ));
        assert!(
            child
                .game_options_ta
                .iter()
                .any(|o| o == &child.game_blank_word_ta),
            "kural #{num}: game_options_ta missing blank word"
        );

        let youth = full.youth.expect("youth row must exist");
        assert!(youth.quiz_correct_idx <= 3);
        assert!(is_nfc(&youth.analysis_ta));

        let adult = full.adult.expect("adult row must exist");
        assert!(is_nfc(&adult.reflection_ta));

        assert!((2..=4).contains(&full.tags.len()));
        for t in &full.tags {
            assert!(CANONICAL_TAGS.iter().any(|(slug, _, _)| *slug == t.slug));
        }
    }

    assert_eq!(aram, 380);
    assert_eq!(porul, 700);
    assert_eq!(inbam, 250);
    assert_eq!(kids_featured_count, 1080);

    // Daily Kural Kids Mode strictly draws from 1..=1080
    for day in 1..=40 {
        let k = db.get_daily_kural(AgeGroup::Kids, day).expect("daily kids");
        assert!(k.kural.number <= 1080);
        assert!(k.child.unwrap().kids_featured);
    }

    // Tag & research theme queries
    assert!(!db.get_kurals_by_adhikaram(1).expect("ch 1").is_empty());
    assert!(
        !db.get_kurals_by_tag("friendship", 5)
            .expect("tag")
            .is_empty()
    );
    assert!(
        !db.get_kurals_by_research_theme(ResearchTheme::Polity, 5)
            .expect("theme")
            .is_empty()
    );

    // Hybrid search verification
    let exact_num = db.search_hybrid("42", None, 10).expect("42");
    assert_eq!(exact_num.hits.len(), 1);
    assert_eq!(exact_num.hits[0].kural.number, 42);
    assert_eq!(exact_num.hits[0].match_kind, SearchMatchKind::ExactNumber);

    let tamil_num = db.search_hybrid("குறள் 481", None, 10).expect("குறள் 481");
    assert_eq!(tamil_num.hits.len(), 1);
    assert_eq!(tamil_num.hits[0].kural.number, 481);
    assert_eq!(tamil_num.hits[0].match_kind, SearchMatchKind::ExactNumber);

    let adhikaram = db
        .search_hybrid("கடவுள் வாழ்த்து", None, 10)
        .expect("கடவுள் வாழ்த்து");
    assert_eq!(adhikaram.hits.len(), 10);
    assert_eq!(adhikaram.hits[0].kural.number, 1);
    assert_eq!(
        adhikaram.hits[0].match_kind,
        SearchMatchKind::AdhikaramMatch
    );

    let en_kw = db
        .search_hybrid("friendship", None, 10)
        .expect("friendship");
    assert!(!en_kw.hits.is_empty());

    let ta_kw = db.search_hybrid("நட்பு", None, 10).expect("நட்பு");
    assert!(!ta_kw.hits.is_empty());

    let fuzzy_en = db
        .search_hybrid("thiruvalvar", None, 10)
        .expect("thiruvalvar");
    assert!(fuzzy_en.did_you_mean.is_some());
    assert!(!fuzzy_en.hits.is_empty());
    assert_eq!(fuzzy_en.hits[0].match_kind, SearchMatchKind::FuzzyFallback);

    let fuzzy_ta = db.search_hybrid("அறிவுடமை", None, 10).expect("அறிவுடமை");
    assert_eq!(fuzzy_ta.did_you_mean.as_deref(), Some("அறிவுடைமை"));
    assert!(!fuzzy_ta.hits.is_empty());

    let oos = db
        .search_hybrid("quantum blockchain spaceship xyz999", None, 10)
        .expect("oos");
    assert!(oos.hits.is_empty());
    assert!(oos.did_you_mean.is_none());

    let kb_hits = db.search_knowledge("Kanyakumari", 5).expect("kb");
    assert!(!kb_hits.is_empty());
}

#[test]
fn full_1330_pipeline_and_hybrid_search_verification() {
    let root = workspace_root();
    let canonical_dir = root.join("seed_data/canonical");
    let kb_path = root.join("seed_data/knowledge_base.json");

    if !canonical_dir.join("thirukkural.json").exists() || !kb_path.exists() {
        panic!("Canonical data missing. Run `cargo run --bin fetch_canonical` first.");
    }

    let canonical = load_and_audit_canonical(&canonical_dir).expect("canonical audit");
    let knowledge = load_knowledge_base(&kb_path).expect("knowledge_base");
    let enriched = build_synthetic_full_enrichment(&canonical);

    let tmp_dir = TempDir::new().expect("tempdir");
    let db_path = tmp_dir.path().join("thirukkural_test.db");

    let build_stats = compile_database(&canonical, &enriched, &knowledge, &db_path, true)
        .expect("strict compile_database");
    assert_eq!(build_stats.build_mode, "full (1330/1330 enriched)");

    let db = KuralDb::open(&db_path).expect("open read-only db");
    assert_all_invariants(&db);
}

#[test]
fn missing_database_returns_clear_hint_instead_of_sqlite_code_14() {
    let tmp_dir = TempDir::new().expect("tempdir");
    let missing = tmp_dir.path().join("nope/thirukkural.db");

    let err = kural_db::resolve_db_path(Some(&missing)).unwrap_err();
    let msg = err.to_string();
    assert!(matches!(err, kural_db::DbError::DatabaseNotFound { .. }));
    assert!(msg.contains(&missing.display().to_string()));
    assert!(msg.contains("cargo run --bin seed_db -- --partial-ok"));

    let open_err = KuralDb::open(&missing).unwrap_err();
    assert!(matches!(
        open_err,
        kural_db::DbError::DatabaseNotFound { .. }
    ));
    assert!(
        !missing.exists(),
        "open must not create an empty database file"
    );
}

#[test]
fn compile_database_creates_missing_parent_directories() {
    let root = workspace_root();
    let canonical = load_and_audit_canonical(&root.join("seed_data/canonical")).expect("canonical");
    let knowledge = load_knowledge_base(&root.join("seed_data/knowledge_base.json")).expect("kb");
    let tmp_dir = TempDir::new().expect("tempdir");
    let nested = tmp_dir.path().join("fresh/data/thirukkural.db");

    let stats = compile_database(&canonical, &BTreeMap::new(), &knowledge, &nested, false)
        .expect("partial build into non-existent directory");
    assert!(nested.is_file());
    assert_eq!(stats.kurals, 1330);
    assert_eq!(stats.integrity_check, "ok");
}

#[test]
fn verify_production_database_strict() {
    let db_path = workspace_root().join("data/thirukkural.db");
    let db = KuralDb::open(&db_path).expect("open data/thirukkural.db");
    let stats = db.stats().expect("stats");
    assert_eq!(stats.build_mode, "full (1330/1330 enriched)");
    assert_all_invariants(&db);
}

struct IntegrationTestEmbedder;

impl TextEmbedder for IntegrationTestEmbedder {
    fn dim(&self) -> usize {
        256
    }

    fn embed_f32(&self, text: &str) -> Result<Vec<f32>, kural_db::DbError> {
        let mut vec = vec![0.0_f32; 256];
        let norm = kural_core::normalize_nfc_trimmed(text).to_ascii_lowercase();
        for tok in norm.split_whitespace() {
            let mut h: u64 = 0xcbf2_9ce4_8422_2325;
            for b in tok.as_bytes() {
                h ^= u64::from(*b);
                h = h.wrapping_mul(0x0100_0000_01b3);
            }
            let idx = (h as usize) % 256;
            let sign = if ((h >> 17) & 1) == 0 { 1.0 } else { -1.0 };
            vec[idx] += sign;
        }
        let l2: f32 = vec.iter().map(|v| v * v).sum::<f32>().sqrt();
        if l2 > 0.0 {
            for v in &mut vec {
                *v /= l2;
            }
        }
        Ok(vec)
    }
}

#[test]
fn vector_indexing_rrf_catalog_pools_and_bookmarks_work_offline() {
    let root = workspace_root();
    let canonical = load_and_audit_canonical(&root.join("seed_data/canonical")).expect("canonical");
    let knowledge = load_knowledge_base(&root.join("seed_data/knowledge_base.json")).expect("kb");
    let enriched = build_synthetic_full_enrichment(&canonical);

    let tmp_dir = TempDir::new().expect("tempdir");
    // Exercise spaces + UTF-8 Tamil chars in path to verify sqlite_immutable_uri encoding!
    let spaced_dir = tmp_dir.path().join("dir with spaces & தமிழ்");
    let db_path = spaced_dir.join("thirukkural.db");
    compile_database(&canonical, &enriched, &knowledge, &db_path, true).expect("compile strict db");

    let embedder = IntegrationTestEmbedder;
    let (k_cnt, kb_cnt) =
        index_database_embeddings(&db_path, &embedder, "hash_test").expect("index vectors");
    assert_eq!(k_cnt, 1330);
    assert!(kb_cnt >= 40);

    let db_no_emb = KuralDb::open(&db_path).expect("open spaced immutable db without embedder");
    assert!(!db_no_emb.embeddings_loaded());
    let db = db_no_emb.with_embedder(Arc::new(embedder));
    assert!(db.embeddings_loaded());

    let stats = db.stats().expect("stats");
    assert_eq!(stats.kural_embeddings, 1330);
    assert_eq!(stats.knowledge_embeddings, kb_cnt);
    assert_eq!(stats.integrity_check, "ok");

    // Catalog + division + pool queries
    let catalog = db.get_catalog().expect("catalog");
    assert_eq!(catalog.divisions.len(), 3);
    assert_eq!(catalog.iyals.len(), 9);
    assert_eq!(catalog.adhikarams.len(), 133);

    assert_eq!(db.get_kurals_by_division(1).expect("div 1").len(), 380);
    assert_eq!(db.get_kurals_by_division(2).expect("div 2").len(), 700);
    assert_eq!(db.get_kurals_by_division(3).expect("div 3").len(), 250);

    let kids_pool = db
        .get_pool_numbers_for_filter(None, true)
        .expect("kids unfiltered pool");
    assert_eq!(kids_pool.len(), 1080);
    assert_eq!(*kids_pool.last().unwrap(), 1080);

    let div_pool = db
        .get_pool_numbers_for_filter(Some(&ActiveFilter::Division(2)), false)
        .expect("div 2 pool");
    assert_eq!(div_pool.len(), 700);

    let theme_pool = db
        .get_pool_numbers_for_filter(
            Some(&ActiveFilter::ResearchTheme(ResearchTheme::Polity)),
            false,
        )
        .expect("polity pool");
    assert!(!theme_pool.is_empty());

    // RRF Kural + KB search
    let rrf = db
        .search_hybrid_rrf("friendship", None, 0.25, 60.0, 10)
        .expect("rrf search");
    assert!(!rrf.outcome.hits.is_empty());
    assert!(rrf.has_exact_or_fts_match);

    let rrf_oos = db
        .search_hybrid_rrf("quantum superconductor zxqv88", None, 0.85, 60.0, 10)
        .expect("rrf oos");
    assert!(!rrf_oos.has_exact_or_fts_match);
    assert!(rrf_oos.outcome.hits.is_empty());

    let kb_rrf = db
        .search_knowledge_rrf("Kanyakumari", 0.25, 60.0, 5)
        .expect("kb rrf");
    assert!(!kb_rrf.is_empty());

    // Hydrated Adult bookmarks round-trip
    let state_path = spaced_dir.join("user_state.db");
    let store = UserStateStore::open(&state_path, 0.85).expect("open user_state");
    let added = store
        .toggle_bookmark(423, Scholar::Parimel, "2026-11-25T10:55:00Z", &db)
        .expect("add bookmark");
    assert_eq!(added.len(), 1);
    assert_eq!(added[0].kural_number, 423);
    assert_eq!(added[0].preferred_scholar, Scholar::Parimel);
    assert!(!added[0].line1_ta.is_empty());

    let removed = store
        .toggle_bookmark(423, Scholar::Parimel, "2026-11-25T10:56:00Z", &db)
        .expect("remove bookmark");
    assert!(removed.is_empty());
}
