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
use image::RgbaImage;

use super::Shot;
use crate::platform::Progress;

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
/// Keep synthetic wheel deltas in the range accepted consistently by macOS clients.
const MAX_SCROLL_DELTA: i32 = 10;

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
    STOP.store(false, Ordering::SeqCst);
}

/// One scrolling capture.
pub struct Job {
    /// Region to photograph, in global points (top-left origin).
    pub rect: (f64, f64, f64, f64),
}

/// Pixel capture and scroll dispatch used by the feedback loop.
pub trait ScrollDriver {
    fn capture(&mut self, rect: (f64, f64, f64, f64)) -> anyhow::Result<(RgbaImage, f64)>;
    fn scroll(&mut self, delta: i32) -> anyhow::Result<()>;
    /// Where the main scroll container is, read from something other than the pixels (the
    /// accessibility tree). `None` when the platform cannot tell; the loop then relies on
    /// pixel comparison alone.
    fn progress(&mut self, _rect: (f64, f64, f64, f64)) -> Option<Progress> {
        None
    }
}

struct PlatformDriver;

impl ScrollDriver for PlatformDriver {
    fn capture(&mut self, rect: (f64, f64, f64, f64)) -> anyhow::Result<(RgbaImage, f64)> {
        crate::platform::current().capturer.rect_image(rect.0, rect.1, rect.2, rect.3)
    }

    fn scroll(&mut self, delta: i32) -> anyhow::Result<()> {
        crate::platform::current().input.post_scroll(delta)
    }

    /// Asked at the point the wheel events land: the region's centre, where the pointer is
    /// parked. Also keeps the browser's accessibility tree warm for the whole capture.
    fn progress(&mut self, rect: (f64, f64, f64, f64)) -> Option<Progress> {
        crate::platform::current().inspector.scroll_progress((rect.0 + rect.2 / 2.0, rect.1 + rect.3 / 2.0))
    }
}

/// The grant this mode needs, as an error rather than a bool, so the caller can refuse before
/// starting a capture.
pub fn require_accessibility() -> anyhow::Result<()> {
    require_accessibility_with(&*crate::platform::current().permissions)
}

/// [`require_accessibility`] against a given permission source.
pub fn require_accessibility_with(permissions: &dyn crate::platform::Permissions) -> anyhow::Result<()> {
    if permissions.accessibility_trusted() {
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
/// `heartbeat` is called once per frame so a watchdog can tell a long page from a stuck one.
pub fn capture(job: Job, heartbeat: impl Fn()) -> anyhow::Result<Shot> {
    let (x, y, w, h) = job.rect;
    require_accessibility()?;
    if w < 40.0 || h < 40.0 {
        anyhow::bail!("selection too small");
    }

    // NB: the stop flag is cleared by the caller before it arms Escape — see [`clear_stop`].
    // Park the cursor mid-region so wheel events reach the right scroller; restore after.
    let input = &crate::platform::current().input;
    let original = input.cursor_position();
    input.warp_cursor((x + w / 2.0, y + h / 2.0));
    std::thread::sleep(Duration::from_millis(120));

    let mut driver = PlatformDriver;
    let result = capture_loop(
        &job,
        &mut driver,
        |frames| !stopping() && wait_for_pointer(&job, frames, &heartbeat),
        || {
            heartbeat();
            nap(SETTLE)
        },
    );
    // Give the pointer back where we found it — but only if it is still where we put it. If the
    // user has moved it themselves (that's what pauses the capture), yanking it back would be
    // the rudest possible ending.
    if let Some(p) = original {
        if pointer_in_region(&job) {
            input.warp_cursor(p);
        }
    }
    result
}

/// Run the feedback loop with a replaceable pixel/scroll adapter.
///
/// The production entry point adds permission, pointer, and Escape policy around this seam.
pub fn capture_with_driver<D: ScrollDriver>(job: Job, driver: &mut D) -> anyhow::Result<Shot> {
    let (_, _, w, h) = job.rect;
    if w < 40.0 || h < 40.0 {
        anyhow::bail!("selection too small");
    }
    capture_loop(&job, driver, |_| true, || true)
}

fn capture_loop<D, W, S>(
    job: &Job,
    driver: &mut D,
    mut before_scroll: W,
    mut settle: S,
) -> anyhow::Result<Shot>
where
    D: ScrollDriver,
    W: FnMut(usize) -> bool,
    S: FnMut() -> bool,
{
    let mut frames: Vec<RgbaImage> = Vec::new();
    let mut capture_dpr;
    let mut last_progress = driver.progress(job.rect);
    loop {
        let (frame, frame_dpr) = driver.capture(job.rect)?;
        capture_dpr = frame_dpr;
        if let Some(prev) = frames.last() {
            if frames_identical(prev, &frame) {
                break;
            }
        }
        frames.push(frame);
        if frames.len() >= MAX_FRAMES {
            log::info!("scrolling capture reached the {MAX_FRAMES}-frame cap");
            break;
        }
        if !before_scroll(frames.len()) {
            break;
        }
        let delta = -((job.rect.3 * SCROLL_FRACTION * frame_dpr).round() as i32);
        dispatch_scroll(driver, delta)?;
        if !settle() {
            break;
        }
        let now = driver.progress(job.rect);
        if let Some(p) = now {
            log::debug!(
                "scrolling capture progress: offset {:.0} of {:.0} (viewport {:.0}){}",
                p.offset,
                p.extent,
                p.viewport,
                if progress_advanced(last_progress, now) { "" } else { " — did not advance" }
            );
        }
        last_progress = now.or(last_progress);
    }

    let first = frames
        .first()
        .ok_or_else(|| anyhow!("no frames captured"))?;
    let expected_scroll = (job.rect.3 * SCROLL_FRACTION * capture_dpr).round() as i64;
    let expected_overlap = first.height() as i64 - expected_scroll;
    let stitched = stitch(frames, expected_overlap);
    let (width, height) = stitched.dimensions();
    Ok(Shot {
        rgba: stitched.into_raw(),
        width,
        height,
        dpr: capture_dpr,
        title: String::new(),
        browser: Default::default(),
    })
}

/// Whether the oracle saw the page move between two readings (1 pt of rounding allowed).
/// Unknown on either side counts as "cannot say", reported as no advance.
fn progress_advanced(before: Option<Progress>, after: Option<Progress>) -> bool {
    match (before, after) {
        (Some(a), Some(b)) => (b.offset - a.offset).abs() > 1.0,
        _ => false,
    }
}

fn dispatch_scroll<D: ScrollDriver>(driver: &mut D, mut delta: i32) -> anyhow::Result<()> {
    while delta != 0 {
        let step = delta.clamp(-MAX_SCROLL_DELTA, MAX_SCROLL_DELTA);
        driver.scroll(step)?;
        delta -= step;
    }
    Ok(())
}

/// Whether the pointer is still over the region — i.e. whether the capture may scroll. A wheel
/// event goes to whatever is under the pointer, so scrolling while the user has taken the mouse
/// elsewhere scrolls the wrong thing and stitches nonsense.
fn pointer_in_region(job: &Job) -> bool {
    match crate::platform::current().input.cursor_position() {
        Some(p) => may_scroll(job, p),
        None => true, // can't tell where the pointer is — carry on rather than stall
    }
}

/// The rule itself, without asking the window server where the pointer is.
fn may_scroll(job: &Job, p: (f64, f64)) -> bool {
    let inside = |(rx, ry, rw, rh): (f64, f64, f64, f64), slack: f64| {
        p.0 >= rx - slack && p.0 <= rx + rw + slack && p.1 >= ry - slack && p.1 <= ry + rh + slack
    };
    inside(job.rect, POINTER_SLACK)
}

/// Hold the capture while the pointer is away, and resume when it comes back. Returns false
/// when the capture should finish with what it has: an Esc, or nobody came back.
fn wait_for_pointer(job: &Job, frames: usize, heartbeat: &impl Fn()) -> bool {
    if pointer_in_region(job) {
        return true;
    }
    let deadline = Instant::now() + PAUSE_LIMIT;
    while Instant::now() < deadline {
        if stopping() {
            return false;
        }
        // A paused capture is waiting on the user, not stuck: keep its deadline ahead of the
        // pause limit so the watchdog doesn't cut it off mid-wait.
        heartbeat();
        std::thread::sleep(TICK);
        if pointer_in_region(job) {
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
        let job = Job { rect: (100.0, 100.0, 800.0, 600.0) };
        assert!(may_scroll(&job, (500.0, 400.0)), "middle of the region");
        assert!(may_scroll(&job, (100.0, 100.0)), "top-left corner counts as inside");
        // A twitch just outside the edge must not hand the pointer back.
        assert!(may_scroll(&job, (905.0, 400.0)), "within the slack");
        assert!(!may_scroll(&job, (960.0, 400.0)), "clearly outside");
        assert!(may_scroll(&job, (500.0, 705.0)), "in the gap, still scrolling");
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

    struct SyntheticPage {
        page: RgbaImage,
        offset: u32,
        view_h: u32,
        scrolls: usize,
    }

    impl ScrollDriver for SyntheticPage {
        fn capture(&mut self, _rect: (f64, f64, f64, f64)) -> anyhow::Result<(RgbaImage, f64)> {
            Ok((
                image::imageops::crop_imm(
                    &self.page,
                    0,
                    self.offset,
                    self.page.width(),
                    self.view_h,
                )
                .to_image(),
                1.0,
            ))
        }

        fn scroll(&mut self, delta: i32) -> anyhow::Result<()> {
            assert!(delta.abs() <= MAX_SCROLL_DELTA);
            self.scrolls += 1;
            let distance = delta.unsigned_abs();
            self.offset = (self.offset + distance).min(self.page.height() - self.view_h);
            Ok(())
        }
    }

    #[test]
    fn scrolling_feedback_loop_advances_a_selected_page() {
        let (w, page_h, view_h) = (64u32, 600u32, 200u32);
        let mut page = RgbaImage::new(w, page_h);
        for y in 0..page_h {
            for x in 0..w {
                let v = ((x * 11 + y * 17) % 251) as u8;
                page.put_pixel(x, y, image::Rgba([v, v.wrapping_mul(3), v ^ 0x5a, 255]));
            }
        }
        let expected = page.clone();
        let mut driver = SyntheticPage {
            page,
            offset: 0,
            view_h,
            scrolls: 0,
        };

        let shot = capture_with_driver(
            Job {
                rect: (0.0, 0.0, w as f64, view_h as f64),
            },
            &mut driver,
        )
        .expect("synthetic capture should complete");

        assert!(driver.scrolls > 0);
        assert!(shot.height > view_h, "capture must advance beyond its first frame");
        assert_eq!(shot.height, page_h, "end-of-content must not duplicate a frame band");
        let stitched = RgbaImage::from_raw(shot.width, shot.height, shot.rgba)
            .expect("shot pixels should have the reported dimensions");
        for y in (0..page_h).step_by(11) {
            for x in (0..w).step_by(7) {
                assert_eq!(stitched.get_pixel(x, y), expected.get_pixel(x, y));
            }
        }
    }

    #[test]
    fn an_early_stop_keeps_the_frames_already_captured() {
        let (w, page_h, view_h) = (64u32, 400u32, 200u32);
        let page = RgbaImage::from_fn(w, page_h, |x, y| {
            image::Rgba([(x % 255) as u8, (y % 255) as u8, ((x + y) % 255) as u8, 255])
        });
        let expected_first = image::imageops::crop_imm(&page, 0, 0, w, view_h).to_image();
        let mut driver = SyntheticPage {
            page,
            offset: 0,
            view_h,
            scrolls: 0,
        };

        // `before_scroll` is the same seam through which the production Escape flag stops the
        // loop. It is deliberately a finish, not a cancellation: the first frame is returned.
        let shot = capture_loop(
            &Job {
                rect: (0.0, 0.0, w as f64, view_h as f64),
            },
            &mut driver,
            |_| false,
            || true,
        )
        .expect("an early stop should still produce the captured content");

        assert_eq!(driver.scrolls, 0);
        assert_eq!((shot.width, shot.height), (w, view_h));
        assert_eq!(
            RgbaImage::from_raw(shot.width, shot.height, shot.rgba).unwrap(),
            expected_first
        );
    }

    struct FailingScrollDriver {
        frame: RgbaImage,
    }

    impl ScrollDriver for FailingScrollDriver {
        fn capture(&mut self, _rect: (f64, f64, f64, f64)) -> anyhow::Result<(RgbaImage, f64)> {
            Ok((self.frame.clone(), 1.0))
        }

        fn scroll(&mut self, _delta: i32) -> anyhow::Result<()> {
            anyhow::bail!("injected scroll failure")
        }
    }

    #[test]
    fn a_scroll_failure_is_reported_instead_of_returning_partial_success() {
        let mut driver = FailingScrollDriver {
            frame: RgbaImage::from_pixel(64, 64, image::Rgba([20, 40, 60, 255])),
        };

        let error = match capture_with_driver(
            Job {
                rect: (0.0, 0.0, 64.0, 64.0),
            },
            &mut driver,
        ) {
            Ok(_) => panic!("a scroll failure must not return a successful capture"),
            Err(error) => error,
        };

        assert_eq!(error.to_string(), "injected scroll failure");
    }

    /// A page with a nested scroller (a code block) under the centre of the window and a
    /// pointer that decides which container a wheel event reaches. Only the page is
    /// reported by the oracle.
    struct TwoContainers {
        page_offset: f64,
        nested_offset: f64,
        pointer_over_nested: bool,
    }

    impl ScrollDriver for TwoContainers {
        fn capture(&mut self, _: (f64, f64, f64, f64)) -> anyhow::Result<(RgbaImage, f64)> {
            // Pixels change whichever container moved — that is what fools a pixel check.
            let v = ((self.page_offset + self.nested_offset) as u32 % 250) as u8;
            Ok((RgbaImage::from_pixel(64, 64, image::Rgba([v, v, v, 255])), 1.0))
        }
        fn scroll(&mut self, delta: i32) -> anyhow::Result<()> {
            let d = delta.unsigned_abs() as f64;
            if self.pointer_over_nested {
                self.nested_offset += d;
            } else {
                self.page_offset = (self.page_offset + d).min(2400.0);
            }
            Ok(())
        }
        fn progress(&mut self, _: (f64, f64, f64, f64)) -> Option<Progress> {
            Some(Progress { offset: self.page_offset, extent: 3000.0, viewport: 600.0 })
        }
    }

    #[test]
    fn progress_advances_with_the_page_and_not_with_a_nested_scroller() {
        let rect = (0.0, 0.0, 64.0, 64.0);
        let mut d = TwoContainers { page_offset: 0.0, nested_offset: 0.0, pointer_over_nested: true };
        let before = d.progress(rect);
        let pixels_before = d.capture(rect).unwrap().0;
        d.scroll(-10).unwrap();
        assert_ne!(d.capture(rect).unwrap().0, pixels_before, "the nested scroller changed pixels");
        assert!(!progress_advanced(before, d.progress(rect)), "but the page did not move");

        d.pointer_over_nested = false;
        let before = d.progress(rect);
        for _ in 0..5 {
            d.scroll(-10).unwrap();
        }
        assert!(progress_advanced(before, d.progress(rect)));
        assert_eq!(d.progress(rect).unwrap().offset, 50.0);
    }

    #[test]
    fn a_driver_without_an_oracle_reports_no_progress() {
        let mut d = FailingScrollDriver { frame: RgbaImage::new(8, 8) };
        assert!(d.progress((0.0, 0.0, 8.0, 8.0)).is_none());
        assert!(!progress_advanced(None, None));
    }

    #[test]
    fn escape_only_stops_the_current_capture() {
        request_stop();
        assert!(stopping());

        clear_stop();

        assert!(!stopping());
    }
}
