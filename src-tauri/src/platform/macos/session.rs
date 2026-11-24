//! Whether the user is at the Mac: the window server's session dictionary says when the
//! screen is locked and when another user has the console (fast user switching).

use core_foundation::base::{CFType, TCFType};
use core_foundation::boolean::CFBoolean;
use core_foundation::dictionary::{CFDictionary, CFDictionaryRef};
use core_foundation::string::CFString;

#[link(name = "CoreGraphics", kind = "framework")]
extern "C" {
    fn CGSessionCopyCurrentDictionary() -> CFDictionaryRef;
}

/// True while the screen is locked or this session is not on the console. Unknown is
/// "not locked", so a missing dictionary never stops anything for good.
pub fn locked() -> bool {
    // SAFETY: takes no arguments; returns a +1 dictionary, or NULL outside a GUI session.
    let raw = unsafe { CGSessionCopyCurrentDictionary() };
    if raw.is_null() {
        return false;
    }
    // SAFETY: `raw` is a non-null CFDictionary we own (create rule).
    let dict: CFDictionary<CFString, CFType> = unsafe { CFDictionary::wrap_under_create_rule(raw) };
    let flag = |key: &'static str| {
        dict.find(CFString::from_static_string(key))
            .and_then(|v| v.downcast::<CFBoolean>())
            .map(bool::from)
    };
    flag("CGSSessionScreenIsLocked").unwrap_or(false) || flag("kCGSSessionOnConsoleKey") == Some(false)
}
