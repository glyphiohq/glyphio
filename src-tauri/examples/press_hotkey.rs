//! Diagnostic: post a global hotkey (keycode + option/shift modifiers) via CGEvent.
//! Run: cargo run --example press_hotkey -- <keycode>   (35 = P, 37 = L, 11 = B)

use core_graphics::event::{CGEvent, CGEventFlags, CGEventTapLocation};
use core_graphics::event_source::{CGEventSource, CGEventSourceStateID};

fn main() {
    let keycode: u16 = std::env::args().nth(1).and_then(|a| a.parse().ok()).unwrap_or(35);
    let src = CGEventSource::new(CGEventSourceStateID::CombinedSessionState).expect("event source");
    let flags = CGEventFlags::CGEventFlagAlternate | CGEventFlags::CGEventFlagShift;
    let down = CGEvent::new_keyboard_event(src.clone(), keycode, true).expect("key down");
    down.set_flags(flags);
    down.post(CGEventTapLocation::HID);
    std::thread::sleep(std::time::Duration::from_millis(60));
    let up = CGEvent::new_keyboard_event(src, keycode, false).expect("key up");
    up.set_flags(flags);
    up.post(CGEventTapLocation::HID);
    println!("posted keycode {keycode} with option+shift");
}
