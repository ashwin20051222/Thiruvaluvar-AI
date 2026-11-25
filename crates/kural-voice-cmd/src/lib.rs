//! Deterministic bilingual Tamil and English voice command router (`< 1 ms`).

pub mod parser;
pub mod router;

pub use parser::{
    AnchoredNumber, FUZZY_MATCH_THRESHOLD, FUZZY_MIN_CHARS, extract_anchored_number,
    fuzzy_phrase_matches, fuzzy_token_matches, is_likely_question, parse_number, parse_quiz_choice,
};
pub use router::{
    RouterContext, VoiceRouteOutcome, check_voice_number_range_error, route_transcript,
    screen_default_target, validate_voice_transcript,
};
