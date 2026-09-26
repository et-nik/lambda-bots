//! Telemetry protocol v2, UDP sink, authenticated command channel, decision trace rings.

#![forbid(unsafe_code)]

pub mod commands;
pub mod sink;

pub use commands::{CommandChannel, CommandRequest, VerifyError};
pub use sink::TelemetrySink;

pub const PROTOCOL_VERSION: u32 = 2;
