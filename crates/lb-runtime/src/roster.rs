//! The roster: every personality the server knows, the filters that decide who may join, weighted choice and
//! personalities created for new nicknames.
//!
//! Sources, strongest first: `profiles/*.yaml` (the admin's), then `data/profiles.yaml` (created by the server).
//! A nickname belongs to one personality for good: the server appends new ones to `data/profiles.yaml` and never
//! rewrites existing entries, so the admin can edit them.

use std::path::{Path, PathBuf};
use std::sync::Arc;

use lb_config::main_config::{RosterConfig, parse_style_filter};
use lb_config::profiles::{GENERATED_HEADER, ProfilesFile};
use lb_config::skill::SkillBand;
use lb_core::rng::Pcg32;
use lb_styles::{Persona, PersonaSource, StyleId, StyleTable};
use rustc_hash::FxHashMap;

/// Who may join: a skill band (`lb_difficulty`) and optionally a set of styles (`lb_style`).
#[derive(Clone, Debug, PartialEq)]
pub struct RosterFilter {
    pub skill: SkillBand,
    /// Empty = any style.
    pub styles: Vec<StyleId>,
}

impl RosterFilter {
    pub fn from_config(c: &RosterConfig) -> RosterFilter {
        RosterFilter {
            skill: SkillBand::parse(&c.difficulty).unwrap_or(SkillBand::ANY),
            styles: parse_styles(&c.styles).unwrap_or_default(),
        }
    }

    pub fn admits(&self, p: &Persona) -> bool {
        self.skill.contains(p.skill) && (self.styles.is_empty() || self.styles.contains(&p.style))
    }

    pub fn describe(&self) -> String {
        let styles = if self.styles.is_empty() {
            "any style".to_string()
        } else {
            self.styles.iter().map(|s| s.as_str()).collect::<Vec<_>>().join(",")
        };
        format!("skill {}, {styles}", self.skill)
    }
}

/// `any` or a comma list of styles.
pub fn parse_styles(s: &str) -> Option<Vec<StyleId>> {
    parse_style_filter(s).map(|names| names.iter().filter_map(|n| StyleId::parse(n)).collect())
}

pub struct Roster {
    personas: Vec<Arc<Persona>>,
    by_name: FxHashMap<String, usize>,
    generated_path: PathBuf,
    /// `data/profiles.yaml` exists but does not parse: new personalities are not appended to it.
    generated_broken: bool,
    /// Load problems, shown by `lb roster`.
    pub problems: Vec<String>,
    /// Entries of `data/profiles.yaml` hidden by a profile with the same name.
    pub shadowed: usize,
}

impl Roster {
    pub fn load(install_dir: &Path, models: &[String], styles: &StyleTable) -> Roster {
        let mut roster = Roster {
            personas: Vec::new(),
            by_name: FxHashMap::default(),
            generated_path: install_dir.join("data").join("profiles.yaml"),
            generated_broken: false,
            problems: Vec::new(),
            shadowed: 0,
        };
        let dir = install_dir.join("profiles");
        let mut files: Vec<PathBuf> = std::fs::read_dir(&dir)
            .map(|rd| {
                rd.flatten()
                    .map(|e| e.path())
                    .filter(|p| p.extension().is_some_and(|e| e == "yaml" || e == "yml"))
                    .collect()
            })
            .unwrap_or_default();
        files.sort();
        for path in files {
            match read_profiles(&path) {
                Ok(file) => {
                    for spec in file.entries() {
                        let key = spec.name.to_lowercase();
                        if let Some(&i) = roster.by_name.get(&key) {
                            roster.problems.push(format!(
                                "{}: `{}` is already defined in {}; this entry is ignored",
                                path.display(),
                                spec.name,
                                roster.personas[i].source.describe()
                            ));
                            continue;
                        }
                        roster.insert(Persona::resolve(
                            spec,
                            PersonaSource::Profile(path.clone()),
                            models,
                            styles,
                        ));
                    }
                }
                Err(e) => roster.problems.push(e),
            }
        }
        let generated = roster.generated_path.clone();
        if generated.exists() {
            match read_profiles(&generated) {
                Ok(file) => {
                    for spec in file.entries() {
                        if roster.by_name.contains_key(&spec.name.to_lowercase()) {
                            roster.shadowed += 1;
                            continue;
                        }
                        roster.insert(Persona::resolve(
                            spec,
                            PersonaSource::Generated(generated.clone()),
                            models,
                            styles,
                        ));
                    }
                }
                Err(e) => {
                    roster.generated_broken = true;
                    roster
                        .problems
                        .push(format!("{e}; new personalities will not be saved until it is fixed"));
                }
            }
        }
        for p in &roster.problems {
            tracing::error!("roster: {p}");
        }
        roster
    }

    fn insert(&mut self, persona: Persona) -> Arc<Persona> {
        let persona = Arc::new(persona);
        self.by_name.insert(persona.name.to_lowercase(), self.personas.len());
        self.personas.push(persona.clone());
        persona
    }

    pub fn len(&self) -> usize {
        self.personas.len()
    }

    pub fn is_empty(&self) -> bool {
        self.personas.is_empty()
    }

    pub fn iter(&self) -> impl Iterator<Item = &Arc<Persona>> {
        self.personas.iter()
    }

    pub fn get(&self, name: &str) -> Option<Arc<Persona>> {
        self.by_name
            .get(&name.to_lowercase())
            .map(|&i| self.personas[i].clone())
    }

    pub fn knows(&self, name: &str) -> bool {
        self.by_name.contains_key(&name.to_lowercase())
    }

    pub fn count_admitted(&self, filter: &RosterFilter) -> usize {
        self.personas.iter().filter(|p| filter.admits(p)).count()
    }

    /// Weighted random choice among admitted personalities that are not `busy` (already playing, or their name is
    /// taken by a human). Weight 0 never joins on its own.
    pub fn pick(&self, filter: &RosterFilter, busy: &dyn Fn(&str) -> bool, rng: &mut Pcg32) -> Option<Arc<Persona>> {
        let candidates: Vec<&Arc<Persona>> = self
            .personas
            .iter()
            .filter(|p| p.weight > 0.0 && filter.admits(p) && !busy(&p.name))
            .collect();
        let total: f32 = candidates.iter().map(|p| p.weight).sum();
        if total <= 0.0 {
            return None;
        }
        let mut x = rng.next_f32() * total;
        for p in &candidates {
            if x < p.weight {
                return Some((*p).clone());
            }
            x -= p.weight;
        }
        candidates.last().map(|p| (*p).clone())
    }

    /// Adds a personality created by the server and appends it to `data/profiles.yaml`.
    pub fn add_generated(&mut self, mut persona: Persona, created: &str) -> Arc<Persona> {
        persona.source = PersonaSource::Unsaved;
        if self.generated_broken {
            tracing::warn!(
                "{} is broken; {} lives only until the server stops",
                self.generated_path.display(),
                persona.name
            );
        } else {
            let entry = persona.to_spec(Some(created.to_string())).to_yaml_entry();
            match append_entry(&self.generated_path, &entry, &persona.name) {
                Ok(()) => persona.source = PersonaSource::Generated(self.generated_path.clone()),
                Err(e) => {
                    tracing::error!("cannot save personality {}: {e}", persona.name);
                    self.generated_broken = true;
                }
            }
        }
        self.insert(persona)
    }

    pub fn generated_path(&self) -> &Path {
        &self.generated_path
    }
}

fn read_profiles(path: &Path) -> Result<ProfilesFile, String> {
    let shown = path.display().to_string();
    let text = std::fs::read_to_string(path).map_err(|e| format!("{shown}: {e}"))?;
    ProfilesFile::parse(&text, &shown).map_err(|e| e.to_string())
}

/// Appends one entry to the server-written profile file. The existing text is kept byte for byte (comments and the
/// admin's edits included); the result is validated before it replaces the file.
fn append_entry(path: &Path, entry: &str, name: &str) -> Result<(), String> {
    let mut text = match std::fs::read_to_string(path) {
        Ok(t) if !t.trim().is_empty() => t,
        Ok(_) => GENERATED_HEADER.to_string(),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => GENERATED_HEADER.to_string(),
        Err(e) => return Err(format!("{}: {e}", path.display())),
    };
    if !text.ends_with('\n') {
        text.push('\n');
    }
    text.push_str(entry);
    let parsed = ProfilesFile::parse(&text, &path.display().to_string()).map_err(|e| e.to_string())?;
    if parsed.entries().last().map(|e| e.name.as_str()) != Some(name) {
        return Err(format!(
            "{}: keep `bots:` the last key, new entries are appended to it",
            path.display()
        ));
    }
    if let Some(dir) = path.parent() {
        std::fs::create_dir_all(dir).map_err(|e| format!("{}: {e}", dir.display()))?;
    }
    let tmp = path.with_extension("yaml.tmp");
    std::fs::write(&tmp, &text).map_err(|e| format!("{}: {e}", tmp.display()))?;
    std::fs::rename(&tmp, path).map_err(|e| format!("{}: {e}", path.display()))
}

/// Today's date as `YYYY-MM-DD` (UTC).
pub fn today() -> String {
    let secs = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0);
    let (y, m, d) = civil_from_days((secs / 86_400) as i64);
    format!("{y:04}-{m:02}-{d:02}")
}

/// Now as `YYYYMMDD-HHMMSS` (UTC), for file names.
pub fn stamp() -> String {
    let secs = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0);
    let (y, m, d) = civil_from_days((secs / 86_400) as i64);
    let t = secs % 86_400;
    format!("{y:04}{m:02}{d:02}-{:02}{:02}{:02}", t / 3600, t / 60 % 60, t % 60)
}

/// Days since 1970-01-01 to a proleptic Gregorian date (H. Hinnant's algorithm).
fn civil_from_days(z: i64) -> (i64, u32, u32) {
    let z = z + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z.rem_euclid(146_097);
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = (doy - (153 * mp + 2) / 5 + 1) as u32;
    let m = if mp < 10 { mp + 3 } else { mp - 9 } as u32;
    (yoe + era * 400 + i64::from(m <= 2), m, d)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn models() -> Vec<String> {
        vec!["gordon".into(), "barney".into()]
    }

    #[test]
    fn shipped_profiles_load_cleanly() {
        let data = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../data");
        let r = Roster::load(&data, &models(), &StyleTable::default());
        assert!(r.problems.is_empty(), "{:?}", r.problems);
        let p = r.get("human.exe").unwrap();
        assert_eq!((p.style, p.skill, p.model.as_str()), (StyleId::Rusher, 75, "robo"));
        assert_eq!((p.aggression, p.fear), (1.0, 0.0));
        // Only the fear is given: the aggression comes from the sniper style's range.
        let p = r.get("safety third").unwrap();
        assert_eq!((p.style, p.skill, p.fear), (StyleId::Sniper, 50, 0.6));
        assert!((0.2..=0.5).contains(&p.aggression));
        assert_eq!(r.get("TODO: Fix This Later").unwrap().skill, 25);
    }

    fn temp_dir(tag: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("lb-roster-{tag}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(dir.join("profiles")).unwrap();
        dir
    }

    fn write(path: &Path, text: &str) {
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(path, text).unwrap();
    }

    #[test]
    fn profiles_win_over_generated_and_duplicates_are_reported() {
        let dir = temp_dir("sources");
        write(
            &dir.join("profiles/a.yaml"),
            "schema: lambdabots/profiles@1\nbots:\n  - name: \"Kleiner\"\n    style: sniper\n    skill: 70\n",
        );
        write(
            &dir.join("profiles/b.yaml"),
            "schema: lambdabots/profiles@1\nbots:\n  - name: \"kleiner\"\n    skill: 10\n",
        );
        write(
            &dir.join("data/profiles.yaml"),
            &format!("{GENERATED_HEADER}  - name: \"KLEINER\"\n    skill: 5\n  - name: \"Gina\"\n    skill: 40\n"),
        );
        let r = Roster::load(&dir, &models(), &StyleTable::default());
        assert_eq!(r.len(), 2);
        let k = r.get("kleiner").unwrap();
        assert_eq!((k.skill, k.style), (70, StyleId::Sniper));
        assert!(matches!(k.source, PersonaSource::Profile(_)));
        assert_eq!(r.problems.len(), 1, "{:?}", r.problems);
        assert_eq!(r.shadowed, 1);
        assert!(matches!(r.get("gina").unwrap().source, PersonaSource::Generated(_)));
    }

    #[test]
    fn generated_personalities_are_appended_and_survive_a_reload() {
        let dir = temp_dir("append");
        let custom = format!("{GENERATED_HEADER}# my note\n  - name: \"Old\"\n    skill: 33 # edited by hand\n");
        write(&dir.join("data/profiles.yaml"), &custom);
        let mut r = Roster::load(&dir, &models(), &StyleTable::default());
        let mut rng = Pcg32::new(3, 4);
        let p = lb_styles::generate(
            "New One",
            &[(StyleId::Rusher, 1.0)],
            SkillBand::parse("hard").unwrap(),
            &models(),
            &StyleTable::default(),
            &mut rng,
        );
        let added = r.add_generated(p, "2026-09-27");
        assert!(matches!(added.source, PersonaSource::Generated(_)));
        let text = std::fs::read_to_string(dir.join("data/profiles.yaml")).unwrap();
        assert!(
            text.starts_with(&custom),
            "existing text, comments and edits stay as they were"
        );
        let again = Roster::load(&dir, &models(), &StyleTable::default());
        let back = again.get("new one").unwrap();
        assert_eq!(
            (back.style, back.skill, back.colors, back.model.clone()),
            (added.style, added.skill, added.colors, added.model.clone())
        );
        assert_eq!(again.get("old").unwrap().skill, 33);
    }

    #[test]
    fn broken_generated_file_is_left_alone() {
        let dir = temp_dir("broken");
        write(
            &dir.join("data/profiles.yaml"),
            "schema: lambdabots/profiles@1\nbots:\n  - name: [\n",
        );
        let mut r = Roster::load(&dir, &models(), &StyleTable::default());
        assert!(!r.problems.is_empty());
        let p = lb_styles::generate(
            "X",
            &[(StyleId::Balanced, 1.0)],
            SkillBand::ANY,
            &models(),
            &StyleTable::default(),
            &mut Pcg32::new(1, 1),
        );
        let added = r.add_generated(p, "2026-09-27");
        assert_eq!(added.source, PersonaSource::Unsaved);
        assert_eq!(
            std::fs::read_to_string(dir.join("data/profiles.yaml")).unwrap(),
            "schema: lambdabots/profiles@1\nbots:\n  - name: [\n"
        );
    }

    #[test]
    fn appending_refuses_when_bots_is_not_the_last_key() {
        let dir = temp_dir("lastkey");
        let path = dir.join("data/profiles.yaml");
        write(
            &path,
            "schema: lambdabots/profiles@1\nbots:\n  - name: \"A\"\nextra: 1\n",
        );
        assert!(append_entry(&path, "  - name: \"B\"\n", "B").is_err());
    }

    #[test]
    fn weighted_pick_respects_filter_weight_and_busy_names() {
        let dir = temp_dir("pick");
        write(
            &dir.join("profiles/r.yaml"),
            "schema: lambdabots/profiles@1\nbots:\n  - name: \"often\"\n    weight: 9\n  - name: \"rare\"\n    weight: 1\n  - name: \"never\"\n    weight: 0\n  - name: \"weak\"\n    skill: 5\n  - name: \"busy\"\n",
        );
        let r = Roster::load(&dir, &models(), &StyleTable::default());
        let filter = RosterFilter {
            skill: SkillBand::parse("normal").unwrap(),
            styles: Vec::new(),
        };
        let busy = |n: &str| n == "busy";
        let mut rng = Pcg32::new(9, 9);
        let mut often = 0;
        for _ in 0..2000 {
            let p = r.pick(&filter, &busy, &mut rng).unwrap();
            assert!(p.name == "often" || p.name == "rare", "{}", p.name);
            often += usize::from(p.name == "often");
        }
        assert!((1650..=1950).contains(&often), "weight 9 vs 1: {often}");
        assert_eq!(r.count_admitted(&filter), 4, "`weak` is outside the band");
    }

    #[test]
    fn style_filter() {
        let f = RosterFilter {
            skill: SkillBand::ANY,
            styles: parse_styles("rusher, sniper").unwrap(),
        };
        assert_eq!(f.styles, vec![StyleId::Rusher, StyleId::Sniper]);
        assert!(parse_styles("camper").is_none());
        assert_eq!(parse_styles("any").unwrap(), Vec::<StyleId>::new());
    }

    #[test]
    fn dates() {
        assert_eq!(civil_from_days(0), (1970, 1, 1));
        assert_eq!(civil_from_days(20_723), (2026, 9, 27));
        assert_eq!(civil_from_days(11_016), (2000, 2, 29));
    }
}
