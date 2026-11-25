//! Pure-Rust Dioxus Desktop entrypoint for Thiruvalluvar AI.
//! Zero cloud URLs, zero Python, strict `unsafe_code = "forbid"`, and Linux `tikv-jemallocator`.

#![forbid(unsafe_code)]

#[cfg(target_os = "linux")]
#[global_allocator]
static GLOBAL: tikv_jemallocator::Jemalloc = tikv_jemallocator::Jemalloc;

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
use dioxus::desktop::tao::window::Fullscreen;
use dioxus::desktop::{Config, LogicalSize, WindowBuilder, use_asset_handler};
use dioxus::prelude::*;
use kural_backend::BackendHandle;
use kural_core::{
    AgeGroup, AppConfig, AppScreen, BackendCommand, CompactTabHint, Language, ModalKind, Scholar,
    SearchTab, Theme,
};
use std::path::PathBuf;
use std::sync::{Arc, Once};
use std::time::Duration;

#[derive(Debug, Clone, PartialEq, Eq, Default)]
struct CliOptions {
    config: Option<PathBuf>,
    pi_preview: bool,
    pi7_preview: bool,
    pi7_hd_preview: bool,
    kiosk: bool,
    verify_layout: bool,
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

#[derive(Clone)]
struct BootContext {
    backend: BackendHandle,
    toast_duration_ms: u64,
    audit_viewport: AuditViewport,
    verify_layout: bool,
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
                return Err(
                    "Usage: kural-app [--config PATH] [--pi-preview | --pi7-preview | --pi7-hd-preview | --kiosk] [--verify-layout]"
                        .to_string(),
                );
            }
            other => return Err(format!("unknown argument: {other}")),
        }
    }
    Ok(opts)
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

fn main() {
    let cli = match parse_cli_args(std::env::args()) {
        Ok(opts) => opts,
        Err(msg) => {
            eprintln!("{msg}");
            std::process::exit(2);
        }
    };

    let config = match AppConfig::load(cli.config.as_deref()) {
        Ok(c) => c,
        Err(err) => {
            eprintln!("Failed to load configuration: {err}");
            std::process::exit(1);
        }
    };

    let viewport_mode = effective_viewport_mode(&config.ui.viewport_mode, &cli);
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
    };

    dioxus::LaunchBuilder::desktop()
        .with_context(boot)
        .with_cfg(desktop_cfg)
        .launch(app);
}

fn app() -> Element {
    use_asset_handler("kural", assets::serve_kural_asset);

    let boot = use_context::<BootContext>();
    let mut ctx = state::provide_app_context(boot.backend.clone(), boot.toast_duration_ms);
    let snap = ctx.snapshot.read().clone();

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
    tokio::time::sleep(Duration::from_millis(140)).await;
    let js = format!(
        r#"return (() => {{
            const root = document.documentElement;
            const sh = root.scrollHeight, ch = root.clientHeight;
            const sw = root.scrollWidth, cw = root.clientWidth;
            const errs = [];
            if (sh > ch) errs.push(`scrollH(${{sh}})>${{ch}}`);
            if (sw > cw) errs.push(`scrollW(${{sw}})>${{cw}}`);
            const shell = document.getElementById('app-shell') || document.querySelector('.pi-root');
            if (shell) {{
                const sr = shell.getBoundingClientRect();
                if (shell.scrollHeight > shell.clientHeight + 1) errs.push(`shellScrollH(${{shell.scrollHeight}})>${{shell.clientHeight}}`);
                if (sr.height > ch + 1) errs.push(`shellH(${{sr.height}})>${{ch}}`);
            }}
            const modal = document.querySelector('.modal-card');
            if (modal) {{
                const mr = modal.getBoundingClientRect();
                if (modal.scrollHeight > modal.clientHeight + 1) errs.push(`modalScrollH(${{modal.scrollHeight}})>${{modal.clientHeight}}`);
                if ({is_pi35} && mr.height > 305) errs.push(`modalH(${{mr.height}})>305`);
                if (!{is_pi35} && {is_touch} && mr.height > ch - 34) errs.push(`modalH(${{mr.height}})>${{ch - 34}}`);
            }}
            if ({expect_kural}) {{
                const c = document.getElementById('kural-couplet');
                if (c) {{
                    const fs = parseFloat(getComputedStyle(c).fontSize);
                    if (fs < 14) errs.push(`coupletFont(${{fs}})<14`);
                    if (!/[\u0B80-\u0BFF]/.test(c.textContent || '')) errs.push('coupletNotTamil');
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
            return errs.length === 0 ? `OK|${{ch}}x${{cw}}|scroll=${{sh}}x${{sw}}` : `FAIL|${{errs.join(',')}}`;
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
        ctx.dispatch(BackendCommand::OpenModal(ModalKind::AiChat {
            seed_kural: Some(1),
        }));
        ctx.dispatch(BackendCommand::AskAiQuestion {
            query: "What is virtue?".to_string(),
            seed_kural: Some(1),
        });
        all_ok &= check_dom_state("Modal/AiChat", viewport, false).await;

        ctx.dispatch(BackendCommand::OpenModal(ModalKind::Search {
            initial_tab: SearchTab::Query,
        }));
        ctx.dispatch(BackendCommand::SubmitTextSearch {
            query: "virtue".to_string(),
            division_id: None,
        });
        all_ok &= check_dom_state("Modal/Search/Query", viewport, false).await;

        ctx.dispatch(BackendCommand::CloseModal);
        tokio::time::sleep(Duration::from_millis(60)).await;
        ctx.dispatch(BackendCommand::OpenModal(ModalKind::Search {
            initial_tab: SearchTab::Divisions,
        }));
        all_ok &= check_dom_state("Modal/Search/Divisions", viewport, false).await;
        ctx.dispatch(BackendCommand::CloseModal);
    }

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
