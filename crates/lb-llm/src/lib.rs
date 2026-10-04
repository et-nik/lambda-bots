//! Chat models over HTTPS: the Anthropic Messages API and OpenAI-compatible chat completions. Calls block, so they
//! belong on a worker thread; nothing here knows about the game.

#![forbid(unsafe_code)]

mod anthropic;
mod client;
mod error;
mod http;
mod openai;
#[doc(hidden)]
pub mod testing;

pub use client::{Client, Completion, Kind, Prompt, Settings, Stop};
pub use error::{ErrorClass, LlmError};
