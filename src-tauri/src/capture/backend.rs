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
    // downscaled (blurry) at 1x.
    let config = SCStreamConfiguration::new()
        .with_width((display.width() as f64 * dpr) as u32)
        .with_height((display.height() as f64 * dpr) as u32);
    let image = SCScreenshotManager::capture_image(&filter, &config)
        .map_err(|e| anyhow!("capture failed: {e:?}"))?;
    let width = image.width() as u32;
    let height = image.height() as u32;
    let rgba = image
        .rgba_data()
        .map_err(|e| anyhow!("reading captured pixels failed: {e:?}"))?;
    Ok(Shot { rgba, width, height, dpr, title: String::new() })
}

/// Bounds (points, global top-left) and title of the frontmost normal window not owned by
/// Glyphio — the target of "scrolling page" capture. The title bar is inset away so window
/// chrome doesn't repeat in every stitched frame.
pub(super) fn frontmost_window_bounds() -> anyhow::Result<(f64, f64, f64, f64, String)> {
    use core_foundation::base::{CFType, TCFType};
    use core_foundation::dictionary::CFDictionary;
    use core_foundation::number::CFNumber;
    use core_foundation::string::CFString;
    use core_graphics::window::{
        copy_window_info, kCGWindowListExcludeDesktopElements, kCGWindowListOptionOnScreenOnly,
    };

    const TITLE_BAR_PT: f64 = 28.0;
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
        return Ok((x, y + TITLE_BAR_PT, w, h - TITLE_BAR_PT, title));
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
