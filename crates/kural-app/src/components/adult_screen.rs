//! Adult / Scholars Mode Screen (`S4`, ages 18+) — Chola filigree watermark (no motes),
//! 38 px 3-tab Pi strip (`Kural | Commentary | Study`) + `<button id="pi-speed-chip">` +
//! Scholar Cycle Pill, 3D `#kural-initial-seal` strictly bound to Tamil `opening_letter_ta`,
//! verbatim Tamil couplet, 5-scholar commentary tabs with `#395`/`#648` Parimel guard,
//! Daily Wisdom Reflection + 4 `.royal-subcard` Research Themes + Saved Bookmarks,
//! standardized `[0.75×, 1.0×, 1.25×]` speed dock, and 5-slot Pi dock.

use crate::components::header::TopHeader;
use crate::components::icons::{
    ChatIcon, MicIcon, ReplayIcon, SearchIcon, SpeakerIcon, StopSquareIcon,
};
use crate::copy::{
    ALL_SCHOLARS, commentary_text, next_scholar, paginate_sentences, research_theme_label,
    scholar_label, tr,
};
use crate::state::{AppContext, is_voice_active};
use dioxus::prelude::*;
use kural_core::{
    ActiveFilter, AudioState, BackendCommand, CompactTabHint, Language, ModalKind,
    PARIMEL_FALLBACK_NOTICE, ResearchTheme, SPEECH_SPEED_PRESETS, Scholar, SearchTab, SpeakTarget,
    Theme, step_speech_speed,
};

#[component]
pub fn AdultScreen() -> Element {
    let mut ctx = use_context::<AppContext>();
    let snap = ctx.snapshot.read().clone();
    let lang = snap.prefs.lang;
    let theme = snap.prefs.theme;
    let full = &snap.current_kural;
    let kural_num = full.kural.number;
    let effective_scholar = snap.effective_scholar;
    let requested_scholar = snap.requested_scholar;
    let is_speaking = snap.audio_state == AudioState::PlayingTts;
    let voice_active = is_voice_active(snap.audio_state);
    let speed = snap.prefs.speech_speed;

    let raw_tab = *ctx.pi_compact_tab.read();
    let pi_tab = match raw_tab {
        CompactTabHint::Commentary | CompactTabHint::Study => raw_tab,
        _ => CompactTabHint::Verse,
    };

    let mut pi_page_idx = use_signal(|| 0_usize);
    let mut last_reset_key = use_signal(|| (kural_num, effective_scholar, pi_tab));
    use_effect(move || {
        let snap_now = ctx.snapshot.read();
        let tab_now = *ctx.pi_compact_tab.read();
        let key = (
            snap_now.current_kural.kural.number,
            snap_now.effective_scholar,
            tab_now,
        );
        if *last_reset_key.peek() != key {
            last_reset_key.set(key);
            pi_page_idx.set(0);
        }
    });

    let comm_body = commentary_text(full, effective_scholar, lang);
    let comm_pages = paginate_sentences(comm_body, 78);
    let reflection_text = full
        .adult
        .as_ref()
        .map(|a| tr(lang, &a.reflection_ta, &a.reflection_en))
        .unwrap_or("");
    let study_pages = paginate_sentences(reflection_text, 78);

    let active_pages = if pi_tab == CompactTabHint::Study {
        &study_pages
    } else {
        &comm_pages
    };
    let safe_page = (*pi_page_idx.read()).min(active_pages.len().saturating_sub(1));

    let parimel_missing = full.kural.meaning_parimel.is_none();
    let show_parimel_fallback =
        requested_scholar == Scholar::Parimel && effective_scholar != Scholar::Parimel;
    let parimel_notice = tr(lang, PARIMEL_FALLBACK_NOTICE.ta, PARIMEL_FALLBACK_NOTICE.en);

    let is_bookmarked = snap.bookmarks.iter().any(|b| b.kural_number == kural_num);
    let adhikaram_label = tr(lang, &full.adhikaram.name_ta, &full.adhikaram.name_en);

    rsx! {
        div { class: "scene-backdrop", aria_hidden: "true",
            div {
                class: if theme == Theme::Light { "royal-canvas-adult-light" } else { "royal-canvas-adult-dark" },
                style: "position:absolute; inset:0;",
            }
            // Subtle Chola filigree watermark (strictly no floating motes on S4)
            svg { class: "chola-watermark", view_box: "0 0 400 300", preserve_aspect_ratio: "xMidYMid slice",
                defs {
                    pattern { id: "cholaFiligree", width: "48", height: "48", pattern_units: "userSpaceOnUse",
                        circle { cx: "24", cy: "24", r: "18", fill: "none", stroke: "#C8A152", stroke_width: "0.7" }
                        path {
                            d: "M24 2 Q30 14 46 24 Q30 34 24 46 Q18 34 2 24 Q18 14 24 2 Z",
                            fill: "none",
                            stroke: "#C8A152",
                            stroke_width: "0.7",
                        }
                    }
                }
                rect { width: "100%", height: "100%", fill: "url(#cholaFiligree)" }
            }
        }

        div { id: "app-shell", class: "royal-shell",
            TopHeader {}

            main { id: "main-stage", class: "main-stage",
                // Pi 480x320 3-tab Segmented Bar (38px) + Speed button + Scholar Cycle Pill
                div { id: "pi-mode-strip", class: "pi-only",
                    button {
                        r#type: "button",
                        class: if pi_tab == CompactTabHint::Verse { "pi-mode-tab active" } else { "pi-mode-tab" },
                        onclick: move |_| ctx.pi_compact_tab.set(CompactTabHint::Verse),
                        "{tr(lang, \"குறள்\", \"Kural\")}"
                    }
                    button {
                        r#type: "button",
                        class: if pi_tab == CompactTabHint::Commentary { "pi-mode-tab active" } else { "pi-mode-tab" },
                        onclick: move |_| ctx.pi_compact_tab.set(CompactTabHint::Commentary),
                        "{tr(lang, \"உரை\", \"Comm.\")}"
                    }
                    button {
                        r#type: "button",
                        class: if pi_tab == CompactTabHint::Study { "pi-mode-tab active" } else { "pi-mode-tab" },
                        onclick: move |_| ctx.pi_compact_tab.set(CompactTabHint::Study),
                        "{tr(lang, \"ஆய்வு\", \"Study\")}"
                    }
                    button {
                        id: "pi-speed-chip",
                        r#type: "button",
                        class: "pi-scholar-cycle-pill tabular-nums",
                        aria_label: tr(lang, "பேச்சு வேகம்", "Speech speed"),
                        onclick: move |_| ctx.dispatch(BackendCommand::SetSpeechSpeed(step_speech_speed(speed, 1))),
                        "{speed:.2}×"
                    }
                    button {
                        id: "pi-scholar-cycle",
                        r#type: "button",
                        class: "pi-scholar-cycle-pill",
                        aria_label: tr(lang, "உரையாளர் மாற்று", "Cycle scholar"),
                        onclick: move |_| ctx.dispatch(BackendCommand::SelectCommentary(next_scholar(requested_scholar))),
                        "{scholar_label(effective_scholar, lang)} ↻"
                    }
                }

                // Left 6 cols (Desktop) / 7fr (7" Pi) / Full Card Stage (3.5" Pi)
                section { class: "col-span-6 adult-main-col",
                    article { id: "kural-card", class: "royal-card-primary kural-card-layout",
                        div { id: "kural-card-header", class: "kural-card-header",
                            div { style: "display:flex; align-items:center; gap:6px; min-width:0;",
                                span { id: "kural-badge-text", class: "kural-badge-pill",
                                    "{tr(lang, \"குறள் #\", \"Kural #\")}{kural_num}"
                                }
                                span { class: "kural-div-pill truncate",
                                    "{adhikaram_label}"
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
                            div { style: "display:flex; align-items:center; gap:5px; flex-shrink:0;",
                                button {
                                    id: "btn-bookmark",
                                    r#type: "button",
                                    class: "icon-circle-btn royal-btn",
                                    style: "width:34px; height:34px;",
                                    aria_label: tr(lang, "குறிப்பு சேர்/நீக்கு", "Toggle bookmark"),
                                    onclick: move |_| ctx.dispatch(BackendCommand::ToggleBookmark(kural_num)),
                                    if is_bookmarked { "★" } else { "☆" }
                                }
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

                        // Verse Block (Visible on Desktop AND Pi Verse tab)
                        div {
                            class: if pi_tab == CompactTabHint::Verse {
                                "kural-center-verse"
                            } else {
                                "kural-center-verse pi-pane-hidden"
                            },
                            div { style: "display:flex; align-items:center; justify-content:center; gap:12px; width:100%;",
                                // 3D Initial Seal strictly bound to Tamil `opening_letter_ta`
                                div {
                                    id: "kural-initial-seal",
                                    class: "royal-seal",
                                    lang: "ta",
                                    aria_label: tr(lang, "முதல் எழுத்து", "Opening Tamil letter"),
                                    "{full.kural.opening_letter_ta}"
                                }
                                // Verbatim Tamil couplet ALWAYS preserved
                                p { id: "kural-couplet", class: "font-kural", style: "text-align:left;", lang: "ta",
                                    "{full.kural.line1_ta}"
                                    br {}
                                    "{full.kural.line2_ta}"
                                }
                            }
                            p { id: "kural-translit",
                                "{full.kural.transliteration1} · {full.kural.transliteration2}"
                            }
                            if lang == Language::English {
                                p { class: "desktop-only", style: "margin-top:4px; font-size:13px; font-style:italic; opacity:0.88;",
                                    "{full.kural.translation_en}"
                                }
                            }
                        }

                        // Commentary Box (Visible on Desktop AND Pi Commentary tab)
                        div {
                            class: if pi_tab == CompactTabHint::Commentary {
                                "comm-box"
                            } else {
                                "comm-box pi-pane-hidden"
                            },
                            if show_parimel_fallback {
                                div { class: "royal-fallback-banner", role: "status",
                                    "⚠ {parimel_notice}"
                                }
                            }

                            // 5 Scholar Tabs (Desktop)
                            div { class: "comm-tabs-row desktop-only",
                                for scholar in ALL_SCHOLARS {
                                    {
                                        let active = scholar == effective_scholar;
                                        let disabled = scholar == Scholar::Parimel && parimel_missing;
                                        let mut cls = if active { "comm-tab active royal-cta-primary" } else { "comm-tab" }.to_string();
                                        if disabled {
                                            cls.push_str(" comm-tab-disabled");
                                        }
                                        rsx! {
                                            button {
                                                key: "{scholar}",
                                                r#type: "button",
                                                class: "{cls}",
                                                disabled: disabled,
                                                title: if disabled { parimel_notice } else { "" },
                                                onclick: move |_| {
                                                    if !disabled {
                                                        ctx.dispatch(BackendCommand::SelectCommentary(scholar));
                                                    }
                                                },
                                                "{scholar_label(scholar, lang)}"
                                            }
                                        }
                                    }
                                }
                            }

                            p { id: "comm-body", class: "desktop-only", style: "font-size:14px; line-height:1.48; overflow:hidden;",
                                "{comm_body}"
                            }

                            // Paginated commentary on Pi (pager sits inline in #kural-actions to guarantee 162px fit)
                            div { class: "pi-only", style: "flex:1; flex-direction:column; justify-content:center;",
                                p { id: "pi-comm-text", class: "pi-prose-text",
                                    "{comm_pages[safe_page]}"
                                }
                            }
                        }

                        // Pi Study Tab (Pi only): Daily Wisdom Reflection paginated
                        div {
                            class: if pi_tab == CompactTabHint::Study {
                                "comm-box pi-only"
                            } else {
                                "comm-box pi-only pi-pane-hidden"
                            },
                            style: "justify-content:center;",
                            p { class: "pi-prose-text",
                                "{study_pages[safe_page]}"
                            }
                        }

                        // Action Row (+ inline Pi sentence pager when in Commentary/Study tabs)
                        div { id: "kural-actions", class: "kural-actions-row",
                            if is_speaking {
                                button {
                                    id: "btn-listen",
                                    r#type: "button",
                                    class: "royal-btn action-pill-btn action-pill-stop",
                                    onclick: move |_| ctx.dispatch(BackendCommand::StopAudio),
                                    StopSquareIcon {}
                                    span { "{tr(lang, \"நிறுத்து\", \"Stop\")}" }
                                }
                            } else {
                                button {
                                    id: "btn-listen",
                                    r#type: "button",
                                    class: "royal-btn action-pill-btn action-pill-primary",
                                    onclick: move |_| ctx.dispatch(BackendCommand::SpeakCurrent(SpeakTarget::ScholarCommentary(effective_scholar))),
                                    SpeakerIcon {}
                                    span { "{tr(lang, \"கேள்\", \"Listen\")}" }
                                }
                            }
                            button {
                                r#type: "button",
                                class: "royal-btn action-pill-btn",
                                aria_label: tr(lang, "மீண்டும் கேள்", "Replay"),
                                disabled: !snap.has_replay_clip,
                                onclick: move |_| ctx.dispatch(BackendCommand::ReplayAudio),
                                ReplayIcon {}
                                span { class: "desktop-only", "{tr(lang, \"மீண்டும்\", \"Replay\")}" }
                            }
                            if (pi_tab == CompactTabHint::Commentary || pi_tab == CompactTabHint::Study)
                                && active_pages.len() > 1
                            {
                                div { class: "pager-row pi-only",
                                    button {
                                        r#type: "button",
                                        class: "pager-btn",
                                        disabled: safe_page == 0,
                                        onclick: move |_| pi_page_idx.set(safe_page.saturating_sub(1)),
                                        "◀"
                                    }
                                    span { class: "tabular-nums", "{safe_page + 1}/{active_pages.len()}" }
                                    button {
                                        r#type: "button",
                                        class: "pager-btn",
                                        disabled: safe_page + 1 >= active_pages.len(),
                                        onclick: move |_| pi_page_idx.set(safe_page + 1),
                                        "▶"
                                    }
                                }
                            }
                            button {
                                r#type: "button",
                                class: "royal-btn action-pill-btn action-pill-chat desktop-only",
                                onclick: move |_| ctx.dispatch(BackendCommand::OpenModal(ModalKind::AiChat {
                                    seed_kural: Some(kural_num),
                                })),
                                ChatIcon {}
                                span { "{tr(lang, \"விவாதி\", \"Discuss\")}" }
                            }
                        }
                    }
                }

                // Right 6 cols (Desktop) / 5fr (7" Pi)
                section { class: "col-span-6 desktop-only adult-side-col",
                    // Daily Wisdom Reflection
                    div { class: "royal-card-secondary",
                        div { style: "display:flex; justify-content:space-between; align-items:center;",
                            h2 { style: "font-size:15px; font-weight:800; color:var(--primary);",
                                "{tr(lang, \"தினசரி ஞான சிந்தனை\", \"Daily Wisdom Reflection\")}"
                            }
                            button {
                                r#type: "button",
                                class: "tag-chip",
                                onclick: move |_| ctx.dispatch(BackendCommand::OpenModal(ModalKind::AiChat {
                                    seed_kural: Some(kural_num),
                                })),
                                "{tr(lang, \"விவாதி →\", \"Discuss →\")}"
                            }
                        }
                        p { class: "line-clamp-3", style: "font-size:13.5px; line-height:1.45; margin-top:6px;",
                            "{reflection_text}"
                        }
                    }

                    // 4 Research Theme Subcards (2x2)
                    div { class: "royal-card-secondary",
                        h3 { style: "font-size:13px; font-weight:800; margin-bottom:8px;",
                            "{tr(lang, \"ஆய்வுத் தலைப்புகள்\", \"Research Themes\")}"
                        }
                        div { class: "quiz-2x2-grid",
                            for theme_val in [
                                ResearchTheme::Virtue,
                                ResearchTheme::Philosophy,
                                ResearchTheme::Polity,
                                ResearchTheme::LoveEthics,
                            ] {
                                {
                                    let (title, desc) = research_theme_label(theme_val, lang);
                                    let is_active = snap.active_filter == Some(ActiveFilter::ResearchTheme(theme_val));
                                    rsx! {
                                        button {
                                            key: "{theme_val}",
                                            r#type: "button",
                                            class: if is_active { "royal-subcard active" } else { "royal-subcard" },
                                            onclick: move |_| {
                                                if is_active {
                                                    ctx.dispatch(BackendCommand::FilterByResearchTheme(None));
                                                } else {
                                                    ctx.dispatch(BackendCommand::FilterByResearchTheme(Some(theme_val)));
                                                }
                                            },
                                            p { style: "font-size:13px; font-weight:800;", "{title}" }
                                            p { style: "font-size:11px; opacity:0.78; margin-top:2px;", "{desc}" }
                                        }
                                    }
                                }
                            }
                        }
                    }

                    // Saved Bookmarks Card
                    div { class: "royal-card-secondary", style: "flex:1; min-height:0; display:flex; flex-direction:column; gap:6px;",
                        div { style: "display:flex; justify-content:space-between; align-items:center;",
                            h3 { style: "font-size:13px; font-weight:800;",
                                "★ {tr(lang, \"சேமித்த குறள்கள்\", \"Saved Bookmarks\")} ({snap.bookmarks.len()})"
                            }
                            button {
                                r#type: "button",
                                class: if is_bookmarked { "tag-chip active" } else { "tag-chip" },
                                onclick: move |_| ctx.dispatch(BackendCommand::ToggleBookmark(kural_num)),
                                if is_bookmarked {
                                    "{tr(lang, \"★ சேமிக்கப்பட்டது\", \"★ Bookmarked\")}"
                                } else {
                                    "{tr(lang, \"☆ சேமி\", \"☆ Bookmark\")}"
                                }
                            }
                        }
                        if snap.bookmarks.is_empty() {
                            p { style: "font-size:12px; opacity:0.75;",
                                "{tr(lang, \"குறள்களைச் சேமிக்க ☆ ஐ அழுத்தவும்.\", \"Tap ☆ to bookmark Kurals for study.\")}"
                            }
                        } else {
                            div { class: "adult-bookmarks-list",
                                for bk in snap.bookmarks.iter().take(4) {
                                    {
                                        let bnum = bk.kural_number;
                                        let sch_lbl = scholar_label(bk.preferred_scholar, lang);
                                        rsx! {
                                            div { key: "{bnum}", style: "display:flex; align-items:center; gap:6px;",
                                                button {
                                                    r#type: "button",
                                                    class: "result-row-btn",
                                                    style: "min-height:32px; padding:4px 10px;",
                                                    onclick: move |_| ctx.dispatch(BackendCommand::OpenKural(bnum)),
                                                    span { class: "truncate",
                                                        strong { "#{bnum} " }
                                                        span { class: "font-kural", lang: "ta", "{bk.line1_ta}" }
                                                    }
                                                    span { class: "tag-chip", "{sch_lbl}" }
                                                }
                                                button {
                                                    r#type: "button",
                                                    class: "icon-circle-btn",
                                                    style: "width:30px; height:30px; font-size:12px;",
                                                    aria_label: tr(lang, "நீக்கு", "Remove bookmark"),
                                                    onclick: move |_| ctx.dispatch(BackendCommand::ToggleBookmark(bnum)),
                                                    "✕"
                                                }
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
                            initial_tab: SearchTab::Divisions,
                        })),
                        "📚 {tr(lang, \"பால் / அதிகாரம்\", \"Paals & Chapters\")}"
                    }
                    button {
                        r#type: "button",
                        class: "royal-btn dock-pill-btn",
                        onclick: move |_| ctx.dispatch(BackendCommand::OpenModal(ModalKind::Search {
                            initial_tab: SearchTab::Query,
                        })),
                        SearchIcon {}
                        span { "{tr(lang, \"தேடு\", \"Search\")}" }
                    }
                }

                button {
                    id: "desktop-voice-btn",
                    r#type: "button",
                    class: "royal-btn voice-bar-btn",
                    onclick: move |_| {
                        if voice_active {
                            ctx.dispatch(BackendCommand::CancelVoice);
                        } else {
                            ctx.dispatch(BackendCommand::StartVoiceCommand);
                        }
                    },
                    MicIcon { size: 16 }
                    span { "{tr(lang, \"\\\"குறள் 423\\\", \\\"சேமி\\\", \\\"பரிமேலழகர்\\\"\", \"Say \\\"Kural 423\\\", \\\"Bookmark\\\", or \\\"Parimel\\\"\")}" }
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

            // Pi 480x320 5-Slot Dock: Paals | Scholars | Voice Orb | Search | Discuss (76px)
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
                    onclick: move |_| ctx.dispatch(BackendCommand::SelectCommentary(next_scholar(requested_scholar))),
                    span { style: "font-size:14px; line-height:1;", "📜" }
                    span { class: "pi-dock-label", "{tr(lang, \"உரையாளர்\", \"Scholars\")}" }
                }
                button {
                    id: "pi-voice-orb",
                    r#type: "button",
                    onclick: move |_| {
                        if voice_active {
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
                    span { class: "pi-dock-label", "{tr(lang, \"விவாதி\", \"Discuss\")}" }
                }
            }
        }
    }
}
