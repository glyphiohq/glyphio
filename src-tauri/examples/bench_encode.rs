//! PERF-01 benchmark: PNG encoding of a synthetic 5K frame (5120x2880), no screen access.
//! Run (release numbers are the ones to quote): cargo run --release --example bench_encode [N]

use std::time::Instant;

use glyphio_lib::capture::{encode_png, Shot};

fn synthetic(width: u32, height: u32) -> Shot {
    // A gradient with periodic detail and a little noise: neither all-flat (unrealistically
    // fast) nor random (unrealistically slow), like a UI screenshot.
    let mut rgba = Vec::with_capacity((width * height * 4) as usize);
    let mut seed = 0x2545_f491u32;
    for y in 0..height {
        for x in 0..width {
            seed = seed.wrapping_mul(1_664_525).wrapping_add(1_013_904_223);
            let noise = ((seed >> 24) & 0x07) as u8;
            let stripe = if (x / 64 + y / 64) % 2 == 0 { 24 } else { 0 };
            rgba.extend_from_slice(&[
                ((x * 255 / width) as u8).saturating_add(stripe),
                ((y * 255 / height) as u8).saturating_add(noise),
                ((x ^ y) as u8) / 2,
                255,
            ]);
        }
    }
    Shot { rgba, png: None, width, height, dpr: 2.0, title: String::new(), browser: Default::default() }
}

fn percentile(sorted: &[f64], p: f64) -> f64 {
    sorted[((sorted.len() - 1) as f64 * p).round() as usize]
}

fn main() {
    let n: usize = std::env::args().nth(1).and_then(|a| a.parse().ok()).unwrap_or(10);
    let shot = synthetic(5120, 2880);
    let _ = encode_png(&shot).expect("warm-up encode");
    let mut ms = Vec::new();
    let mut bytes = 0;
    for _ in 0..n {
        let t = Instant::now();
        let png = encode_png(&shot).expect("encode");
        ms.push(t.elapsed().as_secs_f64() * 1000.0);
        bytes = png.len();
    }
    ms.sort_by(|a, b| a.partial_cmp(b).unwrap());
    println!("encode_png 5120x2880 n={n} bytes={bytes}");
    println!(
        "p50={:.0}ms p90={:.0}ms max={:.0}ms",
        percentile(&ms, 0.5),
        percentile(&ms, 0.9),
        ms[ms.len() - 1]
    );
}
