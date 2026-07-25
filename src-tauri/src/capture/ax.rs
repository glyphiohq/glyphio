//! Accessibility-tree lookup of a browser window's web content area — "capture just the
//! page, like an extension would" without shipping an extension. Finds the frontmost
//! window's `AXWebArea` element and returns its global bounds in points.
//!
//! Works with any browser that exposes its page through Accessibility: Safari does natively;
//! Chromium-family browsers (Chrome, Edge, Arc, Brave) and Electron apps build their AX tree
//! on demand, which the `AXManualAccessibility` opt-in below requests. Uses the same
//! app-level Accessibility grant as scrolling capture — no new permission.

use core_foundation::array::{CFArray, CFArrayGetTypeID, CFArrayRef};
use core_foundation::base::{CFType, CFTypeRef, TCFType};
use core_foundation::boolean::CFBoolean;
use core_foundation::string::{CFString, CFStringRef};
use core_graphics::geometry::{CGPoint, CGSize};

type AXUIElementRef = CFTypeRef;
type AXValueRef = CFTypeRef;

// AXValueType constants (AXValue.h): kAXValueTypeCGPoint / kAXValueTypeCGSize.
const AX_VALUE_CGPOINT: u32 = 1;
const AX_VALUE_CGSIZE: u32 = 2;

#[link(name = "ApplicationServices", kind = "framework")]
extern "C" {
    fn AXUIElementCreateApplication(pid: i32) -> AXUIElementRef;
    fn AXUIElementCopyAttributeValue(
        element: AXUIElementRef,
        attribute: CFStringRef,
        value: *mut CFTypeRef,
    ) -> i32;
    fn AXUIElementSetAttributeValue(
        element: AXUIElementRef,
        attribute: CFStringRef,
        value: CFTypeRef,
    ) -> i32;
    fn AXValueGetValue(value: AXValueRef, value_type: u32, out: *mut std::ffi::c_void) -> bool;
}

/// Global bounds (points, top-left origin) of the web content area of `pid`'s focused
/// window, or `None` when the app exposes no web area (not a browser, AX denied, …).
pub(super) fn web_area_bounds(pid: i32) -> Option<(f64, f64, f64, f64)> {
    if pid <= 0 {
        return None;
    }
    let app = unsafe { AXUIElementCreateApplication(pid) };
    if app.is_null() {
        return None;
    }
    let app = unsafe { CFType::wrap_under_create_rule(app) };

    // Chromium/Electron expose web contents only after an assistive client opts in.
    // Harmless where unsupported (Safari ignores it).
    let manual = CFString::from_static_string("AXManualAccessibility");
    unsafe {
        AXUIElementSetAttributeValue(
            app.as_CFTypeRef(),
            manual.as_concrete_TypeRef(),
            CFBoolean::true_value().as_CFTypeRef(),
        );
    }

    // The freshly opted-in tree may need a beat to materialise — one retry covers it.
    for attempt in 0..2 {
        if attempt > 0 {
            std::thread::sleep(std::time::Duration::from_millis(250));
        }
        let window = copy_attr(&app, "AXFocusedWindow").or_else(|| copy_attr(&app, "AXMainWindow"));
        if let Some(found) = window.and_then(find_web_area) {
            return bounds(&found);
        }
    }
    None
}

/// Breadth-first search for the first `AXWebArea` role. Depth/node caps keep pathological
/// trees (virtualised tables, giant DOMs) from stalling the capture path.
fn find_web_area(root: CFType) -> Option<CFType> {
    const MAX_NODES: usize = 4000;
    const MAX_DEPTH: usize = 40;
    let mut queue = std::collections::VecDeque::from([(root, 0usize)]);
    let mut visited = 0usize;
    while let Some((el, depth)) = queue.pop_front() {
        visited += 1;
        if visited > MAX_NODES {
            break;
        }
        if role(&el).as_deref() == Some("AXWebArea") {
            return Some(el);
        }
        if depth < MAX_DEPTH {
            for child in children(&el) {
                queue.push_back((child, depth + 1));
            }
        }
    }
    None
}

fn copy_attr(el: &CFType, name: &str) -> Option<CFType> {
    let attr = CFString::new(name);
    let mut out: CFTypeRef = std::ptr::null();
    let err = unsafe {
        AXUIElementCopyAttributeValue(el.as_CFTypeRef(), attr.as_concrete_TypeRef(), &mut out)
    };
    if err != 0 || out.is_null() {
        return None;
    }
    Some(unsafe { CFType::wrap_under_create_rule(out) })
}

fn role(el: &CFType) -> Option<String> {
    copy_attr(el, "AXRole")?.downcast::<CFString>().map(|s| s.to_string())
}

fn children(el: &CFType) -> Vec<CFType> {
    let Some(v) = copy_attr(el, "AXChildren") else {
        return Vec::new();
    };
    if v.type_of() != unsafe { CFArrayGetTypeID() } {
        return Vec::new();
    }
    let arr: CFArray<*const std::ffi::c_void> =
        unsafe { CFArray::wrap_under_get_rule(v.as_CFTypeRef() as CFArrayRef) };
    let mut out = Vec::with_capacity(arr.len() as usize);
    for item in arr.iter() {
        if !item.is_null() {
            out.push(unsafe { CFType::wrap_under_get_rule(*item as CFTypeRef) });
        }
    }
    out
}

fn bounds(el: &CFType) -> Option<(f64, f64, f64, f64)> {
    let pos = copy_attr(el, "AXPosition")?;
    let size = copy_attr(el, "AXSize")?;
    let mut p = CGPoint::new(0.0, 0.0);
    let mut s = CGSize::new(0.0, 0.0);
    let ok = unsafe {
        AXValueGetValue(pos.as_CFTypeRef(), AX_VALUE_CGPOINT, &mut p as *mut _ as *mut _)
            && AXValueGetValue(size.as_CFTypeRef(), AX_VALUE_CGSIZE, &mut s as *mut _ as *mut _)
    };
    // Reject degenerate areas (collapsed panes, background tabs mid-layout).
    if !ok || s.width < 40.0 || s.height < 40.0 {
        return None;
    }
    Some((p.x, p.y, s.width, s.height))
}
