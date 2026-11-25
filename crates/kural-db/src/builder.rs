use crate::DbError;
use crate::schema::init_schema;
use chrono::Utc;
use kural_core::{
    CANONICAL_TAGS, CanonicalDataset, DbStats, EnrichedChapterFile, KnowledgeEntry, KuralCore,
    normalize_enriched_chapter, normalize_nfc_trimmed, validate_enriched_chapter,
};
use rusqlite::{Connection, params};
use std::collections::{BTreeMap, HashMap};
use std::fs;
use std::path::Path;

/// Load and validate all available `adhikaram_XXX.json` checkpoint files from `enriched_dir`.
pub fn load_enriched_dir(
    enriched_dir: &Path,
    canonical: &CanonicalDataset,
) -> Result<BTreeMap<u8, EnrichedChapterFile>, DbError> {
    let mut map = BTreeMap::new();
    if !enriched_dir.exists() {
        return Ok(map);
    }

    let mut kurals_by_chapter: BTreeMap<u8, Vec<&KuralCore>> = BTreeMap::new();
    for k in &canonical.kurals {
        kurals_by_chapter.entry(k.adhikaram_id).or_default().push(k);
    }

    for id in 1u8..=133u8 {
        let path = enriched_dir.join(format!("adhikaram_{id:03}.json"));
        if !path.exists() {
            continue;
        }
        let raw = fs::read_to_string(&path).map_err(|source| DbError::Io {
            path: path.clone(),
            source,
        })?;
        let mut parsed: EnrichedChapterFile =
            serde_json::from_str(&raw).map_err(|source| DbError::Json {
                path: path.clone(),
                source,
            })?;
        normalize_enriched_chapter(&mut parsed);

        let refs = kurals_by_chapter.get(&id).map(Vec::as_slice).unwrap_or(&[]);
        if let Err(errs) = validate_enriched_chapter(&parsed, refs) {
            return Err(DbError::Validation(format!(
                "invalid enrichment file {}: {}",
                path.display(),
                errs.join("; ")
            )));
        }
        map.insert(id, parsed);
    }

    Ok(map)
}

/// Load and NFC-normalize `knowledge_base.json`.
pub fn load_knowledge_base(path: &Path) -> Result<Vec<KnowledgeEntry>, DbError> {
    let raw = fs::read_to_string(path).map_err(|source| DbError::Io {
        path: path.to_path_buf(),
        source,
    })?;
    let mut entries: Vec<KnowledgeEntry> =
        serde_json::from_str(&raw).map_err(|source| DbError::Json {
            path: path.to_path_buf(),
            source,
        })?;

    for e in &mut entries {
        e.category = normalize_nfc_trimmed(&e.category);
        e.question_ta = normalize_nfc_trimmed(&e.question_ta);
        e.question_en = normalize_nfc_trimmed(&e.question_en);
        e.answer_ta = normalize_nfc_trimmed(&e.answer_ta);
        e.answer_en = normalize_nfc_trimmed(&e.answer_en);
        e.keywords = normalize_nfc_trimmed(&e.keywords);
    }
    Ok(entries)
}

/// Compile Layer 1 canonical data, Layer 2 enrichments, and the knowledge base into a single
/// compacted, integrity-checked SQLite + FTS5 database at `out_path`.
pub fn compile_database(
    canonical: &CanonicalDataset,
    enriched_by_chapter: &BTreeMap<u8, EnrichedChapterFile>,
    knowledge: &[KnowledgeEntry],
    out_path: &Path,
    strict: bool,
) -> Result<DbStats, DbError> {
    if knowledge.len() < 40 {
        return Err(DbError::Validation(format!(
            "knowledge_base must have at least 40 entries, found {}",
            knowledge.len()
        )));
    }
    for (idx, entry) in knowledge.iter().enumerate() {
        if entry.question_ta.is_empty()
            || entry.question_en.is_empty()
            || entry.answer_ta.is_empty()
            || entry.answer_en.is_empty()
        {
            return Err(DbError::Validation(format!(
                "knowledge_base entry at index {idx} (id={}) has empty fields",
                entry.id
            )));
        }
    }

    if strict && enriched_by_chapter.len() != 133 {
        let missing: Vec<String> = (1u8..=133u8)
            .filter(|id| !enriched_by_chapter.contains_key(id))
            .map(|id| id.to_string())
            .collect();
        return Err(DbError::Validation(format!(
            "strict mode requires all 133 enriched adhikarams (found {}); missing chapters: {}",
            enriched_by_chapter.len(),
            missing.join(", ")
        )));
    }

    let mut kurals_by_chapter: BTreeMap<u8, Vec<&KuralCore>> = BTreeMap::new();
    for k in &canonical.kurals {
        kurals_by_chapter.entry(k.adhikaram_id).or_default().push(k);
    }

    let mut enriched_by_kural = HashMap::with_capacity(enriched_by_chapter.len() * 10);
    for (&chap_id, chapter) in enriched_by_chapter {
        let refs = kurals_by_chapter
            .get(&chap_id)
            .map(Vec::as_slice)
            .unwrap_or(&[]);
        if let Err(errs) = validate_enriched_chapter(chapter, refs) {
            return Err(DbError::Validation(format!(
                "chapter #{chap_id} failed validation: {}",
                errs.join("; ")
            )));
        }
        for rec in &chapter.kurals {
            enriched_by_kural.insert(rec.kural_number, rec);
        }
    }

    if strict && enriched_by_kural.len() != 1330 {
        return Err(DbError::Validation(format!(
            "strict mode requires 1330 enriched kurals, found {}",
            enriched_by_kural.len()
        )));
    }

    if let Some(parent) = out_path.parent()
        && !parent.as_os_str().is_empty()
    {
        fs::create_dir_all(parent).map_err(|source| DbError::Io {
            path: parent.to_path_buf(),
            source,
        })?;
    }

    let staging_path = out_path.with_extension("db.tmp");
    for sidecar in [
        staging_path.clone(),
        Path::new(&format!("{}-wal", staging_path.display())).to_path_buf(),
        Path::new(&format!("{}-shm", staging_path.display())).to_path_buf(),
    ] {
        if sidecar.exists() {
            let _ = fs::remove_file(&sidecar);
        }
    }

    let build_mode = if strict {
        "full (1330/1330 enriched)".to_string()
    } else {
        format!("partial ({}/1330 enriched)", enriched_by_kural.len())
    };

    {
        let mut conn = Connection::open(&staging_path)?;
        conn.execute_batch(
            "PRAGMA journal_mode = WAL;
             PRAGMA synchronous = NORMAL;
             PRAGMA foreign_keys = ON;",
        )?;
        init_schema(&conn)?;

        let tx = conn.transaction()?;

        {
            let mut stmt = tx.prepare(
                "INSERT INTO divisions (id, name_ta, alt_name_ta, name_en, transliteration, kural_start, kural_end)
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)",
            )?;
            for d in &canonical.divisions {
                stmt.execute(params![
                    d.id,
                    d.name_ta,
                    d.alt_name_ta,
                    d.name_en,
                    d.transliteration,
                    d.kural_start,
                    d.kural_end
                ])?;
            }
        }

        {
            let mut stmt = tx.prepare(
                "INSERT INTO iyals (id, division_id, name_ta, alt_name_ta, name_en, transliteration, chapter_start, chapter_end)
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8)",
            )?;
            for iy in &canonical.iyals {
                stmt.execute(params![
                    iy.id,
                    iy.division_id,
                    iy.name_ta,
                    iy.alt_name_ta,
                    iy.name_en,
                    iy.transliteration,
                    iy.chapter_start,
                    iy.chapter_end
                ])?;
            }
        }

        {
            let mut stmt = tx.prepare(
                "INSERT INTO adhikarams (id, division_id, iyal_id, name_ta, name_en, transliteration, kural_start, kural_end)
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8)",
            )?;
            for ad in &canonical.adhikarams {
                stmt.execute(params![
                    ad.id,
                    ad.division_id,
                    ad.iyal_id,
                    ad.name_ta,
                    ad.name_en,
                    ad.transliteration,
                    ad.kural_start,
                    ad.kural_end
                ])?;
            }
        }

        let mut tag_id_by_slug: HashMap<&'static str, u16> = HashMap::with_capacity(20);
        let mut tag_bilingual_by_slug: HashMap<&'static str, (&'static str, &'static str)> =
            HashMap::with_capacity(20);
        {
            let mut stmt = tx
                .prepare("INSERT INTO tags (id, slug, name_ta, name_en) VALUES (?1, ?2, ?3, ?4)")?;
            for (idx, &(slug, name_ta, name_en)) in CANONICAL_TAGS.iter().enumerate() {
                let id = u16::try_from(idx + 1).unwrap_or(0);
                stmt.execute(params![id, slug, name_ta, name_en])?;
                tag_id_by_slug.insert(slug, id);
                tag_bilingual_by_slug.insert(slug, (name_ta, name_en));
            }
        }

        let divisions_by_id: HashMap<u8, _> =
            canonical.divisions.iter().map(|d| (d.id, d)).collect();
        let adhikarams_by_id: HashMap<u8, _> =
            canonical.adhikarams.iter().map(|a| (a.id, a)).collect();

        {
            let mut ins_kural = tx.prepare(
                "INSERT INTO kurals (
                    number, division_id, iyal_id, adhikaram_id,
                    line1_ta, line2_ta, opening_letter_ta,
                    transliteration1, transliteration2,
                    meaning_mu_va, meaning_solomon, meaning_kalaignar, meaning_parimel,
                    translation_en, explanation_en, couplet_en,
                    tts_kural_ta, tts_kural_en
                ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13, ?14, ?15, ?16, ?17, ?18)",
            )?;
            let mut ins_child = tx.prepare(
                "INSERT INTO kural_child (
                    kural_number, kids_featured, simple_meaning_ta, simple_meaning_en,
                    story_title_ta, story_title_en, kutty_story_ta, kutty_story_en,
                    moral_ta, moral_en, game_blank_word_ta, game_options_ta, game_options_en
                ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13)",
            )?;
            let mut ins_youth = tx.prepare(
                "INSERT INTO kural_youth (
                    kural_number, analysis_ta, analysis_en, action_tip_ta, action_tip_en,
                    quiz_question_ta, quiz_question_en, quiz_options_ta, quiz_options_en, quiz_correct_idx
                ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10)",
            )?;
            let mut ins_adult = tx.prepare(
                "INSERT INTO kural_adult (
                    kural_number, reflection_ta, reflection_en, research_theme
                ) VALUES (?1, ?2, ?3, ?4)",
            )?;
            let mut ins_kural_tag =
                tx.prepare("INSERT INTO kural_tags (kural_number, tag_id) VALUES (?1, ?2)")?;
            let mut ins_fts = tx.prepare(
                "INSERT INTO kurals_fts (
                    kural_number, adhikaram_ta, adhikaram_en, division_ta, division_en,
                    line1_ta, line2_ta, transliteration, meaning_ta, translation_en,
                    kutty_story_ta, kutty_story_en, youth_analysis_ta, youth_analysis_en, tags_text
                ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13, ?14, ?15)",
            )?;

            for k in &canonical.kurals {
                ins_kural.execute(params![
                    k.number,
                    k.division_id,
                    k.iyal_id,
                    k.adhikaram_id,
                    k.line1_ta,
                    k.line2_ta,
                    k.opening_letter_ta,
                    k.transliteration1,
                    k.transliteration2,
                    k.meaning_mu_va,
                    k.meaning_solomon,
                    k.meaning_kalaignar,
                    k.meaning_parimel,
                    k.translation_en,
                    k.explanation_en,
                    k.couplet_en,
                    k.tts_kural_ta,
                    k.tts_kural_en,
                ])?;

                let mut kutty_story_ta = String::new();
                let mut kutty_story_en = String::new();
                let mut youth_analysis_ta = String::new();
                let mut youth_analysis_en = String::new();
                let mut tags_parts = Vec::new();

                if let Some(rec) = enriched_by_kural.get(&k.number) {
                    let kids_featured: i64 = i64::from(k.number <= 1080);
                    let game_opts_ta = serde_json::to_string(&rec.child.game_options_ta)
                        .map_err(|e| DbError::Validation(e.to_string()))?;
                    let game_opts_en = serde_json::to_string(&rec.child.game_options_en)
                        .map_err(|e| DbError::Validation(e.to_string()))?;

                    ins_child.execute(params![
                        k.number,
                        kids_featured,
                        rec.child.simple_meaning_ta,
                        rec.child.simple_meaning_en,
                        rec.child.story_title_ta,
                        rec.child.story_title_en,
                        rec.child.kutty_story_ta,
                        rec.child.kutty_story_en,
                        rec.child.moral_ta,
                        rec.child.moral_en,
                        rec.child.game_blank_word_ta,
                        game_opts_ta,
                        game_opts_en,
                    ])?;

                    let quiz_opts_ta = serde_json::to_string(&rec.youth.quiz_options_ta)
                        .map_err(|e| DbError::Validation(e.to_string()))?;
                    let quiz_opts_en = serde_json::to_string(&rec.youth.quiz_options_en)
                        .map_err(|e| DbError::Validation(e.to_string()))?;

                    ins_youth.execute(params![
                        k.number,
                        rec.youth.analysis_ta,
                        rec.youth.analysis_en,
                        rec.youth.action_tip_ta,
                        rec.youth.action_tip_en,
                        rec.youth.quiz_question_ta,
                        rec.youth.quiz_question_en,
                        quiz_opts_ta,
                        quiz_opts_en,
                        rec.youth.quiz_correct_idx,
                    ])?;

                    ins_adult.execute(params![
                        k.number,
                        rec.adult.reflection_ta,
                        rec.adult.reflection_en,
                        rec.adult.research_theme.as_str(),
                    ])?;

                    for slug in &rec.tags {
                        if let Some(&tag_id) = tag_id_by_slug.get(slug.as_str()) {
                            ins_kural_tag.execute(params![k.number, tag_id])?;
                        }
                        if let Some(&(name_ta, name_en)) = tag_bilingual_by_slug.get(slug.as_str())
                        {
                            tags_parts.push(format!("{slug} {name_ta} {name_en}"));
                        }
                    }

                    kutty_story_ta = rec.child.kutty_story_ta.clone();
                    kutty_story_en = rec.child.kutty_story_en.clone();
                    youth_analysis_ta = rec.youth.analysis_ta.clone();
                    youth_analysis_en = rec.youth.analysis_en.clone();
                }

                let div = divisions_by_id[&k.division_id];
                let ad = adhikarams_by_id[&k.adhikaram_id];
                let division_ta = match &div.alt_name_ta {
                    Some(alt) => format!("{} {}", div.name_ta, alt),
                    None => div.name_ta.clone(),
                };
                let translit = format!("{} {}", k.transliteration1, k.transliteration2);
                let meaning_ta = format!(
                    "{} {} {}",
                    k.meaning_mu_va, k.meaning_solomon, k.meaning_kalaignar
                );
                let translation_en = format!("{} {}", k.translation_en, k.explanation_en);
                let tags_text = tags_parts.join(" ");

                ins_fts.execute(params![
                    k.number,
                    ad.name_ta,
                    format!("{} {}", ad.name_en, ad.transliteration),
                    division_ta,
                    format!("{} {}", div.name_en, div.transliteration),
                    k.line1_ta,
                    k.line2_ta,
                    translit,
                    meaning_ta,
                    translation_en,
                    kutty_story_ta,
                    kutty_story_en,
                    youth_analysis_ta,
                    youth_analysis_en,
                    tags_text,
                ])?;
            }
        }

        {
            let mut ins_kb = tx.prepare(
                "INSERT INTO knowledge_base (id, category, question_ta, question_en, answer_ta, answer_en, keywords)
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)",
            )?;
            let mut ins_kb_fts = tx.prepare(
                "INSERT INTO knowledge_fts (kb_id, question_ta, question_en, answer_ta, answer_en, keywords)
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6)",
            )?;

            for (idx, entry) in knowledge.iter().enumerate() {
                let id = if entry.id > 0 {
                    entry.id
                } else {
                    i64::try_from(idx + 1).unwrap_or(1)
                };
                ins_kb.execute(params![
                    id,
                    entry.category,
                    entry.question_ta,
                    entry.question_en,
                    entry.answer_ta,
                    entry.answer_en,
                    entry.keywords,
                ])?;
                ins_kb_fts.execute(params![
                    id,
                    entry.question_ta,
                    entry.question_en,
                    entry.answer_ta,
                    entry.answer_en,
                    entry.keywords,
                ])?;
            }
        }

        {
            let mut ins_meta = tx.prepare("INSERT INTO db_meta (key, value) VALUES (?1, ?2)")?;
            ins_meta.execute(params!["schema_version", "2"])?;
            ins_meta.execute(params!["build_mode", &build_mode])?;
            ins_meta.execute(params!["built_at", Utc::now().to_rfc3339()])?;
            ins_meta.execute(params![
                "enriched_chapters",
                enriched_by_chapter.len().to_string()
            ])?;
            ins_meta.execute(params![
                "enriched_kurals",
                enriched_by_kural.len().to_string()
            ])?;
        }

        tx.commit()?;

        conn.execute_batch(
            "PRAGMA wal_checkpoint(TRUNCATE);
             PRAGMA journal_mode = DELETE;
             VACUUM;
             ANALYZE;",
        )?;

        let integrity: String = conn.query_row("PRAGMA integrity_check;", [], |row| row.get(0))?;
        if integrity != "ok" {
            return Err(DbError::Validation(format!(
                "sqlite integrity_check failed: {integrity}"
            )));
        }
    }

    // Remove any leftover sidecars on destination and staging before atomic rename.
    for sidecar in [
        Path::new(&format!("{}-wal", staging_path.display())).to_path_buf(),
        Path::new(&format!("{}-shm", staging_path.display())).to_path_buf(),
        Path::new(&format!("{}-wal", out_path.display())).to_path_buf(),
        Path::new(&format!("{}-shm", out_path.display())).to_path_buf(),
    ] {
        if sidecar.exists() {
            let _ = fs::remove_file(&sidecar);
        }
    }

    fs::rename(&staging_path, out_path).map_err(|source| DbError::Io {
        path: out_path.to_path_buf(),
        source,
    })?;

    let file_size_bytes = fs::metadata(out_path)
        .map_err(|source| DbError::Io {
            path: out_path.to_path_buf(),
            source,
        })?
        .len();

    Ok(DbStats {
        divisions: canonical.divisions.len(),
        iyals: canonical.iyals.len(),
        adhikarams: canonical.adhikarams.len(),
        kurals: canonical.kurals.len(),
        kural_child: enriched_by_kural.len(),
        kural_youth: enriched_by_kural.len(),
        kural_adult: enriched_by_kural.len(),
        tags: CANONICAL_TAGS.len(),
        kural_tags: enriched_by_kural.values().map(|r| r.tags.len()).sum(),
        knowledge_base: knowledge.len(),
        kural_embeddings: 0,
        knowledge_embeddings: 0,
        build_mode,
        integrity_check: "ok".to_string(),
        file_size_bytes,
    })
}
