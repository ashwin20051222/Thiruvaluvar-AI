use crate::components::header::TopHeader;
use crate::components::icons::MicIcon;
use crate::copy::{age_label, tr};
use crate::state::{AppContext, is_voice_active};
use dioxus::prelude::*;
use kural_core::{AgeGroup, AgeOutcome, AppScreen, BackendCommand, retry_copy};
use std::time::Duration;

fn bump_session(mut sig: Signal<u64>) -> u64 {
    let next = sig.peek().wrapping_add(1);
    sig.set(next);
    next
}

#[component]
pub fn StartScreen() -> Element {
    let ctx = use_context::<AppContext>();
    let boot = use_context::<crate::BootContext>();
    let auto_armed_start_session = use_signal(|| 0_u64);

    use_hook(move || {
        if !boot.verify_layout && ctx.snapshot.peek().auto_age_route_on_start {
            let mut age_sig = ctx.age_result;
            age_sig.set(None);
            let token = bump_session(auto_armed_start_session);
            spawn(async move {
                tokio::time::sleep(Duration::from_millis(350)).await;
                let snap_now = ctx.snapshot.peek().clone();
                if *auto_armed_start_session.peek() == token
                    && snap_now.screen == AppScreen::Start
                    && !is_voice_active(snap_now.audio_state)
                    && snap_now.modal.is_none()
                {
                    ctx.dispatch(BackendCommand::EnterStartAutoListen);
                }
            });
        }
    });

    let snap = ctx.snapshot.read().clone();
    let lang = snap.prefs.lang;
    let listening = is_voice_active(snap.audio_state);
    let age_outcome = ctx.age_result.read().clone();

    let low_conf_highlight = matches!(
        age_outcome,
        Some(AgeOutcome::Detected(ref d)) if d.low_confidence
    ) || matches!(age_outcome, Some(AgeOutcome::Retry(_)));

    let mic_class = if listening {
        "pi-mic-btn is-listening"
    } else {
        "pi-mic-btn"
    };

    let hint_full = if listening {
        tr(
            lang,
            "தானாகக் கேட்கிறது — பேசினால் உங்கள் வயதுப் பிரிவு திறக்கும்",
            "Listening automatically — speak to open Child, Teen, or Adult mode",
        )
    } else {
        tr(
            lang,
            "மைக்கை அழுத்தி பேசுங்கள் (எ.கா. \"வணக்கம், திருக்குறள் கற்க விரும்புகிறேன்\")",
            "Tap the microphone and speak (e.g. \"Hello, I want to learn Thirukkural\")",
        )
    };

    let hint_short = if listening {
        tr(lang, "தானாகக் கேட்கிறது...", "Auto listening...")
    } else {
        tr(lang, "தட்டிப் பேசுங்கள்", "Tap & speak")
    };

    rsx! {
        div { class: "scene-backdrop", aria_hidden: "true",
            svg { class: "pi-corner-art pi-corner-tl desktop-only", view_box: "0 0 240 240", fill: "none", stroke: "currentColor", stroke_width: "1.2",
                path { d: "M-10 30 C 50 30, 100 10, 160 -10" }
                path { d: "M-10 55 C 60 55, 120 30, 195 -10" }
                path { d: "M-10 80 C 70 80, 140 50, 225 -10" }
                path { d: "M20 0 Q 45 65 15 130" }
                path { d: "M55 0 Q 75 50 50 105" }
            }
            svg { class: "pi-corner-art pi-corner-br desktop-only", view_box: "0 0 260 260", fill: "none", stroke: "currentColor", stroke_width: "1.2",
                path { d: "M120 260 L120 220 L140 220 L140 190 L160 190 L160 160 L180 160 L180 130 L195 110 L210 130 L210 160 L230 160 L230 190 L250 190 L250 260" }
                line { x1: "145", y1: "205", x2: "245", y2: "205" }
                line { x1: "165", y1: "175", x2: "225", y2: "175" }
                circle { cx: "195", cy: "96", r: "4" }
            }
        }

        div { class: "pi-root",
            TopHeader {}

            main { class: "pi-main-start",
                div { class: "pi-zone-2",
                    // Left 5 cols: Portrait & Heritage Identity
                    section { class: "pi-hero-col",
                        div { class: "pi-portrait-wrap hero-medallion-frame",
                            img {
                                id: "valluvar-portrait",
                                class: "hero-portrait-img",
                                src: "/kural/app-logo.png",
                                alt: "Thiruvalluvar AI dark-emerald and gold medallion emblem",
                            }
                        }
                        span { class: "pi-badge-full",
                            "{tr(lang, \"ஓலைச்சுவடி ஏந்திய பெருந்தகை • 1,330 குறள்\", \"Sage of the Palm-Leaf • 1,330 Kurals\")}"
                        }
                        span { class: "pi-badge-short",
                            "{tr(lang, \"1,330 குறள்\", \"1,330 Kurals\")}"
                        }
                        h1 { class: "pi-hero-title",
                            "{tr(lang, \"திருவள்ளுவர் AI\", \"THIRUVALLUVAR AI\")}"
                        }
                        p { class: "pi-tagline",
                            "{tr(lang, \"பழமையான ஞானம், புத்திசாலி எதிர்காலம்\", \"Ancient wisdom, intelligent future\")}"
                        }
                        p { class: "pi-hero-intro",
                            "{tr(lang, \"உங்கள் வயது பிரிவுக்கு ஏற்ப 1,330 குறள்களை அறிய தமிழ் அல்லது ஆங்கிலத்தில் பேசுங்கள்.\", \"Speak naturally in Tamil or English to explore the 1,330 Kurals tailored to your age group.\")}"
                        }
                    }

                    // Right 7 cols: Voice Age Gate + 3 Manual Age Cards
                    section { class: "pi-right-col",
                        div { class: "pi-voice-card", aria_label: "Voice age gate",
                            div { class: "pi-mic-wrap",
                                span {
                                    class: "mic-pulse-ring",
                                    style: "position:absolute; inset:-8px; border-radius:9999px; background:var(--gold-frame); pointer-events:none;",
                                    aria_hidden: "true",
                                }
                                button {
                                    id: "mic-btn",
                                    r#type: "button",
                                    class: "{mic_class}",
                                    aria_label: tr(lang, "பேசுக", "Speak"),
                                    onclick: move |_| {
                                        bump_session(auto_armed_start_session);
                                        if listening {
                                            ctx.dispatch(BackendCommand::CancelVoice);
                                        } else {
                                            ctx.dispatch(BackendCommand::StartVoiceAgeDetect);
                                        }
                                    },
                                    MicIcon { size: 28 }
                                    span { class: "pi-mic-label",
                                        if listening {
                                            "{tr(lang, \"கேட்கிறேன்...\", \"Listening...\")}"
                                        } else {
                                            "{tr(lang, \"பேசுக\", \"Speak\")}"
                                        }
                                    }
                                }
                            }

                            p { class: "pi-hint-full", "{hint_full}" }
                            p { class: "pi-hint-short", "{hint_short}" }

                            // Age AI result / Retry readout
                            if let Some(outcome) = age_outcome.clone() {
                                match outcome {
                                    AgeOutcome::Detected(det) => {
                                        let pct = (det.confidence * 100.0).round() as u32;
                                        let lbl = age_label(det.age, lang);
                                        let detected_prefix = tr(lang, "கண்டறியப்பட்டது:", "Detected:");
                                        rsx! {
                                            div { id: "detect-result",
                                                div { style: "flex:1; min-width:0;",
                                                    div { style: "display:flex; justify-content:space-between; font-size:12px; font-weight:700;",
                                                        span { "{detected_prefix} {lbl}" }
                                                        span { class: "tabular-nums", style: "color:var(--success);", "{pct}% • VTL {det.vtl_cm:.1}cm" }
                                                    }
                                                    div { class: "detect-bar-track",
                                                        div { class: "detect-bar-fill", style: "width:{pct.min(100)}%;" }
                                                    }
                                                }
                                                button {
                                                    r#type: "button",
                                                    class: "detect-continue-btn",
                                                    onclick: move |_| {
                                                        bump_session(auto_armed_start_session);
                                                        ctx.dispatch(BackendCommand::SelectAge(det.age));
                                                    },
                                                    "{tr(lang, \"உள்ளே செல் →\", \"Continue →\")}"
                                                }
                                            }
                                        }
                                    }
                                    AgeOutcome::Retry(reason) => {
                                        let msg = retry_copy(reason);
                                        let retry_txt = tr(lang, msg.ta, msg.en);
                                        rsx! {
                                            div { id: "detect-result",
                                                p { style: "font-size:12px; font-weight:700; color:var(--warning);",
                                                    "{retry_txt}"
                                                }
                                            }
                                        }
                                    }
                                }
                            }
                        }

                        div { class: "pi-manual-wrap",
                            div { class: "pi-divider",
                                div { class: "pi-divider-line" }
                                span { "{tr(lang, \"அல்லது நேரடியாகத் தேர்ந்தெடுக்கவும்\", \"Or choose manually\")}" }
                                div { class: "pi-divider-line" }
                            }

                            div { class: "pi-age-grid",
                                {
                                    let card_cls = if low_conf_highlight {
                                        "pi-age-card highlight-low-conf"
                                    } else {
                                        "pi-age-card"
                                    };
                                    rsx! {
                                        button {
                                            r#type: "button",
                                            class: "{card_cls}",
                                            onclick: move |_| {
                                                bump_session(auto_armed_start_session);
                                                ctx.dispatch(BackendCommand::SelectAge(AgeGroup::Kids));
                                            },
                                            span { class: "pi-age-icon age-icon-kids", aria_hidden: "true",
                                                svg { view_box: "0 0 24 24", width: "18", height: "18", fill: "none", stroke: "currentColor", stroke_width: "2",
                                                    circle { cx: "12", cy: "12", r: "9" }
                                                    path { d: "M9 10h.01M15 10h.01" }
                                                    path { d: "M8.5 14.5a4 4 0 0 0 7 0" }
                                                }
                                            }
                                            p { class: "pi-age-title", "{tr(lang, \"சிறுவர்\", \"Child\")}" }
                                            p { class: "pi-age-range", "6–12" }
                                            p { class: "pi-card-sub", "{tr(lang, \"கதைகள் & விளையாட்டுகள்\", \"Stories & games\")}" }
                                        }
                                        button {
                                            r#type: "button",
                                            class: "{card_cls}",
                                            onclick: move |_| {
                                                bump_session(auto_armed_start_session);
                                                ctx.dispatch(BackendCommand::SelectAge(AgeGroup::Youth));
                                            },
                                            span { class: "pi-age-icon age-icon-youth", aria_hidden: "true",
                                                svg { view_box: "0 0 24 24", width: "18", height: "18", fill: "none", stroke: "currentColor", stroke_width: "2",
                                                    circle { cx: "12", cy: "12", r: "9" }
                                                    polygon { points: "15.5 8.5 13.5 13.5 8.5 15.5 10.5 10.5 15.5 8.5" }
                                                }
                                            }
                                            p { class: "pi-age-title", "{tr(lang, \"இளைஞர்\", \"Teen\")}" }
                                            p { class: "pi-age-range", "13–17" }
                                            p { class: "pi-card-sub", "{tr(lang, \"வழிகாட்டுதல் & சவால்கள்\", \"Guidance & challenges\")}" }
                                        }
                                        button {
                                            r#type: "button",
                                            class: "{card_cls}",
                                            onclick: move |_| {
                                                bump_session(auto_armed_start_session);
                                                ctx.dispatch(BackendCommand::SelectAge(AgeGroup::Adult));
                                            },
                                            span { class: "pi-age-icon age-icon-adult", aria_hidden: "true",
                                                svg { view_box: "0 0 24 24", width: "18", height: "18", fill: "none", stroke: "currentColor", stroke_width: "2",
                                                    path { d: "M4 19.5v-15A2.5 2.5 0 0 1 6.5 2H20v20H6.5a2.5 2.5 0 0 1 0-5H20" }
                                                    line { x1: "8", y1: "7", x2: "16", y2: "7" }
                                                    line { x1: "8", y1: "11", x2: "14", y2: "11" }
                                                }
                                            }
                                            p { class: "pi-age-title", "{tr(lang, \"பெரியோர்\", \"Adult\")}" }
                                            p { class: "pi-age-range", "18+" }
                                            p { class: "pi-card-sub", "{tr(lang, \"தத்துவம் & உரைகள்\", \"Philosophy & commentary\")}" }
                                        }
                                    }
                                }
                            }
                        }
                    }
                }
            }

            // Unified Offline Status Footer across all 4 Start variants (Audit Defect #2 fix)
            footer { class: "pi-footer",
                span { class: "status-footer-pill", role: "status",
                    span {
                        class: if snap.system_health.db_ok { "offline-dot" } else { "offline-dot offline-warn" },
                        aria_hidden: "true",
                    }
                    span { "{tr(lang, \"ஆஃப்லைன் • 1,330 குறள்கள் தயார்\", \"Working offline • 1,330 Kurals ready\")}" }
                }
            }
        }
    }
}
