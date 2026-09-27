//! Generated graphs kept on disk: `.lbnav` files in a folder per map, named by the key of what they were made from.
//! A graph is reused only for exactly the same key (the same BSP, generator, physics, rules and overlay), and only
//! the few most recently used are kept.

use std::path::{Path, PathBuf};

use lb_bsp::BspWorld;
use lb_nav::NavGraph;
use lb_nav::store::{GraphKey, read, read_key, write};

use crate::GenOptions;

/// Version of the generator. Bump it whenever the same map and settings would give another graph.
pub const GENERATOR: u32 = 2;
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
        physics: xxhash_rust::xxh3::xxh3_64(format!("{opts:?}").as_bytes()),
        rules,
        overlay,
    }
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
        let mut files: Vec<(std::time::SystemTime, PathBuf)> = std::fs::read_dir(&self.dir)
            .into_iter()
            .flatten()
            .flatten()
            .filter(|e| e.path().extension().is_some_and(|x| x == "lbnav"))
            .filter_map(|e| Some((e.metadata().ok()?.modified().ok()?, e.path())))
            .collect();
        files.sort_by(|a, b| b.0.cmp(&a.0).then_with(|| a.1.cmp(&b.1)));
        files
            .into_iter()
            .filter_map(|(_, p)| {
                let key = read_key(&std::fs::read(&p).ok()?).ok()?;
                Some((p, key))
            })
            .collect()
    }

    fn evict(&self) {
        for (path, _) in self.kept().into_iter().skip(KEEP) {
            if let Err(e) = std::fs::remove_file(&path) {
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
