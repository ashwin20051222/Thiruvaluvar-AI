//! Unified non-blocking application controller (`BackendHandle`) wiring database, audio, AI,
//! voice command routing, and user state persistence for `kural-app`.
#![forbid(unsafe_code)]

mod ai_worker;
mod controller;
mod error;
mod speech_text;

pub use controller::BackendHandle;
pub use error::BackendError;
