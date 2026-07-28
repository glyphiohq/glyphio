//! Accessibility-tree lookup of a browser window's web content area — "capture just the
//! page, like an extension would" without shipping an extension. Finds the frontmost
//! window's `AXWebArea` element and returns its global bounds in points.
//!
//! Works with any browser that exposes its page through Accessibility. Safari does natively.
//! Chromium-family browsers (Chrome, Edge, Arc, Brave) and Electron apps only build their web
//! AX tree once an assistive client asks for it, and they disagree about how to ask:
//! Electron implements `AXManualAccessibility`, Chromium watches `AXEnhancedUserInterface`
//! (the attribute VoiceOver sets) and answers `kAXErrorNotImplemented` to the write while
//! still acting on it. Both are set, and the tree is then polled for — Chrome takes a
//! second or two to push the renderer's tree up to the browser process, so a single retry
//! never saw it. Uses the same app-level Accessibility grant as scrolling capture — no new
//! permission.

use std::collections::HashMap;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Mutex;
use std::time::{Duration, Instant};

use core_foundation::array::{CFArray, CFArrayGetTypeID, CFArrayRef};
use core_foundation::base::{CFType, CFTypeRef, TCFType};
use core_foundation::boolean::CFBoolean;
use core_foundation::string::{CFString, CFStringRef};
use core_foundation::url::CFURL;
use core_graphics::geometry::{CGPoint, CGSize};

type AXUIElementRef = CFTypeRef;
type AXValueRef = CFTypeRef;

// AXValueType constants (AXValue.h): kAXValueTypeCGPoint / kAXValueTypeCGSize.
const AX_VALUE_CGPOINT: u32 = 1;
const AX_VALUE_CGSIZE: u32 = 2;

/// Gap between tree polls while a lazily-built web tree materialises.
const POLL_INTERVAL: Duration = Duration::from_millis(120);
/// Grace for a tree that is already switched on: enough to cover a page mid-navigation,
/// far short of a cold start.
const SETTLE: Duration = Duration::from_millis(600);

#[link(name = "ApplicationServices", kind = "framework")]
extern "C" {
    fn AXUIElementCreateApplication(pid: i32) -> AXUIElementRef;
    fn AXUIElementCreateSystemWide() -> AXUIElementRef;
    fn AXUIElementGetPid(element: AXUIElementRef, pid: *mut i32) -> i32;
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

/// What the AX tree knows about `pid`'s focused window.
pub(super) struct PageGeometry {
    /// The focused window's frame (points, global top-left) — the authoritative window
    /// rect. CGWindowList can return sliver windows (Safari's toolbar strip is its own
    /// window on modern macOS); AX always describes the real focused window.
    pub window: (f64, f64, f64, f64),
    /// The VISIBLE part of the web content area, if the window hosts one. `AXWebArea`
    /// reports the full document extent (tens of thousands of points on a long page),
    /// so the raw bounds are intersected with the window frame to get the viewport.
    pub web_visible: Option<(f64, f64, f64, f64)>,
    /// This call switched the browser's accessibility tree on and the budget ran out before
    /// a web area appeared. Means "ask again in a moment", not "that isn't a web page" — the
    /// two failures look identical from the outside and read very differently to a user.
    pub tree_still_building: bool,
}

/// Read [`PageGeometry`] for `pid`'s focused window, or `None` when AX yields nothing.
///
/// `budget` is how long to keep waiting for a web area that isn't there yet: a browser that
/// has never had an assistive client attached takes ~2s to build its tree and push it to the
/// browser process. The wait is paid once per browser — see [`enable_web_accessibility`].
pub(super) fn page_geometry(pid: i32, budget: Duration) -> Option<PageGeometry> {
    if pid <= 0 {
        return None;
    }
    let app = unsafe { AXUIElementCreateApplication(pid) };
    if app.is_null() {
        return None;
    }
    let app = unsafe { CFType::wrap_under_create_rule(app) };

    // Free pass first: Safari, and any browser already opted in (by us moments ago, or by a
    // screen reader), answer at once.
    let mut geometry = read_geometry(&app);
    if geometry.as_ref().is_some_and(|g| g.web_visible.is_some()) {
        keep_warm(pid);
        return geometry;
    }

    // Only a tree we just switched on is worth waiting seconds for. One that was already on
    // has published everything it is going to: waiting the full budget on a window that has
    // no web area — a popup, a settings tab, a browser's own toolbar overlay — would make
    // "this isn't a page" take as long as the slowest cold start.
    let building = enable_web_accessibility(&app, pid);
    let deadline = Instant::now() + if building { budget } else { SETTLE };
    while Instant::now() < deadline {
        std::thread::sleep(POLL_INTERVAL);
        if let Some(g) = read_geometry(&app) {
            let found = g.web_visible.is_some();
            geometry = Some(g);
            if found {
                break;
            }
        }
    }
    if let Some(g) = geometry.as_mut() {
        g.tree_still_building = building && g.web_visible.is_none();
    }
    geometry
}

/// Apps whose accessibility tree *we* switched on, and when we may switch it back off.
static WARMED: Mutex<Option<HashMap<i32, Instant>>> = Mutex::new(None);
static COOLING: AtomicBool = AtomicBool::new(false);
/// How long an opted-in browser stays opted in after its last capture. Long enough that a
/// burst of captures pays the tree-building wait once; short enough that a browser isn't
/// left rendering accessibility trees because someone took a screenshot at lunchtime.
const WARM_GRACE: Duration = Duration::from_secs(120);
/// How often the cooldown thread checks for browsers to put back.
const COOL_TICK: Duration = Duration::from_secs(5);

/// Ask a Chromium/Electron app to build its web accessibility tree.
///
/// `AXEnhancedUserInterface` is a global, app-wide flag: it keeps a browser rendering an
/// accessibility tree for every tab, which is real memory and CPU we have no business
/// leaving behind. So every app we switch on is remembered and switched back off once the
/// captures stop ([`cool_down`]), and an app where it was already on — a screen reader is
/// running — is left strictly alone, permanently.
///
/// Reports whether this call is what switched the tree on, i.e. whether there is a build to
/// wait for. A tree that was already up needs no wait at all.
fn enable_web_accessibility(app: &CFType, pid: i32) -> bool {
    set_bool(app, "AXManualAccessibility", true); // Electron's opt-in; unsupported elsewhere
    let ours = keep_warm(pid);
    let already_on = copy_attr(app, "AXEnhancedUserInterface")
        .and_then(|v| v.downcast::<CFBoolean>())
        .is_some_and(|b| b.into());
    if !ours && already_on {
        return false; // somebody else's assistive session — not ours to turn off later
    }
    set_bool(app, "AXEnhancedUserInterface", true);
    warmed(|w| {
        w.insert(pid, Instant::now() + WARM_GRACE);
    });
    if !COOLING.swap(true, Ordering::SeqCst) {
        std::thread::spawn(cool_down);
    }
    !already_on
}

/// Extend `pid`'s grace period if it is one of ours; reports whether it was.
fn keep_warm(pid: i32) -> bool {
    warmed(|w| {
        w.get_mut(&pid).map(|deadline| *deadline = Instant::now() + WARM_GRACE).is_some()
    })
}

/// Put every browser we opted in back the way we found it, once its grace period lapses.
fn cool_down() {
    loop {
        std::thread::sleep(COOL_TICK);
        let now = Instant::now();
        let due: Vec<i32> = warmed(|w| {
            let due: Vec<i32> =
                w.iter().filter(|(_, at)| **at <= now).map(|(pid, _)| *pid).collect();
            for pid in &due {
                w.remove(pid);
            }
            if w.is_empty() {
                COOLING.store(false, Ordering::SeqCst); // under the lock: no lost wakeups
            }
            due
        });
        for pid in due {
            let app = unsafe { AXUIElementCreateApplication(pid) };
            if app.is_null() {
                continue;
            }
            let app = unsafe { CFType::wrap_under_create_rule(app) };
            set_bool(&app, "AXEnhancedUserInterface", false);
            set_bool(&app, "AXManualAccessibility", false);
        }
        if !COOLING.load(Ordering::SeqCst) {
            return;
        }
    }
}

/// Hand every still-warm browser back immediately — the cooldown thread dies with the app,
/// so quitting mid-grace-period would otherwise leave a browser with its accessibility tree
/// switched on and nobody left to switch it off.
pub fn restore_web_accessibility() {
    let pids: Vec<i32> = warmed(|w| w.drain().map(|(pid, _)| pid).collect());
    for pid in pids {
        let app = unsafe { AXUIElementCreateApplication(pid) };
        if app.is_null() {
            continue;
        }
        let app = unsafe { CFType::wrap_under_create_rule(app) };
        set_bool(&app, "AXEnhancedUserInterface", false);
        set_bool(&app, "AXManualAccessibility", false);
    }
}

fn warmed<T>(f: impl FnOnce(&mut HashMap<i32, Instant>) -> T) -> T {
    let mut guard = WARMED.lock().unwrap_or_else(|e| e.into_inner());
    f(guard.get_or_insert_with(HashMap::new))
}

/// One pass over the focused window: its frame, plus the visible web area if the tree has one.
fn read_geometry(app: &CFType) -> Option<PageGeometry> {
    let window = copy_attr(app, "AXFocusedWindow").or_else(|| copy_attr(app, "AXMainWindow"))?;
    let frame = bounds(&window)?;
    let web_visible = find_web_area(window)
        .as_ref()
        .and_then(bounds)
        .and_then(|web| intersect(web, frame));
    Some(PageGeometry { window: frame, web_visible, tree_still_building: false })
}

/// The pid of the application holding keyboard focus. Unlike walking the window list this is
/// display- and Space-agnostic: it names the app the user is actually in, however many
/// monitors their windows are spread across.
pub(super) fn focused_app_pid() -> Option<i32> {
    let system = unsafe { AXUIElementCreateSystemWide() };
    if system.is_null() {
        return None;
    }
    let system = unsafe { CFType::wrap_under_create_rule(system) };
    let app = copy_attr(&system, "AXFocusedApplication")?;
    let mut pid: i32 = 0;
    (unsafe { AXUIElementGetPid(app.as_CFTypeRef(), &mut pid) } == 0 && pid > 0).then_some(pid)
}

/// Frame and title of `pid`'s focused window, straight from AX. The window list agrees about
/// geometry but not always about titles — `kCGWindowName` comes back empty for some browser
/// windows, which is how captures ended up labelled "Safari" instead of the page they showed.
pub(super) fn focused_window(pid: i32) -> Option<((f64, f64, f64, f64), String)> {
    if pid <= 0 {
        return None;
    }
    let app = unsafe { AXUIElementCreateApplication(pid) };
    if app.is_null() {
        return None;
    }
    let app = unsafe { CFType::wrap_under_create_rule(app) };
    let window = copy_attr(&app, "AXFocusedWindow").or_else(|| copy_attr(&app, "AXMainWindow"))?;
    let frame = bounds(&window)?;
    let title = copy_attr(&window, "AXTitle")
        .and_then(|v| v.downcast::<CFString>())
        .map(|s| s.to_string())
        .unwrap_or_default();
    Some((frame, title))
}

/// What the browser will tell us about the page in its front window. Empty strings mean
/// "didn't say" — no field is ever guessed.
#[derive(Debug, Default, Clone)]
pub struct BrowserMeta {
    /// The page's own title, without the browser and profile the window title adds.
    pub page_title: String,
    pub url: String,
    /// Chromium's profile name, present in the window title only while more than one profile
    /// is in use — which is exactly when it's worth showing.
    pub profile: String,
}

impl BrowserMeta {
    pub fn is_empty(&self) -> bool {
        self.page_title.is_empty() && self.url.is_empty() && self.profile.is_empty()
    }
}

/// Ask `pid`'s front window what page it is showing. `app_name` is the application's
/// localized name ("Google Chrome"), the key to reading its window title.
///
/// Deliberately passive: it reads what the app already publishes and never switches an
/// accessibility tree on. Chromium answers in full without one — the window's `AXDocument`
/// is the URL and its title carries page and profile — Safari builds its web tree natively,
/// and the page-capture modes have already opted their browser in by the time they call
/// here. So no browser is made to work harder because a banner line is switched on, and a
/// non-browser simply returns nothing.
pub(super) fn browser_meta(pid: i32, app_name: &str) -> BrowserMeta {
    let mut meta = BrowserMeta::default();
    if pid <= 0 {
        return meta;
    }
    let app = unsafe { AXUIElementCreateApplication(pid) };
    if app.is_null() {
        return meta;
    }
    let app = unsafe { CFType::wrap_under_create_rule(app) };
    let Some(window) = copy_attr(&app, "AXFocusedWindow").or_else(|| copy_attr(&app, "AXMainWindow"))
    else {
        return meta;
    };

    let window_title = string_attr(&window, "AXTitle").unwrap_or_default();
    // Chromium publishes the address on the window itself, which is why it answers in full
    // without its web tree being built. Only an http(s) address means "this is a page":
    // document apps publish a file path here too — a terminal offers its working directory —
    // and a screenshot of a terminal has no page for the banner to talk about.
    let mut page_title = String::new();
    if let Some(doc) = string_attr(&window, "AXDocument").filter(|d| is_web_address(d)) {
        meta.url = doc;
    } else {
        // Safari has no AXDocument; its web area has the URL, and the page title lands in
        // AXDescription rather than AXTitle. No web area means no page, and no meta.
        let Some(web) = find_web_area(window) else { return meta };
        meta.url = copy_attr(&web, "AXURL")
            .and_then(|v| v.downcast::<CFURL>())
            .map(|u| u.get_string().to_string())
            .unwrap_or_default();
        page_title = string_attr(&web, "AXTitle")
            .or_else(|| string_attr(&web, "AXDescription"))
            .unwrap_or_default();
    }

    let (from_window, profile) = split_window_title(&window_title, app_name);
    meta.page_title = if page_title.is_empty() { from_window.to_string() } else { page_title };
    meta.profile = profile.unwrap_or_default().to_string();
    meta
}

/// Is this the address of a web page, as opposed to a file the window happens to be about?
fn is_web_address(value: &str) -> bool {
    value.starts_with("http://") || value.starts_with("https://")
}

/// Split a window title into the page and, where the browser appends one, the profile.
///
/// Chromium writes `"<page> - <browser> - <profile>"`, dropping the profile while only one is
/// in use; Firefox uses an em dash and no profile; Safari titles are the page alone. The
/// browser's own segment is matched loosely because the two names need not agree: macOS calls
/// it "Firefox", its windows say "Mozilla Firefox". Anything that doesn't name the app is
/// returned untouched — a non-browser window has no page part to find, and inventing one
/// would put a truncated title on the banner.
fn split_window_title<'a>(title: &'a str, app_name: &str) -> (&'a str, Option<&'a str>) {
    const SEPARATORS: [&str; 3] = [" - ", " — ", " – "];
    let title = title.trim();
    if app_name.is_empty() {
        return (title, None);
    }
    for sep in SEPARATORS {
        // The last segment that names the browser wins: a page whose own title mentions the
        // browser is the page's business, and the suffix comes after it.
        let app_segment = title.rmatch_indices(sep).find(|(at, _)| {
            let start = at + sep.len();
            let end = title[start..].find(sep).map_or(title.len(), |i| start + i);
            title[start..end].contains(app_name)
        });
        let Some((at, _)) = app_segment else { continue };
        let page = title[..at].trim();
        let after_app = at + sep.len();
        let rest = title[after_app..].find(sep).map_or("", |i| title[after_app + i..].trim());
        let profile = rest.strip_prefix(sep.trim()).map(str::trim).filter(|p| !p.is_empty());
        if page.is_empty() && profile.is_none() {
            continue; // the title is just the app name — nothing gained by splitting it
        }
        return (page, profile);
    }
    (title, None)
}

fn string_attr(el: &CFType, name: &str) -> Option<String> {
    copy_attr(el, name)?
        .downcast::<CFString>()
        .map(|s| s.to_string())
        .filter(|s| !s.trim().is_empty())
}

fn set_bool(el: &CFType, name: &str, value: bool) {
    let attr = CFString::new(name);
    let value = if value { CFBoolean::true_value() } else { CFBoolean::false_value() };
    unsafe {
        AXUIElementSetAttributeValue(
            el.as_CFTypeRef(),
            attr.as_concrete_TypeRef(),
            value.as_CFTypeRef(),
        );
    }
}

/// Intersection of two rects, `None` when it is too small to be a useful capture.
fn intersect(
    a: (f64, f64, f64, f64),
    b: (f64, f64, f64, f64),
) -> Option<(f64, f64, f64, f64)> {
    let x = a.0.max(b.0);
    let y = a.1.max(b.1);
    let w = (a.0 + a.2).min(b.0 + b.2) - x;
    let h = (a.1 + a.3).min(b.1 + b.3) - y;
    (w >= 40.0 && h >= 40.0).then_some((x, y, w, h))
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

#[cfg(test)]
mod tests {
    use super::split_window_title;

    /// Real window titles, as read off this desk: Chrome with two profiles signed in, Chrome
    /// with one, Safari (page title alone), and Firefox's em dash.
    #[test]
    fn a_window_title_gives_up_its_page_and_profile() {
        assert_eq!(
            split_window_title("New Tab - Google Chrome - Asad (GaTech)", "Google Chrome"),
            ("New Tab", Some("Asad (GaTech)")),
        );
        assert_eq!(
            split_window_title("OMSCS Resources - Google Chrome", "Google Chrome"),
            ("OMSCS Resources", None),
        );
        assert_eq!(
            split_window_title("SVG Design Creation", "Safari"),
            ("SVG Design Creation", None),
        );
        assert_eq!(
            split_window_title("Bugzilla — Mozilla Firefox", "Firefox"),
            ("Bugzilla", None),
        );
    }

    #[test]
    fn a_title_that_isnt_a_browsers_survives_intact() {
        // No app name in it: a document window, and a page that merely mentions the browser.
        assert_eq!(split_window_title("notes.md — Edited", "Zed"), ("notes.md — Edited", None));
        assert_eq!(
            split_window_title("How to quit Google Chrome - Google Chrome", "Google Chrome"),
            ("How to quit Google Chrome", None),
        );
        // The window title IS the app name (an empty new window) — nothing to split off.
        assert_eq!(split_window_title("Google Chrome", "Google Chrome"), ("Google Chrome", None));
        assert_eq!(split_window_title("Slack | general", ""), ("Slack | general", None));
    }
}
