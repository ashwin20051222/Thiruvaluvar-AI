use crate::components::ContentPlaybackButton;
use crate::components::header::TopHeader;
use crate::components::icons::{BookIcon, ChatIcon, MicIcon, SearchIcon, SpeakerIcon};
use crate::copy::{paginate_sentences, tr};
use crate::state::{AppContext, is_voice_active, is_voice_cancellable};
use dioxus::prelude::*;
use kural_core::{BackendCommand, ModalKind, SPEECH_SPEED_PRESETS, SearchTab, SpeakTarget, Theme};

#[component]
pub fn KidsScreen() -> Element {
    let ctx = use_context::<AppContext>();
    let boot = use_context::<crate::BootContext>();
    let verify_layout = boot.verify_layout;
    let snap = ctx.snapshot.read().clone();
    let lang = snap.prefs.lang;
    let theme = snap.prefs.theme;
    let full = &snap.current_kural;
    let kural_num = full.kural.number;
    let voice_active = is_voice_active(snap.audio_state);
    let voice_cancellable = is_voice_cancellable(snap.audio_state);

    let meaning_raw = full
        .child
        .as_ref()
        .map(|c| match lang {
            kural_core::Language::Tamil => c.simple_meaning_ta.as_str(),
            kural_core::Language::English => c.simple_meaning_en.as_str(),
        })
        .unwrap_or_else(|| tr(lang, &full.kural.meaning_mu_va, &full.kural.explanation_en));

    let pages = paginate_sentences(meaning_raw, 78);
    let mut page_idx = use_signal(|| 0_usize);
    let mut last_kural = use_signal(|| kural_num);
    use_effect(move || {
        let num = ctx.snapshot.read().current_kural.kural.number;
        if *last_kural.peek() != num {
            last_kural.set(num);
            page_idx.set(0);
        }
    });
    let safe_page = (*page_idx.read()).min(pages.len().saturating_sub(1));
    let meaning_display = &pages[safe_page];

    rsx! {
        div { class: "scene-backdrop", aria_hidden: "true",
            if theme == Theme::Light {
                div { class: "kids-sky-light",
                    div { class: "anim-spin-sun", style: "position:absolute; top:-36px; right:-36px; width:170px; height:170px; opacity:0.9;",
                        svg { view_box: "0 0 160 160", width: "170", height: "170",
                            defs {
                                radialGradient { id: "sunGrad", cx: "40%", cy: "35%", r: "65%",
                                    stop { offset: "0%", stop_color: "#FFF176" }
                                    stop { offset: "60%", stop_color: "#FFB300" }
                                    stop { offset: "100%", stop_color: "#FF6A00" }
                                }
                            }
                            circle { cx: "80", cy: "80", r: "44", fill: "url(#sunGrad)" }
                        }
                    }
                    svg { view_box: "0 0 180 80", class: "anim-float-cloud", style: "position:absolute; top:48px; left:14%; width:144px; opacity:0.85;",
                        path { d: "M35 64h110a20 20 0 0 0 4-39.6A28 28 0 0 0 96 14a26 26 0 0 0-48 10A22 22 0 0 0 35 64z", fill: "#FFFFFF" }
                    }
                }
            } else {
                div { class: "kids-sky-dark",
                    svg { view_box: "0 0 100 100", class: "anim-float-slow", style: "position:absolute; top:32px; right:48px; width:96px; height:96px;",
                        path { d: "M62 14a36 36 0 1 0 24 62 28 28 0 0 1-24-62z", fill: "#E6C068" }
                    }
                    span { class: "anim-twinkle", style: "position:absolute; top:56px; left:18%; width:9px; height:9px; border-radius:9999px; background:#E6C068;" }
                    span { class: "anim-firefly", style: "position:absolute; top:34%; left:25%; width:11px; height:11px; border-radius:9999px; background:#FFE066;" }
                    span { class: "anim-firefly", style: "position:absolute; top:48%; right:22%; width:10px; height:10px; border-radius:9999px; background:#80FFDB;" }
                }
            }
        }

        div { id: "app-shell", class: "screen-kids",
            TopHeader {}

            main { id: "main-stage", class: "main-stage",
                section { id: "left-stage", class: "col-span-7",
                    // Desktop Grandpa Greeting Banner
                    div { class: "clay-banner desktop-only",
                        div { style: "display:flex; align-items:center; gap:10px; min-width:0;",
                            img {
                                src: "/kural/valluvar-portrait.png",
                                alt: "Thiruvalluvar Grandpa",
                                class: "anim-bounce-gentle",
                                style: "width:42px; height:42px; border-radius:9999px; object-fit:cover; border:2px solid var(--gold-frame); flex-shrink:0;",
                            }
                            p { class: "truncate", style: "font-weight:700; font-size:14px;",
                                "{tr(lang, \"வணக்கம் குட்டி! இன்று ஒரு அழகான குறள் கற்போமா?\", \"Hello little star! Ready for today's fun Kural?\")}"
                            }
                        }
                        button {
                            r#type: "button",
                            class: "toy-btn action-pill-btn",
                            style: "background:var(--child-sky); color:var(--on-child-sky); border-color:transparent;",
                            onclick: move |_| ctx.dispatch(BackendCommand::OpenModal(ModalKind::Search {
                                initial_tab: SearchTab::Query,
                            })),
                            SearchIcon {}
                            span { "{tr(lang, \"தேடு\", \"Search\")}" }
                        }
                    }

                    // Kids Kural Card
                    article { id: "kural-card", class: "clay-card kural-card-layout",
                        div { id: "kural-card-header", class: "kural-card-header",
                            div { style: "display:flex; align-items:center; gap:6px; min-width:0;",
                                img {
                                    src: "/kural/valluvar-portrait.png",
                                    alt: "Grandpa",
                                    class: "pi-only",
                                    style: "width:22px; height:22px; border-radius:9999px; object-fit:cover; border:1px solid var(--gold-frame); flex-shrink:0;",
                                }
                                span { id: "kural-badge-text", class: "kural-badge-pill",
                                    "{tr(lang, \"குறள் #\", \"Kural #\")}{kural_num}"
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
                                    class: "icon-circle-btn toy-btn",
                                    style: "width:34px; height:34px;",
                                    aria_label: tr(lang, "முந்தைய குறள்", "Previous Kural"),
                                    onclick: move |_| ctx.dispatch(BackendCommand::PrevKural),
                                    "◀"
                                }
                                button {
                                    id: "next-kural",
                                    r#type: "button",
                                    class: "icon-circle-btn toy-btn",
                                    style: "width:34px; height:34px;",
                                    aria_label: tr(lang, "அடுத்த குறள்", "Next Kural"),
                                    onclick: move |_| ctx.dispatch(BackendCommand::NextKural),
                                    "▶"
                                }
                            }
                        }

                        // Strictly Verbatim Tamil Couplet + Sentence-Paginated Simple Meaning
                        div { class: "kural-center-verse",
                            p { id: "kural-couplet", class: "font-kural",
                                "{full.kural.line1_ta}"
                                br {}
                                "{full.kural.line2_ta}"
                            }
                            p { id: "kural-meaning",
                                "{meaning_display}"
                            }
                            if pages.len() > 1 {
                                div { class: "pager-row desktop-only", style: "margin-top:6px;",
                                    button {
                                        r#type: "button",
                                        class: "pager-btn",
                                        disabled: safe_page == 0,
                                        onclick: move |_| page_idx.set(safe_page.saturating_sub(1)),
                                        "◀"
                                    }
                                    span { class: "tabular-nums", "{safe_page + 1} / {pages.len()}" }
                                    button {
                                        r#type: "button",
                                        class: "pager-btn",
                                        disabled: safe_page + 1 >= pages.len(),
                                        onclick: move |_| page_idx.set(safe_page + 1),
                                        "▶"
                                    }
                                }
                            }
                        }

                        // Action Row: Auto-playback Stop / Persistent Replay Audio / Fallback Play, Talk to Grandpa (+ inline Pi pager)
                        div { id: "kural-actions", class: "kural-actions-row",
                            ContentPlaybackButton {
                                btn_base: "toy-btn",
                                button_id: Some("btn-listen"),
                                playback_status: snap.playback_status,
                                audio_state: snap.audio_state,
                                can_replay_tts: snap.can_replay_tts,
                                lang: lang,
                                fallback_target: SpeakTarget::KuralAndMeaning,
                            }
                            button {
                                r#type: "button",
                                class: "toy-btn action-pill-btn action-pill-chat",
                                onclick: move |_| {
                                    ctx.dispatch(BackendCommand::OpenModal(ModalKind::AiChat {
                                        seed_kural: Some(kural_num),
                                    }));
                                    if !verify_layout {
                                        if voice_cancellable {
                                            ctx.dispatch(BackendCommand::CancelVoice);
                                        } else {
                                            ctx.dispatch(BackendCommand::StartVoiceCommand);
                                        }
                                    }
                                },
                                ChatIcon {}
                                span { class: "desktop-only", "{tr(lang, \"தாத்தாவிடம் பேசு\", \"Talk to Grandpa\")}" }
                                span { class: "pi-only", "{tr(lang, \"தாத்தா\", \"Grandpa\")}" }
                            }
                            if pages.len() > 1 {
                                div { class: "pager-row pi-only",
                                    button {
                                        r#type: "button",
                                        class: "pager-btn",
                                        disabled: safe_page == 0,
                                        onclick: move |_| page_idx.set(safe_page.saturating_sub(1)),
                                        "◀"
                                    }
                                    span { class: "tabular-nums", "{safe_page + 1}/{pages.len()}" }
                                    button {
                                        r#type: "button",
                                        class: "pager-btn",
                                        disabled: safe_page + 1 >= pages.len(),
                                        onclick: move |_| page_idx.set(safe_page + 1),
                                        "▶"
                                    }
                                }
                            }
                        }
                    }
                }

                // Right Column (5 cols, Desktop): Story, Word Game, Listen Rhymes
                section { class: "col-span-5 desktop-only",
                    button {
                        r#type: "button",
                        class: "clay-card clay-action-story",
                        style: "flex:1; display:flex; align-items:center; justify-content:space-between; text-align:left; gap:12px;",
                        onclick: move |_| ctx.dispatch(BackendCommand::OpenModal(ModalKind::KidsStory)),
                        div {
                            span { style: "font-size:11px; font-weight:800; opacity:0.8;", "{tr(lang, \"கதை\", \"Story\")}" }
                            h2 { style: "font-size:21px; font-weight:800; margin-top:2px;", "{tr(lang, \"குறள் கதைகள்\", \"Kural Stories\")}" }
                            p { style: "font-size:13px; font-weight:600; opacity:0.85; margin-top:4px;",
                                "{tr(lang, \"குட்டிக் கதைகளும் நீதிகளும்\", \"Moral Kutty Stories from Grandpa\")}"
                            }
                        }
                        BookIcon {}
                    }

                    button {
                        r#type: "button",
                        class: "clay-card clay-action-game",
                        style: "flex:1; display:flex; align-items:center; justify-content:space-between; text-align:left; gap:12px;",
                        onclick: move |_| ctx.dispatch(BackendCommand::OpenModal(ModalKind::KidsWordGame)),
                        div {
                            span { style: "font-size:11px; font-weight:800; opacity:0.8;", "{tr(lang, \"விளையாட்டு\", \"Game\")}" }
                            h2 { style: "font-size:21px; font-weight:800; margin-top:2px;", "{tr(lang, \"சொல் விளையாட்டு\", \"Word Fill Game\")}" }
                            p { style: "font-size:13px; font-weight:600; opacity:0.85; margin-top:4px;",
                                "{tr(lang, \"விடுபட்ட சொல்லைக் கண்டுபிடி! (+1 ⭐)\", \"Find the missing Kural word! (+1 ⭐)\")}"
                            }
                        }
                        span { style: "font-size:28px;", "🎮" }
                    }

                    button {
                        r#type: "button",
                        class: "clay-card clay-action-audio",
                        style: "flex:1; display:flex; align-items:center; justify-content:space-between; text-align:left; gap:12px;",
                        onclick: move |_| ctx.dispatch(BackendCommand::SpeakCurrent(SpeakTarget::KuttyStory)),
                        div {
                            span { style: "font-size:11px; font-weight:800; opacity:0.8;", "{tr(lang, \"ஒலி\", \"Audio\")}" }
                            h3 { style: "font-size:21px; font-weight:800; margin-top:2px;", "{tr(lang, \"கதையைக் கேள்!\", \"Listen to Story\")}" }
                            p { style: "font-size:13px; font-weight:600; opacity:0.85; margin-top:4px;",
                                "{tr(lang, \"தாத்தாவின் குரலில் முழுக் கதை\", \"Narrated moral story in full voice\")}"
                            }
                        }
                        SpeakerIcon {}
                    }
                }
            }

            // Desktop Bottom Bar
            footer { class: "desktop-footer-dock desktop-only",
                div { class: "dock-pill-btn",
                    span { class: "offline-dot" }
                    span { "{tr(lang, \"ஆஃப்லைன் • Raspberry Pi 4B\", \"Offline • Raspberry Pi 4B\")}" }
                }

                button {
                    id: "desktop-voice-btn",
                    r#type: "button",
                    class: if voice_active { "toy-btn voice-bar-btn is-listening" } else { "toy-btn voice-bar-btn" },
                    onclick: move |_| {
                        if voice_cancellable {
                            ctx.dispatch(BackendCommand::CancelVoice);
                        } else {
                            ctx.dispatch(BackendCommand::StartVoiceCommand);
                        }
                    },
                    MicIcon { size: 16 }
                    span { "{tr(lang, \"\\\"அடுத்த குறள்\\\", \\\"கதை\\\", \\\"விளையாட்டு\\\" என்று பேசுங்கள்\", \"Say \\\"Next Kural\\\", \\\"Story\\\", or \\\"Word Game\\\"\")}" }
                }

                div { style: "display:flex; align-items:center; gap:8px;",
                    div { class: "speed-pill-group", aria_label: tr(lang, "பேச்சு வேகம்", "Speech speed"),
                        for &preset in &SPEECH_SPEED_PRESETS {
                            {
                                let active = (snap.prefs.speech_speed - preset).abs() < 0.02;
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

                    {
                        let muted_now = snap.prefs.muted;
                        let sound_label = if muted_now {
                            tr(lang, "ஒலி: மியூட்", "Sound: Off")
                        } else {
                            tr(lang, "ஒலி: ஆன்", "Sound: On")
                        };
                        rsx! {
                            button {
                                id: "sound-toggle",
                                r#type: "button",
                                class: "toy-btn dock-pill-btn",
                                onclick: move |_| ctx.dispatch(BackendCommand::SetMuted(!muted_now)),
                                span { if muted_now { "🔇" } else { "🔊" } }
                                span { "{sound_label}" }
                            }
                        }
                    }
                }
            }

            // Pi 480x320 5-Slot Dock: Story | Game | Voice Orb | Audio | Search (76px)
            footer { id: "pi-zone-3", class: "pi-only pi-grid-only",
                button {
                    r#type: "button",
                    class: "pi-dock-tile clay-action-story",
                    onclick: move |_| ctx.dispatch(BackendCommand::OpenModal(ModalKind::KidsStory)),
                    BookIcon {}
                    span { class: "pi-dock-label", "{tr(lang, \"கதை\", \"Story\")}" }
                }
                button {
                    r#type: "button",
                    class: "pi-dock-tile clay-action-game",
                    onclick: move |_| ctx.dispatch(BackendCommand::OpenModal(ModalKind::KidsWordGame)),
                    span { style: "font-size:15px; line-height:1;", "🎮" }
                    span { class: "pi-dock-label", "{tr(lang, \"விளையாட்டு\", \"Game\")}" }
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
                    class: "pi-dock-tile clay-action-audio",
                    onclick: move |_| ctx.dispatch(BackendCommand::SpeakCurrent(SpeakTarget::KuttyStory)),
                    SpeakerIcon {}
                    span { class: "pi-dock-label", "{tr(lang, \"கேள்\", \"Audio\")}" }
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
            }
        }
    }
}
