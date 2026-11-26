//! Unified non-blocking application controller (`BackendHandle`) wiring database, audio, AI,
//! voice command routing, and user state persistence for `kural-app`.
#![forbid(unsafe_code)]

mod ai_worker;
mod controller;
mod error;
mod speech_text;

pub use controller::BackendHandle;
pub use error::BackendError;

use std::sync::Once;
use tracing_subscriber::EnvFilter;

static TRACING_INIT: Once = Once::new();

/// Initialize global structured tracing once from `KURAL_LOG_LEVEL`, `RUST_LOG`, or `"info"`.
pub fn init_tracing() {
    TRACING_INIT.call_once(|| {
        let level = std::env::var("KURAL_LOG_LEVEL")
            .ok()
            .filter(|s| !s.trim().is_empty())
            .or_else(|| {
                std::env::var("RUST_LOG")
                    .ok()
                    .filter(|s| !s.trim().is_empty())
            })
            .unwrap_or_else(|| "info".to_string());
        let filter = EnvFilter::try_new(&level).unwrap_or_else(|_| EnvFilter::new("info"));
        let _ = tracing_subscriber::fmt()
            .with_env_filter(filter)
            .with_target(true)
            .try_init();
    });
}
