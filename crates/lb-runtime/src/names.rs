//! Bot name pool: names from `names/<lang>.yaml`, unused by any connected player.

use lb_config::names::{NamesFile, sanitize_name};
use lb_core::rng::Pcg32;

const BUILTIN: &[&str] = &[
    "Gordon",
    "Barney",
    "Gina",
    "Colette",
    "Otis",
    "Kleiner",
    "Vance",
    "Magnusson",
    "Rosenberg",
    "Keller",
    "Shephard",
    "Calhoun",
    "Cross",
    "Walter",
    "Freeman",
    "Azure",
    "Cooper",
    "Harold",
    "Simmons",
    "Rosie",
];

pub struct NamePool {
    names: Vec<String>,
}

impl NamePool {
    pub fn load(install_dir: &std::path::Path, language: &str) -> NamePool {
        let mut names = Vec::new();
        for lang in [language, "en"] {
            let path = install_dir.join("names").join(format!("{lang}.yaml"));
            if let Ok(text) = std::fs::read_to_string(&path) {
                match NamesFile::parse(&text, &path.display().to_string()) {
                    Ok(f) => {
                        names = f
                            .names
                            .iter()
                            .map(|n| sanitize_name(n))
                            .filter(|n| !n.is_empty())
                            .collect();
                        break;
                    }
                    Err(e) => tracing::warn!("{e}"),
                }
            }
        }
        if names.is_empty() {
            names = BUILTIN.iter().map(|s| s.to_string()).collect();
        }
        NamePool { names }
    }

    pub fn len(&self) -> usize {
        self.names.len()
    }

    pub fn is_empty(&self) -> bool {
        self.names.is_empty()
    }

    /// Picks a name not used by any connected player; falls back to a numbered name.
    pub fn pick(&self, rng: &mut Pcg32, prefix: &str, taken: &dyn Fn(&str) -> bool) -> String {
        for _ in 0..self.names.len() * 2 {
            let base = &self.names[rng.range_i32(0, self.names.len() as i32 - 1) as usize];
            let name = sanitize_name(&format!("{prefix}{base}"));
            if !name.is_empty() && !taken(&name) {
                return name;
            }
        }
        for i in 1.. {
            let name = sanitize_name(&format!("{prefix}lambda_{i}"));
            if !taken(&name) {
                return name;
            }
        }
        unreachable!()
    }
}
