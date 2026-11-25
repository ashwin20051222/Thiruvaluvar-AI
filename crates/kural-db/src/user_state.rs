use crate::{DbError, KuralDb};
use kural_core::{
    AgeGroup, BookmarkEntry, KidsProgress, Language, Scholar, Theme, UserPrefs, YouthProgress,
    snap_speech_speed,
};
use rusqlite::{Connection, OptionalExtension, params};
use std::collections::BTreeSet;
use std::fs;
use std::path::Path;
use std::sync::{Arc, Mutex};

const USER_STATE_SCHEMA_SQL: &str = r#"
CREATE TABLE IF NOT EXISTS preferences (
    id                 INTEGER PRIMARY KEY CHECK (id = 1),
    lang               TEXT NOT NULL CHECK (lang IN ('tamil', 'english')) DEFAULT 'tamil',
    theme              TEXT NOT NULL CHECK (theme IN ('dark', 'light')) DEFAULT 'dark',
    speech_speed_pct   INTEGER NOT NULL CHECK (speech_speed_pct IN (75, 100, 125)) DEFAULT 100,
    volume             REAL NOT NULL CHECK (volume >= 0.0 AND volume <= 1.0) DEFAULT 0.85,
    muted              INTEGER NOT NULL CHECK (muted IN (0, 1)) DEFAULT 0,
    default_age        TEXT NOT NULL CHECK (default_age IN ('kids', 'youth', 'adult')) DEFAULT 'youth',
    last_kural_kids    INTEGER NOT NULL CHECK (last_kural_kids BETWEEN 1 AND 1330) DEFAULT 1,
    last_kural_youth   INTEGER NOT NULL CHECK (last_kural_youth BETWEEN 1 AND 1330) DEFAULT 481,
    last_kural_adult   INTEGER NOT NULL CHECK (last_kural_adult BETWEEN 1 AND 1330) DEFAULT 423
);

CREATE TABLE IF NOT EXISTS kids_progress (
    id           INTEGER PRIMARY KEY CHECK (id = 1),
    stars_earned INTEGER NOT NULL CHECK (stars_earned >= 0) DEFAULT 0
);

CREATE TABLE IF NOT EXISTS kids_completed_stories (
    kural_number INTEGER PRIMARY KEY CHECK (kural_number BETWEEN 1 AND 1330)
);

CREATE TABLE IF NOT EXISTS kids_completed_games (
    kural_number INTEGER PRIMARY KEY CHECK (kural_number BETWEEN 1 AND 1330)
);

CREATE TABLE IF NOT EXISTS youth_progress (
    id               INTEGER PRIMARY KEY CHECK (id = 1),
    daily_streak     INTEGER NOT NULL CHECK (daily_streak >= 0) DEFAULT 0,
    last_active_date TEXT NOT NULL DEFAULT '',
    quiz_xp          INTEGER NOT NULL CHECK (quiz_xp >= 0) DEFAULT 0
);

CREATE TABLE IF NOT EXISTS youth_answered_quizzes (
    kural_number  INTEGER PRIMARY KEY CHECK (kural_number BETWEEN 1 AND 1330),
    first_correct INTEGER NOT NULL CHECK (first_correct IN (0, 1))
);

CREATE TABLE IF NOT EXISTS adult_bookmarks (
    kural_number      INTEGER PRIMARY KEY CHECK (kural_number BETWEEN 1 AND 1330),
    preferred_scholar TEXT NOT NULL CHECK (preferred_scholar IN ('mu_va', 'solomon', 'kalaignar', 'parimel', 'english')),
    created_at        TEXT NOT NULL
);
"#;

fn speed_to_pct(raw: f32) -> i64 {
    match snap_speech_speed(raw) {
        s if (s - 0.75).abs() < 0.05 => 75,
        s if (s - 1.25).abs() < 0.05 => 125,
        _ => 100,
    }
}

fn pct_to_speed(pct: i64) -> f32 {
    match pct {
        75 => 0.75,
        125 => 1.25,
        _ => 1.0,
    }
}

/// Persistent mutable SQLite store (`data/user_state.db`), isolated from read-only `thirukkural.db`.
#[derive(Clone, Debug)]
pub struct UserStateStore {
    conn: Arc<Mutex<Connection>>,
}

impl UserStateStore {
    /// Open or create `data/user_state.db`, ensuring parent dirs, WAL durability, normalized
    /// tables, and singleton `id = 1` rows exist.
    pub fn open<P: AsRef<Path>>(path: P, seed_volume: f32) -> Result<Self, DbError> {
        let p = path.as_ref();
        if let Some(parent) = p.parent()
            && !parent.as_os_str().is_empty()
        {
            fs::create_dir_all(parent).map_err(|source| DbError::Io {
                path: parent.to_path_buf(),
                source,
            })?;
        }

        let conn = Connection::open(p)?;
        conn.execute_batch(
            "PRAGMA busy_timeout = 3000;
             PRAGMA journal_mode = WAL;
             PRAGMA synchronous = FULL;
             PRAGMA foreign_keys = ON;",
        )?;
        conn.execute_batch(USER_STATE_SCHEMA_SQL)?;

        let safe_vol = if seed_volume.is_finite() {
            seed_volume.clamp(0.0, 1.0)
        } else {
            0.85
        };
        let clamped_vol = f64::from(safe_vol);
        conn.execute(
            "INSERT OR IGNORE INTO preferences (id, volume) VALUES (1, ?1)",
            params![clamped_vol],
        )?;
        conn.execute("INSERT OR IGNORE INTO kids_progress (id) VALUES (1)", [])?;
        conn.execute("INSERT OR IGNORE INTO youth_progress (id) VALUES (1)", [])?;

        Ok(Self {
            conn: Arc::new(Mutex::new(conn)),
        })
    }

    fn with_conn<T, F>(&self, f: F) -> Result<T, DbError>
    where
        F: FnOnce(&mut Connection) -> Result<T, DbError>,
    {
        let mut guard = self.conn.lock().map_err(|_| DbError::LockPoisoned)?;
        f(&mut guard)
    }

    /// Read current persisted `UserPrefs`.
    pub fn get_prefs(&self) -> Result<UserPrefs, DbError> {
        self.with_conn(|conn| {
            conn.query_row(
                "SELECT lang, theme, speech_speed_pct, volume, muted, default_age,
                        last_kural_kids, last_kural_youth, last_kural_adult
                 FROM preferences WHERE id = 1",
                [],
                |r| {
                    let lang_s: String = r.get(0)?;
                    let theme_s: String = r.get(1)?;
                    let speed_pct: i64 = r.get(2)?;
                    let vol: f64 = r.get(3)?;
                    let muted_i: i64 = r.get(4)?;
                    let age_s: String = r.get(5)?;
                    Ok(UserPrefs {
                        lang: lang_s.parse().unwrap_or(Language::Tamil),
                        theme: theme_s.parse().unwrap_or(Theme::Dark),
                        speech_speed: pct_to_speed(speed_pct),
                        volume: (vol as f32).clamp(0.0, 1.0),
                        muted: muted_i != 0,
                        default_age: age_s.parse().unwrap_or(AgeGroup::Youth),
                        last_kural_kids: r.get(6)?,
                        last_kural_youth: r.get(7)?,
                        last_kural_adult: r.get(8)?,
                    })
                },
            )
            .map_err(DbError::from)
        })
    }

    /// Persist `UserPrefs`, snapping discrete speed presets cleanly and guarding Kural ranges.
    pub fn save_prefs(&self, prefs: &UserPrefs) -> Result<(), DbError> {
        for &num in &[
            prefs.last_kural_kids,
            prefs.last_kural_youth,
            prefs.last_kural_adult,
        ] {
            if !(1..=1330).contains(&num) {
                return Err(DbError::InvalidArgument(format!(
                    "last_kural #{num} outside 1..=1330"
                )));
            }
        }
        let pct = speed_to_pct(prefs.speech_speed);
        let safe_vol = if prefs.volume.is_finite() {
            prefs.volume.clamp(0.0, 1.0)
        } else {
            0.85
        };
        let vol = f64::from(safe_vol);
        let muted_i: i64 = i64::from(prefs.muted);

        self.with_conn(|conn| {
            conn.execute(
                "UPDATE preferences SET
                    lang = ?1,
                    theme = ?2,
                    speech_speed_pct = ?3,
                    volume = ?4,
                    muted = ?5,
                    default_age = ?6,
                    last_kural_kids = ?7,
                    last_kural_youth = ?8,
                    last_kural_adult = ?9
                 WHERE id = 1",
                params![
                    prefs.lang.as_str(),
                    prefs.theme.as_str(),
                    pct,
                    vol,
                    muted_i,
                    prefs.default_age.to_string(),
                    prefs.last_kural_kids,
                    prefs.last_kural_youth,
                    prefs.last_kural_adult,
                ],
            )?;
            Ok(())
        })
    }

    /// Load `KidsProgress` with normalized completed story/game sets.
    pub fn get_kids_progress(&self) -> Result<KidsProgress, DbError> {
        self.with_conn(|conn| Self::read_kids_progress_conn(conn))
    }

    fn read_kids_progress_conn(conn: &Connection) -> Result<KidsProgress, DbError> {
        let stars_earned: u32 = conn.query_row(
            "SELECT stars_earned FROM kids_progress WHERE id = 1",
            [],
            |r| r.get(0),
        )?;

        let mut completed_stories = BTreeSet::new();
        {
            let mut stmt = conn.prepare(
                "SELECT kural_number FROM kids_completed_stories ORDER BY kural_number ASC",
            )?;
            for row in stmt.query_map([], |r| r.get::<_, u16>(0))? {
                completed_stories.insert(row?);
            }
        }

        let mut completed_games = BTreeSet::new();
        {
            let mut stmt = conn.prepare(
                "SELECT kural_number FROM kids_completed_games ORDER BY kural_number ASC",
            )?;
            for row in stmt.query_map([], |r| r.get::<_, u16>(0))? {
                completed_games.insert(row?);
            }
        }

        Ok(KidsProgress {
            stars_earned,
            completed_stories,
            completed_games,
        })
    }

    /// Idempotently mark a Kids `KuttyStory` completed (`0` extra stars).
    pub fn record_kids_story_completed(&self, kural_number: u16) -> Result<KidsProgress, DbError> {
        if !(1..=1330).contains(&kural_number) {
            return Err(DbError::InvalidArgument(format!(
                "kural_number #{kural_number} outside 1..=1330"
            )));
        }
        self.with_conn(|conn| {
            conn.execute(
                "INSERT OR IGNORE INTO kids_completed_stories (kural_number) VALUES (?1)",
                params![kural_number],
            )?;
            Self::read_kids_progress_conn(conn)
        })
    }

    /// Record a Kids Word Game answer; awards `+1` star atomically on the first correct answer for `kural_number`.
    pub fn record_kids_game_attempt(
        &self,
        kural_number: u16,
        correct: bool,
    ) -> Result<KidsProgress, DbError> {
        if !(1..=1330).contains(&kural_number) {
            return Err(DbError::InvalidArgument(format!(
                "kural_number #{kural_number} outside 1..=1330"
            )));
        }
        self.with_conn(|conn| {
            let tx = conn.transaction()?;
            if correct {
                let inserted = tx.execute(
                    "INSERT OR IGNORE INTO kids_completed_games (kural_number) VALUES (?1)",
                    params![kural_number],
                )?;
                if inserted > 0 {
                    tx.execute(
                        "UPDATE kids_progress SET stars_earned = stars_earned + 1 WHERE id = 1",
                        [],
                    )?;
                }
            }
            tx.commit()?;
            Self::read_kids_progress_conn(conn)
        })
    }

    /// Load `YouthProgress` with streak and answered quiz set.
    pub fn get_youth_progress(&self) -> Result<YouthProgress, DbError> {
        self.with_conn(|conn| Self::read_youth_progress_conn(conn))
    }

    fn read_youth_progress_conn(conn: &Connection) -> Result<YouthProgress, DbError> {
        let (daily_streak, last_active_date, quiz_xp): (u32, String, u32) = conn.query_row(
            "SELECT daily_streak, last_active_date, quiz_xp FROM youth_progress WHERE id = 1",
            [],
            |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)),
        )?;

        let mut answered_quizzes = BTreeSet::new();
        {
            let mut stmt = conn.prepare(
                "SELECT kural_number FROM youth_answered_quizzes ORDER BY kural_number ASC",
            )?;
            for row in stmt.query_map([], |r| r.get::<_, u16>(0))? {
                answered_quizzes.insert(row?);
            }
        }

        Ok(YouthProgress {
            daily_streak,
            last_active_date,
            quiz_xp,
            answered_quizzes,
        })
    }

    /// Record a Youth Quiz attempt with offline RTC-safe daily streak update (`+20 XP` on first
    /// correct attempt per Kural, `+5 XP` on first wrong participation).
    pub fn record_youth_quiz_attempt(
        &self,
        kural_number: u16,
        correct: bool,
        today_ymd: &str,
        yesterday_ymd: &str,
    ) -> Result<YouthProgress, DbError> {
        if !(1..=1330).contains(&kural_number) {
            return Err(DbError::InvalidArgument(format!(
                "kural_number #{kural_number} outside 1..=1330"
            )));
        }
        self.with_conn(|conn| {
            let tx = conn.transaction()?;

            let (cur_streak, last_date, _xp): (u32, String, u32) = tx.query_row(
                "SELECT daily_streak, last_active_date, quiz_xp FROM youth_progress WHERE id = 1",
                [],
                |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)),
            )?;

            // RTC-safe streak progression: backward clock skew never resets an earned streak.
            let (new_streak, new_date): (u32, &str) = if last_date.as_str() >= today_ymd {
                (cur_streak.max(1), last_date.as_str())
            } else if last_date.as_str() == yesterday_ymd {
                (cur_streak.saturating_add(1), today_ymd)
            } else {
                (1, today_ymd)
            };

            let prev_correct: Option<i64> = tx
                .query_row(
                    "SELECT first_correct FROM youth_answered_quizzes WHERE kural_number = ?1",
                    params![kural_number],
                    |r| r.get(0),
                )
                .optional()?;

            let xp_gain: u32 = match prev_correct {
                None => {
                    tx.execute(
                        "INSERT INTO youth_answered_quizzes (kural_number, first_correct) VALUES (?1, ?2)",
                        params![kural_number, i64::from(correct)],
                    )?;
                    if correct { 20 } else { 5 }
                }
                Some(0) if correct => {
                    tx.execute(
                        "UPDATE youth_answered_quizzes SET first_correct = 1 WHERE kural_number = ?1",
                        params![kural_number],
                    )?;
                    15
                }
                Some(_) => 0,
            };

            tx.execute(
                "UPDATE youth_progress SET
                    daily_streak = ?1,
                    last_active_date = ?2,
                    quiz_xp = quiz_xp + ?3
                 WHERE id = 1",
                params![new_streak, new_date, xp_gain],
            )?;

            tx.commit()?;
            Self::read_youth_progress_conn(conn)
        })
    }

    /// List all bookmarks hydrated with Adhikaram names and `line1_ta` from `corpus`.
    pub fn list_bookmarks(&self, corpus: &KuralDb) -> Result<Vec<BookmarkEntry>, DbError> {
        let raw: Vec<(u16, String, String)> = self.with_conn(|conn| {
            let mut stmt = conn.prepare(
                "SELECT kural_number, preferred_scholar, created_at
                 FROM adult_bookmarks
                 ORDER BY created_at DESC, kural_number ASC",
            )?;
            let rows = stmt.query_map([], |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)))?;
            let mut out = Vec::new();
            for row in rows {
                out.push(row?);
            }
            Ok(out)
        })?;

        let mut entries = Vec::with_capacity(raw.len());
        for (num, scholar_s, created_at) in raw {
            let summary = corpus
                .get_kural_summary(num)?
                .ok_or(DbError::KuralNotFound(num))?;
            entries.push(BookmarkEntry {
                kural_number: num,
                preferred_scholar: scholar_s.parse().unwrap_or(Scholar::MuVa),
                adhikaram_ta: summary.adhikaram_ta,
                adhikaram_en: summary.adhikaram_en,
                line1_ta: summary.line1_ta,
                created_at,
            });
        }
        Ok(entries)
    }

    /// Toggle bookmark on `kural_number` (insert with `scholar` or delete if present) and return
    /// the updated hydrated list.
    pub fn toggle_bookmark(
        &self,
        kural_number: u16,
        scholar: Scholar,
        now_rfc3339: &str,
        corpus: &KuralDb,
    ) -> Result<Vec<BookmarkEntry>, DbError> {
        if !(1..=1330).contains(&kural_number) {
            return Err(DbError::InvalidArgument(format!(
                "kural_number #{kural_number} outside 1..=1330"
            )));
        }
        self.with_conn(|conn| {
            let deleted = conn.execute(
                "DELETE FROM adult_bookmarks WHERE kural_number = ?1",
                params![kural_number],
            )?;
            if deleted == 0 {
                conn.execute(
                    "INSERT INTO adult_bookmarks (kural_number, preferred_scholar, created_at)
                     VALUES (?1, ?2, ?3)",
                    params![kural_number, scholar.as_str(), now_rfc3339],
                )?;
            }
            Ok(())
        })?;
        self.list_bookmarks(corpus)
    }

    /// Load all snapshot state atomically at controller startup.
    pub fn load_snapshot_state(
        &self,
        corpus: &KuralDb,
    ) -> Result<(UserPrefs, KidsProgress, YouthProgress, Vec<BookmarkEntry>), DbError> {
        let prefs = self.get_prefs()?;
        let kids = self.get_kids_progress()?;
        let youth = self.get_youth_progress()?;
        let bookmarks = self.list_bookmarks(corpus)?;
        Ok((prefs, kids, youth, bookmarks))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;

    #[test]
    fn user_state_prefs_kids_and_youth_streak_work_safely() {
        let dir = TempDir::new().expect("tempdir");
        let db_path = dir.path().join("nested/state/user_state.db");
        let store = UserStateStore::open(&db_path, 0.90).expect("open store");

        let mut prefs = store.get_prefs().expect("default prefs");
        assert!((prefs.volume - 0.90).abs() < 1e-5);
        assert_eq!(prefs.lang, Language::Tamil);
        assert_eq!(prefs.speech_speed, 1.0);

        prefs.lang = Language::English;
        prefs.theme = Theme::Light;
        prefs.speech_speed = 1.24; // snaps to 1.25
        prefs.last_kural_kids = 100;
        store.save_prefs(&prefs).expect("save prefs");

        let reloaded = store.get_prefs().expect("reload prefs");
        assert_eq!(reloaded.lang, Language::English);
        assert_eq!(reloaded.theme, Theme::Light);
        assert_eq!(reloaded.speech_speed, 1.25);
        assert_eq!(reloaded.last_kural_kids, 100);

        // Invalid range rejected cleanly
        let mut bad = reloaded.clone();
        bad.last_kural_youth = 1331;
        assert!(matches!(
            store.save_prefs(&bad).unwrap_err(),
            DbError::InvalidArgument(_)
        ));

        // Kids story + game star idempotency
        let p1 = store.record_kids_story_completed(1).expect("story 1");
        assert!(p1.completed_stories.contains(&1));
        assert_eq!(p1.stars_earned, 0);

        let p2 = store
            .record_kids_game_attempt(1, false)
            .expect("wrong guess");
        assert_eq!(p2.stars_earned, 0);
        let p3 = store.record_kids_game_attempt(1, true).expect("first win");
        assert_eq!(p3.stars_earned, 1);
        assert!(p3.completed_games.contains(&1));
        let p4 = store.record_kids_game_attempt(1, true).expect("repeat win");
        assert_eq!(p4.stars_earned, 1);

        // Youth daily streak + backward RTC protection
        let y1 = store
            .record_youth_quiz_attempt(481, false, "2026-11-24", "2026-11-23")
            .expect("day 1 participation");
        assert_eq!(y1.daily_streak, 1);
        assert_eq!(y1.quiz_xp, 5);

        let y2 = store
            .record_youth_quiz_attempt(481, true, "2026-11-25", "2026-11-24")
            .expect("day 2 consecutive");
        assert_eq!(y2.daily_streak, 2);
        assert_eq!(y2.quiz_xp, 20);

        // Clock jumps backward to 2026-11-20: must NOT wipe streak!
        let y3 = store
            .record_youth_quiz_attempt(482, true, "2026-11-20", "2026-11-19")
            .expect("clock skew safe");
        assert_eq!(y3.daily_streak, 2);
        assert_eq!(y3.quiz_xp, 40);
    }
}
