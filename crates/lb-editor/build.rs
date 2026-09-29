//! Embeds the built page (`tools/editor/dist`) in the binary. Without it the server has no page, only the API, so
//! building the Rust side never needs npm.

use std::fmt::Write as _;
use std::path::{Path, PathBuf};

fn walk(root: &Path, dir: &Path, out: &mut Vec<(String, PathBuf)>) {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return;
    };
    for e in entries.flatten() {
        let path = e.path();
        if path.is_dir() {
            walk(root, &path, out);
        } else if let Ok(rel) = path.strip_prefix(root) {
            let rel = rel
                .components()
                .map(|c| c.as_os_str().to_string_lossy())
                .collect::<Vec<_>>()
                .join("/");
            out.push((rel, path));
        }
    }
}

fn main() {
    let here = PathBuf::from(std::env::var("CARGO_MANIFEST_DIR").expect("cargo sets it"));
    let dist = here.join("../../tools/editor/dist");
    println!("cargo:rerun-if-changed={}", dist.display());
    let mut files = Vec::new();
    walk(&dist, &dist, &mut files);
    files.sort();
    let mut code = String::from("pub static ASSETS: &[(&str, &[u8])] = &[\n");
    for (rel, path) in &files {
        println!("cargo:rerun-if-changed={}", path.display());
        writeln!(code, "    ({rel:?}, include_bytes!({:?})),", path.display().to_string()).expect("a string");
    }
    code.push_str("];\n");
    let out = PathBuf::from(std::env::var("OUT_DIR").expect("cargo sets it"));
    std::fs::write(out.join("assets.rs"), code).expect("OUT_DIR is writable");
}
