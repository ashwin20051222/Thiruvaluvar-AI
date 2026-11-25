use std::path::PathBuf;
use thiserror::Error;

/// Errors produced by canonical loading, configuration loading, Unicode/text validation, and contract checks.
#[derive(Error, Debug)]
pub enum CoreError {
    #[error("failed to read file {path}")]
    Io {
        path: PathBuf,
        #[source]
        source: std::io::Error,
    },

    #[error("failed to parse json file {path}")]
    Json {
        path: PathBuf,
        #[source]
        source: serde_json::Error,
    },

    #[error("failed to parse config file {path}")]
    ConfigParse {
        path: PathBuf,
        #[source]
        source: toml::de::Error,
    },

    #[error("invalid configuration: {0}")]
    ConfigValidation(String),

    #[error("canonical audit failed: {0}")]
    CanonicalAudit(String),

    #[error("invalid research theme '{0}'")]
    InvalidResearchTheme(String),

    #[error("invalid age group '{0}'")]
    InvalidAgeGroup(String),

    #[error("invalid language '{0}'")]
    InvalidLanguage(String),

    #[error("invalid theme '{0}'")]
    InvalidTheme(String),

    #[error("invalid scholar '{0}'")]
    InvalidScholar(String),

    #[error("invalid app screen '{0}'")]
    InvalidAppScreen(String),

    #[error("invalid search tab '{0}'")]
    InvalidSearchTab(String),
}
