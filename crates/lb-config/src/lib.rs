//! YAML configuration: schemas, loading, validation, provenance, snapshots.

#![forbid(unsafe_code)]

pub mod check;
pub mod main_config;
pub mod names;
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
