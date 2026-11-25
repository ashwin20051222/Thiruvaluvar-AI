use std::path::PathBuf;
use thiserror::Error;

/// Errors produced by database compilation, schema setup, and typed queries.
#[derive(Error, Debug)]
pub enum DbError {
    #[error("core error: {0}")]
    Core(#[from] kural_core::CoreError),

    #[error("sqlite error: {0}")]
    Sqlite(#[from] rusqlite::Error),

    #[error("io error at {path}")]
    Io {
        path: PathBuf,
        #[source]
        source: std::io::Error,
    },

    #[error("json error at {path}")]
    Json {
        path: PathBuf,
        #[source]
        source: serde_json::Error,
    },

    #[error("strict build validation failed: {0}")]
    Validation(String),

    #[error("kural #{0} not found")]
    KuralNotFound(u16),

    #[error("invalid argument: {0}")]
    InvalidArgument(String),

    #[error("database mutex poisoned")]
    LockPoisoned,

    #[error(
        "database not found at {primary} (also checked: {checked}). Run: cargo run --bin seed_db -- --partial-ok"
    )]
    DatabaseNotFound { primary: PathBuf, checked: String },
}
