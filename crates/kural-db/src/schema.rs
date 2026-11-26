use rusqlite::Connection;

/// Complete relational + FTS5 schema for `data/thirukkural.db`.
pub const SCHEMA_SQL: &str = r#"
CREATE TABLE db_meta (
    key   TEXT PRIMARY KEY,
    value TEXT NOT NULL
);

CREATE TABLE divisions (
    id                 INTEGER PRIMARY KEY CHECK (id BETWEEN 1 AND 3),
    name_ta            TEXT NOT NULL,
    alt_name_ta        TEXT,
    name_en            TEXT NOT NULL,
    transliteration    TEXT NOT NULL,
    kural_start        INTEGER NOT NULL,
    kural_end          INTEGER NOT NULL
);

CREATE TABLE iyals (
    id              INTEGER PRIMARY KEY CHECK (id BETWEEN 1 AND 9),
    division_id     INTEGER NOT NULL REFERENCES divisions(id),
    name_ta         TEXT NOT NULL,
    alt_name_ta     TEXT,
    name_en         TEXT NOT NULL,
    transliteration TEXT NOT NULL,
    chapter_start   INTEGER NOT NULL,
    chapter_end     INTEGER NOT NULL
);

CREATE TABLE adhikarams (
    id              INTEGER PRIMARY KEY CHECK (id BETWEEN 1 AND 133),
    division_id     INTEGER NOT NULL REFERENCES divisions(id),
    iyal_id         INTEGER NOT NULL REFERENCES iyals(id),
    name_ta         TEXT NOT NULL,
    name_en         TEXT NOT NULL,
    transliteration TEXT NOT NULL,
    kural_start     INTEGER NOT NULL,
    kural_end       INTEGER NOT NULL
);

CREATE TABLE kurals (
    number             INTEGER PRIMARY KEY CHECK (number BETWEEN 1 AND 1330),
    division_id        INTEGER NOT NULL REFERENCES divisions(id),
    iyal_id            INTEGER NOT NULL REFERENCES iyals(id),
    adhikaram_id       INTEGER NOT NULL REFERENCES adhikarams(id),
    line1_ta           TEXT NOT NULL,
    line2_ta           TEXT NOT NULL,
    opening_letter_ta  TEXT NOT NULL,
    transliteration1   TEXT NOT NULL,
    transliteration2   TEXT NOT NULL,
    meaning_mu_va      TEXT NOT NULL,
    meaning_solomon    TEXT NOT NULL,
    meaning_kalaignar  TEXT NOT NULL,
    meaning_parimel    TEXT,
    translation_en     TEXT NOT NULL,
    explanation_en     TEXT NOT NULL,
    couplet_en         TEXT NOT NULL,
    tts_kural_ta       TEXT NOT NULL,
    tts_kural_en       TEXT NOT NULL
);

CREATE TABLE kural_child (
    kural_number        INTEGER PRIMARY KEY REFERENCES kurals(number) ON DELETE CASCADE,
    kids_featured       INTEGER NOT NULL CHECK (kids_featured IN (0, 1)),
    simple_meaning_ta   TEXT NOT NULL,
    simple_meaning_en   TEXT NOT NULL,
    story_title_ta      TEXT NOT NULL,
    story_title_en      TEXT NOT NULL,
    kutty_story_ta      TEXT NOT NULL,
    kutty_story_en      TEXT NOT NULL,
    moral_ta            TEXT NOT NULL,
    moral_en            TEXT NOT NULL,
    game_blank_word_ta  TEXT NOT NULL,
    game_options_ta     TEXT NOT NULL CHECK (json_valid(game_options_ta)),
    game_options_en     TEXT NOT NULL CHECK (json_valid(game_options_en))
);

CREATE TABLE kural_youth (
    kural_number      INTEGER PRIMARY KEY REFERENCES kurals(number) ON DELETE CASCADE,
    analysis_ta       TEXT NOT NULL,
    analysis_en       TEXT NOT NULL,
    action_tip_ta     TEXT NOT NULL,
    action_tip_en     TEXT NOT NULL,
    quiz_question_ta  TEXT NOT NULL,
    quiz_question_en  TEXT NOT NULL,
    quiz_options_ta   TEXT NOT NULL CHECK (json_valid(quiz_options_ta)),
    quiz_options_en   TEXT NOT NULL CHECK (json_valid(quiz_options_en)),
    quiz_correct_idx  INTEGER NOT NULL CHECK (quiz_correct_idx BETWEEN 0 AND 3)
);

CREATE TABLE kural_adult (
    kural_number    INTEGER PRIMARY KEY REFERENCES kurals(number) ON DELETE CASCADE,
    reflection_ta   TEXT NOT NULL,
    reflection_en   TEXT NOT NULL,
    research_theme  TEXT NOT NULL CHECK (research_theme IN ('virtue', 'philosophy', 'polity', 'love_ethics'))
);

CREATE TABLE tags (
    id       INTEGER PRIMARY KEY,
    slug     TEXT NOT NULL UNIQUE,
    name_ta  TEXT NOT NULL,
    name_en  TEXT NOT NULL
);

CREATE TABLE kural_tags (
    kural_number INTEGER NOT NULL REFERENCES kurals(number) ON DELETE CASCADE,
    tag_id       INTEGER NOT NULL REFERENCES tags(id) ON DELETE CASCADE,
    PRIMARY KEY (kural_number, tag_id)
);

CREATE TABLE knowledge_base (
    id          INTEGER PRIMARY KEY,
    category    TEXT NOT NULL,
    question_ta TEXT NOT NULL,
    question_en TEXT NOT NULL,
    answer_ta   TEXT NOT NULL,
    answer_en   TEXT NOT NULL,
    keywords    TEXT NOT NULL
);

CREATE INDEX idx_iyals_division ON iyals(division_id);
CREATE INDEX idx_adhikarams_division ON adhikarams(division_id);
CREATE INDEX idx_adhikarams_iyal ON adhikarams(iyal_id);
CREATE INDEX idx_kurals_division ON kurals(division_id);
CREATE INDEX idx_kurals_adhikaram ON kurals(adhikaram_id);
CREATE INDEX idx_kural_child_featured ON kural_child(kids_featured);
CREATE INDEX idx_kural_adult_theme ON kural_adult(research_theme);
CREATE INDEX idx_kural_tags_tag ON kural_tags(tag_id);

CREATE VIRTUAL TABLE kurals_fts USING fts5(
    kural_number UNINDEXED,
    adhikaram_ta,
    adhikaram_en,
    division_ta,
    division_en,
    line1_ta,
    line2_ta,
    transliteration,
    meaning_ta,
    translation_en,
    kutty_story_ta,
    kutty_story_en,
    youth_analysis_ta,
    youth_analysis_en,
    tags_text,
    -- CRITICAL INVARIANT: Keep `remove_diacritics 0`!
    -- Changing this to `remove_diacritics 1` strips Tamil vowel signs
    -- (uyirmei markers like ா, ி, ு, ெ) and pulli/virama (்),
    -- which corrupts Tamil word boundaries and lexical search.
    tokenize = 'unicode61 remove_diacritics 0'
);

CREATE VIRTUAL TABLE knowledge_fts USING fts5(
    kb_id UNINDEXED,
    question_ta,
    question_en,
    answer_ta,
    answer_en,
    keywords,
    -- CRITICAL INVARIANT: Keep `remove_diacritics 0`!
    -- Changing this to `remove_diacritics 1` strips Tamil vowel signs
    -- (uyirmei markers like ா, ி, ு, ெ) and pulli/virama (்),
    -- which corrupts Tamil word boundaries and lexical search.
    tokenize = 'unicode61 remove_diacritics 0'
);

CREATE TABLE IF NOT EXISTS kural_embeddings (
    kural_number INTEGER PRIMARY KEY REFERENCES kurals(number) ON DELETE CASCADE,
    vec_i8       BLOB NOT NULL,
    norm         REAL NOT NULL
);

CREATE TABLE IF NOT EXISTS knowledge_embeddings (
    kb_id        INTEGER PRIMARY KEY REFERENCES knowledge_base(id) ON DELETE CASCADE,
    vec_i8       BLOB NOT NULL,
    norm         REAL NOT NULL
);
"#;

/// Initialize schema tables and FTS5 virtual tables on a freshly created SQLite connection.
pub fn init_schema(conn: &Connection) -> Result<(), rusqlite::Error> {
    conn.execute_batch(SCHEMA_SQL)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn schema_enforces_json_valid_on_child_and_youth_options() {
        let conn = Connection::open_in_memory().expect("in-memory sqlite");
        init_schema(&conn).expect("init_schema with diacritic comments");

        conn.execute_batch(
            "INSERT INTO divisions (id, name_ta, name_en, transliteration, kural_start, kural_end)
             VALUES (1, 'அறம்', 'Virtue', 'Aram', 1, 380);
             INSERT INTO iyals (id, division_id, name_ta, name_en, transliteration, chapter_start, chapter_end)
             VALUES (1, 1, 'பாயிரம்', 'Prologue', 'Payiram', 1, 4);
             INSERT INTO adhikarams (id, division_id, iyal_id, name_ta, name_en, transliteration, kural_start, kural_end)
             VALUES (1, 1, 1, 'கடவுள் வாழ்த்து', 'Praise of God', 'Kadavul Vaazhthu', 1, 10);
             INSERT INTO kurals (
                 number, division_id, iyal_id, adhikaram_id,
                 line1_ta, line2_ta, opening_letter_ta,
                 transliteration1, transliteration2,
                 meaning_mu_va, meaning_solomon, meaning_kalaignar, meaning_parimel,
                 translation_en, explanation_en, couplet_en, tts_kural_ta, tts_kural_en
             ) VALUES (
                 1, 1, 1, 1, 'அகர', 'பகவன்', 'அ', 'Akara', 'Bhagavan',
                 'm1', 'm2', 'm3', NULL, 't', 'e', 'c', 'tts_ta', 'tts_en'
             );",
        )
        .expect("seed base rows");

        let bad_child = conn.execute(
            "INSERT INTO kural_child (
                kural_number, kids_featured, simple_meaning_ta, simple_meaning_en,
                story_title_ta, story_title_en, kutty_story_ta, kutty_story_en,
                moral_ta, moral_en, game_blank_word_ta, game_options_ta, game_options_en
            ) VALUES (1, 1, 's', 's', 't', 't', 'k', 'k', 'm', 'm', 'அகர', 'not-json', '[\"a\",\"b\",\"c\",\"d\"]')",
            [],
        );
        assert!(
            bad_child.is_err(),
            "invalid JSON in game_options_ta must fail CHECK"
        );

        let bad_youth = conn.execute(
            "INSERT INTO kural_youth (
                kural_number, analysis_ta, analysis_en, action_tip_ta, action_tip_en,
                quiz_question_ta, quiz_question_en, quiz_options_ta, quiz_options_en, quiz_correct_idx
            ) VALUES (1, 'a', 'a', 't', 't', 'q', 'q', '[\"a\",\"b\",\"c\",\"d\"]', '{bad', 0)",
            [],
        );
        assert!(
            bad_youth.is_err(),
            "invalid JSON in quiz_options_en must fail CHECK"
        );
    }
}
