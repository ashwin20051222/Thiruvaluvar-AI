//! 100% offline, deterministic pure-Rust Layer 2 synthesizer for Thirukkural AI.

mod lexicon;
mod synthesizer;

pub use lexicon::{determine_research_theme, match_tags};
pub use synthesizer::{synthesize_all_chapters, synthesize_chapter};
