//! Embedded offline static assets (`app.css`, `fonts.css`, `.woff2` fonts, and portrait PNG).

use dioxus::desktop::wry::http::{Response, header::CONTENT_TYPE, status::StatusCode};
use dioxus::desktop::{AssetRequest, RequestAsyncResponder};

const APP_CSS: &[u8] = include_bytes!("../assets/app.css");
const FONTS_CSS: &[u8] = include_bytes!("../assets/fonts.css");
const PORTRAIT_PNG: &[u8] = include_bytes!("../assets/valluvar-portrait.png");
const APP_LOGO_PNG: &[u8] = include_bytes!("../assets/app-logo.png");

struct EmbeddedAsset {
    path: &'static str,
    mime: &'static str,
    bytes: &'static [u8],
}

const ASSETS: &[EmbeddedAsset] = &[
    EmbeddedAsset {
        path: "app.css",
        mime: "text/css; charset=utf-8",
        bytes: APP_CSS,
    },
    EmbeddedAsset {
        path: "fonts.css",
        mime: "text/css; charset=utf-8",
        bytes: FONTS_CSS,
    },
    EmbeddedAsset {
        path: "valluvar-portrait.png",
        mime: "image/png",
        bytes: PORTRAIT_PNG,
    },
    EmbeddedAsset {
        path: "app-logo.png",
        mime: "image/png",
        bytes: APP_LOGO_PNG,
    },
    EmbeddedAsset {
        path: "fonts/font_1_cY9cfjeOW0NHpmOQXranrbDyu7hADpI.woff2",
        mime: "font/woff2",
        bytes: include_bytes!("../assets/fonts/font_1_cY9cfjeOW0NHpmOQXranrbDyu7hADpI.woff2"),
    },
    EmbeddedAsset {
        path: "fonts/font_2_cY9cfjeOW0NHpmOQXranrbDyu7hODpKBZQ.woff2",
        mime: "font/woff2",
        bytes: include_bytes!("../assets/fonts/font_2_cY9cfjeOW0NHpmOQXranrbDyu7hODpKBZQ.woff2"),
    },
    EmbeddedAsset {
        path: "fonts/font_3_cY9cfjeOW0NHpmOQXranrbDyu7hPDpKBZQ.woff2",
        mime: "font/woff2",
        bytes: include_bytes!("../assets/fonts/font_3_cY9cfjeOW0NHpmOQXranrbDyu7hPDpKBZQ.woff2"),
    },
    EmbeddedAsset {
        path: "fonts/font_4_cY9cfjeOW0NHpmOQXranrbDyu7hWDpKBZQ.woff2",
        mime: "font/woff2",
        bytes: include_bytes!("../assets/fonts/font_4_cY9cfjeOW0NHpmOQXranrbDyu7hWDpKBZQ.woff2"),
    },
    EmbeddedAsset {
        path: "fonts/font_5_UcC73FwrK3iLTeHuS_nVMrMxCp50SjIa0ZL7SUc.woff2",
        mime: "font/woff2",
        bytes: include_bytes!(
            "../assets/fonts/font_5_UcC73FwrK3iLTeHuS_nVMrMxCp50SjIa0ZL7SUc.woff2"
        ),
    },
    EmbeddedAsset {
        path: "fonts/font_6_UcC73FwrK3iLTeHuS_nVMrMxCp50SjIa1pL7SUc.woff2",
        mime: "font/woff2",
        bytes: include_bytes!(
            "../assets/fonts/font_6_UcC73FwrK3iLTeHuS_nVMrMxCp50SjIa1pL7SUc.woff2"
        ),
    },
    EmbeddedAsset {
        path: "fonts/font_7_UcC73FwrK3iLTeHuS_nVMrMxCp50SjIa1ZL7.woff2",
        mime: "font/woff2",
        bytes: include_bytes!("../assets/fonts/font_7_UcC73FwrK3iLTeHuS_nVMrMxCp50SjIa1ZL7.woff2"),
    },
    EmbeddedAsset {
        path: "fonts/font_8_UcC73FwrK3iLTeHuS_nVMrMxCp50SjIa25L7SUc.woff2",
        mime: "font/woff2",
        bytes: include_bytes!(
            "../assets/fonts/font_8_UcC73FwrK3iLTeHuS_nVMrMxCp50SjIa25L7SUc.woff2"
        ),
    },
    EmbeddedAsset {
        path: "fonts/font_9_UcC73FwrK3iLTeHuS_nVMrMxCp50SjIa2JL7SUc.woff2",
        mime: "font/woff2",
        bytes: include_bytes!(
            "../assets/fonts/font_9_UcC73FwrK3iLTeHuS_nVMrMxCp50SjIa2JL7SUc.woff2"
        ),
    },
    EmbeddedAsset {
        path: "fonts/font_10_UcC73FwrK3iLTeHuS_nVMrMxCp50SjIa2pL7SUc.woff2",
        mime: "font/woff2",
        bytes: include_bytes!(
            "../assets/fonts/font_10_UcC73FwrK3iLTeHuS_nVMrMxCp50SjIa2pL7SUc.woff2"
        ),
    },
    EmbeddedAsset {
        path: "fonts/font_11_UcC73FwrK3iLTeHuS_nVMrMxCp50SjIa2ZL7SUc.woff2",
        mime: "font/woff2",
        bytes: include_bytes!(
            "../assets/fonts/font_11_UcC73FwrK3iLTeHuS_nVMrMxCp50SjIa2ZL7SUc.woff2"
        ),
    },
    EmbeddedAsset {
        path: "fonts/font_12_ieVp2YdFI3GCY6SyQy1KfStzYKZgzN1z4LKDbeZce-048cFpwEFI.woff2",
        mime: "font/woff2",
        bytes: include_bytes!(
            "../assets/fonts/font_12_ieVp2YdFI3GCY6SyQy1KfStzYKZgzN1z4LKDbeZce-048cFpwEFI.woff2"
        ),
    },
    EmbeddedAsset {
        path: "fonts/font_13_ieVp2YdFI3GCY6SyQy1KfStzYKZgzN1z4LKDbeZce-048ddpwA.woff2",
        mime: "font/woff2",
        bytes: include_bytes!(
            "../assets/fonts/font_13_ieVp2YdFI3GCY6SyQy1KfStzYKZgzN1z4LKDbeZce-048ddpwA.woff2"
        ),
    },
    EmbeddedAsset {
        path: "fonts/font_14_ieVp2YdFI3GCY6SyQy1KfStzYKZgzN1z4LKDbeZce-048dlpwEFI.woff2",
        mime: "font/woff2",
        bytes: include_bytes!(
            "../assets/fonts/font_14_ieVp2YdFI3GCY6SyQy1KfStzYKZgzN1z4LKDbeZce-048dlpwEFI.woff2"
        ),
    },
    EmbeddedAsset {
        path: "fonts/font_15_LYjUdHr-klIgTfc40komjQ5OObazYp-6H94dBF-RX6nNRL340JYReA.woff2",
        mime: "font/woff2",
        bytes: include_bytes!(
            "../assets/fonts/font_15_LYjUdHr-klIgTfc40komjQ5OObazYp-6H94dBF-RX6nNRL340JYReA.woff2"
        ),
    },
    EmbeddedAsset {
        path: "fonts/font_16_LYjUdHr-klIgTfc40komjQ5OObazYp-6H94dBF-RX6nNRL3g0JYReA.woff2",
        mime: "font/woff2",
        bytes: include_bytes!(
            "../assets/fonts/font_16_LYjUdHr-klIgTfc40komjQ5OObazYp-6H94dBF-RX6nNRL3g0JYReA.woff2"
        ),
    },
    EmbeddedAsset {
        path: "fonts/font_17_LYjUdHr-klIgTfc40komjQ5OObazYp-6H94dBF-RX6nNRL3u0JY.woff2",
        mime: "font/woff2",
        bytes: include_bytes!(
            "../assets/fonts/font_17_LYjUdHr-klIgTfc40komjQ5OObazYp-6H94dBF-RX6nNRL3u0JY.woff2"
        ),
    },
];

pub fn resolve_asset(uri_path: &str) -> Option<(&'static str, &'static [u8])> {
    let rel = uri_path
        .trim_start_matches('/')
        .strip_prefix("kural/")
        .unwrap_or(uri_path.trim_start_matches('/'));
    ASSETS
        .iter()
        .find(|a| a.path == rel)
        .map(|a| (a.mime, a.bytes))
}

pub fn serve_kural_asset(request: AssetRequest, responder: RequestAsyncResponder) {
    let resp = match resolve_asset(request.uri().path()) {
        Some((mime, bytes)) => Response::builder()
            .status(StatusCode::OK)
            .header(CONTENT_TYPE, mime)
            .body(bytes.to_vec()),
        None => Response::builder()
            .status(StatusCode::NOT_FOUND)
            .body(b"Asset not found".to_vec()),
    };
    if let Ok(r) = resp {
        responder.respond(r);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn all_offline_assets_resolve_without_external_urls() {
        assert!(resolve_asset("/kural/app.css").is_some());
        assert!(resolve_asset("/kural/fonts.css").is_some());
        let (_, png) = resolve_asset("/kural/valluvar-portrait.png").expect("portrait png");
        assert!(png.starts_with(b"\x89PNG\r\n\x1a\n"));
        let (_, logo_png) = resolve_asset("/kural/app-logo.png").expect("app logo png");
        assert!(logo_png.starts_with(b"\x89PNG\r\n\x1a\n"));

        let http_scheme = ["http", "://"].concat();
        let https_scheme = ["https", "://"].concat();

        let fonts_css = std::str::from_utf8(FONTS_CSS).expect("fonts.css utf8");
        assert!(!fonts_css.contains(&https_scheme));
        assert!(!fonts_css.contains(&http_scheme));

        let app_css = std::str::from_utf8(APP_CSS).expect("app.css utf8");
        assert!(!app_css.contains(&https_scheme));
        assert!(!app_css.contains(&http_scheme));
        assert!(!app_css.contains("height: 1024px"));
        assert!(app_css.contains("height: 100dvh !important"));
        assert!(app_css.contains("#kural-meaning, .pi-prose-text { font-size: 14px !important"));
        assert!(app_css.contains("max-height: 304px !important"));

        // Every /kural/fonts/*.woff2 referenced in fonts.css exists in ASSETS.
        for token in fonts_css.split("url(").skip(1) {
            let url = token.split(')').next().unwrap_or("");
            assert!(
                resolve_asset(url).is_some(),
                "missing bundled font asset: {url}"
            );
        }
    }
}
