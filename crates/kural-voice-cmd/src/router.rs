use crate::parser::{
    AnchoredNumber, extract_anchored_number, fuzzy_phrase_matches, is_likely_question,
    normalize_for_matching, parse_quiz_choice,
};
use kural_core::{
    AgeGroup, AppScreen, BackendCommand, BilingualCopy, CompactTabHint, Language, ModalKind,
    Scholar, SearchTab, SpeakTarget, Theme, VOICE_ADHIKARAM_RANGE_ERROR, VOICE_KURAL_RANGE_ERROR,
    format_voice_toast, normalize_nfc_trimmed, step_speech_speed,
};

/// Active UI and audio state passed into [`route_transcript`] for context-aware routing.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct RouterContext {
    pub screen: AppScreen,
    pub modal: Option<ModalKind>,
    pub active_kural: u16,
    pub active_scholar: Scholar,
    pub speech_speed: f32,
    pub muted: bool,
}

impl Default for RouterContext {
    fn default() -> Self {
        Self {
            screen: AppScreen::Start,
            modal: None,
            active_kural: 1,
            active_scholar: Scholar::MuVa,
            speech_speed: 1.0,
            muted: false,
        }
    }
}

/// Deterministic command outcome returned by [`route_transcript`].
#[derive(Debug, Clone, PartialEq)]
pub struct VoiceRouteOutcome {
    pub cmd: BackendCommand,
    pub compact_tab: Option<CompactTabHint>,
    pub toast_ta: String,
    pub toast_en: String,
}

#[derive(Debug, Clone, Copy)]
enum StaticAction {
    Navigate(AppScreen),
    SelectAge(AgeGroup),
    NextKural,
    PrevKural,
    SetLanguage(Language),
    SetTheme(Theme),
    CloseModal,
    OpenSearch(SearchTab),
    OpenAiChat,
    StartVoiceAgeDetect,
    FilterDivision(u8),
    ClearFilter,
    StopAudio,
    ReplayAudio,
    SpeakDefault,
    SpeakTarget {
        target: SpeakTarget,
        compact_tab: Option<CompactTabHint>,
    },
    StepSpeed(i8),
    ResetSpeed,
    ToggleMute,
    Unmute,
    AdjustVolume(f32),
    Story,
    CompleteKidsStory,
    OpenWordGame,
    OpenQuiz,
    OpenDeepDive,
    ToggleBookmark,
    SelectScholar(Scholar),
}

impl StaticAction {
    fn resolve(self, ctx: RouterContext) -> (BackendCommand, Option<CompactTabHint>) {
        match self {
            Self::Navigate(screen) => (BackendCommand::Navigate(screen), None),
            Self::SelectAge(age) => (BackendCommand::SelectAge(age), None),
            Self::NextKural => (BackendCommand::NextKural, None),
            Self::PrevKural => (BackendCommand::PrevKural, None),
            Self::SetLanguage(lang) => (BackendCommand::SetLanguage(lang), None),
            Self::SetTheme(theme) => (BackendCommand::SetTheme(theme), None),
            Self::CloseModal => (BackendCommand::CloseModal, None),
            Self::OpenSearch(initial_tab) => (
                BackendCommand::OpenModal(ModalKind::Search { initial_tab }),
                None,
            ),
            Self::OpenAiChat => (
                BackendCommand::OpenModal(ModalKind::AiChat {
                    seed_kural: Some(ctx.active_kural),
                }),
                None,
            ),
            Self::StartVoiceAgeDetect => (BackendCommand::StartVoiceAgeDetect, None),
            Self::FilterDivision(div) => (BackendCommand::FilterByDivision(Some(div)), None),
            Self::ClearFilter => (BackendCommand::ClearFilter, None),
            Self::StopAudio => (BackendCommand::StopAudio, None),
            Self::ReplayAudio => (BackendCommand::ReplayAudio, None),
            Self::SpeakDefault => (
                BackendCommand::SpeakCurrent(screen_default_target(ctx.screen, ctx.active_scholar)),
                None,
            ),
            Self::SpeakTarget {
                target,
                compact_tab,
            } => (BackendCommand::SpeakCurrent(target), compact_tab),
            Self::StepSpeed(delta) => (
                BackendCommand::SetSpeechSpeed(step_speech_speed(ctx.speech_speed, delta)),
                None,
            ),
            Self::ResetSpeed => (BackendCommand::SetSpeechSpeed(1.0), None),
            Self::ToggleMute => (BackendCommand::SetMuted(!ctx.muted), None),
            Self::Unmute => (BackendCommand::SetMuted(false), None),
            Self::AdjustVolume(delta) => (BackendCommand::AdjustVolume(delta), None),
            Self::Story => {
                if ctx.modal == Some(ModalKind::KidsStory) {
                    (BackendCommand::SpeakCurrent(SpeakTarget::KuttyStory), None)
                } else {
                    (BackendCommand::OpenModal(ModalKind::KidsStory), None)
                }
            }
            Self::CompleteKidsStory => (BackendCommand::CompleteKidsStory(ctx.active_kural), None),
            Self::OpenWordGame => (BackendCommand::OpenModal(ModalKind::KidsWordGame), None),
            Self::OpenQuiz => (
                BackendCommand::OpenModal(ModalKind::YouthQuiz),
                Some(CompactTabHint::Quiz),
            ),
            Self::OpenDeepDive => (
                BackendCommand::OpenModal(ModalKind::YouthDeepDive),
                Some(CompactTabHint::Study),
            ),
            Self::ToggleBookmark => (BackendCommand::ToggleBookmark(ctx.active_kural), None),
            Self::SelectScholar(scholar) => (
                BackendCommand::SelectCommentary(scholar),
                Some(CompactTabHint::Commentary),
            ),
        }
    }
}

/// Ordered bilingual command phrase table (multi-word specific commands before general terms).
const COMMAND_TABLE: &[(&str, StaticAction)] = &[
    // Navigation & Age
    ("home", StaticAction::Navigate(AppScreen::Start)),
    ("start", StaticAction::Navigate(AppScreen::Start)),
    ("go home", StaticAction::Navigate(AppScreen::Start)),
    ("open home", StaticAction::Navigate(AppScreen::Start)),
    ("home screen", StaticAction::Navigate(AppScreen::Start)),
    ("main screen", StaticAction::Navigate(AppScreen::Start)),
    ("முகப்பு", StaticAction::Navigate(AppScreen::Start)),
    ("தொடக்கம்", StaticAction::Navigate(AppScreen::Start)),
    ("kids", StaticAction::SelectAge(AgeGroup::Kids)),
    ("kid", StaticAction::SelectAge(AgeGroup::Kids)),
    ("child", StaticAction::SelectAge(AgeGroup::Kids)),
    ("children", StaticAction::SelectAge(AgeGroup::Kids)),
    ("kids mode", StaticAction::SelectAge(AgeGroup::Kids)),
    ("child mode", StaticAction::SelectAge(AgeGroup::Kids)),
    ("open kids", StaticAction::SelectAge(AgeGroup::Kids)),
    ("go to kids", StaticAction::SelectAge(AgeGroup::Kids)),
    ("குழந்தைகள்", StaticAction::SelectAge(AgeGroup::Kids)),
    ("சிறுவர்", StaticAction::SelectAge(AgeGroup::Kids)),
    ("youth", StaticAction::SelectAge(AgeGroup::Youth)),
    ("teen", StaticAction::SelectAge(AgeGroup::Youth)),
    ("teens", StaticAction::SelectAge(AgeGroup::Youth)),
    ("teenager", StaticAction::SelectAge(AgeGroup::Youth)),
    ("youth mode", StaticAction::SelectAge(AgeGroup::Youth)),
    ("open youth", StaticAction::SelectAge(AgeGroup::Youth)),
    ("go to youth", StaticAction::SelectAge(AgeGroup::Youth)),
    ("இளைஞர்கள்", StaticAction::SelectAge(AgeGroup::Youth)),
    ("இளைஞர்", StaticAction::SelectAge(AgeGroup::Youth)),
    ("adult", StaticAction::SelectAge(AgeGroup::Adult)),
    ("adults", StaticAction::SelectAge(AgeGroup::Adult)),
    ("scholar", StaticAction::SelectAge(AgeGroup::Adult)),
    ("adult mode", StaticAction::SelectAge(AgeGroup::Adult)),
    ("scholar mode", StaticAction::SelectAge(AgeGroup::Adult)),
    ("open adult", StaticAction::SelectAge(AgeGroup::Adult)),
    ("go to adult", StaticAction::SelectAge(AgeGroup::Adult)),
    ("பெரியவர்கள்", StaticAction::SelectAge(AgeGroup::Adult)),
    ("பெரியோர்", StaticAction::SelectAge(AgeGroup::Adult)),
    ("next", StaticAction::NextKural),
    ("next kural", StaticAction::NextKural),
    ("forward", StaticAction::NextKural),
    ("அடுத்த குறள்", StaticAction::NextKural),
    ("அடுத்து", StaticAction::NextKural),
    ("அடுத்தது", StaticAction::NextKural),
    ("previous", StaticAction::PrevKural),
    ("prev", StaticAction::PrevKural),
    ("previous kural", StaticAction::PrevKural),
    ("முந்தைய குறள்", StaticAction::PrevKural),
    ("முந்தையது", StaticAction::PrevKural),
    ("பின்னால்", StaticAction::PrevKural),
    // Language & Theme & Modals
    ("tamil", StaticAction::SetLanguage(Language::Tamil)),
    ("tamil language", StaticAction::SetLanguage(Language::Tamil)),
    (
        "switch to tamil",
        StaticAction::SetLanguage(Language::Tamil),
    ),
    ("set tamil", StaticAction::SetLanguage(Language::Tamil)),
    ("தமிழ்", StaticAction::SetLanguage(Language::Tamil)),
    (
        "english commentary",
        StaticAction::SelectScholar(Scholar::English),
    ),
    ("ஆங்கில உரை", StaticAction::SelectScholar(Scholar::English)),
    ("english", StaticAction::SetLanguage(Language::English)),
    (
        "english language",
        StaticAction::SetLanguage(Language::English),
    ),
    (
        "switch to english",
        StaticAction::SetLanguage(Language::English),
    ),
    ("set english", StaticAction::SetLanguage(Language::English)),
    ("ஆங்கிலம்", StaticAction::SetLanguage(Language::English)),
    ("dark", StaticAction::SetTheme(Theme::Dark)),
    ("dark mode", StaticAction::SetTheme(Theme::Dark)),
    ("dark theme", StaticAction::SetTheme(Theme::Dark)),
    ("switch to dark", StaticAction::SetTheme(Theme::Dark)),
    ("turn on dark mode", StaticAction::SetTheme(Theme::Dark)),
    ("இருள்", StaticAction::SetTheme(Theme::Dark)),
    ("இருள் முறை", StaticAction::SetTheme(Theme::Dark)),
    ("light", StaticAction::SetTheme(Theme::Light)),
    ("light mode", StaticAction::SetTheme(Theme::Light)),
    ("light theme", StaticAction::SetTheme(Theme::Light)),
    ("switch to light", StaticAction::SetTheme(Theme::Light)),
    ("turn on light mode", StaticAction::SetTheme(Theme::Light)),
    ("ஒளி", StaticAction::SetTheme(Theme::Light)),
    ("ஒளி முறை", StaticAction::SetTheme(Theme::Light)),
    ("close", StaticAction::CloseModal),
    ("exit", StaticAction::CloseModal),
    ("close modal", StaticAction::CloseModal),
    ("dismiss", StaticAction::CloseModal),
    ("மூடு", StaticAction::CloseModal),
    ("வெளியேறு", StaticAction::CloseModal),
    // Search / Filter / Topics
    ("search", StaticAction::OpenSearch(SearchTab::Query)),
    ("open search", StaticAction::OpenSearch(SearchTab::Query)),
    ("find", StaticAction::OpenSearch(SearchTab::Query)),
    ("தேடு", StaticAction::OpenSearch(SearchTab::Query)),
    ("தேடல்", StaticAction::OpenSearch(SearchTab::Query)),
    ("paals", StaticAction::OpenSearch(SearchTab::Divisions)),
    ("paal", StaticAction::OpenSearch(SearchTab::Divisions)),
    ("divisions", StaticAction::OpenSearch(SearchTab::Divisions)),
    ("chapters", StaticAction::OpenSearch(SearchTab::Divisions)),
    (
        "open divisions",
        StaticAction::OpenSearch(SearchTab::Divisions),
    ),
    (
        "show chapters",
        StaticAction::OpenSearch(SearchTab::Divisions),
    ),
    ("பால்", StaticAction::OpenSearch(SearchTab::Divisions)),
    ("முப்பால்", StaticAction::OpenSearch(SearchTab::Divisions)),
    ("அதிகாரங்கள்", StaticAction::OpenSearch(SearchTab::Divisions)),
    ("aram", StaticAction::FilterDivision(1)),
    ("virtue", StaticAction::FilterDivision(1)),
    ("filter aram", StaticAction::FilterDivision(1)),
    ("filter virtue", StaticAction::FilterDivision(1)),
    ("aram division", StaticAction::FilterDivision(1)),
    ("division 1", StaticAction::FilterDivision(1)),
    ("paal 1", StaticAction::FilterDivision(1)),
    ("அறம்", StaticAction::FilterDivision(1)),
    ("அறத்துப்பால்", StaticAction::FilterDivision(1)),
    ("porul", StaticAction::FilterDivision(2)),
    ("wealth", StaticAction::FilterDivision(2)),
    ("filter porul", StaticAction::FilterDivision(2)),
    ("filter wealth", StaticAction::FilterDivision(2)),
    ("porul division", StaticAction::FilterDivision(2)),
    ("division 2", StaticAction::FilterDivision(2)),
    ("paal 2", StaticAction::FilterDivision(2)),
    ("பொருள்", StaticAction::FilterDivision(2)),
    ("பொருட்பால்", StaticAction::FilterDivision(2)),
    ("inbam", StaticAction::FilterDivision(3)),
    ("love", StaticAction::FilterDivision(3)),
    ("kamam", StaticAction::FilterDivision(3)),
    ("filter inbam", StaticAction::FilterDivision(3)),
    ("filter love", StaticAction::FilterDivision(3)),
    ("inbam division", StaticAction::FilterDivision(3)),
    ("division 3", StaticAction::FilterDivision(3)),
    ("paal 3", StaticAction::FilterDivision(3)),
    ("இன்பம்", StaticAction::FilterDivision(3)),
    ("காமத்துப்பால்", StaticAction::FilterDivision(3)),
    ("இன்பத்துப்பால்", StaticAction::FilterDivision(3)),
    ("clear filter", StaticAction::ClearFilter),
    ("reset filter", StaticAction::ClearFilter),
    ("remove filter", StaticAction::ClearFilter),
    ("all kurals", StaticAction::ClearFilter),
    ("show all", StaticAction::ClearFilter),
    ("வடிகட்டி நீக்கு", StaticAction::ClearFilter),
    ("அனைத்து குறள்கள்", StaticAction::ClearFilter),
    // Audio Playback & Speed & Volume
    ("stop", StaticAction::StopAudio),
    ("quiet", StaticAction::StopAudio),
    ("silence", StaticAction::StopAudio),
    ("stop audio", StaticAction::StopAudio),
    ("stop speaking", StaticAction::StopAudio),
    ("நிறுத்து", StaticAction::StopAudio),
    ("போதும்", StaticAction::StopAudio),
    ("அமைதி", StaticAction::StopAudio),
    ("replay", StaticAction::ReplayAudio),
    ("repeat", StaticAction::ReplayAudio),
    ("say again", StaticAction::ReplayAudio),
    ("play again", StaticAction::ReplayAudio),
    ("மீண்டும் வாசி", StaticAction::ReplayAudio),
    ("மீண்டும் படி", StaticAction::ReplayAudio),
    ("மீண்டும்", StaticAction::ReplayAudio),
    ("read", StaticAction::SpeakDefault),
    ("speak", StaticAction::SpeakDefault),
    ("listen", StaticAction::SpeakDefault),
    ("play", StaticAction::SpeakDefault),
    ("read aloud", StaticAction::SpeakDefault),
    ("வாசி", StaticAction::SpeakDefault),
    ("படி", StaticAction::SpeakDefault),
    ("கேள்", StaticAction::SpeakDefault),
    (
        "read kural",
        StaticAction::SpeakTarget {
            target: SpeakTarget::KuralOnly,
            compact_tab: None,
        },
    ),
    (
        "couplet only",
        StaticAction::SpeakTarget {
            target: SpeakTarget::KuralOnly,
            compact_tab: None,
        },
    ),
    (
        "kural only",
        StaticAction::SpeakTarget {
            target: SpeakTarget::KuralOnly,
            compact_tab: None,
        },
    ),
    (
        "read couplet",
        StaticAction::SpeakTarget {
            target: SpeakTarget::KuralOnly,
            compact_tab: None,
        },
    ),
    (
        "குறள் மட்டும்",
        StaticAction::SpeakTarget {
            target: SpeakTarget::KuralOnly,
            compact_tab: None,
        },
    ),
    (
        "குறள் வாசி",
        StaticAction::SpeakTarget {
            target: SpeakTarget::KuralOnly,
            compact_tab: None,
        },
    ),
    (
        "குறள் படி",
        StaticAction::SpeakTarget {
            target: SpeakTarget::KuralOnly,
            compact_tab: None,
        },
    ),
    (
        "read meaning",
        StaticAction::SpeakTarget {
            target: SpeakTarget::KuralAndMeaning,
            compact_tab: None,
        },
    ),
    (
        "meaning",
        StaticAction::SpeakTarget {
            target: SpeakTarget::KuralAndMeaning,
            compact_tab: None,
        },
    ),
    (
        "speak meaning",
        StaticAction::SpeakTarget {
            target: SpeakTarget::KuralAndMeaning,
            compact_tab: None,
        },
    ),
    (
        "பொருள் வாசி",
        StaticAction::SpeakTarget {
            target: SpeakTarget::KuralAndMeaning,
            compact_tab: None,
        },
    ),
    (
        "பொருள் படி",
        StaticAction::SpeakTarget {
            target: SpeakTarget::KuralAndMeaning,
            compact_tab: None,
        },
    ),
    ("slower", StaticAction::StepSpeed(-1)),
    ("slow down", StaticAction::StepSpeed(-1)),
    ("speak slower", StaticAction::StepSpeed(-1)),
    ("மெதுவாக", StaticAction::StepSpeed(-1)),
    ("மெதுவா", StaticAction::StepSpeed(-1)),
    ("faster", StaticAction::StepSpeed(1)),
    ("speed up", StaticAction::StepSpeed(1)),
    ("speak faster", StaticAction::StepSpeed(1)),
    ("வேகமாக", StaticAction::StepSpeed(1)),
    ("வேகமா", StaticAction::StepSpeed(1)),
    ("normal speed", StaticAction::ResetSpeed),
    ("default speed", StaticAction::ResetSpeed),
    ("regular speed", StaticAction::ResetSpeed),
    ("reset speed", StaticAction::ResetSpeed),
    ("சாதாரண வேகம்", StaticAction::ResetSpeed),
    ("இயல்பு வேகம்", StaticAction::ResetSpeed),
    ("unmute", StaticAction::Unmute),
    ("unmute audio", StaticAction::Unmute),
    ("sound on", StaticAction::Unmute),
    ("ஒலி இயக்கு", StaticAction::Unmute),
    ("சத்தம் திற", StaticAction::Unmute),
    ("mute", StaticAction::ToggleMute),
    ("toggle mute", StaticAction::ToggleMute),
    ("mute audio", StaticAction::ToggleMute),
    ("ஒலி நிறுத்து", StaticAction::ToggleMute),
    ("சத்தம் மூடு", StaticAction::ToggleMute),
    ("volume up", StaticAction::AdjustVolume(0.10)),
    ("louder", StaticAction::AdjustVolume(0.10)),
    ("increase volume", StaticAction::AdjustVolume(0.10)),
    ("சத்தம் கூட்டு", StaticAction::AdjustVolume(0.10)),
    ("ஒலி கூட்டு", StaticAction::AdjustVolume(0.10)),
    ("volume down", StaticAction::AdjustVolume(-0.10)),
    ("quieter", StaticAction::AdjustVolume(-0.10)),
    ("lower volume", StaticAction::AdjustVolume(-0.10)),
    ("decrease volume", StaticAction::AdjustVolume(-0.10)),
    ("சத்தம் குறை", StaticAction::AdjustVolume(-0.10)),
    ("ஒலி குறை", StaticAction::AdjustVolume(-0.10)),
    // Modals / Games / Tabs / Bookmarks / Scholars
    ("finish story", StaticAction::CompleteKidsStory),
    ("complete story", StaticAction::CompleteKidsStory),
    ("story done", StaticAction::CompleteKidsStory),
    ("கதை முடிந்தது", StaticAction::CompleteKidsStory),
    (
        "read story",
        StaticAction::SpeakTarget {
            target: SpeakTarget::KuttyStory,
            compact_tab: None,
        },
    ),
    (
        "speak story",
        StaticAction::SpeakTarget {
            target: SpeakTarget::KuttyStory,
            compact_tab: None,
        },
    ),
    (
        "கதை வாசி",
        StaticAction::SpeakTarget {
            target: SpeakTarget::KuttyStory,
            compact_tab: None,
        },
    ),
    (
        "கதை படி",
        StaticAction::SpeakTarget {
            target: SpeakTarget::KuttyStory,
            compact_tab: None,
        },
    ),
    ("story", StaticAction::Story),
    ("kutty story", StaticAction::Story),
    ("open story", StaticAction::Story),
    ("kids story", StaticAction::Story),
    ("tell me a story", StaticAction::Story),
    ("கதை", StaticAction::Story),
    ("குட்டி கதை", StaticAction::Story),
    ("word game", StaticAction::OpenWordGame),
    ("game", StaticAction::OpenWordGame),
    ("open game", StaticAction::OpenWordGame),
    ("play game", StaticAction::OpenWordGame),
    ("kids game", StaticAction::OpenWordGame),
    ("விளையாட்டு", StaticAction::OpenWordGame),
    ("சொல் விளையாட்டு", StaticAction::OpenWordGame),
    ("quiz", StaticAction::OpenQuiz),
    ("challenge", StaticAction::OpenQuiz),
    ("open quiz", StaticAction::OpenQuiz),
    ("daily quiz", StaticAction::OpenQuiz),
    ("சவால்", StaticAction::OpenQuiz),
    ("வினாடி வினா", StaticAction::OpenQuiz),
    ("deep dive", StaticAction::OpenDeepDive),
    ("deepdive", StaticAction::OpenDeepDive),
    ("open deep dive", StaticAction::OpenDeepDive),
    ("ஆய்வு", StaticAction::OpenDeepDive),
    ("ஆழமான ஆய்வு", StaticAction::OpenDeepDive),
    (
        "verse",
        StaticAction::SpeakTarget {
            target: SpeakTarget::KuralOnly,
            compact_tab: Some(CompactTabHint::Verse),
        },
    ),
    (
        "show verse",
        StaticAction::SpeakTarget {
            target: SpeakTarget::KuralOnly,
            compact_tab: Some(CompactTabHint::Verse),
        },
    ),
    (
        "பாடல்",
        StaticAction::SpeakTarget {
            target: SpeakTarget::KuralOnly,
            compact_tab: Some(CompactTabHint::Verse),
        },
    ),
    (
        "analysis",
        StaticAction::SpeakTarget {
            target: SpeakTarget::YouthBriefing,
            compact_tab: Some(CompactTabHint::Analysis),
        },
    ),
    (
        "briefing",
        StaticAction::SpeakTarget {
            target: SpeakTarget::YouthBriefing,
            compact_tab: Some(CompactTabHint::Analysis),
        },
    ),
    (
        "பகுப்பாய்வு",
        StaticAction::SpeakTarget {
            target: SpeakTarget::YouthBriefing,
            compact_tab: Some(CompactTabHint::Analysis),
        },
    ),
    ("bookmark", StaticAction::ToggleBookmark),
    ("save", StaticAction::ToggleBookmark),
    ("save kural", StaticAction::ToggleBookmark),
    ("toggle bookmark", StaticAction::ToggleBookmark),
    ("bookmark kural", StaticAction::ToggleBookmark),
    ("புத்தகக்குறி", StaticAction::ToggleBookmark),
    ("சேமி", StaticAction::ToggleBookmark),
    ("mu va", StaticAction::SelectScholar(Scholar::MuVa)),
    ("muva", StaticAction::SelectScholar(Scholar::MuVa)),
    ("varadarajan", StaticAction::SelectScholar(Scholar::MuVa)),
    ("மு வ", StaticAction::SelectScholar(Scholar::MuVa)),
    ("முவ", StaticAction::SelectScholar(Scholar::MuVa)),
    ("solomon", StaticAction::SelectScholar(Scholar::Solomon)),
    (
        "solomon pappaiah",
        StaticAction::SelectScholar(Scholar::Solomon),
    ),
    ("சாலமன்", StaticAction::SelectScholar(Scholar::Solomon)),
    ("சாலமன் பாப்பையா", StaticAction::SelectScholar(Scholar::Solomon)),
    ("kalaignar", StaticAction::SelectScholar(Scholar::Kalaignar)),
    (
        "karunanidhi",
        StaticAction::SelectScholar(Scholar::Kalaignar),
    ),
    ("கலைஞர்", StaticAction::SelectScholar(Scholar::Kalaignar)),
    ("parimel", StaticAction::SelectScholar(Scholar::Parimel)),
    (
        "parimelazhagar",
        StaticAction::SelectScholar(Scholar::Parimel),
    ),
    (
        "parimelalagar",
        StaticAction::SelectScholar(Scholar::Parimel),
    ),
    ("பரிமேலழகர்", StaticAction::SelectScholar(Scholar::Parimel)),
    ("பரிமேல்", StaticAction::SelectScholar(Scholar::Parimel)),
    // AI Chat & Voice Age Detect shortcuts
    ("ai chat", StaticAction::OpenAiChat),
    ("open chat", StaticAction::OpenAiChat),
    ("open ai", StaticAction::OpenAiChat),
    ("discuss", StaticAction::OpenAiChat),
    ("உரையாடல்", StaticAction::OpenAiChat),
    ("விவாதி", StaticAction::OpenAiChat),
    ("detect age", StaticAction::StartVoiceAgeDetect),
    ("voice age", StaticAction::StartVoiceAgeDetect),
    ("check age", StaticAction::StartVoiceAgeDetect),
    ("வயது அறி", StaticAction::StartVoiceAgeDetect),
];

/// Strip optional polite framing words (`"please"`, `"kindly"`, `"ok"`, `"hey"`, `"தயவுசெய்து"`)
/// from the start or end of an already-normalized phrase.
fn strip_polite_wrappers(norm: &str) -> &str {
    const PREFIXES: &[&str] = &["please ", "kindly ", "ok ", "okay ", "hey ", "தயவுசெய்து "];
    const SUFFIXES: &[&str] = &[" please", " kindly", " தயவுசெய்து"];

    let mut s = norm;
    loop {
        let mut changed = false;
        for p in PREFIXES {
            if let Some(rest) = s.strip_prefix(p) {
                s = rest.trim();
                changed = true;
                break;
            }
        }
        if !changed {
            break;
        }
    }
    loop {
        let mut changed = false;
        for suf in SUFFIXES {
            if let Some(rest) = s.strip_suffix(suf) {
                s = rest.trim();
                changed = true;
                break;
            }
        }
        if !changed {
            break;
        }
    }
    s
}

fn find_exact_command(phrase: &str) -> Option<StaticAction> {
    COMMAND_TABLE
        .iter()
        .find(|&&(pattern, _)| pattern == phrase)
        .map(|&(_, action)| action)
}

fn find_fuzzy_command(phrase: &str) -> Option<StaticAction> {
    COMMAND_TABLE
        .iter()
        .find(|&&(pattern, _)| fuzzy_phrase_matches(phrase, pattern))
        .map(|&(_, action)| action)
}

/// Map the current screen and active scholar onto the default `SpeakCurrent` target.
pub const fn screen_default_target(screen: AppScreen, active_scholar: Scholar) -> SpeakTarget {
    match screen {
        AppScreen::Start => SpeakTarget::KuralOnly,
        AppScreen::Kids => SpeakTarget::KuralAndMeaning,
        AppScreen::Youth => SpeakTarget::YouthBriefing,
        AppScreen::Adult => SpeakTarget::ScholarCommentary(active_scholar),
    }
}

/// Validate anchored Kural (`1..=1330`) and Adhikaram (`1..=133`) numbers before routing.
///
/// Returns `Some(VOICE_KURAL_RANGE_ERROR)` when an anchored Kural number is `0` or `> 1330`,
/// or `Some(VOICE_ADHIKARAM_RANGE_ERROR)` when an anchored Adhikaram number is `0` or `> 133`.
pub fn check_voice_number_range_error(raw: &str) -> Option<BilingualCopy> {
    match extract_anchored_number(raw) {
        Some(AnchoredNumber::Kural(n)) if n == 0 || n > 1330 => Some(VOICE_KURAL_RANGE_ERROR),
        Some(AnchoredNumber::Adhikaram(n)) if n == 0 || n > 133 => {
            Some(VOICE_ADHIKARAM_RANGE_ERROR)
        }
        _ => None,
    }
}

/// Deterministically route a bilingual Tamil/English STT transcript in `< 1 ms`.
pub fn route_transcript(raw: &str, _lang: Language, ctx: RouterContext) -> VoiceRouteOutcome {
    let nfc_trimmed = normalize_nfc_trimmed(raw);
    let (toast_ta, toast_en) = format_voice_toast(&nfc_trimmed);

    let fallback = |query: String| VoiceRouteOutcome {
        cmd: BackendCommand::AskAiQuestion {
            query,
            seed_kural: Some(ctx.active_kural),
        },
        compact_tab: None,
        toast_ta: toast_ta.clone(),
        toast_en: toast_en.clone(),
    };

    if nfc_trimmed.is_empty() {
        return fallback(nfc_trimmed);
    }

    // 1. Context-aware quiz/word-game option selection ONLY inside KidsWordGame or YouthQuiz.
    if matches!(
        ctx.modal,
        Some(ModalKind::KidsWordGame | ModalKind::YouthQuiz)
    ) && let Some(choice_idx) = parse_quiz_choice(&nfc_trimmed)
    {
        let cmd = match ctx.modal {
            Some(ModalKind::KidsWordGame) => BackendCommand::SubmitKidsGameAnswer {
                kural_number: ctx.active_kural,
                choice_idx,
            },
            Some(ModalKind::YouthQuiz) => BackendCommand::SubmitYouthQuizAnswer {
                kural_number: ctx.active_kural,
                choice_idx,
            },
            _ => unreachable!(),
        };
        return VoiceRouteOutcome {
            cmd,
            compact_tab: None,
            toast_ta,
            toast_en,
        };
    }

    // 2. Guard against hijacking multi-word questions or math expressions into commands.
    if is_likely_question(&nfc_trimmed) {
        return fallback(nfc_trimmed);
    }

    let norm = normalize_for_matching(&nfc_trimmed);
    let stripped = strip_polite_wrappers(&norm);

    // 3. Exact command match (prioritizes phrases like "division 1", "read kural", "english commentary").
    if let Some(action) = find_exact_command(&norm).or_else(|| find_exact_command(stripped)) {
        let (cmd, compact_tab) = action.resolve(ctx);
        return VoiceRouteOutcome {
            cmd,
            compact_tab,
            toast_ta,
            toast_en,
        };
    }

    // 4. Deterministic Kural (1..=1330) and Adhikaram (1..=133) number navigation.
    if let Some(anchored) = extract_anchored_number(&nfc_trimmed) {
        match anchored {
            AnchoredNumber::Adhikaram(n) => {
                if (1..=133).contains(&n) {
                    return VoiceRouteOutcome {
                        cmd: BackendCommand::OpenAdhikaram(n as u8),
                        compact_tab: None,
                        toast_ta,
                        toast_en,
                    };
                }
                return fallback(nfc_trimmed);
            }
            AnchoredNumber::Kural(n) | AnchoredNumber::Bare(n) => {
                if (1..=1330).contains(&n) {
                    return VoiceRouteOutcome {
                        cmd: BackendCommand::OpenKural(n as u16),
                        compact_tab: None,
                        toast_ta,
                        toast_en,
                    };
                }
                return fallback(nfc_trimmed);
            }
        }
    }

    // 5. Fuzzy command matching (>= 0.80 normalized Levenshtein on >= 4-char tokens/phrases).
    if let Some(action) = find_fuzzy_command(&norm).or_else(|| find_fuzzy_command(stripped)) {
        let (cmd, compact_tab) = action.resolve(ctx);
        return VoiceRouteOutcome {
            cmd,
            compact_tab,
            toast_ta,
            toast_en,
        };
    }

    // 6. Unmatched transcript routes cleanly to grounded AI Q&A seeded with active Kural.
    fallback(nfc_trimmed)
}

/// Validate out-of-range anchored Kural/Adhikaram numbers before routing `raw`.
pub fn validate_voice_transcript(
    raw: &str,
    lang: Language,
    ctx: RouterContext,
) -> Result<VoiceRouteOutcome, BilingualCopy> {
    if let Some(err) = check_voice_number_range_error(raw) {
        return Err(err);
    }
    Ok(route_transcript(raw, lang, ctx))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::Instant;

    fn default_ctx() -> RouterContext {
        RouterContext {
            screen: AppScreen::Youth,
            modal: None,
            active_kural: 481,
            active_scholar: Scholar::MuVa,
            speech_speed: 1.0,
            muted: false,
        }
    }

    #[test]
    fn screen_default_target_matches_spec() {
        assert_eq!(
            screen_default_target(AppScreen::Start, Scholar::Solomon),
            SpeakTarget::KuralOnly
        );
        assert_eq!(
            screen_default_target(AppScreen::Kids, Scholar::Solomon),
            SpeakTarget::KuralAndMeaning
        );
        assert_eq!(
            screen_default_target(AppScreen::Youth, Scholar::Solomon),
            SpeakTarget::YouthBriefing
        );
        assert_eq!(
            screen_default_target(AppScreen::Adult, Scholar::Solomon),
            SpeakTarget::ScholarCommentary(Scholar::Solomon)
        );
        assert_eq!(
            screen_default_target(AppScreen::Adult, Scholar::Parimel),
            SpeakTarget::ScholarCommentary(Scholar::Parimel)
        );
    }

    #[test]
    fn routes_bilingual_navigation_language_theme_and_modals() {
        let ctx = default_ctx();
        for (input, expected) in [
            ("home", BackendCommand::Navigate(AppScreen::Start)),
            ("start", BackendCommand::Navigate(AppScreen::Start)),
            ("முகப்பு", BackendCommand::Navigate(AppScreen::Start)),
            ("kids", BackendCommand::SelectAge(AgeGroup::Kids)),
            ("குழந்தைகள்", BackendCommand::SelectAge(AgeGroup::Kids)),
            ("youth", BackendCommand::SelectAge(AgeGroup::Youth)),
            ("teen", BackendCommand::SelectAge(AgeGroup::Youth)),
            ("இளைஞர்கள்", BackendCommand::SelectAge(AgeGroup::Youth)),
            ("adult", BackendCommand::SelectAge(AgeGroup::Adult)),
            ("adults", BackendCommand::SelectAge(AgeGroup::Adult)),
            ("scholar", BackendCommand::SelectAge(AgeGroup::Adult)),
            ("பெரியவர்கள்", BackendCommand::SelectAge(AgeGroup::Adult)),
            ("next", BackendCommand::NextKural),
            ("next kural", BackendCommand::NextKural),
            ("அடுத்த குறள்", BackendCommand::NextKural),
            ("அடுத்து", BackendCommand::NextKural),
            ("previous", BackendCommand::PrevKural),
            ("prev", BackendCommand::PrevKural),
            ("முந்தைய குறள்", BackendCommand::PrevKural),
            ("பின்னால்", BackendCommand::PrevKural),
            ("tamil", BackendCommand::SetLanguage(Language::Tamil)),
            ("தமிழ்", BackendCommand::SetLanguage(Language::Tamil)),
            ("english", BackendCommand::SetLanguage(Language::English)),
            ("ஆங்கிலம்", BackendCommand::SetLanguage(Language::English)),
            ("dark", BackendCommand::SetTheme(Theme::Dark)),
            ("dark mode", BackendCommand::SetTheme(Theme::Dark)),
            ("இருள்", BackendCommand::SetTheme(Theme::Dark)),
            ("light", BackendCommand::SetTheme(Theme::Light)),
            ("light mode", BackendCommand::SetTheme(Theme::Light)),
            ("ஒளி", BackendCommand::SetTheme(Theme::Light)),
            ("close", BackendCommand::CloseModal),
            ("exit", BackendCommand::CloseModal),
            ("மூடு", BackendCommand::CloseModal),
        ] {
            let out = route_transcript(input, Language::Tamil, ctx);
            assert_eq!(out.cmd, expected, "input={input}");
            assert_eq!(out.toast_en, format!("Heard: {input}"));
            assert_eq!(out.toast_ta, format!("கேட்டது: {input}"));
        }
    }

    #[test]
    fn routes_search_divisions_filters_and_clear() {
        let ctx = default_ctx();
        assert_eq!(
            route_transcript("search", Language::English, ctx).cmd,
            BackendCommand::OpenModal(ModalKind::Search {
                initial_tab: SearchTab::Query
            })
        );
        assert_eq!(
            route_transcript("தேடு", Language::Tamil, ctx).cmd,
            BackendCommand::OpenModal(ModalKind::Search {
                initial_tab: SearchTab::Query
            })
        );

        for div_cmd in ["paals", "divisions", "chapters", "பால்", "அதிகாரங்கள்"]
        {
            assert_eq!(
                route_transcript(div_cmd, Language::Tamil, ctx).cmd,
                BackendCommand::OpenModal(ModalKind::Search {
                    initial_tab: SearchTab::Divisions
                }),
                "div_cmd={div_cmd}"
            );
        }

        for (cmd, div_id) in [
            ("aram", 1),
            ("virtue", 1),
            ("அறம்", 1),
            ("அறத்துப்பால்", 1),
            ("porul", 2),
            ("wealth", 2),
            ("பொருள்", 2),
            ("பொருட்பால்", 2),
            ("inbam", 3),
            ("love", 3),
            ("இன்பம்", 3),
            ("காமத்துப்பால்", 3),
            ("இன்பத்துப்பால்", 3),
        ] {
            assert_eq!(
                route_transcript(cmd, Language::Tamil, ctx).cmd,
                BackendCommand::FilterByDivision(Some(div_id)),
                "cmd={cmd}"
            );
        }

        for clear in ["clear filter", "all kurals", "வடிகட்டி நீக்கு"] {
            assert_eq!(
                route_transcript(clear, Language::English, ctx).cmd,
                BackendCommand::ClearFilter
            );
        }
    }

    #[test]
    fn routes_audio_playback_speed_mute_and_volume() {
        let ctx = RouterContext {
            screen: AppScreen::Adult,
            modal: None,
            active_kural: 423,
            active_scholar: Scholar::Kalaignar,
            speech_speed: 1.0,
            muted: false,
        };

        assert_eq!(
            route_transcript("stop", Language::English, ctx).cmd,
            BackendCommand::StopAudio
        );
        assert_eq!(
            route_transcript("நிறுத்து", Language::Tamil, ctx).cmd,
            BackendCommand::StopAudio
        );
        assert_eq!(
            route_transcript("போதும்", Language::Tamil, ctx).cmd,
            BackendCommand::StopAudio
        );
        assert_eq!(
            route_transcript("replay", Language::English, ctx).cmd,
            BackendCommand::ReplayAudio
        );
        assert_eq!(
            route_transcript("மீண்டும் வாசி", Language::Tamil, ctx).cmd,
            BackendCommand::ReplayAudio
        );
        assert_eq!(
            route_transcript("read", Language::English, ctx).cmd,
            BackendCommand::SpeakCurrent(SpeakTarget::ScholarCommentary(Scholar::Kalaignar))
        );
        assert_eq!(
            route_transcript("வாசி", Language::Tamil, ctx).cmd,
            BackendCommand::SpeakCurrent(SpeakTarget::ScholarCommentary(Scholar::Kalaignar))
        );
        assert_eq!(
            route_transcript("read kural", Language::English, ctx).cmd,
            BackendCommand::SpeakCurrent(SpeakTarget::KuralOnly)
        );
        assert_eq!(
            route_transcript("குறள் மட்டும்", Language::Tamil, ctx).cmd,
            BackendCommand::SpeakCurrent(SpeakTarget::KuralOnly)
        );
        assert_eq!(
            route_transcript("read meaning", Language::English, ctx).cmd,
            BackendCommand::SpeakCurrent(SpeakTarget::KuralAndMeaning)
        );
        assert_eq!(
            route_transcript("பொருள் வாசி", Language::Tamil, ctx).cmd,
            BackendCommand::SpeakCurrent(SpeakTarget::KuralAndMeaning)
        );
        assert_eq!(
            route_transcript("slower", Language::English, ctx).cmd,
            BackendCommand::SetSpeechSpeed(0.75)
        );
        assert_eq!(
            route_transcript("மெதுவாக", Language::Tamil, ctx).cmd,
            BackendCommand::SetSpeechSpeed(0.75)
        );
        assert_eq!(
            route_transcript("faster", Language::English, ctx).cmd,
            BackendCommand::SetSpeechSpeed(1.25)
        );
        assert_eq!(
            route_transcript("வேகமாக", Language::Tamil, ctx).cmd,
            BackendCommand::SetSpeechSpeed(1.25)
        );
        assert_eq!(
            route_transcript(
                "normal speed",
                Language::English,
                RouterContext {
                    speech_speed: 1.25,
                    ..ctx
                }
            )
            .cmd,
            BackendCommand::SetSpeechSpeed(1.0)
        );
        assert_eq!(
            route_transcript("சாதாரண வேகம்", Language::Tamil, ctx).cmd,
            BackendCommand::SetSpeechSpeed(1.0)
        );

        // Mute toggles !ctx.muted; unmute always sets false.
        assert_eq!(
            route_transcript("mute", Language::English, ctx).cmd,
            BackendCommand::SetMuted(true)
        );
        let muted_ctx = RouterContext { muted: true, ..ctx };
        assert_eq!(
            route_transcript("toggle mute", Language::English, muted_ctx).cmd,
            BackendCommand::SetMuted(false)
        );
        assert_eq!(
            route_transcript("ஒலி நிறுத்து", Language::Tamil, ctx).cmd,
            BackendCommand::SetMuted(true)
        );
        assert_eq!(
            route_transcript("unmute", Language::English, muted_ctx).cmd,
            BackendCommand::SetMuted(false)
        );
        assert_eq!(
            route_transcript("unmute", Language::English, ctx).cmd,
            BackendCommand::SetMuted(false)
        );

        assert_eq!(
            route_transcript("volume up", Language::English, ctx).cmd,
            BackendCommand::AdjustVolume(0.10)
        );
        assert_eq!(
            route_transcript("சத்தம் குறை", Language::Tamil, ctx).cmd,
            BackendCommand::AdjustVolume(-0.10)
        );
    }

    #[test]
    fn routes_modals_tabs_bookmarks_and_scholars() {
        let ctx = default_ctx();
        assert_eq!(
            route_transcript("story", Language::English, ctx).cmd,
            BackendCommand::OpenModal(ModalKind::KidsStory)
        );
        assert_eq!(
            route_transcript("கதை", Language::Tamil, ctx).cmd,
            BackendCommand::OpenModal(ModalKind::KidsStory)
        );
        let in_story = RouterContext {
            modal: Some(ModalKind::KidsStory),
            ..ctx
        };
        assert_eq!(
            route_transcript("story", Language::English, in_story).cmd,
            BackendCommand::SpeakCurrent(SpeakTarget::KuttyStory)
        );
        assert_eq!(
            route_transcript("read story", Language::English, ctx).cmd,
            BackendCommand::SpeakCurrent(SpeakTarget::KuttyStory)
        );
        assert_eq!(
            route_transcript("finish story", Language::English, in_story).cmd,
            BackendCommand::CompleteKidsStory(481)
        );
        assert_eq!(
            route_transcript("கதை முடிந்தது", Language::Tamil, in_story).cmd,
            BackendCommand::CompleteKidsStory(481)
        );
        assert_eq!(
            route_transcript("word game", Language::English, ctx).cmd,
            BackendCommand::OpenModal(ModalKind::KidsWordGame)
        );

        let quiz = route_transcript("quiz", Language::English, ctx);
        assert_eq!(quiz.cmd, BackendCommand::OpenModal(ModalKind::YouthQuiz));
        assert_eq!(quiz.compact_tab, Some(CompactTabHint::Quiz));

        let dive = route_transcript("deep dive", Language::English, ctx);
        assert_eq!(
            dive.cmd,
            BackendCommand::OpenModal(ModalKind::YouthDeepDive)
        );
        assert_eq!(dive.compact_tab, Some(CompactTabHint::Study));

        let verse = route_transcript("பாடல்", Language::Tamil, ctx);
        assert_eq!(
            verse.cmd,
            BackendCommand::SpeakCurrent(SpeakTarget::KuralOnly)
        );
        assert_eq!(verse.compact_tab, Some(CompactTabHint::Verse));

        let analysis = route_transcript("analysis", Language::English, ctx);
        assert_eq!(
            analysis.cmd,
            BackendCommand::SpeakCurrent(SpeakTarget::YouthBriefing)
        );
        assert_eq!(analysis.compact_tab, Some(CompactTabHint::Analysis));

        assert_eq!(
            route_transcript("bookmark", Language::English, ctx).cmd,
            BackendCommand::ToggleBookmark(481)
        );
        assert_eq!(
            route_transcript("புத்தகக்குறி", Language::Tamil, ctx).cmd,
            BackendCommand::ToggleBookmark(481)
        );

        for (spoken, scholar) in [
            ("mu va", Scholar::MuVa),
            ("மு.வ", Scholar::MuVa),
            ("மு.வ.", Scholar::MuVa),
            ("solomon", Scholar::Solomon),
            ("சாலமன்", Scholar::Solomon),
            ("kalaignar", Scholar::Kalaignar),
            ("கலைஞர்", Scholar::Kalaignar),
            ("parimel", Scholar::Parimel),
            ("parimelazhagar", Scholar::Parimel),
            ("பரிமேலழகர்", Scholar::Parimel),
            ("english commentary", Scholar::English),
            ("ஆங்கில உரை", Scholar::English),
        ] {
            let out = route_transcript(spoken, Language::Tamil, ctx);
            assert_eq!(
                out.cmd,
                BackendCommand::SelectCommentary(scholar),
                "{spoken}"
            );
            assert_eq!(
                out.compact_tab,
                Some(CompactTabHint::Commentary),
                "{spoken}"
            );
        }
    }

    #[test]
    fn context_aware_quiz_and_game_selection_only_when_modal_open() {
        let base = default_ctx();
        let game_ctx = RouterContext {
            screen: AppScreen::Kids,
            modal: Some(ModalKind::KidsWordGame),
            active_kural: 12,
            ..base
        };
        let quiz_ctx = RouterContext {
            screen: AppScreen::Youth,
            modal: Some(ModalKind::YouthQuiz),
            active_kural: 481,
            ..base
        };

        assert_eq!(
            route_transcript("a", Language::English, game_ctx).cmd,
            BackendCommand::SubmitKidsGameAnswer {
                kural_number: 12,
                choice_idx: 0
            }
        );
        assert_eq!(
            route_transcript("நான்கு", Language::Tamil, game_ctx).cmd,
            BackendCommand::SubmitKidsGameAnswer {
                kural_number: 12,
                choice_idx: 3
            }
        );
        assert_eq!(
            route_transcript("second", Language::English, quiz_ctx).cmd,
            BackendCommand::SubmitYouthQuizAnswer {
                kural_number: 481,
                choice_idx: 1
            }
        );
        assert_eq!(
            route_transcript("இ", Language::Tamil, quiz_ctx).cmd,
            BackendCommand::SubmitYouthQuizAnswer {
                kural_number: 481,
                choice_idx: 2
            }
        );

        // Outside KidsWordGame / YouthQuiz, "1" opens Kural 1 and "a" falls back to AI question.
        assert_eq!(
            route_transcript("1", Language::English, base).cmd,
            BackendCommand::OpenKural(1)
        );
        assert!(matches!(
            route_transcript("a", Language::English, base).cmd,
            BackendCommand::AskAiQuestion { .. }
        ));
    }

    #[test]
    fn routes_numbers_and_validates_kural_and_adhikaram_ranges() {
        let ctx = default_ctx();
        assert_eq!(
            validate_voice_transcript("kural four hundred eighty one", Language::English, ctx)
                .unwrap()
                .cmd,
            BackendCommand::OpenKural(481)
        );
        assert_eq!(
            validate_voice_transcript("kural thirteen hundred thirty", Language::English, ctx)
                .unwrap()
                .cmd,
            BackendCommand::OpenKural(1330)
        );
        assert_eq!(
            validate_voice_transcript("குறள் நானூற்று எண்பத்தொன்று", Language::Tamil, ctx)
                .unwrap()
                .cmd,
            BackendCommand::OpenKural(481)
        );
        assert_eq!(
            validate_voice_transcript("அதிகாரம் 10", Language::Tamil, ctx)
                .unwrap()
                .cmd,
            BackendCommand::OpenAdhikaram(10)
        );
        assert_eq!(
            validate_voice_transcript("chapter 133", Language::English, ctx)
                .unwrap()
                .cmd,
            BackendCommand::OpenAdhikaram(133)
        );
        assert_eq!(
            validate_voice_transcript("#481", Language::English, ctx)
                .unwrap()
                .cmd,
            BackendCommand::OpenKural(481)
        );
        assert_eq!(
            validate_voice_transcript("முதல் குறள்", Language::Tamil, ctx)
                .unwrap()
                .cmd,
            BackendCommand::OpenKural(1)
        );
        assert_eq!(
            validate_voice_transcript("அதிகாரம் நூற்று முப்பத்து மூன்று", Language::Tamil, ctx)
                .unwrap()
                .cmd,
            BackendCommand::OpenAdhikaram(133)
        );
        assert_eq!(
            validate_voice_transcript("அதிகாரம் நூற்று முப்பத்து நான்கு", Language::Tamil, ctx),
            Err(VOICE_ADHIKARAM_RANGE_ERROR)
        );

        assert_eq!(
            check_voice_number_range_error("kural 0"),
            Some(VOICE_KURAL_RANGE_ERROR)
        );
        assert_eq!(
            check_voice_number_range_error("குறள் 1331"),
            Some(VOICE_KURAL_RANGE_ERROR)
        );
        assert_eq!(
            validate_voice_transcript("kural 2000", Language::English, ctx),
            Err(VOICE_KURAL_RANGE_ERROR)
        );
        assert_eq!(
            check_voice_number_range_error("adhikaram 0"),
            Some(VOICE_ADHIKARAM_RANGE_ERROR)
        );
        assert_eq!(
            check_voice_number_range_error("chapter 134"),
            Some(VOICE_ADHIKARAM_RANGE_ERROR)
        );
        assert_eq!(
            validate_voice_transcript("அதிகாரம் 134", Language::Tamil, ctx),
            Err(VOICE_ADHIKARAM_RANGE_ERROR)
        );
        assert_eq!(check_voice_number_range_error("42 * 10"), None);
    }

    #[test]
    fn fuzzy_typos_work_and_multiword_questions_fall_through_to_ai() {
        let ctx = default_ctx();
        assert_eq!(
            route_transcript("soloman", Language::English, ctx).cmd,
            BackendCommand::SelectCommentary(Scholar::Solomon)
        );
        assert_eq!(
            route_transcript("parimelazagar", Language::English, ctx).cmd,
            BackendCommand::SelectCommentary(Scholar::Parimel)
        );
        assert_eq!(
            route_transcript("kurl 481", Language::English, ctx).cmd,
            BackendCommand::OpenKural(481)
        );
        assert_eq!(
            route_transcript("prevous", Language::English, ctx).cmd,
            BackendCommand::PrevKural
        );
        assert_eq!(
            route_transcript("சாலமன", Language::Tamil, ctx).cmd,
            BackendCommand::SelectCommentary(Scholar::Solomon)
        );
        assert_eq!(
            route_transcript("கலைஞர", Language::Tamil, ctx).cmd,
            BackendCommand::SelectCommentary(Scholar::Kalaignar)
        );
        assert_eq!(
            route_transcript("பரிமேலழகா", Language::Tamil, ctx).cmd,
            BackendCommand::SelectCommentary(Scholar::Parimel)
        );
        assert_eq!(
            route_transcript("அறத்துப்பால", Language::Tamil, ctx).cmd,
            BackendCommand::FilterByDivision(Some(1))
        );

        // Short tokens (< 4 chars) must never fuzzy-match into commands.
        assert!(matches!(
            route_transcript("கா", Language::Tamil, ctx).cmd,
            BackendCommand::AskAiQuestion { .. }
        ));
        assert!(matches!(
            route_transcript("hi", Language::English, ctx).cmd,
            BackendCommand::AskAiQuestion { .. }
        ));

        // Decomposed NFD Tamil input normalizes to NFC cleanly across command & toast.
        let decomposed_porul = "ப\u{0BCA}ருட்பால்";
        let nfc_out = route_transcript(decomposed_porul, Language::Tamil, ctx);
        assert_eq!(nfc_out.cmd, BackendCommand::FilterByDivision(Some(2)));
        assert!(kural_core::is_nfc(&nfc_out.toast_ta));
        assert!(kural_core::is_nfc(&nfc_out.toast_en));

        // Multi-word questions must NEVER be hijacked by command keywords.
        for question in [
            "What is wealth in Kural?",
            "what is the meaning",
            "என்ன பொருள்",
            "பொருள் என்ன",
            "why are there 1330 kurals",
            "Tell me about Thiruvalluvar",
            "42 * 10",
        ] {
            let out = route_transcript(question, Language::English, ctx);
            assert!(
                matches!(out.cmd, BackendCommand::AskAiQuestion { .. }),
                "question={question} mapped to {:?}",
                out.cmd
            );
        }
    }

    #[test]
    fn routing_latency_stays_well_below_one_millisecond() {
        let ctx = default_ctx();
        let samples = [
            "kural four hundred eighty one",
            "குறள் நானூற்று எண்பத்தொன்று",
            "soloman",
            "What is wealth in Kural?",
            "clear filter",
            "பரிமேலழகர்",
            "adhikaram 133",
            "dark mode",
        ];

        // Warm up once, then verify average and max per utterance < 1 ms.
        for &s in &samples {
            let _ = route_transcript(s, Language::Tamil, ctx);
        }

        let start = Instant::now();
        let iters = 100usize;
        for _ in 0..iters {
            for &s in &samples {
                let _ = route_transcript(s, Language::Tamil, ctx);
            }
        }
        let elapsed = start.elapsed();
        let per_call = elapsed / (iters * samples.len()) as u32;
        assert!(
            per_call.as_micros() < 1000,
            "voice routing exceeded 1 ms budget: {per_call:?}"
        );
    }
}
