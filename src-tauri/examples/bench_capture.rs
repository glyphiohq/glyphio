//! PERF-01 benchmark: the non-interactive capture modes through the real backend, then the
//! encode of what they returned. Needs Screen Recording permission for the terminal running it,
//! and a display. Run: cargo run --release --example bench_capture [N]
//! Interactive modes (snip, fullWindow, scrolling) are not benchmarked: a person is the cost.

use std::time::Instant;

use glyphio_lib::capture::{bench_capture, encode_png};

fn percentile(sorted: &[f64], p: f64) -> f64 {
    sorted[((sorted.len() - 1) as f64 * p).round() as usize]
}

fn main() {
    let n: usize = std::env::args().nth(1).and_then(|a| a.parse().ok()).unwrap_or(10);
    for mode in ["visible", "frontWindow"] {
        let (mut acquire, mut encode) = (Vec::new(), Vec::new());
        let mut size = (0, 0);
        for _ in 0..n {
            let t = Instant::now();
            let shot = match bench_capture(mode) {
                Ok(s) => s,
                Err(e) => {
                    eprintln!("{mode}: capture failed: {e:#}");
                    std::process::exit(1);
                }
            };
            acquire.push(t.elapsed().as_secs_f64() * 1000.0);
            size = (shot.width, shot.height);
            let t = Instant::now();
            let _ = encode_png(&shot).expect("encode");
            encode.push(t.elapsed().as_secs_f64() * 1000.0);
        }
        for v in [&mut acquire, &mut encode] {
            v.sort_by(|a, b| a.partial_cmp(b).unwrap());
        }
        println!(
            "{mode} {}x{} n={n}: acquire p50={:.0} p90={:.0} ms | encode p50={:.0} p90={:.0} ms",
            size.0,
            size.1,
            percentile(&acquire, 0.5),
            percentile(&acquire, 0.9),
            percentile(&encode, 0.5),
            percentile(&encode, 0.9),
        );
    }
}
