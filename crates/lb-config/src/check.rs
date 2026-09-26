//! Validation of any lambdabots YAML file by its `schema:` line (used by `lb-cli config check` and CI).

use std::path::{Path, PathBuf};

use crate::names::NamesFile;
use crate::{ConfigError, MainConfig};

/// `schema: lambdabots/<kind>@<major>` from the first lines of a file.
pub fn schema_kind(text: &str) -> Option<(&str, &str)> {
    let line = text.lines().map(str::trim).find(|l| l.starts_with("schema:"))?;
    let value = line["schema:".len()..].trim().trim_matches('"');
    let rest = value.strip_prefix("lambdabots/")?;
    rest.split_once('@')
}

/// Parses and validates one file; returns its kind.
pub fn check_text(text: &str, path: &str) -> Result<String, ConfigError> {
    let Some((kind, _)) = schema_kind(text) else {
        return Err(ConfigError::Invalid {
            path: path.to_string(),
            field: "schema".into(),
            message: "missing `schema: lambdabots/<kind>@<major>` line".into(),
        });
    };
    match kind {
        "main" => MainConfig::parse(text, path).map(|_| ()),
        "names" => NamesFile::parse(text, path).map(|_| ()),
        other => Err(ConfigError::Invalid {
            path: path.to_string(),
            field: "schema".into(),
            message: format!("unknown kind `{other}`"),
        }),
    }?;
    Ok(kind.to_string())
}

pub fn check_file(path: &Path) -> Result<String, ConfigError> {
    let shown = path.display().to_string();
    let text = std::fs::read_to_string(path).map_err(|source| ConfigError::Io {
        path: shown.clone(),
        source,
    })?;
    check_text(&text, &shown)
}

/// All `*.yaml` files under `root`, sorted.
pub fn yaml_files(root: &Path) -> Vec<PathBuf> {
    let mut out = Vec::new();
    let mut stack = vec![root.to_path_buf()];
    while let Some(dir) = stack.pop() {
        let Ok(entries) = std::fs::read_dir(&dir) else { continue };
        for entry in entries.flatten() {
            let p = entry.path();
            if p.is_dir() {
                stack.push(p);
            } else if p.extension().is_some_and(|e| e == "yaml" || e == "yml") {
                out.push(p);
            }
        }
    }
    out.sort();
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn schema_line_is_found() {
        assert_eq!(
            schema_kind("# c\nschema: lambdabots/names@1\nnames: []"),
            Some(("names", "1"))
        );
        assert_eq!(schema_kind("schema: \"lambdabots/main@1\""), Some(("main", "1")));
        assert_eq!(schema_kind("names: []"), None);
    }

    #[test]
    fn shipped_data_is_valid() {
        let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../data");
        let files = yaml_files(&root);
        assert!(files.len() >= 3, "expected the shipped config and name files");
        for f in files {
            check_file(&f).unwrap_or_else(|e| panic!("{e}"));
        }
    }

    #[test]
    fn unknown_kind_is_rejected() {
        assert!(check_text("schema: lambdabots/nope@1\n", "x.yaml").is_err());
    }
}
