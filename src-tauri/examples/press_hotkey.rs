//! Diagnostic: post a global hotkey via CGEvent.
//! Run: cargo run --example press_hotkey -- <keycode> [modifiers…]
//!   keycodes: 35 = P, 37 = L, 11 = B, 49 = Space, 36 = Return
//!   modifiers: alt shift cmd ctrl — default `alt shift`, matching Glyphio's own defaults.
//!
//! Synthesised keys have to come from here rather than System Events: the global-shortcut
//! registration doesn't see AppleScript's `key code`, which is why a palette driven that way
//! looks like it is ignoring you.

use core_graphics::event::{CGEvent, CGEventFlags, CGEventTapLocation};
use core_graphics::event_source::{CGEventSource, CGEventSourceStateID};

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let keycode: u16 = args.first().and_then(|a| a.parse().ok()).unwrap_or(35);
    let named: Vec<&str> = args.iter().skip(1).map(String::as_str).collect();
    let names: &[&str] = if named.is_empty() { &["alt", "shift"] } else { &named };

    let mut flags = CGEventFlags::empty();
    for name in names {
        flags |= match *name {
            "alt" | "option" => CGEventFlags::CGEventFlagAlternate,
            "shift" => CGEventFlags::CGEventFlagShift,
            "cmd" | "command" => CGEventFlags::CGEventFlagCommand,
            "ctrl" | "control" => CGEventFlags::CGEventFlagControl,
            "none" => CGEventFlags::empty(),
            other => panic!("unknown modifier: {other}"),
        };
    }

    let src = CGEventSource::new(CGEventSourceStateID::CombinedSessionState).expect("event source");
    let down = CGEvent::new_keyboard_event(src.clone(), keycode, true).expect("key down");
    down.set_flags(flags);
    down.post(CGEventTapLocation::HID);
    std::thread::sleep(std::time::Duration::from_millis(60));
    let up = CGEvent::new_keyboard_event(src, keycode, false).expect("key up");
    up.set_flags(flags);
    up.post(CGEventTapLocation::HID);
    println!("posted keycode {keycode} with {}", names.join("+"));
}
