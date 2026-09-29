//! Every map of the stand in the editor's buffers: lightmaps that tile the lump, faces and textures all there.
//! Skipped without maps (`LB_MAPS_DIR` or the macOS stand); the WADs are taken from the directory above the maps.

use std::path::Path;

use lb_mapmesh::{Wad, WadRef, build, listed_wads};

fn wads_for(bsp: &[u8], game: &Path) -> Vec<(String, Option<Wad>)> {
    listed_wads(bsp)
        .expect("a BSP")
        .into_iter()
        .map(|name| {
            let wad = std::fs::read(game.join(&name)).ok().and_then(Wad::parse);
            (name, wad)
        })
        .collect()
}

#[test]
fn every_map_meshes_with_its_lightmaps_and_textures() {
    let Some(dir) = lb_bsp::test_maps_dir() else {
        eprintln!("no maps: set LB_MAPS_DIR");
        return;
    };
    let game = dir.parent().expect("maps sit in the game directory");
    let mut maps: Vec<_> = std::fs::read_dir(&dir)
        .expect("maps directory")
        .filter_map(|e| e.ok().map(|e| e.path()))
        .filter(|p| p.extension().is_some_and(|x| x == "bsp"))
        .collect();
    maps.sort();
    let mut problems = Vec::new();
    for path in &maps {
        let name = path.file_stem().unwrap().to_string_lossy().to_string();
        let bytes = std::fs::read(path).unwrap();
        let wads = wads_for(&bytes, game);
        let refs: Vec<WadRef<'_>> = wads
            .iter()
            .map(|(n, w)| WadRef {
                name: n.clone(),
                wad: w.as_ref(),
            })
            .collect();
        let started = std::time::Instant::now();
        let mesh = match build(&name, &bytes, &refs) {
            Ok(m) => m,
            Err(e) => {
                problems.push(format!("{name}: {e}"));
                continue;
            }
        };
        let ms = started.elapsed().as_millis();
        let s = &mesh.manifest.stats;
        let missing_wads: Vec<_> = wads
            .iter()
            .filter(|(_, w)| w.is_none())
            .map(|(n, _)| n.as_str())
            .collect();
        println!(
            "{name:<12} {:>6} faces {:>6} tris {:>3} textures ({} missing) {} pages of {} lightmaps: {} float, {} \
             mismatched; {} skipped; {ms} ms; wads missing {missing_wads:?}",
            s.faces,
            s.triangles,
            mesh.manifest.textures.len(),
            s.missing_textures,
            mesh.manifest.lightmaps.pages,
            mesh.manifest.lightmaps.size,
            s.lightmaps.float,
            s.lightmaps.mismatched,
            s.skipped,
        );
        assert_eq!(mesh.mesh.len(), mesh.manifest.buffers.bytes);
        if s.faces == 0 || s.lightmaps.mismatched > 0 || s.skipped > 0 {
            problems.push(format!("{name}: {s:?}"));
        }
        if s.missing_textures > 0 && missing_wads.is_empty() {
            problems.push(format!(
                "{name}: {} textures missing with every WAD found",
                s.missing_textures
            ));
        }
        for i in 0..mesh.manifest.lightmaps.pages {
            assert!(mesh.lightmap_png(i).is_some_and(|p| p.starts_with(b"\x89PNG")));
        }
        let first = (0..mesh.manifest.textures.len()).find_map(|i| mesh.texture_png(i));
        assert!(
            first.is_some_and(|p| p.starts_with(b"\x89PNG")),
            "{name}: no texture made a PNG"
        );
    }
    assert!(
        problems.is_empty(),
        "{} maps with problems:\n{}",
        problems.len(),
        problems.join("\n")
    );
}

#[test]
fn damaged_maps_are_errors_or_meshes_never_panics() {
    let Some(dir) = lb_bsp::test_maps_dir() else {
        return;
    };
    let Ok(bytes) = std::fs::read(dir.join("crossfire.bsp")) else {
        return;
    };
    // Deterministic damage: runs of bytes overwritten with a sliding pattern all over the file.
    let mut seed = 0x9e37_79b9_7f4a_7c15u64;
    for _ in 0..200 {
        let mut b = bytes.clone();
        for _ in 0..16 {
            seed ^= seed << 13;
            seed ^= seed >> 7;
            seed ^= seed << 17;
            let at = (seed % b.len() as u64) as usize;
            let run = 1 + (seed >> 32) as usize % 64;
            for (k, x) in b[at..(at + run).min(bytes.len())].iter_mut().enumerate() {
                *x = (seed >> (k % 8 * 8)) as u8;
            }
        }
        let _ = build("damaged", &b, &[]);
    }
}
