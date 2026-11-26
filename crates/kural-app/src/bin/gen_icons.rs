//! Pure-Rust PNG icon generator for Linux hicolor desktop icons (`packaging/linux/icons/`)
//! and the embedded in-app logo (`crates/kural-app/assets/app-logo.png`).
//!
//! Usage:
//!   cargo run -p kural-app --bin gen-icons
//!   cargo run -p kural-app --bin gen-icons -- --input path/to/logo.png [--circle-alpha]
//!   cargo run -p kural-app --bin gen-icons -- --from-portrait [PORTRAIT_PNG]

#![forbid(unsafe_code)]

use image::imageops::FilterType;
use image::{DynamicImage, GenericImageView, ImageBuffer, Rgba, RgbaImage};
use std::collections::VecDeque;
use std::env;
use std::fs;
use std::path::{Path, PathBuf};

const HICOLOR_SIZES: &[u32] = &[512, 256, 128, 64, 48, 32];
const CANVAS_SIZE: u32 = 512;

fn workspace_root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../..")
        .canonicalize()
        .unwrap_or_else(|_| PathBuf::from("."))
}

fn lerp_u8(a: u8, b: u8, t: f32) -> u8 {
    let clamped = t.clamp(0.0, 1.0);
    (f32::from(a) + (f32::from(b) - f32::from(a)) * clamped)
        .round()
        .clamp(0.0, 255.0) as u8
}

fn blend_rgba(base: Rgba<u8>, over: Rgba<u8>) -> Rgba<u8> {
    let a = f32::from(over[3]) / 255.0;
    let inv = 1.0 - a;
    Rgba([
        ((f32::from(over[0]) * a + f32::from(base[0]) * inv).round()).clamp(0.0, 255.0) as u8,
        ((f32::from(over[1]) * a + f32::from(base[1]) * inv).round()).clamp(0.0, 255.0) as u8,
        ((f32::from(over[2]) * a + f32::from(base[2]) * inv).round()).clamp(0.0, 255.0) as u8,
        (f32::from(base[3]).max(f32::from(over[3])).round()).clamp(0.0, 255.0) as u8,
    ])
}

fn is_near_white(p: Rgba<u8>) -> bool {
    p[0] >= 242 && p[1] >= 242 && p[2] >= 242
}

/// Removes exterior solid white background from a circular medallion render,
/// crops tightly around the gold outer rim with slight margin, and scales to 512x512 RGBA8.
fn extract_transparent_medallion(img: DynamicImage) -> Result<RgbaImage, String> {
    let mut rgba = img.to_rgba8();
    let (w, h) = rgba.dimensions();
    if w < 64 || h < 64 {
        return Err("source medallion image is unexpectedly small".into());
    }

    // 1. Flood-fill from image corners/edges through near-white pixels to strip outer background cleanly.
    let mut visited = vec![false; (w * h) as usize];
    let mut queue = VecDeque::new();
    for x in 0..w {
        for y in [0_u32, h - 1] {
            let idx = (y * w + x) as usize;
            if !visited[idx] && is_near_white(*rgba.get_pixel(x, y)) {
                visited[idx] = true;
                queue.push_back((x, y));
            }
        }
    }
    for y in 0..h {
        for x in [0_u32, w - 1] {
            let idx = (y * w + x) as usize;
            if !visited[idx] && is_near_white(*rgba.get_pixel(x, y)) {
                visited[idx] = true;
                queue.push_back((x, y));
            }
        }
    }

    while let Some((x, y)) = queue.pop_front() {
        rgba.put_pixel(x, y, Rgba([0, 0, 0, 0]));
        let neighbors = [
            (x.wrapping_sub(1), y, x > 0),
            (x + 1, y, x + 1 < w),
            (x, y.wrapping_sub(1), y > 0),
            (x, y + 1, y + 1 < h),
        ];
        for (nx, ny, ok) in neighbors {
            if ok {
                let idx = (ny * w + nx) as usize;
                if !visited[idx] && is_near_white(*rgba.get_pixel(nx, ny)) {
                    visited[idx] = true;
                    queue.push_back((nx, ny));
                }
            }
        }
    }

    // 2. Find bounding box of opaque medallion pixels.
    let (mut min_x, mut min_y, mut max_x, mut max_y) = (w, h, 0_u32, 0_u32);
    for y in 0..h {
        for x in 0..w {
            if rgba.get_pixel(x, y)[3] > 0 {
                min_x = min_x.min(x);
                min_y = min_y.min(y);
                max_x = max_x.max(x);
                max_y = max_y.max(y);
            }
        }
    }
    if max_x <= min_x || max_y <= min_y {
        return Err("no non-white medallion pixels found".into());
    }

    // 3. Apply clean anti-aliased circular trim right along the outer gold rim to remove any anti-alias white fringe.
    let cx = (min_x + max_x) as f32 / 2.0;
    let cy = (min_y + max_y) as f32 / 2.0;
    let diam = ((max_x - min_x + 1).max(max_y - min_y + 1)) as f32;
    let r_cut = diam / 2.0 - 1.5;

    for y in min_y..=max_y {
        for x in min_x..=max_x {
            let dx = x as f32 - cx;
            let dy = y as f32 - cy;
            let dist = (dx * dx + dy * dy).sqrt();
            let p = *rgba.get_pixel(x, y);
            if dist > r_cut + 0.8 {
                rgba.put_pixel(x, y, Rgba([0, 0, 0, 0]));
            } else if dist > r_cut - 1.5 {
                // Feather and decontaminate light fringe pixels on outer 1.5px edge
                let alpha = ((r_cut + 0.8 - dist) / 2.3).clamp(0.0, 1.0);
                let luma = (u16::from(p[0]) + u16::from(p[1]) + u16::from(p[2])) / 3;
                let a = if luma > 225 {
                    (alpha * 0.25 * 255.0).round() as u8
                } else {
                    (alpha * f32::from(p[3])).round().clamp(0.0, 255.0) as u8
                };
                rgba.put_pixel(x, y, Rgba([p[0], p[1], p[2], a]));
            }
        }
    }

    // 4. Center tightly into a square with ~2.5% transparent breathing margin and resize to 512x512.
    let side = (max_x - min_x + 1).max(max_y - min_y + 1);
    let pad = (side as f32 * 0.025).round() as u32;
    let sq_side = side + pad * 2;
    let mut square = ImageBuffer::from_pixel(sq_side, sq_side, Rgba([0, 0, 0, 0]));
    let off_x = (sq_side - (max_x - min_x + 1)) / 2;
    let off_y = (sq_side - (max_y - min_y + 1)) / 2;
    for y in min_y..=max_y {
        for x in min_x..=max_x {
            let p = *rgba.get_pixel(x, y);
            if p[3] > 0 {
                square.put_pixel(off_x + (x - min_x), off_y + (y - min_y), p);
            }
        }
    }

    Ok(DynamicImage::ImageRgba8(square)
        .resize_exact(CANVAS_SIZE, CANVAS_SIZE, FilterType::Lanczos3)
        .to_rgba8())
}

/// Fallback: renders a crisp transparent 512x512 gold-bordered medallion from the portrait asset.
fn build_medallion_from_portrait(portrait_path: &Path) -> Result<RgbaImage, String> {
    let img = image::open(portrait_path)
        .map_err(|e| format!("failed to read portrait {}: {e}", portrait_path.display()))?;
    let (w, h) = img.dimensions();
    let side = w.min(h);
    let cropped = img.crop_imm((w - side) / 2, (h - side) / 2, side, side);

    let inner_diam = 468_u32;
    let scaled_portrait = cropped
        .resize_exact(inner_diam, inner_diam, FilterType::Lanczos3)
        .to_rgba8();

    let mut out: RgbaImage = ImageBuffer::from_pixel(CANVAS_SIZE, CANVAS_SIZE, Rgba([0, 0, 0, 0]));
    let center = (CANVAS_SIZE as f32 - 1.0) / 2.0;
    let r_outer = 246.0_f32;
    let r_gold_outer = 244.0_f32;
    let r_inner = 232.0_f32;
    let half_inner = inner_diam as f32 / 2.0;

    let forest_top = Rgba([15, 46, 40, 255]);
    let forest_bot = Rgba([9, 30, 26, 255]);
    let gold_bright = Rgba([244, 214, 120, 255]);
    let gold_mid = Rgba([212, 175, 55, 255]);
    let gold_deep = Rgba([164, 126, 32, 255]);

    for y in 0..CANVAS_SIZE {
        for x in 0..CANVAS_SIZE {
            let dx = x as f32 - center;
            let dy = y as f32 - center;
            let dist = (dx * dx + dy * dy).sqrt();
            if dist > r_outer + 1.2 {
                continue;
            }

            let aa_outer = ((r_outer + 0.8) - dist).clamp(0.0, 1.0);
            if dist >= r_inner - 0.5 {
                let grad_t = ((dx + dy) / (2.0 * r_gold_outer) + 0.5).clamp(0.0, 1.0);
                let rim_col = if dist >= r_gold_outer - 2.0 {
                    gold_bright
                } else if dist <= r_inner + 2.0 {
                    gold_deep
                } else {
                    Rgba([
                        lerp_u8(gold_bright[0], gold_mid[0], grad_t),
                        lerp_u8(gold_bright[1], gold_mid[1], grad_t),
                        lerp_u8(gold_bright[2], gold_mid[2], grad_t),
                        255,
                    ])
                };
                let alpha = (aa_outer * 255.0).round() as u8;
                out.put_pixel(x, y, Rgba([rim_col[0], rim_col[1], rim_col[2], alpha]));
            } else {
                let t_y = ((dy + r_inner) / (2.0 * r_inner)).clamp(0.0, 1.0);
                let bg = Rgba([
                    lerp_u8(forest_top[0], forest_bot[0], t_y),
                    lerp_u8(forest_top[1], forest_bot[1], t_y),
                    lerp_u8(forest_top[2], forest_bot[2], t_y),
                    255,
                ]);
                let px = (dx + half_inner).round() as i32;
                let py = (dy + half_inner).round() as i32;
                let pixel =
                    if px >= 0 && py >= 0 && (px as u32) < inner_diam && (py as u32) < inner_diam {
                        let src = *scaled_portrait.get_pixel(px as u32, py as u32);
                        let edge_fade = ((r_inner - dist) / 14.0).clamp(0.0, 1.0);
                        if src[3] == 255 {
                            Rgba([
                                lerp_u8(bg[0], src[0], edge_fade),
                                lerp_u8(bg[1], src[1], edge_fade),
                                lerp_u8(bg[2], src[2], edge_fade),
                                255,
                            ])
                        } else {
                            blend_rgba(bg, src)
                        }
                    } else {
                        bg
                    };
                out.put_pixel(x, y, pixel);
            }
        }
    }

    Ok(out)
}

fn square_center_crop(img: DynamicImage) -> RgbaImage {
    let (w, h) = img.dimensions();
    if w == h {
        return img.to_rgba8();
    }
    let side = w.min(h);
    img.crop_imm((w - side) / 2, (h - side) / 2, side, side)
        .to_rgba8()
}

fn run() -> Result<(), String> {
    let root = workspace_root();
    let icons_dir = root.join("packaging/linux/icons");
    let source_logo_path = icons_dir.join("source_logo.png");
    let portrait_default = root.join("crates/kural-app/assets/valluvar-portrait.png");
    let app_logo_path = root.join("crates/kural-app/assets/app-logo.png");

    fs::create_dir_all(&icons_dir)
        .map_err(|e| format!("failed to create {}: {e}", icons_dir.display()))?;

    let mut args = env::args().skip(1);
    let mut from_portrait: Option<PathBuf> = None;
    let mut input_override: Option<PathBuf> = None;
    let mut circle_alpha = false;
    while let Some(arg) = args.next() {
        match arg.as_str() {
            "--from-portrait" => {
                let next = args
                    .next()
                    .map(PathBuf::from)
                    .unwrap_or_else(|| portrait_default.clone());
                from_portrait = Some(next);
            }
            "--input" => {
                let p = args
                    .next()
                    .ok_or_else(|| "missing path after --input".to_string())?;
                input_override = Some(PathBuf::from(p));
            }
            "--circle-alpha" => {
                circle_alpha = true;
            }
            "--help" | "-h" => {
                println!(
                    "Usage: gen-icons [--input LOGO_IMG [--circle-alpha]] [--from-portrait [PORTRAIT_PNG]]\n\
                     Generates 512..32 hicolor PNG icons in packaging/linux/icons/ and crates/kural-app/assets/app-logo.png."
                );
                return Ok(());
            }
            other => return Err(format!("unknown argument: {other}")),
        }
    }

    if let Some(portrait) = from_portrait {
        let medallion = build_medallion_from_portrait(&portrait)?;
        DynamicImage::ImageRgba8(medallion)
            .save(&source_logo_path)
            .map_err(|e| format!("failed writing {}: {e}", source_logo_path.display()))?;
    } else if let Some(custom_input) = input_override {
        let raw = image::open(&custom_input)
            .map_err(|e| format!("failed reading {}: {e}", custom_input.display()))?;
        let processed = if circle_alpha {
            DynamicImage::ImageRgba8(extract_transparent_medallion(raw)?)
        } else {
            let sq = square_center_crop(raw);
            DynamicImage::ImageRgba8(sq).resize_exact(
                CANVAS_SIZE,
                CANVAS_SIZE,
                FilterType::Lanczos3,
            )
        };
        processed
            .save(&source_logo_path)
            .map_err(|e| format!("failed writing {}: {e}", source_logo_path.display()))?;
    } else if !source_logo_path.is_file() {
        let medallion = build_medallion_from_portrait(&portrait_default)?;
        DynamicImage::ImageRgba8(medallion)
            .save(&source_logo_path)
            .map_err(|e| format!("failed writing {}: {e}", source_logo_path.display()))?;
    }

    let source_img = image::open(&source_logo_path)
        .map_err(|e| format!("failed opening {}: {e}", source_logo_path.display()))?;
    let square = DynamicImage::ImageRgba8(square_center_crop(source_img));

    for &sz in HICOLOR_SIZES {
        let out_path = icons_dir.join(format!("{sz}x{sz}.png"));
        let resized = square.resize_exact(sz, sz, FilterType::Lanczos3);
        resized
            .save(&out_path)
            .map_err(|e| format!("failed saving {}: {e}", out_path.display()))?;
        println!("Wrote {}", out_path.display());

        if sz == 128 {
            resized
                .save(&app_logo_path)
                .map_err(|e| format!("failed saving {}: {e}", app_logo_path.display()))?;
            println!("Wrote {}", app_logo_path.display());
        }
    }

    Ok(())
}

fn main() {
    if let Err(err) = run() {
        eprintln!("gen-icons error: {err}");
        std::process::exit(1);
    }
}
