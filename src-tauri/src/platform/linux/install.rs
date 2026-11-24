//! The real [`Installation`] (Linux). There is no app bundle to move: an AppImage, a package or
//! an unpacked portable folder each stays where the user put it, so the move is refused.

use std::ffi::CString;
use std::os::unix::ffi::OsStrExt;
use std::path::Path;

use crate::platform::unsupported::unsupported;

pub struct Os;

impl crate::platform::Installation for Os {
    fn read_only(&self, path: &Path) -> bool {
        let Ok(c) = CString::new(path.as_os_str().as_bytes()) else {
            return false;
        };
        // SAFETY: `c` is a valid NUL-terminated path and `fs` is a properly sized out-param.
        let mut fs: libc::statvfs = unsafe { std::mem::zeroed() };
        if unsafe { libc::statvfs(c.as_ptr(), &mut fs) } != 0 {
            return false;
        }
        (fs.f_flag & libc::ST_RDONLY) != 0
    }

    fn dir_writable(&self, dir: &Path) -> bool {
        let Ok(c) = CString::new(dir.as_os_str().as_bytes()) else {
            return false;
        };
        // SAFETY: `c` is a valid NUL-terminated path.
        unsafe { libc::access(c.as_ptr(), libc::W_OK) == 0 }
    }

    fn bundle_identifier(&self, _: &Path) -> Option<String> {
        None
    }

    fn copy_bundle(&self, _: &Path, _: &Path) -> anyhow::Result<()> {
        Err(unsupported("Moving the app"))
    }

    fn open_bundle(&self, _: &Path) -> anyhow::Result<()> {
        Err(unsupported("Opening another copy of the app"))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::platform::Installation;

    #[test]
    fn a_temp_dir_is_writable_and_not_read_only() {
        let dir = tempfile::tempdir().unwrap();
        assert!(Os.dir_writable(dir.path()));
        assert!(!Os.read_only(dir.path()));
        assert!(!Os.dir_writable(&dir.path().join("missing")));
    }
}
