//! The real [`Installation`] (macOS): volume flags, bundle copy with `ditto`, launch with `open`.

use std::ffi::CString;
use std::os::unix::ffi::OsStrExt;
use std::path::Path;
use std::process::Command;

use anyhow::{bail, Context};

pub struct Os;

impl crate::platform::Installation for Os {
    fn read_only(&self, path: &Path) -> bool {
        let Ok(c) = CString::new(path.as_os_str().as_bytes()) else {
            return false;
        };
        // SAFETY: `c` is a valid NUL-terminated path and `fs` is a properly sized out-param.
        let mut fs: libc::statfs = unsafe { std::mem::zeroed() };
        if unsafe { libc::statfs(c.as_ptr(), &mut fs) } != 0 {
            return false;
        }
        (fs.f_flags & libc::MNT_RDONLY as u32) != 0
    }

    fn dir_writable(&self, dir: &Path) -> bool {
        let Ok(c) = CString::new(dir.as_os_str().as_bytes()) else {
            return false;
        };
        // SAFETY: `c` is a valid NUL-terminated path.
        unsafe { libc::access(c.as_ptr(), libc::W_OK) == 0 }
    }

    fn bundle_identifier(&self, bundle: &Path) -> Option<String> {
        super::autostart::bundle_identifier_of(bundle)
    }

    fn copy_bundle(&self, from: &Path, to: &Path) -> anyhow::Result<()> {
        let out = Command::new("/usr/bin/ditto").arg(from).arg(to).output().context("running ditto")?;
        if !out.status.success() {
            bail!("could not copy Glyphio: {}", String::from_utf8_lossy(&out.stderr).trim());
        }
        Ok(())
    }

    fn open_bundle(&self, bundle: &Path) -> anyhow::Result<()> {
        let out = Command::new("/usr/bin/open").arg("-n").arg(bundle).output().context("running open")?;
        if !out.status.success() {
            bail!("could not open {}: {}", bundle.display(), String::from_utf8_lossy(&out.stderr).trim());
        }
        Ok(())
    }
}
