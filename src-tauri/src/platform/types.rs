//! Plain data crossing the platform boundary.

/// Frontmost normal window not owned by Glyphio — the target of "scrolling page", "front
/// window", and "browser page" capture.
pub struct FrontWindow {
    pub x: f64,
    pub y: f64,
    pub w: f64,
    pub h: f64,
    pub title: String,
    /// Owning process — lets the AX layer find the window's web content area.
    pub pid: i32,
    /// The owning application's name as macOS presents it ("Google Chrome"). Browsers build
    /// their window title around it, which is how the page title and profile come apart.
    pub app_name: String,
}

/// What the AX tree knows about `pid`'s focused window.
pub struct PageGeometry {
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
