//! Youth Mode Screen (`S3`, ages 13–17) — Chola lattice watermark, ambient desktop motes,
//! 38 px 3-tab Pi strip + inline speed chip, verbatim Tamil couplet, desktop analytics &
//! interactive topic tag search + Daily Quiz CTA, standardized `[0.75×, 1.0×, 1.25×]` speed dock,
//! and 5-slot Pi dock.

use crate::components::ContentPlaybackButton;
use crate::components::header::TopHeader;
use crate::components::icons::{BookIcon, ChatIcon, MicIcon, SearchIcon};
use crate::copy::{english_verse_couplet, paginate_sentences, research_theme_label, tr};
use crate::state::{AppContext, is_voice_active, is_voice_cancellable};
use dioxus::prelude::*;
use kural_core::{
    ActiveFilter, BackendCommand, CANONICAL_TAGS, CompactTabHint, Language, ModalKind,
    SPEECH_SPEED_PRESETS, SearchTab, SpeakTarget, Theme, step_speech_speed,
};

#[component]
pub fn YouthScreen() -> Element {
    let mut ctx = use_context::<AppContext>();
    let snap = ctx.snapshot.read().clone();
    let lang = snap.prefs.lang;
    let theme = snap.prefs.theme;
    let full = &snap.current_kural;
    let kural_num = full.kural.number;
    let voice_active = is_voice_active(snap.audio_state);
    let voice_cancellable = is_voice_cancellable(snap.audio_state);
    let speed = snap.prefs.speech_speed;

    let raw_tab = *ctx.pi_compact_tab.read();
    let pi_tab = match raw_tab {
        CompactTabHint::Analysis | CompactTabHint::Quiz => raw_tab,
        _ => CompactTabHint::Verse,
    };

    let mut page_idx = use_signal(|| 0_usize);
    let mut last_reset_key = use_signal(|| kural_num);
    use_effect(move || {
        let num = ctx.snapshot.read().current_kural.kural.number;
        if *last_reset_key.peek() != num {
            last_reset_key.set(num);
            page_idx.set(0);
        }
    });

    let mut tag_search = use_signal(String::new);

    let core_meaning = match lang {
        Language::Tamil => {
            if full.kural.meaning_mu_va.trim().is_empty() {
                full.kural.meaning_solomon.as_str()
            } else {
                full.kural.meaning_mu_va.as_str()
            }
        }
        Language::English => {
            if full.kural.explanation_en.trim().is_empty() {
                full.kural.translation_en.as_str()
            } else {
                full.kural.explanation_en.as_str()
            }
        }
    };
    let pi_analysis_raw = full
        .youth
        .as_ref()
        .map(|y| {
            let analysis = tr(lang, &y.analysis_ta, &y.analysis_en);
            let tip = tr(lang, &y.action_tip_ta, &y.action_tip_en);
            format!("{core_meaning} {analysis} {tip}")
        })
        .unwrap_or_else(|| core_meaning.to_string());
    let pi_pages = paginate_sentences(&pi_analysis_raw, 84);
    let safe_page = (*page_idx.read()).min(pi_pages.len().saturating_sub(1));

    let adhikaram_label = tr(lang, &full.adhikaram.name_ta, &full.adhikaram.name_en);
    let div_label = tr(lang, &full.division.name_ta, &full.division.name_en);

    let filter_needle = tag_search.read().trim().to_ascii_lowercase();
    let active_tag_slug = match &snap.active_filter {
        Some(ActiveFilter::Tag(s)) => Some(s.clone()),
        _ => None,
    };

    // Prioritize current Kural's tags when unfiltered, otherwise search all 20 canonical tags.
    let displayed_tags: Vec<(&'static str, &'static str, &'static str)> =
        if filter_needle.is_empty() && !full.tags.is_empty() {
            let mut items: Vec<_> = full
                .tags
                .iter()
                .filter_map(|t| {
                    CANONICAL_TAGS
                        .iter()
                        .copied()
                        .find(|(slug, _, _)| *slug == t.slug)
                })
                .collect();
            for item in CANONICAL_TAGS {
                if items.len() >= 8 {
                    break;
                }
                if !items.iter().any(|existing| existing.0 == item.0) {
                    items.push(item);
                }
            }
            items
        } else {
            CANONICAL_TAGS
                .iter()
                .copied()
                .filter(|(slug, ta, en)| {
                    filter_needle.is_empty()
                        || slug.contains(&filter_needle)
                        || ta.contains(&filter_needle)
                        || en.to_ascii_lowercase().contains(&filter_needle)
                })
                .take(8)
                .collect()
        };

    rsx! {
        div { class: "scene-backdrop", aria_hidden: "true",
            div {
                class: if theme == Theme::Light { "royal-canvas-youth-light" } else { "royal-canvas-youth-dark" },
                style: "position:absolute; inset:0;",
            }
            // Subtle Chola lattice watermark
            svg { class: "chola-watermark", view_box: "0 0 400 300", preserve_aspect_ratio: "xMidYMid slice",
                defs {
                    pattern { id: "cholaLattice", width: "40", height: "40", pattern_units: "userSpaceOnUse",
                        path {
                            d: "M20 0 L40 20 L20 40 L0 20 Z M20 8 L32 20 L20 32 L8 20 Z",
                            fill: "none",
                            stroke: "#E6C068",
                            stroke_width: "0.9",
                        }
                    }
                }
                rect { width: "100%", height: "100%", fill: "url(#cholaLattice)" }
            }
            // Ambient desktop gold motes (desktop-only)
            div { class: "desktop-only", style: "position:absolute; inset:0; pointer-events:none;",
                span { class: "anim-mote", style: "position:absolute; top:18%; left:14%; width:7px; height:7px; border-radius:9999px; background:#E6C068;" }
                span { class: "anim-mote", style: "position:absolute; top:62%; left:44%; width:6px; height:6px; border-radius:9999px; background:#19C9A0; animation-delay:1.4s;" }
                span { class: "anim-mote", style: "position:absolute; top:28%; right:16%; width:8px; height:8px; border-radius:9999px; background:#E6C068; animation-delay:2.6s;" }
            }
        }

        div { id: "app-shell", class: "royal-shell",
            TopHeader {}

            main { id: "main-stage", class: "main-stage",
                // Pi 480x320 3-tab Segmented Bar (38px) + Speed Chip
                div { id: "pi-mode-strip", class: "pi-only",
                    button {
                        r#type: "button",
                        class: if pi_tab == CompactTabHint::Verse { "pi-mode-tab active" } else { "pi-mode-tab" },
                        onclick: move |_| ctx.pi_compact_tab.set(CompactTabHint::Verse),
                        "{tr(lang, \"பாடல்\", \"Verse\")}"
                    }
                    button {
                        r#type: "button",
                        class: if pi_tab == CompactTabHint::Analysis { "pi-mode-tab active" } else { "pi-mode-tab" },
                        onclick: move |_| ctx.pi_compact_tab.set(CompactTabHint::Analysis),
                        "{tr(lang, \"பொருள்\", \"Meaning\")}"
                    }
                    button {
                        r#type: "button",
                        class: if pi_tab == CompactTabHint::Quiz { "pi-mode-tab active" } else { "pi-mode-tab" },
                        onclick: move |_| ctx.pi_compact_tab.set(CompactTabHint::Quiz),
                        "{tr(lang, \"சவால்\", \"Quiz\")}"
                    }
                    button {
                        id: "pi-speed-chip",
                        r#type: "button",
                        class: "pi-scholar-cycle-pill tabular-nums",
                        aria_label: tr(lang, "வேகம்", "Speech speed"),
                        onclick: move |_| ctx.dispatch(BackendCommand::SetSpeechSpeed(step_speech_speed(speed, 1))),
                        "{speed:.2}×"
                    }
                }

                // Left 7 cols (Desktop) / Full Card Stage (Pi)
                section { class: "col-span-7",
                    article { id: "kural-card", class: "royal-card-primary kural-card-layout",
                        div { id: "kural-card-header", class: "kural-card-header",
                            div { style: "display:flex; align-items:center; gap:6px; min-width:0;",
                                span { id: "kural-badge-text", class: "kural-badge-pill",
                                    "{tr(lang, \"குறள் #\", \"Kural #\")}{kural_num}"
                                }
                                span { class: "kural-div-pill truncate",
                                    "{div_label} • {adhikaram_label}"
                                }
                                if snap.active_filter.is_some() {
                                    button {
                                        r#type: "button",
                                        class: "filter-banner-chip",
                                        onclick: move |_| ctx.dispatch(BackendCommand::ClearFilter),
                                        "✕ {tr(lang, \"நீக்கு\", \"Clear\")}"
                                    }
                                }
                            }
                            div { style: "display:flex; align-items:center; gap:6px; flex-shrink:0;",
                                button {
                                    id: "prev-kural",
                                    r#type: "button",
                                    class: "icon-circle-btn royal-btn",
                                    style: "width:34px; height:34px;",
                                    aria_label: tr(lang, "முந்தைய குறள்", "Previous Kural"),
                                    onclick: move |_| ctx.dispatch(BackendCommand::PrevKural),
                                    "◀"
                                }
                                button {
                                    id: "next-kural",
                                    r#type: "button",
                                    class: "icon-circle-btn royal-btn",
                                    style: "width:34px; height:34px;",
                                    aria_label: tr(lang, "அடுத்த குறள்", "Next Kural"),
                                    onclick: move |_| ctx.dispatch(BackendCommand::NextKural),
                                    "▶"
                                }
                            }
                        }

                        // Pane 1: Language-Matched Kural Couplet + Subtitle (+ Desktop Meaning)
                        div {
                            class: if pi_tab == CompactTabHint::Verse {
                                "kural-center-verse"
                            } else {
                                "kural-center-verse pi-pane-hidden"
                            },
                            if lang == Language::English {
                                p { id: "kural-couplet", class: "kural-couplet-en", lang: "en",
                                    "{english_verse_couplet(full)}"
                                }
                                p { id: "kural-translit", class: "desktop-hd-only", lang: "ta",
                                    "{full.kural.line1_ta} / {full.kural.line2_ta}"
                                }
                            } else {
                                p { id: "kural-couplet", class: "font-kural", lang: "ta",
                                    "{full.kural.line1_ta}"
                                    br {}
                                    "{full.kural.line2_ta}"
                                }
                                p { id: "kural-translit",
                                    "{full.kural.transliteration1} · {full.kural.transliteration2}"
                                }
                            }
                            p {
                                id: "kural-meaning",
                                class: "desktop-only",
                                lang: if lang == Language::English { "en" } else { "ta" },
                                "{core_meaning}"
                            }
                        }

                        // Pane 2 (Pi only): Sentence-paginated Meaning + Practical Takeaway
                        div {
                            class: if pi_tab == CompactTabHint::Analysis {
                                "kural-center-verse pi-only"
                            } else {
                                "kural-center-verse pi-only pi-pane-hidden"
                            },
                            p { class: "pi-prose-text",
                                "{pi_pages[safe_page]}"
                            }
                        }

                        // Pane 3 (Pi only): Daily Quiz & Deep Dive Launchers
                        div {
                            class: if pi_tab == CompactTabHint::Quiz {
                                "kural-center-verse pi-only"
                            } else {
                                "kural-center-verse pi-only pi-pane-hidden"
                            },
                            style: "gap:6px;",
                            p { class: "pi-prose-text", style: "font-weight:700;",
                                "{tr(lang, \"உங்கள் புரிதலைச் சோதிக்கவும்!\", \"Test your understanding!\")}"
                            }
                            div { style: "display:flex; gap:8px; flex-wrap:nowrap; justify-content:center;",
                                button {
                                    r#type: "button",
                                    class: "royal-btn royal-cta-primary action-pill-btn",
                                    onclick: move |_| ctx.dispatch(BackendCommand::OpenModal(ModalKind::YouthQuiz)),
                                    "🏆 {tr(lang, \"வினாடி வினா\", \"Quiz\")}"
                                }
                                button {
                                    r#type: "button",
                                    class: "royal-btn action-pill-btn",
                                    onclick: move |_| ctx.dispatch(BackendCommand::OpenModal(ModalKind::YouthDeepDive)),
                                    BookIcon {}
                                    span { "{tr(lang, \"ஆழ்ந்து கற்றல்\", \"Deep Dive\")}" }
                                }
                            }
                        }

                        // Card Audio & Action Row (+ inline Pi sentence pager for Analysis tab)
                        div { id: "kural-actions", class: "kural-actions-row",
                            ContentPlaybackButton {
                                btn_base: "royal-btn",
                                button_id: Some("btn-listen"),
                                playback_status: snap.playback_status,
                                audio_state: snap.audio_state,
                                can_replay_tts: snap.can_replay_tts,
                                lang: lang,
                                fallback_target: SpeakTarget::KuralAndMeaning,
                            }
                            if pi_tab == CompactTabHint::Analysis && pi_pages.len() > 1 {
                                div { class: "pager-row pi-only",
                                    button {
                                        r#type: "button",
                                        class: "pager-btn",
                                        disabled: safe_page == 0,
                                        onclick: move |_| page_idx.set(safe_page.saturating_sub(1)),
                                        "◀"
                                    }
                                    span { class: "tabular-nums", "{safe_page + 1}/{pi_pages.len()}" }
                                    button {
                                        r#type: "button",
                                        class: "pager-btn",
                                        disabled: safe_page + 1 >= pi_pages.len(),
                                        onclick: move |_| page_idx.set(safe_page + 1),
                                        "▶"
                                    }
                                }
                            }
                            button {
                                r#type: "button",
                                class: "royal-btn action-pill-btn desktop-only",
                                onclick: move |_| ctx.dispatch(BackendCommand::OpenModal(ModalKind::YouthDeepDive)),
                                BookIcon {}
                                span { "{tr(lang, \"விரிவான உரை\", \"Deep Dive\")}" }
                            }
                            button {
                                r#type: "button",
                                class: "royal-btn action-pill-btn action-pill-chat desktop-only",
                                onclick: move |_| ctx.dispatch(BackendCommand::OpenModal(ModalKind::AiChat {
                                    seed_kural: Some(kural_num),
                                })),
                                ChatIcon {}
                                span { "{tr(lang, \"AI உரையாடல்\", \"Ask AI\")}" }
                            }
                        }
                    }
                }

                // Right 5 cols (Desktop & 7" Pi): Analytics, Takeaway, Topic Tags Search/Chips, Daily Quiz CTA
                section { class: "col-span-5 desktop-only",
                    div { class: "royal-card-secondary youth-analysis-card", style: "display:flex; flex-direction:column; gap:8px;",
                        div { style: "display:flex; justify-content:space-between; align-items:center;",
                            h2 { style: "font-size:15px; font-weight:800; color:var(--primary);",
                                "{tr(lang, \"இளைஞர் பகுப்பாய்வு & வழிகாட்டுதல்\", \"Youth Analysis & Action Tip\")}"
                            }
                            button {
                                r#type: "button",
                                class: "tag-chip",
                                onclick: move |_| ctx.dispatch(BackendCommand::OpenModal(ModalKind::YouthDeepDive)),
                                "{tr(lang, \"மேலும் →\", \"More →\")}"
                            }
                        }
                        if let Some(y) = &full.youth {
                            p { class: "line-clamp-2", style: "font-size:13px; line-height:1.45;",
                                "{tr(lang, &y.analysis_ta, &y.analysis_en)}"
                            }
                            div { class: "royal-subcard",
                                span { style: "font-size:11px; font-weight:800; opacity:0.8;",
                                    "💡 {tr(lang, \"நடைமுறைப் படி\", \"Action Tip\")}"
                                }
                                p { class: "line-clamp-2", style: "font-size:13px; font-weight:600; margin-top:2px;",
                                    "{tr(lang, &y.action_tip_ta, &y.action_tip_en)}"
                                }
                            }
                        }
                    }

                    // Topic Tags + Quick Quiz Card
                    div { class: "royal-card-secondary youth-tags-card", style: "flex:1; min-height:0; display:flex; flex-direction:column; justify-content:space-between; gap:8px;",
                        div { style: "display:flex; flex-direction:column; gap:6px;",
                            div { style: "display:flex; align-items:center; justify-content:space-between; gap:8px;",
                                h3 { style: "font-size:13px; font-weight:800;",
                                    "{tr(lang, \"தலைப்பு வடிகட்டி\", \"Topic Tags\")}"
                                }
                                input {
                                    id: "tag-search",
                                    r#type: "text",
                                    class: "text-input",
                                    style: "min-height:30px; max-width:170px; font-size:12px;",
                                    placeholder: tr(lang, "தலைப்பு தேடு…", "Filter tags…"),
                                    value: "{tag_search}",
                                    oninput: move |e| tag_search.set(e.value()),
                                }
                            }
                            div { class: "tags-chips-row",
                                for (slug, ta, en) in displayed_tags {
                                    {
                                        let is_active = active_tag_slug.as_deref() == Some(slug);
                                        let slug_owned = slug.to_string();
                                        rsx! {
                                            button {
                                                key: "{slug}",
                                                r#type: "button",
                                                class: if is_active { "tag-chip active" } else { "tag-chip" },
                                                onclick: move |_| {
                                                    if is_active {
                                                        ctx.dispatch(BackendCommand::FilterByTag(None));
                                                    } else {
                                                        ctx.dispatch(BackendCommand::FilterByTag(Some(slug_owned.clone())));
                                                    }
                                                },
                                                "{tr(lang, ta, en)}"
                                            }
                                        }
                                    }
                                }
                            }
                        }

                        div { style: "display:flex; gap:8px;",
                            button {
                                r#type: "button",
                                class: "royal-subcard",
                                style: "flex:1; display:flex; align-items:center; justify-content:space-between;",
                                onclick: move |_| ctx.dispatch(BackendCommand::OpenModal(ModalKind::YouthQuiz)),
                                div {
                                    span { style: "font-size:11px; font-weight:800; color:var(--primary);",
                                        "🏆 {tr(lang, \"தினசரி சவால் (+20 XP)\", \"Daily Quiz (+20 XP)\")}"
                                    }
                                    p { style: "font-size:12px; font-weight:700; margin-top:2px;",
                                        "{tr(lang, \"வினாடி வினா தொடங்கு\", \"Answer Kural Quiz\")}"
                                    }
                                }
                                span { "→" }
                            }
                            if let Some(adult) = &full.adult {
                                {
                                    let (theme_title, _) = research_theme_label(adult.research_theme, lang);
                                    let rt = adult.research_theme;
                                    let is_rt = snap.active_filter == Some(ActiveFilter::ResearchTheme(rt));
                                    rsx! {
                                        button {
                                            r#type: "button",
                                            class: if is_rt { "royal-subcard active" } else { "royal-subcard" },
                                            style: "flex:1;",
                                            onclick: move |_| {
                                                if is_rt {
                                                    ctx.dispatch(BackendCommand::FilterByResearchTheme(None));
                                                } else {
                                                    ctx.dispatch(BackendCommand::FilterByResearchTheme(Some(rt)));
                                                }
                                            },
                                            span { style: "font-size:11px; font-weight:800; opacity:0.8;",
                                                "{tr(lang, \"ஆய்வுப் பிரிவு\", \"Theme\")}"
                                            }
                                            p { style: "font-size:12px; font-weight:700; margin-top:2px;",
                                                "{theme_title}"
                                            }
                                        }
                                    }
                                }
                            }
                        }
                    }
                }
            }

            // Desktop Footer Dock
            footer { class: "desktop-footer-dock desktop-only",
                div { style: "display:flex; align-items:center; gap:8px;",
                    button {
                        r#type: "button",
                        class: "royal-btn dock-pill-btn",
                        onclick: move |_| ctx.dispatch(BackendCommand::OpenModal(ModalKind::Search {
                            initial_tab: SearchTab::Query,
                        })),
                        SearchIcon {}
                        span { "{tr(lang, \"தேடு\", \"Search\")}" }
                    }
                    button {
                        r#type: "button",
                        class: "royal-btn dock-pill-btn",
                        onclick: move |_| ctx.dispatch(BackendCommand::OpenModal(ModalKind::Search {
                            initial_tab: SearchTab::Divisions,
                        })),
                        "📚 {tr(lang, \"பால்கள்\", \"Divisions\")}"
                    }
                }

                button {
                    id: "desktop-voice-btn",
                    r#type: "button",
                    class: if voice_active { "royal-btn voice-bar-btn is-listening" } else { "royal-btn voice-bar-btn" },
                    onclick: move |_| {
                        if voice_cancellable {
                            ctx.dispatch(BackendCommand::CancelVoice);
                        } else {
                            ctx.dispatch(BackendCommand::StartVoiceCommand);
                        }
                    },
                    MicIcon { size: 16 }
                    span { "{tr(lang, \"\\\"அடுத்த குறள்\\\", \\\"வினாடி வினா\\\", \\\"நிறுத்து\\\"\", \"Say \\\"Next Kural\\\", \\\"Quiz\\\", or \\\"Stop\\\"\")}" }
                }

                div { style: "display:flex; align-items:center; gap:8px;",
                    div { class: "speed-pill-group", aria_label: tr(lang, "பேச்சு வேகம்", "Speech speed"),
                        for &preset in &SPEECH_SPEED_PRESETS {
                            {
                                let active = (speed - preset).abs() < 0.02;
                                rsx! {
                                    button {
                                        r#type: "button",
                                        class: if active { "speed-chip active" } else { "speed-chip" },
                                        onclick: move |_| ctx.dispatch(BackendCommand::SetSpeechSpeed(preset)),
                                        "{preset:.2}×"
                                    }
                                }
                            }
                        }
                    }

                    button {
                        id: "sound-toggle",
                        r#type: "button",
                        class: "royal-btn dock-pill-btn",
                        onclick: move |_| ctx.dispatch(BackendCommand::SetMuted(!snap.prefs.muted)),
                        span { if snap.prefs.muted { "🔇" } else { "🔊" } }
                    }
                }
            }

            // Pi 480x320 5-Slot Dock: Paals | Deep | Voice Orb | Search | AI (76px)
            footer { id: "pi-zone-3", class: "pi-only pi-grid-only",
                button {
                    r#type: "button",
                    class: "pi-dock-tile",
                    onclick: move |_| ctx.dispatch(BackendCommand::OpenModal(ModalKind::Search {
                        initial_tab: SearchTab::Divisions,
                    })),
                    span { style: "font-size:14px; line-height:1;", "📚" }
                    span { class: "pi-dock-label", "{tr(lang, \"பால்கள்\", \"Paals\")}" }
                }
                button {
                    r#type: "button",
                    class: "pi-dock-tile",
                    onclick: move |_| ctx.dispatch(BackendCommand::OpenModal(ModalKind::YouthDeepDive)),
                    BookIcon {}
                    span { class: "pi-dock-label", "{tr(lang, \"ஆழ்\", \"Deep\")}" }
                }
                button {
                    id: "pi-voice-orb",
                    r#type: "button",
                    class: if voice_active { "is-listening" } else { "" },
                    onclick: move |_| {
                        if voice_cancellable {
                            ctx.dispatch(BackendCommand::CancelVoice);
                        } else {
                            ctx.dispatch(BackendCommand::StartVoiceCommand);
                        }
                    },
                    MicIcon { size: 20 }
                    span { "{tr(lang, \"பேசு\", \"Speak\")}" }
                }
                button {
                    r#type: "button",
                    class: "pi-dock-tile",
                    onclick: move |_| ctx.dispatch(BackendCommand::OpenModal(ModalKind::Search {
                        initial_tab: SearchTab::Query,
                    })),
                    SearchIcon {}
                    span { class: "pi-dock-label", "{tr(lang, \"தேடு\", \"Search\")}" }
                }
                button {
                    r#type: "button",
                    class: "pi-dock-tile",
                    onclick: move |_| ctx.dispatch(BackendCommand::OpenModal(ModalKind::AiChat {
                        seed_kural: Some(kural_num),
                    })),
                    ChatIcon {}
                    span { class: "pi-dock-label", "AI" }
                }
            }
        }
    }
}
