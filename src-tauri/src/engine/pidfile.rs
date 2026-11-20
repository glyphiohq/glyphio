//! The recorded daemon pid, `<runtime>/glyphio-engine.pid`.

use std::path::{Path, PathBuf};

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

#[cfg(test)]
mod tests {
    use super::*;

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
