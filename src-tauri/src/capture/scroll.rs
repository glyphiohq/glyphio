//! Scrolling capture: select a region (a whole page, or just a scrollable panel inside one),
//! then the app repeatedly captures that exact rect while injecting scroll-wheel events at its
//! centre, stitches the frames on their overlap, and hands the tall result to the editor.
//!
//! Fully in-app and application-agnostic — no browser extension, no DOM access: it works on
//! web pages, Slack threads, PDFs, terminals, IDE panes… anything that responds to a scroll
//! wheel. Fixed headers and footers are detected and kept once (see `stitch`); parallax
//! elements can still ghost at seams.
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

use image::RgbaImage;

use super::Shot;
use stitch::{band_gray, mad, stitch};
use crate::platform::Progress;

mod stitch;

/// Default height limit in points; `scrollMaxHeightPt` overrides it.
pub const DEFAULT_MAX_HEIGHT_PT: f64 = 30_000.0;
/// Fraction of the region height scrolled between frames (leaves ~40% overlap to match on).
const SCROLL_FRACTION: f64 = 0.6;
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
    /// Stop once this much content (points) is gathered.
    pub max_height_pt: f64,
}

impl Job {
    pub fn new(rect: (f64, f64, f64, f64)) -> Self {
        Self { rect, max_height_pt: DEFAULT_MAX_HEIGHT_PT }
    }
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
/// `heartbeat` is called often enough (every settle read) that a watchdog can tell a long page
/// from a stuck one; `progress` receives the text for the menu bar.
pub fn capture(
    job: Job,
    heartbeat: impl Fn(),
    progress: impl Fn(String),
) -> anyhow::Result<Outcome> {
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
    let mut control = LiveControl { job: &job, heartbeat: &heartbeat, progress: &progress };
    let result = capture_loop(&job, &mut driver, &mut control);
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
/// Waits are virtual here, so settling and retry logic runs instantly under test.
pub fn capture_with_driver<D: ScrollDriver>(job: Job, driver: &mut D) -> anyhow::Result<Outcome> {
    let (_, _, w, h) = job.rect;
    if w < 40.0 || h < 40.0 {
        anyhow::bail!("selection too small");
    }
    capture_loop(&job, driver, &mut Unattended)
}

/// The environment the loop runs in: whether it may continue, how to wait, where to report.
trait LoopControl {
    /// Checked before every scroll. `Some` ends the capture with that reason.
    fn before_scroll(&mut self, frames: usize) -> Option<StopReason>;
    /// Wait `d`, keeping the capture's deadline alive. False when an Esc cut the wait short.
    fn pause(&mut self, d: Duration) -> bool;
    /// Progress text for the menu bar.
    fn progress(&mut self, text: String);
}

struct Unattended;

impl LoopControl for Unattended {
    fn before_scroll(&mut self, _: usize) -> Option<StopReason> {
        None
    }
    fn pause(&mut self, _: Duration) -> bool {
        true
    }
    fn progress(&mut self, _: String) {}
}

struct LiveControl<'a, H: Fn(), P: Fn(String)> {
    job: &'a Job,
    heartbeat: &'a H,
    progress: &'a P,
}

impl<H: Fn(), P: Fn(String)> LoopControl for LiveControl<'_, H, P> {
    fn before_scroll(&mut self, frames: usize) -> Option<StopReason> {
        if stopping() {
            return Some(StopReason::Stopped);
        }
        if wait_for_pointer(self.job, frames, self.heartbeat) {
            return None;
        }
        Some(if stopping() { StopReason::Stopped } else { StopReason::PointerLeft })
    }
    fn pause(&mut self, d: Duration) -> bool {
        (self.heartbeat)();
        nap(d)
    }
    fn progress(&mut self, text: String) {
        (self.progress)(text);
    }
}

/// Why a scrolling capture ended. Logged for every capture and surfaced when it matters.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum StopReason {
    /// The bottom of the content: the oracle said so, or nothing changed after retries.
    ReachedEnd,
    /// Escape.
    Stopped,
    /// The pointer stayed away from the region until the pause limit.
    PointerLeft,
    /// The height limit was reached; there is more below.
    LimitReached,
    /// The oracle says more remains but scrolling made no progress.
    Stalled,
}

impl StopReason {
    /// One line for the log and, when the capture is incomplete, for the menu bar.
    pub fn describe(self) -> &'static str {
        match self {
            StopReason::ReachedEnd => "reached the end of the page",
            StopReason::Stopped => "stopped early",
            StopReason::PointerLeft => "stopped because the pointer left the region",
            StopReason::LimitReached => "stopped at the height limit; more of the page remains",
            StopReason::Stalled => "stopped because the page stopped scrolling",
        }
    }

    /// Whether the user got less than the whole scrollable content, and should be told.
    pub fn is_incomplete(self) -> bool {
        !matches!(self, StopReason::ReachedEnd)
    }
}

/// A finished scrolling capture and the reason it ended.
pub struct Outcome {
    pub shot: Shot,
    pub reason: StopReason,
}

/// Wait between reads while a page settles after a scroll.
const SETTLE_INTERVAL: Duration = Duration::from_millis(80);
/// Reads before giving up on a page that never settles (≈1.5 s of waiting).
const SETTLE_READS: usize = 19;
/// Base of the extra waits before a frame that looks like the end is believed (doubled each time).
const END_RETRY_BASE: Duration = Duration::from_millis(300);
/// Retries before "nothing changed" means the end, when there is no oracle.
const END_RETRIES: u32 = 2;
/// Scrolls with no oracle progress, while it says more remains, before giving up.
const STALL_LIMIT: u32 = 3;

/// Menu-bar text: a percentage when the oracle knows how far along the page we are, otherwise
/// the frame count.
fn progress_text(frames: usize, p: Option<Progress>) -> String {
    match p {
        Some(p) => format!("{}%", (p.fraction() * 100.0).round() as u32),
        None => format!("{frames} frame{}", if frames == 1 { "" } else { "s" }),
    }
}

/// Capture until two consecutive reads match (smooth scrolling, lazy rendering), or the read
/// budget runs out, in which case the last read is used. `None` when Esc interrupted.
fn settled_frame<D: ScrollDriver, C: LoopControl>(
    driver: &mut D,
    ctl: &mut C,
    rect: (f64, f64, f64, f64),
) -> anyhow::Result<Option<(RgbaImage, f64)>> {
    let mut prev: Option<RgbaImage> = None;
    for _ in 0..SETTLE_READS {
        if !ctl.pause(SETTLE_INTERVAL) {
            return Ok(None);
        }
        let (frame, dpr) = driver.capture(rect)?;
        if prev.as_ref().is_some_and(|p| frames_identical(p, &frame)) {
            return Ok(Some((frame, dpr)));
        }
        prev = Some(frame);
    }
    // Never settled (an animation, a video): take the latest and let the oracle judge.
    let (frame, dpr) = driver.capture(rect)?;
    Ok(Some((frame, dpr)))
}

fn capture_loop<D: ScrollDriver, C: LoopControl>(
    job: &Job,
    driver: &mut D,
    ctl: &mut C,
) -> anyhow::Result<Outcome> {
    let mut frames: Vec<RgbaImage> = Vec::new();
    // An oracle that already claims the end before anything has scrolled is describing the
    // viewport, not the page (some browsers expose no document extent). Believing it would
    // return the visible screen alone, so such an oracle is ignored for the whole capture and
    // the pixels decide.
    let mut progress = driver.progress(job.rect).filter(|p| !p.at_end());
    let oracle_trusted = progress.is_some();
    let (first, mut capture_dpr) = driver.capture(job.rect)?;
    // Height gathered so far in points, the basis for the height limit.
    let mut covered_pt = first.height() as f64 / capture_dpr;
    frames.push(first);
    ctl.progress(progress_text(frames.len(), progress));
    let mut stalls = 0u32;
    // Measured page movement between consecutive kept frames, in pixels, when the oracle knew.
    let mut scrolls: Vec<Option<i64>> = Vec::new();

    let reason = 'scrolling: loop {
        if let Some(reason) = ctl.before_scroll(frames.len()) {
            break reason;
        }
        if progress.is_some_and(|p| p.at_end()) {
            break StopReason::ReachedEnd;
        }
        if covered_pt >= job.max_height_pt {
            break StopReason::LimitReached;
        }
        let delta = -((job.rect.3 * SCROLL_FRACTION * capture_dpr).round() as i32);
        dispatch_scroll(driver, delta)?;
        let Some((mut frame, mut dpr)) = settled_frame(driver, ctl, job.rect)? else {
            break StopReason::Stopped;
        };
        let now = if oracle_trusted { driver.progress(job.rect) } else { None };
        let moved = match (progress, now) {
            (Some(_), Some(_)) => progress_advanced(progress, now),
            _ => !frames.last().is_some_and(|prev| frames_identical(prev, &frame)),
        };

        if now.is_some() && progress.is_some() {
            if !moved {
                stalls += 1;
                if now.is_some_and(|p| p.at_end()) {
                    break StopReason::ReachedEnd;
                }
                if stalls >= STALL_LIMIT {
                    break StopReason::Stalled;
                }
                continue; // the pixels may have changed (a nested scroller): don't keep them
            }
        } else if !moved {
            // No oracle and nothing changed: believe it is the end only after waiting longer,
            // twice, in case the page was merely slow.
            let mut wait = END_RETRY_BASE;
            let mut changed = false;
            for _ in 0..END_RETRIES {
                wait *= 2;
                if !ctl.pause(wait) {
                    break 'scrolling StopReason::Stopped;
                }
                let Some((f, d)) = settled_frame(driver, ctl, job.rect)? else {
                    break 'scrolling StopReason::Stopped;
                };
                let differs = !frames.last().is_some_and(|prev| frames_identical(prev, &f));
                frame = f;
                dpr = d;
                if differs {
                    changed = true;
                    break;
                }
            }
            if !changed {
                break StopReason::ReachedEnd;
            }
        }
        stalls = 0;
        capture_dpr = dpr;
        covered_pt = match now {
            Some(p) => p.offset + p.viewport,
            None => covered_pt + job.rect.3 * SCROLL_FRACTION,
        };
        scrolls.push(match (progress, now) {
            (Some(a), Some(b)) => Some(((b.offset - a.offset) * dpr).round() as i64),
            _ => None,
        });
        progress = now.or(progress);
        frames.push(frame);
        ctl.progress(progress_text(frames.len(), progress));
    };
    log::info!(
        "scrolling capture ended: {} ({} frame(s), {:.0} pt)",
        reason.describe(),
        frames.len(),
        covered_pt
    );

    anyhow::ensure!(!frames.is_empty(), "no frames captured");
    let nominal_scroll = (job.rect.3 * SCROLL_FRACTION * capture_dpr).round() as i64;
    let stitched = stitch(frames, &scrolls, nominal_scroll);
    let (width, height) = stitched.dimensions();
    Ok(Outcome {
        shot: Shot {
            rgba: stitched.into_raw(),
            width,
            height,
            dpr: capture_dpr,
            title: String::new(),
            browser: Default::default(),
        },
        reason,
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

// ---- stitching (see `stitch`) --------------------------------------------------

fn frames_identical(a: &RgbaImage, b: &RgbaImage) -> bool {
    a.dimensions() == b.dimensions()
        && mad(&band_gray(a, 0, a.height()), &band_gray(b, 0, b.height())) < 1.0
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_capture_scrolls_only_while_the_pointer_is_its_own() {
        let job = Job::new((100.0, 100.0, 800.0, 600.0));
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

        let outcome = capture_with_driver(Job::new((0.0, 0.0, w as f64, view_h as f64)), &mut driver)
            .expect("synthetic capture should complete");
        assert_eq!(outcome.reason, StopReason::ReachedEnd);
        let shot = outcome.shot;

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
        struct StopNow;
        impl LoopControl for StopNow {
            fn before_scroll(&mut self, _: usize) -> Option<StopReason> {
                Some(StopReason::Stopped)
            }
            fn pause(&mut self, _: Duration) -> bool {
                true
            }
            fn progress(&mut self, _: String) {}
        }
        let outcome = capture_loop(&Job::new((0.0, 0.0, w as f64, view_h as f64)), &mut driver, &mut StopNow)
            .expect("an early stop should still produce the captured content");
        assert_eq!(outcome.reason, StopReason::Stopped);
        let shot = outcome.shot;

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

        let error = match capture_with_driver(Job::new((0.0, 0.0, 64.0, 64.0)), &mut driver) {
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

    /// A tall page behind a viewport, with the behaviours the loop must survive: slow
    /// settling, an oracle, an animation at the bottom.
    struct Page {
        page: RgbaImage,
        offset: u32,
        view_h: u32,
        /// Reads after a scroll that still show the previous position (needs several reads).
        reads_to_settle: usize,
        stale_reads: usize,
        shown_offset: u32,
        oracle: bool,
        /// Pixels keep changing once at the bottom, as an animation would.
        animate_at_end: bool,
        ticks: u32,
        /// The oracle never moves (scrolling is stuck).
        frozen: bool,
        /// The oracle reports a document exactly as tall as the viewport (a browser that does
        /// not expose the full page extent), so it claims to be at the end from the start.
        oracle_blind: bool,
        /// Fixed header and footer drawn over every frame.
        bars: Option<(RgbaImage, RgbaImage)>,
    }

    fn textured(w: u32, h: u32) -> RgbaImage {
        textured_seed(w, h, 0)
    }

    fn textured_seed(w: u32, h: u32, seed: u32) -> RgbaImage {
        // A hash of the position: no row repeats, as on a real page.
        RgbaImage::from_fn(w, h, |x, y| {
            let mut v = x.wrapping_mul(0x9E37_79B1) ^ y.wrapping_mul(0x85EB_CA6B) ^ seed.wrapping_mul(0xC2B2_AE35);
            v ^= v >> 15;
            v = v.wrapping_mul(0x2C1B_3C6D);
            v ^= v >> 12;
            image::Rgba([(v & 0xff) as u8, ((v >> 8) & 0xff) as u8, ((v >> 16) & 0xff) as u8, 255])
        })
    }

    impl Page {
        fn new(w: u32, page_h: u32, view_h: u32) -> Self {
            Self {
                page: textured(w, page_h),
                offset: 0,
                view_h,
                reads_to_settle: 0,
                stale_reads: 0,
                shown_offset: 0,
                oracle: false,
                animate_at_end: false,
                ticks: 0,
                frozen: false,
                oracle_blind: false,
                bars: None,
            }
        }
        fn at_end(&self) -> bool {
            self.offset + self.view_h >= self.page.height()
        }
    }

    impl ScrollDriver for Page {
        fn capture(&mut self, _: (f64, f64, f64, f64)) -> anyhow::Result<(RgbaImage, f64)> {
            // A read during smooth scrolling shows a half-moved position.
            let off = if self.stale_reads > 0 {
                // Each read is further along than the last, as a smooth scroll would be.
                let step = (self.reads_to_settle - self.stale_reads + 1) as u32;
                self.stale_reads -= 1;
                self.shown_offset + (self.offset - self.shown_offset) * step / (self.reads_to_settle as u32 + 1)
            } else {
                self.shown_offset = self.offset;
                self.offset
            };
            let mut img =
                image::imageops::crop_imm(&self.page, 0, off, self.page.width(), self.view_h).to_image();
            if let Some((header, footer)) = &self.bars {
                image::imageops::replace(&mut img, header, 0, 0);
                image::imageops::replace(&mut img, footer, 0, (self.view_h - footer.height()) as i64);
            }
            if self.animate_at_end && self.at_end() {
                self.ticks += 1;
                img.put_pixel(0, 0, image::Rgba([(self.ticks % 251) as u8, 1, 2, 255]));
                for x in 0..img.width() {
                    img.put_pixel(x, 1, image::Rgba([(self.ticks * 7 % 251) as u8, 9, 9, 255]));
                }
            }
            Ok((img, 1.0))
        }
        fn scroll(&mut self, delta: i32) -> anyhow::Result<()> {
            if self.frozen {
                return Ok(());
            }
            self.shown_offset = self.offset;
            self.offset = (self.offset + delta.unsigned_abs()).min(self.page.height() - self.view_h);
            self.stale_reads = self.reads_to_settle;
            Ok(())
        }
        fn progress(&mut self, _: (f64, f64, f64, f64)) -> Option<Progress> {
            self.oracle.then(|| Progress {
                offset: if self.oracle_blind { 0.0 } else { self.offset as f64 },
                extent: if self.oracle_blind { self.view_h } else { self.page.height() } as f64,
                viewport: self.view_h as f64,
            })
        }
    }

    fn run(page: &mut Page, max_height_pt: f64) -> Outcome {
        let rect = (0.0, 0.0, page.page.width() as f64, page.view_h as f64);
        capture_with_driver(Job { rect, max_height_pt }, page).expect("capture")
    }

    #[test]
    fn a_page_that_needs_several_reads_to_settle_is_not_cut_short_or_torn() {
        let mut page = Page::new(64, 600, 200);
        page.reads_to_settle = 3;
        let expected = page.page.clone();
        let out = run(&mut page, DEFAULT_MAX_HEIGHT_PT);
        assert_eq!(out.reason, StopReason::ReachedEnd);
        assert_eq!(out.shot.height, 600);
        let got = RgbaImage::from_raw(out.shot.width, out.shot.height, out.shot.rgba).unwrap();
        assert_eq!(got, expected, "no half-moved frame may reach the stitch");
    }

    #[test]
    fn the_oracle_ending_the_page_wins_over_pixels_that_never_stop_changing() {
        let mut page = Page::new(64, 600, 200);
        page.oracle = true;
        page.animate_at_end = true;
        let out = run(&mut page, DEFAULT_MAX_HEIGHT_PT);
        assert_eq!(out.reason, StopReason::ReachedEnd);
        assert!(out.shot.height >= 600);
    }

    #[test]
    fn without_an_oracle_an_unchanged_page_ends_after_the_retries() {
        let mut page = Page::new(64, 600, 200);
        let out = run(&mut page, DEFAULT_MAX_HEIGHT_PT);
        assert_eq!(out.reason, StopReason::ReachedEnd);
        assert!(!out.reason.is_incomplete());
    }

    #[test]
    fn a_page_taller_than_the_limit_ends_at_the_limit_and_says_so() {
        let mut page = Page::new(64, 20_000, 200);
        let out = run(&mut page, 1000.0);
        assert_eq!(out.reason, StopReason::LimitReached);
        assert!(out.reason.is_incomplete());
        assert!(out.reason.describe().contains("limit"));
        assert!(out.shot.height >= 1000 && out.shot.height < 1400, "{}", out.shot.height);
    }

    #[test]
    fn an_oracle_that_says_more_remains_but_never_moves_is_a_stall() {
        let mut page = Page::new(64, 2000, 200);
        page.oracle = true;
        page.frozen = true;
        let out = run(&mut page, DEFAULT_MAX_HEIGHT_PT);
        assert_eq!(out.reason, StopReason::Stalled);
    }

    #[test]
    fn an_oracle_that_claims_the_end_before_any_scroll_is_not_believed() {
        let mut page = Page::new(48, 900, 300);
        page.oracle = true;
        page.oracle_blind = true;
        let expected = page.page.clone();
        let out = run(&mut page, DEFAULT_MAX_HEIGHT_PT);
        assert_eq!(out.reason, StopReason::ReachedEnd);
        let got = RgbaImage::from_raw(out.shot.width, out.shot.height, out.shot.rgba).unwrap();
        assert!(got == expected, "captured {} of 900 px", got.height());
    }

    #[test]
    fn a_long_page_ends_once_with_no_repeated_band_at_the_bottom() {
        // 1 600 px viewport, 3 300 px page: the loop's last scroll is partial.
        for oracle in [false, true] {
            let mut page = Page::new(48, 3300, 1600);
            page.oracle = oracle;
            let expected = page.page.clone();
            let out = run(&mut page, DEFAULT_MAX_HEIGHT_PT);
            assert_eq!(out.reason, StopReason::ReachedEnd, "oracle {oracle}");
            let got = RgbaImage::from_raw(out.shot.width, out.shot.height, out.shot.rgba).unwrap();
            assert!(got == expected, "oracle {oracle}: height {}", got.height());
        }
    }

    #[test]
    fn a_fixed_header_and_footer_appear_once_in_a_captured_page() {
        for oracle in [false, true] {
            let mut page = Page::new(48, 3300, 1600);
            page.oracle = oracle;
            let header = textured_seed(48, 80, 11);
            let footer = textured_seed(48, 100, 12);
            page.bars = Some((header.clone(), footer.clone()));
            let doc = page.page.clone();
            let out = run(&mut page, DEFAULT_MAX_HEIGHT_PT);
            assert_eq!(out.reason, StopReason::ReachedEnd, "oracle {oracle}");
            let got = RgbaImage::from_raw(out.shot.width, out.shot.height, out.shot.rgba).unwrap();
            // header, the document rows the bars leave visible, footer
            assert_eq!(got.height(), 80 + (3300 - 100 - 80) + 100, "oracle {oracle}");
            assert!(image::imageops::crop_imm(&got, 0, 0, 48, 80).to_image() == header);
            assert!(image::imageops::crop_imm(&got, 0, 80, 48, 3120).to_image()
                == image::imageops::crop_imm(&doc, 0, 80, 48, 3120).to_image());
            assert!(image::imageops::crop_imm(&got, 0, 3200, 48, 100).to_image() == footer);
        }
    }

    #[test]
    fn progress_is_a_percentage_with_an_oracle_and_a_frame_count_without() {
        let p = Progress { offset: 400.0, extent: 1400.0, viewport: 400.0 };
        assert_eq!(progress_text(3, Some(p)), "40%");
        assert_eq!(progress_text(1, None), "1 frame");
        assert_eq!(progress_text(4, None), "4 frames");
    }

    #[test]
    fn escape_only_stops_the_current_capture() {
        request_stop();
        assert!(stopping());

        clear_stop();

        assert!(!stopping());
    }
}
