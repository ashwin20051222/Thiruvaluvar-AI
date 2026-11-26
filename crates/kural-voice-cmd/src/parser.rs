use kural_core::normalize_nfc_trimmed;
use strsim::normalized_levenshtein;

/// Minimum normalized Levenshtein similarity accepted for fuzzy command matching.
pub const FUZZY_MATCH_THRESHOLD: f64 = 0.80;

/// Minimum Unicode scalar count required before applying fuzzy matching.
pub const FUZZY_MIN_CHARS: usize = 4;

/// A parsed numeric reference optionally anchored to Kural or Adhikaram intent.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AnchoredNumber {
    /// Explicitly anchored to a Kural (e.g. `"kural 481"`, `"குறள் 481"`, `"#481"`, `"open 481"`).
    Kural(u32),
    /// Explicitly anchored to an Adhikaram/chapter (e.g. `"adhikaram 10"`, `"chapter 49"`, `"அதிகாரம் 10"`).
    Adhikaram(u32),
    /// Pure standalone number utterance without an anchor keyword (e.g. `"481"`, `"நானூற்று எண்பத்தொன்று"`).
    Bare(u32),
}

/// Check whether `c` is a Tamil block codepoint (`U+0B80..=U+0BFF`).
#[inline]
const fn is_tamil_char(c: char) -> bool {
    matches!(c, '\u{0B80}'..='\u{0BFF}')
}

/// Check whether `c` should be preserved inside normalized command tokens.
#[inline]
fn is_token_char(c: char) -> bool {
    c.is_alphanumeric() || is_tamil_char(c)
}

/// Normalize an arbitrary utterance into NFC lowercase, replacing `-` and `.` separators cleanly
/// while preserving Tamil script and Unicode combining marks.
pub(crate) fn normalize_for_matching(raw: &str) -> String {
    let nfc = normalize_nfc_trimmed(raw);
    let mut out = String::with_capacity(nfc.len());
    let mut prev_space = true;

    for ch in nfc.chars().flat_map(char::to_lowercase) {
        if is_token_char(ch) {
            out.push(ch);
            prev_space = false;
        } else if (ch.is_whitespace() || ch == '-' || ch == '.' || ch == '_') && !prev_space {
            out.push(' ');
            prev_space = true;
        }
    }

    if out.ends_with(' ') {
        out.pop();
    }
    out
}

/// Strip punctuation at token boundaries while keeping internal dots (e.g., `"மு.வ"`), `#` prefix if needed,
/// and Tamil combining vowel/virama signs intact.
fn trim_token_edges(token: &str) -> &str {
    token.trim_matches(|c: char| !is_token_char(c))
}

/// Return `true` when `token` matches `candidate` exactly or via normalized Levenshtein `>= 0.80`
/// (with fuzzy matching strictly restricted to tokens of `>= 4` Unicode scalar characters).
pub fn fuzzy_token_matches(token: &str, candidate: &str) -> bool {
    if token == candidate {
        return true;
    }
    let tok_len = token.chars().count();
    let cand_len = candidate.chars().count();
    if tok_len < FUZZY_MIN_CHARS || cand_len < FUZZY_MIN_CHARS {
        return false;
    }
    let (min_len, max_len) = if tok_len <= cand_len {
        (tok_len, cand_len)
    } else {
        (cand_len, tok_len)
    };
    // Exact upper-bound filter: normalized_levenshtein <= min_len / max_len.
    if 5 * min_len < 4 * max_len {
        return false;
    }
    normalized_levenshtein(token, candidate) >= FUZZY_MATCH_THRESHOLD
}

/// Return `true` when a normalized multi-word phrase matches `candidate` either token-by-token
/// or as a full phrase (`>= 0.80`) when token counts are identical (or single-word merged form).
pub fn fuzzy_phrase_matches(input: &str, candidate: &str) -> bool {
    if input == candidate {
        return true;
    }
    let in_len = input.chars().count();
    let cand_len = candidate.chars().count();
    if in_len < FUZZY_MIN_CHARS || cand_len < FUZZY_MIN_CHARS {
        return false;
    }

    let in_words: Vec<&str> = input.split_whitespace().collect();
    let cand_words: Vec<&str> = candidate.split_whitespace().collect();

    if in_words.len() == cand_words.len() {
        if in_words
            .iter()
            .zip(cand_words.iter())
            .all(|(a, b)| fuzzy_token_matches(a, b))
        {
            return true;
        }
        let (min_len, max_len) = if in_len <= cand_len {
            (in_len, cand_len)
        } else {
            (cand_len, in_len)
        };
        if 5 * min_len >= 4 * max_len {
            return normalized_levenshtein(input, candidate) >= FUZZY_MATCH_THRESHOLD;
        }
        return false;
    }

    // Allow a single fused word like "darkmode" to fuzzy-match "dark mode".
    if in_words.len() == 1 || cand_words.len() == 1 {
        let (min_len, max_len) = if in_len <= cand_len {
            (in_len, cand_len)
        } else {
            (cand_len, in_len)
        };
        if 5 * min_len >= 4 * max_len {
            return normalized_levenshtein(input, candidate) >= FUZZY_MATCH_THRESHOLD;
        }
    }

    false
}

/// Return `true` when the raw utterance is clearly a general question or explanation request
/// that must route to `AskAiQuestion` rather than being hijacked by command keywords.
pub fn is_likely_question(raw: &str) -> bool {
    let nfc = normalize_nfc_trimmed(raw);
    if nfc.is_empty() {
        return false;
    }
    if nfc.contains('*') || nfc.contains('+') || nfc.contains('/') || nfc.contains('=') {
        return true;
    }

    let norm = normalize_for_matching(&nfc);
    let words: Vec<&str> = norm.split_whitespace().collect();
    if words.is_empty() {
        return false;
    }

    let first = words[0];
    const EN_QUESTION_STARTERS: &[&str] = &[
        "what", "whats", "why", "how", "who", "whose", "whom", "where", "when", "which", "explain",
        "describe", "define", "does", "did", "is", "are", "was", "were",
    ];
    if EN_QUESTION_STARTERS.contains(&first) {
        return true;
    }

    if norm.starts_with("tell me about ")
        || norm.starts_with("can you explain")
        || norm.starts_with("could you explain")
        || norm.starts_with("can you tell")
        || norm.starts_with("could you tell")
        || norm.starts_with("would you explain")
        || norm.starts_with("do you know")
    {
        return true;
    }

    const TA_QUESTION_WORDS: &[&str] = &[
        "என்ன",
        "ஏன்",
        "எப்படி",
        "யார்",
        "எங்கே",
        "எங்கு",
        "எப்போது",
        "எந்த",
        "எத்தனை",
        "எவ்வாறு",
        "விளக்கு",
        "விளக்குங்கள்",
        "விளக்கம்",
        "பற்றி",
        "குறித்து",
        "என்றால்",
        "அர்த்தம்",
    ];
    if words.iter().any(|w| TA_QUESTION_WORDS.contains(w)) {
        return true;
    }

    if nfc.ends_with('?') && words.len() >= 2 {
        let is_nav = matches!(first, "open" | "show" | "go" | "திற" | "காட்டு");
        if !is_nav {
            return true;
        }
    }

    false
}

/// Convert Tamil Unicode digits (`௦..=௯`) or ASCII digits (with optional commas / ordinal suffix)
/// into `u32`.
fn parse_digit_token(tok: &str) -> Option<u32> {
    let mut cleaned = tok
        .strip_suffix("வது")
        .or_else(|| tok.strip_suffix("ஆவது"))
        .or_else(|| tok.strip_suffix("ஆம்"))
        .unwrap_or(tok);

    for suffix in ["st", "nd", "rd", "th"] {
        if let Some(prefix) = cleaned.strip_suffix(suffix)
            && !prefix.is_empty()
            && prefix.chars().all(|c| c.is_ascii_digit())
        {
            cleaned = prefix;
            break;
        }
    }

    if cleaned.is_empty() {
        return None;
    }

    let mut ascii_buf = String::with_capacity(cleaned.len());
    let mut has_digit = false;
    for ch in cleaned.chars() {
        if ch.is_ascii_digit() {
            ascii_buf.push(ch);
            has_digit = true;
        } else if ('\u{0BE6}'..='\u{0BEF}').contains(&ch) {
            let d = (ch as u32 - '\u{0BE6}' as u32) as u8;
            ascii_buf.push((b'0' + d) as char);
            has_digit = true;
        } else if ch == ',' && has_digit {
            continue;
        } else {
            return None;
        }
    }

    if !has_digit {
        return None;
    }
    ascii_buf.parse::<u32>().ok()
}

/// Classification of parsed number token values for multi-word English/Tamil accumulation.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum NumPart {
    Zero,
    Unit(u32),        // 1..=9
    Teen(u32),        // 10..=19
    Ten(u32),         // 20..=90 (multiple of 10)
    HundredUnit(u32), // 200..=900 (already multiplied)
    Hundred,          // 100 (can multiply 1..=99, e.g. "thirteen hundred")
    Thousand,         // 1000 (can multiply 1..=999)
    Million,          // 1_000_000
    Compound(u32),    // Tamil compound number word (e.g. "எண்பத்தொன்று" -> 81)
    Digit(u32),       // Numeric digit literal (e.g. "481")
}

fn classify_english_word(word: &str) -> Option<NumPart> {
    match word {
        "zero" => Some(NumPart::Zero),
        "one" | "first" => Some(NumPart::Unit(1)),
        "two" | "second" => Some(NumPart::Unit(2)),
        "three" | "third" => Some(NumPart::Unit(3)),
        "four" | "fourth" => Some(NumPart::Unit(4)),
        "five" | "fifth" => Some(NumPart::Unit(5)),
        "six" | "sixth" => Some(NumPart::Unit(6)),
        "seven" | "seventh" => Some(NumPart::Unit(7)),
        "eight" | "eighth" => Some(NumPart::Unit(8)),
        "nine" | "ninth" => Some(NumPart::Unit(9)),
        "ten" | "tenth" => Some(NumPart::Teen(10)),
        "eleven" | "eleventh" => Some(NumPart::Teen(11)),
        "twelve" | "twelfth" => Some(NumPart::Teen(12)),
        "thirteen" | "thirteenth" => Some(NumPart::Teen(13)),
        "fourteen" | "fourteenth" => Some(NumPart::Teen(14)),
        "fifteen" | "fifteenth" => Some(NumPart::Teen(15)),
        "sixteen" | "sixteenth" => Some(NumPart::Teen(16)),
        "seventeen" | "seventeenth" => Some(NumPart::Teen(17)),
        "eighteen" | "eighteenth" => Some(NumPart::Teen(18)),
        "nineteen" | "nineteenth" => Some(NumPart::Teen(19)),
        "twenty" | "twentieth" => Some(NumPart::Ten(20)),
        "thirty" | "thirtieth" => Some(NumPart::Ten(30)),
        "forty" | "fourty" | "fortieth" => Some(NumPart::Ten(40)),
        "fifty" | "fiftieth" => Some(NumPart::Ten(50)),
        "sixty" | "sixtieth" => Some(NumPart::Ten(60)),
        "seventy" | "seventieth" => Some(NumPart::Ten(70)),
        "eighty" | "eightieth" => Some(NumPart::Ten(80)),
        "ninety" | "ninetieth" => Some(NumPart::Ten(90)),
        "hundred" | "hundredth" => Some(NumPart::Hundred),
        "thousand" | "thousandth" => Some(NumPart::Thousand),
        "million" => Some(NumPart::Million),
        _ => None,
    }
}

fn exact_tamil_leaf(tok: &str) -> Option<NumPart> {
    match tok {
        "பூஜ்யம்" | "பூஜ்ஜியம்" | "சுழி" | "சுழியம்" | "சைபர்" | "ஜீரோ" | "ஸீரோ" => {
            Some(NumPart::Zero)
        }
        "ஒன்று"
        | "ஒண்ணு"
        | "ஒன்னு"
        | "ஒன்றாவது"
        | "ஒண்ணாவது"
        | "முதல்"
        | "முதலாவது"
        | "முதலாம்"
        | "ஒன்றாம்"
        | "ஒன்"
        | "வன்" => Some(NumPart::Unit(1)),
        "இரண்டு"
        | "ரெண்டு"
        | "ரண்டு"
        | "இரண்டாவது"
        | "ரெண்டாவது"
        | "இரண்டாம்"
        | "ரெண்டாம்"
        | "டூ"
        | "டுூ"
        | "தூ" => Some(NumPart::Unit(2)),
        "மூன்று" | "மூணு" | "மூன்றாவது" | "மூணாவது" | "மூன்றாம்" | "மூணாம்" | "த்ரீ" | "திரீ" => {
            Some(NumPart::Unit(3))
        }
        "நான்கு" | "நாலு" | "நான்காவது" | "நாலாவது" | "நான்காம்" | "நாலாம்" | "ஃபோர்" | "ஃபார்" => {
            Some(NumPart::Unit(4))
        }
        "ஐந்து" | "அஞ்சு" | "ஐந்தாவது" | "அஞ்சாவது" | "ஐந்தாம்" | "அஞ்சாம்" | "ஃபைவ்" | "பைவ்" | "ஃபை" => {
            Some(NumPart::Unit(5))
        }
        "ஆறு" | "ஆறாவது" | "ஆறாம்" | "சிக்ஸ்" | "ஸிக்ஸ்" => {
            Some(NumPart::Unit(6))
        }
        "ஏழு" | "ஏழாவது" | "ஏழாம்" | "செவன்" | "ஸெவன்" => {
            Some(NumPart::Unit(7))
        }
        "எட்டு" | "எட்டாவது" | "எட்டாம்" | "எயிட்" | "எய்ட்" | "ஏட்" | "எய்டு" => {
            Some(NumPart::Unit(8))
        }
        "ஒன்பது" | "ஒன்பதாவது" | "ஒன்பதாம்" | "நைன்" | "நயன்" => {
            Some(NumPart::Unit(9))
        }
        "பத்து" | "பத்தாவது" | "பத்தாம்" | "டென்" => {
            Some(NumPart::Teen(10))
        }
        "பதினொன்று" | "பதினொண்ணு" | "பதினொன்னு" | "பதினொரு" | "லெவன்" | "இலெவன்" | "எலெவன்" => {
            Some(NumPart::Teen(11))
        }
        "பன்னிரண்டு"
        | "பன்னெண்டு"
        | "பனிரண்டு"
        | "பனிரெண்டு"
        | "பன்னிரெண்டு"
        | "பன்னிரு"
        | "ட்வெல்வ்"
        | "ட்வெல்"
        | "டுவெல்" => Some(NumPart::Teen(12)),
        "பதிமூன்று" | "பதிமூணு" | "பதின்மூன்று" | "தேர்டீன்" | "தர்டீன்" => {
            Some(NumPart::Teen(13))
        }
        "பதினான்கு" | "பதிநான்கு" | "பதினாலு" | "ஃபோர்டீன்" | "போர்டீன்" => {
            Some(NumPart::Teen(14))
        }
        "பதினைந்து" | "பதினஞ்சு" | "ஃபிஃப்டீன்" | "ஃபிப்டீன்" | "பிப்டீன்" => {
            Some(NumPart::Teen(15))
        }
        "பதினாறு" | "பதினாரு" | "சிக்ஸ்டீன்" | "ஸிக்ஸ்டீன்" => {
            Some(NumPart::Teen(16))
        }
        "பதினேழு" | "செவன்டீன்" | "ஸெவன்டீன்" => {
            Some(NumPart::Teen(17))
        }
        "பதினெட்டு" | "எய்டீன்" | "எயிடீன்" => {
            Some(NumPart::Teen(18))
        }
        "பத்தொன்பது" | "பத்தொம்பது" | "நைன்டீன்" => {
            Some(NumPart::Teen(19))
        }
        "இருபது" | "இருவது" | "ட்வென்டி" | "ட்வெண்டி" | "டவெண்டி" | "டவென்டி" | "டுவென்டி" | "டுவெண்டி" => {
            Some(NumPart::Ten(20))
        }
        "முப்பது" | "தேர்டி" | "தர்டி" | "தேர்ட்டி" => {
            Some(NumPart::Ten(30))
        }
        "நாற்பது" | "நாப்பது" | "ஃபோர்டி" | "ஃபோர்ட்டி" | "போர்டி" | "போர்ட்டி" | "ஃபார்டி" => {
            Some(NumPart::Ten(40))
        }
        "ஐம்பது" | "அம்பது" | "ஃபிஃப்டி" | "ஃபிப்டி" | "பிப்டி" => {
            Some(NumPart::Ten(50))
        }
        "அறுபது" | "அறுவது" | "சிக்ஸ்டி" | "ஸிக்ஸ்டி" => {
            Some(NumPart::Ten(60))
        }
        "எழுபது" | "எழுவது" | "செவன்டி" | "ஸெவன்டி" => {
            Some(NumPart::Ten(70))
        }
        "எண்பது" | "எம்பது" | "எய்டி" | "எய்ட்டி" | "எயிடி" => {
            Some(NumPart::Ten(80))
        }
        "தொண்ணூறு" | "தொண்ணூரு" | "நைன்டி" | "நைன்ட்டி" => {
            Some(NumPart::Ten(90))
        }
        "நூறு" | "நூரு" | "நூறாவது" | "நூறாம்" | "ஹண்ட்ரட்" | "ஹன்ட்ரட்" | "அண்ட்ரட்" | "ஹண்ட்ரெட்" => {
            Some(NumPart::Hundred)
        }
        "இருநூறு" | "இருநூரு" => Some(NumPart::HundredUnit(200)),
        "முந்நூறு" | "மூந்நூறு" | "முன்னூறு" => {
            Some(NumPart::HundredUnit(300))
        }
        "நானூறு" | "நானூரு" => Some(NumPart::HundredUnit(400)),
        "ஐந்நூறு" | "ஐநூறு" => Some(NumPart::HundredUnit(500)),
        "அறுநூறு" => Some(NumPart::HundredUnit(600)),
        "எழுநூறு" => Some(NumPart::HundredUnit(700)),
        "எண்ணூறு" => Some(NumPart::HundredUnit(800)),
        "தொள்ளாயிரம்" => Some(NumPart::HundredUnit(900)),
        "ஆயிரம்" | "ஓராயிரம்" | "ஆயிரமாவது" | "தவுசண்ட்" | "தௌசண்ட்" | "தௌசன்ட்" => {
            Some(NumPart::Thousand)
        }
        "இரண்டாயிரம்" | "ரெண்டாயிரம்" | "ஈராயிரம்" => {
            Some(NumPart::Compound(2000))
        }
        "மூவாயிரம்" | "மூன்றாயிரம்" => {
            Some(NumPart::Compound(3000))
        }
        "நான்காயிரம்" | "நாலாயிரம்" => {
            Some(NumPart::Compound(4000))
        }
        "ஐயாயிரம்" | "ஐந்தாயிரம்" => {
            Some(NumPart::Compound(5000))
        }
        "ஆறாயிரம்" => Some(NumPart::Compound(6000)),
        "ஏழாயிரம்" => Some(NumPart::Compound(7000)),
        "எண்ணாயிரம்" => Some(NumPart::Compound(8000)),
        "ஒன்பதாயிரம்" => Some(NumPart::Compound(9000)),
        "பத்தாயிரம்" => Some(NumPart::Compound(10000)),
        _ => None,
    }
}

/// Combining stems for Tamil compound numbers (longest prefixes first within each tier).
const TAMIL_COMBINING_STEMS: &[(&str, u32)] = &[
    // Thousands
    ("இரண்டாயிரத்த", 2000),
    ("ரெண்டாயிரத்த", 2000),
    ("ஈராயிரத்த", 2000),
    ("மூன்றாயிரத்த", 3000),
    ("மூவாயிரத்த", 3000),
    ("நான்காயிரத்த", 4000),
    ("நாலாயிரத்த", 4000),
    ("ஐந்தாயிரத்த", 5000),
    ("ஐயாயிரத்த", 5000),
    ("ஓராயிரத்த", 1000),
    ("ஆயிரத்த", 1000),
    // Hundreds (literary + colloquial `-த்த` forms)
    ("தொள்ளாயிரத்த", 900),
    ("எண்ணூற்ற", 800),
    ("எண்ணூத்த", 800),
    ("எழுநூற்ற", 700),
    ("எழுநூத்த", 700),
    ("அறுநூற்ற", 600),
    ("அறுநூத்த", 600),
    ("ஐந்நூற்ற", 500),
    ("ஐந்நூத்த", 500),
    ("ஐநூற்ற", 500),
    ("ஐநூத்த", 500),
    ("நானூற்ற", 400),
    ("நானூத்த", 400),
    ("முந்நூற்ற", 300),
    ("முந்நூத்த", 300),
    ("மூந்நூற்ற", 300),
    ("முன்னூற்ற", 300),
    ("முன்னூத்த", 300),
    ("இருநூற்ற", 200),
    ("இருநூத்த", 200),
    ("நூற்ற", 100),
    ("நூத்த", 100),
    // Tens (literary + colloquial)
    ("தொண்ணூற்ற", 90),
    ("தொண்ணூத்த", 90),
    ("எண்பத்த", 80),
    ("எம்பத்த", 80),
    ("எழுபத்த", 70),
    ("எழுவத்த", 70),
    ("அறுபத்த", 60),
    ("அறுவத்த", 60),
    ("ஐம்பத்த", 50),
    ("அம்பத்த", 50),
    ("நாற்பத்த", 40),
    ("நாப்பத்த", 40),
    ("முப்பத்த", 30),
    ("இருபத்த", 20),
    ("இருவத்த", 20),
];

/// Reconstruct candidate Tamil words following a combining stem using Tamil vowel sandhi rules.
fn reconstruct_tamil_sandhi_candidates(rest: &str) -> Vec<String> {
    let mut chars = rest.chars();
    let Some(first) = chars.next() else {
        return Vec::new();
    };
    let tail = chars.as_str();
    match first {
        // '-த்து' / '-ற்று' + consonant -> '-த்து'
        '\u{0BC1}' => vec![tail.to_string()],
        // '-த்தி' / '-ற்றி' can elide either nothing (e.g. நாற்பத்திமூன்று -> மூன்று),
        // initial 'இ' (e.g. நாற்பத்திரண்டு -> இரண்டு, நூற்றிருபது -> இருபது),
        // or carry a 'ய' glide before a vowel (e.g. நூற்றியிருபத்தெட்டு -> இருபத்தெட்டு).
        '\u{0BBF}' => {
            let mut cands = vec![tail.to_string(), format!("இ{tail}")];
            if let Some(after_ya) = tail.strip_prefix('ய') {
                cands.extend(reconstruct_tamil_sandhi_candidates(after_ya));
            }
            cands
        }
        '\u{0BBE}' => vec![format!("ஆ{tail}")],
        '\u{0BC0}' => vec![format!("ஈ{tail}")],
        '\u{0BC2}' => vec![format!("ஊ{tail}")],
        '\u{0BC6}' => vec![format!("எ{tail}")],
        '\u{0BC7}' => vec![format!("ஏ{tail}")],
        '\u{0BC8}' => vec![format!("ஐ{tail}")],
        '\u{0BCA}' => vec![format!("ஒ{tail}")],
        '\u{0BCB}' => vec![format!("ஓ{tail}")],
        '\u{0BCD}' => Vec::new(),
        // Inherent 'அ' merged onto the stem's final consonant (e.g. இருபத்தஞ்சு -> அஞ்சு)
        _ => vec![format!("அ{rest}")],
    }
}

fn parse_tamil_token_rec(tok: &str, depth: u8) -> Option<NumPart> {
    if tok.is_empty() || depth > 4 {
        return None;
    }
    if let Some(part) = exact_tamil_leaf(tok) {
        return Some(part);
    }

    // Handle arbitrary Tamil ordinal suffixes like "...ஆவது" / "...ஆம்"
    for ord_suffix in ["\u{0BBE}வது", "\u{0BBE}ம்"] {
        if let Some(stem) = tok.strip_suffix(ord_suffix) {
            let as_u = format!("{stem}\u{0BC1}");
            if let Some(part) = parse_tamil_token_rec(&as_u, depth + 1) {
                return Some(part);
            }
            let as_m = format!("{stem}ம்");
            if let Some(part) = parse_tamil_token_rec(&as_m, depth + 1) {
                return Some(part);
            }
        }
    }

    for &(stem, stem_val) in TAMIL_COMBINING_STEMS {
        if let Some(rest) = tok.strip_prefix(stem) {
            if rest.is_empty() || rest == "\u{0BC1}" || rest == "\u{0BBF}" {
                return Some(match stem_val {
                    100 => NumPart::Hundred,
                    1000 => NumPart::Thousand,
                    20..=90 => NumPart::Ten(stem_val),
                    200..=900 => NumPart::HundredUnit(stem_val),
                    _ => NumPart::Compound(stem_val),
                });
            }
            for sub_tok in reconstruct_tamil_sandhi_candidates(rest) {
                if let Some(sub_part) = parse_tamil_token_rec(&sub_tok, depth + 1) {
                    let sub_val = match sub_part {
                        NumPart::Unit(v)
                        | NumPart::Teen(v)
                        | NumPart::Ten(v)
                        | NumPart::HundredUnit(v)
                        | NumPart::Compound(v)
                        | NumPart::Digit(v) => v,
                        NumPart::Hundred => 100,
                        NumPart::Thousand => 1000,
                        NumPart::Zero | NumPart::Million => continue,
                    };
                    if sub_val > 0 && sub_val < stem_val {
                        return Some(NumPart::Compound(stem_val + sub_val));
                    }
                }
            }
        }
    }

    None
}

fn classify_number_token(tok: &str) -> Option<NumPart> {
    if let Some(n) = parse_digit_token(tok) {
        return Some(if n == 0 {
            NumPart::Zero
        } else {
            NumPart::Digit(n)
        });
    }
    if let Some(part) = classify_english_word(tok) {
        return Some(part);
    }
    parse_tamil_token_rec(tok, 0)
}

/// Contextual ASR homophone number tokens that are ONLY accepted when adjacent to another
/// unambiguous number token (prevents Tamil words like "போர்" / English "oh" firing alone).
fn classify_contextual_number_token(tok: &str) -> Option<NumPart> {
    match tok {
        "oh" | "o" | "ஓ" => Some(NumPart::Zero),
        "டு" => Some(NumPart::Unit(2)),
        "போர்" => Some(NumPart::Unit(4)),
        "வெயிட்" => Some(NumPart::Unit(8)),
        _ => None,
    }
}

fn single_digit_part(tok: &str) -> Option<(u32, bool)> {
    if let Some(part) = classify_number_token(tok) {
        return match part {
            NumPart::Zero => Some((0, false)),
            NumPart::Unit(d) if d <= 9 => Some((d, false)),
            NumPart::Digit(d) if d <= 9 => Some((d, false)),
            _ => None,
        };
    }
    classify_contextual_number_token(tok).and_then(|part| match part {
        NumPart::Zero => Some((0, true)),
        NumPart::Unit(d) if d <= 9 => Some((d, true)),
        _ => None,
    })
}

/// Parse a digit-by-digit spoken sequence such as `"one two eight"` -> `128`,
/// `"ஒன்று இரண்டு எட்டு"` -> `128`, `"ஒன் டுூ வெயிட்"` -> `128`,
/// or anchored 2-digit `"kural four two"` -> `42`.
fn try_parse_digit_sequence(tokens: &[&str], allow_two_digits: bool) -> Option<(u32, usize)> {
    let min_len = if allow_two_digits { 2 } else { 3 };
    if tokens.len() < min_len {
        return None;
    }

    let mut digits = Vec::with_capacity(4);
    let mut saw_non_contextual = false;

    for &raw_tok in tokens.iter().take(4) {
        let tok = trim_token_edges(raw_tok);
        if tok.is_empty() {
            break;
        }
        let Some((d, is_contextual)) = single_digit_part(tok) else {
            break;
        };
        if digits.is_empty() && d == 0 {
            return None;
        }
        if !is_contextual {
            saw_non_contextual = true;
        }
        digits.push(d);
    }

    if digits.len() < min_len || !saw_non_contextual {
        return None;
    }

    // Do not steal digits from a larger multiplicative expression like "one two hundred".
    if let Some(&next_raw) = tokens.get(digits.len()) {
        let next_tok = trim_token_edges(next_raw);
        if let Some(next_part) = classify_number_token(next_tok)
            && matches!(
                next_part,
                NumPart::Hundred
                    | NumPart::Thousand
                    | NumPart::Million
                    | NumPart::Ten(_)
                    | NumPart::Teen(_)
                    | NumPart::HundredUnit(_)
            )
        {
            return None;
        }
    }

    let mut value: u32 = 0;
    for d in &digits {
        value = value.checked_mul(10)?.checked_add(*d)?;
    }
    Some((value, digits.len()))
}

/// Accumulate a contiguous slice of tokens into a single `u32` number, returning the number
/// and how many tokens were consumed (stopping before non-number tokens).
fn parse_number_span_with_opts(tokens: &[&str], allow_two_digit_seq: bool) -> Option<(u32, usize)> {
    if let Some(seq) = try_parse_digit_sequence(tokens, allow_two_digit_seq) {
        return Some(seq);
    }

    let mut total: u64 = 0;
    let mut current: u64 = 0;
    let mut saw_value = false;
    let mut saw_zero = false;
    let mut last_non_connector_consumed = 0usize;
    let mut idx = 0usize;

    while idx < tokens.len() {
        let tok = trim_token_edges(tokens[idx]);
        if tok.is_empty() {
            break;
        }
        if tok == "and" || ((tok == "a" || tok == "ஒரு") && !saw_value) {
            // Only treat "and" / "a" / "ஒரு" as part of the number if followed by a number token.
            if idx + 1 < tokens.len()
                && let Some(next_part) = classify_number_token(trim_token_edges(tokens[idx + 1]))
            {
                if tok == "a" || tok == "ஒரு" {
                    // Only valid before hundred/thousand/million (prevents "ஒரு கதை" -> 1).
                    if matches!(
                        next_part,
                        NumPart::Hundred | NumPart::Thousand | NumPart::Million
                    ) {
                        idx += 1;
                        continue;
                    }
                    break;
                }
                if saw_value {
                    idx += 1;
                    continue;
                }
            }
            break;
        }

        // Handle trailing standalone Tamil ordinal marker ("481 வது", "10 ஆம்")
        if saw_value && matches!(tok, "வது" | "ஆவது" | "ஆம்") {
            last_non_connector_consumed = idx + 1;
            break;
        }

        // Spoken hundreds shorthand with zero/oh: e.g. "one oh five" -> 105, "ten oh one" -> 1001.
        if saw_value
            && total == 0
            && (1..=13).contains(&current)
            && matches!(
                classify_number_token(tok).or_else(|| classify_contextual_number_token(tok)),
                Some(NumPart::Zero)
            )
            && idx + 1 < tokens.len()
            && let Some(NumPart::Unit(u)) = classify_number_token(trim_token_edges(tokens[idx + 1]))
                .or_else(|| classify_contextual_number_token(trim_token_edges(tokens[idx + 1])))
        {
            current = current * 100 + u64::from(u);
            idx += 2;
            last_non_connector_consumed = idx;
            continue;
        }

        let part = match classify_number_token(tok) {
            Some(p) => p,
            None if saw_value => match classify_contextual_number_token(tok) {
                Some(p) => p,
                None => break,
            },
            None => break,
        };

        match part {
            NumPart::Zero => {
                if saw_value || saw_zero {
                    break;
                }
                saw_zero = true;
                saw_value = true;
            }
            NumPart::Unit(v) => {
                if saw_zero || !current.is_multiple_of(10) {
                    break;
                }
                current += u64::from(v);
                saw_value = true;
            }
            NumPart::Teen(v) => {
                if saw_zero {
                    break;
                }
                if current.is_multiple_of(100) {
                    current += u64::from(v);
                } else if total == 0 && (1..=13).contains(&current) {
                    // Spoken hundreds shorthand: "one nineteen" -> 119, "twelve fourteen" -> 1214.
                    current = current * 100 + u64::from(v);
                } else {
                    break;
                }
                saw_value = true;
            }
            NumPart::Ten(v) => {
                if saw_zero {
                    break;
                }
                if current.is_multiple_of(100) {
                    current += u64::from(v);
                } else if total == 0 && (1..=13).contains(&current) {
                    // Spoken hundreds shorthand: "one twenty eight" -> 128, "thirteen thirty" -> 1330.
                    current = current * 100 + u64::from(v);
                } else {
                    break;
                }
                saw_value = true;
            }
            NumPart::HundredUnit(v) => {
                if saw_zero || !current.is_multiple_of(1000) {
                    break;
                }
                current += u64::from(v);
                saw_value = true;
            }
            NumPart::Hundred => {
                if saw_zero || current >= 100 {
                    break;
                }
                current = if current == 0 { 100 } else { current * 100 };
                saw_value = true;
            }
            NumPart::Thousand => {
                if saw_zero || current >= 1000 {
                    break;
                }
                let factor = if current == 0 { 1 } else { current };
                total += factor * 1000;
                current = 0;
                saw_value = true;
            }
            NumPart::Million => {
                if saw_zero || current >= 1000 {
                    break;
                }
                let factor = if current == 0 { 1 } else { current };
                total += factor * 1_000_000;
                current = 0;
                saw_value = true;
            }
            NumPart::Compound(v) => {
                if saw_zero {
                    break;
                }
                let v64 = u64::from(v);
                if (v < 100 && current.is_multiple_of(100))
                    || ((100..1000).contains(&v) && current.is_multiple_of(1000))
                    || (current == 0 && total == 0)
                {
                    current += v64;
                } else if v < 100 && total == 0 && (1..=13).contains(&current) {
                    current = current * 100 + v64;
                } else {
                    break;
                }
                saw_value = true;
            }
            NumPart::Digit(v) => {
                if saw_zero || current != 0 || total != 0 {
                    break;
                }
                current = u64::from(v);
                saw_value = true;
            }
        }

        idx += 1;
        last_non_connector_consumed = idx;
    }

    if !saw_value || last_non_connector_consumed == 0 {
        return None;
    }

    let val = u32::try_from(total + current).ok()?;
    Some((val, last_non_connector_consumed))
}

fn parse_number_span(tokens: &[&str]) -> Option<(u32, usize)> {
    parse_number_span_with_opts(tokens, false)
}

/// Parse an entire string as a single Tamil or English cardinal/ordinal number or digits.
pub fn parse_number(raw: &str) -> Option<u32> {
    let norm = normalize_for_matching(raw);
    let tokens: Vec<&str> = norm.split_whitespace().collect();
    if tokens.is_empty() {
        return None;
    }
    let (val, consumed) = parse_number_span(&tokens)?;
    if consumed == tokens.len() {
        Some(val)
    } else {
        None
    }
}

/// Parse a quiz or word-game choice (`"a"`..=`"d"`, `"1"`..=`"4"`, `"first"`..=`"fourth"`,
/// `"ஒன்று"`..=`"நான்கு"`, `"அ"`..=`"ஈ"`) into a zero-based index `0..=3`.
pub fn parse_quiz_choice(raw: &str) -> Option<u8> {
    let norm = normalize_for_matching(raw);
    if norm.is_empty() {
        return None;
    }

    let words: Vec<&str> = norm.split_whitespace().collect();
    let stripped = match words.as_slice() {
        [prefix, rest]
            if matches!(
                *prefix,
                "option" | "choice" | "answer" | "number" | "விடை" | "தேர்வு" | "எண்"
            ) =>
        {
            *rest
        }
        [single] => *single,
        _ => return None,
    };

    const CHOICES: &[(&[&str], u8)] = &[
        (
            &[
                "a",
                "1",
                "one",
                "first",
                "1st",
                "அ",
                "ஒன்று",
                "ஒண்ணு",
                "ஒன்னு",
                "முதல்",
                "முதலாவது",
                "ஒன்றாவது",
            ],
            0,
        ),
        (
            &[
                "b",
                "2",
                "two",
                "second",
                "2nd",
                "ஆ",
                "இரண்டு",
                "ரெண்டு",
                "இரண்டாவது",
                "ரெண்டாவது",
            ],
            1,
        ),
        (
            &[
                "c",
                "3",
                "three",
                "third",
                "3rd",
                "இ",
                "மூன்று",
                "மூணு",
                "மூன்றாவது",
                "மூணாவது",
            ],
            2,
        ),
        (
            &[
                "d",
                "4",
                "four",
                "fourth",
                "4th",
                "ஈ",
                "நான்கு",
                "நாலு",
                "நான்காவது",
                "நாலாவது",
            ],
            3,
        ),
    ];

    for (aliases, idx) in CHOICES {
        if aliases
            .iter()
            .any(|&cand| fuzzy_token_matches(stripped, cand))
        {
            return Some(*idx);
        }
    }
    None
}

fn is_kural_anchor(tok: &str) -> bool {
    const KURAL_ANCHORS: &[&str] = &[
        "kural",
        "kurals",
        "couplet",
        "verse",
        "thirukkural",
        "thirukural",
        "ural",
        "ura",
        "curl",
        "curial",
        "coral",
        "koral",
        "kooral",
        "choral",
        "cural",
        "kuril",
        "gural",
        "quarrel",
        "குறள்",
        "திருக்குறள்",
        "குரல்",
        "குறல்",
        "குறால்",
        "கூரல்",
        "குறல",
        "குழல்",
        "திருக்குரல்",
        "திருக்குறல்",
    ];
    KURAL_ANCHORS
        .iter()
        .any(|&cand| fuzzy_token_matches(tok, cand))
}

fn is_adhikaram_anchor(tok: &str) -> bool {
    const ADHIKARAM_ANCHORS: &[&str] = &["adhikaram", "adhikaaram", "chapter", "அதிகாரம்"];
    ADHIKARAM_ANCHORS
        .iter()
        .any(|&cand| fuzzy_token_matches(tok, cand))
}

fn is_nav_number_anchor(tok: &str) -> bool {
    matches!(
        tok,
        "open"
            | "show"
            | "goto"
            | "number"
            | "read"
            | "tell"
            | "give"
            | "play"
            | "speak"
            | "எண்"
            | "நம்பர்"
            | "திற"
            | "திறக்கவும்"
            | "காட்டு"
            | "காட்டுங்கள்"
            | "படி"
            | "படியுங்கள்"
            | "வாசி"
            | "வாசியுங்கள்"
            | "போடு"
            | "போடுங்கள்"
            | "கூறு"
            | "கூறுங்கள்"
            | "கேள்"
            | "சொல்"
            | "சொல்லு"
            | "சொல்லுங்கள்"
    )
}

fn is_polite_or_connector(tok: &str) -> bool {
    matches!(
        tok,
        "please"
            | "kindly"
            | "ok"
            | "okay"
            | "hey"
            | "the"
            | "to"
            | "go"
            | "me"
            | "no"
            | "number"
            | "read"
            | "tell"
            | "give"
            | "play"
            | "speak"
            | "எண்"
            | "நம்பர்"
            | "படி"
            | "படியுங்கள்"
            | "வாசி"
            | "வாசியுங்கள்"
            | "போடு"
            | "போடுங்கள்"
            | "கூறு"
            | "கூறுங்கள்"
            | "கேள்"
            | "சொல்"
            | "சொல்லு"
            | "சொல்லுங்கள்"
            | "தயவுசெய்து"
    )
}

/// Extract an anchored Kural, anchored Adhikaram, or bare standalone number from `raw`.
pub fn extract_anchored_number(raw: &str) -> Option<AnchoredNumber> {
    let nfc = normalize_nfc_trimmed(raw);
    if nfc.is_empty()
        || nfc.contains('*')
        || nfc.contains('+')
        || nfc.contains('/')
        || nfc.contains('=')
    {
        return None;
    }

    let has_hash_prefix = nfc
        .split_whitespace()
        .any(|w| w.starts_with('#') && w.len() > 1);
    let norm = normalize_for_matching(&nfc);
    let tokens: Vec<&str> = norm.split_whitespace().collect();
    if tokens.is_empty() {
        return None;
    }

    // 1. Scan for an explicit Adhikaram anchor adjacent to a number span.
    for i in 0..tokens.len() {
        if is_adhikaram_anchor(tokens[i]) {
            let mut j = i + 1;
            while j < tokens.len() && matches!(tokens[j], "number" | "no" | "எண்" | "நம்பர்")
            {
                j += 1;
            }
            if j < tokens.len()
                && let Some((n, _)) = parse_number_span_with_opts(&tokens[j..], true)
            {
                return Some(AnchoredNumber::Adhikaram(n));
            }
            // Also support pre-nominal ordinal: "10th chapter", "பத்தாவது அதிகாரம்"
            if i > 0
                && let Some((n, consumed)) = parse_number_span_with_opts(&tokens[..i], true)
                && consumed == i
            {
                return Some(AnchoredNumber::Adhikaram(n));
            }
        }
    }

    // 2. Scan for an explicit Kural anchor adjacent to a number span.
    for i in 0..tokens.len() {
        if is_kural_anchor(tokens[i]) {
            let mut j = i + 1;
            while j < tokens.len() && matches!(tokens[j], "number" | "no" | "எண்" | "நம்பர்")
            {
                j += 1;
            }
            if j < tokens.len()
                && let Some((n, _)) = parse_number_span_with_opts(&tokens[j..], true)
            {
                return Some(AnchoredNumber::Kural(n));
            }
            // Pre-nominal ordinal/number: "481st kural", "முதல் குறள்", "42 வது குறள் படி"
            let tail_ok = tokens[i + 1..]
                .iter()
                .all(|t| is_polite_or_connector(t) || is_nav_number_anchor(t));
            if tail_ok {
                for start in 0..i {
                    if let Some((n, consumed)) =
                        parse_number_span_with_opts(&tokens[start..i], true)
                        && start + consumed == i
                    {
                        let prefix_ok = tokens[..start]
                            .iter()
                            .all(|t| is_polite_or_connector(t) || is_nav_number_anchor(t));
                        if prefix_ok {
                            return Some(AnchoredNumber::Kural(n));
                        }
                    }
                }
            }
        }
    }

    // 3. Whole utterance with navigation/hash anchor (e.g. "#481", "open 481", "go to 1500", "number 0").
    let mut idx = 0;
    let mut saw_anchor = has_hash_prefix;
    while idx < tokens.len() {
        let tok = tokens[idx];
        if is_nav_number_anchor(tok) || is_polite_or_connector(tok) {
            if is_nav_number_anchor(tok) {
                saw_anchor = true;
            }
            idx += 1;
        } else {
            break;
        }
    }

    if idx < tokens.len()
        && let Some((n, consumed)) = parse_number_span_with_opts(&tokens[idx..], saw_anchor)
    {
        let tail = &tokens[idx + consumed..];
        let tail_ok = tail
            .iter()
            .all(|t| is_polite_or_connector(t) || is_nav_number_anchor(t));
        if tail_ok {
            let tail_has_anchor = tail.iter().any(|t| is_nav_number_anchor(t));
            if saw_anchor || tail_has_anchor {
                return Some(AnchoredNumber::Kural(n));
            }
            if idx == 0 && consumed == tokens.len() {
                return Some(AnchoredNumber::Bare(n));
            }
        }
    }

    None
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_digits_and_tamil_digits() {
        assert_eq!(parse_number("1"), Some(1));
        assert_eq!(parse_number("481"), Some(481));
        assert_eq!(parse_number("1330"), Some(1330));
        assert_eq!(parse_number("0"), Some(0));
        assert_eq!(parse_number("481st"), Some(481));
        assert_eq!(parse_number("2nd"), Some(2));
        assert_eq!(parse_number("௪௮௧"), Some(481));
        assert_eq!(parse_number("௧௩௩௦"), Some(1330));
    }

    #[test]
    fn parses_english_cardinal_and_ordinal_words() {
        assert_eq!(parse_number("zero"), Some(0));
        assert_eq!(parse_number("one"), Some(1));
        assert_eq!(parse_number("first"), Some(1));
        assert_eq!(parse_number("thirteen"), Some(13));
        assert_eq!(parse_number("twenty-five"), Some(25));
        assert_eq!(parse_number("four hundred eighty one"), Some(481));
        assert_eq!(parse_number("four hundred and eighty-one"), Some(481));
        assert_eq!(parse_number("thirteen hundred thirty"), Some(1330));
        assert_eq!(
            parse_number("one thousand three hundred thirty"),
            Some(1330)
        );
        assert_eq!(
            parse_number("one thousand three hundred and thirty"),
            Some(1330)
        );
        assert_eq!(parse_number("two thousand"), Some(2000));
        assert_eq!(parse_number("one two"), None);
    }

    #[test]
    fn parses_tamil_cardinal_and_compound_numbers() {
        assert_eq!(parse_number("பூஜ்யம்"), Some(0));
        assert_eq!(parse_number("ஒன்று"), Some(1));
        assert_eq!(parse_number("இரண்டு"), Some(2));
        assert_eq!(parse_number("மூன்று"), Some(3));
        assert_eq!(parse_number("நான்கு"), Some(4));
        assert_eq!(parse_number("ஐந்து"), Some(5));
        assert_eq!(parse_number("ஆறு"), Some(6));
        assert_eq!(parse_number("ஏழு"), Some(7));
        assert_eq!(parse_number("எட்டு"), Some(8));
        assert_eq!(parse_number("ஒன்பது"), Some(9));
        assert_eq!(parse_number("பத்து"), Some(10));
        assert_eq!(parse_number("நூறு"), Some(100));
        assert_eq!(parse_number("ஒரு நூறு"), Some(100));
        assert_eq!(parse_number("நூற்று முப்பத்து மூன்று"), Some(133));
        assert_eq!(parse_number("நூற்று முப்பத்து நான்கு"), Some(134));
        assert_eq!(parse_number("ஆயிரம்"), Some(1000));
        assert_eq!(parse_number("ஒரு ஆயிரம்"), Some(1000));
        assert_eq!(parse_number("ஆயிரத்து நூற்று முப்பது"), Some(1130));
        assert_eq!(parse_number("நானூற்று எண்பத்தொன்று"), Some(481));
        assert_eq!(parse_number("நானூற்றெண்பத்தொன்று"), Some(481));
        assert_eq!(parse_number("ஆயிரத்து முந்நூற்று முப்பது"), Some(1330));
        assert_eq!(parse_number("ஆயிரத்து முந்நூற்று முப்பத்தொன்று"), Some(1331));
        assert_eq!(parse_number("இரண்டாயிரம்"), Some(2000));
        assert_eq!(parse_number("பத்தாவது"), Some(10));
        assert_eq!(parse_number("நான்காம்"), Some(4));
        assert_eq!(parse_number("ஒரு கதை"), None);
    }

    #[test]
    fn parses_quiz_choices_zero_to_three() {
        for (input, expected) in [
            ("a", 0),
            ("1", 0),
            ("first", 0),
            ("ஒன்று", 0),
            ("அ", 0),
            ("b", 1),
            ("2", 1),
            ("second", 1),
            ("இரண்டு", 1),
            ("ஆ", 1),
            ("c", 2),
            ("3", 2),
            ("third", 2),
            ("மூன்று", 2),
            ("இ", 2),
            ("d", 3),
            ("4", 3),
            ("fourth", 3),
            ("நான்கு", 3),
            ("ஈ", 3),
            ("option B", 1),
            ("விடை ஈ", 3),
        ] {
            assert_eq!(
                parse_quiz_choice(input),
                Some(expected),
                "failed on {input}"
            );
        }
        assert_eq!(parse_quiz_choice("0"), None);
        assert_eq!(parse_quiz_choice("5"), None);
        assert_eq!(parse_quiz_choice("e"), None);
    }

    #[test]
    fn fuzzy_matcher_respects_threshold_and_min_length() {
        assert!(fuzzy_token_matches("kurl", "kural")); // 4/5 = 0.80
        assert!(fuzzy_token_matches("soloman", "solomon"));
        assert!(fuzzy_token_matches("parimelazagar", "parimelazhagar"));
        assert!(!fuzzy_token_matches("kids", "bids")); // 3/4 = 0.75
        assert!(!fuzzy_token_matches("ai", "am")); // < 4 chars
        assert!(!fuzzy_token_matches("k", "kural")); // < 4 chars
        assert!(fuzzy_phrase_matches("dark mod", "dark mode"));
        assert!(fuzzy_phrase_matches("clear flter", "clear filter"));
    }

    #[test]
    fn anchored_number_extraction_ignores_math_and_unanchored_sentences() {
        assert_eq!(
            extract_anchored_number("kural 481"),
            Some(AnchoredNumber::Kural(481))
        );
        assert_eq!(
            extract_anchored_number("குறள் 1331"),
            Some(AnchoredNumber::Kural(1331))
        );
        assert_eq!(
            extract_anchored_number("adhikaram 134"),
            Some(AnchoredNumber::Adhikaram(134))
        );
        assert_eq!(
            extract_anchored_number("அதிகாரம் 0"),
            Some(AnchoredNumber::Adhikaram(0))
        );
        assert_eq!(
            extract_anchored_number("481"),
            Some(AnchoredNumber::Bare(481))
        );
        assert_eq!(extract_anchored_number("42 * 10"), None);
        assert_eq!(extract_anchored_number("what happened in 2025"), None);
    }

    #[test]
    fn parses_tamil_sandhi_compounds_and_all_kural_number_forms() {
        assert_eq!(parse_number("நாற்பத்திரண்டு"), Some(42));
        assert_eq!(parse_number("இருபத்திரண்டு"), Some(22));
        assert_eq!(parse_number("நூற்றிரண்டு"), Some(102));
        assert_eq!(parse_number("நூற்றிருபது"), Some(120));

        for (utterance, expected) in [
            ("குறள் நாற்பத்திரண்டு", 42),
            ("குறள் ஒன்று", 1),
            ("குறள் இரண்டு", 2),
            ("குறள் பத்து", 10),
            ("குறள் நூறு", 100),
            ("read kural 42", 42),
            ("read 42", 42),
            ("குறள் 42 படி", 42),
            ("42 படி", 42),
            ("42 படியுங்கள்", 42),
            ("42 சொல்", 42),
            ("42 சொல்லு", 42),
            ("42 சொல்லுங்கள்", 42),
            ("42 வது குறள்", 42),
            ("குறள் எண் 42", 42),
            ("number 42", 42),
            ("open kural 42", 42),
            ("kural one", 1),
            ("kural forty two", 42),
            ("kural four two", 42),
            ("kural one hundred and five", 105),
            ("kural four hundred twenty three", 423),
            ("kural one twenty eight", 128),
            ("URAL ONE HUNDRED TWENTY EIGHT", 128),
            ("D CURL FORTY TWO", 42),
            ("குரல் 128", 128),
            ("குரல் நூற்று இருபத்தெட்டு", 128),
            ("குறள் நம்பர் 128", 128),
            ("play kural 128", 128),
            ("128 வாசி", 128),
        ] {
            assert_eq!(
                extract_anchored_number(utterance),
                Some(AnchoredNumber::Kural(expected)),
                "failed on {utterance}"
            );
        }
    }

    #[test]
    fn parses_live_asr_tamil_colloquial_transliterated_and_shorthand_numbers() {
        for (input, expected) in [
            ("நூற்ற இருபத்தெட்டு", 128),
            ("நூற்று இருபத்தி எட்டு", 128),
            ("நூத்தி இருபத்தெட்டு", 128),
            ("நூற்றியிருபத்தெட்டு", 128),
            ("நூற்றியெட்டு", 108),
            ("நூற்றியொன்று", 101),
            ("ஒன் ஹண்ட்ரட் டவெண்டி எயிட்", 128),
            ("ஒன் டுூ வெயிட்", 128),
            ("ஒன்று இரண்டு எட்டு", 128),
            ("one twenty eight", 128),
            ("one two eight", 128),
            ("one oh five", 105),
            ("one zero eight", 108),
            ("four twenty three", 423),
            ("thirteen thirty", 1330),
            ("one three three zero", 1330),
        ] {
            assert_eq!(parse_number(input), Some(expected), "failed on '{input}'");
            assert_eq!(
                extract_anchored_number(input),
                Some(AnchoredNumber::Bare(expected)),
                "bare extract failed on '{input}'"
            );
        }

        // Contextual ASR homophones must NOT hijack ordinary Tamil/English words alone.
        assert_eq!(parse_number("போர்"), None);
        assert_eq!(parse_number("வெயிட்"), None);
        assert_eq!(parse_number("oh"), None);
        assert_eq!(extract_anchored_number("போர் பற்றி கூறு"), None);
    }
}
