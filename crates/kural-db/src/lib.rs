//! Offline SQLite + FTS5 + INT8 semantic vector engine and mutable `UserStateStore`.

mod builder;
mod embedder;
mod error;
mod query;
mod schema;
mod search;
mod user_state;
mod vector;

pub use builder::{compile_database, load_enriched_dir, load_knowledge_base};
#[cfg(any(test, feature = "test-fallback"))]
pub use embedder::TestHashEmbedder;
pub use embedder::{
    Model2VecEmbedder, TextEmbedder, bytes_to_i8_vec, compute_embeddings_model_hash, cosine_i8,
    i8_slice_to_bytes, index_database_embeddings, index_database_embeddings_with_hash,
    knowledge_embedding_text, kural_embedding_text, quantize_l2_i8,
};
pub use error::DbError;
pub use query::{DB_PATH_ENV, KuralDb, resolve_db_path};
pub use schema::SCHEMA_SQL;
pub use search::{
    HybridRrfOutcome, KnowledgeRrfResult, RrfSearchParams, execute_hybrid_rrf_search,
    execute_hybrid_rrf_search_no_fuzzy, execute_hybrid_search, execute_knowledge_rrf_search,
    execute_knowledge_search,
};
pub use user_state::{USER_STATE_SCHEMA_VERSION, UserStateStore};
pub use vector::VectorIndex;
