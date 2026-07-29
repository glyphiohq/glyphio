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
use screencapturekit::shareable_content::{SCDisplay, SCShareableContent, SCWindow};
use screencapturekit::stream::configuration::SCStreamConfiguration;
use screencapturekit::stream::content_filter::SCContentFilter;
use tauri::AppHandle;
use uuid::Uuid;

use super::Shot;

pub fn capture(app: &AppHandle, mode: &str) -> anyhow::Result<Shot> {
    match mode {
        "fullWindow" => screencapture_interactive(&["-i", "-W"]),
        "snip" => screencapture_interactive(&["-i"]),
        "frontWindow" => capture_front_window(super::wants_browser_details(app)),
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
    Ok(Shot {
        rgba: img.into_raw(),
        width,
        height,
        dpr: cursor_display_scale(),
        title: String::new(),
        browser: Default::default(),
    })
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

    let windows = content.windows();
    let filter = SCContentFilter::create()
        .with_display(display)
        .with_excluding_windows(&our_windows(&windows))
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
    Ok(Shot { rgba, width, height, dpr, title: String::new(), browser: Default::default() })
}

/// Non-interactive capture of the frontmost window (whole window, chrome included) — one
/// keystroke, no picker. The motivating case: grab just the browser window instead of the
/// entire screen.
fn capture_front_window(with_browser_details: bool) -> anyhow::Result<Shot> {
    let win = frontmost_window_bounds_with_inset(0.0)?;
    let browser = if with_browser_details { win.browser_meta() } else { Default::default() };
    let (img, dpr) = capture_rect_image(win.x, win.y, win.w, win.h)?;
    let (width, height) = img.dimensions();
    Ok(Shot { rgba: img.into_raw(), width, height, dpr, title: win.title, browser })
}

/// Capture an arbitrary global rect (points, top-left origin of the main display) and return
/// the pixels plus the effective pixels-per-point scale.
///
/// Deliberately built on `SCContentFilter` + `SCStreamConfiguration.sourceRect` — the same
/// machinery as display capture — and NOT on `SCScreenshotManager.captureImage(in:)`: that
/// convenience composites a virtual-desktop rect and, on multi-display setups, fills whatever
/// it fails to map with black (the "scrolling capture comes out black" bug).
///
/// One display and the part of a requested rect that falls on it, as (x, y, w, h).
type DisplayPiece<'a> = (&'a SCDisplay, (f64, f64, f64, f64));

/// A content filter can only describe one display, so a rect straddling two monitors — a
/// window dragged across the seam, which is ordinary on a multi-monitor desk — is captured
/// per display and composited here. The result covers the part of the rect that is actually
/// on some display; anything hanging off the edge of the desktop is trimmed rather than
/// padded. Mixed-DPI arrangements resolve to the sharpest scale involved, upscaling the
/// coarser piece so the seam lines up.
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
    let windows = content.windows();
    let ours = our_windows(&windows);

    // (display, intersection with the requested rect), sharpest display first so the output
    // scale is settled before anything is captured.
    let mut pieces: Vec<DisplayPiece<'_>> = displays
        .iter()
        .filter_map(|d| {
            let b = CGDisplay::new(d.display_id()).bounds();
            let bounds = (b.origin.x, b.origin.y, b.size.width, b.size.height);
            intersect((x, y, w, h), bounds).map(|r| (d, r))
        })
        .collect();
    if pieces.is_empty() {
        anyhow::bail!("selection is outside every display");
    }
    pieces.sort_by(|a, b| {
        backing_scale(b.0.display_id()).total_cmp(&backing_scale(a.0.display_id()))
    });
    let scale = backing_scale(pieces[0].0.display_id());

    if pieces.len() == 1 {
        let (d, (cx, cy, cw, ch)) = pieces[0];
        let img = capture_display_rect(d, &ours, cx, cy, cw, ch, scale)?;
        let pw = img.width();
        return Ok((img, pw as f64 / cw));
    }

    // Union of the covered pieces: the rect minus whatever falls off the desktop.
    let (ux, uy) = (
        pieces.iter().map(|p| p.1 .0).fold(f64::MAX, f64::min),
        pieces.iter().map(|p| p.1 .1).fold(f64::MAX, f64::min),
    );
    let (ux2, uy2) = (
        pieces.iter().map(|p| p.1 .0 + p.1 .2).fold(f64::MIN, f64::max),
        pieces.iter().map(|p| p.1 .1 + p.1 .3).fold(f64::MIN, f64::max),
    );
    let mut out = image::RgbaImage::new(
        ((ux2 - ux) * scale).round().max(1.0) as u32,
        ((uy2 - uy) * scale).round().max(1.0) as u32,
    );
    let spanned = pieces.len();
    for (d, (cx, cy, cw, ch)) in pieces {
        let piece = capture_display_rect(d, &ours, cx, cy, cw, ch, scale)?;
        image::imageops::overlay(
            &mut out,
            &piece,
            ((cx - ux) * scale).round() as i64,
            ((cy - uy) * scale).round() as i64,
        );
    }
    log::info!(
        "capture rect spanned {spanned} displays -> {}x{} @{scale}x",
        out.width(),
        out.height()
    );
    Ok((out, scale))
}

/// One display's share of a rect, rendered at `scale` pixels per point.
fn capture_display_rect(
    display: &SCDisplay,
    exclude: &[&SCWindow],
    cx: f64,
    cy: f64,
    cw: f64,
    ch: f64,
    scale: f64,
) -> anyhow::Result<image::RgbaImage> {
    let b = CGDisplay::new(display.display_id()).bounds();
    let filter = SCContentFilter::create()
        .with_display(display)
        .with_excluding_windows(exclude)
        .build();
    // sourceRect is in the display's own top-left-origin point space; width/height request
    // pixel output so Retina captures aren't downscaled — and, when displays disagree about
    // scale, pull the coarser one up to the output scale instead of stitching a size mismatch.
    let config = SCStreamConfiguration::new()
        .with_width((cw * scale).round().max(1.0) as u32)
        .with_height((ch * scale).round().max(1.0) as u32)
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
    image::RgbaImage::from_raw(pw, ph, rgba).ok_or_else(|| anyhow!("capture buffer size mismatch"))
}

/// Glyphio's own windows, to be left out of the shot. An editor window from the previous
/// capture, or the palette, sitting over the target is not part of what the user is
/// photographing — and worse, in a scrolling capture it swallows the scroll events aimed at
/// the page underneath, so the "capture" is one frame of Glyphio's own UI.
fn our_windows(windows: &[SCWindow]) -> Vec<&SCWindow> {
    let us = std::process::id() as i32;
    windows
        .iter()
        .filter(|w| w.owning_application().is_some_and(|app| app.process_id() == us))
        .collect()
}

/// Overlap of two rects `(x, y, w, h)`, or `None` when they don't meaningfully meet.
fn intersect(a: (f64, f64, f64, f64), b: (f64, f64, f64, f64)) -> Option<(f64, f64, f64, f64)> {
    let x = a.0.max(b.0);
    let y = a.1.max(b.1);
    let w = (a.0 + a.2).min(b.0 + b.2) - x;
    let h = (a.1 + a.3).min(b.1 + b.3) - y;
    (w >= 1.0 && h >= 1.0).then_some((x, y, w, h))
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
    /// The owning application's name as macOS presents it ("Google Chrome"). Browsers build
    /// their window title around it, which is how the page title and profile come apart.
    pub app_name: String,
}

impl FrontWindow {
    /// What the browser says about the page in this window — empty for anything that isn't
    /// showing one. See [`super::ax::browser_meta`]: nothing is switched on to find out.
    pub fn browser_meta(&self) -> super::ax::BrowserMeta {
        super::ax::browser_meta(self.pid, &self.app_name)
    }
}

/// Bounds (points, global top-left), title, and pid of the frontmost window. Scrolling capture
/// insets the title bar away (`TITLE_BAR_PT`) so window chrome doesn't repeat in every
/// stitched frame; front-window capture wants the whole window and passes 0.
pub(super) fn frontmost_window_bounds() -> anyhow::Result<FrontWindow> {
    const TITLE_BAR_PT: f64 = 28.0;
    frontmost_window_bounds_with_inset(TITLE_BAR_PT)
}

/// Which app holds keyboard focus is asked of macOS itself, and which of its windows is asked
/// of the accessibility API — that is the question the user is really asking, and it has one
/// answer however many displays the windows are spread over. Reading the window list in
/// z-order is only a stand-in: it flattens every visible display into one front-to-back
/// ordering, so a window last touched on another monitor can outrank the one being looked at.
///
/// The catch is that accessibility describes windows on *other Spaces* just as happily as the
/// one on screen, and captures are pixels: aiming at a rect whose window is on another desktop
/// would photograph whatever happens to be sitting at those coordinates instead. So the
/// answer only stands if the window list — which is on-screen only — confirms the window is
/// where AX says it is.
fn frontmost_window_bounds_with_inset(title_bar_inset: f64) -> anyhow::Result<FrontWindow> {
    let onscreen = on_screen_windows();
    let focused = frontmost_app()
        .filter(|(pid, _)| *pid != std::process::id() as i32)
        .and_then(|(pid, name)| {
            super::ax::focused_window(pid).map(|(frame, title)| (pid, name, frame, title))
        });

    if let Some((pid, app_name, frame, ax_title)) = focused {
        if let Some(listed) = onscreen.iter().find(|w| w.pid == pid && w.is_at(frame)) {
            // AX has the better title for most apps but none for some, so take whichever is
            // non-empty, preferring AX: `kCGWindowName` comes back empty for full-screen
            // browser windows, which is how captures ended up labelled just "Safari".
            let title = if ax_title.trim().is_empty() { listed.title.clone() } else { ax_title };
            let (x, y, w, h) = frame;
            return Ok(FrontWindow {
                x,
                y: y + title_bar_inset,
                w,
                h: h - title_bar_inset,
                title,
                pid,
                app_name,
            });
        }
    }
    let first = onscreen.into_iter().next().ok_or_else(|| anyhow!("no capturable window found"))?;
    Ok(FrontWindow {
        x: first.x,
        y: first.y + title_bar_inset,
        w: first.w,
        h: first.h - title_bar_inset,
        title: first.title,
        pid: first.pid,
        app_name: first.owner,
    })
}

/// A visible, normal-sized window belonging to someone other than us.
struct ListedWindow {
    pid: i32,
    x: f64,
    y: f64,
    w: f64,
    h: f64,
    title: String,
    /// The owning application's name, which is not the window title even when the window
    /// list falls back to it.
    owner: String,
}

impl ListedWindow {
    /// Whether this is the window an accessibility frame describes. Tolerant by a couple of
    /// points: the two APIs round independently.
    fn is_at(&self, (x, y, w, h): (f64, f64, f64, f64)) -> bool {
        const SLACK: f64 = 2.0;
        (self.x - x).abs() <= SLACK
            && (self.y - y).abs() <= SLACK
            && (self.w - w).abs() <= SLACK
            && (self.h - h).abs() <= SLACK
    }
}

/// The active application — its pid and the name macOS shows for it. `NSWorkspace` is the
/// only source that answers this correctly on a multi-display desk; the system-wide
/// accessibility element's `AXFocusedApplication` returns `kAXErrorCannotComplete` on current
/// macOS even for a trusted process, so it is only a backstop (and knows no name).
pub(super) fn frontmost_app() -> Option<(i32, String)> {
    let front = objc2_app_kit::NSWorkspace::sharedWorkspace().frontmostApplication();
    if let Some(app) = front {
        let pid = app.processIdentifier();
        if pid > 0 {
            let name = app.localizedName().map(|n| n.to_string()).unwrap_or_default();
            return Some((pid, name));
        }
    }
    super::ax::focused_app_pid().map(|pid| (pid, String::new()))
}

/// Every normal, non-Glyphio window currently on screen, front to back. "On screen" is the
/// point: windows on other Spaces are absent, which is what makes this the check that a
/// window is really in front of the user rather than merely focused.
fn on_screen_windows() -> Vec<ListedWindow> {
    use core_foundation::base::{CFType, TCFType};
    use core_foundation::dictionary::CFDictionary;
    use core_foundation::number::CFNumber;
    use core_foundation::string::CFString;
    use core_graphics::window::{
        copy_window_info, kCGWindowListExcludeDesktopElements, kCGWindowListOptionOnScreenOnly,
    };

    let mut out = Vec::new();
    let Some(list) = copy_window_info(
        kCGWindowListOptionOnScreenOnly | kCGWindowListExcludeDesktopElements,
        0, // kCGNullWindowID
    ) else {
        return out; // no Screen Recording grant; the capture itself will say so
    };

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
        let Some(bounds) = dict
            .find(CFString::from_static_string("kCGWindowBounds"))
            .and_then(|v| v.downcast::<CFDictionary>())
        else {
            continue;
        };
        let bounds: CFDictionary<CFString, CFType> =
            unsafe { CFDictionary::wrap_under_get_rule(bounds.as_concrete_TypeRef() as _) };
        let (x, y, w, h) = (
            get_num(&bounds, "X").unwrap_or(0.0),
            get_num(&bounds, "Y").unwrap_or(0.0),
            get_num(&bounds, "Width").unwrap_or(0.0),
            get_num(&bounds, "Height").unwrap_or(0.0),
        );
        if w < 200.0 || h < 150.0 {
            // Panels, popovers, and chrome slivers — e.g. Safari's toolbar strip is its
            // own full-width ~80pt CGWindow on modern macOS and must not win here.
            continue;
        }
        let title = dict
            .find(CFString::from_static_string("kCGWindowName"))
            .and_then(|v| v.downcast::<CFString>())
            .map(|s| s.to_string())
            .filter(|s| !s.is_empty())
            .unwrap_or_else(|| owner.clone());
        let pid = dict
            .find(CFString::from_static_string("kCGWindowOwnerPID"))
            .and_then(|v| v.downcast::<CFNumber>())
            .and_then(|n| n.to_i64())
            .unwrap_or(0) as i32;
        out.push(ListedWindow { pid, x, y, w, h, title, owner });
    }
    out
}

/// (origin, size) in points of the display under the cursor (top-left origin, global coords).
/// Which display the focused app's window is on, without the window-list cross-check.
///
/// [`frontmost_window_bounds`] confirms its answer against the on-screen window list because a
/// capture aims at *pixels*: a rect belonging to a window on another Space would photograph
/// whatever happens to sit at those coordinates. Placing a summoned window has no such stake —
/// "the display the user is looking at" is the entire requirement — and that confirmation is
/// the expensive half, because it enumerates and allocates a dictionary per on-screen window.
///
/// So this asks the two cheap questions only. It is the difference between the palette
/// appearing at once and appearing after a beat.
pub(super) fn focused_window_display() -> Option<((f64, f64), (f64, f64))> {
    let (pid, _) = frontmost_app()?;
    let ((x, y, w, h), _) = super::ax::focused_window(pid)?;
    display_bounds_containing_point(x + w / 2.0, y + h / 2.0)
}

pub(super) fn display_bounds_under_cursor() -> ((f64, f64), (f64, f64)) {
    let id = display_id_under_cursor().unwrap_or_else(|| CGDisplay::main().id);
    let b = CGDisplay::new(id).bounds();
    ((b.origin.x, b.origin.y), (b.size.width, b.size.height))
}

/// (origin, size) in points of the display containing a global point, if any.
pub(super) fn display_bounds_containing_point(x: f64, y: f64) -> Option<((f64, f64), (f64, f64))> {
    for id in CGDisplay::active_displays().ok()? {
        let b = CGDisplay::new(id).bounds();
        if x >= b.origin.x
            && x < b.origin.x + b.size.width
            && y >= b.origin.y
            && y < b.origin.y + b.size.height
        {
            return Some(((b.origin.x, b.origin.y), (b.size.width, b.size.height)));
        }
    }
    None
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

#[cfg(test)]
mod tests {
    use super::intersect;

    /// The three-monitors-side-by-side arrangement this was written for: displays at
    /// x = -1920, 0 and 1920, and a window dragged across the seam between two of them.
    #[test]
    fn a_window_across_the_seam_resolves_to_one_piece_per_display() {
        let left = (-1920.0, 0.0, 1920.0, 1080.0);
        let middle = (0.0, 0.0, 1920.0, 1080.0);
        let right = (1920.0, 0.0, 1920.0, 1080.0);
        let window = (-300.0, 100.0, 900.0, 700.0); // 300pt on the left display, 600 on the middle

        assert_eq!(intersect(window, left), Some((-300.0, 100.0, 300.0, 700.0)));
        assert_eq!(intersect(window, middle), Some((0.0, 100.0, 600.0, 700.0)));
        assert_eq!(intersect(window, right), None);

        // The pieces tile the window exactly — no overlap, no gap, so the composite lines up.
        let pieces = [intersect(window, left).unwrap(), intersect(window, middle).unwrap()];
        assert_eq!(pieces[0].0 + pieces[0].2, pieces[1].0);
        assert_eq!(pieces.iter().map(|p| p.2).sum::<f64>(), window.2);
    }

    #[test]
    fn a_window_hanging_off_the_desktop_is_trimmed_not_padded() {
        let display = (0.0, 0.0, 1920.0, 1080.0);
        // Half off the left edge of a single-display desktop.
        assert_eq!(intersect((-400.0, 50.0, 800.0, 600.0), display), Some((0.0, 50.0, 400.0, 600.0)));
        // Entirely off it.
        assert_eq!(intersect((-900.0, 50.0, 800.0, 600.0), display), None);
        // Edge-to-edge contact is not an overlap worth capturing.
        assert_eq!(intersect((1920.0, 0.0, 400.0, 400.0), display), None);
    }
}
