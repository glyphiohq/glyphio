//! Memory of a long scrolling capture, measured with a counting allocator.
//!
//! Its own test binary so nothing else allocates while it measures. The page is a long
//! article (text, code, pictures, blank space) drawn a frame at a time, never held whole.

use std::alloc::{GlobalAlloc, Layout, System};
use std::sync::atomic::{AtomicUsize, Ordering};

use glyphio_lib::capture::scroll::{capture_with_driver, Job, ScrollDriver, StopReason};
use glyphio_lib::platform::Progress;
use image::RgbaImage;

#[allow(dead_code)]
#[path = "../src/capture/scroll/fixtures.rs"]
mod fixtures;

use fixtures::{render, Article, Rows};

struct Counting;

static LIVE: AtomicUsize = AtomicUsize::new(0);
static PEAK: AtomicUsize = AtomicUsize::new(0);

fn grew(n: usize) {
    let live = LIVE.fetch_add(n, Ordering::SeqCst) + n;
    PEAK.fetch_max(live, Ordering::SeqCst);
}

// SAFETY: every call is forwarded to the system allocator unchanged; only sizes are counted.
unsafe impl GlobalAlloc for Counting {
    unsafe fn alloc(&self, layout: Layout) -> *mut u8 {
        let p = unsafe { System.alloc(layout) };
        if !p.is_null() {
            grew(layout.size());
        }
        p
    }
    unsafe fn alloc_zeroed(&self, layout: Layout) -> *mut u8 {
        let p = unsafe { System.alloc_zeroed(layout) };
        if !p.is_null() {
            grew(layout.size());
        }
        p
    }
    unsafe fn dealloc(&self, ptr: *mut u8, layout: Layout) {
        unsafe { System.dealloc(ptr, layout) };
        LIVE.fetch_sub(layout.size(), Ordering::SeqCst);
    }
    unsafe fn realloc(&self, ptr: *mut u8, layout: Layout, new_size: usize) -> *mut u8 {
        let p = unsafe { System.realloc(ptr, layout, new_size) };
        if !p.is_null() {
            LIVE.fetch_sub(layout.size(), Ordering::SeqCst);
            grew(new_size);
        }
        p
    }
}

#[global_allocator]
static ALLOCATOR: Counting = Counting;

/// A page behind a viewport at Retina scale, with a scroll-position oracle.
struct Viewport<'a> {
    page: &'a dyn Rows,
    offset: u32,
    view_h: u32,
    drawn: Option<(u32, RgbaImage)>,
}

impl ScrollDriver for Viewport<'_> {
    fn capture(&mut self, _: (f64, f64, f64, f64)) -> anyhow::Result<(RgbaImage, f64)> {
        if let Some((at, img)) = &self.drawn {
            if *at == self.offset {
                return Ok((img.clone(), 2.0));
            }
        }
        // Drop the last frame before drawing the next, as a capture would hand it over.
        self.drawn = None;
        let img = render(self.page, self.offset, self.view_h);
        self.drawn = Some((self.offset, img.clone()));
        Ok((img, 2.0))
    }
    fn scroll(&mut self, delta: i32) -> anyhow::Result<()> {
        self.offset = (self.offset + delta.unsigned_abs()).min(self.page.height() - self.view_h);
        Ok(())
    }
    fn progress(&mut self, _: (f64, f64, f64, f64)) -> Option<Progress> {
        Some(Progress {
            offset: self.offset as f64 / 2.0,
            extent: self.page.height() as f64 / 2.0,
            viewport: self.view_h as f64 / 2.0,
        })
    }
}

#[test]
fn a_30000_point_page_at_scale_2_stays_under_400_mb() {
    const MB: usize = 1024 * 1024;
    // 800 pt wide, a 900 pt viewport, 30 000 pt of page: 1 600 × 60 000 px. Held whole as
    // RGBA the result alone would be 366 MB, and every frame together 1.6 times that again.
    let (width, view_h) = (1600u32, 1800u32);
    let page = Article::new(width, 60_000, 2, 5);
    let raw = width as usize * 60_000 * 4;

    let before = LIVE.load(Ordering::SeqCst);
    PEAK.store(before, Ordering::SeqCst);
    let mut driver = Viewport { page: &page, offset: 0, view_h, drawn: None };
    let job = Job::new((0.0, 0.0, width as f64 / 2.0, view_h as f64 / 2.0));
    let out = capture_with_driver(job, &mut driver).expect("capture");
    drop(driver);
    let peak = PEAK.load(Ordering::SeqCst) - before;

    assert!(matches!(out.reason, StopReason::ReachedEnd | StopReason::LimitReached), "{:?}", out.reason);
    assert!(out.shot.height >= 59_000, "height {}", out.shot.height);
    let png = out.shot.png.as_ref().expect("encoded as it was taken");
    eprintln!(
        "peak {} MB for a {}×{} capture ({} MB as RGBA, {} MB as PNG)",
        peak / MB,
        out.shot.width,
        out.shot.height,
        raw / MB,
        png.len() / MB
    );
    assert!(peak < 400 * MB, "peak {} MB", peak / MB);
    // The page never existed uncompressed: the peak is a fraction of the raw image.
    assert!(peak < raw / 2, "peak {} MB against {} MB raw", peak / MB, raw / MB);
}
