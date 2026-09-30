//! The game's maps and WADs, and the maps built for the page, kept while they are in use.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use std::time::SystemTime;

use lb_mapmesh::{MapMesh, Wad, WadRef};
use serde::Serialize;

/// Built maps kept at once.
const KEEP: usize = 4;

/// The directories the engine takes content from, in its order: add-ons, downloads, then the mod itself.
pub struct Game {
    dirs: Vec<PathBuf>,
}

#[derive(Clone, Debug, Serialize)]
pub struct MapFile {
    pub name: String,
    pub bytes: u64,
    #[serde(skip)]
    pub path: PathBuf,
}

/// A map name the API takes: a file name without its extension, nothing that climbs out of `maps/`.
pub fn valid_name(name: &str) -> bool {
    !name.is_empty()
        && name.len() <= 64
        && !name.starts_with('.')
        && name.bytes().all(|b| b.is_ascii_alphanumeric() || b"_-.+".contains(&b))
}

impl Game {
    pub fn new(game: &Path) -> Game {
        let name = game
            .file_name()
            .map(|n| n.to_string_lossy().into_owned())
            .unwrap_or_default();
        let mut dirs: Vec<PathBuf> = ["_addon", "_downloads"]
            .into_iter()
            .map(|suffix| game.with_file_name(format!("{name}{suffix}")))
            .filter(|d| d.is_dir())
            .collect();
        dirs.push(game.to_path_buf());
        Game { dirs }
    }

    pub fn dirs(&self) -> &[PathBuf] {
        &self.dirs
    }

    /// Every map by name; where two directories have one, the engine's first.
    pub fn maps(&self) -> Vec<MapFile> {
        let mut out: BTreeMap<String, MapFile> = BTreeMap::new();
        for dir in &self.dirs {
            let Ok(entries) = std::fs::read_dir(dir.join("maps")) else {
                continue;
            };
            for e in entries.flatten() {
                let path = e.path();
                if !path.extension().is_some_and(|x| x.eq_ignore_ascii_case("bsp")) {
                    continue;
                }
                let Some(name) = path.file_stem().map(|s| s.to_string_lossy().into_owned()) else {
                    continue;
                };
                if !valid_name(&name) {
                    continue;
                }
                let bytes = e.metadata().map_or(0, |m| m.len());
                out.entry(name.to_ascii_lowercase())
                    .or_insert(MapFile { name, bytes, path });
            }
        }
        out.into_values().collect()
    }

    pub fn map(&self, name: &str) -> Option<MapFile> {
        if !valid_name(name) {
            return None;
        }
        self.maps().into_iter().find(|m| m.name.eq_ignore_ascii_case(name))
    }

    /// A file in the top of the game directories, in any case (maps name their WADs as they please).
    pub fn file(&self, name: &str) -> Option<PathBuf> {
        self.dirs.iter().find_map(|dir| {
            std::fs::read_dir(dir).ok()?.flatten().map(|e| e.path()).find(|p| {
                p.is_file()
                    && p.file_name()
                        .is_some_and(|n| n.to_string_lossy().eq_ignore_ascii_case(name))
            })
        })
    }
}

/// A PNG made once, `None` for an image the map does not have.
type Png = Option<Arc<Vec<u8>>>;
/// A file's modification time and size: when either changes, what was made from it is made again.
type Stamp = (Option<SystemTime>, u64);
/// A WAD as read, `None` for a file that is not one.
type ReadWad = (Stamp, Option<Arc<Wad>>);

/// A map built for the page, and its images once asked for.
pub struct Built {
    pub mesh: MapMesh,
    /// The manifest as JSON.
    pub manifest: Vec<u8>,
    images: Mutex<BTreeMap<(Image, usize), Png>>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub enum Image {
    Texture,
    Lightmap,
}

impl Built {
    pub fn fingerprint(&self) -> &str {
        &self.mesh.manifest.fingerprint
    }

    /// A texture or lightmap page as a PNG, made once.
    pub fn png(&self, kind: Image, i: usize) -> Png {
        let mut images = self.images.lock().expect("no panics while locked");
        images
            .entry((kind, i))
            .or_insert_with(|| {
                match kind {
                    Image::Texture => self.mesh.texture_png(i),
                    Image::Lightmap => self.mesh.lightmap_png(i),
                }
                .map(Arc::new)
            })
            .clone()
    }
}

#[derive(Debug)]
pub enum MapError {
    NotFound,
    Read(String),
    Broken(String),
}

impl std::fmt::Display for MapError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            MapError::NotFound => write!(f, "no such map"),
            MapError::Read(e) => write!(f, "cannot read the map: {e}"),
            MapError::Broken(e) => write!(f, "the map cannot be drawn: {e}"),
        }
    }
}

struct Kept {
    path: PathBuf,
    stamp: Stamp,
    built: Arc<Built>,
}

pub struct Maps {
    pub game: Game,
    built: Mutex<Vec<Kept>>,
    wads: Mutex<BTreeMap<PathBuf, ReadWad>>,
}

fn stamp(path: &Path) -> Option<Stamp> {
    let meta = std::fs::metadata(path).ok()?;
    Some((meta.modified().ok(), meta.len()))
}

impl Maps {
    pub fn new(game: Game) -> Maps {
        Maps {
            game,
            built: Mutex::new(Vec::new()),
            wads: Mutex::new(BTreeMap::new()),
        }
    }

    /// Map `name` built for the page: from the cache while its file is unchanged.
    pub fn get(&self, name: &str) -> Result<Arc<Built>, MapError> {
        let file = self.game.map(name).ok_or(MapError::NotFound)?;
        let now = stamp(&file.path).ok_or(MapError::NotFound)?;
        {
            let mut kept = self.built.lock().expect("no panics while locked");
            if let Some(i) = kept.iter().position(|k| k.path == file.path && k.stamp == now) {
                let k = kept.remove(i);
                let built = k.built.clone();
                kept.insert(0, k);
                return Ok(built);
            }
        }
        let bytes = std::fs::read(&file.path).map_err(|e| MapError::Read(e.to_string()))?;
        let names = lb_mapmesh::listed_wads(&bytes).map_err(|e| MapError::Broken(e.to_string()))?;
        let wads: Vec<(String, Option<Arc<Wad>>)> = names.into_iter().map(|n| (n.clone(), self.wad(&n))).collect();
        let refs: Vec<WadRef<'_>> = wads
            .iter()
            .map(|(name, wad)| WadRef {
                name: name.clone(),
                wad: wad.as_deref(),
            })
            .collect();
        let started = std::time::Instant::now();
        let mesh = lb_mapmesh::build(&file.name, &bytes, &refs).map_err(|e| MapError::Broken(e.to_string()))?;
        let manifest = serde_json::to_vec(&mesh.manifest).map_err(|e| MapError::Broken(e.to_string()))?;
        let s = &mesh.manifest.stats;
        tracing::info!(
            "{}: {} faces, {} triangles, {} textures missing, {} lightmaps off, in {} ms",
            file.name,
            s.faces,
            s.triangles,
            s.missing_textures,
            s.lightmaps.mismatched,
            started.elapsed().as_millis()
        );
        let built = Arc::new(Built {
            mesh,
            manifest,
            images: Mutex::new(BTreeMap::new()),
        });
        let mut kept = self.built.lock().expect("no panics while locked");
        kept.retain(|k| k.path != file.path);
        kept.insert(
            0,
            Kept {
                path: file.path,
                stamp: now,
                built: built.clone(),
            },
        );
        kept.truncate(KEEP);
        Ok(built)
    }

    /// WAD `name` from the game directories, read once while its file is unchanged.
    fn wad(&self, name: &str) -> Option<Arc<Wad>> {
        let path = self.game.file(name)?;
        let now = stamp(&path)?;
        let mut wads = self.wads.lock().expect("no panics while locked");
        if let Some((s, wad)) = wads.get(&path)
            && *s == now
        {
            return wad.clone();
        }
        let wad = std::fs::read(&path).ok().and_then(Wad::parse).map(Arc::new);
        if wad.is_none() {
            tracing::warn!("{} is not a WAD3 file", path.display());
        }
        wads.insert(path, (now, wad.clone()));
        wad
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn names_stay_inside_maps() {
        for good in [
            "crossfire",
            "dm_snow",
            "1hp_crazy_rooms_beta5",
            "gg_octagon_v2",
            "Adv_Crossfire",
        ] {
            assert!(valid_name(good), "{good}");
        }
        for bad in ["", "../valve", "a/b", "a\\b", ".hidden", "x y", "a%2e"] {
            assert!(!valid_name(bad), "{bad}");
        }
    }

    #[test]
    fn maps_come_from_every_game_directory_the_engine_reads() {
        let root = std::env::temp_dir().join(format!("lb-editor-maps-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        for (dir, map) in [
            ("valve", "crossfire"),
            ("valve", "Stalkyard"),
            ("valve_downloads", "dm_snow"),
            ("valve_downloads", "crossfire"),
        ] {
            std::fs::create_dir_all(root.join(dir).join("maps")).unwrap();
            std::fs::write(root.join(dir).join("maps").join(format!("{map}.bsp")), dir).unwrap();
        }
        std::fs::write(root.join("valve/maps/notes.txt"), "").unwrap();
        std::fs::write(root.join("valve/HalfLife.WAD"), "").unwrap();
        let game = Game::new(&root.join("valve"));
        let maps = game.maps();
        let names: Vec<_> = maps.iter().map(|m| m.name.as_str()).collect();
        assert_eq!(names, ["crossfire", "dm_snow", "Stalkyard"]);
        assert!(
            maps[0].path.starts_with(root.join("valve_downloads")),
            "downloads come before the mod"
        );
        assert!(game.map("stalkyard").is_some() && game.map("../valve/maps/crossfire").is_none());
        assert!(game.file("halflife.wad").is_some());
        let _ = std::fs::remove_dir_all(&root);
    }
}
