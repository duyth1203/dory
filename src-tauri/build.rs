use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;

use image::imageops::{crop_imm, overlay, resize, FilterType};
use image::{Rgba, RgbaImage};

/// Bump when dock optical / sizing / mask changes (forces one `tauri icon` refresh for everyone).
const DOCK_ICON_PIPELINE_ID: &str = "dock-optical-v6-inset-64-boost-0.95";
const DOCK_INTERMEDIATE_PNG: &str = "icons/_dock_source_1024.png";

const DOCK_CANVAS_PX: u32 = 1024;
/// Inner margin inside the 1024 tile (each side), before the squircle mask.
const DOCK_INSET_PX: u32 = 64;
/// Scale factor for the trimmed artwork within the inset box (`1.0` = tight fit).
const DOCK_OPTICAL_BOOST: f64 = 0.95;

fn main() {
    tauri_build::build();

    let manifest_dir = PathBuf::from(std::env::var("CARGO_MANIFEST_DIR").unwrap());
    println!("cargo:rerun-if-changed=icon-source.png");
    println!("cargo:rerun-if-changed=build.rs");

    if std::env::var("SKIP_TAURI_BUNDLE_ICON_REGEN").is_ok() {
        println!("cargo:warning=SKIP_TAURI_BUNDLE_ICON_REGEN set: not refreshing Dock/.icns assets");
        return;
    }

    if !bundle_icons_need_regen(&manifest_dir) {
        return;
    }

    let icon_path = manifest_dir.join("icon-source.png");
    let img = match image::open(&icon_path) {
        Ok(img) => img.into_rgba8(),
        Err(e) => panic!(
            "app icon: could not open {}: {}",
            icon_path.display(),
            e
        ),
    };

    let mut dock = fill_square_trim_boost(
        &img,
        DOCK_CANVAS_PX,
        DOCK_INSET_PX,
        DOCK_OPTICAL_BOOST,
    );
    apply_macos_icon_corner_mask(&mut dock);
    let bridge = manifest_dir.join(DOCK_INTERMEDIATE_PNG);
    dock.save(&bridge).unwrap_or_else(|e| {
        panic!("dock icon intermediate: write {}: {e}", bridge.display());
    });

    let repo_root = manifest_dir
        .parent()
        .expect("CARGO_MANIFEST_DIR should have a parent (project root)");

    let rel_bridge = bridge
        .strip_prefix(repo_root)
        .unwrap()
        .to_string_lossy()
        .to_string();

    let st = Command::new("pnpm")
        .current_dir(repo_root)
        .args(["exec", "tauri", "icon", &rel_bridge])
        .status()
        .unwrap_or_else(|e| panic!("failed to run `pnpm exec tauri icon`: {e}"));

    if !st.success() {
        panic!(
            "`pnpm exec tauri icon` failed with {}. \
             Fix the error or set SKIP_TAURI_BUNDLE_ICON_REGEN=1 to skip Dock icon refresh.",
            st
        );
    }

    let ui_icon = repo_root.join("src/assets/icon.png");
    if let Err(e) = fs::copy(&bridge, &ui_icon) {
        println!(
            "cargo:warning=could not sync in-app icon to {}: {e}",
            ui_icon.display()
        );
    }

    let marker = manifest_dir.join("icons/.dock_pipeline");
    fs::write(&marker, DOCK_ICON_PIPELINE_ID).unwrap_or_else(|e| {
        panic!("write {}: {e}", marker.display());
    });
}

fn bundle_icons_need_regen(manifest_dir: &Path) -> bool {
    let marker = manifest_dir.join("icons/.dock_pipeline");
    if fs::read_to_string(&marker)
        .map(|s| s.trim() != DOCK_ICON_PIPELINE_ID)
        .unwrap_or(true)
    {
        return true;
    }

    let source = manifest_dir.join("icon-source.png");
    let icns = manifest_dir.join("icons/icon.icns");

    let Ok(src_meta) = fs::metadata(&source) else {
        return false;
    };
    let Ok(icns_meta) = fs::metadata(&icns) else {
        return true;
    };
    let Ok(st) = src_meta.modified() else {
        return true;
    };
    let Ok(it) = icns_meta.modified() else {
        return true;
    };
    st > it
}

/// Tight crop on non-transparent pixels, scale with optical boost, then center-crop or center-paste.
fn fill_square_trim_boost(
    src: &RgbaImage,
    canvas_px: u32,
    inset_px: u32,
    optical_boost: f64,
) -> RgbaImage {
    let alpha_threshold: u8 = 8;
    let (w, h) = src.dimensions();

    let mut min_x = w;
    let mut min_y = h;
    let mut max_x = 0_u32;
    let mut max_y = 0_u32;
    let mut any = false;

    for y in 0..h {
        for x in 0..w {
            if src.get_pixel(x, y).0[3] > alpha_threshold {
                any = true;
                min_x = min_x.min(x);
                min_y = min_y.min(y);
                max_x = max_x.max(x);
                max_y = max_y.max(y);
            }
        }
    }

    let cropped = if any && min_x <= max_x {
        let cw = max_x - min_x + 1;
        let ch = max_y - min_y + 1;
        crop_imm(src, min_x, min_y, cw, ch).to_image()
    } else {
        src.clone()
    };

    let (cw, ch) = cropped.dimensions();
    if cw == 0 || ch == 0 {
        return RgbaImage::from_fn(canvas_px, canvas_px, |_, _| Rgba([0, 0, 0, 0]));
    }

    let inner_max = canvas_px.saturating_sub(2 * inset_px).max(1);
    let scale = f64::from(inner_max) / f64::from(cw.max(ch)) * optical_boost;
    let nw = ((f64::from(cw) * scale).round() as u32).max(1);
    let nh = ((f64::from(ch) * scale).round() as u32).max(1);

    let scaled = resize(&cropped, nw, nh, FilterType::Lanczos3);

    let tw = canvas_px;
    let th = canvas_px;

    if scaled.width() >= tw && scaled.height() >= th {
        let cx = (scaled.width() - tw) / 2;
        let cy = (scaled.height() - th) / 2;
        return crop_imm(&scaled, cx, cy, tw, th).to_image();
    }

    let mut canvas = RgbaImage::from_fn(canvas_px, canvas_px, |_, _| Rgba([0, 0, 0, 0]));

    let ox = inset_px.saturating_add(inner_max.saturating_sub(scaled.width()) / 2);
    let oy = inset_px.saturating_add(inner_max.saturating_sub(scaled.height()) / 2);
    overlay(&mut canvas, &scaled, i64::from(ox), i64::from(oy));

    canvas
}

/// macOS applies a squircle to app icons, but a fully opaque 1024 square reads as a sharp tile.
/// Knock out corner alpha with ~22.3% corner radius (HIG rule of thumb) so the Dock matches peers.
fn apply_macos_icon_corner_mask(img: &mut RgbaImage) {
    let w = img.width() as f32;
    let h = img.height() as f32;
    if w <= 1.0 || h <= 1.0 || (w - h).abs() > 0.01 {
        return;
    }

    let r = (w * 0.223_f32).clamp(1.0, w / 2.0 - 1.0);
    let cx = w / 2.0;
    let cy = h / 2.0;
    let hx = w / 2.0 - r;
    let hy = h / 2.0 - r;
    let aa = 1.5_f32;

    for y in 0..img.height() {
        for x in 0..img.width() {
            let px = x as f32 + 0.5 - cx;
            let py = y as f32 + 0.5 - cy;
            let qx = px.abs() - hx;
            let qy = py.abs() - hy;
            let ax = qx.max(0.0);
            let ay = qy.max(0.0);
            let outside = (ax * ax + ay * ay).sqrt();
            let inside = qx.min(qy).min(0.0);
            let d = outside + inside - r;

            let m = 1.0 - smoothstep(-aa, aa, d);
            let p = img.get_pixel_mut(x, y);
            p.0[3] = ((p.0[3] as f32) * m).round().clamp(0.0, 255.0) as u8;
        }
    }
}

#[inline]
fn smoothstep(edge0: f32, edge1: f32, x: f32) -> f32 {
    let t = ((x - edge0) / (edge1 - edge0)).clamp(0.0, 1.0);
    t * t * (3.0 - 2.0 * t)
}
