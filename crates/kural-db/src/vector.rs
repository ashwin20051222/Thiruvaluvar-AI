use crate::DbError;
use crate::embedder::{bytes_to_i8_vec, cosine_i8};
use rusqlite::{Connection, OptionalExtension};
use std::collections::HashSet;

/// In-memory flat INT8 vector matrix loaded once on startup for sub-2 ms cosine scoring.
#[derive(Debug, Clone)]
pub struct VectorIndex {
    dim: usize,
    kural_numbers: Vec<u16>,
    kural_division_ids: Vec<u8>,
    kural_vecs: Vec<i8>,
    kural_norms: Vec<f32>,
    kb_ids: Vec<i64>,
    kb_vecs: Vec<i8>,
    kb_norms: Vec<f32>,
}

impl VectorIndex {
    /// Load quantized INT8 vectors from `conn`. Returns `Ok(None)` cleanly if embedding tables
    /// are missing or empty so databases built prior to `index_embeddings` run FTS5-only mode.
    pub fn try_load(conn: &Connection) -> Result<Option<Self>, DbError> {
        let has_tables: Option<i64> = conn
            .query_row(
                "SELECT 1 FROM sqlite_master WHERE type = 'table' AND name = 'kural_embeddings'",
                [],
                |r| r.get(0),
            )
            .optional()?;
        if has_tables.is_none() {
            return Ok(None);
        }

        let mut dim: usize = 0;
        let mut kural_numbers = Vec::with_capacity(1330);
        let mut kural_division_ids = Vec::with_capacity(1330);
        let mut kural_vecs = Vec::new();
        let mut kural_norms = Vec::with_capacity(1330);

        {
            let mut stmt = conn.prepare(
                "SELECT e.kural_number, k.division_id, e.vec_i8, e.norm
                 FROM kural_embeddings e
                 JOIN kurals k ON k.number = e.kural_number
                 ORDER BY e.kural_number ASC",
            )?;
            let rows = stmt.query_map([], |r| {
                Ok((
                    r.get::<_, u16>(0)?,
                    r.get::<_, u8>(1)?,
                    r.get::<_, Vec<u8>>(2)?,
                    r.get::<_, f64>(3)? as f32,
                ))
            })?;
            for row in rows {
                let (num, div_id, blob, norm) = row?;
                let i8_vec = bytes_to_i8_vec(&blob);
                if dim == 0 {
                    dim = i8_vec.len();
                    kural_vecs.reserve(dim * 1330);
                } else if i8_vec.len() != dim {
                    return Err(DbError::Validation(format!(
                        "inconsistent kural_embeddings dim at #{num}: expected {dim}, got {}",
                        i8_vec.len()
                    )));
                }
                kural_numbers.push(num);
                kural_division_ids.push(div_id);
                kural_vecs.extend_from_slice(&i8_vec);
                kural_norms.push(norm);
            }
        }

        if kural_numbers.is_empty() || dim == 0 {
            return Ok(None);
        }

        let has_kb_table: Option<i64> = conn
            .query_row(
                "SELECT 1 FROM sqlite_master WHERE type = 'table' AND name = 'knowledge_embeddings'",
                [],
                |r| r.get(0),
            )
            .optional()?;

        let mut kb_ids = Vec::with_capacity(64);
        let mut kb_vecs = Vec::new();
        let mut kb_norms = Vec::with_capacity(64);

        if has_kb_table.is_some() {
            let mut stmt = conn.prepare(
                "SELECT kb_id, vec_i8, norm FROM knowledge_embeddings ORDER BY kb_id ASC",
            )?;
            let rows = stmt.query_map([], |r| {
                Ok((
                    r.get::<_, i64>(0)?,
                    r.get::<_, Vec<u8>>(1)?,
                    r.get::<_, f64>(2)? as f32,
                ))
            })?;
            for row in rows {
                let (kb_id, blob, norm) = row?;
                let i8_vec = bytes_to_i8_vec(&blob);
                if i8_vec.len() != dim {
                    return Err(DbError::Validation(format!(
                        "inconsistent knowledge_embeddings dim at kb_id={kb_id}: expected {dim}, got {}",
                        i8_vec.len()
                    )));
                }
                kb_ids.push(kb_id);
                kb_vecs.extend_from_slice(&i8_vec);
                kb_norms.push(norm);
            }
        }

        Ok(Some(Self {
            dim,
            kural_numbers,
            kural_division_ids,
            kural_vecs,
            kural_norms,
            kb_ids,
            kb_vecs,
            kb_norms,
        }))
    }

    pub fn dim(&self) -> usize {
        self.dim
    }

    pub fn kural_count(&self) -> usize {
        self.kural_numbers.len()
    }

    pub fn kb_count(&self) -> usize {
        self.kb_ids.len()
    }

    /// Score all indexed Kurals, returning up to `limit` hits strictly above `min_cosine`
    /// plus the overall maximum cosine observed (for OOS confidence diagnostics).
    pub fn search_kurals(
        &self,
        q_vec: &[i8],
        q_norm: f32,
        division_id: Option<u8>,
        allowed_numbers: Option<&HashSet<u16>>,
        min_cosine: f32,
        limit: usize,
    ) -> (Vec<(u16, f32)>, f32) {
        if q_vec.len() != self.dim || q_norm <= 0.0 || limit == 0 {
            return (Vec::new(), 0.0);
        }

        let mut max_cosine = f32::NEG_INFINITY;
        let mut scored: Vec<(u16, f32)> = Vec::new();
        for (idx, (&kural_num, &div_id)) in self
            .kural_numbers
            .iter()
            .zip(self.kural_division_ids.iter())
            .enumerate()
        {
            if division_id.is_some_and(|d| d != div_id) {
                continue;
            }
            if allowed_numbers.is_some_and(|set| !set.contains(&kural_num)) {
                continue;
            }
            let start = idx * self.dim;
            let sim = cosine_i8(
                q_vec,
                q_norm,
                &self.kural_vecs[start..start + self.dim],
                self.kural_norms[idx],
            );
            if sim > max_cosine {
                max_cosine = sim;
            }
            if sim >= min_cosine {
                scored.push((kural_num, sim));
            }
        }

        if max_cosine == f32::NEG_INFINITY {
            max_cosine = 0.0;
        }
        scored.sort_by(|a, b| b.1.total_cmp(&a.1).then_with(|| a.0.cmp(&b.0)));
        scored.truncate(limit);
        (scored, max_cosine)
    }

    /// Score all indexed Knowledge Base items, returning up to `limit` hits strictly above `min_cosine`.
    pub fn search_knowledge(
        &self,
        q_vec: &[i8],
        q_norm: f32,
        min_cosine: f32,
        limit: usize,
    ) -> (Vec<(i64, f32)>, f32) {
        if q_vec.len() != self.dim || q_norm <= 0.0 || limit == 0 || self.kb_ids.is_empty() {
            return (Vec::new(), 0.0);
        }

        let mut max_cosine = f32::NEG_INFINITY;
        let mut scored: Vec<(i64, f32)> = Vec::new();
        for (idx, &kb_id) in self.kb_ids.iter().enumerate() {
            let start = idx * self.dim;
            let sim = cosine_i8(
                q_vec,
                q_norm,
                &self.kb_vecs[start..start + self.dim],
                self.kb_norms[idx],
            );
            if sim > max_cosine {
                max_cosine = sim;
            }
            if sim >= min_cosine {
                scored.push((kb_id, sim));
            }
        }

        if max_cosine == f32::NEG_INFINITY {
            max_cosine = 0.0;
        }
        scored.sort_by(|a, b| b.1.total_cmp(&a.1).then_with(|| a.0.cmp(&b.0)));
        scored.truncate(limit);
        (scored, max_cosine)
    }
}
