//! Native macOS capture backend.
//!
//! Two paths, per the approved UX (interactive picker + multi-monitor):
//!   * `snip` / `fullWindow` -> the native **interactive picker** via `/usr/sbin/screencapture -i`
//!     (`-iW` starts in window-pick mode). This is Apple's ScreenCaptureKit-backed picker: a dim
//!     overlay across every display, hover-highlights windows, click (or drag a region) to
//!     capture — exactly the requested UX, multi-monitor, with none of the fragility of a custom
//!     overlay. (Deviation from "SCK crate for everything", documented in docs/PHASE1.md.)
//!   * `visible` -> automatic, non-interactive capture of the **display under the cursor** via the
//!     `screencapturekit` crate (SCScreenshotManager) — "just grab the screen I'm on".
//!
//! Both require the Screen Recording TCC permission; the first attempt triggers the system prompt.

use anyhow::anyhow;
use core_graphics::display::CGDisplay;
use core_graphics::event::CGEvent;
use core_graphics::event_source::{CGEventSource, CGEventSourceStateID};
use screencapturekit::cg::{CGPoint as ScPoint, CGRect as ScRect, CGSize as ScSize};
use screencapturekit::screenshot_manager::{CGImageExt, SCScreenshotManager};
use screencapturekit::shareable_content::SCShareableContent;
use screencapturekit::stream::configuration::SCStreamConfiguration;
use screencapturekit::stream::content_filter::SCContentFilter;
use tauri::AppHandle;
use uuid::Uuid;

use super::Shot;

pub fn capture(_app: &AppHandle, mode: &str) -> anyhow::Result<Shot> {
    match mode {
        "fullWindow" => screencapture_interactive(&["-i", "-W"]),
        "snip" => screencapture_interactive(&["-i"]),
        "frontWindow" => capture_front_window(),
        _ => capture_display_under_cursor(), // "visible"
    }
}

/// Backing-scale factor (pixels per point) of a display — 2.0 on Retina. The editor sizes the
/// banner in CSS points and multiplies by this, so text isn't microscopic on Retina captures.
fn backing_scale(display_id: u32) -> f64 {
    CGDisplay::new(display_id)
        .display_mode()
        .map(|m| {
            let points = m.width() as f64;
            if points > 0.0 { m.pixel_width() as f64 / points } else { 1.0 }
        })
        .unwrap_or(1.0)
}

/// Scale of the display the cursor is on — the best available answer for interactive captures,
/// where the picked window/region is where the user last clicked.
fn cursor_display_scale() -> f64 {
    backing_scale(display_id_under_cursor().unwrap_or_else(|| CGDisplay::main().id))
}

/// Native interactive picker. Blocks until the user picks (or cancels with Esc). Returns the
/// captured PNG decoded to RGBA. Cancel → error (caller skips opening the editor).
fn screencapture_interactive(flags: &[&str]) -> anyhow::Result<Shot> {
    let tmp = std::env::temp_dir().join(format!("glyphio-capture-{}.png", Uuid::new_v4()));
    let status = std::process::Command::new("/usr/sbin/screencapture")
        .args(flags)
        .arg("-o") // no window shadow
        .arg(&tmp)
        .status()
        .map_err(|e| anyhow!("failed to launch screencapture: {e}"))?;

    if !status.success() || !tmp.exists() {
        anyhow::bail!("capture cancelled");
    }
    let bytes = std::fs::read(&tmp)?;
    let _ = std::fs::remove_file(&tmp);
    let img = image::load_from_memory(&bytes)
        .map_err(|e| anyhow!("decoding capture failed: {e}"))?
        .to_rgba8();
    let (width, height) = img.dimensions();
    Ok(Shot { rgba: img.into_raw(), width, height, dpr: cursor_display_scale(), title: String::new() })
}

/// SCK capture of whichever display currently contains the mouse cursor (multi-monitor aware).
fn capture_display_under_cursor() -> anyhow::Result<Shot> {
    let target_id = display_id_under_cursor().unwrap_or_else(|| CGDisplay::main().id);
    let dpr = backing_scale(target_id);

    let content = SCShareableContent::get().map_err(|e| {
        anyhow!("screen capture unavailable — grant Screen Recording permission in System Settings > Privacy & Security, then retry ({e:?})")
    })?;
    let displays = content.displays();
    let display = displays
        .iter()
        .find(|d| d.display_id() == target_id)
        .or_else(|| displays.first())
        .ok_or_else(|| anyhow!("no display found"))?;

    let filter = SCContentFilter::create()
        .with_display(display)
        .with_excluding_windows(&[])
        .build();
    // SCDisplay dimensions are in points; request pixel dimensions or Retina captures come out
    // downscaled (blurry) at 1x. Cursor excluded, matching the system screenshot default.
    let config = SCStreamConfiguration::new()
        .with_width((display.width() as f64 * dpr) as u32)
        .with_height((display.height() as f64 * dpr) as u32)
        .with_shows_cursor(false);
    let image = SCScreenshotManager::capture_image(&filter, &config)
        .map_err(|e| anyhow!("capture failed: {e:?}"))?;
    let width = image.width() as u32;
    let height = image.height() as u32;
    let rgba = image
        .rgba_data()
        .map_err(|e| anyhow!("reading captured pixels failed: {e:?}"))?;
    Ok(Shot { rgba, width, height, dpr, title: String::new() })
}

/// Non-interactive capture of the frontmost window (whole window, chrome included) — one
/// keystroke, no picker. The motivating case: grab just the browser window instead of the
/// entire screen.
fn capture_front_window() -> anyhow::Result<Shot> {
    let win = frontmost_window_bounds_with_inset(0.0)?;
    let (img, dpr) = capture_rect_image(win.x, win.y, win.w, win.h)?;
    let (width, height) = img.dimensions();
    Ok(Shot { rgba: img.into_raw(), width, height, dpr, title: win.title })
}

/// Capture an arbitrary global rect (points, top-left origin of the main display) and return
/// the pixels plus the effective pixels-per-point scale.
///
/// Deliberately built on `SCContentFilter` + `SCStreamConfiguration.sourceRect` — the same
/// machinery as display capture — and NOT on `SCScreenshotManager.captureImage(in:)`: that
/// convenience composites a virtual-desktop rect and, on multi-display setups, fills whatever
/// it fails to map with black (the "scrolling capture comes out black" bug). The rect is
/// clamped to the display it overlaps most; a filter can't span displays.
pub(super) fn capture_rect_image(
    x: f64,
    y: f64,
    w: f64,
    h: f64,
) -> anyhow::Result<(image::RgbaImage, f64)> {
    let content = SCShareableContent::get().map_err(|e| {
        anyhow!("screen capture unavailable — grant Screen Recording permission in System Settings > Privacy & Security, then retry ({e:?})")
    })?;
    let displays = content.displays();

    let overlap_area = |id: u32| -> f64 {
        let b = CGDisplay::new(id).bounds();
        let ow = (x + w).min(b.origin.x + b.size.width) - x.max(b.origin.x);
        let oh = (y + h).min(b.origin.y + b.size.height) - y.max(b.origin.y);
        if ow > 0.0 && oh > 0.0 { ow * oh } else { 0.0 }
    };
    let display = displays
        .iter()
        .max_by(|a, b| {
            overlap_area(a.display_id())
                .total_cmp(&overlap_area(b.display_id()))
        })
        .filter(|d| overlap_area(d.display_id()) > 0.0)
        .ok_or_else(|| anyhow!("selection is outside every display"))?;

    let b = CGDisplay::new(display.display_id()).bounds();
    let cx = x.max(b.origin.x);
    let cy = y.max(b.origin.y);
    let cw = (x + w).min(b.origin.x + b.size.width) - cx;
    let ch = (y + h).min(b.origin.y + b.size.height) - cy;
    if cw < 1.0 || ch < 1.0 {
        anyhow::bail!("selection is outside every display");
    }

    let scale = backing_scale(display.display_id());
    let filter = SCContentFilter::create()
        .with_display(display)
        .with_excluding_windows(&[])
        .build();
    // sourceRect is in the display's own top-left-origin point space; width/height request
    // pixel output so Retina captures aren't downscaled.
    let config = SCStreamConfiguration::new()
        .with_width((cw * scale).round() as u32)
        .with_height((ch * scale).round() as u32)
        .with_source_rect(ScRect {
            origin: ScPoint { x: cx - b.origin.x, y: cy - b.origin.y },
            size: ScSize { width: cw, height: ch },
        })
        .with_shows_cursor(false);
    let image = SCScreenshotManager::capture_image(&filter, &config)
        .map_err(|e| anyhow!("capture failed: {e:?} — is Screen Recording granted?"))?;
    let (pw, ph) = (image.width() as u32, image.height() as u32);
    let rgba = image
        .rgba_data()
        .map_err(|e| anyhow!("reading captured pixels failed: {e:?}"))?;
    let img = image::RgbaImage::from_raw(pw, ph, rgba)
        .ok_or_else(|| anyhow!("capture buffer size mismatch"))?;
    Ok((img, pw as f64 / cw))
}

/// Frontmost normal window not owned by Glyphio — the target of "scrolling page", "front
/// window", and "browser page" capture.
pub(super) struct FrontWindow {
    pub x: f64,
    pub y: f64,
    pub w: f64,
    pub h: f64,
    pub title: String,
    /// Owning process — lets the AX layer find the window's web content area.
    pub pid: i32,
}

/// Bounds (points, global top-left), title, and pid of the frontmost window. Scrolling capture
/// insets the title bar away (`TITLE_BAR_PT`) so window chrome doesn't repeat in every
/// stitched frame; front-window capture wants the whole window and passes 0.
pub(super) fn frontmost_window_bounds() -> anyhow::Result<FrontWindow> {
    const TITLE_BAR_PT: f64 = 28.0;
    frontmost_window_bounds_with_inset(TITLE_BAR_PT)
}

fn frontmost_window_bounds_with_inset(title_bar_inset: f64) -> anyhow::Result<FrontWindow> {
    use core_foundation::base::{CFType, TCFType};
    use core_foundation::dictionary::CFDictionary;
    use core_foundation::number::CFNumber;
    use core_foundation::string::CFString;
    use core_graphics::window::{
        copy_window_info, kCGWindowListExcludeDesktopElements, kCGWindowListOptionOnScreenOnly,
    };

    let list = copy_window_info(
        kCGWindowListOptionOnScreenOnly | kCGWindowListExcludeDesktopElements,
        0, // kCGNullWindowID
    )
    .ok_or_else(|| anyhow!("could not list windows — is Screen Recording granted?"))?;

    let get_num = |d: &CFDictionary<CFString, CFType>, k: &str| -> Option<f64> {
        d.find(CFString::new(k))
            .and_then(|v| v.downcast::<CFNumber>())
            .and_then(|n| n.to_f64())
    };

    for item in list.iter() {
        let dict: CFDictionary<CFString, CFType> =
            unsafe { CFDictionary::wrap_under_get_rule(*item as _) };
        let layer = dict
            .find(CFString::from_static_string("kCGWindowLayer"))
            .and_then(|v| v.downcast::<CFNumber>())
            .and_then(|n| n.to_i64())
            .unwrap_or(-1);
        if layer != 0 {
            continue; // not a normal app window
        }
        let owner = dict
            .find(CFString::from_static_string("kCGWindowOwnerName"))
            .and_then(|v| v.downcast::<CFString>())
            .map(|s| s.to_string())
            .unwrap_or_default();
        if owner.to_lowercase().contains("glyphio") {
            continue;
        }
        let bounds = dict
            .find(CFString::from_static_string("kCGWindowBounds"))
            .and_then(|v| v.downcast::<CFDictionary>())
            .ok_or_else(|| anyhow!("window has no bounds"))?;
        let bounds: CFDictionary<CFString, CFType> =
            unsafe { CFDictionary::wrap_under_get_rule(bounds.as_concrete_TypeRef() as _) };
        let (x, y, w, h) = (
            get_num(&bounds, "X").unwrap_or(0.0),
            get_num(&bounds, "Y").unwrap_or(0.0),
            get_num(&bounds, "Width").unwrap_or(0.0),
            get_num(&bounds, "Height").unwrap_or(0.0),
        );
        if w < 80.0 || h < 80.0 {
            continue; // panels/popovers
        }
        let title = dict
            .find(CFString::from_static_string("kCGWindowName"))
            .and_then(|v| v.downcast::<CFString>())
            .map(|s| s.to_string())
            .filter(|s| !s.is_empty())
            .unwrap_or(owner);
        let pid = dict
            .find(CFString::from_static_string("kCGWindowOwnerPID"))
            .and_then(|v| v.downcast::<CFNumber>())
            .and_then(|n| n.to_i64())
            .unwrap_or(0) as i32;
        return Ok(FrontWindow {
            x,
            y: y + title_bar_inset,
            w,
            h: h - title_bar_inset,
            title,
            pid,
        });
    }
    anyhow::bail!("no capturable window found")
}

/// (origin, size) in points of the display under the cursor (top-left origin, global coords).
pub(super) fn display_bounds_under_cursor() -> ((f64, f64), (f64, f64)) {
    let id = display_id_under_cursor().unwrap_or_else(|| CGDisplay::main().id);
    let b = CGDisplay::new(id).bounds();
    ((b.origin.x, b.origin.y), (b.size.width, b.size.height))
}

/// The `CGDirectDisplayID` of the display containing the current cursor position.
fn display_id_under_cursor() -> Option<u32> {
    let source = CGEventSource::new(CGEventSourceStateID::CombinedSessionState).ok()?;
    let point = CGEvent::new(source).ok()?.location();
    for id in CGDisplay::active_displays().ok()? {
        let b = CGDisplay::new(id).bounds();
        if point.x >= b.origin.x
            && point.x < b.origin.x + b.size.width
            && point.y >= b.origin.y
            && point.y < b.origin.y + b.size.height
        {
            return Some(id);
        }
    }
    None
}
