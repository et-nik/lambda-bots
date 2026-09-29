//! A map as the editor draws it: BSP v30 faces in triangles with texture and lightmap coordinates, textures from the
//! map and its WAD3 files, lightmaps packed into pages. Pure: bytes in, buffers out.

#![forbid(unsafe_code)]

pub mod light;
pub mod lumps;
pub mod mesh;
pub mod miptex;
mod png;
pub mod wad;

pub use mesh::{FORMAT, Kind, Manifest, MapMesh, WadRef, build, layer_of, listed_wads};
pub use wad::Wad;
