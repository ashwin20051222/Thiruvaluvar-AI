//! All 6 zero-scroll paginated overlay modals (`max-height: 304px` on `480×320` Pi LCD):
//! Search (Query + 3-level Divisions drill-down), AiChat, KidsStory,
//! KidsWordGame (`2×2` grid), YouthQuiz (`2×2` grid), and YouthDeepDive.

use crate::components::ContentPlaybackButton;
use crate::components::icons::{MicIcon, ReplayIcon, SearchIcon, SpeakerIcon, StopSquareIcon};
use crate::copy::{mask_game_blank, paginate_sentences, tr};
use crate::state::{AppContext, is_voice_active, is_voice_cancellable};
use dioxus::prelude::*;
use kural_core::{
    AudioPlaybackStatus, AudioState, BackendCommand, Language, ModalKind, SearchTab, SpeakTarget,
    VoiceReplayMode,
};

#[component]
pub fn ModalHost() -> Element {
    let ctx = use_context::<AppContext>();
    let snap = ctx.snapshot.read().clone();
    let Some(modal) = snap.modal else {
        return rsx! {};
    };
    let lang = snap.prefs.lang;

    rsx! {
        div {
            class: "modal-backdrop",
            role: "dialog",
            aria_modal: "true",
            onclick: move |_| ctx.dispatch(BackendCommand::CloseModal),

            div {
                class: "modal-card",
                onclick: move |evt| evt.stop_propagation(),

                match modal {
                    ModalKind::Search { initial_tab } => rsx! {
                        SearchModal { initial_tab }
                    },
                    ModalKind::AiChat { seed_kural } => rsx! {
                        AiChatModal { seed_kural }
                    },
                    ModalKind::KidsStory => rsx! {
                        KidsStoryModal {}
                    },
                    ModalKind::KidsWordGame => rsx! {
                        KidsWordGameModal {}
                    },
                    ModalKind::YouthQuiz => rsx! {
                        YouthQuizModal {}
                    },
                    ModalKind::YouthDeepDive => rsx! {
                        YouthDeepDiveModal {}
                    },
                }

                // Always keep an accessible close hint when lang changes
                span { class: "hidden", "{tr(lang, \"மூடு\", \"Close\")}" }
            }
        }
    }
}

#[component]
fn ModalHeader(title: String, #[props(default)] badge: Option<String>) -> Element {
    let ctx = use_context::<AppContext>();
    let lang = ctx.lang();
    rsx! {
        div { class: "modal-header",
            h2 { class: "truncate", style: "font-size:16px; font-weight:800;", "{title}" }
            if let Some(b) = badge {
                span { class: "active-replay-badge", "{b}" }
            }
            button {
                r#type: "button",
                class: "modal-close-btn",
                aria_label: tr(lang, "மூடு", "Close"),
                onclick: move |_| ctx.dispatch(BackendCommand::CloseModal),
                "✕"
            }
        }
    }
}

// -----------------------------------------------------------------------------
// 1. Search Modal (Query + Divisions 3-level drill-down)
// -----------------------------------------------------------------------------

#[component]
fn SearchModal(initial_tab: SearchTab) -> Element {
    let ctx = use_context::<AppContext>();
    let lang = ctx.lang();

    let mut active_tab = use_signal(|| initial_tab);
    let mut query_text = use_signal(String::new);
    let mut res_page = use_signal(|| 0_usize);
    let mut div_page = use_signal(|| 0_usize);
    let mut selected_div: Signal<Option<u8>> = use_signal(|| None);
    let mut selected_adh: Signal<Option<u8>> = use_signal(|| None);

    let outcome = ctx.search_outcome.read().clone();
    let catalog = ctx.backend.read().catalog().clone();

    let submit_label = tr(lang, "இதைக் கருதினீர்களா:", "Did you mean:");

    rsx! {
        ModalHeader { title: tr(lang, "திருக்குறள் தேடல் & பிரிவுகள்", "Search & Divisions").to_string() }
        div { class: "modal-body",
            div { class: "modal-tabs-row",
                button {
                    r#type: "button",
                    class: if *active_tab.read() == SearchTab::Query { "modal-tab-btn active" } else { "modal-tab-btn" },
                    onclick: move |_| {
                        active_tab.set(SearchTab::Query);
                        res_page.set(0);
                    },
                    "{tr(lang, \"தேடல்\", \"Search\")}"
                }
                button {
                    r#type: "button",
                    class: if *active_tab.read() == SearchTab::Divisions { "modal-tab-btn active" } else { "modal-tab-btn" },
                    onclick: move |_| {
                        active_tab.set(SearchTab::Divisions);
                        div_page.set(0);
                    },
                    "{tr(lang, \"பால்கள் & அதிகாரம்\", \"Paals & Chapters\")}"
                }
            }

            if *active_tab.read() == SearchTab::Query {
                div { class: "search-input-row",
                    input {
                        r#type: "text",
                        class: "text-input",
                        placeholder: tr(lang, "குறள் எண், சொல் அல்லது பொருள்…", "Number, Tamil/English word, or theme…"),
                        value: "{query_text}",
                        oninput: move |e| query_text.set(e.value()),
                        onkeydown: move |e| {
                            if e.key() == Key::Enter {
                                run_search(ctx, res_page, *selected_div.read(), query_text.read().clone());
                            }
                        },
                    }
                    button {
                        r#type: "button",
                        class: "royal-btn action-pill-btn action-pill-primary",
                        onclick: move |_| run_search(ctx, res_page, *selected_div.read(), query_text.read().clone()),
                        SearchIcon {}
                        span { "{tr(lang, \"தேடு\", \"Go\")}" }
                    }
                }

                if let Some(out) = outcome {
                    if let Some(suggest) = out.did_you_mean.clone() {
                        {
                            let suggest_click = suggest.clone();
                            rsx! {
                                button {
                                    r#type: "button",
                                    class: "tag-chip",
                                    onclick: move |_| {
                                        query_text.set(suggest_click.clone());
                                        run_search(ctx, res_page, *selected_div.read(), suggest_click.clone());
                                    },
                                    "{submit_label} {suggest}"
                                }
                            }
                        }
                    }

                    if out.hits.is_empty() {
                        p { style: "font-size:13px; color:var(--text-muted); text-align:center; padding:12px 0;",
                            "{tr(lang, \"பொருந்தும் குறள்கள் இல்லை.\", \"No matching Kurals found.\")}"
                        }
                    } else {
                        {
                            let per_page = if out.did_you_mean.is_some() { 2_usize } else { 3_usize };
                            let total_pages = out.hits.len().div_ceil(per_page).max(1);
                            let safe = (*res_page.read()).min(total_pages - 1);
                            let slice = &out.hits[safe * per_page..((safe + 1) * per_page).min(out.hits.len())];
                            rsx! {
                                div { style: "display:flex; flex-direction:column; gap:5px; flex:1; min-height:0;",
                                    for hit in slice {
                                        {
                                            let num = hit.kural.number;
                                            let adh = tr(lang, &hit.kural.adhikaram_ta, &hit.kural.adhikaram_en);
                                            let speak_text = match lang {
                                                Language::Tamil => format!("{} {}", hit.kural.line1_ta, hit.kural.line2_ta),
                                                Language::English => {
                                                    if hit.kural.translation_en.trim().is_empty() {
                                                        format!("{} {}", hit.kural.line1_ta, hit.kural.line2_ta)
                                                    } else {
                                                        hit.kural.translation_en.clone()
                                                    }
                                                }
                                            };
                                            rsx! {
                                                div {
                                                    key: "{num}",
                                                    class: "search-hit-row",
                                                    button {
                                                        r#type: "button",
                                                        class: "result-row-btn",
                                                        onclick: move |_| {
                                                            ctx.dispatch(BackendCommand::OpenKural(num));
                                                            ctx.dispatch(BackendCommand::CloseModal);
                                                        },
                                                        span { class: "truncate",
                                                            strong { "#{num} " }
                                                            span { class: "font-kural", lang: "ta", "{hit.kural.line1_ta}" }
                                                        }
                                                        span { class: "tag-chip", style: "flex-shrink:0;", "{adh}" }
                                                    }
                                                    button {
                                                        r#type: "button",
                                                        class: "search-listen-btn",
                                                        title: tr(lang, "கேள்", "Listen"),
                                                        aria_label: tr(lang, "குறளைக் கேள்", "Listen to Kural"),
                                                        onclick: move |_| {
                                                            ctx.dispatch(BackendCommand::SpeakCustomText {
                                                                text: speak_text.clone(),
                                                                lang,
                                                                label: Some(format!("#{num}")),
                                                            });
                                                        },
                                                        SpeakerIcon {}
                                                    }
                                                }
                                            }
                                        }
                                    }
                                }
                                if total_pages > 1 {
                                    div { class: "pager-row",
                                        button {
                                            r#type: "button",
                                            class: "pager-btn",
                                            disabled: safe == 0,
                                            onclick: move |_| res_page.set(safe.saturating_sub(1)),
                                            "◀"
                                        }
                                        span { class: "tabular-nums", "{safe + 1} / {total_pages}" }
                                        button {
                                            r#type: "button",
                                            class: "pager-btn",
                                            disabled: safe + 1 >= total_pages,
                                            onclick: move |_| res_page.set(safe + 1),
                                            "▶"
                                        }
                                    }
                                }
                            }
                        }
                    }
                } else {
                    p { style: "font-size:13px; color:var(--text-muted); text-align:center; padding:12px 0;",
                        "{tr(lang, \"1 முதல் 1330 குறள்களுக்குள் தேடுங்கள்.\", \"Search across all 1,330 Kurals offline.\")}"
                    }
                }
            } else {
                // Divisions 3-level drill-down (3–4 rows/page so Pi <= 304px zero-scroll holds)
                {
                    let div_sel = *selected_div.read();
                    let adh_sel = *selected_adh.read();
                    if let Some(adh_id) = adh_sel {
                        let kurals = ctx.backend.read().kurals_in_adhikaram(adh_id).unwrap_or_default();
                        let per_page = 3_usize;
                        let total_pages = kurals.len().div_ceil(per_page).max(1);
                        let safe = (*div_page.read()).min(total_pages - 1);
                        let slice = if kurals.is_empty() {
                            &[][..]
                        } else {
                            &kurals[safe * per_page..((safe + 1) * per_page).min(kurals.len())]
                        };
                        rsx! {
                            div { style: "display:flex; justify-content:space-between; align-items:center;",
                                button {
                                    r#type: "button",
                                    class: "tag-chip",
                                    onclick: move |_| {
                                        selected_adh.set(None);
                                        div_page.set(0);
                                    },
                                    "← {tr(lang, \"அதிகாரங்கள்\", \"Chapters\")}"
                                }
                                button {
                                    r#type: "button",
                                    class: "tag-chip active",
                                    onclick: move |_| {
                                        ctx.dispatch(BackendCommand::OpenAdhikaram(adh_id));
                                        ctx.dispatch(BackendCommand::CloseModal);
                                    },
                                    "{tr(lang, \"முதல் குறள் திற\", \"Open Chapter\")}"
                                }
                            }
                            div { style: "display:flex; flex-direction:column; gap:5px; flex:1; min-height:0;",
                                for k in slice {
                                    {
                                        let num = k.number;
                                        let speak_text = match lang {
                                            Language::Tamil => format!("{} {}", k.line1_ta, k.line2_ta),
                                            Language::English => {
                                                if k.translation_en.trim().is_empty() {
                                                    format!("{} {}", k.line1_ta, k.line2_ta)
                                                } else {
                                                    k.translation_en.clone()
                                                }
                                            }
                                        };
                                        rsx! {
                                            div {
                                                key: "{num}",
                                                class: "search-hit-row",
                                                button {
                                                    r#type: "button",
                                                    class: "result-row-btn",
                                                    onclick: move |_| {
                                                        ctx.dispatch(BackendCommand::OpenKural(num));
                                                        ctx.dispatch(BackendCommand::CloseModal);
                                                    },
                                                    span { class: "truncate",
                                                        strong { "#{num} " }
                                                        span { class: "font-kural", lang: "ta", "{k.line1_ta}" }
                                                    }
                                                }
                                                button {
                                                    r#type: "button",
                                                    class: "search-listen-btn",
                                                    title: tr(lang, "கேள்", "Listen"),
                                                    aria_label: tr(lang, "குறளைக் கேள்", "Listen to Kural"),
                                                    onclick: move |_| {
                                                        ctx.dispatch(BackendCommand::SpeakCustomText {
                                                            text: speak_text.clone(),
                                                            lang,
                                                            label: Some(format!("#{num}")),
                                                        });
                                                    },
                                                    SpeakerIcon {}
                                                }
                                            }
                                        }
                                    }
                                }
                            }
                            if total_pages > 1 {
                                div { class: "pager-row",
                                    button {
                                        r#type: "button",
                                        class: "pager-btn",
                                        disabled: safe == 0,
                                        onclick: move |_| div_page.set(safe.saturating_sub(1)),
                                        "◀"
                                    }
                                    span { class: "tabular-nums", "{safe + 1} / {total_pages}" }
                                    button {
                                        r#type: "button",
                                        class: "pager-btn",
                                        disabled: safe + 1 >= total_pages,
                                        onclick: move |_| div_page.set(safe + 1),
                                        "▶"
                                    }
                                }
                            }
                        }
                    } else if let Some(div_id) = div_sel {
                        let adhikarams: Vec<_> = catalog
                            .adhikarams
                            .iter()
                            .filter(|a| a.division_id == div_id)
                            .collect();
                        let per_page = 3_usize;
                        let total_pages = adhikarams.len().div_ceil(per_page).max(1);
                        let safe = (*div_page.read()).min(total_pages - 1);
                        let slice = &adhikarams[safe * per_page..((safe + 1) * per_page).min(adhikarams.len())];
                        rsx! {
                            div { style: "display:flex; justify-content:space-between; align-items:center;",
                                button {
                                    r#type: "button",
                                    class: "tag-chip",
                                    onclick: move |_| {
                                        selected_div.set(None);
                                        div_page.set(0);
                                    },
                                    "← {tr(lang, \"பால்கள்\", \"Paals\")}"
                                }
                                button {
                                    r#type: "button",
                                    class: "tag-chip active",
                                    onclick: move |_| {
                                        ctx.dispatch(BackendCommand::FilterByDivision(Some(div_id)));
                                        ctx.dispatch(BackendCommand::CloseModal);
                                    },
                                    "{tr(lang, \"இப்பாலில் வடிகட்டு\", \"Filter This Paal\")}"
                                }
                            }
                            div { style: "display:flex; flex-direction:column; gap:5px; flex:1; min-height:0;",
                                for adh in slice {
                                    {
                                        let id = adh.id;
                                        let title = tr(lang, &adh.name_ta, &adh.name_en);
                                        rsx! {
                                            button {
                                                key: "{id}",
                                                r#type: "button",
                                                class: "result-row-btn",
                                                onclick: move |_| {
                                                    selected_adh.set(Some(id));
                                                    div_page.set(0);
                                                },
                                                span { class: "truncate", "{id}. {title}" }
                                                span { class: "tabular-nums", "#{adh.kural_start}–{adh.kural_end}" }
                                            }
                                        }
                                    }
                                }
                            }
                            if total_pages > 1 {
                                div { class: "pager-row",
                                    button {
                                        r#type: "button",
                                        class: "pager-btn",
                                        disabled: safe == 0,
                                        onclick: move |_| div_page.set(safe.saturating_sub(1)),
                                        "◀"
                                    }
                                    span { class: "tabular-nums", "{safe + 1} / {total_pages}" }
                                    button {
                                        r#type: "button",
                                        class: "pager-btn",
                                        disabled: safe + 1 >= total_pages,
                                        onclick: move |_| div_page.set(safe + 1),
                                        "▶"
                                    }
                                }
                            }
                        }
                    } else {
                        rsx! {
                            div { style: "display:flex; flex-direction:column; gap:6px; flex:1; min-height:0;",
                                for div in &catalog.divisions {
                                    {
                                        let id = div.id;
                                        let name = tr(lang, &div.name_ta, &div.name_en);
                                        rsx! {
                                            button {
                                                key: "{id}",
                                                r#type: "button",
                                                class: "result-row-btn",
                                                onclick: move |_| {
                                                    selected_div.set(Some(id));
                                                    div_page.set(0);
                                                },
                                                span { strong { "{name}" } }
                                                span { class: "tabular-nums", "#{div.kural_start}–{div.kural_end} →" }
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
    }
}

// -----------------------------------------------------------------------------
// 2. AI Chat Modal (48x48 Voice Orb + 3 Quick Prompt Chips + Paginated Answer)
// -----------------------------------------------------------------------------

fn run_search(ctx: AppContext, mut res_page: Signal<usize>, division_id: Option<u8>, q: String) {
    res_page.set(0);
    if !q.trim().is_empty() {
        ctx.dispatch(BackendCommand::SubmitTextSearch {
            query: q,
            division_id,
        });
    }
}

fn run_ai_question(
    ctx: AppContext,
    mut ans_page: Signal<usize>,
    seed_kural: Option<u16>,
    q: String,
) {
    if !q.trim().is_empty() {
        ans_page.set(0);
        ctx.dispatch(BackendCommand::AskAiQuestion {
            query: q,
            seed_kural,
        });
    }
}

#[component]
fn AiChatModal(seed_kural: Option<u16>) -> Element {
    let ctx = use_context::<AppContext>();
    let snap = ctx.snapshot.read().clone();
    let lang = snap.prefs.lang;
    let voice_active = is_voice_active(snap.audio_state);
    let voice_cancellable = is_voice_cancellable(snap.audio_state);
    let is_speaking = snap.audio_state == AudioState::PlayingTts;
    let answer = ctx.ai_answer.read().clone();

    let mut prompt = use_signal(String::new);
    let mut ans_page = use_signal(|| 0_usize);
    let mut last_ans_len = use_signal(|| 0_usize);
    use_effect(move || {
        let len = ctx
            .ai_answer
            .read()
            .as_ref()
            .map(|a| a.answer_ta.len())
            .unwrap_or(0);
        if *last_ans_len.peek() != len {
            last_ans_len.set(len);
            ans_page.set(0);
        }
    });

    let chips: [(&str, &str); 3] = [
        ("இதன் பொருள் என்ன?", "Explain this Kural"),
        ("வாழ்க்கை உதாரணம்", "Give a life example"),
        ("திருவள்ளுவர் யார்?", "Who is Thiruvalluvar?"),
    ];
    let has_answer = answer.is_some();
    let auto_speak_on = snap.auto_speak_text;
    let voice_loop_on = snap.voice_to_voice_loop;
    let latest_turn = snap.recent_voice_turns.last().cloned();
    let show_replay_strip = snap.can_replay_voice_turn && latest_turn.is_some();
    let auto_speak_full = if snap.auto_speak_text {
        tr(lang, "🔊 தானியங்கி பதில்: ஆம்", "🔊 Auto-Speak: ON")
    } else {
        tr(lang, "🔇 தானியங்கி பதில்: இல்லை", "🔇 Auto-Speak: OFF")
    };
    let auto_speak_short = if snap.auto_speak_text {
        tr(lang, "🔊 பதில் ஒலி: ஆம்", "🔊 Auto-TTS: ON")
    } else {
        tr(lang, "🔇 பதில் ஒலி: இல்", "🔇 Auto-TTS: OFF")
    };
    let loop_full = if snap.voice_to_voice_loop {
        tr(lang, "🎙️ தொடர் குரல்: ஆம்", "🎙️ Auto-Voice: ON")
    } else {
        tr(lang, "🎙️ தொடர் குரல்: இல்", "🎙️ Auto-Voice: OFF")
    };
    let loop_short = if snap.voice_to_voice_loop {
        tr(lang, "🎙️ தொடர்: ஆம்", "🎙️ Loop: ON")
    } else {
        tr(lang, "🎙️ தொடர்: இல்", "🎙️ Loop: OFF")
    };

    rsx! {
        ModalHeader {
            title: match seed_kural {
                Some(n) => format!("{} (#{n})", tr(lang, "வள்ளுவர் AI உரையாடல்", "Ask Valluvar AI")),
                None => tr(lang, "வள்ளுவர் AI உரையாடல்", "Ask Valluvar AI").to_string(),
            },
            badge: snap.active_replay_label.clone(),
        }
        div { class: if has_answer { "modal-body ai-modal-has-answer" } else { "modal-body" },
            div { class: "ai-input-bar",
                button {
                    r#type: "button",
                    class: if voice_active {
                        "icon-circle-btn action-pill-stop ai-mic-btn is-listening"
                    } else {
                        "icon-circle-btn action-pill-primary ai-mic-btn"
                    },
                    aria_label: tr(lang, "பேசி கேள்", "Ask by voice"),
                    onclick: move |_| {
                        if voice_cancellable {
                            ctx.dispatch(BackendCommand::CancelVoice);
                        } else {
                            ctx.dispatch(BackendCommand::StartVoiceCommand);
                        }
                    },
                    MicIcon { size: 22 }
                }
                div { class: "search-input-row", style: "flex:1; min-width:0;",
                    input {
                        r#type: "text",
                        class: "text-input",
                        placeholder: tr(lang, "குறள் குறித்து கேளுங்கள்…", "Ask about Thirukkural…"),
                        value: "{prompt}",
                        oninput: move |e| prompt.set(e.value()),
                        onkeydown: move |e| {
                            if e.key() == Key::Enter {
                                let q = prompt.read().clone();
                                prompt.set(String::new());
                                run_ai_question(ctx, ans_page, seed_kural, q);
                            }
                        },
                    }
                    button {
                        r#type: "button",
                        class: "royal-btn action-pill-btn action-pill-primary",
                        onclick: move |_| {
                            let q = prompt.read().clone();
                            prompt.set(String::new());
                            run_ai_question(ctx, ans_page, seed_kural, q);
                        },
                        "{tr(lang, \"கேள்\", \"Ask\")}"
                    }
                }
            }

            // 3 quick prompt chips (>= 38px touch friendly; hidden on 480x320 once answered)
            div { class: "ai-quick-chips",
                for (ta, en) in chips {
                    button {
                        key: "{en}",
                        r#type: "button",
                        class: "tag-chip",
                        style: "min-height:38px;",
                        onclick: move |_| run_ai_question(ctx, ans_page, seed_kural, tr(lang, ta, en).to_string()),
                        "{tr(lang, ta, en)}"
                    }
                }
            }

            // Auto-Speak Text & Hands-Free Voice-to-Voice Loop toggles
            div { class: "voice-toggles-row",
                button {
                    r#type: "button",
                    class: if auto_speak_on { "voice-toggle-btn active" } else { "voice-toggle-btn" },
                    aria_pressed: auto_speak_on,
                    onclick: move |_| ctx.dispatch(BackendCommand::SetAutoSpeakText(!auto_speak_on)),
                    span { class: "desktop-only", "{auto_speak_full}" }
                    span { class: "pi-only", "{auto_speak_short}" }
                }
                button {
                    r#type: "button",
                    class: if voice_loop_on { "voice-toggle-btn active" } else { "voice-toggle-btn" },
                    aria_pressed: voice_loop_on,
                    onclick: move |_| ctx.dispatch(BackendCommand::SetVoiceToVoiceLoop(!voice_loop_on)),
                    span { class: "desktop-only", "{loop_full}" }
                    span { class: "pi-only", "{loop_short}" }
                }
            }

            if let Some(ans) = answer {
                {
                    let body = tr(lang, &ans.answer_ta, &ans.answer_en);
                    let pages = paginate_sentences(body, 90);
                    let safe = (*ans_page.read()).min(pages.len().saturating_sub(1));
                    let speak_body = body.trim().to_string();
                    let speak_label = seed_kural.map(|n| format!("#{n}"));
                    let heard_text = latest_turn
                        .as_ref()
                        .map(|t| t.user_text.trim().to_string())
                        .filter(|t| !t.is_empty());
                    rsx! {
                        div {
                            class: if ans.out_of_scope { "royal-fallback-banner ai-answer-card" } else { "royal-subcard ai-answer-card" },
                            style: "flex:1; min-height:0; display:flex; flex-direction:column; justify-content:space-between; gap:6px;",
                            if let Some(heard) = heard_text {
                                p {
                                    class: "desktop-only truncate",
                                    style: "font-size:12px; opacity:0.72; margin:0;",
                                    "🎤 {heard}"
                                }
                            }
                            p { style: "font-size:14px; line-height:1.45;", "{pages[safe]}" }
                            div { style: "display:flex; align-items:center; justify-content:space-between; gap:6px; flex-wrap:wrap;",
                                div { style: "display:flex; gap:4px; flex-wrap:wrap;",
                                    for cited in &ans.cited_kurals {
                                        {
                                            let num = cited.kural.number;
                                            let snippet = cited.kural.line1_ta.clone();
                                            rsx! {
                                                button {
                                                    key: "{num}",
                                                    r#type: "button",
                                                    class: "cited-badge-btn",
                                                    title: "{snippet}",
                                                    onclick: move |_| {
                                                        ctx.dispatch(BackendCommand::OpenKural(num));
                                                        ctx.dispatch(BackendCommand::CloseModal);
                                                    },
                                                    span { "#{num} " }
                                                    span { class: "font-kural truncate desktop-only", lang: "ta", style: "max-width:150px;", "{snippet}" }
                                                }
                                            }
                                        }
                                    }
                                }
                                div { style: "display:flex; align-items:center; gap:6px;",
                                    if pages.len() > 1 {
                                        div { class: "pager-row",
                                            button {
                                                r#type: "button",
                                                class: "pager-btn",
                                                disabled: safe == 0,
                                                onclick: move |_| ans_page.set(safe.saturating_sub(1)),
                                                "◀"
                                            }
                                            span { class: "tabular-nums", "{safe + 1}/{pages.len()}" }
                                            button {
                                                r#type: "button",
                                                class: "pager-btn",
                                                disabled: safe + 1 >= pages.len(),
                                                onclick: move |_| ans_page.set(safe + 1),
                                                "▶"
                                            }
                                        }
                                    }
                                    if !ans.out_of_scope {
                                        if is_speaking || matches!(
                                            snap.playback_status,
                                            AudioPlaybackStatus::Loading
                                                | AudioPlaybackStatus::Playing
                                                | AudioPlaybackStatus::Replaying
                                        ) {
                                            button {
                                                r#type: "button",
                                                class: "tag-chip",
                                                style: "min-height:38px;",
                                                onclick: move |_| ctx.dispatch(BackendCommand::StopAudio),
                                                StopSquareIcon {}
                                                span { " {tr(lang, \"நிறுத்து\", \"Stop\")}" }
                                            }
                                        } else if snap.playback_status == AudioPlaybackStatus::Completed && snap.can_replay_tts {
                                            button {
                                                r#type: "button",
                                                class: "tag-chip active",
                                                style: "min-height:38px;",
                                                onclick: move |_| ctx.dispatch(BackendCommand::ReplayLastTts),
                                                ReplayIcon {}
                                                span { " {tr(lang, \"மீண்டும் கேள்\", \"Replay Audio\")}" }
                                            }
                                        } else {
                                            button {
                                                r#type: "button",
                                                class: "tag-chip active",
                                                style: "min-height:38px;",
                                                onclick: move |_| {
                                                    ctx.dispatch(BackendCommand::SpeakCustomText {
                                                        text: speak_body.clone(),
                                                        lang,
                                                        label: speak_label.clone(),
                                                    });
                                                },
                                                SpeakerIcon {}
                                                span { " {tr(lang, \"ஒலியை இயக்கு\", \"Play Audio\")}" }
                                            }
                                        }
                                    }
                                }
                            }
                        }
                    }
                }
            } else {
                div {
                    id: "ai-empty-prompt",
                    class: "royal-subcard",
                    style: "padding:5px 10px; font-size:13px; line-height:1.32; opacity:0.82;",
                    "{tr(lang, \"குறள் எண், பொருள் அல்லது திருவள்ளுவர் குறித்து கேளுங்கள் — அல்லது மேலுள்ள கேள்விகளில் ஒன்றைத் தேர்வு செய்யுங்கள்.\", \"Ask by voice or text about any Kural, meaning, or tap a starter question above.\")}"
                }
            }

            if show_replay_strip {
                if let Some(turn) = latest_turn {
                    {
                        let turn_id = turn.id;
                        let has_user = turn.has_user_audio;
                        let has_ai = turn.has_ai_audio;
                        rsx! {
                            div { class: "voice-replay-strip",
                                button {
                                    r#type: "button",
                                    class: "voice-replay-btn",
                                    disabled: !has_user,
                                    aria_label: tr(lang, "என் குரலை மீண்டும் போடு", "Replay My Voice"),
                                    onclick: move |_| ctx.dispatch(BackendCommand::ReplayVoiceTurn {
                                        turn_id: Some(turn_id),
                                        mode: VoiceReplayMode::UserVoiceOnly,
                                    }),
                                    ReplayIcon {}
                                    span { class: "desktop-only", "{tr(lang, \"என் குரல்\", \"My Voice\")}" }
                                    span { class: "pi-only", "{tr(lang, \"குரல்\", \"Mine\")}" }
                                }
                                button {
                                    r#type: "button",
                                    class: "voice-replay-btn",
                                    disabled: !has_ai,
                                    aria_label: tr(lang, "AI பதிலை மீண்டும் போடு", "Replay AI Reply"),
                                    onclick: move |_| ctx.dispatch(BackendCommand::ReplayVoiceTurn {
                                        turn_id: Some(turn_id),
                                        mode: VoiceReplayMode::AiReplyOnly,
                                    }),
                                    SpeakerIcon {}
                                    span { class: "desktop-only", "{tr(lang, \"AI பதில்\", \"AI Reply\")}" }
                                    span { class: "pi-only", "{tr(lang, \"பதில்\", \"AI\")}" }
                                }
                                button {
                                    r#type: "button",
                                    class: "voice-replay-btn",
                                    disabled: !(has_user && has_ai),
                                    aria_label: tr(lang, "முழு உரையாடலை மீண்டும் போடு", "Replay Full Exchange"),
                                    onclick: move |_| ctx.dispatch(BackendCommand::ReplayVoiceTurn {
                                        turn_id: Some(turn_id),
                                        mode: VoiceReplayMode::FullExchange,
                                    }),
                                    ReplayIcon {}
                                    span { class: "desktop-only", "{tr(lang, \"முழு உரையாடல்\", \"Full Exchange\")}" }
                                    span { class: "pi-only", "{tr(lang, \"முழுதும்\", \"Both\")}" }
                                }
                            }
                        }
                    }
                }
            }
        }
    }
}

// -----------------------------------------------------------------------------
// 3. Kids Story Modal
// -----------------------------------------------------------------------------

#[component]
fn KidsStoryModal() -> Element {
    let ctx = use_context::<AppContext>();
    let snap = ctx.snapshot.read().clone();
    let lang = snap.prefs.lang;
    let full = &snap.current_kural;
    let kural_num = full.kural.number;
    let done = snap.kids_progress.completed_stories.contains(&kural_num);

    let (title, body, moral) = full
        .child
        .as_ref()
        .map(|c| {
            (
                tr(lang, &c.story_title_ta, &c.story_title_en),
                tr(lang, &c.kutty_story_ta, &c.kutty_story_en),
                tr(lang, &c.moral_ta, &c.moral_en),
            )
        })
        .unwrap_or(("", "", ""));

    let combined = format!("{body} {moral}");
    let pages = paginate_sentences(&combined, 88);
    let mut page_idx = use_signal(|| 0_usize);
    let mut last_kural = use_signal(|| kural_num);
    use_effect(move || {
        let num = ctx.snapshot.read().current_kural.kural.number;
        if *last_kural.peek() != num {
            last_kural.set(num);
            page_idx.set(0);
        }
    });
    let safe = (*page_idx.read()).min(pages.len().saturating_sub(1));

    rsx! {
        ModalHeader { title: format!("📖 {title}") }
        div { class: "modal-body", style: "justify-content:space-between;",
            p { style: "font-size:14.5px; line-height:1.48;", "{pages[safe]}" }

            if pages.len() > 1 {
                div { class: "pager-row",
                    button {
                        r#type: "button",
                        class: "pager-btn",
                        disabled: safe == 0,
                        onclick: move |_| page_idx.set(safe.saturating_sub(1)),
                        "◀"
                    }
                    span { class: "tabular-nums", "{safe + 1} / {pages.len()}" }
                    button {
                        r#type: "button",
                        class: "pager-btn",
                        disabled: safe + 1 >= pages.len(),
                        onclick: move |_| page_idx.set(safe + 1),
                        "▶"
                    }
                }
            }

            div { class: "kural-actions-row",
                ContentPlaybackButton {
                    btn_base: "toy-btn",
                    button_id: None,
                    playback_status: snap.playback_status,
                    audio_state: snap.audio_state,
                    can_replay_tts: snap.can_replay_tts,
                    lang: lang,
                    fallback_target: SpeakTarget::KuttyStory,
                }
                button {
                    r#type: "button",
                    class: "toy-btn action-pill-btn action-pill-chat",
                    disabled: done,
                    onclick: move |_| ctx.dispatch(BackendCommand::CompleteKidsStory(kural_num)),
                    if done {
                        "⭐ {tr(lang, \"முடிந்தது!\", \"Completed!\")}"
                    } else {
                        "📖 {tr(lang, \"கதை முடிந்தது\", \"Finish Story\")}"
                    }
                }
            }
        }
    }
}

// -----------------------------------------------------------------------------
// 4. Kids Word Game Modal (2x2 grid, masked blank word, paired TA+EN options)
// -----------------------------------------------------------------------------

#[component]
fn KidsWordGameModal() -> Element {
    let mut ctx = use_context::<AppContext>();
    let snap = ctx.snapshot.read().clone();
    let lang = snap.prefs.lang;
    let full = &snap.current_kural;
    let kural_num = full.kural.number;

    let eval = ctx
        .kids_game_eval
        .read()
        .clone()
        .filter(|e| e.kural_number == kural_num);
    let chosen = *ctx.last_kids_choice.read();

    let Some(c) = &full.child else {
        return rsx! { ModalHeader { title: tr(lang, "சொல் விளையாட்டு", "Word Game").to_string() } };
    };
    let (masked_1, masked_2) = mask_game_blank(
        &full.kural.line1_ta,
        &full.kural.line2_ta,
        &c.game_blank_word_ta,
    );

    rsx! {
        ModalHeader { title: tr(lang, "🎮 சொல் நிரப்பு விளையாட்டு", "🎮 Word Fill Game").to_string() }
        div { class: "modal-body", style: "justify-content:space-between;",
            div { style: "text-align:center;",
                p { class: "font-kural", style: "font-size:15px; line-height:1.45; font-weight:700;", lang: "ta",
                    "{masked_1}"
                    br {}
                    "{masked_2}"
                }
                if lang == kural_core::Language::English {
                    p { style: "font-size:12px; color:var(--text-muted); margin-top:2px;",
                        "{c.simple_meaning_en}"
                    }
                }
            }

            div { class: "quiz-2x2-grid",
                for i in 0..4_u8 {
                    {
                        let opt_ta = &c.game_options_ta[usize::from(i)];
                        let opt_en = &c.game_options_en[usize::from(i)];
                        let cls = match &eval {
                            Some(e) if i == e.correct_idx => "quiz-option-btn opt-correct",
                            Some(e) if chosen == Some(i) && !e.correct => "quiz-option-btn opt-wrong",
                            _ => "quiz-option-btn",
                        };
                        rsx! {
                            button {
                                key: "{i}",
                                r#type: "button",
                                class: "{cls}",
                                onclick: move |_| {
                                    ctx.last_kids_choice.set(Some(i));
                                    ctx.dispatch(BackendCommand::SubmitKidsGameAnswer {
                                        kural_number: kural_num,
                                        choice_idx: i,
                                    });
                                },
                                span { class: "font-kural", lang: "ta", "{opt_ta}" }
                                if lang == kural_core::Language::English {
                                    span { style: "font-size:11px; opacity:0.8;", "{opt_en}" }
                                }
                            }
                        }
                    }
                }
            }

            if let Some(e) = eval {
                p { style: "text-align:center; font-size:13px; font-weight:800;",
                    if e.correct {
                        "🌟 {tr(lang, \"சபாஷ்! +1 நட்சத்திரம் கிடைத்தது!\", \"Awesome! +1 Star earned!\")}"
                    } else {
                        "{tr(lang, \"மீண்டும் முயலவும்!\", \"Try once more!\")}"
                    }
                }
            }
        }
    }
}

// -----------------------------------------------------------------------------
// 5. Youth Quiz Modal (2x2 grid + XP & Streak feedback)
// -----------------------------------------------------------------------------

#[component]
fn YouthQuizModal() -> Element {
    let mut ctx = use_context::<AppContext>();
    let snap = ctx.snapshot.read().clone();
    let lang = snap.prefs.lang;
    let full = &snap.current_kural;
    let kural_num = full.kural.number;

    let eval = ctx
        .youth_quiz_eval
        .read()
        .clone()
        .filter(|e| e.kural_number == kural_num);
    let chosen = *ctx.last_youth_choice.read();

    let Some(y) = &full.youth else {
        return rsx! { ModalHeader { title: tr(lang, "வினாடி வினா", "Youth Quiz").to_string() } };
    };

    rsx! {
        ModalHeader { title: tr(lang, "🏆 இளைஞர் வினாடி வினா", "🏆 Daily Kural Quiz").to_string() }
        div { class: "modal-body", style: "justify-content:space-between;",
            p { style: "font-size:14px; font-weight:700; line-height:1.4;",
                "{tr(lang, &y.quiz_question_ta, &y.quiz_question_en)}"
            }

            div { class: "quiz-2x2-grid",
                for i in 0..4_u8 {
                    {
                        let label = tr(lang, &y.quiz_options_ta[usize::from(i)], &y.quiz_options_en[usize::from(i)]);
                        let cls = match &eval {
                            Some(e) if i == e.correct_idx => "quiz-option-btn opt-correct",
                            Some(e) if chosen == Some(i) && !e.correct => "quiz-option-btn opt-wrong",
                            _ => "quiz-option-btn",
                        };
                        rsx! {
                            button {
                                key: "{i}",
                                r#type: "button",
                                class: "{cls}",
                                onclick: move |_| {
                                    ctx.last_youth_choice.set(Some(i));
                                    ctx.dispatch(BackendCommand::SubmitYouthQuizAnswer {
                                        kural_number: kural_num,
                                        choice_idx: i,
                                    });
                                },
                                "{label}"
                            }
                        }
                    }
                }
            }

            if let Some(e) = eval {
                p { style: "text-align:center; font-size:12.5px; font-weight:800;",
                    if e.correct {
                        "🔥 {tr(lang, \"சரி!\", \"Correct!\")} +20 XP • {snap.youth_progress.quiz_xp} XP"
                    } else {
                        "{tr(lang, \"சரியான விடையைப் பாருங்கள்.\", \"Check the highlighted answer.\")}"
                    }
                }
            }
        }
    }
}

// -----------------------------------------------------------------------------
// 6. Youth Deep Dive Modal (Paginated Analysis + Reflection)
// -----------------------------------------------------------------------------

#[component]
fn YouthDeepDiveModal() -> Element {
    let ctx = use_context::<AppContext>();
    let snap = ctx.snapshot.read().clone();
    let lang = snap.prefs.lang;
    let full = &snap.current_kural;
    let kural_num = full.kural.number;

    let body = match (&full.youth, &full.adult) {
        (Some(y), Some(a)) => format!(
            "{} {} {}",
            tr(lang, &y.analysis_ta, &y.analysis_en),
            tr(lang, &y.action_tip_ta, &y.action_tip_en),
            tr(lang, &a.reflection_ta, &a.reflection_en),
        ),
        (Some(y), None) => format!(
            "{} {}",
            tr(lang, &y.analysis_ta, &y.analysis_en),
            tr(lang, &y.action_tip_ta, &y.action_tip_en),
        ),
        _ => tr(lang, &full.kural.meaning_mu_va, &full.kural.explanation_en).to_string(),
    };
    let pages = paginate_sentences(&body, 88);
    let mut page_idx = use_signal(|| 0_usize);
    let mut last_kural = use_signal(|| kural_num);
    use_effect(move || {
        let num = ctx.snapshot.read().current_kural.kural.number;
        if *last_kural.peek() != num {
            last_kural.set(num);
            page_idx.set(0);
        }
    });
    let safe = (*page_idx.read()).min(pages.len().saturating_sub(1));

    rsx! {
        ModalHeader { title: format!("📜 {} (#{kural_num})", tr(lang, "ஆழ்ந்து கற்றல்", "Deep Dive")) }
        div { class: "modal-body", style: "justify-content:space-between;",
            p { style: "font-size:14.5px; line-height:1.48;", "{pages[safe]}" }

            if pages.len() > 1 {
                div { class: "pager-row",
                    button {
                        r#type: "button",
                        class: "pager-btn",
                        disabled: safe == 0,
                        onclick: move |_| page_idx.set(safe.saturating_sub(1)),
                        "◀"
                    }
                    span { class: "tabular-nums", "{safe + 1} / {pages.len()}" }
                    button {
                        r#type: "button",
                        class: "pager-btn",
                        disabled: safe + 1 >= pages.len(),
                        onclick: move |_| page_idx.set(safe + 1),
                        "▶"
                    }
                }
            }

            div { class: "kural-actions-row",
                ContentPlaybackButton {
                    btn_base: "royal-btn",
                    button_id: None,
                    playback_status: snap.playback_status,
                    audio_state: snap.audio_state,
                    can_replay_tts: snap.can_replay_tts,
                    lang: lang,
                    fallback_target: SpeakTarget::YouthBriefing,
                }
                button {
                    r#type: "button",
                    class: "royal-btn action-pill-btn action-pill-chat",
                    onclick: move |_| ctx.dispatch(BackendCommand::OpenModal(ModalKind::AiChat {
                        seed_kural: Some(kural_num),
                    })),
                    "🤖 {tr(lang, \"AI கேள்\", \"Ask AI\")}"
                }
            }
        }
    }
}
