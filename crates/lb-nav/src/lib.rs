//! Navigation: the graph, the yapb importer, offline movement checks, planning and path following.

#![forbid(unsafe_code)]

pub mod follow;
pub mod graph;
pub mod import;
pub mod plan;
pub mod validate;
pub mod yapb;

pub use graph::{LinkKind, NavGraph, NavLink, NavNode, NodeFlags, NodeId};
