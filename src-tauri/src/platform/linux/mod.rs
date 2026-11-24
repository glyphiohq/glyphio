//! Linux implementation of the platform traits (`docs/LINUX.md`).
//!
//! Built up capability by capability. What is here: process control through `/proc`, the login
//! item as an XDG autostart entry, and install detection that knows about AppImages. Capture,
//! window queries, input, the clipboard counter and permissions still answer through
//! [`Unsupported`] until their pieces land.

use std::path::{Path, PathBuf};

use super::unsupported::Unsupported;
use super::{Autostart, Platform};
use crate::autostart::{Migration, Outcome};

mod autostart;
mod install;
mod process;

pub fn platform() -> Platform {
    Platform {
        capturer: Box::new(Unsupported),
        picker: Box::new(Unsupported),
        windows: Box::new(Unsupported),
        inspector: Box::new(Unsupported),
        input: Box::new(Unsupported),
        pasteboard: Box::new(Unsupported),
        permissions: Box::new(Unsupported),
        autostart: Box::new(Linux),
        process: Box::new(process::Os),
        shell: Box::new(Unsupported),
        install: Box::new(install::Os),
    }
}

/// The folder the user put the app in. Inside an AppImage the executable runs from a temporary
/// mount (`/tmp/.mount_GlyphiXXXX/usr/bin`); the runtime says where the image itself is.
pub fn app_dir() -> Option<PathBuf> {
    appimage().and_then(|image| image.parent().map(Path::to_path_buf))
}

/// The AppImage this process runs from, if it does.
pub fn appimage() -> Option<PathBuf> {
    std::env::var_os("APPIMAGE").filter(|v| !v.is_empty()).map(PathBuf::from)
}

struct Linux;

impl Autostart for Linux {
    fn set_enabled(&self, want: bool) -> Outcome {
        crate::autostart::apply_setting(&mut autostart::XdgEntry::for_this_copy(), want)
    }
    fn reconcile(&self, want: bool) -> Outcome {
        crate::autostart::reconcile_at_launch(&mut autostart::XdgEntry::for_this_copy(), want)
    }
    fn migrate_legacy(&self, _: &Path, _: bool) -> Migration {
        // No older Linux login item ever existed to clean up.
        crate::autostart::migrate_scripted_item(true, || Ok(()))
    }
    fn launch_property(&self) -> Option<u32> {
        // The autostart entry passes an argument, since Linux has no launch event to read.
        std::env::args()
            .any(|a| a == autostart::AT_LOGIN_ARGUMENT)
            .then_some(crate::startup::LAUNCHED_AS_LOGIN_ITEM)
    }
}
