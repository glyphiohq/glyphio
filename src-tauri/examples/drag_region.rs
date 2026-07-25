//! Diagnostic: drag a rectangle with the mouse, to drive `screencapture -i` (the snip mode)
//! without a human at the trackpad.
//!
//! Run: cargo run --example drag_region -- <x1> <y1> <x2> <y2>

use core_graphics::event::{CGEvent, CGEventTapLocation, CGEventType, CGMouseButton};
use core_graphics::event_source::{CGEventSource, CGEventSourceStateID};
use core_graphics::geometry::CGPoint;

fn main() {
    let a: Vec<f64> = std::env::args().skip(1).filter_map(|s| s.parse().ok()).collect();
    let (x1, y1, x2, y2) = match a.as_slice() {
        [x1, y1, x2, y2] => (*x1, *y1, *x2, *y2),
        _ => (400.0, 300.0, 1100.0, 800.0),
    };
    let src = || CGEventSource::new(CGEventSourceStateID::CombinedSessionState).expect("source");
    let post = |kind, point, button| {
        CGEvent::new_mouse_event(src(), kind, point, button)
            .expect("mouse event")
            .post(CGEventTapLocation::HID);
        std::thread::sleep(std::time::Duration::from_millis(60));
    };

    post(CGEventType::MouseMoved, CGPoint::new(x1, y1), CGMouseButton::Left);
    post(CGEventType::LeftMouseDown, CGPoint::new(x1, y1), CGMouseButton::Left);
    // Several intermediate drags — screencapture tracks the selection as the mouse moves,
    // and a single jump can be missed entirely.
    for i in 1..=8 {
        let t = f64::from(i) / 8.0;
        post(
            CGEventType::LeftMouseDragged,
            CGPoint::new(x1 + (x2 - x1) * t, y1 + (y2 - y1) * t),
            CGMouseButton::Left,
        );
    }
    post(CGEventType::LeftMouseUp, CGPoint::new(x2, y2), CGMouseButton::Left);
    println!("dragged ({x1},{y1}) → ({x2},{y2})");
}
