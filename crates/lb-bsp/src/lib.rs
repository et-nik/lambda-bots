//! BSP v30 loader, hull tracer, entity lump, PVS and PAS.

#![forbid(unsafe_code)]

pub mod entities;
pub mod file;
pub mod hull;
pub mod mech;
pub mod vis;
pub mod world;

pub use entities::{Entity, parse_entities};
pub use file::{Bsp, BspError};
pub use vis::MapVis;
pub use world::BspWorld;

/// Directory with `.bsp` files for tests: `LB_MAPS_DIR`, else the macOS stand's maps when present.
pub fn test_maps_dir() -> Option<std::path::PathBuf> {
    if let Ok(dir) = std::env::var("LB_MAPS_DIR") {
        return Some(dir.into());
    }
    let home = std::env::var("HOME").ok()?;
    let stand = std::path::Path::new(&home).join("Git/half-life/xash3d-fwgs-apple-arm64/valve/maps");
    stand.is_dir().then_some(stand)
}
