//! `names/<lang>.yaml`: bot name pools.

use serde::Deserialize;

use crate::ConfigError;
use crate::yaml;

#[derive(Clone, Debug, Deserialize, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct NamesFile {
    pub schema: String,
    pub names: Vec<String>,
}

impl NamesFile {
    pub fn parse(text: &str, path: &str) -> Result<NamesFile, ConfigError> {
        let f: NamesFile = yaml::from_str(text, path)?;
        yaml::check_schema(&f.schema, "names", 1, path)?;
        Ok(f)
    }
}

/// Keeps a name usable in the engine: at most 31 bytes, no quotes, semicolons, percent signs or
/// control characters.
pub fn sanitize_name(name: &str) -> String {
    let mut out = String::new();
    for ch in name.chars() {
        if ch.is_control() || matches!(ch, '"' | ';' | '%' | '\\') {
            continue;
        }
        if out.len() + ch.len_utf8() > 31 {
            break;
        }
        out.push(ch);
    }
    out.trim().to_string()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sanitizes() {
        assert_eq!(sanitize_name("a\"b;c%d"), "abcd");
        assert!(sanitize_name(&"x".repeat(40)).len() <= 31);
        assert_eq!(sanitize_name("Ёжик"), "Ёжик");
    }
}
