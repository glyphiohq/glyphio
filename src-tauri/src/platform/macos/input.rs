//! Synthetic input: scroll wheel events and cursor reads/moves (Quartz).

use anyhow::anyhow;
use core_graphics::display::CGDisplay;
use core_graphics::event::{CGEvent, CGEventTapLocation, ScrollEventUnit};
use core_graphics::event_source::{CGEventSource, CGEventSourceStateID};
use core_graphics::geometry::CGPoint;

pub fn cursor_position() -> Option<(f64, f64)> {
    let source = CGEventSource::new(CGEventSourceStateID::CombinedSessionState).ok()?;
    let p = CGEvent::new(source).ok()?.location();
    Some((p.x, p.y))
}

pub fn warp_cursor(p: (f64, f64)) {
    let _ = CGDisplay::warp_mouse_cursor_position(CGPoint::new(p.0, p.1));
}

/// Post a pixel-unit scroll (negative = content moves up / scrolls down).
pub fn post_scroll(delta: i32) -> anyhow::Result<()> {
    let source = CGEventSource::new(CGEventSourceStateID::CombinedSessionState)
        .map_err(|_| anyhow!("could not create event source"))?;
    let ev = CGEvent::new_scroll_event(source, ScrollEventUnit::PIXEL, 1, delta, 0, 0)
        .map_err(|_| anyhow!("could not create scroll event"))?;
    ev.post(CGEventTapLocation::HID);
    Ok(())
}
