//! `maps/<map>/moves.yaml`: how players were seen to get about a map, taken from recordings of their games
//! (`lb-cli demo moves`): the high ground nobody walks onto that they get up onto with gauss boosts, and the ways up
//! they take. The generator makes boosts onto the high ground it is shown often enough, and the nodes up there.
//! Counts only: nobody's name is in it.

use serde::{Deserialize, Serialize};

use crate::ConfigError;
use crate::yaml;

pub const KIND: &str = "moves";
pub const MAJOR: u32 = 1;

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct MovesFile {
    pub schema: String,
    /// The map it is for.
    pub map: String,
    /// Size of the BSP the moves were seen on; when set, they are not used on another build of the map.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub bsp_size: Option<u64>,
    #[serde(default)]
    pub source: Source,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub high_ground: Vec<HighGround>,
}

/// What the moves were taken from.
#[derive(Clone, Debug, Default, Deserialize, Serialize, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct Source {
    /// Recordings of the map (a demo holds one for each map played in it).
    pub recordings: u32,
    /// How many of them are trusted: the moves of a player who shows how the map is played.
    #[serde(default, skip_serializing_if = "is_zero")]
    pub trusted: u32,
    /// Minutes the players in them were seen alive, all together.
    pub minutes: f32,
}

/// High ground nobody walks onto that players got up onto with gauss boosts.
#[derive(Clone, Debug, Deserialize, Serialize, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct HighGround {
    /// Climbs up onto it...
    pub seen: u32,
    /// ...by this many players...
    pub players: u32,
    /// ...in this many recordings...
    pub recordings: u32,
    /// ...this many of them in trusted ones.
    #[serde(default, skip_serializing_if = "is_zero")]
    pub trusted: u32,
    /// The ways up they took, the most used first.
    pub ways: Vec<Way>,
}

/// A way up: off the floor at `from` (standing there or jumping from there), a charged shot, and down at `to`, higher
/// than a jump gets. Points are player origins (hull centres) over the floor; climbs off near the same spot onto near
/// the same spot are one way.
#[derive(Clone, Debug, Deserialize, Serialize, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct Way {
    #[serde(serialize_with = "flow")]
    pub from: [f32; 3],
    #[serde(serialize_with = "flow")]
    pub to: [f32; 3],
    /// Climbs seen along it...
    pub seen: u32,
    /// ...this many of them in trusted recordings.
    #[serde(default, skip_serializing_if = "is_zero")]
    pub trusted: u32,
}

fn is_zero(n: &u32) -> bool {
    *n == 0
}

/// A point on one line: `[x, y, z]`.
fn flow<S: serde::Serializer>(p: &[f32; 3], s: S) -> Result<S::Ok, S::Error> {
    serde::Serialize::serialize(&serde_saphyr::FlowSeq(p), s)
}

impl MovesFile {
    pub fn new(map: &str) -> MovesFile {
        MovesFile {
            schema: format!("lambdabots/{KIND}@{MAJOR}"),
            map: map.to_string(),
            bsp_size: None,
            source: Source::default(),
            high_ground: Vec::new(),
        }
    }

    pub fn parse(text: &str, path: &str) -> Result<MovesFile, ConfigError> {
        let f: MovesFile = yaml::from_str(text, path)?;
        yaml::check_schema(&f.schema, KIND, MAJOR, path)?;
        let bad = |field: String, message: &str| ConfigError::Invalid {
            path: path.to_string(),
            field,
            message: message.to_string(),
        };
        for (i, h) in f.high_ground.iter().enumerate() {
            let at = format!("high_ground[{i}]");
            if h.ways.is_empty() {
                return Err(bad(at, "has no ways up"));
            }
            if h.seen == 0 || h.players == 0 || h.recordings == 0 {
                return Err(bad(at, "seen, players and recordings must be above 0"));
            }
            if h.players > h.seen || h.recordings > h.seen || h.trusted > h.seen {
                return Err(bad(
                    at,
                    "players, recordings and trusted are counted among the climbs seen",
                ));
            }
            if h.ways.iter().map(|w| w.seen).sum::<u32>() != h.seen
                || h.ways.iter().map(|w| w.trusted).sum::<u32>() != h.trusted
            {
                return Err(bad(at, "the climbs seen along its ways do not add up to its own"));
            }
            for (k, w) in h.ways.iter().enumerate() {
                let at = format!("high_ground[{i}].ways[{k}]");
                if w.from.iter().chain(&w.to).any(|v| !v.is_finite()) {
                    return Err(bad(at, "a point is not a number"));
                }
                if w.seen == 0 || w.trusted > w.seen {
                    return Err(bad(at, "seen must be above 0, and trusted no more than seen"));
                }
            }
        }
        Ok(f)
    }

    pub fn to_yaml(&self) -> Result<String, ConfigError> {
        yaml::to_string(self)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const SAMPLE: &str = "schema: lambdabots/moves@1
map: dm_example
bsp_size: 123456
source:
  recordings: 12
  trusted: 1
  minutes: 340.5
high_ground:
  - seen: 6
    players: 4
    recordings: 3
    ways:
      - from: [100, 200, -216]
        to: [260, 210, -24]
        seen: 5
      - from: [500, 220, -216]
        to: [330, 215, -24]
        seen: 1
  - seen: 1
    players: 1
    recordings: 1
    trusted: 1
    ways:
      - from: [-500, 40, 36]
        to: [-640, 40, 228]
        seen: 1
        trusted: 1
";

    #[test]
    fn a_sample_reads_and_writes_back() {
        let f = MovesFile::parse(SAMPLE, "moves.yaml").unwrap();
        assert_eq!(f.source.recordings, 12);
        assert_eq!(f.high_ground.len(), 2);
        assert_eq!(f.high_ground[0].ways[0].to, [260.0, 210.0, -24.0]);
        assert_eq!((f.high_ground[0].trusted, f.high_ground[1].trusted), (0, 1));
        let text = f.to_yaml().unwrap();
        assert!(text.contains("from: [100.0, 200.0, -216.0]"), "{text}");
        let again = MovesFile::parse(&text, "again.yaml").unwrap();
        assert_eq!(again, f);
        assert!(crate::check::check_text(SAMPLE, "moves.yaml").is_ok());
    }

    #[test]
    fn counts_that_cannot_be_are_refused() {
        let none = SAMPLE.replace("recordings: 3", "recordings: 0");
        let e = MovesFile::parse(&none, "m.yaml").unwrap_err().to_string();
        assert!(e.contains("high_ground[0]"), "{e}");
        let more = SAMPLE.replace("players: 4", "players: 9");
        assert!(MovesFile::parse(&more, "m.yaml").is_err());
        let apart = SAMPLE.replace("seen: 5", "seen: 4");
        let e = MovesFile::parse(&apart, "m.yaml").unwrap_err().to_string();
        assert!(e.contains("add up"), "{e}");
        let unknown = SAMPLE.replace("seen: 5", "seen: 5\n        speed: 3");
        assert!(MovesFile::parse(&unknown, "m.yaml").is_err());
    }
}
