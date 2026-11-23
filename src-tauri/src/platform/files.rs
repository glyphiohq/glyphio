//! Owner-only files. The log, the clipboard history and the engine socket hold what the user
//! types and copies, so on a shared computer no other account may read them. On Unix that is
//! mode 0600; elsewhere the profile directory's own access list already does it.

use std::path::Path;

/// Make `path` readable and writable by its owner only.
pub fn restrict_to_owner(path: &Path) -> std::io::Result<()> {
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o600))
    }
    #[cfg(not(unix))]
    {
        let _ = path;
        Ok(())
    }
}

/// Open `path` for appending, creating it owner-only. A file an older build made readable by
/// others is fixed up too, since the creation mode only applies to a new file.
pub fn open_private_append(path: &Path) -> std::io::Result<std::fs::File> {
    let mut opts = std::fs::OpenOptions::new();
    opts.create(true).append(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        opts.mode(0o600);
    }
    let file = opts.open(path)?;
    let _ = restrict_to_owner(path);
    Ok(file)
}

/// Whether only the owner can reach `path`, where the platform can tell (`None` otherwise).
pub fn is_owner_only(path: &Path) -> Option<bool> {
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let mode = std::fs::metadata(path).ok()?.permissions().mode();
        Some(mode & 0o077 == 0)
    }
    #[cfg(not(unix))]
    {
        let _ = path;
        None
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn private_files_are_owner_only_where_the_platform_can_say() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("private.log");
        open_private_append(&path).unwrap();
        assert_ne!(is_owner_only(&path), Some(false));
    }
}
