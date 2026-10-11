//! YAML configuration: schemas, loading, validation, provenance, snapshots.

#![forbid(unsafe_code)]

pub mod chat_bots;
pub mod chat_maps;
pub mod chat_phrases;
pub mod chat_players;
pub mod chat_server;
pub mod check;
pub mod main_config;
pub mod map_tests;
pub mod names;
pub mod overlay;
pub mod profiles;
pub mod skill;
pub mod styles;
pub mod yaml;

pub use main_config::MainConfig;

#[derive(Debug, thiserror::Error)]
pub enum ConfigError {
    #[error("{path}: {message}")]
    Parse { path: String, message: String },
    #[error("{path}: schema `{found}` is not supported (expected `{expected}`)")]
    Schema {
        path: String,
        found: String,
        expected: String,
    },
    #[error("{path}: {field}: {message}")]
    Invalid {
        path: String,
        field: String,
        message: String,
    },
    #[error("{path}: {source}")]
    Io { path: String, source: std::io::Error },
}

/// Where the effective value of a setting came from.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Provenance {
    Default,
    File(String),
    Cvar(String),
    Command,
}
