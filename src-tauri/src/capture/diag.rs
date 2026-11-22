//! Diagnostics for the frontmost-page capture pipeline (`examples/page_probe.rs`).
//! Prints every intermediate step of what `scrollingPage` does so failures can be
//! localised without the app's UI in the way.

use std::time::{Duration, Instant};

use crate::platform;

/// Run the scrollingPage front-half step by step; with `capture` also grab pixels and
/// run a short 3-frame scroll loop, saving PNGs next to the report for inspection.
pub fn page_probe(capture: bool) {
    println!("accessibility trusted: {}", platform::current().permissions.accessibility_trusted());
    println!("frontmost app: {:?}", platform::current().windows.frontmost_app());

    // The two ways of answering "which display is the user on" — one confirms itself against
    // the window list, one doesn't. Summoned windows use the cheap one; captures can't.
    // Both are timed repeatedly after a warm-up, because the first accessibility call of a
    // process pays for the connection and would otherwise be charged to whichever ran first.
    let _ = platform::current().windows.focused_window_display();
    let _ = platform::current().windows.frontmost_window();
    let bench = |label: &str, f: &dyn Fn() -> bool| {
        let mut best = Duration::MAX;
        let mut ok = true;
        for _ in 0..5 {
            let t = Instant::now();
            ok &= f();
            best = best.min(t.elapsed());
        }
        println!("placement {label}: ok={ok} best of 5 = {best:?}");
    };
    bench("no-window-list", &|| platform::current().windows.focused_window_display().is_some());
    bench("window-list", &|| platform::current().windows.frontmost_window().is_ok());

    let t = Instant::now();
    let win = match platform::current().windows.frontmost_window() {
        Ok(w) => w,
        Err(e) => {
            println!("frontmost_window_bounds FAILED after {:?}: {e}", t.elapsed());
            return;
        }
    };
    println!(
        "front window: '{}' pid={} app='{}' rect=({}, {}) {}x{} [{:?}]",
        win.title, win.pid, win.app_name, win.x, win.y, win.w, win.h, t.elapsed()
    );

    let t = Instant::now();
    println!("browser meta: {:?} [{:?}]", platform::current().inspector.browser_meta(win.pid, &win.app_name), t.elapsed());

    let t = Instant::now();
    match platform::current().inspector.page_geometry(win.pid, super::PAGE_TREE_BUDGET) {
        Some(g) => {
            let (wx, wy, ww, wh) = g.window;
            println!("ax window frame: ({wx}, {wy}) {ww}x{wh} [{:?}]", t.elapsed());
            match g.web_visible {
                Some((x, y, w, h)) => {
                    println!("web viewport: ({x}, {y}) {w}x{h}");
                    if capture {
                        probe_captures(x, y, w, h);
                    }
                }
                None => {
                    println!(
                        "web viewport: NONE (scrollingPage would use the window frame) \
                         [tree_still_building={}]",
                        g.tree_still_building
                    );
                    if capture {
                        probe_captures(wx, wy, ww, wh);
                    }
                }
            }
        }
        None => {
            println!("ax geometry: NONE after {:?} (fallback: CGWindow bounds)", t.elapsed());
            if capture {
                probe_captures(win.x, win.y, win.w, win.h);
            }
        }
    }
}

fn probe_captures(x: f64, y: f64, w: f64, h: f64) {
    let out = std::env::temp_dir();
    for i in 0..3 {
        let t = Instant::now();
        match platform::current().capturer.rect_image(x, y, w, h) {
            Ok((img, dpr)) => {
                let (pw, ph) = img.dimensions();
                let black = img
                    .pixels()
                    .filter(|p| p[0] < 8 && p[1] < 8 && p[2] < 8)
                    .count() as f64
                    / (pw as f64 * ph as f64);
                let path = out.join(format!("glyphio-page-probe-{i}.png"));
                let _ = img.save(&path);
                println!(
                    "frame {i}: {pw}x{ph} dpr={dpr:.2} black={:.1}% [{:?}] -> {}",
                    black * 100.0,
                    t.elapsed(),
                    path.display()
                );
            }
            Err(e) => {
                println!("frame {i}: capture FAILED after {:?}: {e}", t.elapsed());
                return;
            }
        }
        // The progress oracle (SCROLL-01): record per browser whether it answers and advances.
        match platform::current().inspector.scroll_progress((x + w / 2.0, y + h / 2.0)) {
            Some(p) => println!(
                "frame {i}: progress offset={:.0} extent={:.0} viewport={:.0} end={}",
                p.offset, p.extent, p.viewport, p.at_end()
            ),
            None => println!("frame {i}: progress NONE (pixel comparison only)"),
        }
        if i < 2 {
            if let Err(e) = scroll_once(x + w / 2.0, y + h / 2.0, (h * 0.6) as i32) {
                println!("scroll after frame {i} FAILED: {e}");
                return;
            }
            std::thread::sleep(std::time::Duration::from_millis(300));
        }
    }
}

fn scroll_once(cx: f64, cy: f64, amount: i32) -> anyhow::Result<()> {
    let input = &platform::current().input;
    input.warp_cursor((cx, cy));
    std::thread::sleep(std::time::Duration::from_millis(120));
    input.post_scroll(-amount)
}
