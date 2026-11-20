//! The real [`ProcessControl`], the pid file and the daemon lock probe (macOS/unix).

use std::path::{Path, PathBuf};

use super::reaper::ProcessControl;

pub struct Os;

impl ProcessControl for Os {
    fn exe_path(&self, pid: u32) -> Option<PathBuf> {
        let mut buf = vec![0u8; libc::PROC_PIDPATHINFO_MAXSIZE as usize];
        // SAFETY: the buffer is valid for its length; proc_pidpath writes at most that many bytes.
        let n = unsafe { libc::proc_pidpath(pid as i32, buf.as_mut_ptr().cast(), buf.len() as u32) };
        if n <= 0 {
            return None;
        }
        buf.truncate(n as usize);
        Some(PathBuf::from(String::from_utf8_lossy(&buf).into_owned()))
    }

    fn kill_tree(&self, pid: u32) {
        // Children first (the daemon's worker), then the daemon, so nothing is reparented.
        let mut kids = [0i32; 256];
        // SAFETY: the buffer is valid for the stated byte size.
        let n = unsafe {
            libc::proc_listchildpids(pid as i32, kids.as_mut_ptr().cast(), std::mem::size_of_val(&kids) as i32)
        };
        let count = if n > 0 { n as usize / std::mem::size_of::<i32>() } else { 0 };
        for &kid in kids.iter().take(count).filter(|k| **k > 0) {
            self.kill_tree(kid as u32);
        }
        // SAFETY: plain signal to a pid we verified belongs to our engine.
        unsafe {
            libc::kill(pid as i32, libc::SIGKILL);
        }
    }
}

pub fn pid_file(runtime: &Path) -> PathBuf {
    runtime.join("glyphio-engine.pid")
}

pub fn read_pid(runtime: &Path) -> Option<u32> {
    std::fs::read_to_string(pid_file(runtime)).ok()?.trim().parse().ok()
}

pub fn write_pid(runtime: &Path, pid: u32) {
    if let Err(e) = std::fs::write(pid_file(runtime), pid.to_string()) {
        log::warn!("[engine] could not record pid: {e}");
    }
}

pub fn clear_pid(runtime: &Path) {
    let _ = std::fs::remove_file(pid_file(runtime));
}

/// Whether the engine daemon's lock (`espanso-daemon.lock`, an flock) can be taken, i.e. no
/// daemon holds it. The probe releases it immediately.
pub fn daemon_lock_free(runtime: &Path) -> bool {
    use std::os::fd::AsRawFd;
    let Ok(file) = std::fs::OpenOptions::new()
        .read(true)
        .write(true)
        .create(true)
        .truncate(false)
        .open(runtime.join("espanso-daemon.lock"))
    else {
        return true;
    };
    // SAFETY: the fd is open for the lifetime of `file`.
    unsafe {
        if libc::flock(file.as_raw_fd(), libc::LOCK_EX | libc::LOCK_NB) == 0 {
            libc::flock(file.as_raw_fd(), libc::LOCK_UN);
            true
        } else {
            false
        }
    }
}

/// This bundle's engine: the sidecar sits next to the main executable.
pub fn bundled_engine_exe() -> Option<PathBuf> {
    let exe = std::env::current_exe().ok()?.canonicalize().ok()?;
    Some(exe.parent()?.join("glyphio-engine"))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::os::fd::AsRawFd;

    #[test]
    fn exe_path_of_this_process_is_this_executable() {
        let me = std::env::current_exe().unwrap().canonicalize().unwrap();
        assert_eq!(Os.exe_path(std::process::id()), Some(me));
    }

    #[test]
    fn exe_path_of_a_dead_pid_is_none() {
        let mut child = std::process::Command::new("true").spawn().unwrap();
        let pid = child.id();
        child.wait().unwrap();
        assert_eq!(Os.exe_path(pid), None);
    }

    #[test]
    fn lock_probe_sees_a_held_lock_and_a_free_one() {
        let dir = tempfile::tempdir().unwrap();
        assert!(daemon_lock_free(dir.path()));
        let held = std::fs::File::create(dir.path().join("espanso-daemon.lock")).unwrap();
        // SAFETY: valid fd for the life of `held`.
        assert_eq!(unsafe { libc::flock(held.as_raw_fd(), libc::LOCK_EX | libc::LOCK_NB) }, 0);
        assert!(!daemon_lock_free(dir.path()));
        drop(held);
        assert!(daemon_lock_free(dir.path()));
    }

    #[test]
    fn pid_file_round_trips() {
        let dir = tempfile::tempdir().unwrap();
        assert_eq!(read_pid(dir.path()), None);
        write_pid(dir.path(), 4242);
        assert_eq!(read_pid(dir.path()), Some(4242));
        clear_pid(dir.path());
        assert_eq!(read_pid(dir.path()), None);
    }
}
