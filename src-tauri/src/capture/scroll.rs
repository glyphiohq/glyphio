//! Scrolling capture: select a region (a whole page, or just a scrollable panel inside one),
//! then the app repeatedly captures that exact rect while injecting scroll-wheel events at its
//! centre, stitches the frames on their overlap as they arrive, and hands the tall result to the
//! editor. Rows are encoded as they are stitched, so only a few frames and the compressed image
//! are ever in memory, and every frame comes from one capture session.
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
use png_stream::PngStream;
use stitch::{band_gray, mad, SizeChanged, Stitcher};
use crate::platform::{Progress, RectSession};

#[cfg(test)]
mod fixtures;
mod png_stream;
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

/// The platform's capturer and input, with one capture session for the whole capture.
#[derive(Default)]
struct PlatformDriver {
    session: Reused,
}

impl ScrollDriver for PlatformDriver {
    fn capture(&mut self, rect: (f64, f64, f64, f64)) -> anyhow::Result<(RgbaImage, f64)> {
        self.session.frame(|| crate::platform::current().capturer.rect_session(rect))
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

/// One capture session reused for every frame of a capture.
///
/// Deciding what to capture (which displays the region falls on, which windows to leave out)
/// is a round trip to the window server that costs more than taking the picture, and the loop
/// takes several pictures per scroll while the page settles. So it is done once, when the first
/// frame is asked for. If a frame fails (a display was unplugged or rearranged mid-capture) the
/// session is begun again once before the error is believed.
#[derive(Default)]
struct Reused {
    session: Option<Box<dyn RectSession + 'static>>,
}

impl Reused {
    fn frame(
        &mut self,
        begin: impl Fn() -> anyhow::Result<Box<dyn RectSession + 'static>>,
    ) -> anyhow::Result<(RgbaImage, f64)> {
        let fresh = self.session.is_none();
        let session = match &mut self.session {
            Some(s) => s,
            slot => slot.insert(begin()?),
        };
        match session.frame() {
            Ok(frame) => Ok(frame),
            Err(e) if fresh => Err(e),
            Err(e) => {
                log::info!("scrolling capture: frame failed ({e:#}); beginning a new capture session");
                self.session = None;
                let session = self.session.insert(begin()?);
                session.frame()
            }
        }
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

    let mut driver = PlatformDriver::default();
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
    /// The region came back at another size (a resize, a move to a display of another scale).
    Resized,
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
            StopReason::Resized => "stopped because the captured region changed size",
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
    // An oracle that already claims the end before anything has scrolled is describing the
    // viewport, not the page (some browsers expose no document extent). Believing it would
    // return the visible screen alone, so such an oracle is ignored for the whole capture and
    // the pixels decide.
    let mut progress = driver.progress(job.rect).filter(|p| !p.at_end());
    let oracle_trusted = progress.is_some();
    let (first, mut capture_dpr) = driver.capture(job.rect)?;
    // Height gathered so far in points, the basis for the height limit.
    let mut covered_pt = first.height() as f64 / capture_dpr;
    // How far each scroll asks the page to move, in pixels, and so the expected overlap.
    let nominal_scroll = (job.rect.3 * SCROLL_FRACTION * capture_dpr).round() as i64;
    // Frames are joined as they come and their fresh rows encoded straight away: only the last
    // couple of frames and the compressed image are ever held.
    let mut stitcher = Stitcher::new(PngStream::new(first.width()), nominal_scroll);
    stitcher.push(first, None)?;
    ctl.progress(progress_text(stitcher.frames(), progress));
    let mut stalls = 0u32;

    let reason = 'scrolling: loop {
        if let Some(reason) = ctl.before_scroll(stitcher.frames()) {
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
            _ => !stitcher.last().is_some_and(|prev| frames_identical(prev, &frame)),
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
                let differs = !stitcher.last().is_some_and(|prev| frames_identical(prev, &f));
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
        // Measured page movement since the last kept frame, in pixels, when the oracle knew.
        let scroll_px = match (progress, now) {
            (Some(a), Some(b)) => Some(((b.offset - a.offset) * dpr).round() as i64),
            _ => None,
        };
        match stitcher.push(frame, scroll_px) {
            Ok(()) => {}
            // Rows of another width cannot join the image: keep what is there and say why.
            Err(e) if e.is::<SizeChanged>() => break StopReason::Resized,
            Err(e) => return Err(e),
        }
        stalls = 0;
        capture_dpr = dpr;
        covered_pt = match now {
            Some(p) => p.offset + p.viewport,
            None => covered_pt + job.rect.3 * SCROLL_FRACTION,
        };
        progress = now.or(progress);
        ctl.progress(progress_text(stitcher.frames(), progress));
    };
    let frames = stitcher.frames();
    log::info!(
        "scrolling capture ended: {} ({frames} frame(s), {covered_pt:.0} pt)",
        reason.describe(),
    );

    let png = stitcher.finish()?;
    let width = png.width();
    let (png, height) = png.finish()?;
    Ok(Outcome {
        shot: Shot {
            rgba: Vec::new(),
            png: Some(png),
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

    /// The pixels of a finished capture, decoded from the PNG it carries.
    fn pixels(shot: &Shot) -> RgbaImage {
        let png = shot.png.as_ref().expect("a scrolling capture arrives encoded");
        let img = image::load_from_memory_with_format(png, image::ImageFormat::Png).expect("valid PNG").to_rgba8();
        assert_eq!(img.dimensions(), (shot.width, shot.height), "reported size");
        img
    }

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
        let stitched = pixels(&shot);
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
            pixels(&shot),
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
        let got = pixels(&out.shot);
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
        let got = pixels(&out.shot);
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
            let got = pixels(&out.shot);
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
            let got = pixels(&out.shot);
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

    // ---- pages that look like real ones (see `fixtures`) --------------------------------

    use fixtures::{render, whole, Article, Chrome, Feed, Rows};

    /// A fixture page behind a viewport, at a Retina scale, optionally with fixed chrome.
    struct Fixture<'a> {
        page: &'a dyn Rows,
        chrome: Option<&'a Chrome>,
        offset: u32,
        view_h: u32,
        dpr: f64,
        oracle: bool,
        /// The last frame drawn and its offset: the loop reads each position several times.
        drawn: Option<(u32, RgbaImage)>,
    }

    impl<'a> Fixture<'a> {
        fn new(page: &'a dyn Rows, view_h: u32, oracle: bool) -> Self {
            Self { page, chrome: None, offset: 0, view_h, dpr: 2.0, oracle, drawn: None }
        }

        fn job(&self) -> Job {
            Job::new((0.0, 0.0, self.page.width() as f64 / self.dpr, self.view_h as f64 / self.dpr))
        }

        /// What the stitched result should be: the page with the chrome where the first and
        /// last frames show it.
        fn expected(&self) -> RgbaImage {
            let mut img = whole(self.page);
            if let Some(c) = self.chrome {
                let (w, h) = img.dimensions();
                let last_top = h - self.view_h;
                let stride = w as usize * 4;
                for (y, row) in img.chunks_exact_mut(stride).enumerate() {
                    let y = y as u32;
                    if y < c.header {
                        c.row(y, self.view_h, w, row);
                    } else if y >= h - c.footer {
                        c.row(y - last_top, self.view_h, w, row);
                    }
                }
            }
            img
        }
    }

    impl ScrollDriver for Fixture<'_> {
        fn capture(&mut self, _: (f64, f64, f64, f64)) -> anyhow::Result<(RgbaImage, f64)> {
            if let Some((at, img)) = &self.drawn {
                if *at == self.offset {
                    return Ok((img.clone(), self.dpr));
                }
            }
            let mut img = render(self.page, self.offset, self.view_h);
            if let Some(c) = self.chrome {
                let w = img.width();
                for (vy, row) in img.chunks_exact_mut(w as usize * 4).enumerate() {
                    if (vy as u32) < c.header || vy as u32 >= self.view_h - c.footer || c.sidebar > 0 {
                        c.row(vy as u32, self.view_h, w, row);
                    }
                }
            }
            self.drawn = Some((self.offset, img.clone()));
            Ok((img, self.dpr))
        }
        fn scroll(&mut self, delta: i32) -> anyhow::Result<()> {
            self.offset = (self.offset + delta.unsigned_abs()).min(self.page.height() - self.view_h);
            Ok(())
        }
        fn progress(&mut self, _: (f64, f64, f64, f64)) -> Option<Progress> {
            self.oracle.then(|| Progress {
                offset: self.offset as f64 / self.dpr,
                extent: self.page.height() as f64 / self.dpr,
                viewport: self.view_h as f64 / self.dpr,
            })
        }
    }

    fn capture_fixture(f: &mut Fixture) -> RgbaImage {
        let out = capture_with_driver(f.job(), f).expect("capture");
        assert_eq!(out.reason, StopReason::ReachedEnd);
        assert_eq!(out.shot.dpr, 2.0);
        pixels(&out.shot)
    }

    /// Where two images first differ, for a readable failure.
    fn first_difference(got: &RgbaImage, want: &RgbaImage) -> String {
        if got.dimensions() != want.dimensions() {
            return format!("size {:?}, expected {:?}", got.dimensions(), want.dimensions());
        }
        let stride = got.width() as usize * 4;
        let row = got.chunks_exact(stride).zip(want.chunks_exact(stride)).position(|(a, b)| a != b);
        format!("first differing row {row:?}")
    }

    #[test]
    fn a_long_article_with_blank_space_and_code_stitches_exactly() {
        // 320 pt wide at 2x (a reading column), a 750 pt viewport: paragraphs, headings, code, pictures, rules and
        // runs of blank rows longer than the overlap probe.
        for seed in [1, 3] {
            let page = Article::new(640, 6000, 2, seed);
            for oracle in [true, false] {
                let mut f = Fixture::new(&page, 1500, oracle);
                let want = f.expected();
                let got = capture_fixture(&mut f);
                assert!(got == want, "seed {seed}, oracle {oracle}: {}", first_difference(&got, &want));
            }
        }
    }

    #[test]
    fn a_feed_of_identical_cards_keeps_every_card_once() {
        // The card layout repeats every 288 px; only the text tells cards apart, so a join
        // that slips by one card would still match the chrome.
        let page = Feed::new(640, 24, 2);
        for oracle in [true, false] {
            let mut f = Fixture::new(&page, 1400, oracle);
            let want = f.expected();
            let got = capture_fixture(&mut f);
            assert!(got == want, "oracle {oracle}: {}", first_difference(&got, &want));
        }
    }

    #[test]
    fn a_chat_window_keeps_its_header_and_composer_once() {
        let page = Article::new(640, 5000, 2, 7);
        let chrome = Chrome::new(112, 136, 0, 2);
        for oracle in [true, false] {
            let mut f = Fixture::new(&page, 1500, oracle);
            f.chrome = Some(&chrome);
            let want = f.expected();
            let got = capture_fixture(&mut f);
            assert!(got == want, "oracle {oracle}: {}", first_difference(&got, &want));
        }
    }

    #[test]
    fn a_fixed_sidebar_does_not_disturb_the_join_of_the_pane_beside_it() {
        let page = Article::new(720, 5000, 2, 8);
        let chrome = Chrome::new(112, 136, 180, 2);
        for oracle in [true, false] {
            let mut f = Fixture::new(&page, 1500, oracle);
            f.chrome = Some(&chrome);
            let want = f.expected();
            let got = capture_fixture(&mut f);
            assert_eq!(got.dimensions(), want.dimensions(), "oracle {oracle}");
            // The sidebar itself repeats down the image (a known limitation); the pane beside
            // it is exact.
            let pane = |img: &RgbaImage| image::imageops::crop_imm(img, 180, 112, 540, img.height() - 248).to_image();
            assert!(pane(&got) == pane(&want), "oracle {oracle}: {}", first_difference(&pane(&got), &pane(&want)));
        }
    }

    // ---- one capture session per capture ------------------------------------------------

    /// Counts sessions begun and frames taken; fails the frames it is told to.
    #[derive(Default)]
    struct Sessions {
        begun: std::cell::Cell<u32>,
        frames: std::rc::Rc<std::cell::Cell<u32>>,
        fail_on: std::rc::Rc<std::cell::RefCell<Vec<u32>>>,
    }

    struct CountingSession {
        frames: std::rc::Rc<std::cell::Cell<u32>>,
        fail_on: std::rc::Rc<std::cell::RefCell<Vec<u32>>>,
    }

    impl RectSession for CountingSession {
        fn frame(&mut self) -> anyhow::Result<(RgbaImage, f64)> {
            let n = self.frames.get() + 1;
            self.frames.set(n);
            if self.fail_on.borrow().contains(&n) {
                anyhow::bail!("display went away");
            }
            Ok((RgbaImage::new(4, 4), 2.0))
        }
    }

    impl Sessions {
        fn begin(&self) -> anyhow::Result<Box<dyn RectSession + 'static>> {
            self.begun.set(self.begun.get() + 1);
            Ok(Box::new(CountingSession { frames: self.frames.clone(), fail_on: self.fail_on.clone() }))
        }
    }

    #[test]
    fn every_frame_of_a_capture_comes_from_one_session() {
        let s = Sessions::default();
        let mut reused = Reused::default();
        for _ in 0..25 {
            reused.frame(|| s.begin()).unwrap();
        }
        assert_eq!((s.begun.get(), s.frames.get()), (1, 25));
    }

    #[test]
    fn a_failed_frame_begins_the_session_again_once() {
        let s = Sessions::default();
        let mut reused = Reused::default();
        reused.frame(|| s.begin()).unwrap();
        // A display is unplugged: the next frame fails, a new session takes it.
        s.fail_on.borrow_mut().push(2);
        reused.frame(|| s.begin()).unwrap();
        assert_eq!((s.begun.get(), s.frames.get()), (2, 3));
        reused.frame(|| s.begin()).unwrap();
        assert_eq!(s.begun.get(), 2, "the new session is kept");
        // A failure the new session cannot fix either is reported, not retried forever.
        s.fail_on.borrow_mut().extend([5, 6]);
        assert!(reused.frame(|| s.begin()).is_err());
        assert_eq!(s.begun.get(), 3);
        // Nor does a session that fails on its very first frame get a second try.
        let fresh = Sessions::default();
        fresh.fail_on.borrow_mut().push(1);
        assert!(Reused::default().frame(|| fresh.begin()).is_err());
        assert_eq!(fresh.begun.get(), 1);
    }

    #[test]
    fn a_region_that_changes_size_ends_the_capture_with_what_it_has() {
        struct Shrinks(Page);
        impl ScrollDriver for Shrinks {
            fn capture(&mut self, r: (f64, f64, f64, f64)) -> anyhow::Result<(RgbaImage, f64)> {
                let (img, dpr) = self.0.capture(r)?;
                // The window is resized once the page has scrolled twice.
                if self.0.offset >= 240 {
                    return Ok((image::imageops::crop_imm(&img, 0, 0, 40, img.height()).to_image(), dpr));
                }
                Ok((img, dpr))
            }
            fn scroll(&mut self, d: i32) -> anyhow::Result<()> {
                self.0.scroll(d)
            }
        }
        let mut d = Shrinks(Page::new(64, 2000, 200));
        let out = capture_with_driver(Job::new((0.0, 0.0, 64.0, 200.0)), &mut d).unwrap();
        assert_eq!(out.reason, StopReason::Resized);
        assert!(out.reason.is_incomplete());
        let got = pixels(&out.shot);
        // The first two frames: 200 rows, then 120 fresh ones.
        assert_eq!(got.dimensions(), (64, 320));
        assert!(got == image::imageops::crop_imm(&d.0.page, 0, 0, 64, 320).to_image());
    }

}
