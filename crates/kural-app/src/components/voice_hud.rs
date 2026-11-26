//! Voice HUD overlay (30 Hz RMS 5-bar equalizer + status + cancel) and transient toast banner.

use crate::copy::tr;
use crate::state::{AppContext, is_voice_active};
use dioxus::prelude::*;
use kural_core::{AudioState, BackendCommand};

#[component]
pub fn VoiceHud() -> Element {
    let ctx = use_context::<AppContext>();
    let snap = ctx.snapshot.read().clone();
    let lang = snap.prefs.lang;
    let voice_active = is_voice_active(snap.audio_state);
    let rms = (*ctx.rms.read()).clamp(0.0, 1.0);
    let toast = ctx.toast.read().clone();

    rsx! {
        if let Some(banner) = toast {
            {
                let localized = tr(lang, &banner.text_ta, &banner.text_en);
                let display_text = if localized.trim().is_empty() {
                    if banner.text_ta.trim().is_empty() {
                        banner.text_en.as_str()
                    } else {
                        banner.text_ta.as_str()
                    }
                } else {
                    localized
                };
                rsx! {
                    if !display_text.trim().is_empty() {
                        div {
                            id: "toast-banner",
                            class: if banner.is_error { "toast-error" } else { "" },
                            style: if voice_active { "transform: translate(-50%, -44px);" } else { "" },
                            role: "status",
                            aria_live: "polite",
                            "{display_text}"
                        }
                    }
                }
            }
        }

        if voice_active {
            div {
                id: "voice-hud",
                role: "status",
                aria_live: "polite",
                div { class: "eq-bars", aria_hidden: "true",
                    for (i, mult) in [0.55_f32, 0.85, 1.0, 0.78, 0.6].iter().enumerate() {
                        {
                            let px = (4.0 + rms * 28.0 * *mult).clamp(4.0, 20.0).round() as u32;
                            rsx! {
                                div {
                                    key: "{i}",
                                    class: "eq-bar",
                                    style: "height: {px}px;",
                                }
                            }
                        }
                    }
                }
                span { style: "font-size:13px; font-weight:800;",
                    match snap.audio_state {
                        AudioState::Listening => tr(lang, "கேட்கிறேன்… பேசுங்கள்", "Listening… Speak now"),
                        AudioState::CapturingSpeech => tr(lang, "உங்கள் குரல் பதிவாகிறது…", "Capturing speech…"),
                        AudioState::Processing => tr(lang, "செயலாக்குகிறேன்…", "Thinking…"),
                        _ => "",
                    }
                }
                button {
                    r#type: "button",
                    class: "modal-close-btn",
                    style: "width:28px; height:28px; font-size:12px;",
                    aria_label: tr(lang, "குரல் ரத்து", "Cancel voice"),
                    onclick: move |_| ctx.dispatch(BackendCommand::CancelVoice),
                    "✕"
                }
            }
        }
    }
}
