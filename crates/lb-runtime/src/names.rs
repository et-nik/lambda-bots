//! Nicknames for new personalities: `names/<lang>.yaml`, with a small built-in fallback.

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
    /// File the names came from, or `None` for the built-in fallback.
    pub source: Option<std::path::PathBuf>,
}

impl NamePool {
    pub fn load(install_dir: &std::path::Path, language: &str) -> NamePool {
        let mut names = Vec::new();
        let mut source = None;
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
                        source = Some(path);
                        break;
                    }
                    Err(e) => tracing::warn!("{e}"),
                }
            }
        }
        if names.is_empty() {
            tracing::warn!(
                "no bot names in {}; using {} built-in names",
                install_dir.join("names").display(),
                BUILTIN.len()
            );
            names = BUILTIN.iter().map(|s| s.to_string()).collect();
            source = None;
        }
        NamePool { names, source }
    }

    pub fn len(&self) -> usize {
        self.names.len()
    }

    pub fn is_empty(&self) -> bool {
        self.names.is_empty()
    }

    /// A random nickname for a new personality among those `used` rejects (already a personality, or taken by a
    /// player); `None` when the list is exhausted.
    pub fn pick_unused(&self, rng: &mut Pcg32, used: &dyn Fn(&str) -> bool) -> Option<String> {
        let free: Vec<&String> = self.names.iter().filter(|n| !used(n)).collect();
        (!free.is_empty()).then(|| free[rng.range_i32(0, free.len() as i32 - 1) as usize].clone())
    }
}
