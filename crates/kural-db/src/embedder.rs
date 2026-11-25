use crate::DbError;
use kural_core::{FullKural, KnowledgeEntry};
use model2vec_rs::model::StaticModel;
use rusqlite::{Connection, params};
use std::path::Path;

/// Quantize a float embedding vector into signed `i8` plus precomputed Euclidean L2 norm.
pub fn quantize_l2_i8(vec: &[f32]) -> (Vec<i8>, f32) {
    if vec.is_empty() {
        return (Vec::new(), 0.0);
    }
    let max_abs = vec.iter().copied().map(f32::abs).fold(0.0_f32, f32::max);
    if max_abs == 0.0 || !max_abs.is_finite() {
        return (vec![0i8; vec.len()], 0.0);
    }
    let scale = max_abs / 127.0;
    let mut out = Vec::with_capacity(vec.len());
    let mut sq_sum = 0.0_f32;
    for &x in vec {
        let q = (x / scale).round().clamp(-127.0, 127.0) as i8;
        let qf = f32::from(q);
        sq_sum += qf * qf;
        out.push(q);
    }
    (out, sq_sum.sqrt())
}

/// Compute cosine similarity between two quantized `i8` vectors using safe `i32` accumulation.
pub fn cosine_i8(q: &[i8], q_norm: f32, r: &[i8], r_norm: f32) -> f32 {
    if q.len() != r.len()
        || q.is_empty()
        || !q_norm.is_finite()
        || !r_norm.is_finite()
        || q_norm <= 0.0
        || r_norm <= 0.0
    {
        return 0.0;
    }
    let dot: i32 = q
        .iter()
        .zip(r.iter())
        .map(|(&a, &b)| i32::from(a) * i32::from(b))
        .sum();
    (dot as f32) / (q_norm * r_norm)
}

/// Pack signed `i8` vector elements cleanly as `u8` bytes for SQLite BLOB storage.
pub fn i8_slice_to_bytes(vec: &[i8]) -> Vec<u8> {
    vec.iter().map(|&v| v as u8).collect()
}

/// Unpack raw SQLite BLOB bytes back into signed `i8` vector elements.
pub fn bytes_to_i8_vec(bytes: &[u8]) -> Vec<i8> {
    bytes.iter().map(|&b| b as i8).collect()
}

/// Text-to-embedding encoder abstraction shared between offline indexing and runtime query scoring.
pub trait TextEmbedder: Send + Sync {
    fn dim(&self) -> usize;
    fn embed_f32(&self, text: &str) -> Result<Vec<f32>, DbError>;

    fn embed_i8(&self, text: &str) -> Result<(Vec<i8>, f32), DbError> {
        let f = self.embed_f32(text)?;
        Ok(quantize_l2_i8(&f))
    }
}

/// Offline local `model2vec-rs` encoder (`potion-multilingual-128M-int8` from disk only).
pub struct Model2VecEmbedder {
    model: StaticModel,
    dim: usize,
}

impl Model2VecEmbedder {
    /// Load quantized `StaticModel` from a local model directory without network calls.
    pub fn load(model_dir: &Path) -> Result<Self, DbError> {
        let (embedder, _) = Self::load_with_rss_check(model_dir)?;
        Ok(embedder)
    }

    /// Load `StaticModel` and return optional process RSS delta in MiB measured via `/proc/self/statm`.
    pub fn load_with_rss_check(model_dir: &Path) -> Result<(Self, Option<u64>), DbError> {
        if !model_dir.is_dir() {
            return Err(DbError::Io {
                path: model_dir.to_path_buf(),
                source: std::io::Error::new(
                    std::io::ErrorKind::NotFound,
                    "embeddings model directory not found",
                ),
            });
        }
        let rss_before = read_process_rss_bytes();
        let raw_model =
            StaticModel::from_pretrained(model_dir, None, Some(true), None).map_err(|e| {
                DbError::Validation(format!(
                    "failed loading model2vec from {}: {e}",
                    model_dir.display()
                ))
            })?;
        let model = raw_model.quantize_int8();
        let rss_after = read_process_rss_bytes();
        let delta_mib = match (rss_before, rss_after) {
            (Some(b), Some(a)) if a >= b => Some((a - b) / (1024 * 1024)),
            (Some(_), Some(_)) => Some(0),
            _ => None,
        };
        let dim = model.dim();
        Ok((Self { model, dim }, delta_mib))
    }
}

impl TextEmbedder for Model2VecEmbedder {
    fn dim(&self) -> usize {
        self.dim
    }

    fn embed_f32(&self, text: &str) -> Result<Vec<f32>, DbError> {
        Ok(self.model.encode_single(text))
    }
}

fn read_process_rss_bytes() -> Option<u64> {
    let raw = std::fs::read_to_string("/proc/self/statm").ok()?;
    let rss_pages: u64 = raw.split_whitespace().nth(1)?.parse().ok()?;
    Some(rss_pages.saturating_mul(4096))
}

/// Deterministic offline character 3-gram & word projection embedder strictly for tests and
/// `test-fallback` CI verification when downloaded `potion-multilingual-128M-int8` weights are absent.
#[cfg(any(test, feature = "test-fallback"))]
#[derive(Debug, Clone, Default)]
pub struct TestHashEmbedder;

#[cfg(any(test, feature = "test-fallback"))]
impl TextEmbedder for TestHashEmbedder {
    fn dim(&self) -> usize {
        256
    }

    fn embed_f32(&self, text: &str) -> Result<Vec<f32>, DbError> {
        let mut vec = vec![0.0_f32; 256];
        let norm = kural_core::normalize_nfc_trimmed(text).to_ascii_lowercase();
        let chars: Vec<char> = norm.chars().collect();
        if chars.is_empty() {
            return Ok(vec);
        }

        let mut hash_add = |bytes: &[u8], weight: f32| {
            let mut h: u64 = 0xcbf2_9ce4_8422_2325;
            for &b in bytes {
                h ^= u64::from(b);
                h = h.wrapping_mul(0x0100_0000_01b3);
            }
            let idx = (h as usize) % 256;
            let sign = if ((h >> 17) & 1) == 0 { 1.0 } else { -1.0 };
            vec[idx] += sign * weight;
        };

        for tok in norm.split_whitespace() {
            hash_add(tok.as_bytes(), 2.0);
        }
        for win in chars.windows(3) {
            let mut buf = [0u8; 16];
            let mut len = 0usize;
            for &c in win {
                let mut tmp = [0u8; 4];
                let s = c.encode_utf8(&mut tmp).as_bytes();
                if len + s.len() <= buf.len() {
                    buf[len..len + s.len()].copy_from_slice(s);
                    len += s.len();
                }
            }
            hash_add(&buf[..len], 1.0);
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

/// Canonical bilingual string indexed into `kural_embeddings`.
pub fn kural_embedding_text(full: &FullKural) -> String {
    let tag_names: String = full
        .tags
        .iter()
        .map(|t| format!("{} {} {}", t.slug, t.name_ta, t.name_en))
        .collect::<Vec<_>>()
        .join(" ");
    format!(
        "{} {} {} {} {} {}",
        full.adhikaram.name_ta,
        full.adhikaram.name_en,
        full.kural.meaning_mu_va,
        full.kural.translation_en,
        full.kural.explanation_en,
        tag_names
    )
}

/// Canonical bilingual string indexed into `knowledge_embeddings` (includes `answer_ta`).
pub fn knowledge_embedding_text(entry: &KnowledgeEntry) -> String {
    format!(
        "{} {} {} {} {}",
        entry.question_ta, entry.question_en, entry.answer_ta, entry.answer_en, entry.keywords
    )
}

/// Populate `kural_embeddings` (`1,330` rows) and `knowledge_embeddings` (`>= 40` rows) in a single
/// atomic transaction inside an existing `thirukkural.db`, leaving it compacted in `DELETE` journal mode.
pub fn index_database_embeddings(
    db_path: &Path,
    embedder: &dyn TextEmbedder,
    mode_label: &str,
) -> Result<(usize, usize), DbError> {
    if !db_path.is_file() {
        return Err(DbError::DatabaseNotFound {
            primary: db_path.to_path_buf(),
            checked: "explicit db path".to_string(),
        });
    }

    let kural_db = crate::KuralDb::open(db_path)?;
    let mut kural_rows: Vec<(u16, Vec<u8>, f32)> = Vec::with_capacity(1330);
    for num in 1u16..=1330u16 {
        let full = kural_db.get_kural_full(num)?;
        let text = kural_embedding_text(&full);
        let (vec_i8, norm) = embedder.embed_i8(&text)?;
        kural_rows.push((num, i8_slice_to_bytes(&vec_i8), norm));
    }

    let kb_entries = kural_db.list_all_knowledge()?;
    let mut kb_rows: Vec<(i64, Vec<u8>, f32)> = Vec::with_capacity(kb_entries.len());
    for entry in &kb_entries {
        let text = knowledge_embedding_text(entry);
        let (vec_i8, norm) = embedder.embed_i8(&text)?;
        kb_rows.push((entry.id, i8_slice_to_bytes(&vec_i8), norm));
    }
    drop(kural_db);

    let mut conn = Connection::open(db_path)?;
    conn.execute_batch(
        "PRAGMA journal_mode = WAL;
         PRAGMA synchronous = NORMAL;
         PRAGMA foreign_keys = ON;",
    )?;
    conn.execute_batch(
        "CREATE TABLE IF NOT EXISTS kural_embeddings (
             kural_number INTEGER PRIMARY KEY REFERENCES kurals(number) ON DELETE CASCADE,
             vec_i8       BLOB NOT NULL,
             norm         REAL NOT NULL
         );
         CREATE TABLE IF NOT EXISTS knowledge_embeddings (
             kb_id        INTEGER PRIMARY KEY REFERENCES knowledge_base(id) ON DELETE CASCADE,
             vec_i8       BLOB NOT NULL,
             norm         REAL NOT NULL
         );",
    )?;

    let tx = conn.transaction()?;
    tx.execute("DELETE FROM kural_embeddings", [])?;
    tx.execute("DELETE FROM knowledge_embeddings", [])?;
    {
        let mut ins_k = tx.prepare(
            "INSERT INTO kural_embeddings (kural_number, vec_i8, norm) VALUES (?1, ?2, ?3)",
        )?;
        for (num, blob, norm) in &kural_rows {
            ins_k.execute(params![num, blob, norm])?;
        }
    }
    {
        let mut ins_kb = tx.prepare(
            "INSERT INTO knowledge_embeddings (kb_id, vec_i8, norm) VALUES (?1, ?2, ?3)",
        )?;
        for (id, blob, norm) in &kb_rows {
            ins_kb.execute(params![id, blob, norm])?;
        }
    }
    tx.execute(
        "INSERT OR REPLACE INTO db_meta (key, value) VALUES ('embeddings_mode', ?1)",
        params![mode_label],
    )?;
    tx.execute(
        "INSERT OR REPLACE INTO db_meta (key, value) VALUES ('embeddings_dim', ?1)",
        params![embedder.dim().to_string()],
    )?;
    tx.commit()?;

    conn.execute_batch(
        "PRAGMA wal_checkpoint(TRUNCATE);
         PRAGMA journal_mode = DELETE;
         VACUUM;
         ANALYZE;",
    )?;
    let integrity: String = conn.query_row("PRAGMA integrity_check;", [], |r| r.get(0))?;
    if integrity != "ok" {
        return Err(DbError::Validation(format!(
            "sqlite integrity_check failed after indexing embeddings: {integrity}"
        )));
    }

    Ok((kural_rows.len(), kb_rows.len()))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn quantize_and_cosine_handle_zero_and_matching_vectors() {
        let (z_vec, z_norm) = quantize_l2_i8(&[0.0, 0.0, 0.0]);
        assert_eq!(z_norm, 0.0);
        assert_eq!(cosine_i8(&z_vec, z_norm, &z_vec, z_norm), 0.0);

        let a = [1.0_f32, 0.5, -0.25, 0.75];
        let b = [1.0_f32, 0.5, -0.25, 0.75];
        let (qa, na) = quantize_l2_i8(&a);
        let (qb, nb) = quantize_l2_i8(&b);
        let sim = cosine_i8(&qa, na, &qb, nb);
        assert!(sim > 0.999, "expected ~1.0, got {sim}");

        let (qc, nc) = quantize_l2_i8(&[-1.0_f32, -0.5, 0.25, -0.75]);
        let neg_sim = cosine_i8(&qa, na, &qc, nc);
        assert!(neg_sim < -0.999, "expected ~-1.0, got {neg_sim}");
    }
}
