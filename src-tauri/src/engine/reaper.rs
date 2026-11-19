//! Clears a stale engine daemon by recorded pid, never by process name.
//!
//! The engine daemon holds a lock, so an orphan from a crashed run stops the next daemon from
//! starting. The old fix was `pkill -f glyphio-engine`, which also killed another install's
//! engine (a dev build next to a release build). Here the supervisor records the daemon's pid
//! and only a process that is still alive *and* whose executable is this bundle's engine is
//! terminated. Everything OS-specific sits behind [`ProcessControl`], so the decisions are
//! tested with a fake.

use std::path::Path;
use std::time::{Duration, Instant};

pub trait ProcessControl {
    /// Executable path of a live process, or `None` if the pid is not running.
    fn exe_path(&self, pid: u32) -> Option<std::path::PathBuf>;
    /// Terminate the process and its children (the daemon's worker).
    fn kill_tree(&self, pid: u32);
}

#[derive(Debug, PartialEq, Eq)]
pub enum Reaped {
    /// No pid was recorded.
    NothingRecorded,
    /// A pid was recorded but the process is gone.
    Gone(u32),
    /// The pid is alive but runs a different executable (pid reuse, or another install).
    Foreign(u32),
    /// Our stale engine was terminated.
    Killed(u32),
}

/// Terminate the recorded daemon only if it is our engine.
pub fn reap<C: ProcessControl>(ctl: &C, recorded: Option<u32>, engine_exe: &Path) -> Reaped {
    let Some(pid) = recorded else {
        return Reaped::NothingRecorded;
    };
    match ctl.exe_path(pid) {
        None => Reaped::Gone(pid),
        Some(exe) if exe == engine_exe => {
            ctl.kill_tree(pid);
            Reaped::Killed(pid)
        }
        Some(_) => Reaped::Foreign(pid),
    }
}

/// Poll `released` every `step` until it is true or `timeout` passes. Returns whether it was.
pub fn wait_until(
    mut released: impl FnMut() -> bool,
    timeout: Duration,
    step: Duration,
    mut sleep: impl FnMut(Duration),
) -> bool {
    let start = Instant::now();
    loop {
        if released() {
            return true;
        }
        if start.elapsed() >= timeout {
            return false;
        }
        sleep(step);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::cell::RefCell;
    use std::path::PathBuf;

    #[derive(Default)]
    struct Fake {
        procs: Vec<(u32, PathBuf)>,
        killed: RefCell<Vec<u32>>,
    }
    impl ProcessControl for Fake {
        fn exe_path(&self, pid: u32) -> Option<PathBuf> {
            self.procs.iter().find(|(p, _)| *p == pid).map(|(_, e)| e.clone())
        }
        fn kill_tree(&self, pid: u32) {
            self.killed.borrow_mut().push(pid);
        }
    }

    fn mine() -> PathBuf {
        PathBuf::from("/Applications/Glyphio.app/Contents/MacOS/glyphio-engine")
    }

    #[test]
    fn stale_engine_of_this_bundle_is_killed() {
        let ctl = Fake { procs: vec![(10, mine())], ..Default::default() };
        assert_eq!(reap(&ctl, Some(10), &mine()), Reaped::Killed(10));
        assert_eq!(*ctl.killed.borrow(), vec![10]);
    }

    #[test]
    fn another_installs_engine_is_left_alone() {
        let other = PathBuf::from("/Users/dev/build/Glyphio.app/Contents/MacOS/glyphio-engine");
        let ctl = Fake { procs: vec![(10, other)], ..Default::default() };
        assert_eq!(reap(&ctl, Some(10), &mine()), Reaped::Foreign(10));
        assert!(ctl.killed.borrow().is_empty());
    }

    #[test]
    fn a_reused_pid_running_something_else_is_left_alone() {
        let ctl = Fake { procs: vec![(10, PathBuf::from("/usr/bin/vim"))], ..Default::default() };
        assert_eq!(reap(&ctl, Some(10), &mine()), Reaped::Foreign(10));
        assert!(ctl.killed.borrow().is_empty());
    }

    #[test]
    fn a_dead_or_unrecorded_pid_kills_nothing() {
        let ctl = Fake::default();
        assert_eq!(reap(&ctl, Some(10), &mine()), Reaped::Gone(10));
        assert_eq!(reap(&ctl, None, &mine()), Reaped::NothingRecorded);
        assert!(ctl.killed.borrow().is_empty());
    }

    #[test]
    fn wait_returns_as_soon_as_released_and_gives_up_at_the_timeout() {
        let mut calls = 0;
        assert!(wait_until(
            || {
                calls += 1;
                calls == 3
            },
            Duration::from_secs(5),
            Duration::from_millis(1),
            |_| {},
        ));
        assert_eq!(calls, 3);
        assert!(!wait_until(|| false, Duration::ZERO, Duration::from_millis(1), |_| {}));
    }
}
