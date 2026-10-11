//! Facade over the YAML crate so it can be swapped without touching schemas.

use serde::de::DeserializeOwned;
use serde::{Deserialize, Deserializer, Serialize};

use crate::ConfigError;

pub fn from_str<T: DeserializeOwned>(text: &str, path: &str) -> Result<T, ConfigError> {
    serde_saphyr::from_str::<T>(text).map_err(|e| ConfigError::Parse {
        path: path.to_string(),
        message: e.to_string(),
    })
}

pub fn to_string<T: Serialize>(value: &T) -> Result<String, ConfigError> {
    serde_saphyr::to_string(value).map_err(|e| ConfigError::Parse {
        path: "<serialize>".to_string(),
        message: e.to_string(),
    })
}

/// `s` as a double-quoted scalar. The escapes are JSON's, which YAML accepts too.
pub fn quote(s: &str) -> String {
    let mut out = String::from("\"");
    for c in s.chars() {
        match c {
            '"' => out.push_str("\\\""),
            '\\' => out.push_str("\\\\"),
            c if c.is_control() => out.push_str(&format!("\\u{:04x}", c as u32)),
            c => out.push(c),
        }
    }
    out.push('"');
    out
}

#[derive(Deserialize)]
#[serde(untagged)]
enum OneOrMany {
    One(String),
    Many(Vec<String>),
}

/// One string or a list of them (`alias: Ник`, `map: [gg_*, ag_*]`), for `deserialize_with`.
pub fn one_or_many<'de, D: Deserializer<'de>>(d: D) -> Result<Vec<String>, D::Error> {
    Ok(match OneOrMany::deserialize(d)? {
        OneOrMany::One(one) => vec![one],
        OneOrMany::Many(many) => many,
    })
}

/// Checks the mandatory `schema: lambdabots/<kind>@<major>` header.
pub fn check_schema(found: &str, kind: &str, major: u32, path: &str) -> Result<(), ConfigError> {
    let expected = format!("lambdabots/{kind}@{major}");
    if found == expected {
        Ok(())
    } else {
        Err(ConfigError::Schema {
            path: path.to_string(),
            found: found.to_string(),
            expected,
        })
    }
}
