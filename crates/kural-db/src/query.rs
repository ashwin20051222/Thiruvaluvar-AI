use crate::DbError;
use crate::embedder::{Model2VecEmbedder, TextEmbedder};
use crate::search::{
    HybridRrfOutcome, KnowledgeRrfResult, RrfSearchParams, execute_hybrid_rrf_search,
    execute_hybrid_rrf_search_no_fuzzy, execute_hybrid_search, execute_knowledge_rrf_search,
    execute_knowledge_search, fetch_kural_summary, row_to_knowledge_entry, row_to_kural_summary,
};
use crate::vector::VectorIndex;
use kural_core::{
    ActiveFilter, Adhikaram, AgeGroup, CorpusCatalog, DbStats, Division, FullKural, Iyal,
    KnowledgeEntry, KuralAdult, KuralChild, KuralCore, KuralSummary, KuralYouth, ResearchTheme,
    SearchOutcome, Tag,
};
use rusqlite::{Connection, OpenFlags, OptionalExtension, params};
use std::collections::HashSet;
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};

/// Environment variable that overrides the default database location.
pub const DB_PATH_ENV: &str = "KURAL_DB_PATH";

const DEFAULT_DB_RELATIVE: &str = "data/thirukkural.db";

/// Resolve the compiled database path.
///
/// Order: explicit `cli_arg` → `KURAL_DB_PATH` env var → `./data/thirukkural.db` →
/// `../data/thirukkural.db` → `../../data/thirukkural.db` → the workspace root this crate was
/// built from.
pub fn resolve_db_path(cli_arg: Option<&Path>) -> Result<PathBuf, DbError> {
    let explicit = cli_arg.map(Path::to_path_buf).or_else(|| {
        std::env::var_os(DB_PATH_ENV)
            .filter(|v| !v.is_empty())
            .map(PathBuf::from)
    });
    if let Some(path) = explicit {
        if path.is_file() {
            return Ok(path);
        }
        return Err(DbError::DatabaseNotFound {
            primary: path,
            checked: "explicit --db / KURAL_DB_PATH only".to_string(),
        });
    }

    let workspace_default = Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../..")
        .join(DEFAULT_DB_RELATIVE);
    let candidates = [
        PathBuf::from(DEFAULT_DB_RELATIVE),
        Path::new("..").join(DEFAULT_DB_RELATIVE),
        Path::new("../..").join(DEFAULT_DB_RELATIVE),
        workspace_default,
    ];
    if let Some(found) = candidates.iter().find(|p| p.is_file()) {
        return Ok(found.clone());
    }

    let primary = std::env::current_dir()
        .map(|cwd| cwd.join(DEFAULT_DB_RELATIVE))
        .unwrap_or_else(|_| PathBuf::from(DEFAULT_DB_RELATIVE));
    let checked = candidates
        .iter()
        .skip(1)
        .map(|p| p.display().to_string())
        .collect::<Vec<_>>()
        .join(", ");
    Err(DbError::DatabaseNotFound { primary, checked })
}

/// Format an immutable read-only SQLite URI (`file:<encoded-path>?mode=ro&immutable=1`)
/// handling workspace directory paths that contain spaces, URI delimiters, or UTF-8 cleanly.
fn sqlite_immutable_uri(path: &Path) -> String {
    let resolved = path.canonicalize().unwrap_or_else(|_| path.to_path_buf());
    let raw = resolved.to_string_lossy();
    let mut encoded = String::with_capacity(raw.len() + 32);
    encoded.push_str("file:");
    for ch in raw.chars() {
        match ch {
            '%' => encoded.push_str("%25"),
            '?' => encoded.push_str("%3F"),
            '#' => encoded.push_str("%23"),
            '&' => encoded.push_str("%26"),
            ' ' => encoded.push_str("%20"),
            other => encoded.push(other),
        }
    }
    encoded.push_str("?mode=ro&immutable=1");
    encoded
}

/// Thread-safe, read-only SQLite + FTS5 + INT8 vector handle optimized for Raspberry Pi 4B.
#[derive(Clone)]
pub struct KuralDb {
    path: PathBuf,
    conn: Arc<Mutex<Connection>>,
    vectors: Option<Arc<VectorIndex>>,
    embedder: Option<Arc<dyn TextEmbedder>>,
}

impl std::fmt::Debug for KuralDb {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("KuralDb")
            .field("path", &self.path)
            .field("embeddings_loaded", &self.embeddings_loaded())
            .finish()
    }
}

impl KuralDb {
    /// Open an existing compiled `data/thirukkural.db` strictly read-only with memory-mapped I/O.
    /// Uses `?immutable=1` when no `-wal` sidecar is present (standard finalized DELETE journal).
    pub fn open<P: AsRef<Path>>(path: P) -> Result<Self, DbError> {
        let path_buf = path.as_ref().to_path_buf();
        if !path_buf.is_file() {
            return Err(DbError::DatabaseNotFound {
                primary: path_buf,
                checked: "no other locations".to_string(),
            });
        }

        let wal_sidecar = PathBuf::from(format!("{}-wal", path_buf.display()));
        let conn = if !wal_sidecar.exists() {
            let uri = sqlite_immutable_uri(&path_buf);
            let flags = OpenFlags::SQLITE_OPEN_READ_ONLY
                | OpenFlags::SQLITE_OPEN_NO_MUTEX
                | OpenFlags::SQLITE_OPEN_URI;
            Connection::open_with_flags(&uri, flags)?
        } else {
            let flags = OpenFlags::SQLITE_OPEN_READ_ONLY | OpenFlags::SQLITE_OPEN_NO_MUTEX;
            Connection::open_with_flags(&path_buf, flags)?
        };

        conn.execute_batch(
            "PRAGMA busy_timeout = 3000;
             PRAGMA query_only = ON;
             PRAGMA foreign_keys = ON;
             PRAGMA cache_size = -16000;
             PRAGMA temp_store = MEMORY;
             PRAGMA mmap_size = 67108864;",
        )?;

        let vectors = VectorIndex::try_load(&conn)?.map(Arc::new);

        Ok(Self {
            path: path_buf,
            conn: Arc::new(Mutex::new(conn)),
            vectors,
            embedder: None,
        })
    }

    /// Open `data/thirukkural.db` and attempt to load local `model2vec-rs` weights from
    /// `<models_dir>/embeddings` if present (degrades cleanly to FTS5-only when absent).
    pub fn open_with_models_dir<P: AsRef<Path>, M: AsRef<Path>>(
        db_path: P,
        models_dir: M,
    ) -> Result<Self, DbError> {
        let mut db = Self::open(db_path)?;
        let emb_dir = models_dir.as_ref().join("embeddings");
        if db.vectors.is_some() && emb_dir.join("model.safetensors").is_file() {
            match Model2VecEmbedder::load(&emb_dir) {
                Ok(emb) => db.embedder = Some(Arc::new(emb)),
                Err(err) => {
                    tracing::warn!(
                        path = %emb_dir.display(),
                        error = %err,
                        "could not load embeddings model; continuing with FTS5 only"
                    );
                }
            }
        }
        Ok(db)
    }

    /// Attach a custom query embedder (e.g., in tests or custom startup).
    pub fn with_embedder(mut self, embedder: Arc<dyn TextEmbedder>) -> Self {
        self.embedder = Some(embedder);
        self
    }

    /// Return true only when BOTH precomputed INT8 vectors AND a query embedder are loaded.
    pub fn embeddings_loaded(&self) -> bool {
        self.vectors.as_ref().is_some_and(|v| v.kural_count() > 0) && self.embedder.is_some()
    }

    fn with_conn<T, F>(&self, f: F) -> Result<T, DbError>
    where
        F: FnOnce(&Connection) -> Result<T, DbError>,
    {
        let guard = self.conn.lock().map_err(|_| DbError::LockPoisoned)?;
        f(&guard)
    }

    /// Fetch lightweight summary metadata for a single Kural (`1..=1330`).
    pub fn get_kural_summary(&self, number: u16) -> Result<Option<KuralSummary>, DbError> {
        if !(1..=1330).contains(&number) {
            return Err(DbError::KuralNotFound(number));
        }
        self.with_conn(|conn| fetch_kural_summary(conn, number))
    }

    /// Return the full Kural bundle (Layer 1 + Division + Iyal + Adhikaram + Child + Youth + Adult + Tags).
    pub fn get_kural_full(&self, number: u16) -> Result<FullKural, DbError> {
        if !(1..=1330).contains(&number) {
            return Err(DbError::KuralNotFound(number));
        }
        self.with_conn(|conn| {
            let kural: KuralCore = conn
                .query_row(
                    "SELECT number, division_id, iyal_id, adhikaram_id,
                            line1_ta, line2_ta, opening_letter_ta,
                            transliteration1, transliteration2,
                            meaning_mu_va, meaning_solomon, meaning_kalaignar, meaning_parimel,
                            translation_en, explanation_en, couplet_en,
                            tts_kural_ta, tts_kural_en
                     FROM kurals WHERE number = ?1",
                    params![number],
                    |r| {
                        Ok(KuralCore {
                            number: r.get(0)?,
                            division_id: r.get(1)?,
                            iyal_id: r.get(2)?,
                            adhikaram_id: r.get(3)?,
                            line1_ta: r.get(4)?,
                            line2_ta: r.get(5)?,
                            opening_letter_ta: r.get(6)?,
                            transliteration1: r.get(7)?,
                            transliteration2: r.get(8)?,
                            meaning_mu_va: r.get(9)?,
                            meaning_solomon: r.get(10)?,
                            meaning_kalaignar: r.get(11)?,
                            meaning_parimel: r.get(12)?,
                            translation_en: r.get(13)?,
                            explanation_en: r.get(14)?,
                            couplet_en: r.get(15)?,
                            tts_kural_ta: r.get(16)?,
                            tts_kural_en: r.get(17)?,
                        })
                    },
                )
                .optional()?
                .ok_or(DbError::KuralNotFound(number))?;

            let division = conn.query_row(
                "SELECT id, name_ta, alt_name_ta, name_en, transliteration, kural_start, kural_end
                 FROM divisions WHERE id = ?1",
                params![kural.division_id],
                |r| {
                    Ok(Division {
                        id: r.get(0)?,
                        name_ta: r.get(1)?,
                        alt_name_ta: r.get(2)?,
                        name_en: r.get(3)?,
                        transliteration: r.get(4)?,
                        kural_start: r.get(5)?,
                        kural_end: r.get(6)?,
                    })
                },
            )?;

            let iyal = conn.query_row(
                "SELECT id, division_id, name_ta, alt_name_ta, name_en, transliteration, chapter_start, chapter_end
                 FROM iyals WHERE id = ?1",
                params![kural.iyal_id],
                |r| {
                    Ok(Iyal {
                        id: r.get(0)?,
                        division_id: r.get(1)?,
                        name_ta: r.get(2)?,
                        alt_name_ta: r.get(3)?,
                        name_en: r.get(4)?,
                        transliteration: r.get(5)?,
                        chapter_start: r.get(6)?,
                        chapter_end: r.get(7)?,
                    })
                },
            )?;

            let adhikaram = conn.query_row(
                "SELECT id, division_id, iyal_id, name_ta, name_en, transliteration, kural_start, kural_end
                 FROM adhikarams WHERE id = ?1",
                params![kural.adhikaram_id],
                |r| {
                    Ok(Adhikaram {
                        id: r.get(0)?,
                        division_id: r.get(1)?,
                        iyal_id: r.get(2)?,
                        name_ta: r.get(3)?,
                        name_en: r.get(4)?,
                        transliteration: r.get(5)?,
                        kural_start: r.get(6)?,
                        kural_end: r.get(7)?,
                    })
                },
            )?;

            let child = conn
                .query_row(
                    "SELECT kural_number, kids_featured, simple_meaning_ta, simple_meaning_en,
                            story_title_ta, story_title_en, kutty_story_ta, kutty_story_en,
                            moral_ta, moral_en, game_blank_word_ta, game_options_ta, game_options_en
                     FROM kural_child WHERE kural_number = ?1",
                    params![number],
                    |r| {
                        let featured_int: i64 = r.get(1)?;
                        let opts_ta_raw: String = r.get(11)?;
                        let opts_en_raw: String = r.get(12)?;
                        let game_options_ta: [String; 4] =
                            serde_json::from_str(&opts_ta_raw).map_err(|e| {
                                rusqlite::Error::FromSqlConversionFailure(
                                    11,
                                    rusqlite::types::Type::Text,
                                    Box::new(e),
                                )
                            })?;
                        let game_options_en: [String; 4] =
                            serde_json::from_str(&opts_en_raw).map_err(|e| {
                                rusqlite::Error::FromSqlConversionFailure(
                                    12,
                                    rusqlite::types::Type::Text,
                                    Box::new(e),
                                )
                            })?;
                        Ok(KuralChild {
                            kural_number: r.get(0)?,
                            kids_featured: featured_int != 0,
                            simple_meaning_ta: r.get(2)?,
                            simple_meaning_en: r.get(3)?,
                            story_title_ta: r.get(4)?,
                            story_title_en: r.get(5)?,
                            kutty_story_ta: r.get(6)?,
                            kutty_story_en: r.get(7)?,
                            moral_ta: r.get(8)?,
                            moral_en: r.get(9)?,
                            game_blank_word_ta: r.get(10)?,
                            game_options_ta,
                            game_options_en,
                        })
                    },
                )
                .optional()?;

            let youth = conn
                .query_row(
                    "SELECT kural_number, analysis_ta, analysis_en, action_tip_ta, action_tip_en,
                            quiz_question_ta, quiz_question_en, quiz_options_ta, quiz_options_en, quiz_correct_idx
                     FROM kural_youth WHERE kural_number = ?1",
                    params![number],
                    |r| {
                        let opts_ta_raw: String = r.get(7)?;
                        let opts_en_raw: String = r.get(8)?;
                        let quiz_options_ta: [String; 4] =
                            serde_json::from_str(&opts_ta_raw).map_err(|e| {
                                rusqlite::Error::FromSqlConversionFailure(
                                    7,
                                    rusqlite::types::Type::Text,
                                    Box::new(e),
                                )
                            })?;
                        let quiz_options_en: [String; 4] =
                            serde_json::from_str(&opts_en_raw).map_err(|e| {
                                rusqlite::Error::FromSqlConversionFailure(
                                    8,
                                    rusqlite::types::Type::Text,
                                    Box::new(e),
                                )
                            })?;
                        Ok(KuralYouth {
                            kural_number: r.get(0)?,
                            analysis_ta: r.get(1)?,
                            analysis_en: r.get(2)?,
                            action_tip_ta: r.get(3)?,
                            action_tip_en: r.get(4)?,
                            quiz_question_ta: r.get(5)?,
                            quiz_question_en: r.get(6)?,
                            quiz_options_ta,
                            quiz_options_en,
                            quiz_correct_idx: r.get(9)?,
                        })
                    },
                )
                .optional()?;

            let adult = conn
                .query_row(
                    "SELECT kural_number, reflection_ta, reflection_en, research_theme
                     FROM kural_adult WHERE kural_number = ?1",
                    params![number],
                    |r| {
                        let theme_raw: String = r.get(3)?;
                        let research_theme: ResearchTheme = theme_raw.parse().map_err(|e| {
                            rusqlite::Error::FromSqlConversionFailure(
                                3,
                                rusqlite::types::Type::Text,
                                Box::new(e),
                            )
                        })?;
                        Ok(KuralAdult {
                            kural_number: r.get(0)?,
                            reflection_ta: r.get(1)?,
                            reflection_en: r.get(2)?,
                            research_theme,
                        })
                    },
                )
                .optional()?;

            let mut tag_stmt = conn.prepare_cached(
                "SELECT t.id, t.slug, t.name_ta, t.name_en
                 FROM tags t
                 JOIN kural_tags kt ON kt.tag_id = t.id
                 WHERE kt.kural_number = ?1
                 ORDER BY t.id ASC",
            )?;
            let tag_rows = tag_stmt.query_map(params![number], |r| {
                Ok(Tag {
                    id: r.get(0)?,
                    slug: r.get(1)?,
                    name_ta: r.get(2)?,
                    name_en: r.get(3)?,
                })
            })?;
            let mut tags = Vec::new();
            for t in tag_rows {
                tags.push(t?);
            }

            Ok(FullKural {
                kural,
                division,
                iyal,
                adhikaram,
                child,
                youth,
                adult,
                tags,
            })
        })
    }

    /// Deterministically select a daily Kural for `age` and `day_of_year`.
    pub fn get_daily_kural(&self, age: AgeGroup, day_of_year: u32) -> Result<FullKural, DbError> {
        let number = self.with_conn(|conn| {
            let pool: Vec<u16> = match age {
                AgeGroup::Kids => {
                    let mut stmt = conn.prepare_cached(
                        "SELECT kural_number FROM kural_child WHERE kids_featured = 1 ORDER BY kural_number ASC",
                    )?;
                    let mut rows: Vec<u16> = stmt
                        .query_map([], |r| r.get(0))?
                        .collect::<Result<Vec<_>, _>>()?;
                    if rows.is_empty() {
                        let mut fallback = conn.prepare_cached(
                            "SELECT number FROM kurals WHERE number <= 1080 ORDER BY number ASC",
                        )?;
                        rows = fallback
                            .query_map([], |r| r.get(0))?
                            .collect::<Result<Vec<_>, _>>()?;
                    }
                    rows
                }
                AgeGroup::Youth | AgeGroup::Adult => {
                    let mut stmt =
                        conn.prepare_cached("SELECT number FROM kurals ORDER BY number ASC")?;
                    stmt.query_map([], |r| r.get(0))?
                        .collect::<Result<Vec<_>, _>>()?
                }
            };

            if pool.is_empty() {
                return Err(DbError::Validation("kural pool is empty".into()));
            }
            let step: usize = match age {
                AgeGroup::Kids => 37,
                AgeGroup::Youth => 53,
                AgeGroup::Adult => 71,
            };
            let idx = (usize::try_from(day_of_year).unwrap_or(0) * step) % pool.len();
            Ok(pool[idx])
        })?;

        self.get_kural_full(number)
    }

    /// List all 3 Divisions ordered by id.
    pub fn list_divisions(&self) -> Result<Vec<Division>, DbError> {
        self.with_conn(|conn| {
            let mut stmt = conn.prepare_cached(
                "SELECT id, name_ta, alt_name_ta, name_en, transliteration, kural_start, kural_end
                 FROM divisions ORDER BY id ASC",
            )?;
            let rows = stmt.query_map([], |r| {
                Ok(Division {
                    id: r.get(0)?,
                    name_ta: r.get(1)?,
                    alt_name_ta: r.get(2)?,
                    name_en: r.get(3)?,
                    transliteration: r.get(4)?,
                    kural_start: r.get(5)?,
                    kural_end: r.get(6)?,
                })
            })?;
            Ok(rows.collect::<Result<Vec<_>, _>>()?)
        })
    }

    /// List Iyals (all 9, or filtered by `division_id`).
    pub fn list_iyals(&self, division_id: Option<u8>) -> Result<Vec<Iyal>, DbError> {
        self.with_conn(|conn| {
            let mut stmt = conn.prepare_cached(
                "SELECT id, division_id, name_ta, alt_name_ta, name_en, transliteration, chapter_start, chapter_end
                 FROM iyals
                 WHERE (?1 IS NULL OR division_id = ?1)
                 ORDER BY id ASC",
            )?;
            let rows = stmt.query_map(params![division_id], |r| {
                Ok(Iyal {
                    id: r.get(0)?,
                    division_id: r.get(1)?,
                    name_ta: r.get(2)?,
                    alt_name_ta: r.get(3)?,
                    name_en: r.get(4)?,
                    transliteration: r.get(5)?,
                    chapter_start: r.get(6)?,
                    chapter_end: r.get(7)?,
                })
            })?;
            Ok(rows.collect::<Result<Vec<_>, _>>()?)
        })
    }

    /// List Adhikarams (all 133, or filtered by `division_id`).
    pub fn list_adhikarams(&self, division_id: Option<u8>) -> Result<Vec<Adhikaram>, DbError> {
        self.with_conn(|conn| {
            let mut stmt = conn.prepare_cached(
                "SELECT id, division_id, iyal_id, name_ta, name_en, transliteration, kural_start, kural_end
                 FROM adhikarams
                 WHERE (?1 IS NULL OR division_id = ?1)
                 ORDER BY id ASC",
            )?;
            let rows = stmt.query_map(params![division_id], |r| {
                Ok(Adhikaram {
                    id: r.get(0)?,
                    division_id: r.get(1)?,
                    iyal_id: r.get(2)?,
                    name_ta: r.get(3)?,
                    name_en: r.get(4)?,
                    transliteration: r.get(5)?,
                    kural_start: r.get(6)?,
                    kural_end: r.get(7)?,
                })
            })?;
            Ok(rows.collect::<Result<Vec<_>, _>>()?)
        })
    }

    /// Cache complete 3-Paal / 9-Iyal / 133-Adhikaram hierarchy in one call for `kural-app`.
    pub fn get_catalog(&self) -> Result<CorpusCatalog, DbError> {
        Ok(CorpusCatalog {
            divisions: self.list_divisions()?,
            iyals: self.list_iyals(None)?,
            adhikarams: self.list_adhikarams(None)?,
        })
    }

    /// Return all Kural summaries belonging to `division_id` (`1..=3`) ordered ascending.
    pub fn get_kurals_by_division(&self, division_id: u8) -> Result<Vec<KuralSummary>, DbError> {
        self.with_conn(|conn| {
            let mut stmt = conn.prepare_cached(
                "SELECT k.number, k.division_id, k.adhikaram_id, a.name_ta, a.name_en,
                        k.line1_ta, k.line2_ta, k.transliteration1, k.transliteration2, k.translation_en
                 FROM kurals k
                 JOIN adhikarams a ON a.id = k.adhikaram_id
                 WHERE k.division_id = ?1
                 ORDER BY k.number ASC",
            )?;
            let rows = stmt.query_map(params![division_id], row_to_kural_summary)?;
            Ok(rows.collect::<Result<Vec<_>, _>>()?)
        })
    }

    /// Return all 10 Kural summaries belonging to `adhikaram_id`.
    pub fn get_kurals_by_adhikaram(&self, adhikaram_id: u8) -> Result<Vec<KuralSummary>, DbError> {
        self.with_conn(|conn| {
            let mut stmt = conn.prepare_cached(
                "SELECT k.number, k.division_id, k.adhikaram_id, a.name_ta, a.name_en,
                        k.line1_ta, k.line2_ta, k.transliteration1, k.transliteration2, k.translation_en
                 FROM kurals k
                 JOIN adhikarams a ON a.id = k.adhikaram_id
                 WHERE k.adhikaram_id = ?1
                 ORDER BY k.number ASC",
            )?;
            let rows = stmt.query_map(params![adhikaram_id], row_to_kural_summary)?;
            Ok(rows.collect::<Result<Vec<_>, _>>()?)
        })
    }

    /// Return Kural summaries tagged with `tag_slug`.
    pub fn get_kurals_by_tag(
        &self,
        tag_slug: &str,
        limit: usize,
    ) -> Result<Vec<KuralSummary>, DbError> {
        let slug = tag_slug.trim().to_ascii_lowercase();
        let limit_i64 = i64::try_from(limit).unwrap_or(50);
        self.with_conn(|conn| {
            let mut stmt = conn.prepare_cached(
                "SELECT k.number, k.division_id, k.adhikaram_id, a.name_ta, a.name_en,
                        k.line1_ta, k.line2_ta, k.transliteration1, k.transliteration2, k.translation_en
                 FROM kurals k
                 JOIN adhikarams a ON a.id = k.adhikaram_id
                 JOIN kural_tags kt ON kt.kural_number = k.number
                 JOIN tags t ON t.id = kt.tag_id
                 WHERE t.slug = ?1
                 ORDER BY k.number ASC
                 LIMIT ?2",
            )?;
            let rows = stmt.query_map(params![slug, limit_i64], row_to_kural_summary)?;
            Ok(rows.collect::<Result<Vec<_>, _>>()?)
        })
    }

    /// Return Kural summaries matching a scholar `ResearchTheme`.
    pub fn get_kurals_by_research_theme(
        &self,
        theme: ResearchTheme,
        limit: usize,
    ) -> Result<Vec<KuralSummary>, DbError> {
        let limit_i64 = i64::try_from(limit).unwrap_or(50);
        self.with_conn(|conn| {
            let mut stmt = conn.prepare_cached(
                "SELECT k.number, k.division_id, k.adhikaram_id, a.name_ta, a.name_en,
                        k.line1_ta, k.line2_ta, k.transliteration1, k.transliteration2, k.translation_en
                 FROM kurals k
                 JOIN adhikarams a ON a.id = k.adhikaram_id
                 JOIN kural_adult ka ON ka.kural_number = k.number
                 WHERE ka.research_theme = ?1
                 ORDER BY k.number ASC
                 LIMIT ?2",
            )?;
            let rows = stmt.query_map(params![theme.as_str(), limit_i64], row_to_kural_summary)?;
            Ok(rows.collect::<Result<Vec<_>, _>>()?)
        })
    }

    /// Return ordered Kural numbers for `PrevKural`/`NextKural` pool stepping.
    pub fn get_pool_numbers_for_filter(
        &self,
        filter: Option<&ActiveFilter>,
        kids_only: bool,
    ) -> Result<Vec<u16>, DbError> {
        self.with_conn(|conn| {
            let out: Vec<u16> = match filter {
                None => {
                    if kids_only {
                        let mut stmt = conn.prepare_cached(
                            "SELECT kural_number FROM kural_child
                             WHERE kids_featured = 1 AND kural_number <= 1080
                             ORDER BY kural_number ASC",
                        )?;
                        stmt.query_map([], |r| r.get(0))?
                            .collect::<Result<Vec<_>, _>>()?
                    } else {
                        let mut stmt =
                            conn.prepare_cached("SELECT number FROM kurals ORDER BY number ASC")?;
                        stmt.query_map([], |r| r.get(0))?
                            .collect::<Result<Vec<_>, _>>()?
                    }
                }
                Some(ActiveFilter::Division(div_id)) => {
                    let mut stmt = conn.prepare_cached(
                        "SELECT k.number FROM kurals k
                         LEFT JOIN kural_child kc ON kc.kural_number = k.number
                         WHERE k.division_id = ?1
                           AND (?2 = 0 OR (kc.kids_featured = 1 AND k.number <= 1080))
                         ORDER BY k.number ASC",
                    )?;
                    stmt.query_map(params![div_id, i64::from(kids_only)], |r| r.get(0))?
                        .collect::<Result<Vec<_>, _>>()?
                }
                Some(ActiveFilter::Tag(slug)) => {
                    let clean_slug = slug.trim().to_ascii_lowercase();
                    let mut stmt = conn.prepare_cached(
                        "SELECT k.number FROM kurals k
                         JOIN kural_tags kt ON kt.kural_number = k.number
                         JOIN tags t ON t.id = kt.tag_id
                         LEFT JOIN kural_child kc ON kc.kural_number = k.number
                         WHERE t.slug = ?1
                           AND (?2 = 0 OR (kc.kids_featured = 1 AND k.number <= 1080))
                         ORDER BY k.number ASC",
                    )?;
                    stmt.query_map(params![clean_slug, i64::from(kids_only)], |r| r.get(0))?
                        .collect::<Result<Vec<_>, _>>()?
                }
                Some(ActiveFilter::ResearchTheme(theme)) => {
                    let mut stmt = conn.prepare_cached(
                        "SELECT k.number FROM kurals k
                         JOIN kural_adult ka ON ka.kural_number = k.number
                         LEFT JOIN kural_child kc ON kc.kural_number = k.number
                         WHERE ka.research_theme = ?1
                           AND (?2 = 0 OR (kc.kids_featured = 1 AND k.number <= 1080))
                         ORDER BY k.number ASC",
                    )?;
                    stmt.query_map(params![theme.as_str(), i64::from(kids_only)], |r| r.get(0))?
                        .collect::<Result<Vec<_>, _>>()?
                }
            };
            Ok(out)
        })
    }

    /// Hybrid 4-tier search (exact number -> Adhikaram -> FTS5 BM25 -> strsim fuzzy fallback).
    pub fn search_hybrid(
        &self,
        query: &str,
        division_id: Option<u8>,
        limit: usize,
    ) -> Result<SearchOutcome, DbError> {
        self.with_conn(|conn| execute_hybrid_search(conn, query, division_id, limit))
    }

    /// Cosine-filtered Reciprocal Rank Fusion merging FTS5 BM25 + INT8 vector cosine.
    pub fn search_hybrid_rrf(
        &self,
        query: &str,
        division_id: Option<u8>,
        min_vec_cosine: f32,
        rrf_k: f32,
        limit: usize,
    ) -> Result<HybridRrfOutcome, DbError> {
        self.search_hybrid_rrf_inner(query, division_id, min_vec_cosine, rrf_k, limit, true)
    }

    /// Cosine-filtered Reciprocal Rank Fusion without the O(N) fuzzy fallback table scan.
    pub fn search_hybrid_rrf_no_fuzzy(
        &self,
        query: &str,
        division_id: Option<u8>,
        min_vec_cosine: f32,
        rrf_k: f32,
        limit: usize,
    ) -> Result<HybridRrfOutcome, DbError> {
        self.search_hybrid_rrf_inner(query, division_id, min_vec_cosine, rrf_k, limit, false)
    }

    fn search_hybrid_rrf_inner(
        &self,
        query: &str,
        division_id: Option<u8>,
        min_vec_cosine: f32,
        rrf_k: f32,
        limit: usize,
        allow_fuzzy_fallback: bool,
    ) -> Result<HybridRrfOutcome, DbError> {
        let vi = self.vectors.as_deref();
        let emb = self.embedder.as_deref();
        let opts = RrfSearchParams {
            division_id,
            min_vec_cosine,
            rrf_k,
            limit,
        };
        self.with_conn(|conn| {
            if allow_fuzzy_fallback {
                execute_hybrid_rrf_search(conn, vi, emb, query, opts)
            } else {
                execute_hybrid_rrf_search_no_fuzzy(conn, vi, emb, query, opts)
            }
        })
    }

    /// Search the curated offline Thiruvalluvar biography & FAQ knowledge base.
    pub fn search_knowledge(
        &self,
        query: &str,
        limit: usize,
    ) -> Result<Vec<KnowledgeEntry>, DbError> {
        self.with_conn(|conn| execute_knowledge_search(conn, query, limit))
    }

    /// Cosine-filtered Reciprocal Rank Fusion search over `knowledge_base`.
    pub fn search_knowledge_rrf(
        &self,
        query: &str,
        min_vec_cosine: f32,
        rrf_k: f32,
        limit: usize,
    ) -> Result<Vec<KnowledgeRrfResult>, DbError> {
        let vi = self.vectors.as_deref();
        let emb = self.embedder.as_deref();
        self.with_conn(|conn| {
            execute_knowledge_rrf_search(conn, vi, emb, query, min_vec_cosine, rrf_k, limit)
        })
    }

    /// Return all entries from `knowledge_base` ordered by `id`.
    pub fn list_all_knowledge(&self) -> Result<Vec<KnowledgeEntry>, DbError> {
        self.with_conn(|conn| {
            let mut stmt = conn.prepare_cached(
                "SELECT id, category, question_ta, question_en, answer_ta, answer_en, keywords
                 FROM knowledge_base ORDER BY id ASC",
            )?;
            let rows = stmt.query_map([], row_to_knowledge_entry)?;
            Ok(rows.collect::<Result<Vec<_>, _>>()?)
        })
    }

    /// Inspect table counts, build mode, file size, and SQLite quick integrity check.
    pub fn stats(&self) -> Result<DbStats, DbError> {
        let file_size_bytes = fs::metadata(&self.path)
            .map_err(|source| DbError::Io {
                path: self.path.clone(),
                source,
            })?
            .len();

        self.with_conn(|conn| {
            let mut table_stmt =
                conn.prepare("SELECT name FROM sqlite_master WHERE type = 'table'")?;
            let table_set: HashSet<String> = table_stmt
                .query_map([], |r| r.get::<_, String>(0))?
                .collect::<Result<HashSet<_>, _>>()?;

            let count_if_present = |table: &str| -> Result<usize, DbError> {
                if !table_set.contains(table) {
                    return Ok(0);
                }
                let sql = format!("SELECT COUNT(*) FROM {table}");
                let n: i64 = conn.query_row(&sql, [], |r| r.get(0))?;
                Ok(usize::try_from(n).unwrap_or(0))
            };

            const CORE_TABLES: [&str; 10] = [
                "divisions",
                "iyals",
                "adhikarams",
                "kurals",
                "kural_child",
                "kural_youth",
                "kural_adult",
                "tags",
                "kural_tags",
                "knowledge_base",
            ];
            let all_core_present = CORE_TABLES.iter().all(|t| table_set.contains(*t));

            let (
                divisions,
                iyals,
                adhikarams,
                kurals,
                kural_child,
                kural_youth,
                kural_adult,
                tags,
                kural_tags,
                knowledge_base,
            ) = if all_core_present {
                conn.query_row(
                    "SELECT
                        (SELECT COUNT(*) FROM divisions),
                        (SELECT COUNT(*) FROM iyals),
                        (SELECT COUNT(*) FROM adhikarams),
                        (SELECT COUNT(*) FROM kurals),
                        (SELECT COUNT(*) FROM kural_child),
                        (SELECT COUNT(*) FROM kural_youth),
                        (SELECT COUNT(*) FROM kural_adult),
                        (SELECT COUNT(*) FROM tags),
                        (SELECT COUNT(*) FROM kural_tags),
                        (SELECT COUNT(*) FROM knowledge_base)",
                    [],
                    |r| {
                        let to_usize = |col: usize| -> rusqlite::Result<usize> {
                            let n: i64 = r.get(col)?;
                            Ok(usize::try_from(n).unwrap_or(0))
                        };
                        Ok((
                            to_usize(0)?,
                            to_usize(1)?,
                            to_usize(2)?,
                            to_usize(3)?,
                            to_usize(4)?,
                            to_usize(5)?,
                            to_usize(6)?,
                            to_usize(7)?,
                            to_usize(8)?,
                            to_usize(9)?,
                        ))
                    },
                )?
            } else {
                (
                    count_if_present("divisions")?,
                    count_if_present("iyals")?,
                    count_if_present("adhikarams")?,
                    count_if_present("kurals")?,
                    count_if_present("kural_child")?,
                    count_if_present("kural_youth")?,
                    count_if_present("kural_adult")?,
                    count_if_present("tags")?,
                    count_if_present("kural_tags")?,
                    count_if_present("knowledge_base")?,
                )
            };

            let kural_embeddings = count_if_present("kural_embeddings")?;
            let knowledge_embeddings = count_if_present("knowledge_embeddings")?;

            let build_mode: String = if table_set.contains("db_meta") {
                conn.query_row(
                    "SELECT value FROM db_meta WHERE key = 'build_mode'",
                    [],
                    |r| r.get(0),
                )
                .optional()?
                .unwrap_or_else(|| "unknown".to_string())
            } else {
                "unknown".to_string()
            };

            let integrity_check: String =
                conn.query_row("PRAGMA quick_check;", [], |r| r.get(0))?;

            Ok(DbStats {
                divisions,
                iyals,
                adhikarams,
                kurals,
                kural_child,
                kural_youth,
                kural_adult,
                tags,
                kural_tags,
                knowledge_base,
                kural_embeddings,
                knowledge_embeddings,
                build_mode,
                integrity_check,
                file_size_bytes,
            })
        })
    }
}
