//! Diagnostic: run the exact scrollingPage front-half (frontmost window + AX web area)
//! and report every intermediate value. No pixels are captured unless --capture is given.
//! Run: cargo run --example page_probe [-- --capture]

use glyphio_lib::capture::diag;

fn main() {
    let capture = std::env::args().any(|a| a == "--capture");
    diag::page_probe(capture);
}
