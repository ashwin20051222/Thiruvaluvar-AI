//! Reactive UI state bridge synchronizing `BackendHandle` channels into Dioxus signals.

use dioxus::prelude::*;
use kural_backend::BackendHandle;
use kural_core::{
    AgeOutcome, AudioState, BackendCommand, CompactTabHint, GroundedAnswer, Language,
    SearchOutcome, UiSnapshot, UiTransientEvent,
};
use std::sync::Arc;
use std::time::Duration;

#[derive(Clone, Debug, PartialEq)]
pub struct ToastBanner {
    pub text_ta: String,
    pub text_en: String,
    pub is_error: bool,
}

#[derive(Clone, Debug, PartialEq)]
pub struct KidsGameEval {
    pub kural_number: u16,
    pub choice_idx: u8,
    pub correct: bool,
    pub correct_idx: u8,
}

#[derive(Clone, Debug, PartialEq)]
pub struct YouthQuizEval {
    pub kural_number: u16,
    pub choice_idx: u8,
    pub correct: bool,
    pub correct_idx: u8,
}

#[derive(Clone, Copy)]
pub struct AppContext {
    pub backend: Signal<BackendHandle>,
    pub snapshot: Signal<Arc<UiSnapshot>>,
    pub rms: Signal<f32>,
    pub toast: Signal<Option<ToastBanner>>,
    pub age_result: Signal<Option<AgeOutcome>>,
    pub search_outcome: Signal<Option<SearchOutcome>>,
    pub ai_answer: Signal<Option<GroundedAnswer>>,
    pub kids_game_eval: Signal<Option<KidsGameEval>>,
    pub youth_quiz_eval: Signal<Option<YouthQuizEval>>,
    pub last_kids_choice: Signal<Option<u8>>,
    pub last_youth_choice: Signal<Option<u8>>,
    pub pi_compact_tab: Signal<CompactTabHint>,
}

impl AppContext {
    pub fn dispatch(&self, cmd: BackendCommand) {
        self.backend.read().dispatch(cmd);
    }

    pub fn lang(&self) -> Language {
        self.snapshot.read().prefs.lang
    }
}

pub fn provide_app_context(backend: BackendHandle, toast_duration_ms: u64) -> AppContext {
    let initial = backend.current_snapshot();
    let mut snap_sig = use_signal(move || initial);
    let mut rms_sig = use_signal(|| 0.0_f32);
    let mut toast_sig = use_signal(|| None::<ToastBanner>);
    let mut toast_seq = use_signal(|| 0_u64);
    let mut age_result_sig = use_signal(|| None::<AgeOutcome>);
    let mut search_outcome_sig = use_signal(|| None::<SearchOutcome>);
    let mut ai_answer_sig = use_signal(|| None::<GroundedAnswer>);
    let mut kids_game_eval_sig = use_signal(|| None::<KidsGameEval>);
    let mut youth_quiz_eval_sig = use_signal(|| None::<YouthQuizEval>);
    let last_kids_choice_sig = use_signal(|| None::<u8>);
    let last_youth_choice_sig = use_signal(|| None::<u8>);
    let mut pi_compact_tab_sig = use_signal(|| CompactTabHint::Verse);
    let backend_sig = use_signal(|| backend.clone());

    // Subscribe to watch<Arc<UiSnapshot>>
    let snap_rx = backend.subscribe_snapshot();
    use_hook(move || {
        spawn(async move {
            let mut rx = snap_rx;
            while rx.changed().await.is_ok() {
                let latest = rx.borrow_and_update().clone();
                snap_sig.set(latest);
            }
        });
    });

    // Subscribe to 30 Hz mic RMS stream (throttled updates to avoid idle repaints)
    let rms_rx = backend.subscribe_rms();
    use_hook(move || {
        spawn(async move {
            let mut rx = rms_rx;
            let mut prev = 0.0_f32;
            while rx.changed().await.is_ok() {
                let val = *rx.borrow_and_update();
                let should_update = (val - prev).abs() > 0.02 || (val == 0.0 && prev != 0.0);
                if should_update {
                    prev = val;
                    rms_sig.set(val);
                }
            }
        });
    });

    // Subscribe to transient UI events
    let events_rx = backend.subscribe_events();
    let toast_ms = toast_duration_ms.max(800);
    use_hook(move || {
        spawn(async move {
            let mut rx = events_rx;
            while let Some(evt) = rx.recv().await {
                match evt {
                    UiTransientEvent::SearchResultsReady(outcome) => {
                        search_outcome_sig.set(Some(outcome));
                    }
                    UiTransientEvent::AdhikaramPreviewReady { .. } => {}
                    UiTransientEvent::AgeResult(outcome) => {
                        age_result_sig.set(Some(outcome));
                    }
                    UiTransientEvent::VoiceCommandRecognized {
                        toast_ta,
                        toast_en,
                        compact_tab,
                        ..
                    } => {
                        if let Some(tab) = compact_tab {
                            pi_compact_tab_sig.set(tab);
                        }
                        show_auto_toast(
                            &mut toast_sig,
                            &mut toast_seq,
                            ToastBanner {
                                text_ta: toast_ta,
                                text_en: toast_en,
                                is_error: false,
                            },
                            toast_ms.max(2600),
                        );
                    }
                    UiTransientEvent::AiAnswerReady(ans) => {
                        ai_answer_sig.set(Some(ans));
                    }
                    UiTransientEvent::KidsGameEvaluated {
                        kural_number,
                        correct,
                        correct_idx,
                        ..
                    } => {
                        let chosen = last_kids_choice_sig.read().unwrap_or(correct_idx);
                        kids_game_eval_sig.set(Some(KidsGameEval {
                            kural_number,
                            choice_idx: chosen,
                            correct,
                            correct_idx,
                        }));
                    }
                    UiTransientEvent::YouthQuizEvaluated {
                        kural_number,
                        correct,
                        correct_idx,
                        ..
                    } => {
                        let chosen = last_youth_choice_sig.read().unwrap_or(correct_idx);
                        youth_quiz_eval_sig.set(Some(YouthQuizEval {
                            kural_number,
                            choice_idx: chosen,
                            correct,
                            correct_idx,
                        }));
                    }
                    UiTransientEvent::CommentaryFallbackNotice {
                        notice_ta,
                        notice_en,
                        ..
                    }
                    | UiTransientEvent::InfoToast {
                        message_ta: notice_ta,
                        message_en: notice_en,
                    } => {
                        show_auto_toast(
                            &mut toast_sig,
                            &mut toast_seq,
                            ToastBanner {
                                text_ta: notice_ta,
                                text_en: notice_en,
                                is_error: false,
                            },
                            toast_ms,
                        );
                    }
                    UiTransientEvent::ErrorToast {
                        message_ta,
                        message_en,
                    } => {
                        show_auto_toast(
                            &mut toast_sig,
                            &mut toast_seq,
                            ToastBanner {
                                text_ta: message_ta,
                                text_en: message_en,
                                is_error: true,
                            },
                            toast_ms,
                        );
                    }
                    _ => {}
                }
            }
        });
    });

    let ctx = AppContext {
        backend: backend_sig,
        snapshot: snap_sig,
        rms: rms_sig,
        toast: toast_sig,
        age_result: age_result_sig,
        search_outcome: search_outcome_sig,
        ai_answer: ai_answer_sig,
        kids_game_eval: kids_game_eval_sig,
        youth_quiz_eval: youth_quiz_eval_sig,
        last_kids_choice: last_kids_choice_sig,
        last_youth_choice: last_youth_choice_sig,
        pi_compact_tab: pi_compact_tab_sig,
    };
    use_context_provider(|| ctx);
    ctx
}

fn show_auto_toast(
    toast_sig: &mut Signal<Option<ToastBanner>>,
    seq_sig: &mut Signal<u64>,
    banner: ToastBanner,
    duration_ms: u64,
) {
    let next_seq = (*seq_sig.read()).wrapping_add(1);
    seq_sig.set(next_seq);
    toast_sig.set(Some(banner));

    let mut toast_clone = *toast_sig;
    let seq_clone = *seq_sig;
    spawn(async move {
        tokio::time::sleep(Duration::from_millis(duration_ms)).await;
        if *seq_clone.read() == next_seq {
            toast_clone.set(None);
        }
    });
}

#[must_use]
pub fn is_voice_active(audio_state: AudioState) -> bool {
    matches!(
        audio_state,
        AudioState::Listening | AudioState::CapturingSpeech | AudioState::Processing
    )
}

#[must_use]
pub fn is_voice_cancellable(audio_state: AudioState) -> bool {
    matches!(
        audio_state,
        AudioState::Listening | AudioState::CapturingSpeech
    )
}
