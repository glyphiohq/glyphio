//! An in-memory platform for tests. It implements every capability trait, records what it was
//! asked to do, and answers from its fields, so the pure cores can be driven without an OS.
//! [`AppShell`] needs a live `tauri::AppHandle`, so its methods only record the call.

use std::path::{Path, PathBuf};
use std::sync::Mutex;
use std::time::Duration;

use image::RgbaImage;

use super::{
    AppShell, Autostart, BrowserMeta, DateStyle, DisplayBounds, KeyboardLayout, LocaleDates, FrontWindow, InputInjector, Installation, PageGeometry,
    PageInspector, Pasteboard, PermissionKind, Permissions, PickKind, ProcessControl, Progress, RegionPicker,
    ScreenCapturer, WindowQuery,
};
use crate::autostart::{Migration, Outcome};
use crate::capture::Shot;

#[derive(Default)]
pub struct Fake {
    pub accessibility: bool,
    pub screen_recording: bool,
    pub calls: Mutex<Vec<String>>,
    pub scrolls: Mutex<Vec<i32>>,
    /// The screen, as one solid colour of this size in pixels (at scale 1).
    pub screen: (u32, u32),
    /// Scroll position reported by the inspector.
    pub progress: Option<Progress>,
    pub cursor: Mutex<Option<(f64, f64)>>,
    pub pasteboard_counter: i64,
    pub concealed: bool,
    pub login_item: Mutex<bool>,
    pub read_only: bool,
    pub bundle_ids: Vec<(PathBuf, String)>,
    /// What [`Pasteboard::write_png`] was given; `refuse_copy` makes it fail.
    pub copied: Mutex<Vec<Vec<u8>>>,
    pub refuse_copy: bool,
    /// What [`WindowQuery::session_locked`] answers.
    pub locked: bool,
    /// What [`KeyboardLayout::key_labels`] answers.
    pub key_labels: Option<Vec<(String, String)>>,
}

impl Fake {
    fn record(&self, call: impl Into<String>) {
        self.calls.lock().unwrap().push(call.into());
    }

    fn shot(&self, title: &str) -> Shot {
        let (width, height) = self.screen;
        Shot {
            rgba: vec![0x80; (width * height * 4) as usize],
            png: None,
            width,
            height,
            dpr: 1.0,
            title: title.into(),
            browser: BrowserMeta::default(),
        }
    }

    fn display(&self) -> DisplayBounds {
        ((0.0, 0.0), (self.screen.0 as f64, self.screen.1 as f64))
    }
}

impl ScreenCapturer for Fake {
    fn visible(&self) -> anyhow::Result<Shot> {
        self.record("capture visible");
        Ok(self.shot(""))
    }
    fn front_window(&self, _: bool) -> anyhow::Result<Shot> {
        self.record("capture front window");
        Ok(self.shot("Front"))
    }
    fn rect_image(&self, x: f64, y: f64, w: f64, h: f64) -> anyhow::Result<(RgbaImage, f64)> {
        self.record(format!("capture rect {x},{y} {w}x{h}"));
        Ok((RgbaImage::new(w as u32, h as u32), 1.0))
    }
}

impl RegionPicker for Fake {
    fn pick(&self, kind: PickKind) -> anyhow::Result<Shot> {
        self.record(format!("pick {kind:?}"));
        match kind {
            PickKind::Region => Ok(self.shot("")),
            PickKind::Window => anyhow::bail!("cancelled"),
        }
    }
}

impl WindowQuery for Fake {
    fn frontmost_window(&self) -> anyhow::Result<FrontWindow> {
        let ((x, y), (w, h)) = self.display();
        Ok(FrontWindow { x, y, w, h, title: "Front".into(), pid: 42, app_name: "Fake App".into() })
    }
    fn frontmost_app(&self) -> Option<(i32, String)> {
        Some((42, "Fake App".into()))
    }
    fn focused_window_display(&self) -> Option<DisplayBounds> {
        Some(self.display())
    }
    fn display_bounds_under_cursor(&self) -> DisplayBounds {
        self.display()
    }
    fn display_bounds_containing_point(&self, x: f64, y: f64) -> Option<DisplayBounds> {
        let ((dx, dy), (w, h)) = self.display();
        (x >= dx && y >= dy && x < dx + w && y < dy + h).then(|| self.display())
    }
    fn foreground_app_name(&self) -> String {
        "Fake App".into()
    }
    fn session_locked(&self) -> bool {
        self.locked
    }
}

impl PageInspector for Fake {
    fn page_geometry(&self, _: i32, _: Duration) -> Option<PageGeometry> {
        let ((x, y), (w, h)) = self.display();
        Some(PageGeometry { window: (x, y, w, h), web_visible: None, tree_still_building: false })
    }
    fn browser_meta(&self, _: i32, _: &str) -> BrowserMeta {
        BrowserMeta { page_title: "Page".into(), url: "https://example.com/".into(), profile: String::new() }
    }
    fn scroll_progress(&self, _: (f64, f64)) -> Option<Progress> {
        self.progress
    }
    fn release(&self) {
        self.record("release");
    }
}

impl InputInjector for Fake {
    fn post_scroll(&self, delta: i32) -> anyhow::Result<()> {
        self.scrolls.lock().unwrap().push(delta);
        Ok(())
    }
    fn cursor_position(&self) -> Option<(f64, f64)> {
        *self.cursor.lock().unwrap()
    }
    fn warp_cursor(&self, p: (f64, f64)) {
        *self.cursor.lock().unwrap() = Some(p);
    }
    fn send_paste(&self) -> anyhow::Result<()> {
        self.record("paste");
        Ok(())
    }
    fn can_send_paste(&self) -> bool {
        self.accessibility
    }
}

impl Pasteboard for Fake {
    fn change_counter(&self) -> i64 {
        self.pasteboard_counter
    }
    fn is_concealed(&self) -> bool {
        self.concealed
    }
    fn write_png(&self, png: &[u8]) -> anyhow::Result<()> {
        if self.refuse_copy {
            anyhow::bail!("the pasteboard refused the image");
        }
        self.copied.lock().unwrap().push(png.to_vec());
        Ok(())
    }
}

impl LocaleDates for Fake {
    fn format(&self, millis: i64, date: DateStyle, time: DateStyle, locale: Option<&str>, zone: Option<&str>) -> Option<String> {
        Some(format!("{millis} {date:?}/{time:?} {} {}", locale.unwrap_or("-"), zone.unwrap_or("-")))
    }
}

impl KeyboardLayout for Fake {
    fn key_labels(&self) -> Option<Vec<(String, String)>> {
        self.key_labels.clone()
    }
}

impl Permissions for Fake {
    fn accessibility_trusted(&self) -> bool {
        self.accessibility
    }
    fn request_accessibility(&self) -> bool {
        self.record("request Accessibility");
        self.accessibility
    }
    fn screen_recording_granted(&self) -> bool {
        self.screen_recording
    }
    fn request_screen_recording(&self) -> bool {
        self.record("request ScreenRecording");
        true
    }
    fn screen_recording_probe(&self) -> bool {
        self.record("probe");
        false
    }
    fn reset(&self, kind: PermissionKind) -> anyhow::Result<()> {
        self.record(format!("reset {kind:?}"));
        Ok(())
    }
    fn settings_url(&self, _: PermissionKind) -> Option<&'static str> {
        None
    }
}

impl Autostart for Fake {
    fn set_enabled(&self, want: bool) -> Outcome {
        let mut on = self.login_item.lock().unwrap();
        if *on == want {
            return Outcome::Unchanged;
        }
        *on = want;
        if want { Outcome::Registered } else { Outcome::Unregistered }
    }
    fn reconcile(&self, want: bool) -> Outcome {
        self.set_enabled(want)
    }
    fn migrate_legacy(&self, _: &Path, existing_install: bool) -> Migration {
        if existing_install { Migration::Removed } else { Migration::AlreadyDone }
    }
    fn launch_property(&self) -> Option<u32> {
        None
    }
}

impl AppShell for Fake {
    fn init_agent(&self, _: &tauri::AppHandle) {
        self.record("init agent");
    }
    fn set_regular(&self, _: &tauri::AppHandle, regular: bool) {
        self.record(format!("regular {regular}"));
    }
    fn hide_app(&self, _: &tauri::AppHandle) {
        self.record("hide");
    }
    fn follow_user_across_spaces(&self, _: &tauri::WebviewWindow) {}
}

impl ProcessControl for Fake {
    fn exe_path(&self, _: u32) -> Option<PathBuf> {
        None
    }
    fn kill_tree(&self, pid: u32) {
        self.record(format!("kill {pid}"));
    }
    fn daemon_lock_free(&self, _: &Path) -> bool {
        true
    }
    fn bundled_engine_exe(&self) -> Option<PathBuf> {
        None
    }
}

impl Installation for Fake {
    fn read_only(&self, _: &Path) -> bool {
        self.read_only
    }
    fn dir_writable(&self, _: &Path) -> bool {
        !self.read_only
    }
    fn bundle_identifier(&self, bundle: &Path) -> Option<String> {
        self.bundle_ids.iter().find(|(p, _)| p == bundle).map(|(_, id)| id.clone())
    }
    fn copy_bundle(&self, from: &Path, to: &Path) -> anyhow::Result<()> {
        self.record(format!("copy {} -> {}", from.display(), to.display()));
        Ok(())
    }
    fn open_bundle(&self, bundle: &Path) -> anyhow::Result<()> {
        self.record(format!("open {}", bundle.display()));
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::capture::scroll::require_accessibility_with;

    #[test]
    fn scrolling_capture_refuses_without_accessibility_and_proceeds_with_it() {
        let denied = Fake::default();
        let err = require_accessibility_with(&denied).unwrap_err().to_string();
        assert!(err.contains("Accessibility"), "{err}");
        assert!(require_accessibility_with(&Fake { accessibility: true, ..Default::default() }).is_ok());
    }

    #[test]
    fn input_goes_through_the_trait() {
        let f = Fake::default();
        f.post_scroll(-40).unwrap();
        assert_eq!(*f.scrolls.lock().unwrap(), vec![-40]);
    }

    #[test]
    fn capture_traits_hand_back_frames_of_the_screen_size() {
        let f = Fake { screen: (4, 3), ..Default::default() };
        let shot = f.visible().unwrap();
        assert_eq!((shot.width, shot.height, shot.rgba.len()), (4, 3, 48));
        assert_eq!(f.front_window(false).unwrap().title, "Front");
        let mut session = f.rect_session((0.0, 0.0, 2.0, 2.0)).unwrap();
        assert_eq!(session.frame().unwrap().0.dimensions(), (2, 2));
        assert!(f.pick(PickKind::Region).is_ok());
        assert!(f.pick(PickKind::Window).is_err(), "a cancelled pick is an error");
    }

    #[test]
    fn window_query_answers_from_one_display() {
        let f = Fake { screen: (100, 50), ..Default::default() };
        assert_eq!(f.frontmost_window().unwrap().w, 100.0);
        assert_eq!(f.display_bounds_containing_point(10.0, 10.0), Some(((0.0, 0.0), (100.0, 50.0))));
        assert_eq!(f.display_bounds_containing_point(200.0, 10.0), None);
        assert_eq!(f.frontmost_app().unwrap().1, f.foreground_app_name());
    }

    #[test]
    fn inspector_reports_the_configured_scroll_position() {
        let p = Progress { offset: 10.0, extent: 1000.0, viewport: 100.0 };
        let f = Fake { screen: (8, 8), progress: Some(p), ..Default::default() };
        assert_eq!(f.scroll_progress((0.0, 0.0)), Some(p));
        assert!(f.page_geometry(42, Duration::ZERO).unwrap().web_visible.is_none());
        assert_eq!(f.browser_meta(42, "Fake App").url, "https://example.com/");
        f.release();
        assert_eq!(*f.calls.lock().unwrap(), vec!["release"]);
    }

    #[test]
    fn cursor_warps_and_pasteboard_reports_its_fields() {
        let f = Fake { pasteboard_counter: 7, concealed: true, ..Default::default() };
        assert_eq!(f.cursor_position(), None);
        f.warp_cursor((3.0, 4.0));
        assert_eq!(f.cursor_position(), Some((3.0, 4.0)));
        assert_eq!((f.change_counter(), f.is_concealed()), (7, true));
    }

    #[test]
    fn permissions_record_requests_and_resets() {
        let f = Fake::default();
        assert!(!f.request_accessibility());
        f.reset(PermissionKind::ScreenRecording).unwrap();
        assert_eq!(*f.calls.lock().unwrap(), vec!["request Accessibility", "reset ScreenRecording"]);
    }

    #[test]
    fn autostart_registers_once_and_reports_no_change_after() {
        let f = Fake::default();
        assert!(matches!(f.set_enabled(true), Outcome::Registered));
        assert!(matches!(f.reconcile(true), Outcome::Unchanged));
        assert!(matches!(f.set_enabled(false), Outcome::Unregistered));
        assert!(matches!(f.migrate_legacy(Path::new("/m"), false), Migration::AlreadyDone));
    }

    #[test]
    fn process_and_installation_record_what_they_were_asked() {
        let app = PathBuf::from("/Applications/Glyphio.app");
        let f = Fake { bundle_ids: vec![(app.clone(), "io.glyphio".into())], ..Default::default() };
        f.kill_tree(9);
        assert!(f.daemon_lock_free(Path::new("/run")));
        assert_eq!(f.bundle_identifier(&app).as_deref(), Some("io.glyphio"));
        assert!(f.dir_writable(Path::new("/Applications")));
        f.copy_bundle(Path::new("/Volumes/G/Glyphio.app"), &app).unwrap();
        f.open_bundle(&app).unwrap();
        assert_eq!(
            *f.calls.lock().unwrap(),
            vec!["kill 9", "copy /Volumes/G/Glyphio.app -> /Applications/Glyphio.app", "open /Applications/Glyphio.app"]
        );
    }
}
