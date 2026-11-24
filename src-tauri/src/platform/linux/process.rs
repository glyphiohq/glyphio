//! The real [`ProcessControl`] (Linux), through `/proc` and `flock`.

use std::path::{Path, PathBuf};

pub struct Os;

impl crate::platform::ProcessControl for Os {
    fn exe_path(&self, pid: u32) -> Option<PathBuf> {
        let link = std::fs::read_link(format!("/proc/{pid}/exe")).ok()?;
        // A replaced binary (an upgrade while it runs) reads as "/path (deleted)".
        let text = link.to_string_lossy();
        Some(match text.strip_suffix(" (deleted)") {
            Some(path) => PathBuf::from(path),
            None => link,
        })
    }

    fn kill_tree(&self, pid: u32) {
        // Children first (the daemon's worker), then the daemon, so nothing is reparented.
        for kid in children(pid) {
            self.kill_tree(kid);
        }
        // SAFETY: plain signal to a pid the caller verified belongs to our engine.
        unsafe {
            libc::kill(pid as i32, libc::SIGKILL);
        }
    }

    fn daemon_lock_free(&self, runtime: &Path) -> bool {
        daemon_lock_free(runtime)
    }

    fn bundled_engine_exe(&self) -> Option<PathBuf> {
        // The bundler puts sidecars beside the main executable (`/usr/bin`, or `usr/bin` in the
        // AppImage), named without the target triple.
        let exe = std::env::current_exe().ok()?.canonicalize().ok()?;
        Some(exe.parent()?.join("glyphio-engine"))
    }
}

/// Direct children of `pid`, from every thread's `children` list.
fn children(pid: u32) -> Vec<u32> {
    let Ok(tasks) = std::fs::read_dir(format!("/proc/{pid}/task")) else {
        return Vec::new();
    };
    let mut kids: Vec<u32> = tasks
        .filter_map(|t| t.ok())
        .filter_map(|t| std::fs::read_to_string(t.path().join("children")).ok())
        .flat_map(|list| list.split_whitespace().filter_map(|p| p.parse().ok()).collect::<Vec<_>>())
        .collect();
    kids.sort_unstable();
    kids.dedup();
    kids
}

/// Whether the engine daemon's lock (`espanso-daemon.lock`, an flock) can be taken, i.e. no
/// daemon holds it. The probe releases it immediately.
fn daemon_lock_free(runtime: &Path) -> bool {
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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::platform::ProcessControl;
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
    fn kill_tree_takes_the_children_down_too() {
        // A shell whose child sleeps: killing the shell's tree must kill the sleeper.
        let mut sh = std::process::Command::new("sh").args(["-c", "sleep 30 & wait"]).spawn().unwrap();
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(5);
        let kids = loop {
            let kids = children(sh.id());
            if !kids.is_empty() || std::time::Instant::now() > deadline {
                break kids;
            }
            std::thread::sleep(std::time::Duration::from_millis(20));
        };
        assert_eq!(kids.len(), 1, "sleep should be the shell's only child");
        Os.kill_tree(sh.id());
        sh.wait().unwrap();
        let gone = (0..100).any(|_| {
            std::thread::sleep(std::time::Duration::from_millis(20));
            std::fs::read_to_string(format!("/proc/{}/stat", kids[0]))
                .map_or(true, |stat| stat.split_whitespace().nth(2) == Some("Z"))
        });
        assert!(gone, "the child outlived kill_tree");
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
}
