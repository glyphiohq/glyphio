//! A minimal in-memory platform for tests: enough to prove the seam, extended as pure cores
//! start taking `&dyn` capabilities instead of calling [`super::current`].

use std::path::{Path, PathBuf};
use std::sync::Mutex;

use super::{InputInjector, PermissionKind, Permissions, ProcessControl};

#[derive(Default)]
pub struct Fake {
    pub accessibility: bool,
    pub scrolls: Mutex<Vec<i32>>,
}

impl Permissions for Fake {
    fn accessibility_trusted(&self) -> bool {
        self.accessibility
    }
    fn request_accessibility(&self) -> bool {
        self.accessibility
    }
    fn screen_recording_granted(&self) -> bool {
        true
    }
    fn request_screen_recording(&self) -> bool {
        true
    }
    fn settings_url(&self, _: PermissionKind) -> Option<&'static str> {
        None
    }
}

impl InputInjector for Fake {
    fn post_scroll(&self, delta: i32) -> anyhow::Result<()> {
        self.scrolls.lock().unwrap().push(delta);
        Ok(())
    }
    fn cursor_position(&self) -> Option<(f64, f64)> {
        None
    }
    fn warp_cursor(&self, _: (f64, f64)) {}
    fn send_paste(&self) -> anyhow::Result<()> {
        Ok(())
    }
    fn can_send_paste(&self) -> bool {
        self.accessibility
    }
}

impl ProcessControl for Fake {
    fn exe_path(&self, _: u32) -> Option<PathBuf> {
        None
    }
    fn kill_tree(&self, _: u32) {}
    fn daemon_lock_free(&self, _: &Path) -> bool {
        true
    }
    fn bundled_engine_exe(&self) -> Option<PathBuf> {
        None
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
}
