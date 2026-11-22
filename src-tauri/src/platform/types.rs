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

/// Where the main scroll container is, in points, independent of what the pixels show.
///
/// The scroll driver's progress oracle: a page that repeats itself, animates, or has sticky
/// bands fools a pixel comparison, but the accessibility tree still knows the offset.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Progress {
    /// Distance from the top of the content to the top of the viewport.
    pub offset: f64,
    /// Full height of the scrollable content.
    pub extent: f64,
    /// Height of the visible part.
    pub viewport: f64,
}

impl Progress {
    /// Progress of a web area: `web` is the area's frame (its full document extent, with a
    /// top that moves above `visible_top` as the page scrolls) and `visible` the viewport.
    pub fn from_web_area(web: (f64, f64, f64, f64), visible: (f64, f64, f64, f64)) -> Option<Self> {
        let (offset, extent, viewport) = ((visible.1 - web.1).max(0.0), web.3, visible.3);
        (extent.is_finite() && viewport > 0.0 && extent >= viewport - 1.0)
            .then_some(Self { offset, extent, viewport })
    }

    /// Progress of a native scroll view from its vertical scroll bar value (0 at the top,
    /// 1 at the end), the content height and the area height.
    pub fn from_scroll_bar(value: f64, content: f64, viewport: f64) -> Option<Self> {
        if !(value.is_finite() && content.is_finite() && viewport > 0.0 && content >= viewport) {
            return None;
        }
        Some(Self { offset: value.clamp(0.0, 1.0) * (content - viewport), extent: content, viewport })
    }

    /// Whether the viewport is at the end of the content (1 pt of rounding allowed).
    pub fn at_end(&self) -> bool {
        self.offset + self.viewport >= self.extent - 1.0
    }

    /// How far through the scrollable distance the viewport is, 0 to 1.
    pub fn fraction(&self) -> f64 {
        let range = (self.extent - self.viewport).max(0.0);
        if range <= 0.0 { 1.0 } else { (self.offset / range).clamp(0.0, 1.0) }
    }
}

#[cfg(test)]
mod progress_tests {
    use super::Progress;

    #[test]
    fn web_area_offset_is_how_far_its_top_sits_above_the_viewport() {
        let visible = (0.0, 100.0, 800.0, 600.0);
        let p = Progress::from_web_area((0.0, -400.0, 800.0, 3000.0), visible).unwrap();
        assert_eq!(p, Progress { offset: 500.0, extent: 3000.0, viewport: 600.0 });
        assert!(!p.at_end());
        let end = Progress::from_web_area((0.0, 100.0 - 2400.0, 800.0, 3000.0), visible).unwrap();
        assert!(end.at_end());
        assert_eq!(end.fraction(), 1.0);
    }

    #[test]
    fn scroll_bar_value_maps_onto_the_content_range() {
        let p = Progress::from_scroll_bar(0.5, 2000.0, 500.0).unwrap();
        assert_eq!(p.offset, 750.0);
        assert!(Progress::from_scroll_bar(1.0, 2000.0, 500.0).unwrap().at_end());
        assert!(Progress::from_scroll_bar(f64::NAN, 2000.0, 500.0).is_none());
        assert!(Progress::from_scroll_bar(0.5, 100.0, 500.0).is_none());
    }
}
