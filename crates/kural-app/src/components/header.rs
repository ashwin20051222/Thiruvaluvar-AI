use crate::components::icons::{MoonIcon, SearchIcon, SunIcon};
use crate::copy::tr;
use crate::state::AppContext;
use dioxus::prelude::*;
use kural_core::{AgeGroup, AppScreen, BackendCommand, Language, ModalKind, SearchTab, Theme};

#[component]
pub fn TopHeader() -> Element {
    let ctx = use_context::<AppContext>();
    let snap = ctx.snapshot.read().clone();
    let lang = snap.prefs.lang;
    let theme = snap.prefs.theme;
    let screen = snap.screen;
    let health = snap.system_health;
    let healthy = health.db_ok && health.user_state_ok;

    let toggle_theme = move |_| {
        let next = match theme {
            Theme::Dark => Theme::Light,
            Theme::Light => Theme::Dark,
        };
        ctx.dispatch(BackendCommand::SetTheme(next));
    };

    let toggle_lang = move |_| {
        let next = match lang {
            Language::Tamil => Language::English,
            Language::English => Language::Tamil,
        };
        ctx.dispatch(BackendCommand::SetLanguage(next));
    };

    let header_class = if screen == AppScreen::Start {
        "app-header pi-header-start"
    } else {
        "app-header"
    };

    let brand_pill_cls = match screen {
        AppScreen::Kids => "brand-pill brand-pill-kids",
        AppScreen::Youth | AppScreen::Adult => "brand-pill brand-pill-royal",
        AppScreen::Start => "brand-pill",
    };

    let offline_cls = if healthy {
        "offline-dot"
    } else {
        "offline-dot offline-warn"
    };

    rsx! {
        header { id: "main-header", class: "{header_class}",
            // Left Cluster: Back arrow (S2-S4), Emblem, Title, Green Offline Dot, Gamification Pill
            div { class: "header-cluster",
                if screen != AppScreen::Start {
                    button {
                        id: "nav-home",
                        r#type: "button",
                        aria_label: tr(lang, "முகப்பு", "Home"),
                        class: "icon-circle-btn toy-btn",
                        onclick: move |_| ctx.dispatch(BackendCommand::Navigate(AppScreen::Start)),
                        "←"
                    }
                } else {
                    div { class: "emblem-badge", aria_hidden: "true", "வ" }
                }

                if screen == AppScreen::Start {
                    span { class: "pi-app-name font-bold", "{tr(lang, \"திருவள்ளுவர் AI\", \"Thiruvalluvar AI\")}" }
                    span {
                        class: "pi-only {offline_cls}",
                        role: "status",
                        title: tr(lang, "இணையம் இன்றி இயங்குகிறது", "Working offline"),
                    }
                } else {
                    div { class: "{brand_pill_cls}",
                        span {
                            class: "{offline_cls}",
                            role: "status",
                            title: tr(lang, "இணையம் இன்றி இயங்குகிறது", "Offline Ready"),
                        }
                        span { class: "truncate font-bold",
                            match (screen, lang) {
                                (AppScreen::Kids, Language::Tamil) => "சிறுவர் திருக்குறள்",
                                (AppScreen::Kids, Language::English) => "Kids Thirukkural",
                                (AppScreen::Youth, Language::Tamil) => "இளைஞர் மையம்",
                                (AppScreen::Youth, Language::English) => "Youth Hub",
                                (AppScreen::Adult, Language::Tamil) => "திருக்குறள் • பெரியோர்",
                                (AppScreen::Adult, Language::English) => "Scholarly Mode",
                                _ => "திருவள்ளுவர் AI",
                            }
                        }
                    }
                }

                if screen == AppScreen::Kids {
                    div { class: "star-pill", title: tr(lang, "நட்சத்திரங்கள்", "Stars earned"),
                        span { "⭐ {snap.kids_progress.stars_earned}" }
                    }
                }
                if screen == AppScreen::Youth {
                    div { class: "streak-pill", title: tr(lang, "தொடர் சாதனை & XP", "Daily Streak & XP"),
                        span { "🔥 {snap.youth_progress.daily_streak} • {snap.youth_progress.quiz_xp} XP" }
                    }
                }
            }

            // Right Cluster: Age Switcher (S2-S4 Desktop), Theme Toggle, Language Toggle
            div { class: "header-cluster",
                if screen != AppScreen::Start {
                    nav { aria_label: "Age Group", class: "age-nav desktop-only",
                        for (age, label_ta, label_en) in [
                            (AgeGroup::Kids, "சிறுவர்", "Child"),
                            (AgeGroup::Youth, "இளைஞர்", "Teen"),
                            (AgeGroup::Adult, "பெரியோர்", "Adult"),
                        ] {
                            {
                                let is_cur = matches!(
                                    (screen, age),
                                    (AppScreen::Kids, AgeGroup::Kids)
                                        | (AppScreen::Youth, AgeGroup::Youth)
                                        | (AppScreen::Adult, AgeGroup::Adult)
                                );
                                let cls = if !is_cur {
                                    "age-nav-btn"
                                } else if screen == AppScreen::Kids {
                                    "age-nav-btn active-kids"
                                } else if theme == Theme::Dark {
                                    "age-nav-btn active-royal-dark"
                                } else {
                                    "age-nav-btn active-royal-light"
                                };
                                rsx! {
                                    button {
                                        key: "{age}",
                                        r#type: "button",
                                        class: "{cls}",
                                        onclick: move |_| ctx.dispatch(BackendCommand::SelectAge(age)),
                                        "{tr(lang, label_ta, label_en)}"
                                    }
                                }
                            }
                        }
                    }

                    button {
                        r#type: "button",
                        class: "icon-circle-btn desktop-only",
                        aria_label: tr(lang, "தேடு", "Search"),
                        title: tr(lang, "தேடு", "Search"),
                        onclick: move |_| ctx.dispatch(BackendCommand::OpenModal(ModalKind::Search {
                            initial_tab: SearchTab::Query,
                        })),
                        SearchIcon {}
                    }
                }

                button {
                    id: "theme-toggle",
                    r#type: "button",
                    class: "icon-circle-btn",
                    aria_label: tr(lang, "தோற்றத்தை மாற்று", "Toggle theme"),
                    onclick: toggle_theme,
                    if theme == Theme::Dark { SunIcon {} } else { MoonIcon {} }
                }

                button {
                    id: "lang-toggle",
                    r#type: "button",
                    class: "lang-pill-btn",
                    aria_label: tr(lang, "மொழியை மாற்று", "Switch language"),
                    onclick: toggle_lang,
                    span {
                        id: "lang-ta",
                        class: if lang == Language::Tamil { "lang-chip active" } else { "lang-chip" },
                        "தமிழ்"
                    }
                    span { style: "opacity: 0.45", "|" }
                    span {
                        id: "lang-en",
                        class: if lang == Language::English { "lang-chip active" } else { "lang-chip" },
                        "EN"
                    }
                }
            }
        }
    }
}
