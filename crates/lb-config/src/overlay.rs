//! `maps/<map>/overlay.yaml` (written by hand) and `maps/<map>/editor.yaml` (written by the editors): what a
//! map needs besides what the generator finds in it — named places, and patches to its navigation graph. Both files
//! have the same schema; the hand-written one is applied last, so it has the last word.

use serde::{Deserialize, Serialize};

use crate::ConfigError;
use crate::yaml;

pub const KIND: &str = "overlay";
pub const MAJOR: u32 = 1;

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct OverlayFile {
    pub schema: String,
    /// The map it is for.
    pub map: String,
    /// Size of the BSP it was made against; when set, an overlay for another build of the map is not applied.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub bsp_size: Option<u64>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub places: Vec<Place>,
    #[serde(default, skip_serializing_if = "NavPatches::is_empty")]
    pub nav: NavPatches,
}

/// A named spot of the map (a bunker, a sniper nest), for behavior and for people reading logs.
#[derive(Clone, Debug, Deserialize, Serialize, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct Place {
    pub name: String,
    pub at: [f32; 3],
    #[serde(default = "default_radius")]
    pub radius: f32,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub tags: Vec<String>,
}

fn default_radius() -> f32 {
    128.0
}

#[derive(Clone, Debug, Default, Deserialize, Serialize, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct NavPatches {
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub patches: Vec<Patch>,
}

impl NavPatches {
    pub fn is_empty(&self) -> bool {
        self.patches.is_empty()
    }
}

/// A change to the generated graph. Points are player origins (hull centres) or near them: each end is the node
/// nearest to the point, within 64 units.
#[derive(Clone, Debug, Deserialize, Serialize, PartialEq)]
#[serde(tag = "op", rename_all = "snake_case", deny_unknown_fields)]
pub enum Patch {
    /// Bots never plan through the nodes within `radius` of `at`.
    Forbid {
        at: [f32; 3],
        radius: f32,
        #[serde(default, skip_serializing_if = "String::is_empty")]
        note: String,
    },
    /// A link the generator missed. It is checked like a generated one (a jump is simulated, a door needs its
    /// opener, ...); `trust` adds it even if the check fails.
    AddLink {
        from: [f32; 3],
        to: [f32; 3],
        /// `jump` makes the check try a jump first; `crouch` checks walking crouched; `longjump` and `gauss_boost`
        /// plan that trick (only bots with the module, or the gauss and its uranium, take them). Other kinds, or none:
        /// the check finds what the link is.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        kind: Option<String>,
        #[serde(default)]
        both: bool,
        #[serde(default)]
        trust: bool,
        #[serde(default, skip_serializing_if = "String::is_empty")]
        note: String,
    },
    /// A link the generator made that does not work.
    RemoveLink {
        from: [f32; 3],
        to: [f32; 3],
        #[serde(default)]
        both: bool,
        #[serde(default, skip_serializing_if = "String::is_empty")]
        note: String,
    },
    /// A node where the generator put none (a ledge, a crate top): set down on the floor under `at` and linked both
    /// ways to the nodes around it, every link checked like a generated one. Patches after it may use it.
    AddNode {
        at: [f32; 3],
        #[serde(default, skip_serializing_if = "String::is_empty")]
        note: String,
    },
    /// The node nearest `from` set down on the floor under `to` instead, keeping its number. Its links are checked
    /// again there: those that no longer hold are taken out (trusted ones stay), and it is linked with the nodes
    /// around it like a node put in. Patches after it find it at `to`.
    MoveNode {
        from: [f32; 3],
        to: [f32; 3],
        #[serde(default, skip_serializing_if = "String::is_empty")]
        note: String,
    },
}

impl OverlayFile {
    pub fn new(map: &str) -> OverlayFile {
        OverlayFile {
            schema: format!("lambdabots/{KIND}@{MAJOR}"),
            map: map.to_string(),
            bsp_size: None,
            places: Vec::new(),
            nav: NavPatches::default(),
        }
    }

    pub fn parse(text: &str, path: &str) -> Result<OverlayFile, ConfigError> {
        let f: OverlayFile = yaml::from_str(text, path)?;
        yaml::check_schema(&f.schema, KIND, MAJOR, path)?;
        let bad = |field: String, message: String| ConfigError::Invalid {
            path: path.to_string(),
            field,
            message,
        };
        for (i, p) in f.places.iter().enumerate() {
            if p.name.trim().is_empty() {
                return Err(bad(format!("places[{i}].name"), "must not be empty".into()));
            }
            if f.places[..i].iter().any(|o| o.name == p.name) {
                return Err(bad(
                    format!("places[{i}].name"),
                    format!("`{}` is defined twice", p.name),
                ));
            }
            if p.radius.is_nan() || p.radius <= 0.0 {
                return Err(bad(format!("places[{i}].radius"), "must be above 0".into()));
            }
        }
        for (i, p) in f.nav.patches.iter().enumerate() {
            match p {
                Patch::Forbid { radius, .. } if radius.is_nan() || *radius <= 0.0 => {
                    return Err(bad(format!("nav.patches[{i}].radius"), "must be above 0".into()));
                }
                Patch::AddLink { kind: Some(k), .. } if !LINK_KINDS.contains(&k.as_str()) => {
                    return Err(bad(
                        format!("nav.patches[{i}].kind"),
                        format!("`{k}`: expected one of {}", LINK_KINDS.join(", ")),
                    ));
                }
                _ => {}
            }
        }
        Ok(f)
    }

    pub fn to_yaml(&self) -> Result<String, ConfigError> {
        yaml::to_string(self)
    }
}

/// Link kinds an added link may name.
pub const LINK_KINDS: &[&str] = &[
    "walk",
    "crouch",
    "jump",
    "drop",
    "ladder",
    "swim",
    "door",
    "lift",
    "teleport",
    "breakable",
    "longjump",
    "gauss_boost",
];

#[cfg(test)]
mod tests {
    use super::*;

    const SAMPLE: &str = "schema: lambdabots/overlay@1
map: crossfire
places:
  - name: bunker
    at: [0, -2300, -1820]
    radius: 300
    tags: [shelter]
nav:
  patches:
    - op: forbid
      at: [100, 200, 300]
      radius: 64
      note: the airstrike's kill zone
    - op: add_link
      from: [1, 2, 3]
      to: [4, 5, 6]
      kind: jump
      both: true
    - op: remove_link
      from: [1, 2, 3]
      to: [7, 8, 9]
    - op: add_node
      at: [10, 20, 30]
      note: the crate top
    - op: move_node
      from: [1, 2, 3]
      to: [40, 2, 3]
";

    #[test]
    fn a_sample_overlay_reads_and_writes_back() {
        let f = OverlayFile::parse(SAMPLE, "overlay.yaml").unwrap();
        assert_eq!(f.places[0].name, "bunker");
        assert_eq!(f.nav.patches.len(), 5);
        assert!(matches!(
            f.nav.patches[3],
            Patch::AddNode {
                at: [10.0, 20.0, 30.0],
                ..
            }
        ));
        assert!(matches!(
            f.nav.patches[4],
            Patch::MoveNode {
                to: [40.0, 2.0, 3.0],
                ..
            }
        ));
        assert!(matches!(
            f.nav.patches[1],
            Patch::AddLink {
                both: true,
                trust: false,
                ..
            }
        ));
        let again = OverlayFile::parse(&f.to_yaml().unwrap(), "again.yaml").unwrap();
        assert_eq!(again, f);
    }

    #[test]
    fn mistakes_are_reported_with_the_field() {
        let wrong_kind = SAMPLE.replace("kind: jump", "kind: rocket");
        let e = OverlayFile::parse(&wrong_kind, "o.yaml").unwrap_err().to_string();
        assert!(e.contains("nav.patches[1].kind"), "{e}");
        let unknown = SAMPLE.replace("radius: 64", "radius: 64\n      depth: 3");
        assert!(OverlayFile::parse(&unknown, "o.yaml").is_err());
        let twice = SAMPLE.replace("places:\n", "places:\n  - name: bunker\n    at: [0, 0, 0]\n");
        let e = OverlayFile::parse(&twice, "o.yaml").unwrap_err().to_string();
        assert!(e.contains("defined twice"), "{e}");
    }
}
