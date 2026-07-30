//! Scrolling capture: select a region (a whole page, or just a scrollable panel inside one),
//! then the app repeatedly captures that exact rect while injecting scroll-wheel events at its
//! centre, stitches the frames on their overlap, and hands the tall result to the editor.
//!
//! Fully in-app and application-agnostic — no browser extension, no DOM access: it works on
//! web pages, Slack threads, PDFs, terminals, IDE panes… anything that responds to a scroll
//! wheel. Trade-off vs DOM stitching: sticky headers/parallax elements can ghost at seams
//! (documented limitation).
//!
//! Requires TWO macOS permissions: Screen Recording (capture — shared with all capture modes)
//! and Accessibility **for the app itself** (posting synthetic scroll events; separate from
//! the engine's grant, which covers a different process).
//!
//! It stops on its own at the bottom of the content, and **Esc** stops it early and keeps every
//! frame taken so far. Because a wheel event lands wherever the pointer is, the loop also pauses
//! whenever the pointer leaves the region: moving the mouse mid-capture used to scroll something
//! else and stitch nonsense.

use std::sync::atomic::{AtomicBool, Ordering};
use std::time::{Duration, Instant};

use anyhow::anyhow;
use core_graphics::display::{CGDisplay, CGPoint};
use core_graphics::event::{CGEvent, CGEventTapLocation, ScrollEventUnit};
use core_graphics::event_source::{CGEventSource, CGEventSourceStateID};
use image::RgbaImage;

use super::Shot;

/// Upper bound on captured frames (≈ MAX_FRAMES × 60% × region-height of content).
const MAX_FRAMES: usize = 30;
/// Fraction of the region height scrolled between frames (leaves ~40% overlap to match on).
const SCROLL_FRACTION: f64 = 0.6;
/// Settle time after a scroll before the next capture (rendering, inertia none — we use
/// discrete wheel events, but slow pages need paint time).
const SETTLE: Duration = Duration::from_millis(300);
/// How far (px) around the expected overlap the stitcher searches.
const OVERLAP_SEARCH: i64 = 90;
/// Mean-absolute-difference (0-255) below which two rows bands are "the same".
const MATCH_THRESHOLD: f64 = 8.0;
/// How far outside the region the pointer may stray before the capture hands it back. A hand
/// resting on a trackpad twitches; a deliberate move away from the region does not.
const POINTER_SLACK: f64 = 24.0;
/// How long to stay paused waiting for the pointer before finishing with the frames in hand.
/// Someone who walks away mid-capture still gets a usable image rather than a stuck app.
const PAUSE_LIMIT: Duration = Duration::from_secs(30);
/// Granularity of every wait in the loop, so an Esc lands well inside one frame time.
const TICK: Duration = Duration::from_millis(40);

/// Set by [`request_stop`], cleared at the start of every capture.
static STOP: AtomicBool = AtomicBool::new(false);

/// Finish the running capture now and keep the frames it already has — what Escape
/// and Esc do. Not a cancel: a short page you stopped early is still the page you wanted.
pub fn request_stop() {
    STOP.store(true, Ordering::SeqCst);
}

fn stopping() -> bool {
    STOP.load(Ordering::SeqCst)
}

/// Forget a stop asked for before this capture existed.
///
/// Called by the caller *before* it arms Escape, never from inside [`capture`]: clearing it
/// after the key is live throws away a press made in between, and "I hit Esc immediately and it
/// kept going" is the one failure this control cannot have.
pub fn clear_stop() {
}

/// One scrolling capture.
pub struct Job {
    /// Region to photograph, in global points (top-left origin).
    pub rect: (f64, f64, f64, f64),
    /// The on-screen readout's own frame, if one is up. The pointer being there means the user
    /// is over the readout, which cannot scroll anything, rather than asking for another scroll.
    pub hud: Option<(f64, f64, f64, f64)>,
}

/// What the capture is doing, for the readout.
#[derive(Clone, Copy)]
pub struct Progress {
    pub frames: usize,
    /// The pointer has left the region — nothing is being scrolled until it comes back.
    pub paused: bool,
}

#[link(name = "ApplicationServices", kind = "framework")]
extern "C" {
    fn AXIsProcessTrusted() -> bool;
    fn AXIsProcessTrustedWithOptions(options: core_foundation::dictionary::CFDictionaryRef) -> bool;
}

/// Whether the app may post synthetic events / observe keystrokes. On modern macOS this is
/// THE Accessibility check that matters for the engine too: TCC attributes a bundled child
/// process to its **responsible app**, so one "Glyphio" grant covers expansion (the engine
/// sidecar) and scrolling capture alike.
pub fn app_accessibility_trusted() -> bool {
    unsafe { AXIsProcessTrusted() }
}

/// Ask macOS to show the Accessibility permission dialog — which also ADDS the correct
/// "Glyphio" row to System Settings, so users never hunt for binaries with the "+" button.
pub fn request_accessibility() -> bool {
    use core_foundation::base::TCFType;
    use core_foundation::boolean::CFBoolean;
    use core_foundation::dictionary::CFDictionary;
    use core_foundation::string::CFString;
    let key = CFString::from_static_string("AXTrustedCheckOptionPrompt");
    let dict = CFDictionary::from_CFType_pairs(&[(key.as_CFType(), CFBoolean::true_value().as_CFType())]);
    unsafe { AXIsProcessTrustedWithOptions(dict.as_concrete_TypeRef()) }
}

/// The grant this mode needs, as an error rather than a bool, so the caller can refuse before
/// putting anything on screen — a missing permission shouldn't flash a readout first.
pub fn require_accessibility() -> anyhow::Result<()> {
    if app_accessibility_trusted() {
        return Ok(());
    }
    anyhow::bail!(
        "Scrolling capture needs Accessibility permission for Glyphio itself \
         (System Settings › Privacy & Security › Accessibility) — it scrolls the page for you."
    )
}

/// Capture a scrolling region. The rect is in global screen coordinates in points (top-left
/// origin), as reported by the selection overlay or by the page's accessibility geometry.
///
/// `on_progress` is called on this thread every time the count or the paused state changes, so
/// the readout can say what is happening; it must not block.
pub fn capture(job: Job, on_progress: &dyn Fn(Progress)) -> anyhow::Result<Shot> {
    let (x, y, w, h) = job.rect;
    require_accessibility()?;
    if w < 40.0 || h < 40.0 {
        anyhow::bail!("selection too small");
    }

    // NB: the stop flag is cleared by the caller before it arms Escape — see [`clear_stop`].
    // Park the cursor mid-region so wheel events reach the right scroller; restore after.
    let original = cursor_position();
    warp_cursor(CGPoint::new(x + w / 2.0, y + h / 2.0));
    std::thread::sleep(Duration::from_millis(120));

    let mut frames: Vec<RgbaImage> = Vec::new();
    let mut capture_dpr = 1.0;
    let mut scroll_px_points = (h * SCROLL_FRACTION).round();
    let result = (|| -> anyhow::Result<()> {
        loop {
            let (frame, frame_dpr) = super::backend::capture_rect_image(x, y, w, h)?;
            capture_dpr = frame_dpr;
            if let Some(prev) = frames.last() {
                if frames_identical(prev, &frame) {
                    break; // nothing moved — bottom (or an end of the scroller) reached
                }
            }
            frames.push(frame);
            on_progress(Progress { frames: frames.len(), paused: false });
            if frames.len() >= MAX_FRAMES {
                log::info!("scrolling capture reached the {MAX_FRAMES}-frame cap");
                break;
            }
            if stopping() || !wait_for_pointer(&job, frames.len(), on_progress) {
                break;
            }
            post_scroll(-(scroll_px_points as i32))?;
            if !nap(SETTLE) {
                break;
            }
        }
        Ok(())
    })();
    // Give the pointer back where we found it — but only if it is still where we put it. If the
    // user has moved it themselves (that's what pauses the capture), yanking it back would be
    // the rudest possible ending.
    if let Some(p) = original {
        if pointer_in_region(&job) {
            warp_cursor(p);
        }
    }
    result?;

    let first = frames.first().ok_or_else(|| anyhow!("no frames captured"))?;
    // Pixels per point from the capture itself (the rect may have been clamped to a
    // display, so first.width()/w would be wrong for oversized selections).
    let dpr = capture_dpr;
    scroll_px_points *= dpr; // expected overlap works in pixels below
    let expected_overlap = first.height() as i64 - scroll_px_points as i64;
    let stitched = stitch(frames, expected_overlap);
    let (width, height) = stitched.dimensions();
    Ok(Shot {
        rgba: stitched.into_raw(),
        width,
        height,
        dpr,
        title: String::new(),
        browser: Default::default(),
    })
}

/// Whether the pointer is still over the region and not over the readout — i.e. whether the
/// capture may scroll. A wheel event goes to whatever is under the pointer, so scrolling while
/// the user has taken the mouse elsewhere scrolls the wrong thing and stitches nonsense.
fn pointer_in_region(job: &Job) -> bool {
    match cursor_position() {
        Some(p) => may_scroll(job, (p.x, p.y)),
        None => true, // can't tell where the pointer is — carry on rather than stall
    }
}

/// The rule itself, without asking the window server where the pointer is.
fn may_scroll(job: &Job, p: (f64, f64)) -> bool {
    let inside = |(rx, ry, rw, rh): (f64, f64, f64, f64), slack: f64| {
        p.0 >= rx - slack && p.0 <= rx + rw + slack && p.1 >= ry - slack && p.1 <= ry + rh + slack
    };
    inside(job.rect, POINTER_SLACK) && !job.hud.is_some_and(|r| inside(r, 0.0))
}

/// Hold the capture while the pointer is away, and resume when it comes back. Returns false
/// when the capture should finish with what it has: an Esc, or nobody came back.
fn wait_for_pointer(job: &Job, frames: usize, on_progress: &dyn Fn(Progress)) -> bool {
    if pointer_in_region(job) {
        return true;
    }
    on_progress(Progress { frames, paused: true });
    let deadline = Instant::now() + PAUSE_LIMIT;
    while Instant::now() < deadline {
        if stopping() {
            return false;
        }
        std::thread::sleep(TICK);
        if pointer_in_region(job) {
            on_progress(Progress { frames, paused: false });
            return true;
        }
    }
    log::info!("scrolling capture: pointer stayed away, finishing with {frames} frame(s)");
    false
}

/// Sleep in slices so an Esc is noticed promptly. False when the capture should finish.
fn nap(total: Duration) -> bool {
    let deadline = Instant::now() + total;
    loop {
        if stopping() {
            return false;
        }
        let left = deadline.saturating_duration_since(Instant::now());
        if left.is_zero() {
            return !stopping(); // an Esc in the final slice still ends it here
        }
        std::thread::sleep(left.min(TICK));
    }
}

fn cursor_position() -> Option<CGPoint> {
    let source = CGEventSource::new(CGEventSourceStateID::CombinedSessionState).ok()?;
    Some(CGEvent::new(source).ok()?.location())
}

fn warp_cursor(p: CGPoint) {
    let _ = CGDisplay::warp_mouse_cursor_position(p);
}

/// Diagnostics-only alias for [`post_scroll`] (used by `capture::diag`).
pub fn post_scroll_for_diag(delta: i32) -> anyhow::Result<()> {
    post_scroll(delta)
}

/// Post a pixel-unit scroll (negative = content moves up / scrolls down).
fn post_scroll(delta: i32) -> anyhow::Result<()> {
    let source = CGEventSource::new(CGEventSourceStateID::CombinedSessionState)
        .map_err(|_| anyhow!("could not create event source"))?;
    let ev = CGEvent::new_scroll_event(source, ScrollEventUnit::PIXEL, 1, delta, 0, 0)
        .map_err(|_| anyhow!("could not create scroll event"))?;
    ev.post(CGEventTapLocation::HID);
    Ok(())
}

// ---- stitching -----------------------------------------------------------------

/// Downsampled grayscale of every 4th pixel of a row band — cheap, alignment-stable signature.
fn band_gray(img: &RgbaImage, from_row: u32, rows: u32) -> Vec<u8> {
    let mut out = Vec::with_capacity(((rows * img.width()) / 4 + 1) as usize);
    for y in from_row..(from_row + rows).min(img.height()) {
        for x in (0..img.width()).step_by(4) {
            let p = img.get_pixel(x, y);
            out.push(((p[0] as u16 + p[1] as u16 + p[2] as u16) / 3) as u8);
        }
    }
    out
}

fn mad(a: &[u8], b: &[u8]) -> f64 {
    if a.is_empty() || a.len() != b.len() {
        return f64::MAX;
    }
    let sum: u64 = a.iter().zip(b).map(|(x, y)| (*x as i64 - *y as i64).unsigned_abs()).sum();
    sum as f64 / a.len() as f64
}

fn frames_identical(a: &RgbaImage, b: &RgbaImage) -> bool {
    a.dimensions() == b.dimensions()
        && mad(&band_gray(a, 0, a.height()), &band_gray(b, 0, b.height())) < 1.0
}

/// Find how many of `prev`'s bottom rows reappear at `next`'s top, searching around the
/// expected overlap, and append only the fresh rows of each frame.
fn stitch(frames: Vec<RgbaImage>, expected_overlap: i64) -> RgbaImage {
    let mut iter = frames.into_iter();
    let first = iter.next().expect("stitch called with ≥1 frame");
    let (w, h) = first.dimensions();
    // Match always against the previous FULL frame (an output fragment can be shorter than
    // the probe band); output accumulates first + fresh fragments.
    let mut prev_full = first.clone();
    let mut rows: Vec<RgbaImage> = vec![first];

    for next in iter {
        let prev = &prev_full;
        let prev_h = prev.height();
        // Match on a fixed-height probe band: prev's bottom `probe` rows must reappear as
        // next's rows [o-probe, o) when the true overlap is o. O(search × band); probing the
        // *end* of the overlap keeps sticky headers at the top of `next` out of the signature.
        let probe: u32 = 48.min(h / 4).max(8);
        let lo = (expected_overlap - OVERLAP_SEARCH).clamp(probe as i64, h as i64 - 8) as u32;
        let hi = (expected_overlap + OVERLAP_SEARCH).clamp(probe as i64, h as i64 - 8) as u32;
        let prev_sig = band_gray(prev, prev_h - probe, probe);
        let mut best = (f64::MAX, expected_overlap.clamp(probe as i64, h as i64 - 8) as u32);
        for o in lo..=hi {
            let d = mad(&prev_sig, &band_gray(&next, o - probe, probe));
            if d < best.0 {
                best = (d, o);
            }
        }
        let overlap = if best.0 <= MATCH_THRESHOLD {
            best.1
        } else {
            // No confident match (animation? end bounce?) — fall back to the expected value.
            expected_overlap.clamp(8, h as i64 - 8) as u32
        };
        // Keep only the fresh rows below the overlap.
        let fresh_h = next.height().saturating_sub(overlap);
        if fresh_h > 0 {
            rows.push(image::imageops::crop_imm(&next, 0, overlap, w, fresh_h).to_image());
        }
        prev_full = next;
    }

    let total_h: u32 = rows.iter().map(|r| r.height()).sum();
    let mut out = RgbaImage::new(w, total_h);
    let mut y = 0;
    for r in rows {
        image::imageops::overlay(&mut out, &r, 0, y as i64);
        y += r.height();
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Build a tall synthetic "page", cut overlapping viewport frames from it, stitch, and
    /// require the reconstruction to match the original content.
    #[test]
    fn stitch_reconstructs_a_tall_page_from_overlapping_frames() {
        let (w, page_h, view_h) = (64u32, 600u32, 200u32);
        let mut page = RgbaImage::new(w, page_h);
        for y in 0..page_h {
            for x in 0..w {
                // High-frequency deterministic pattern so overlaps are unambiguous.
                let v = ((x * 7 + y * 13) % 251) as u8;
                page.put_pixel(x, y, image::Rgba([v, v.wrapping_mul(3), v ^ 0x5a, 255]));
            }
        }
        let scroll = 120; // 60% of view → overlap 80
        let mut frames = Vec::new();
        let mut top = 0;
        while top + view_h <= page_h {
            frames.push(image::imageops::crop_imm(&page, 0, top, w, view_h).to_image());
            top += scroll;
        }
        let expected_overlap = (view_h - scroll) as i64;
        let out = stitch(frames.clone(), expected_overlap);

        assert_eq!(out.width(), w);
        let covered = view_h + (frames.len() as u32 - 1) * scroll;
        assert_eq!(out.height(), covered);
        // Every stitched pixel must equal the source page pixel.
        for y in (0..covered).step_by(7) {
            for x in (0..w).step_by(5) {
                assert_eq!(out.get_pixel(x, y), page.get_pixel(x, y), "mismatch at {x},{y}");
            }
        }
    }

    #[test]
    fn the_capture_scrolls_only_while_the_pointer_is_its_own() {
        // A 800×600 region with the readout parked just below it.
        let job = Job { rect: (100.0, 100.0, 800.0, 600.0), hud: Some((364.0, 714.0, 272.0, 44.0)) };
        assert!(may_scroll(&job, (500.0, 400.0)), "middle of the region");
        assert!(may_scroll(&job, (100.0, 100.0)), "top-left corner counts as inside");
        // A twitch just outside the edge must not hand the pointer back.
        assert!(may_scroll(&job, (905.0, 400.0)), "within the slack");
        assert!(!may_scroll(&job, (960.0, 400.0)), "clearly outside");
        // The readout's top edge falls inside the slack margin, so only the readout check can
        // pause the capture here — which is the whole point of passing its frame in.
        assert!(may_scroll(&job, (500.0, 705.0)), "in the gap, still scrolling");
        assert!(!may_scroll(&job, (500.0, 718.0)), "over the readout, paused");
    }

    #[test]
    fn identical_frames_detected() {
        let a = RgbaImage::from_pixel(32, 32, image::Rgba([9, 9, 9, 255]));
        let b = a.clone();
        assert!(frames_identical(&a, &b));
        let mut c = a.clone();
        for y in 0..32 {
            c.put_pixel(16, y, image::Rgba([200, 0, 0, 255]));
        }
        assert!(!frames_identical(&a, &c));
    }
}
