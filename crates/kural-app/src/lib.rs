//! Pure-Rust Dioxus Desktop library & launcher for Thiruvalluvar AI.
//! Zero cloud URLs, zero Python, strict `unsafe_code = "forbid"`.

#![forbid(unsafe_code)]

mod assets;
mod components;
mod copy;
mod state;

use components::adult_screen::AdultScreen;
use components::kids_screen::KidsScreen;
use components::modals::ModalHost;
use components::start_screen::StartScreen;
use components::voice_hud::VoiceHud;
use components::youth_screen::YouthScreen;
use dioxus::desktop::tao::event::{Event, WindowEvent};
use dioxus::desktop::tao::window::{Fullscreen, Icon};
use dioxus::desktop::{
    Config, DesktopContext, LogicalSize, WindowBuilder, use_asset_handler, use_window,
};
use dioxus::prelude::*;
use kural_backend::BackendHandle;
use kural_core::{
    AgeGroup, AppConfig, AppScreen, BackendCommand, CompactTabHint, Language, ModalKind, Scholar,
    SearchTab, Theme, VoiceTurnSummary,
};
use std::path::PathBuf;
use std::sync::{Arc, Once};
use std::time::Duration;

const WINDOW_ICON_PNG: &[u8] = include_bytes!("../../../packaging/linux/icons/128x128.png");

const USAGE_TEXT: &str = "\
Usage: thiruvalluvar-ai [--config PATH] [--pi-preview | --pi7-preview | --pi7-hd-preview | --kiosk] [--verify-layout] [--version] [--help]

Options:
  --config PATH     Load configuration from PATH (also via KURAL_CONFIG env var)
  --pi-preview      Force 3.5-inch LCD preview window (480x320)
  --pi7-preview     Force 7-inch standard LCD preview window (800x480)
  --pi7-hd-preview  Force 7-inch HD LCD preview window (1024x600)
  --kiosk           Borderless fullscreen kiosk mode (Raspberry Pi / cage)
  --verify-layout   Run automated headless WebKitGTK DOM geometry audit
  -V, --version     Print version information and exit
  -h, --help        Print this help message and exit";

#[derive(Debug, Clone, PartialEq, Eq, Default)]
struct CliOptions {
    config: Option<PathBuf>,
    pi_preview: bool,
    pi7_preview: bool,
    pi7_hd_preview: bool,
    kiosk: bool,
    verify_layout: bool,
    show_help: bool,
    show_version: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum AuditViewport {
    Desktop,
    Pi35,
    Pi7,
    Pi7Hd,
}

impl AuditViewport {
    fn from_mode(mode: &str) -> Self {
        match mode {
            "pi_preview" => Self::Pi35,
            "pi7_preview" => Self::Pi7,
            "pi7_hd_preview" => Self::Pi7Hd,
            _ => Self::Desktop,
        }
    }

    fn is_touch_pi(self) -> bool {
        matches!(self, Self::Pi35 | Self::Pi7 | Self::Pi7Hd)
    }

    fn is_pi35(self) -> bool {
        self == Self::Pi35
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum ConnectedScreenTier {
    Pi35Lcd,
    Pi7Lcd,
    Desktop,
}

#[derive(Clone)]
pub(crate) struct BootContext {
    backend: BackendHandle,
    toast_duration_ms: u64,
    audit_viewport: AuditViewport,
    pub(crate) verify_layout: bool,
    auto_adapt_screen: bool,
}

fn parse_cli_args<I, S>(args: I) -> Result<CliOptions, String>
where
    I: IntoIterator<Item = S>,
    S: AsRef<str>,
{
    let mut opts = CliOptions::default();
    let mut it = args.into_iter();
    let _bin = it.next();
    while let Some(raw) = it.next() {
        match raw.as_ref() {
            "--pi-preview" => opts.pi_preview = true,
            "--pi7-preview" => opts.pi7_preview = true,
            "--pi7-hd-preview" => opts.pi7_hd_preview = true,
            "--kiosk" => opts.kiosk = true,
            "--verify-layout" => opts.verify_layout = true,
            "--config" => {
                let path = it
                    .next()
                    .ok_or_else(|| "missing path after --config".to_string())?;
                opts.config = Some(PathBuf::from(path.as_ref()));
            }
            "--help" | "-h" => {
                opts.show_help = true;
                return Ok(opts);
            }
            "--version" | "-V" => {
                opts.show_version = true;
                return Ok(opts);
            }
            other => return Err(format!("unknown argument: {other}")),
        }
    }
    Ok(opts)
}

fn load_window_icon() -> Option<Icon> {
    let img = image::load_from_memory(WINDOW_ICON_PNG).ok()?.into_rgba8();
    let (w, h) = img.dimensions();
    Icon::from_rgba(img.into_raw(), w, h).ok()
}

fn logical_monitor_size(phys_w: u32, phys_h: u32, scale_factor: f64) -> (f64, f64) {
    let scale = if scale_factor.is_finite() && scale_factor > 0.0 {
        scale_factor
    } else {
        1.0
    };
    (f64::from(phys_w) / scale, f64::from(phys_h) / scale)
}

fn classify_connected_screen(logical_w: f64, logical_h: f64) -> ConnectedScreenTier {
    if logical_w <= 560.0 && logical_h <= 380.0 {
        ConnectedScreenTier::Pi35Lcd
    } else if logical_w <= 1060.0 && logical_h <= 640.0 {
        ConnectedScreenTier::Pi7Lcd
    } else {
        ConnectedScreenTier::Desktop
    }
}

fn should_auto_adapt_screen(viewport_mode: &str, cli: &CliOptions) -> bool {
    let has_cli_override = cli.kiosk || cli.pi_preview || cli.pi7_preview || cli.pi7_hd_preview;
    !has_cli_override && !cli.verify_layout && viewport_mode == "desktop"
}

fn apply_connected_screen_adaptation(window: &DesktopContext) -> bool {
    let Some(monitor) = window
        .current_monitor()
        .or_else(|| window.primary_monitor())
    else {
        return false;
    };
    let phys = monitor.size();
    let (logical_w, logical_h) =
        logical_monitor_size(phys.width, phys.height, monitor.scale_factor());
    match classify_connected_screen(logical_w, logical_h) {
        ConnectedScreenTier::Pi35Lcd => {
            window.set_decorations(false);
            window.set_inner_size(LogicalSize::new(logical_w, logical_h));
            window.set_maximized(true);
        }
        ConnectedScreenTier::Pi7Lcd => {
            window.set_inner_size(LogicalSize::new(logical_w, logical_h));
            window.set_maximized(true);
        }
        ConnectedScreenTier::Desktop => {}
    }
    true
}

fn effective_viewport_mode(config_mode: &str, cli: &CliOptions) -> &'static str {
    if cli.kiosk {
        "kiosk"
    } else if cli.pi_preview {
        "pi_preview"
    } else if cli.pi7_preview {
        "pi7_preview"
    } else if cli.pi7_hd_preview {
        "pi7_hd_preview"
    } else if config_mode == "kiosk" {
        "kiosk"
    } else if config_mode == "pi_preview" {
        "pi_preview"
    } else if config_mode == "pi7_preview" {
        "pi7_preview"
    } else if config_mode == "pi7_hd_preview" {
        "pi7_hd_preview"
    } else {
        "desktop"
    }
}

fn build_desktop_config(mode: &str, backend: BackendHandle) -> Config {
    let mut wb = WindowBuilder::new().with_title("Thiruvalluvar AI — திருவள்ளுவர் AI");
    if let Some(icon) = load_window_icon() {
        wb = wb.with_window_icon(Some(icon));
    }
    wb = match mode {
        "kiosk" => wb
            .with_decorations(false)
            .with_fullscreen(Some(Fullscreen::Borderless(None))),
        "pi_preview" => wb
            .with_inner_size(LogicalSize::new(480.0, 320.0))
            .with_resizable(false),
        "pi7_preview" => wb
            .with_inner_size(LogicalSize::new(800.0, 480.0))
            .with_resizable(false),
        "pi7_hd_preview" => wb
            .with_inner_size(LogicalSize::new(1024.0, 600.0))
            .with_resizable(false),
        _ => wb
            .with_inner_size(LogicalSize::new(1280.0, 720.0))
            .with_min_inner_size(LogicalSize::new(480.0, 320.0))
            .with_resizable(true),
    };

    let head = r#"
        <meta name="viewport" content="width=device-width, initial-scale=1.0, maximum-scale=1.0, user-scalable=no">
        <style>html,body{width:100%;height:100dvh;margin:0;overflow:hidden;background:#0B1412;}</style>
    "#
    .to_string();

    let shutdown_once = Arc::new(Once::new());
    let shutdown_handle = backend;

    Config::new()
        .with_window(wb)
        .with_menu(None)
        .with_data_directory(std::env::temp_dir().join("kural-webkit-cache"))
        .with_disable_context_menu(true)
        .with_disable_drag_drop_handler(true)
        .with_navigation_handler(|uri| uri.starts_with("dioxus://"))
        .with_background_color((11, 20, 18, 255))
        .with_custom_head(head)
        .with_custom_event_handler(move |event, _| match event {
            Event::WindowEvent {
                event: WindowEvent::CloseRequested,
                ..
            }
            | Event::LoopDestroyed => {
                let handle = shutdown_handle.clone();
                shutdown_once.call_once(move || handle.shutdown_ref());
            }
            _ => {}
        })
}

/// Launch the desktop GUI application (shared by `thiruvalluvar-ai` and `kural-app`).
pub fn run_app() {
    kural_backend::init_tracing();
    let cli = match parse_cli_args(std::env::args()) {
        Ok(opts) => opts,
        Err(msg) => {
            eprintln!("{msg}");
            std::process::exit(2);
        }
    };

    if cli.show_help {
        println!("{USAGE_TEXT}");
        return;
    }
    if cli.show_version {
        println!("thiruvalluvar-ai {}", env!("CARGO_PKG_VERSION"));
        return;
    }

    let config = match AppConfig::load(cli.config.as_deref()) {
        Ok(c) => c,
        Err(err) => {
            eprintln!("Failed to load configuration: {err}");
            std::process::exit(1);
        }
    };

    let viewport_mode = effective_viewport_mode(&config.ui.viewport_mode, &cli);
    let auto_adapt_screen = should_auto_adapt_screen(viewport_mode, &cli);
    let toast_duration_ms = config.ui.toast_duration_ms;

    let backend = match BackendHandle::start(config) {
        Ok(b) => b,
        Err(err) => {
            eprintln!("Failed to start offline Thiruvalluvar AI backend: {err}");
            std::process::exit(1);
        }
    };

    let desktop_cfg = build_desktop_config(viewport_mode, backend.clone());
    let boot = BootContext {
        backend,
        toast_duration_ms,
        audit_viewport: AuditViewport::from_mode(viewport_mode),
        verify_layout: cli.verify_layout,
        auto_adapt_screen,
    };

    dioxus::LaunchBuilder::desktop()
        .with_context(boot)
        .with_cfg(desktop_cfg)
        .launch(app);
}

fn app() -> Element {
    use_asset_handler("kural", assets::serve_kural_asset);

    let desktop_window = use_window();
    let boot = use_context::<BootContext>();
    let mut ctx = state::provide_app_context(boot.backend.clone(), boot.toast_duration_ms);
    let snap = ctx.snapshot.read().clone();

    if boot.auto_adapt_screen {
        use_hook(move || {
            let window = desktop_window.clone();
            if !apply_connected_screen_adaptation(&window) {
                spawn(async move {
                    tokio::time::sleep(Duration::from_millis(120)).await;
                    let _ = apply_connected_screen_adaptation(&window);
                });
            }
        });
    }

    if boot.verify_layout {
        let backend_exit = boot.backend.clone();
        let viewport = boot.audit_viewport;
        use_hook(move || {
            spawn(async move {
                let ok = run_layout_audit(&mut ctx, viewport).await;
                let _ = std::thread::spawn(move || backend_exit.shutdown_ref()).join();
                std::process::exit(if ok { 0 } else { 1 });
            });
        });
    }

    use_effect(move || {
        let current = ctx.snapshot.read();
        let lang_code = match current.prefs.lang {
            Language::Tamil => "ta",
            Language::English => "en",
        };
        let theme_code = match current.prefs.theme {
            Theme::Dark => "dark",
            Theme::Light => "light",
        };
        let js = format!(
            "document.documentElement.lang = '{lang_code}'; document.documentElement.setAttribute('data-theme', '{theme_code}');"
        );
        let _ = dioxus::document::eval(&js);
    });

    rsx! {
        document::Link { rel: "stylesheet", href: "/kural/fonts.css" }
        document::Link { rel: "stylesheet", href: "/kural/app.css" }
        match snap.screen {
            AppScreen::Start => rsx! { StartScreen {} },
            AppScreen::Kids => rsx! { KidsScreen {} },
            AppScreen::Youth => rsx! { YouthScreen {} },
            AppScreen::Adult => rsx! { AdultScreen {} },
        }
        ModalHost {}
        VoiceHud {}
    }
}

async fn check_dom_state(label: &str, viewport: AuditViewport, expect_kural: bool) -> bool {
    let is_touch = viewport.is_touch_pi();
    let is_pi35 = viewport.is_pi35();
    let is_pi7 = matches!(viewport, AuditViewport::Pi7 | AuditViewport::Pi7Hd);
    let expect_ai_empty = label == "Modal/AiChat/Empty";
    let expect_ai_populated = label == "Modal/AiChat/VoiceTurn";
    tokio::time::sleep(Duration::from_millis(140)).await;
    let js = format!(
        r#"return (() => {{
            const root = document.documentElement;
            const sh = root.scrollHeight, ch = root.clientHeight;
            const sw = root.scrollWidth, cw = root.clientWidth;
            const iw = window.innerWidth, ih = window.innerHeight;
            const errs = [];
            if (sh > ch) errs.push(`scrollH(${{sh}})>${{ch}}`);
            if (sw > cw) errs.push(`scrollW(${{sw}})>${{cw}}`);
            if (sh > ih + 1) errs.push(`scrollH(${{sh}})>innerH(${{ih}})`);
            if (sw > iw + 1) errs.push(`scrollW(${{sw}})>innerW(${{iw}})`);

            const shell = document.getElementById('app-shell') || document.querySelector('.pi-root');
            if (shell) {{
                const sr = shell.getBoundingClientRect();
                if (shell.scrollHeight > shell.clientHeight + 1) errs.push(`shellScrollH(${{shell.scrollHeight}})>${{shell.clientHeight}}`);
                if (sr.height > ch + 1) errs.push(`shellH(${{sr.height}})>${{ch}}`);
            }}

            const hdr = document.getElementById('main-header');
            if (hdr) {{
                const hr = hdr.getBoundingClientRect();
                if ({is_pi7} && hr.height > 56) errs.push(`hdrWrapH(${{hr.height.toFixed(1)}})>56`);
                const clusters = Array.from(hdr.querySelectorAll(':scope > .header-cluster')).filter(el => {{
                    const r = el.getBoundingClientRect();
                    return r.width > 0 && r.height > 0;
                }});
                if (clusters.length === 2) {{
                    const c0 = clusters[0].getBoundingClientRect();
                    const c1 = clusters[1].getBoundingClientRect();
                    if (c0.right > c1.left + 2) errs.push(`hdrCollide(${{c0.right.toFixed(1)}}>${{c1.left.toFixed(1)}})`);
                    if (Math.abs(c0.top - c1.top) > 16) errs.push(`hdrClusterWrap(${{c0.top.toFixed(1)}}vs${{c1.top.toFixed(1)}})`);
                }}
            }}

            const modal = document.querySelector('.modal-card');
            if (!{is_pi35}) {{
                for (const el of document.querySelectorAll('.pi-only')) {{
                    if (el.classList.contains('offline-dot')) continue;
                    const r = el.getBoundingClientRect();
                    if (r.width > 0 || r.height > 0) {{
                        errs.push(`piOnlyLeaked(${{el.id || el.className}}:${{r.width.toFixed(0)}}x${{r.height.toFixed(0)}})`);
                    }}
                }}
            }}
            if (modal) {{
                const mr = modal.getBoundingClientRect();
                if (modal.scrollHeight > modal.clientHeight + 1) errs.push(`modalScrollH(${{modal.scrollHeight}})>${{modal.clientHeight}}`);
                const mBody = modal.querySelector('.modal-body');
                if (mBody) {{
                    if (mBody.scrollWidth > mBody.clientWidth + 1) {{
                        errs.push(`modalBodyScrollW(${{mBody.scrollWidth}})>${{mBody.clientWidth}}`);
                    }}
                    if ({is_pi35} && mBody.scrollHeight > mBody.clientHeight + 1) {{
                        errs.push(`modalBodyScrollH(${{mBody.scrollHeight}})>${{mBody.clientHeight}}`);
                    }}
                }}
                if ({is_pi35} && mr.height > 305) errs.push(`modalH(${{mr.height}})>305`);
                if ({is_pi7} && mr.height > ch - 30) errs.push(`modalH(${{mr.height}})>${{ch - 30}}`);
                if ({is_pi7} && mr.right > cw + 1) errs.push(`modalRight(${{mr.right}})>${{cw}}`);
            }} else if ({is_pi7}) {{
                // Verify primary stage columns are horizontally adjacent (NEVER stacked vertically)
                const zone2 = document.querySelector('.pi-zone-2');
                if (zone2) {{
                    const lEl = zone2.querySelector('.pi-hero-col');
                    const rEl = zone2.querySelector('.pi-right-col');
                    if (!lEl || !rEl) {{
                        errs.push('missingStartCols');
                    }} else {{
                        const lr = lEl.getBoundingClientRect();
                        const rr = rEl.getBoundingClientRect();
                        if (lr.width <= 0 || rr.width <= 0) errs.push(`zeroStartCol(${{lr.width}}x${{rr.width}})`);
                        if (lr.right > rr.left + 4) errs.push(`startStackedX(L.r=${{lr.right.toFixed(1)}}>R.l=${{rr.left.toFixed(1)}})`);
                        if (Math.abs(lr.top - rr.top) > 24) {{
                            errs.push(`startStackedY(L.t=${{lr.top.toFixed(1)}},R.t=${{rr.top.toFixed(1)}})`);
                        }}
                    }}
                    const ageCards = Array.from(zone2.querySelectorAll('.pi-age-card')).map(el => el.getBoundingClientRect());
                    if (ageCards.length === 3) {{
                        for (let i = 1; i < ageCards.length; i++) {{
                            if (Math.abs(ageCards[i].top - ageCards[0].top) > 8 || ageCards[i - 1].right > ageCards[i].left + 4) {{
                                errs.push(`ageGridStacked(${{i}})`);
                            }}
                        }}
                    }}
                }}

                const stage = document.getElementById('main-stage');
                if (stage) {{
                    const stRect = stage.getBoundingClientRect();
                    const cols = Array.from(stage.querySelectorAll(':scope > section')).filter(el => {{
                        const r = el.getBoundingClientRect();
                        return r.width > 0 && r.height > 0;
                    }});
                    if (cols.length !== 2) {{
                        errs.push(`stageCols(${{cols.length}})!=2`);
                    }} else {{
                        const lr = cols[0].getBoundingClientRect();
                        const rr = cols[1].getBoundingClientRect();
                        if (lr.right > rr.left + 4) errs.push(`colsStackedX(L.r=${{lr.right.toFixed(1)}}>R.l=${{rr.left.toFixed(1)}})`);
                        if (Math.abs(lr.top - rr.top) > 24) errs.push(`colsStackedY(L.t=${{lr.top.toFixed(1)}},R.t=${{rr.top.toFixed(1)}})`);
                        if (rr.bottom > stRect.bottom + 2) errs.push(`rightColOverflow(${{rr.bottom.toFixed(1)}}>${{stRect.bottom.toFixed(1)}})`);
                        if (lr.bottom > stRect.bottom + 2) errs.push(`leftColOverflow(${{lr.bottom.toFixed(1)}}>${{stRect.bottom.toFixed(1)}})`);
                        const rKids = Array.from(cols[1].children).map(el => el.getBoundingClientRect()).filter(r => r.width > 0 && r.height > 0);
                        for (let i = 1; i < rKids.length; i++) {{
                            if (rKids[i - 1].bottom > rKids[i].top + 2) {{
                                errs.push(`rightCardsOverlap(${{i}}:${{rKids[i - 1].bottom.toFixed(1)}}>${{rKids[i].top.toFixed(1)}})`);
                            }}
                        }}
                    }}
                }}

                for (const card of document.querySelectorAll('#kural-card, .royal-card-secondary, .clay-card, .pi-voice-card, .kural-center-verse, .comm-box')) {{
                    if (card.classList.contains('pi-only')) continue;
                    const r = card.getBoundingClientRect();
                    if (r.width > 0 && r.height > 0 && card.scrollHeight > card.clientHeight + 3) {{
                        errs.push(`cardClipped(${{card.id || card.className}}:sh=${{card.scrollHeight}}>ch=${{card.clientHeight}})`);
                    }}
                }}

                const kCard = document.getElementById('kural-card');
                if (kCard) {{
                    const kh = document.getElementById('kural-card-header');
                    const kc = document.getElementById('kural-couplet');
                    const ka = document.getElementById('kural-actions');
                    if (kh && kc) {{
                        const khr = kh.getBoundingClientRect();
                        const kcr = kc.getBoundingClientRect();
                        if (kcr.top < khr.bottom - 2) errs.push(`coupletOverlapsHdr(${{kcr.top.toFixed(1)}}<${{khr.bottom.toFixed(1)}})`);
                    }}
                    if (ka) {{
                        const kar = ka.getBoundingClientRect();
                        if (kar.height > 48) errs.push(`actionsWrapped(h=${{kar.height.toFixed(1)}})`);
                    }}
                }}

                const commBody = document.getElementById('comm-body');
                if (commBody) {{
                    const cbr = commBody.getBoundingClientRect();
                    if (cbr.height < 34) errs.push(`commBodyCrushed(h=${{cbr.height.toFixed(1)}})`);
                }}

                const dock = document.querySelector('.desktop-footer-dock');
                if (dock) {{
                    const dr = dock.getBoundingClientRect();
                    if (dr.height <= 0 || dr.height > 64) errs.push(`dockH(${{dr.height.toFixed(1)}})`);
                    const dKids = Array.from(dock.children).map(el => el.getBoundingClientRect()).filter(r => r.width > 0 && r.height > 0);
                    for (let i = 1; i < dKids.length; i++) {{
                        if (dKids[i - 1].right > dKids[i].left + 2) errs.push(`dockOverlap(${{i}})`);
                        if (Math.abs(dKids[i].top - dKids[0].top) > 20) errs.push(`dockWrap(${{i}})`);
                    }}
                }}
            }}

            if (document.querySelector('.adult-main-col, .adult-side-col') && document.querySelector('.adult-themes-card')) {{
                errs.push('themesCardPresent');
            }}

            if ({expect_kural}) {{
                const c = document.getElementById('kural-couplet');
                if (c) {{
                    const fs = parseFloat(getComputedStyle(c).fontSize);
                    const minFs = (!{is_touch} && document.documentElement.lang === 'en') ? 16 : 14;
                    if (fs < minFs) errs.push(`coupletFont(${{fs}})<${{minFs}}`);
                    const txt = (c.textContent || '').trim();
                    if (!txt) errs.push('coupletEmpty');
                    if (document.documentElement.lang === 'en') {{
                        if (!/[A-Za-z]/.test(txt)) errs.push('coupletNotEnglish');
                        if (/[\u0B80-\u0BFF]/.test(txt)) errs.push('coupletHasTamilInEn');
                    }} else if (!/[\u0B80-\u0BFF]/.test(txt)) {{
                        errs.push('coupletNotTamil');
                    }}
                }}
                for (const el of document.querySelectorAll('#kural-meaning, #comm-body, .pi-prose-text')) {{
                    const r = el.getBoundingClientRect();
                    if (r.width > 0 && r.height > 0) {{
                        const fs = parseFloat(getComputedStyle(el).fontSize);
                        if (fs < 14) errs.push(`proseFont(${{fs}})<14`);
                    }}
                }}
            }}
            if ({is_touch}) {{
                for (const btn of document.querySelectorAll('button, input')) {{
                    const r = btn.getBoundingClientRect();
                    if (r.width > 0 && r.height > 0 && r.height < 37.5) {{
                        errs.push(`smallCtrl(${{btn.id || btn.className}}:${{r.width.toFixed(0)}}x${{r.height.toFixed(1)}})`);
                    }}
                }}
            }}
            if ({expect_ai_empty}) {{
                if (document.querySelector('.ai-answer-card')) {{
                    errs.push('aiAnswerCardPresentInEmpty');
                }}
                if (!document.getElementById('ai-empty-prompt')) {{
                    errs.push('aiEmptyPromptMissing');
                }}
            }}
            if ({expect_ai_populated}) {{
                if (!document.querySelector('.ai-answer-card')) {{
                    errs.push('aiAnswerCardMissingInPopulated');
                }}
                if (document.getElementById('ai-empty-prompt')) {{
                    errs.push('aiEmptyPromptPresentInPopulated');
                }}
            }}
            return errs.length === 0 ? `OK|${{ch}}x${{cw}}|scroll=${{sh}}x${{sw}}` : `FAIL|${{ch}}x${{cw}}|${{errs.join(',')}}`;
        }})();"#
    );
    for attempt in 0..2 {
        match dioxus::document::eval(&js).join::<String>().await {
            Ok(res) if res.starts_with("OK|") => {
                println!("AUDIT PASS [{label}]: {res}");
                return true;
            }
            Ok(res) => {
                eprintln!("AUDIT FAIL [{label}]: {res}");
                return false;
            }
            Err(err) if attempt == 0 => {
                tokio::time::sleep(Duration::from_millis(80)).await;
                let _ = err;
            }
            Err(err) => {
                eprintln!("AUDIT ERR  [{label}]: {err:?}");
                return false;
            }
        }
    }
    false
}

async fn run_layout_audit(ctx: &mut state::AppContext, viewport: AuditViewport) -> bool {
    let is_touch = viewport.is_touch_pi();
    tokio::time::sleep(Duration::from_millis(450)).await;
    ctx.dispatch(BackendCommand::SetAutoSpeakText(false));
    tokio::time::sleep(Duration::from_millis(80)).await;
    let mut all_ok = true;

    let combos: &[(&str, BackendCommand, bool)] = &[
        ("Start", BackendCommand::Navigate(AppScreen::Start), false),
        ("Kids", BackendCommand::SelectAge(AgeGroup::Kids), true),
        ("Youth", BackendCommand::SelectAge(AgeGroup::Youth), true),
        ("Adult", BackendCommand::SelectAge(AgeGroup::Adult), true),
    ];

    for (name, nav_cmd, expect_kural) in combos {
        ctx.dispatch(nav_cmd.clone());
        if *name == "Adult" {
            ctx.dispatch(BackendCommand::OpenKural(395));
            ctx.dispatch(BackendCommand::SelectCommentary(Scholar::Parimel));
        } else if *name == "Kids" || *name == "Youth" {
            ctx.dispatch(BackendCommand::OpenKural(1));
        }
        for (lang_lbl, lang) in [("ta", Language::Tamil), ("en", Language::English)] {
            ctx.dispatch(BackendCommand::SetLanguage(lang));
            for (theme_lbl, theme) in [("dark", Theme::Dark), ("light", Theme::Light)] {
                ctx.dispatch(BackendCommand::SetTheme(theme));
                ctx.pi_compact_tab.set(CompactTabHint::Verse);
                for _ in 0..40 {
                    let ready = {
                        let s = ctx.snapshot.peek();
                        s.prefs.lang == lang && s.prefs.theme == theme
                    };
                    if ready {
                        break;
                    }
                    tokio::time::sleep(Duration::from_millis(20)).await;
                }
                let expected_html_lang = match lang {
                    Language::Tamil => "ta",
                    Language::English => "en",
                };
                let _ = dioxus::document::eval(&format!(
                    "document.documentElement.lang = '{expected_html_lang}';"
                ))
                .join::<()>()
                .await;
                let label = format!("{name}/{lang_lbl}/{theme_lbl}");
                all_ok &= check_dom_state(&label, viewport, *expect_kural).await;
            }
        }
    }

    if is_touch {
        ctx.dispatch(BackendCommand::SetLanguage(Language::Tamil));
        ctx.dispatch(BackendCommand::SetTheme(Theme::Dark));
        ctx.dispatch(BackendCommand::SelectAge(AgeGroup::Youth));
        ctx.dispatch(BackendCommand::OpenKural(1));
        for tab in [CompactTabHint::Analysis, CompactTabHint::Quiz] {
            ctx.pi_compact_tab.set(tab);
            all_ok &= check_dom_state(&format!("Youth/tab/{tab:?}"), viewport, false).await;
        }

        ctx.dispatch(BackendCommand::SelectAge(AgeGroup::Adult));
        ctx.dispatch(BackendCommand::OpenKural(395));
        ctx.dispatch(BackendCommand::SelectCommentary(Scholar::Parimel));
        for tab in [CompactTabHint::Commentary, CompactTabHint::Study] {
            ctx.pi_compact_tab.set(tab);
            all_ok &= check_dom_state(&format!("Adult/tab/{tab:?}"), viewport, false).await;
        }

        ctx.dispatch(BackendCommand::SelectAge(AgeGroup::Kids));
        ctx.dispatch(BackendCommand::OpenKural(1));
        for modal in [ModalKind::KidsStory, ModalKind::KidsWordGame] {
            ctx.dispatch(BackendCommand::OpenModal(modal));
            all_ok &= check_dom_state(&format!("Modal/{modal:?}"), viewport, false).await;
        }
        ctx.dispatch(BackendCommand::SelectAge(AgeGroup::Youth));
        for modal in [ModalKind::YouthQuiz, ModalKind::YouthDeepDive] {
            ctx.dispatch(BackendCommand::OpenModal(modal));
            all_ok &= check_dom_state(&format!("Modal/{modal:?}"), viewport, false).await;
        }
        ctx.dispatch(BackendCommand::OpenModal(ModalKind::Search {
            initial_tab: SearchTab::Divisions,
        }));
        all_ok &= check_dom_state("Modal/Search/Divisions", viewport, false).await;
        ctx.dispatch(BackendCommand::CloseModal);
    }

    // Verify Text-to-Voice + Voice Replay modals on EVERY viewport tier (Desktop, 7", 7" HD, 3.5").
    ctx.dispatch(BackendCommand::SetLanguage(Language::Tamil));
    ctx.dispatch(BackendCommand::SetTheme(Theme::Dark));
    ctx.dispatch(BackendCommand::SelectAge(AgeGroup::Youth));
    ctx.dispatch(BackendCommand::OpenKural(1));
    ctx.dispatch(BackendCommand::SetAutoSpeakText(false));
    ctx.dispatch(BackendCommand::OpenModal(ModalKind::AiChat {
        seed_kural: Some(1),
    }));
    for _ in 0..20 {
        if matches!(ctx.snapshot.peek().modal, Some(ModalKind::AiChat { .. })) {
            break;
        }
        tokio::time::sleep(Duration::from_millis(10)).await;
    }
    tokio::time::sleep(Duration::from_millis(80)).await;
    all_ok &= check_dom_state("Modal/AiChat/Empty", viewport, false).await;

    ctx.dispatch(BackendCommand::AskAiQuestion {
        query: "What is virtue?".to_string(),
        seed_kural: Some(1),
    });
    for _ in 0..40 {
        if ctx.ai_answer.peek().is_some() {
            break;
        }
        tokio::time::sleep(Duration::from_millis(40)).await;
    }
    tokio::time::sleep(Duration::from_millis(120)).await;
    let mut audited = ctx.snapshot.peek().as_ref().clone();
    audited.playback_status = kural_core::AudioPlaybackStatus::Completed;
    audited.can_replay_tts = true;
    audited.can_replay_voice_turn = true;
    audited.recent_voice_turns = vec![VoiceTurnSummary {
        id: 1,
        user_text: "What is virtue?".to_string(),
        ai_text: "Virtue keeps thought pure.".to_string(),
        lang: Language::Tamil,
        has_user_audio: true,
        has_ai_audio: true,
        cited_kurals: vec![1],
    }];
    ctx.snapshot.set(Arc::new(audited));
    all_ok &= check_dom_state("Modal/AiChat/VoiceTurn", viewport, false).await;

    ctx.dispatch(BackendCommand::OpenModal(ModalKind::Search {
        initial_tab: SearchTab::Query,
    }));
    ctx.dispatch(BackendCommand::SubmitTextSearch {
        query: "virtue".to_string(),
        division_id: None,
    });
    all_ok &= check_dom_state("Modal/Search/Query", viewport, false).await;
    ctx.dispatch(BackendCommand::CloseModal);
    ctx.dispatch(BackendCommand::SetAutoSpeakText(true));

    all_ok
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    use std::path::Path;

    fn collect_text_files(dir: &Path, out: &mut Vec<PathBuf>) {
        if let Ok(entries) = fs::read_dir(dir) {
            for entry in entries.flatten() {
                let path = entry.path();
                if path.is_dir() {
                    collect_text_files(&path, out);
                } else if matches!(
                    path.extension().and_then(|s| s.to_str()),
                    Some("rs" | "css" | "html" | "toml")
                ) {
                    out.push(path);
                }
            }
        }
    }

    #[test]
    fn cli_flags_and_viewport_mode_resolve_deterministically() {
        let opts = parse_cli_args(["kural-app", "--pi-preview", "--config", "custom.toml"])
            .expect("parse flags");
        assert!(opts.pi_preview);
        assert!(!opts.pi7_preview);
        assert!(!opts.pi7_hd_preview);
        assert!(!opts.kiosk);
        assert_eq!(opts.config.as_deref(), Some(Path::new("custom.toml")));
        assert_eq!(effective_viewport_mode("desktop", &opts), "pi_preview");

        let pi7 = parse_cli_args(["kural-app", "--pi7-preview"]).unwrap();
        assert_eq!(effective_viewport_mode("desktop", &pi7), "pi7_preview");
        assert_eq!(AuditViewport::from_mode("pi7_preview"), AuditViewport::Pi7);

        let pi7_hd = parse_cli_args(["kural-app", "--pi7-hd-preview"]).unwrap();
        assert_eq!(
            effective_viewport_mode("desktop", &pi7_hd),
            "pi7_hd_preview"
        );
        assert_eq!(
            AuditViewport::from_mode("pi7_hd_preview"),
            AuditViewport::Pi7Hd
        );

        let kiosk = parse_cli_args(["kural-app", "--pi-preview", "--kiosk"]).unwrap();
        assert_eq!(effective_viewport_mode("desktop", &kiosk), "kiosk");
        assert!(parse_cli_args(["kural-app", "--unknown"]).is_err());

        let help = parse_cli_args(["thiruvalluvar-ai", "--help"]).unwrap();
        assert!(help.show_help);
        let ver = parse_cli_args(["thiruvalluvar-ai", "--version"]).unwrap();
        assert!(ver.show_version);
        assert!(load_window_icon().is_some());
    }

    #[test]
    fn connected_screen_classification_matches_tri_display_tiers() {
        assert_eq!(logical_monitor_size(480, 320, 1.0), (480.0, 320.0));
        assert_eq!(logical_monitor_size(2560, 1440, 2.0), (1280.0, 720.0));
        assert_eq!(logical_monitor_size(800, 480, 0.0), (800.0, 480.0));

        assert_eq!(
            classify_connected_screen(480.0, 320.0),
            ConnectedScreenTier::Pi35Lcd
        );
        assert_eq!(
            classify_connected_screen(560.0, 380.0),
            ConnectedScreenTier::Pi35Lcd
        );
        assert_eq!(
            classify_connected_screen(800.0, 480.0),
            ConnectedScreenTier::Pi7Lcd
        );
        assert_eq!(
            classify_connected_screen(1024.0, 600.0),
            ConnectedScreenTier::Pi7Lcd
        );
        assert_eq!(
            classify_connected_screen(1280.0, 720.0),
            ConnectedScreenTier::Desktop
        );
        assert_eq!(
            classify_connected_screen(1920.0, 1080.0),
            ConnectedScreenTier::Desktop
        );

        let default_cli = parse_cli_args(["thiruvalluvar-ai"]).unwrap();
        assert!(should_auto_adapt_screen("desktop", &default_cli));
        assert!(!should_auto_adapt_screen("kiosk", &default_cli));

        let pi_flag = parse_cli_args(["thiruvalluvar-ai", "--pi7-preview"]).unwrap();
        assert!(!should_auto_adapt_screen("desktop", &pi_flag));

        let verify_flag = parse_cli_args(["thiruvalluvar-ai", "--verify-layout"]).unwrap();
        assert!(!should_auto_adapt_screen("desktop", &verify_flag));
    }

    #[test]
    fn kural_app_contains_zero_external_http_urls() {
        let root = Path::new(env!("CARGO_MANIFEST_DIR"));
        let mut files = Vec::new();
        collect_text_files(&root.join("src"), &mut files);
        collect_text_files(&root.join("assets"), &mut files);
        assert!(!files.is_empty());

        let http_scheme = ["http", "://"].concat();
        let https_scheme = ["https", "://"].concat();
        for file in files {
            let content = fs::read_to_string(&file).expect("read text file");
            for (idx, line) in content.lines().enumerate() {
                if (line.contains(&http_scheme) || line.contains(&https_scheme))
                    && !line.contains("http://www.w3.org/2000/svg")
                {
                    panic!(
                        "external URL found in {} line {}: {}",
                        file.display(),
                        idx + 1,
                        line.trim()
                    );
                }
            }
        }
    }
}
