# Thiruvalluvar AI — Approved Offline Database Design Specification (EGM Gate)

**Status:** APPROVED  
**Phase:** Database-Only Build (100% Pure Rust, Zero Python)  
**Target Workspace:** `thiruvalluvar-ai-rs/`  
**Target Runtime:** Raspberry Pi 4B (ARM64 Linux, SD-card longevity) & Desktop Linux/macOS/Windows

---

## 1. Architecture & Workspace Layout

```text
thiruvalluvar-ai-rs/
├── Cargo.toml                        # Workspace root (Edition 2024)
├── .rustfmt.toml                     # Deterministic formatting
├── docs/designs/
│   └── db_design_approved.md         # This EGM design & readiness spec
├── seed_data/
│   ├── canonical/                    # Verified Layer 1 raw JSON
│   ├── enriched/                     # Layer 2 checkpoints: adhikaram_001.json .. adhikaram_133.json
│   └── knowledge_base.json           # >= 40 curated bilingual bio/structure/FAQ entries
├── data/
│   └── thirukkural.db                # Final compiled SQLite + FTS5 DB (< 35 MB)
└── crates/
    ├── kural-core/                   # Domain models, NFC/grapheme helpers, canonical & enrichment validators
    ├── kural-db/                     # rusqlite (bundled + FTS5) + strsim hybrid search + CLIs:
    │   └── src/bin/
    │       ├── fetch_canonical.rs    # Pure-Rust canonical downloader & forensic auditor
    │       ├── seed_db.rs            # Pure-Rust compiler: Layer 1 + Layer 2 -> data/thirukkural.db
    │       └── kural_cli.rs          # Terminal verification & inspection CLI
    └── kural-enricher/               # Pure-Rust resumable async Layer 2 enricher (Gemini + Ollama)
```

Future crates (`kural-audio`, `kural-ai`, `kural-voice-cmd`, `kural-app`) will depend solely on `kural-core` and `kural-db` (`KuralDb`).

---

## 2. Layer 1 Canonical Data & Structural Normalization

### 2.1 Sources
1. `https://raw.githubusercontent.com/tk120404/thirukkural/master/thirukkural.json` (Apache-2.0) — 1,330 couplets (`Line1`, `Line2`, `Translation`, `mv`, `sp`, `mk`, `explanation`, `couplet`, `transliteration1`, `transliteration2`).
2. `https://raw.githubusercontent.com/tk120404/thirukkural/master/detail.json` (Apache-2.0) — 3 Divisions, 133 Adhikarams.
3. `https://raw.githubusercontent.com/vijayanandrp/Thirukkural-Tamil-Dataset/master/data/thirukkural_meanings.json` (Optional, non-fatal) — `pari_melakar` commentary (`meaning_parimel`, nullable).
4. `https://github.com/ashwin20051222/AI-MODEL-1` (Read-only reference for cultural bio/monument/FAQ facts only; all `.py` files ignored).

### 2.2 Classical 3-Paal / 9-Iyal / 133-Adhikaram Mapping
`detail.json` splits `அங்கவியல்` into 5 sub-groups (6..=10) and names Division 3 `"காமத்துப்பால்"`. The canonical loader normalizes this into the classical 3-Paal, 9-Iyal, 133-Adhikaram hierarchy:

| Division ID | Tamil Name (Primary / Alt) | English Name | Chapters | Kurals | Count |
|---:|---|---|---|---:|---:|
| 1 | அறத்துப்பால் | Book of Virtue | 1..=38 | 1..=380 | 380 |
| 2 | பொருட்பால் | Book of Wealth & Polity | 39..=108 | 381..=1080 | 700 |
| 3 | இன்பத்துப்பால் (alt: காமத்துப்பால்) | Book of Love & Affection | 109..=133 | 1081..=1330 | 250 |

The 9 canonical Iyals:
1. `பாயிரவியல்` (Prologue, Ch 1..=4, Kurals 1..=40)
2. `இல்லறவியல்` (Domestic Virtue, Ch 5..=24, Kurals 41..=240)
3. `துறவறவியல்` (Ascetic Virtue, Ch 25..=37, Kurals 241..=370)
4. `ஊழியல்` (Fate, Ch 38..=38, Kurals 371..=380)
5. `அரசியல்` (Royalty & Polity, Ch 39..=63, Kurals 381..=630)
6. `அங்கவியல்` (Ministers & Elements of State, Ch 64..=95, Kurals 631..=950; merges `detail.json` groups 6..=10)
7. `ஒழிபியல்` (alt `குடியியல்`, Citizenship & Appendix, Ch 96..=108, Kurals 951..=1080)
8. `களவியல்` (Pre-marital Love, Ch 109..=116, Kurals 1081..=1160)
9. `கற்பியல்` (Post-marital Love, Ch 117..=133, Kurals 1161..=1330)

### 2.3 Forensic Audit Rules (Layer 1)
- Contiguous Kural numbers `1..=1330`, zero gaps, zero duplicates.
- Every string NFC-normalized via `unicode-normalization`.
- `opening_letter_ta` computed via `unicode-segmentation::UnicodeSegmentation::graphemes(line1_ta.trim(), true).next()` so compound letters (`கெ`, `கு`, `எ`, etc.) stay intact.
- Non-empty check on `line1_ta`, `line2_ta`, `transliteration1`, `transliteration2`, `meaning_mu_va`, `meaning_solomon`, `meaning_kalaignar`, `translation_en`, `explanation_en`, `couplet_en`.
- Phonetic transliteration sanity check:
  - Kural #1 `transliteration1` must start with `Akara`.
  - Kural #2 `transliteration1` must start with `Katradhanaal`.
  - Every Kural's opening Tamil base character must match its `transliteration1` opening ASCII letter (`அ/ஆ/ஐ->a`, `இ->i`, `ஈ/எ/ஏ->e`, `உ->u`, `ஊ/ஒ/ஓ->o`, `க->k`, `ச->s`, `ஞ->g`, `த->t`, `ந->n`, `ப->p`, `ம->m`, `ய->y`, `வ->v`).

---

## 3. SQLite + FTS5 Schema Contract

- Engine: SQLite via `rusqlite` (`bundled`, `modern_sqlite`) with FTS5 enabled.
- Build-time PRAGMAs: `PRAGMA journal_mode = WAL; PRAGMA foreign_keys = ON; PRAGMA synchronous = NORMAL;` followed by `VACUUM; ANALYZE; PRAGMA wal_checkpoint(TRUNCATE); PRAGMA integrity_check;`.
- Runtime read-only PRAGMAs (`KuralDb::open`): `SQLITE_OPEN_READ_ONLY`, `PRAGMA query_only = ON; PRAGMA foreign_keys = ON; PRAGMA cache_size = -16000; PRAGMA temp_store = MEMORY; PRAGMA mmap_size = 67108864;` (zero SD-card writes).

```sql
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
    game_options_ta     TEXT NOT NULL, -- JSON array[4]
    game_options_en     TEXT NOT NULL  -- JSON array[4]
);

CREATE TABLE kural_youth (
    kural_number      INTEGER PRIMARY KEY REFERENCES kurals(number) ON DELETE CASCADE,
    analysis_ta       TEXT NOT NULL,
    analysis_en       TEXT NOT NULL,
    action_tip_ta     TEXT NOT NULL,
    action_tip_en     TEXT NOT NULL,
    quiz_question_ta  TEXT NOT NULL,
    quiz_question_en  TEXT NOT NULL,
    quiz_options_ta   TEXT NOT NULL, -- JSON array[4]
    quiz_options_en   TEXT NOT NULL, -- JSON array[4]
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
    tokenize = 'unicode61 remove_diacritics 0'
);

CREATE VIRTUAL TABLE knowledge_fts USING fts5(
    kb_id UNINDEXED,
    question_ta,
    question_en,
    answer_ta,
    answer_en,
    keywords,
    tokenize = 'unicode61 remove_diacritics 0'
);
```

---

## 4. Layer 2 Enrichment Checkpoint Contract (`adhikaram_XXX.json`)

Each checkpoint file stores 10 `EnrichedKuralRecord` objects for `adhikaram_id` (`1..=133`):

```json
{
  "adhikaram_id": 1,
  "provider": "ollama",
  "model": "qwen3.5:2b",
  "generated_at": "2026-11-24T18:00:00Z",
  "kurals": [
    {
      "kural_number": 1,
      "child": {
        "simple_meaning_ta": "...",
        "simple_meaning_en": "...",
        "story_title_ta": "...",
        "story_title_en": "...",
        "kutty_story_ta": "...",
        "kutty_story_en": "...",
        "moral_ta": "...",
        "moral_en": "...",
        "game_blank_word_ta": "அகர",
        "game_options_ta": ["அகர", "மலர்", "கடல்", "நிலம்"],
        "game_options_en": ["First letter A", "Ocean", "Mountain", "Cloud"]
      },
      "youth": {
        "analysis_ta": "...",
        "analysis_en": "...",
        "action_tip_ta": "...",
        "action_tip_en": "...",
        "quiz_question_ta": "...",
        "quiz_question_en": "...",
        "quiz_options_ta": ["...", "...", "...", "..."],
        "quiz_options_en": ["...", "...", "...", "..."],
        "quiz_correct_idx": 0
      },
      "adult": {
        "reflection_ta": "...",
        "reflection_en": "...",
        "research_theme": "philosophy"
      },
      "tags": ["wisdom", "humility", "gratitude"]
    }
  ]
}
```

### Canonical 20 Tag Slugs
`leadership`, `ethics`, `friendship`, `wisdom`, `time`, `education`, `self-mastery`, `gratitude`, `truth`, `courage`, `kindness`, `discipline`, `humility`, `justice`, `family`, `speech`, `perseverance`, `wealth`, `love`, `harmony`.

### Validation Invariants
1. Exact 10 Kurals matching `((adhikaram_id - 1) * 10 + 1)..=(adhikaram_id * 10)`, zero gaps or duplicates.
2. Every Tamil and English field is non-empty and NFC-normalized.
3. `game_blank_word_ta` is a single token present verbatim in `line1_ta` or `line2_ta` (raw token or punctuation-trimmed token) and appears inside `game_options_ta` (4 distinct options).
4. `game_options_en`, `quiz_options_ta`, `quiz_options_en` each have 4 distinct non-empty items; `quiz_correct_idx` is `0..=3`.
5. `tags` contains 2..=4 unique slugs, all in the canonical 20-tag list.
6. `research_theme` is one of `virtue | philosophy | polity | love_ethics`.
7. `kids_featured` is derived deterministically in `seed_db`: `1` for Kurals `1..=1080`, `0` for `1081..=1330` (with child-safe family/kindness/harmony framing in Inbathuppal).

---

## 5. Hybrid 4-Tier Search Engine

1. **Tier 1 — Exact Kural Number**: Parses `"42"`, `"குறள் 481"`, `"kural 1330"`, `"#1"` -> direct PK lookup (`SearchMatchKind::ExactNumber`).
2. **Tier 2 — Adhikaram Match**: Parses `"அதிகாரம் 1"`, `"adhikaram 49"`, or exact/prefix match on `adhikarams.name_ta`, `name_en`, or `transliteration` -> returns all 10 Kurals in that Adhikaram (`SearchMatchKind::AdhikaramMatch`).
3. **Tier 3 — FTS5 BM25 Full-Text**: Tokenizes and double-quotes safe tokens, executes `kurals_fts MATCH ... ORDER BY bm25(kurals_fts) ASC`, optional `division_id` filter (`SearchMatchKind::FullText`).
4. **Tier 4 — `strsim` Fuzzy Fallback**: Uses `strsim::normalized_damerau_levenshtein` (`>= 0.72` threshold) over Adhikaram names, Tags, Divisions, cultural anchors (`Thiruvalluvar`/`திருவள்ளுவர்`), and indexed Kural words to recover misspelled queries (`"thiruvalvar"`, `"அறிவுடமை"`) and populate `did_you_mean` (`SearchMatchKind::FuzzyFallback`). Out-of-scope queries return empty `hits` and `did_you_mean: None`.

---

## 6. EGM Audit Sign-Off

- [x] **Design Critic**: SQLite WAL build + read-only Pi 4B mmap/cache PRAGMAs, single-transaction build with atomic rename, FTS5 `unicode61 remove_diacritics 0` preserves Tamil vowel signs cleanly.
- [x] **Implementation Readiness**: Every table, column, JSON key, tag slug, CLI flag, and validation invariant is locked with zero open decisions.
- [x] **Scope Discipline**: Zero Python anywhere; pure-Rust workspace strictly limited to the offline database layer and CLIs.

