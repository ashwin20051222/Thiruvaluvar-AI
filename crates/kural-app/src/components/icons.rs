use dioxus::prelude::*;

#[component]
pub fn SunIcon() -> Element {
    rsx! {
        svg { view_box: "0 0 24 24", width: "18", height: "18", fill: "none", stroke: "currentColor", stroke_width: "2", stroke_linecap: "round", stroke_linejoin: "round", "aria-hidden": "true",
            circle { cx: "12", cy: "12", r: "4" }
            path { d: "M12 2v2M12 20v2M4.93 4.93l1.41 1.41M17.66 17.66l1.41 1.41M2 12h2M20 12h2M6.34 17.66l-1.41 1.41M19.07 4.93l-1.41 1.41" }
        }
    }
}

#[component]
pub fn MoonIcon() -> Element {
    rsx! {
        svg { view_box: "0 0 24 24", width: "18", height: "18", fill: "none", stroke: "currentColor", stroke_width: "2", stroke_linecap: "round", stroke_linejoin: "round", "aria-hidden": "true",
            path { d: "M21 12.79A9 9 0 1 1 11.21 3 7 7 0 0 0 21 12.79z" }
        }
    }
}

#[component]
pub fn MicIcon(#[props(default = 20)] size: u32) -> Element {
    rsx! {
        svg { view_box: "0 0 24 24", width: "{size}", height: "{size}", fill: "none", stroke: "currentColor", stroke_width: "2.3", stroke_linecap: "round", stroke_linejoin: "round", "aria-hidden": "true",
            rect { x: "9", y: "2", width: "6", height: "11", rx: "3" }
            path { d: "M5 10v2a7 7 0 0 0 14 0v-2" }
            line { x1: "12", y1: "19", x2: "12", y2: "22" }
        }
    }
}

#[component]
pub fn SearchIcon() -> Element {
    rsx! {
        svg { view_box: "0 0 24 24", width: "15", height: "15", fill: "none", stroke: "currentColor", stroke_width: "2.4", stroke_linecap: "round", stroke_linejoin: "round", "aria-hidden": "true",
            circle { cx: "11", cy: "11", r: "8" }
            line { x1: "21", y1: "21", x2: "16.65", y2: "16.65" }
        }
    }
}

#[component]
pub fn SpeakerIcon() -> Element {
    rsx! {
        svg { view_box: "0 0 24 24", width: "16", height: "16", fill: "none", stroke: "currentColor", stroke_width: "2.3", stroke_linecap: "round", stroke_linejoin: "round", "aria-hidden": "true",
            polygon { points: "11 5 6 9 2 9 2 15 6 15 11 19 11 5" }
            path { d: "M15.54 8.46a5 5 0 0 1 0 7.07" }
            path { d: "M19.07 4.93a10 10 0 0 1 0 14.14" }
        }
    }
}

#[component]
pub fn ReplayIcon() -> Element {
    rsx! {
        svg { view_box: "0 0 24 24", width: "15", height: "15", fill: "none", stroke: "currentColor", stroke_width: "2.4", stroke_linecap: "round", stroke_linejoin: "round", "aria-hidden": "true",
            path { d: "M3 12a9 9 0 1 0 3-6.7L3 8" }
            path { d: "M3 3v5h5" }
        }
    }
}

#[component]
pub fn StopSquareIcon() -> Element {
    rsx! {
        svg { view_box: "0 0 24 24", width: "14", height: "14", fill: "currentColor", "aria-hidden": "true",
            rect { x: "5", y: "5", width: "14", height: "14", rx: "2.5" }
        }
    }
}

#[component]
pub fn ChatIcon() -> Element {
    rsx! {
        svg { view_box: "0 0 24 24", width: "15", height: "15", fill: "none", stroke: "currentColor", stroke_width: "2.2", stroke_linecap: "round", stroke_linejoin: "round", "aria-hidden": "true",
            path { d: "M21 15a2 2 0 0 1-2 2H7l-4 4V5a2 2 0 0 1 2-2h14a2 2 0 0 1 2 2z" }
        }
    }
}

#[component]
pub fn BookIcon() -> Element {
    rsx! {
        svg { view_box: "0 0 24 24", width: "16", height: "16", fill: "none", stroke: "currentColor", stroke_width: "2.2", stroke_linecap: "round", stroke_linejoin: "round", "aria-hidden": "true",
            path { d: "M2 3h6a4 4 0 0 1 4 4v14a3 3 0 0 0-3-3H2z" }
            path { d: "M22 3h-6a4 4 0 0 0-4 4v14a3 3 0 0 1 3-3h7z" }
        }
    }
}
