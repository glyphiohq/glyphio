//! What each character key types on the active keyboard layout, through the Text Input Sources
//! API and `UCKeyTranslate`. Global shortcuts are registered by virtual key code (the physical
//! key), so this is what turns `KeyQ` into the "A" an AZERTY user has to press.
//!
//! Text Input Sources must be used on the main thread on current macOS; the command that
//! calls this is synchronous, which Tauri runs there.

use std::ffi::c_void;

use core_foundation::base::{CFType, TCFType};
use core_foundation::data::{CFData, CFDataRef};
use core_foundation::string::CFStringRef;

#[link(name = "Carbon", kind = "framework")]
extern "C" {
    fn TISCopyCurrentKeyboardLayoutInputSource() -> *const c_void;
    fn TISGetInputSourceProperty(source: *const c_void, key: CFStringRef) -> *const c_void;
    static kTISPropertyUnicodeKeyLayoutData: CFStringRef;
    fn LMGetKbdType() -> u8;
    fn UCKeyTranslate(
        layout: *const c_void,
        virtual_key: u16,
        action: u16,
        modifier_state: u32,
        keyboard_type: u32,
        options: u32,
        dead_key_state: *mut u32,
        max_len: usize,
        actual_len: *mut usize,
        chars: *mut u16,
    ) -> i32;
}

const K_UC_KEY_ACTION_DISPLAY: u16 = 3;
const K_UC_KEY_TRANSLATE_NO_DEAD_KEYS: u32 = 1;

/// W3C `code` names and the ANSI virtual key codes (`kVK_ANSI_*`) of the keys a shortcut can
/// use whose meaning depends on the layout.
const KEYS: &[(&str, u16)] = &[
    ("KeyA", 0x00), ("KeyS", 0x01), ("KeyD", 0x02), ("KeyF", 0x03), ("KeyH", 0x04),
    ("KeyG", 0x05), ("KeyZ", 0x06), ("KeyX", 0x07), ("KeyC", 0x08), ("KeyV", 0x09),
    ("KeyB", 0x0B), ("KeyQ", 0x0C), ("KeyW", 0x0D), ("KeyE", 0x0E), ("KeyR", 0x0F),
    ("KeyY", 0x10), ("KeyT", 0x11), ("Digit1", 0x12), ("Digit2", 0x13), ("Digit3", 0x14),
    ("Digit4", 0x15), ("Digit6", 0x16), ("Digit5", 0x17), ("Equal", 0x18), ("Digit9", 0x19),
    ("Digit7", 0x1A), ("Minus", 0x1B), ("Digit8", 0x1C), ("Digit0", 0x1D),
    ("BracketRight", 0x1E), ("KeyO", 0x1F), ("KeyU", 0x20), ("BracketLeft", 0x21),
    ("KeyI", 0x22), ("KeyP", 0x23), ("KeyL", 0x25), ("KeyJ", 0x26), ("Quote", 0x27),
    ("KeyK", 0x28), ("Semicolon", 0x29), ("Backslash", 0x2A), ("Comma", 0x2B),
    ("Slash", 0x2C), ("KeyN", 0x2D), ("KeyM", 0x2E), ("Period", 0x2F), ("Backquote", 0x32),
];

pub fn key_labels() -> Option<Vec<(String, String)>> {
    // SAFETY: returns a +1 TISInputSourceRef (a CFType) or NULL.
    let source = unsafe { TISCopyCurrentKeyboardLayoutInputSource() };
    if source.is_null() {
        return None;
    }
    // SAFETY: non-null and owned (copy rule); released when `_source` drops.
    let _source = unsafe { CFType::wrap_under_create_rule(source as _) };
    // SAFETY: `source` is live; the property is borrowed (get rule) and may be NULL for
    // input methods without a Unicode layout.
    let data = unsafe { TISGetInputSourceProperty(source, kTISPropertyUnicodeKeyLayoutData) };
    if data.is_null() {
        return None;
    }
    // SAFETY: a borrowed CFData that lives as long as `_source`.
    let data = unsafe { CFData::wrap_under_get_rule(data as CFDataRef) };
    let layout = data.bytes().as_ptr() as *const c_void;
    // SAFETY: no arguments.
    let keyboard_type = u32::from(unsafe { LMGetKbdType() });

    let mut out = Vec::with_capacity(KEYS.len());
    for &(code, vk) in KEYS {
        let mut dead = 0u32;
        let mut chars = [0u16; 4];
        let mut len = 0usize;
        // SAFETY: `layout` points at the UCKeyboardLayout in `data`; the out-pointers are
        // valid for the lengths given.
        let status = unsafe {
            UCKeyTranslate(
                layout,
                vk,
                K_UC_KEY_ACTION_DISPLAY,
                0,
                keyboard_type,
                K_UC_KEY_TRANSLATE_NO_DEAD_KEYS,
                &mut dead,
                chars.len(),
                &mut len,
                chars.as_mut_ptr(),
            )
        };
        if status != 0 || len == 0 {
            continue;
        }
        let text = String::from_utf16_lossy(&chars[..len.min(chars.len())]);
        if !text.trim().is_empty() {
            out.push((code.to_string(), text));
        }
    }
    Some(out)
}
