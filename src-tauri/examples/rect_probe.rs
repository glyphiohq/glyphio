//! Diagnostic: probe SCScreenshotManager rect captures on every display.
//! Run: cargo run --example rect_probe
//! Writes PNGs + a black-fraction report to the path in GLYPHIO_PROBE_OUT (or /tmp).

use core_graphics::display::CGDisplay;
use screencapturekit::cg::{CGPoint as ScPoint, CGRect as ScRect, CGSize as ScSize};
use screencapturekit::screenshot_manager::{CGImageExt, SCScreenshotManager};
use screencapturekit::shareable_content::SCShareableContent;
use screencapturekit::stream::configuration::SCStreamConfiguration;
use screencapturekit::stream::content_filter::SCContentFilter;

fn black_fraction(rgba: &[u8]) -> f64 {
    let mut black = 0usize;
    let total = rgba.len() / 4;
    for px in rgba.chunks_exact(4) {
        if px[0] < 8 && px[1] < 8 && px[2] < 8 {
            black += 1;
        }
    }
    black as f64 / total.max(1) as f64
}

fn save(name: &str, w: u32, h: u32, rgba: Vec<u8>, out: &std::path::Path) {
    let frac = black_fraction(&rgba);
    println!("{name}: {w}x{h}  black={:.1}%", frac * 100.0);
    if let Some(img) = image::RgbaImage::from_raw(w, h, rgba) {
        let p = out.join(format!("{name}.png"));
        let _ = img.save(&p);
    }
}

fn main() {
    let out = std::env::var("GLYPHIO_PROBE_OUT")
        .map(std::path::PathBuf::from)
        .unwrap_or_else(|_| std::env::temp_dir());
    std::fs::create_dir_all(&out).ok();
    println!("output dir: {}", out.display());

    let content = match SCShareableContent::get() {
        Ok(c) => c,
        Err(e) => {
            eprintln!("SCShareableContent failed (Screen Recording?): {e:?}");
            std::process::exit(2);
        }
    };

    for (i, d) in content.displays().iter().enumerate() {
        let b = CGDisplay::new(d.display_id()).bounds();
        println!(
            "display[{i}] id={} global=({}, {}) {}x{}",
            d.display_id(),
            b.origin.x,
            b.origin.y,
            b.size.width,
            b.size.height
        );

        // Path A: filter+config full-display capture (known-good baseline).
        let filter = SCContentFilter::create()
            .with_display(d)
            .with_excluding_windows(&[])
            .build();
        let config = SCStreamConfiguration::new()
            .with_width(d.width())
            .with_height(d.height());
        match SCScreenshotManager::capture_image(&filter, &config) {
            Ok(img) => {
                let (w, h) = (img.width() as u32, img.height() as u32);
                match img.rgba_data() {
                    Ok(rgba) => save(&format!("display{i}_filter"), w, h, rgba, &out),
                    Err(e) => println!("display{i}_filter: rgba_data failed: {e:?}"),
                }
            }
            Err(e) => println!("display{i}_filter: capture failed: {e:?}"),
        }

        // Path B (OLD, buggy on multi-display): capture_image_in_rect convenience API.
        let rect = ScRect {
            origin: ScPoint { x: b.origin.x + 200.0, y: b.origin.y + 150.0 },
            size: ScSize { width: 600.0, height: 400.0 },
        };
        match SCScreenshotManager::capture_image_in_rect(rect) {
            Ok(img) => {
                let (w, h) = (img.width() as u32, img.height() as u32);
                match img.rgba_data() {
                    Ok(rgba) => save(&format!("display{i}_rect_old"), w, h, rgba, &out),
                    Err(e) => println!("display{i}_rect_old: rgba_data failed: {e:?}"),
                }
            }
            Err(e) => println!("display{i}_rect_old: capture failed: {e:?}"),
        }

        // Path C (NEW, what the app now uses): filter + sourceRect on the same rect.
        let filter2 = SCContentFilter::create()
            .with_display(d)
            .with_excluding_windows(&[])
            .build();
        let config2 = SCStreamConfiguration::new()
            .with_width(600)
            .with_height(400)
            .with_source_rect(ScRect {
                origin: ScPoint { x: 200.0, y: 150.0 },
                size: ScSize { width: 600.0, height: 400.0 },
            });
        match SCScreenshotManager::capture_image(&filter2, &config2) {
            Ok(img) => {
                let (w, h) = (img.width() as u32, img.height() as u32);
                match img.rgba_data() {
                    Ok(rgba) => save(&format!("display{i}_rect_new"), w, h, rgba, &out),
                    Err(e) => println!("display{i}_rect_new: rgba_data failed: {e:?}"),
                }
            }
            Err(e) => println!("display{i}_rect_new: capture failed: {e:?}"),
        }
    }
}
