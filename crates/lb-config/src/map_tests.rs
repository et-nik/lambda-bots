//! `maps/<map>/tests.yaml`: tests of how a bot gets about the map (`lb test`, `lb-cli nav try`). Each names where the
//! bot sets off from and the spot it must get to, what it is given, the tricks it may use and how long it has.

use serde::{Deserialize, Serialize};

use crate::ConfigError;
use crate::yaml;

pub const KIND: &str = "tests";
pub const MAJOR: u32 = 1;
/// The file's name in the map's directory.
pub const FILE: &str = "tests.yaml";
/// Tricks a test may allow.
pub const TRICKS: &[&str] = &["jump", "longjump", "gauss", "any", "none"];

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct MapTests {
    pub schema: String,
    pub map: String,
    #[serde(default)]
    pub tests: Vec<MapTest>,
}

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct MapTest {
    pub id: String,
    /// What it checks, for people.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub note: Option<String>,
    /// Where the bot stands before each attempt (it goes there first); without one it sets off from where it is.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub start: Option<[f32; 3]>,
    /// The spot to get to: where a player stands there (the origin of a standing player).
    pub goal: [f32; 3],
    #[serde(default = "default_radius")]
    pub radius: f32,
    /// What the bot is given before each attempt: weapons (with ammo), `uranium`, `longjump`, `health`, `armor`, or
    /// `weapon_`, `ammo_` and `item_` classnames.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub give: Vec<String>,
    /// Tricks it may use: `jump`, `longjump`, `gauss`; `any` by default.
    #[serde(default = "default_tricks")]
    pub tricks: Vec<String>,
    /// Seconds an attempt may take.
    #[serde(default = "default_timeout")]
    pub timeout: f32,
    #[serde(default = "default_repeat")]
    pub repeat: u32,
    /// What should come of it: the bot gets there, or finds no way.
    #[serde(default)]
    pub expect: Expect,
}

#[derive(Clone, Copy, Debug, Default, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum Expect {
    #[default]
    Arrive,
    NoWay,
}

impl Expect {
    pub fn as_str(self) -> &'static str {
        match self {
            Expect::Arrive => "arrive",
            Expect::NoWay => "no_way",
        }
    }
}

pub const RADIUS: f32 = 32.0;
pub const TIMEOUT: f32 = 60.0;

fn default_radius() -> f32 {
    RADIUS
}

fn default_tricks() -> Vec<String> {
    vec!["any".into()]
}

fn default_timeout() -> f32 {
    TIMEOUT
}

fn default_repeat() -> u32 {
    1
}

impl MapTest {
    /// A test of getting to `goal` with the defaults.
    pub fn new(id: &str, goal: [f32; 3]) -> MapTest {
        MapTest {
            id: id.to_string(),
            note: None,
            start: None,
            goal,
            radius: RADIUS,
            give: Vec::new(),
            tricks: default_tricks(),
            timeout: TIMEOUT,
            repeat: 1,
            expect: Expect::Arrive,
        }
    }
}

impl MapTests {
    pub fn new(map: &str) -> MapTests {
        MapTests {
            schema: format!("lambdabots/{KIND}@{MAJOR}"),
            map: map.to_string(),
            tests: Vec::new(),
        }
    }

    pub fn parse(text: &str, path: &str) -> Result<MapTests, ConfigError> {
        let f: MapTests = yaml::from_str(text, path)?;
        yaml::check_schema(&f.schema, KIND, MAJOR, path)?;
        let bad = |i: usize, field: &str, message: String| ConfigError::Invalid {
            path: path.to_string(),
            field: format!("tests[{i}].{field}"),
            message,
        };
        for (i, t) in f.tests.iter().enumerate() {
            if t.id.trim().is_empty() || t.id.contains(char::is_whitespace) {
                return Err(bad(i, "id", "must be one word".into()));
            }
            if f.tests[..i].iter().any(|o| o.id == t.id) {
                return Err(bad(i, "id", format!("`{}` is defined twice", t.id)));
            }
            let finite = |p: &[f32; 3]| p.iter().all(|v| v.is_finite());
            if !finite(&t.goal) || t.start.is_some_and(|s| !finite(&s)) {
                return Err(bad(i, "goal", "coordinates must be numbers".into()));
            }
            if t.radius.is_nan() || t.radius <= 0.0 {
                return Err(bad(i, "radius", "must be above 0".into()));
            }
            if t.timeout.is_nan() || t.timeout <= 0.0 {
                return Err(bad(i, "timeout", "must be above 0".into()));
            }
            if t.repeat == 0 {
                return Err(bad(i, "repeat", "must be 1 or more".into()));
            }
            if let Some(k) = t.tricks.iter().find(|k| !TRICKS.contains(&k.as_str())) {
                return Err(bad(i, "tricks", format!("`{k}`: expected {}", TRICKS.join(", "))));
            }
        }
        Ok(f)
    }

    pub fn get(&self, id: &str) -> Option<&MapTest> {
        self.tests.iter().find(|t| t.id == id)
    }

    /// The file as people write it: one line per field, spots as `[x, y, z]`, defaults left out.
    pub fn to_yaml(&self) -> String {
        let point = |p: &[f32; 3]| format!("[{}, {}, {}]", num(p[0]), num(p[1]), num(p[2]));
        let list = |v: &[String]| {
            let words: Vec<String> = v.iter().map(|w| yaml::quote(w)).collect();
            format!("[{}]", words.join(", "))
        };
        let mut out = format!("schema: {}\nmap: {}\n", self.schema, yaml::quote(&self.map));
        if self.tests.is_empty() {
            out.push_str("tests: []\n");
            return out;
        }
        out.push_str("tests:\n");
        for t in &self.tests {
            out.push_str(&format!("  - id: {}\n", yaml::quote(&t.id)));
            if let Some(n) = &t.note {
                out.push_str(&format!("    note: {}\n", yaml::quote(n)));
            }
            if let Some(s) = &t.start {
                out.push_str(&format!("    start: {}\n", point(s)));
            }
            out.push_str(&format!("    goal: {}\n", point(&t.goal)));
            if t.radius != RADIUS {
                out.push_str(&format!("    radius: {}\n", num(t.radius)));
            }
            if !t.give.is_empty() {
                out.push_str(&format!("    give: {}\n", list(&t.give)));
            }
            if t.tricks != default_tricks() {
                out.push_str(&format!("    tricks: {}\n", list(&t.tricks)));
            }
            if t.timeout != TIMEOUT {
                out.push_str(&format!("    timeout: {}\n", num(t.timeout)));
            }
            if t.repeat != 1 {
                out.push_str(&format!("    repeat: {}\n", t.repeat));
            }
            if t.expect != Expect::Arrive {
                out.push_str(&format!("    expect: {}\n", t.expect.as_str()));
            }
        }
        out
    }
}

/// A number the short way: whole ones without a fraction.
fn num(v: f32) -> String {
    if v.fract() == 0.0 && v.abs() < 1e7 {
        format!("{}", v as i64)
    } else {
        format!("{v}")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const SAMPLE: &str = "schema: lambdabots/tests@1
map: dm_snow
tests:
  - id: ledge
    note: onto the ledge over the bridge
    start: [-120, 340, 36]
    goal: [610, 20.5, 236]
    give: [gauss]
    tricks: [gauss]
    repeat: 3
  - id: pit
    goal: [0, 0, -500]
    expect: no_way
";

    #[test]
    fn a_tests_file_reads_and_writes_back_the_same() {
        let f = MapTests::parse(SAMPLE, "tests.yaml").unwrap();
        assert_eq!(f.tests.len(), 2);
        let ledge = f.get("ledge").unwrap();
        assert_eq!(ledge.start, Some([-120.0, 340.0, 36.0]));
        assert_eq!((ledge.radius, ledge.timeout, ledge.repeat), (RADIUS, TIMEOUT, 3));
        assert_eq!(f.get("pit").unwrap().tricks, vec!["any".to_string()]);
        assert_eq!(f.get("pit").unwrap().expect, Expect::NoWay);
        let text = f.to_yaml();
        assert!(text.contains("    goal: [610, 20.5, 236]\n"), "{text}");
        assert!(!text.contains("radius"), "defaults are left out: {text}");
        assert_eq!(MapTests::parse(&text, "tests.yaml").unwrap(), f);
        assert_eq!(
            MapTests::parse(&MapTests::new("x").to_yaml(), "t").unwrap(),
            MapTests::new("x")
        );
    }

    #[test]
    fn a_bad_test_is_named() {
        let twice = SAMPLE.replace("id: pit", "id: ledge");
        let e = MapTests::parse(&twice, "tests.yaml").unwrap_err().to_string();
        assert!(e.contains("tests[1].id"), "{e}");
        let trick = SAMPLE.replace("tricks: [gauss]", "tricks: [fly]");
        let e = MapTests::parse(&trick, "tests.yaml").unwrap_err().to_string();
        assert!(e.contains("tests[0].tricks") && e.contains("fly"), "{e}");
        let zero = SAMPLE.replace("repeat: 3", "repeat: 0");
        assert!(MapTests::parse(&zero, "tests.yaml").is_err());
    }
}
