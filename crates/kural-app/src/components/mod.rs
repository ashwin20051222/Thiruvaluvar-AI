use crate::components::icons::{ReplayIcon, SpeakerIcon, StopSquareIcon};
use crate::copy::tr;
use crate::state::AppContext;
use dioxus::prelude::*;
use kural_core::{AudioPlaybackStatus, AudioState, BackendCommand, Language, SpeakTarget};

pub mod adult_screen;
pub mod header;
pub mod icons;
pub mod kids_screen;
pub mod modals;
pub mod start_screen;
pub mod voice_hud;
pub mod youth_screen;

/// State-driven audio lifecycle button: hides unnecessary pre-playback Play controls, shows
/// `Loading…` / `Stop` during active synthesis/playback, reveals persistent `Replay Audio` after
/// initial completion, and exposes one-click `Play Audio` when auto-playback is blocked or stopped.
#[component]
pub fn ContentPlaybackButton(
    btn_base: &'static str,
    button_id: Option<&'static str>,
    playback_status: AudioPlaybackStatus,
    audio_state: AudioState,
    can_replay_tts: bool,
    lang: Language,
    fallback_target: SpeakTarget,
) -> Element {
    let ctx = use_context::<AppContext>();
    let is_speaking = audio_state == AudioState::PlayingTts
        || matches!(
            playback_status,
            AudioPlaybackStatus::Playing | AudioPlaybackStatus::Replaying
        );

    if is_speaking {
        rsx! {
            button {
                id: button_id,
                r#type: "button",
                class: "{btn_base} action-pill-btn action-pill-stop",
                onclick: move |_| ctx.dispatch(BackendCommand::StopAudio),
                StopSquareIcon {}
                span { "{tr(lang, \"நிறுத்து\", \"Stop\")}" }
            }
        }
    } else if playback_status == AudioPlaybackStatus::Loading {
        rsx! {
            button {
                id: button_id,
                r#type: "button",
                class: "{btn_base} action-pill-btn action-pill-stop",
                aria_label: tr(lang, "தயாராகிறது — நிறுத்து", "Loading Audio — Stop"),
                onclick: move |_| ctx.dispatch(BackendCommand::StopAudio),
                StopSquareIcon {}
                span { class: "desktop-only", "{tr(lang, \"தயாராகிறது…\", \"Loading…\")}" }
                span { class: "pi-only", "{tr(lang, \"நிறுத்து\", \"Stop\")}" }
            }
        }
    } else if playback_status == AudioPlaybackStatus::Completed && can_replay_tts {
        rsx! {
            button {
                id: button_id,
                r#type: "button",
                class: "{btn_base} action-pill-btn action-pill-primary",
                aria_label: tr(lang, "மீண்டும் கேள்", "Replay Audio"),
                onclick: move |_| ctx.dispatch(BackendCommand::ReplayLastTts),
                ReplayIcon {}
                span { class: "desktop-only", "{tr(lang, \"மீண்டும் கேள்\", \"Replay Audio\")}" }
                span { class: "pi-only", "{tr(lang, \"மீண்டும்\", \"Replay\")}" }
            }
        }
    } else if matches!(
        playback_status,
        AudioPlaybackStatus::FallbackReady | AudioPlaybackStatus::Completed
    ) {
        rsx! {
            button {
                id: button_id,
                r#type: "button",
                class: "{btn_base} action-pill-btn action-pill-primary",
                aria_label: tr(lang, "ஒலியை இயக்கு", "Play Audio"),
                onclick: move |_| ctx.dispatch(BackendCommand::SpeakCurrent(fallback_target)),
                SpeakerIcon {}
                span { class: "desktop-only", "{tr(lang, \"ஒலியை இயக்கு\", \"Play Audio\")}" }
                span { class: "pi-only", "{tr(lang, \"கேள்\", \"Play\")}" }
            }
        }
    } else {
        rsx! {}
    }
}
