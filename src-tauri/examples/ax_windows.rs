//! Diagnostic: dump every window (title, role, and all static text) of a pid via AX.
//! Run: cargo run --example ax_windows -- <pid>

use core_foundation::array::{CFArray, CFArrayGetTypeID, CFArrayRef};
use core_foundation::base::{CFType, CFTypeRef, TCFType};
use core_foundation::string::{CFString, CFStringRef};

type AXUIElementRef = CFTypeRef;

#[link(name = "ApplicationServices", kind = "framework")]
extern "C" {
    fn AXUIElementCreateApplication(pid: i32) -> AXUIElementRef;
    fn AXUIElementCopyAttributeValue(
        element: AXUIElementRef,
        attribute: CFStringRef,
        value: *mut CFTypeRef,
    ) -> i32;
}

fn attr(el: &CFType, name: &str) -> Option<CFType> {
    let a = CFString::new(name);
    let mut out: CFTypeRef = std::ptr::null();
    let err = unsafe { AXUIElementCopyAttributeValue(el.as_CFTypeRef(), a.as_concrete_TypeRef(), &mut out) };
    if err != 0 || out.is_null() { return None; }
    Some(unsafe { CFType::wrap_under_create_rule(out) })
}

fn s(el: &CFType, name: &str) -> String {
    attr(el, name).and_then(|v| v.downcast::<CFString>()).map(|v| v.to_string()).unwrap_or_default()
}

fn kids(el: &CFType) -> Vec<CFType> {
    let Some(v) = attr(el, "AXChildren") else { return Vec::new() };
    if v.type_of() != unsafe { CFArrayGetTypeID() } { return Vec::new(); }
    let arr: CFArray<*const std::ffi::c_void> =
        unsafe { CFArray::wrap_under_get_rule(v.as_CFTypeRef() as CFArrayRef) };
    arr.iter().filter(|i| !i.is_null())
        .map(|i| unsafe { CFType::wrap_under_get_rule(*i as CFTypeRef) })
        .collect()
}

fn dump_texts(el: &CFType, depth: usize) {
    if depth > 12 { return; }
    let role = s(el, "AXRole");
    if role == "AXStaticText" {
        let v = s(el, "AXValue");
        if !v.is_empty() { println!("    text: {v}"); }
    }
    for c in kids(el) { dump_texts(&c, depth + 1); }
}

fn main() {
    let pid: i32 = std::env::args().nth(1).and_then(|a| a.parse().ok()).expect("usage: ax_windows <pid>");
    let app = unsafe { AXUIElementCreateApplication(pid) };
    assert!(!app.is_null());
    let app = unsafe { CFType::wrap_under_create_rule(app) };
    let Some(wins) = attr(&app, "AXWindows") else { println!("no AXWindows"); return; };
    if wins.type_of() != unsafe { CFArrayGetTypeID() } { println!("AXWindows not an array"); return; }
    let arr: CFArray<*const std::ffi::c_void> =
        unsafe { CFArray::wrap_under_get_rule(wins.as_CFTypeRef() as CFArrayRef) };
    println!("{} window(s)", arr.len());
    for (i, item) in arr.iter().enumerate() {
        if item.is_null() { continue; }
        let w = unsafe { CFType::wrap_under_get_rule(*item as CFTypeRef) };
        println!("  [{i}] role={} subrole={} title='{}'", s(&w, "AXRole"), s(&w, "AXSubrole"), s(&w, "AXTitle"));
        dump_texts(&w, 0);
    }
}
