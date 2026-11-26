//! Primary Pure-Rust Dioxus Desktop entrypoint (`thiruvalluvar-ai`).
#![forbid(unsafe_code)]

#[cfg(target_os = "linux")]
#[global_allocator]
static GLOBAL: tikv_jemallocator::Jemalloc = tikv_jemallocator::Jemalloc;

fn main() {
    kural_app::run_app();
}
