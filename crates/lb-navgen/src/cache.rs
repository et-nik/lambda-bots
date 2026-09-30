//! Generated graphs kept on disk: `.lbnav` files in a folder per map, named by the key of what they were made from.
//! A graph is reused only for exactly the same key (the same BSP, generator, physics, rules and overlay), and only
//! the few most recently used are kept.

use std::path::{Path, PathBuf};

use lb_bsp::BspWorld;
use lb_nav::NavGraph;
use lb_nav::store::{GraphKey, read, read_key_from, write};

use crate::GenOptions;

/// Version of the generator. Bump it whenever the same map and settings would give another graph.
pub const GENERATOR: u32 = 6;
/// Graphs kept per map.
pub const KEEP: usize = 4;

/// The key of the graph `generate` makes for `world` with `opts`; `rules` and `overlay` hash the server rules and
/// the map's overlay the graph depends on.
pub fn key(world: &BspWorld, opts: &GenOptions, rules: u64, overlay: u64) -> GraphKey {
    let (bsp, bsp_size) = world.bsp.fingerprint;
    GraphKey {
        bsp,
        bsp_size,
        generator: GENERATOR,
        physics: physics_hash(opts),
        rules,
        overlay,
        movement: opts.physics,
    }
}

/// The key's hash of the settings a graph is made with (the player physics among them).
pub fn physics_hash(opts: &GenOptions) -> u64 {
    xxhash_rust::xxh3::xxh3_64(format!("{opts:?}").as_bytes())
}

/// The graphs of one map.
pub struct GraphCache {
    dir: PathBuf,
}

impl GraphCache {
    /// Graphs of `map` under `root` (`addons/lambdabots/nav`).
    pub fn new(root: &Path, map: &str) -> GraphCache {
        GraphCache { dir: root.join(map) }
    }

    pub fn dir(&self) -> &Path {
        &self.dir
    }

    /// The graph made for `key`, if kept. A damaged or stale file is not an error: the graph is made again.
    pub fn load(&self, key: &GraphKey) -> Option<NavGraph> {
        let path = self.dir.join(key.file_name());
        let bytes = std::fs::read(&path).ok()?;
        match read(&bytes) {
            Ok((k, graph)) if &k == key => {
                // Most recently used: the last to go.
                if let Ok(f) = std::fs::File::options().append(true).open(&path) {
                    let _ = f.set_modified(std::time::SystemTime::now());
                }
                Some(graph)
            }
            Ok(_) => None,
            Err(e) => {
                tracing::warn!("{}: {e}; the graph is made again", path.display());
                None
            }
        }
    }

    /// Keeps the graph made for `key` (written whole or not at all), and forgets the least recently used beyond
    /// `KEEP`.
    pub fn store(&self, key: &GraphKey, graph: &NavGraph) -> Result<PathBuf, String> {
        std::fs::create_dir_all(&self.dir).map_err(|e| format!("{}: {e}", self.dir.display()))?;
        let path = self.dir.join(key.file_name());
        let tmp = path.with_extension("lbnav.tmp");
        std::fs::write(&tmp, write(graph, key)).map_err(|e| format!("{}: {e}", tmp.display()))?;
        std::fs::rename(&tmp, &path).map_err(|e| format!("{}: {e}", path.display()))?;
        self.evict();
        Ok(path)
    }

    /// Keys of the graphs kept, most recently used first.
    pub fn kept(&self) -> Vec<(PathBuf, GraphKey)> {
        self.scan().0
    }

    /// The graph files: those whose key reads (only the header is read), most recently used first, and the others.
    fn scan(&self) -> (Vec<(PathBuf, GraphKey)>, Vec<PathBuf>) {
        let mut readable = Vec::new();
        let mut unreadable = Vec::new();
        let paths = std::fs::read_dir(&self.dir)
            .into_iter()
            .flatten()
            .flatten()
            .map(|e| e.path());
        for path in paths.filter(|p| p.extension().is_some_and(|x| x == "lbnav") && p.is_file()) {
            let key = std::fs::File::open(&path)
                .ok()
                .and_then(|mut f| read_key_from(&mut f).ok());
            match key {
                Some(key) => {
                    let used = std::fs::metadata(&path)
                        .and_then(|m| m.modified())
                        .unwrap_or(std::time::SystemTime::UNIX_EPOCH);
                    readable.push((used, path, key));
                }
                None => unreadable.push(path),
            }
        }
        readable.sort_by(|a, b| b.0.cmp(&a.0).then_with(|| a.1.cmp(&b.1)));
        (readable.into_iter().map(|(_, p, k)| (p, k)).collect(), unreadable)
    }

    /// Removes the graph files whose key does not read and the least recently used beyond `KEEP`.
    fn evict(&self) {
        let (readable, unreadable) = self.scan();
        for path in unreadable.iter().chain(readable.iter().skip(KEEP).map(|(p, _)| p)) {
            if let Err(e) = std::fs::remove_file(path) {
                tracing::warn!("{}: {e}", path.display());
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use lb_nav::graph::{GraphStats, NavNode, NodeFlags};

    fn tiny(nodes: usize) -> NavGraph {
        let nodes: Vec<NavNode> = (0..nodes)
            .map(|i| NavNode {
                origin: lb_core::Vec3::new(i as f32 * 64.0, 0.0, 0.0),
                flags: NodeFlags::empty(),
                radius: 16.0,
                support: 0,
                first_link: 0,
                link_count: 0,
            })
            .collect();
        let out = vec![Vec::new(); nodes.len()];
        NavGraph::from_parts(nodes, out, Vec::new(), "test", GraphStats::default())
    }

    fn key(n: u8) -> GraphKey {
        GraphKey {
            bsp: [n; 32],
            bsp_size: 100,
            generator: GENERATOR,
            ..GraphKey::default()
        }
    }

    fn scratch(name: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("lb-navgen-cache-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        dir
    }

    #[test]
    fn a_graph_comes_back_only_for_its_key() {
        let root = scratch("key");
        let cache = GraphCache::new(&root, "crossfire");
        assert!(cache.load(&key(1)).is_none());
        cache.store(&key(1), &tiny(3)).unwrap();
        assert_eq!(cache.load(&key(1)).unwrap(), tiny(3));
        assert!(cache.load(&key(2)).is_none());
        let other_rules = GraphKey { rules: 9, ..key(1) };
        assert!(cache.load(&other_rules).is_none());
        let _ = std::fs::remove_dir_all(root);
    }

    #[test]
    fn only_the_most_recently_used_are_kept() {
        let root = scratch("lru");
        let cache = GraphCache::new(&root, "crossfire");
        for n in 1..=KEEP as u8 {
            cache.store(&key(n), &tiny(n as usize)).unwrap();
            std::thread::sleep(std::time::Duration::from_millis(20));
        }
        // Using the oldest makes it the newest: the next store drops the second oldest instead.
        assert!(cache.load(&key(1)).is_some());
        std::thread::sleep(std::time::Duration::from_millis(20));
        cache.store(&key(9), &tiny(9)).unwrap();
        let kept: Vec<GraphKey> = cache.kept().into_iter().map(|(_, k)| k).collect();
        assert_eq!(kept.len(), KEEP);
        assert_eq!(kept[0], key(9));
        assert!(kept.contains(&key(1)));
        assert!(!kept.contains(&key(2)));
        let _ = std::fs::remove_dir_all(root);
    }

    #[test]
    fn files_without_a_readable_key_go_on_the_next_store() {
        let root = scratch("unreadable");
        let cache = GraphCache::new(&root, "crossfire");
        let first = cache.store(&key(1), &tiny(3)).unwrap();
        let junk = cache.dir().join("0000000000000000.lbnav");
        std::fs::write(&junk, b"not a graph").unwrap();
        // A graph cut short after its key still names what it was made from: only the header is read.
        let bytes = std::fs::read(&first).unwrap();
        let cut = cache.dir().join("1111111111111111.lbnav");
        std::fs::write(&cut, &bytes[..bytes.len() - 5]).unwrap();
        assert_eq!(cache.kept().len(), 2);
        cache.store(&key(2), &tiny(2)).unwrap();
        assert!(!junk.exists());
        assert!(cut.exists() && first.exists());
        let _ = std::fs::remove_dir_all(root);
    }

    #[test]
    fn a_damaged_file_is_made_again() {
        let root = scratch("damaged");
        let cache = GraphCache::new(&root, "crossfire");
        let path = cache.store(&key(1), &tiny(3)).unwrap();
        let mut bytes = std::fs::read(&path).unwrap();
        let mid = bytes.len() / 2;
        bytes[mid] ^= 1;
        std::fs::write(&path, bytes).unwrap();
        assert!(cache.load(&key(1)).is_none());
        let _ = std::fs::remove_dir_all(root);
    }
}
