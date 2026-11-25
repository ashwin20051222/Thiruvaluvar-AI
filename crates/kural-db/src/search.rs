use crate::DbError;
use crate::embedder::TextEmbedder;
use crate::vector::VectorIndex;
use kural_core::{
    KnowledgeEntry, KuralSummary, SearchHit, SearchMatchKind, SearchOutcome, normalize_nfc_trimmed,
};
use rusqlite::{Connection, params};
use serde::{Deserialize, Serialize};
use std::collections::{HashMap, HashSet};
use strsim::normalized_damerau_levenshtein;

const FUZZY_THRESHOLD: f64 = 0.72;
const RRF_CANDIDATE_LIMIT: usize = 50;

/// Ranking and filtering parameters for RRF hybrid search.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct RrfSearchParams {
    pub division_id: Option<u8>,
    pub min_vec_cosine: f32,
    pub rrf_k: f32,
    pub limit: usize,
}

/// Combined hybrid search result carrying OOS confidence diagnostics.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct HybridRrfOutcome {
    pub outcome: SearchOutcome,
    pub max_vec_cosine: f32,
    pub max_rrf_score: f32,
    pub has_exact_or_fts_match: bool,
}

/// Ranked Knowledge Base match carrying cosine + FTS5 RRF scores.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct KnowledgeRrfResult {
    pub entry: KnowledgeEntry,
    pub vec_cosine: f32,
    pub rrf_score: f32,
    pub has_fts_match: bool,
}

#[derive(Clone)]
struct FuzzyCandidate {
    display: String,
    search_query: String,
    adhikaram_id: Option<u8>,
}

/// Parse exact Kural number references like `"42"`, `"#42"`, `"kural 42"`, `"couplet 42"`, `"குறள் 481"`.
pub fn parse_exact_kural_number(raw: &str) -> Option<u16> {
    let norm = normalize_nfc_trimmed(raw);
    if norm.is_empty() {
        return None;
    }

    let lowered = norm.to_ascii_lowercase();
    let stripped = lowered
        .strip_prefix("குறள் எண்")
        .or_else(|| lowered.strip_prefix("குறள்"))
        .or_else(|| lowered.strip_prefix("kural number"))
        .or_else(|| lowered.strip_prefix("kural no"))
        .or_else(|| lowered.strip_prefix("kural"))
        .or_else(|| lowered.strip_prefix("couplet"))
        .or_else(|| lowered.strip_prefix("thirukkural"))
        .or_else(|| lowered.strip_prefix("திருக்குறள்"))
        .unwrap_or(&lowered)
        .trim()
        .trim_start_matches(['#', ':', '-', '.'])
        .trim();

    if stripped.is_empty() || !stripped.chars().all(|c| c.is_ascii_digit()) {
        return None;
    }

    let num: u16 = stripped.parse().ok()?;
    if (1..=1330).contains(&num) {
        Some(num)
    } else {
        None
    }
}

/// Parse explicit Adhikaram number references like `"அதிகாரம் 1"`, `"adhikaram 49"`, `"chapter 109"`.
pub fn parse_adhikaram_number(raw: &str) -> Option<u8> {
    let norm = normalize_nfc_trimmed(raw);
    let lowered = norm.to_ascii_lowercase();
    let stripped = lowered
        .strip_prefix("அதிகாரம்")
        .or_else(|| lowered.strip_prefix("adhikaram"))
        .or_else(|| lowered.strip_prefix("chapter"))
        .map(str::trim)?;

    let digits = stripped.trim_start_matches(['#', ':', '-', '.']).trim();
    if digits.is_empty() || !digits.chars().all(|c| c.is_ascii_digit()) {
        return None;
    }
    let id: u8 = digits.parse().ok()?;
    if (1..=133).contains(&id) {
        Some(id)
    } else {
        None
    }
}

fn extract_search_tokens(raw: &str) -> Vec<String> {
    let norm = normalize_nfc_trimmed(raw);
    norm.split(|c: char| {
        c.is_whitespace()
            || matches!(
                c,
                '"' | '\''
                    | ','
                    | '.'
                    | ';'
                    | ':'
                    | '!'
                    | '?'
                    | '('
                    | ')'
                    | '['
                    | ']'
                    | '{'
                    | '}'
                    | '/'
                    | '\\'
                    | '*'
                    | '+'
                    | '-'
                    | '^'
            )
    })
    .filter(|s| !s.is_empty())
    .map(ToString::to_string)
    .collect()
}

fn build_fts_query(tokens: &[String], joiner: &str) -> Option<String> {
    if tokens.is_empty() {
        return None;
    }
    let parts: Vec<String> = tokens
        .iter()
        .map(|tok| {
            let escaped = tok.replace('"', "\"\"");
            if tok.chars().count() >= 3 {
                format!("\"{escaped}\"*")
            } else {
                format!("\"{escaped}\"")
            }
        })
        .collect();
    Some(parts.join(joiner))
}

pub(crate) fn row_to_kural_summary(
    row: &rusqlite::Row<'_>,
) -> Result<KuralSummary, rusqlite::Error> {
    Ok(KuralSummary {
        number: row.get(0)?,
        division_id: row.get(1)?,
        adhikaram_id: row.get(2)?,
        adhikaram_ta: row.get(3)?,
        adhikaram_en: row.get(4)?,
        line1_ta: row.get(5)?,
        line2_ta: row.get(6)?,
        transliteration1: row.get(7)?,
        transliteration2: row.get(8)?,
        translation_en: row.get(9)?,
    })
}

pub(crate) fn row_to_knowledge_entry(
    row: &rusqlite::Row<'_>,
) -> Result<KnowledgeEntry, rusqlite::Error> {
    Ok(KnowledgeEntry {
        id: row.get(0)?,
        category: row.get(1)?,
        question_ta: row.get(2)?,
        question_en: row.get(3)?,
        answer_ta: row.get(4)?,
        answer_en: row.get(5)?,
        keywords: row.get(6)?,
    })
}

pub(crate) fn fetch_kural_summary(
    conn: &Connection,
    number: u16,
) -> Result<Option<KuralSummary>, DbError> {
    let mut stmt = conn.prepare_cached(
        "SELECT k.number, k.division_id, k.adhikaram_id, a.name_ta, a.name_en,
                k.line1_ta, k.line2_ta, k.transliteration1, k.transliteration2, k.translation_en
         FROM kurals k
         JOIN adhikarams a ON a.id = k.adhikaram_id
         WHERE k.number = ?1",
    )?;
    let mut rows = stmt.query(params![number])?;
    if let Some(row) = rows.next()? {
        Ok(Some(row_to_kural_summary(row)?))
    } else {
        Ok(None)
    }
}

fn fetch_adhikaram_summaries(
    conn: &Connection,
    adhikaram_id: u8,
    division_id: Option<u8>,
    limit: usize,
) -> Result<Vec<KuralSummary>, DbError> {
    let limit_i64 = i64::try_from(limit).unwrap_or(100);
    let mut stmt = conn.prepare_cached(
        "SELECT k.number, k.division_id, k.adhikaram_id, a.name_ta, a.name_en,
                k.line1_ta, k.line2_ta, k.transliteration1, k.transliteration2, k.translation_en
         FROM kurals k
         JOIN adhikarams a ON a.id = k.adhikaram_id
         WHERE k.adhikaram_id = ?1
           AND (?2 IS NULL OR k.division_id = ?2)
         ORDER BY k.number ASC
         LIMIT ?3",
    )?;
    let rows = stmt.query_map(params![adhikaram_id, division_id, limit_i64], |r| {
        row_to_kural_summary(r)
    })?;
    let mut out = Vec::new();
    for r in rows {
        out.push(r?);
    }
    Ok(out)
}

fn find_matching_adhikaram(
    conn: &Connection,
    query: &str,
    division_id: Option<u8>,
) -> Result<Option<(u8, String)>, DbError> {
    if let Some(id) = parse_adhikaram_number(query) {
        let mut stmt =
            conn.prepare_cached("SELECT id, division_id, name_ta FROM adhikarams WHERE id = ?1")?;
        let mut rows = stmt.query(params![id])?;
        if let Some(row) = rows.next()? {
            let row_id: u8 = row.get(0)?;
            let row_div: u8 = row.get(1)?;
            let name_ta: String = row.get(2)?;
            if division_id.is_none_or(|d| d == row_div) {
                return Ok(Some((row_id, name_ta)));
            }
        }
        return Ok(None);
    }

    let q_norm = normalize_nfc_trimmed(query);
    if q_norm.chars().count() < 3 {
        return Ok(None);
    }
    let q_lower = q_norm.to_ascii_lowercase();

    let mut stmt = conn.prepare_cached(
        "SELECT id, division_id, name_ta, name_en, transliteration FROM adhikarams ORDER BY id ASC",
    )?;
    let rows = stmt.query_map([], |row| {
        Ok((
            row.get::<_, u8>(0)?,
            row.get::<_, u8>(1)?,
            row.get::<_, String>(2)?,
            row.get::<_, String>(3)?,
            row.get::<_, String>(4)?,
        ))
    })?;

    let mut prefix_match: Option<(u8, String)> = None;
    for r in rows {
        let (id, div_id, name_ta, name_en, translit) = r?;
        if division_id.is_some_and(|d| d != div_id) {
            continue;
        }
        let en_lower = name_en.to_ascii_lowercase();
        let tr_lower = translit.to_ascii_lowercase();

        if name_ta == q_norm || en_lower == q_lower || tr_lower == q_lower {
            return Ok(Some((id, name_ta)));
        }
        if prefix_match.is_none()
            && (name_ta.starts_with(&q_norm)
                || en_lower.starts_with(&q_lower)
                || tr_lower.starts_with(&q_lower))
        {
            prefix_match = Some((id, name_ta));
        }
    }

    Ok(prefix_match)
}

fn query_kurals_fts(
    conn: &Connection,
    fts_expr: &str,
    division_id: Option<u8>,
    limit: usize,
) -> Result<Vec<(KuralSummary, f64)>, DbError> {
    let limit_i64 = i64::try_from(limit).unwrap_or(50);
    let mut stmt = conn.prepare_cached(
        "SELECT k.number, k.division_id, k.adhikaram_id, a.name_ta, a.name_en,
                k.line1_ta, k.line2_ta, k.transliteration1, k.transliteration2, k.translation_en,
                -bm25(kurals_fts) AS rank_score
         FROM kurals_fts
         JOIN kurals k ON k.number = CAST(kurals_fts.kural_number AS INTEGER)
         JOIN adhikarams a ON a.id = k.adhikaram_id
         WHERE kurals_fts MATCH ?1
           AND (?2 IS NULL OR k.division_id = ?2)
         ORDER BY bm25(kurals_fts) ASC, k.number ASC
         LIMIT ?3",
    )?;
    let rows = stmt.query_map(params![fts_expr, division_id, limit_i64], |r| {
        Ok((row_to_kural_summary(r)?, r.get::<_, f64>(10)?))
    })?;
    let mut out = Vec::new();
    for r in rows {
        out.push(r?);
    }
    Ok(out)
}

fn run_fts_search(
    conn: &Connection,
    query: &str,
    division_id: Option<u8>,
    limit: usize,
) -> Result<Vec<(KuralSummary, f64)>, DbError> {
    let tokens = extract_search_tokens(query);
    if let Some(and_expr) = build_fts_query(&tokens, " AND ") {
        let hits = query_kurals_fts(conn, &and_expr, division_id, limit)?;
        if !hits.is_empty() {
            return Ok(hits);
        }
    }
    if tokens.len() > 1
        && let Some(or_expr) = build_fts_query(&tokens, " OR ")
    {
        return query_kurals_fts(conn, &or_expr, division_id, limit);
    }
    Ok(Vec::new())
}

fn collect_fuzzy_candidates(conn: &Connection) -> Result<Vec<FuzzyCandidate>, DbError> {
    let mut out = Vec::new();
    let mut seen = HashSet::new();

    let mut push_candidate = |display: String, search_query: String, adhikaram_id: Option<u8>| {
        let key = display.to_ascii_lowercase();
        if seen.insert(key) {
            out.push(FuzzyCandidate {
                display,
                search_query,
                adhikaram_id,
            });
        }
    };

    for (display, search_query, adhikaram_id) in [
        ("Thiruvalluvar", "virtue", Some(1)),
        ("Thirukkural", "virtue", Some(1)),
        ("திருவள்ளுவர்", "கடவுள் வாழ்த்து", Some(1)),
        ("திருக்குறள்", "அறம்", Some(1)),
    ] {
        push_candidate(display.to_string(), search_query.to_string(), adhikaram_id);
    }

    {
        let mut stmt = conn.prepare_cached(
            "SELECT id, name_ta, name_en, transliteration FROM adhikarams ORDER BY id ASC",
        )?;
        let rows = stmt.query_map([], |r| {
            Ok((
                r.get::<_, u8>(0)?,
                r.get::<_, String>(1)?,
                r.get::<_, String>(2)?,
                r.get::<_, String>(3)?,
            ))
        })?;
        for r in rows {
            let (id, name_ta, name_en, translit) = r?;
            push_candidate(name_ta.clone(), name_ta, Some(id));
            push_candidate(name_en.clone(), name_en, Some(id));
            push_candidate(translit.clone(), translit, Some(id));
        }
    }

    {
        let mut stmt = conn.prepare_cached("SELECT slug, name_ta, name_en FROM tags")?;
        let rows = stmt.query_map([], |r| {
            Ok((
                r.get::<_, String>(0)?,
                r.get::<_, String>(1)?,
                r.get::<_, String>(2)?,
            ))
        })?;
        for r in rows {
            let (slug, name_ta, name_en) = r?;
            push_candidate(slug.clone(), slug, None);
            push_candidate(name_ta.clone(), name_ta, None);
            push_candidate(name_en.clone(), name_en, None);
        }
    }

    {
        let mut stmt = conn.prepare_cached(
            "SELECT name_ta, alt_name_ta, name_en, transliteration FROM divisions",
        )?;
        let rows = stmt.query_map([], |r| {
            Ok((
                r.get::<_, String>(0)?,
                r.get::<_, Option<String>>(1)?,
                r.get::<_, String>(2)?,
                r.get::<_, String>(3)?,
            ))
        })?;
        for r in rows {
            let (name_ta, alt_ta, name_en, translit) = r?;
            push_candidate(name_ta.clone(), name_ta, None);
            if let Some(alt) = alt_ta {
                push_candidate(alt.clone(), alt, None);
            }
            push_candidate(name_en.clone(), name_en, None);
            push_candidate(translit.clone(), translit, None);
        }
    }

    {
        let mut stmt =
            conn.prepare_cached("SELECT line1_ta, line2_ta, translation_en FROM kurals")?;
        let rows = stmt.query_map([], |r| {
            Ok((
                r.get::<_, String>(0)?,
                r.get::<_, String>(1)?,
                r.get::<_, String>(2)?,
            ))
        })?;
        for r in rows {
            let (l1, l2, en) = r?;
            for tok in extract_search_tokens(&l1)
                .into_iter()
                .chain(extract_search_tokens(&l2))
                .chain(extract_search_tokens(&en))
            {
                if tok.chars().count() >= 4 {
                    push_candidate(tok.clone(), tok, None);
                }
            }
        }
    }

    Ok(out)
}

fn similarity_score(query: &str, candidate: &str) -> f64 {
    let q = query.to_ascii_lowercase();
    let c = candidate.to_ascii_lowercase();
    normalized_damerau_levenshtein(&q, &c)
}

fn run_fuzzy_fallback(
    conn: &Connection,
    clean_query: &str,
    division_id: Option<u8>,
    limit: usize,
) -> Result<SearchOutcome, DbError> {
    let candidates = collect_fuzzy_candidates(conn)?;
    let query_tokens = extract_search_tokens(clean_query);

    let mut best: Option<(&FuzzyCandidate, f64)> = None;
    for cand in &candidates {
        let mut score = similarity_score(clean_query, &cand.display);
        for tok in &query_tokens {
            if tok.chars().count() >= 4 {
                score = score.max(similarity_score(tok, &cand.display));
            }
        }
        if score >= FUZZY_THRESHOLD
            && best
                .as_ref()
                .is_none_or(|(_, best_score)| score > *best_score)
        {
            best = Some((cand, score));
        }
    }

    if let Some((best_cand, sim_score)) = best {
        let mut hits = Vec::new();
        if let Some(ad_id) = best_cand.adhikaram_id {
            for kural in fetch_adhikaram_summaries(conn, ad_id, division_id, limit)? {
                hits.push(SearchHit {
                    kural,
                    match_kind: SearchMatchKind::FuzzyFallback,
                    score: sim_score,
                });
            }
        }
        if hits.is_empty() {
            for (kural, bm_score) in
                run_fts_search(conn, &best_cand.search_query, division_id, limit)?
            {
                hits.push(SearchHit {
                    kural,
                    match_kind: SearchMatchKind::FuzzyFallback,
                    score: (sim_score + bm_score).max(sim_score),
                });
            }
        }
        if !hits.is_empty() {
            return Ok(SearchOutcome {
                query: clean_query.to_string(),
                did_you_mean: Some(best_cand.display.clone()),
                hits,
            });
        }
    }

    Ok(SearchOutcome {
        query: clean_query.to_string(),
        did_you_mean: None,
        hits: Vec::new(),
    })
}

fn try_exact_or_adhikaram_match(
    conn: &Connection,
    clean_query: &str,
    division_id: Option<u8>,
    limit: usize,
) -> Result<Option<SearchOutcome>, DbError> {
    if let Some(num) = parse_exact_kural_number(clean_query)
        && let Some(summary) = fetch_kural_summary(conn, num)?
        && division_id.is_none_or(|d| d == summary.division_id)
    {
        return Ok(Some(SearchOutcome {
            query: clean_query.to_string(),
            did_you_mean: None,
            hits: vec![SearchHit {
                kural: summary,
                match_kind: SearchMatchKind::ExactNumber,
                score: 100.0,
            }],
        }));
    }

    if let Some((adhikaram_id, _)) = find_matching_adhikaram(conn, clean_query, division_id)? {
        let summaries = fetch_adhikaram_summaries(conn, adhikaram_id, division_id, limit)?;
        if !summaries.is_empty() {
            let hits = summaries
                .into_iter()
                .map(|kural| SearchHit {
                    kural,
                    match_kind: SearchMatchKind::AdhikaramMatch,
                    score: 90.0,
                })
                .collect();
            return Ok(Some(SearchOutcome {
                query: clean_query.to_string(),
                did_you_mean: None,
                hits,
            }));
        }
    }

    Ok(None)
}

/// Execute the 4-tier hybrid search against `conn` (FTS5 + fuzzy).
pub fn execute_hybrid_search(
    conn: &Connection,
    query: &str,
    division_id: Option<u8>,
    limit: usize,
) -> Result<SearchOutcome, DbError> {
    let clean_query = normalize_nfc_trimmed(query);
    if clean_query.is_empty() || limit == 0 {
        return Ok(SearchOutcome {
            query: clean_query,
            did_you_mean: None,
            hits: Vec::new(),
        });
    }

    if let Some(outcome) = try_exact_or_adhikaram_match(conn, &clean_query, division_id, limit)? {
        return Ok(outcome);
    }

    let fts_hits = run_fts_search(conn, &clean_query, division_id, limit)?;
    if !fts_hits.is_empty() {
        let hits = fts_hits
            .into_iter()
            .map(|(kural, score)| SearchHit {
                kural,
                match_kind: SearchMatchKind::FullText,
                score,
            })
            .collect();
        return Ok(SearchOutcome {
            query: clean_query,
            did_you_mean: None,
            hits,
        });
    }

    run_fuzzy_fallback(conn, &clean_query, division_id, limit)
}

/// Execute cosine-filtered Reciprocal Rank Fusion combining FTS5 BM25 + INT8 embeddings.
pub fn execute_hybrid_rrf_search(
    conn: &Connection,
    vectors: Option<&VectorIndex>,
    embedder: Option<&dyn TextEmbedder>,
    query: &str,
    opts: RrfSearchParams,
) -> Result<HybridRrfOutcome, DbError> {
    execute_hybrid_rrf_inner(conn, vectors, embedder, query, opts, true)
}

/// Execute cosine-filtered RRF search without the O(N) fuzzy fallback scan when FTS + vector hits are empty.
pub fn execute_hybrid_rrf_search_no_fuzzy(
    conn: &Connection,
    vectors: Option<&VectorIndex>,
    embedder: Option<&dyn TextEmbedder>,
    query: &str,
    opts: RrfSearchParams,
) -> Result<HybridRrfOutcome, DbError> {
    execute_hybrid_rrf_inner(conn, vectors, embedder, query, opts, false)
}

fn execute_hybrid_rrf_inner(
    conn: &Connection,
    vectors: Option<&VectorIndex>,
    embedder: Option<&dyn TextEmbedder>,
    query: &str,
    opts: RrfSearchParams,
    allow_fuzzy_fallback: bool,
) -> Result<HybridRrfOutcome, DbError> {
    let division_id = opts.division_id;
    let min_vec_cosine = opts.min_vec_cosine;
    let rrf_k = opts.rrf_k;
    let limit = opts.limit;
    let clean_query = normalize_nfc_trimmed(query);
    if clean_query.is_empty() || limit == 0 {
        return Ok(HybridRrfOutcome {
            outcome: SearchOutcome {
                query: clean_query,
                did_you_mean: None,
                hits: Vec::new(),
            },
            max_vec_cosine: 0.0,
            max_rrf_score: 0.0,
            has_exact_or_fts_match: false,
        });
    }

    if let Some(outcome) = try_exact_or_adhikaram_match(conn, &clean_query, division_id, limit)? {
        return Ok(HybridRrfOutcome {
            outcome,
            max_vec_cosine: 1.0,
            max_rrf_score: 1.0,
            has_exact_or_fts_match: true,
        });
    }

    let k = if rrf_k > 0.0 { rrf_k } else { 60.0 };
    let fts_hits = run_fts_search(conn, &clean_query, division_id, RRF_CANDIDATE_LIMIT)?;
    let has_fts = !fts_hits.is_empty();

    let (vec_hits, max_vec_cosine) = match (vectors, embedder) {
        (Some(vi), Some(emb)) => {
            let (q_vec, q_norm) = emb.embed_i8(&clean_query)?;
            vi.search_kurals(
                &q_vec,
                q_norm,
                division_id,
                None,
                min_vec_cosine,
                RRF_CANDIDATE_LIMIT,
            )
        }
        _ => (Vec::new(), 0.0),
    };

    if fts_hits.is_empty() && vec_hits.is_empty() {
        let outcome = if allow_fuzzy_fallback {
            run_fuzzy_fallback(conn, &clean_query, division_id, limit)?
        } else {
            SearchOutcome {
                query: clean_query,
                did_you_mean: None,
                hits: Vec::new(),
            }
        };
        return Ok(HybridRrfOutcome {
            outcome,
            max_vec_cosine,
            max_rrf_score: 0.0,
            has_exact_or_fts_match: false,
        });
    }

    let mut summary_by_num: HashMap<u16, KuralSummary> = HashMap::new();
    let mut rrf_scores: HashMap<u16, f32> = HashMap::new();

    for (idx, (summary, _bm25)) in fts_hits.into_iter().enumerate() {
        let rank = (idx + 1) as f32;
        *rrf_scores.entry(summary.number).or_insert(0.0) += 1.0 / (k + rank);
        summary_by_num.insert(summary.number, summary);
    }

    for (idx, (num, _cos)) in vec_hits.into_iter().enumerate() {
        let rank = (idx + 1) as f32;
        *rrf_scores.entry(num).or_insert(0.0) += 1.0 / (k + rank);
        if let std::collections::hash_map::Entry::Vacant(e) = summary_by_num.entry(num)
            && let Some(summary) = fetch_kural_summary(conn, num)?
        {
            e.insert(summary);
        }
    }

    let mut ranked: Vec<(u16, f32)> = rrf_scores.into_iter().collect();
    ranked.sort_by(|a, b| b.1.total_cmp(&a.1).then_with(|| a.0.cmp(&b.0)));

    let max_rrf_score = ranked.first().map(|&(_, s)| s).unwrap_or(0.0);
    let hits: Vec<SearchHit> = ranked
        .into_iter()
        .take(limit)
        .filter_map(|(num, rrf)| {
            summary_by_num.remove(&num).map(|kural| SearchHit {
                kural,
                match_kind: SearchMatchKind::FullText,
                score: f64::from(rrf),
            })
        })
        .collect();

    Ok(HybridRrfOutcome {
        outcome: SearchOutcome {
            query: clean_query,
            did_you_mean: None,
            hits,
        },
        max_vec_cosine,
        max_rrf_score,
        has_exact_or_fts_match: has_fts,
    })
}

fn run_knowledge_fts(
    conn: &Connection,
    clean_query: &str,
    limit: usize,
) -> Result<Vec<KnowledgeEntry>, DbError> {
    let limit_i64 = i64::try_from(limit).unwrap_or(50);
    let tokens = extract_search_tokens(clean_query);

    let run_kb_fts = |expr: &str| -> Result<Vec<KnowledgeEntry>, DbError> {
        let mut stmt = conn.prepare_cached(
            "SELECT kb.id, kb.category, kb.question_ta, kb.question_en, kb.answer_ta, kb.answer_en, kb.keywords
             FROM knowledge_fts
             JOIN knowledge_base kb ON kb.id = CAST(knowledge_fts.kb_id AS INTEGER)
             WHERE knowledge_fts MATCH ?1
             ORDER BY bm25(knowledge_fts) ASC, kb.id ASC
             LIMIT ?2",
        )?;
        let rows = stmt.query_map(params![expr, limit_i64], row_to_knowledge_entry)?;
        let mut out = Vec::new();
        for r in rows {
            out.push(r?);
        }
        Ok(out)
    };

    if let Some(and_expr) = build_fts_query(&tokens, " AND ") {
        let hits = run_kb_fts(&and_expr)?;
        if !hits.is_empty() {
            return Ok(hits);
        }
    }
    if tokens.len() > 1
        && let Some(or_expr) = build_fts_query(&tokens, " OR ")
    {
        return run_kb_fts(&or_expr);
    }
    Ok(Vec::new())
}

/// Search the curated offline `knowledge_base` via FTS5 with fuzzy fallback.
pub fn execute_knowledge_search(
    conn: &Connection,
    query: &str,
    limit: usize,
) -> Result<Vec<KnowledgeEntry>, DbError> {
    let clean_query = normalize_nfc_trimmed(query);
    if clean_query.is_empty() || limit == 0 {
        return Ok(Vec::new());
    }

    let fts_hits = run_knowledge_fts(conn, &clean_query, limit)?;
    if !fts_hits.is_empty() {
        return Ok(fts_hits);
    }

    let tokens = extract_search_tokens(&clean_query);
    let mut stmt = conn.prepare_cached(
        "SELECT id, category, question_ta, question_en, answer_ta, answer_en, keywords FROM knowledge_base",
    )?;
    let rows = stmt.query_map([], row_to_knowledge_entry)?;

    let mut scored: Vec<(f64, KnowledgeEntry)> = Vec::new();
    for r in rows {
        let entry = r?;
        let mut max_sim = 0.0_f64;
        let haystack = format!(
            "{} {} {}",
            entry.question_ta, entry.question_en, entry.keywords
        );
        for word in extract_search_tokens(&haystack) {
            if word.chars().count() < 4 {
                continue;
            }
            max_sim = max_sim.max(similarity_score(&clean_query, &word));
            for tok in &tokens {
                if tok.chars().count() >= 4 {
                    max_sim = max_sim.max(similarity_score(tok, &word));
                }
            }
        }
        if max_sim >= FUZZY_THRESHOLD {
            scored.push((max_sim, entry));
        }
    }
    scored.sort_by(|a, b| b.0.total_cmp(&a.0).then_with(|| a.1.id.cmp(&b.1.id)));
    Ok(scored.into_iter().take(limit).map(|(_, e)| e).collect())
}

/// Cosine-filtered Reciprocal Rank Fusion search over `knowledge_base`.
pub fn execute_knowledge_rrf_search(
    conn: &Connection,
    vectors: Option<&VectorIndex>,
    embedder: Option<&dyn TextEmbedder>,
    query: &str,
    min_vec_cosine: f32,
    rrf_k: f32,
    limit: usize,
) -> Result<Vec<KnowledgeRrfResult>, DbError> {
    let clean_query = normalize_nfc_trimmed(query);
    if clean_query.is_empty() || limit == 0 {
        return Ok(Vec::new());
    }
    let k = if rrf_k > 0.0 { rrf_k } else { 60.0 };

    let fts_hits = run_knowledge_fts(conn, &clean_query, RRF_CANDIDATE_LIMIT)?;
    let (vec_hits, _) = match (vectors, embedder) {
        (Some(vi), Some(emb)) => {
            let (q_vec, q_norm) = emb.embed_i8(&clean_query)?;
            vi.search_knowledge(&q_vec, q_norm, min_vec_cosine, RRF_CANDIDATE_LIMIT)
        }
        _ => (Vec::new(), 0.0),
    };

    if fts_hits.is_empty() && vec_hits.is_empty() {
        return Ok(Vec::new());
    }

    let mut by_id: HashMap<i64, KnowledgeEntry> = HashMap::new();
    let mut rrf_scores: HashMap<i64, f32> = HashMap::new();
    let mut cos_by_id: HashMap<i64, f32> = HashMap::new();
    let mut fts_ids: HashSet<i64> = HashSet::new();

    for (idx, entry) in fts_hits.into_iter().enumerate() {
        let rank = (idx + 1) as f32;
        fts_ids.insert(entry.id);
        *rrf_scores.entry(entry.id).or_insert(0.0) += 1.0 / (k + rank);
        by_id.insert(entry.id, entry);
    }

    for (idx, (id, cos)) in vec_hits.into_iter().enumerate() {
        let rank = (idx + 1) as f32;
        *rrf_scores.entry(id).or_insert(0.0) += 1.0 / (k + rank);
        cos_by_id.insert(id, cos);
        if let std::collections::hash_map::Entry::Vacant(e) = by_id.entry(id) {
            let mut stmt = conn.prepare_cached(
                "SELECT id, category, question_ta, question_en, answer_ta, answer_en, keywords
                 FROM knowledge_base WHERE id = ?1",
            )?;
            let mut rows = stmt.query(params![id])?;
            if let Some(r) = rows.next()? {
                e.insert(row_to_knowledge_entry(r)?);
            }
        }
    }

    let mut ranked: Vec<(i64, f32)> = rrf_scores.into_iter().collect();
    ranked.sort_by(|a, b| b.1.total_cmp(&a.1).then_with(|| a.0.cmp(&b.0)));

    Ok(ranked
        .into_iter()
        .take(limit)
        .filter_map(|(id, rrf_score)| {
            by_id.remove(&id).map(|entry| KnowledgeRrfResult {
                entry,
                vec_cosine: cos_by_id.get(&id).copied().unwrap_or(0.0),
                rrf_score,
                has_fts_match: fts_ids.contains(&id),
            })
        })
        .collect())
}
