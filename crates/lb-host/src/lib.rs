//! Rust half of the EngineDriver: host API, event arena decoding, command emission.

pub mod arena;
pub mod driver;
pub mod ffi_host;
pub mod host;
pub mod record;
pub mod strings;

pub use host::{
    CompatFacts, CreateBotOutcome, CreateBotRequest, CvarHandle, CvarSpec, DebugPrim, EntitySnapshot, Host, PrintKind,
    TraceKind, TraceRequest, TraceResult, TrackRule,
};
