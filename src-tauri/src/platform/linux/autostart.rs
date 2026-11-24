//! The login item on Linux: an XDG autostart entry (`~/.config/autostart/io.glyphio.app.desktop`),
//! which GNOME, KDE, Xfce and the other desktops that follow the spec launch at login. It is a
//! plain file the user can see and delete; the desktop's own "Startup Applications" lists it.

use std::path::{Path, PathBuf};

use crate::autostart::{LoginItem, Status, BUNDLE_IDENTIFIER};

/// What the entry passes, so this process can tell a login launch from the user opening it.
pub const AT_LOGIN_ARGUMENT: &str = "--at-login";

pub struct XdgEntry {
    /// The `.desktop` file.
    path: Option<PathBuf>,
    /// The program to start: the AppImage, or the installed executable. `None` when this copy
    /// cannot be started again from where it is (a temporary mount, say).
    program: Option<PathBuf>,
}

impl XdgEntry {
    pub fn for_this_copy() -> Self {
        let program = super::appimage().or_else(|| std::env::current_exe().ok()?.canonicalize().ok());
        Self {
            path: dirs::config_dir().map(|d| d.join("autostart").join(format!("{BUNDLE_IDENTIFIER}.desktop"))),
            program: program.filter(|p| launchable(p)),
        }
    }

    fn wanted(&self) -> Option<String> {
        self.program.as_deref().and_then(entry_for)
    }
}

impl LoginItem for XdgEntry {
    fn status(&self) -> Status {
        let Some(wanted) = self.wanted() else {
            return Status::NotFound;
        };
        let current = self.path.as_deref().and_then(|p| std::fs::read_to_string(p).ok());
        match current {
            Some(text) if is_enabled(&text) && exec_line(&text) == exec_line(&wanted) => Status::Enabled,
            _ => Status::NotRegistered,
        }
    }

    fn register(&mut self) -> Result<(), String> {
        let (Some(path), Some(entry)) = (self.path.as_deref(), self.wanted()) else {
            return Err("this copy of Glyphio can't be started from where it is".into());
        };
        if let Some(dir) = path.parent() {
            std::fs::create_dir_all(dir).map_err(|e| format!("could not create {}: {e}", dir.display()))?;
        }
        std::fs::write(path, entry).map_err(|e| format!("could not write {}: {e}", path.display()))
    }

    fn unregister(&mut self) -> Result<(), String> {
        let Some(path) = self.path.as_deref() else {
            return Ok(());
        };
        match std::fs::remove_file(path) {
            Err(e) if e.kind() != std::io::ErrorKind::NotFound => {
                Err(format!("could not remove {}: {e}", path.display()))
            }
            _ => Ok(()),
        }
    }
}

/// Whether `program` will still be there at the next login. An AppImage's own mount and the
/// temp folder vanish; everything else is taken at its word.
fn launchable(program: &Path) -> bool {
    let text = program.to_string_lossy();
    !(text.contains("/.mount_") || program.starts_with(std::env::temp_dir()))
}

/// The autostart entry that starts `program`. `None` for a path a desktop entry can't hold.
fn entry_for(program: &Path) -> Option<String> {
    let exec = quote_exec_argument(program.to_str()?)?;
    Some(format!(
        "[Desktop Entry]\n\
         Type=Application\n\
         Name=Glyphio\n\
         Comment=Snippets, screen capture and clipboard history\n\
         Exec={exec} {AT_LOGIN_ARGUMENT}\n\
         Icon=glyphio\n\
         Terminal=false\n\
         X-GNOME-Autostart-enabled=true\n"
    ))
}

/// One `Exec` argument, quoted per the Desktop Entry spec: inside double quotes `"`, `` ` ``,
/// `$` and `\` take a backslash, `%` is doubled, and then every backslash is doubled again for
/// the file's own string escaping.
fn quote_exec_argument(arg: &str) -> Option<String> {
    if arg.contains(['\n', '\r']) {
        return None;
    }
    let mut quoted = String::from("\"");
    for c in arg.chars() {
        match c {
            '"' | '`' | '$' | '\\' => {
                quoted.push('\\');
                quoted.push(c);
            }
            '%' => quoted.push_str("%%"),
            _ => quoted.push(c),
        }
    }
    quoted.push('"');
    Some(quoted.replace('\\', "\\\\"))
}

fn exec_line(entry: &str) -> Option<&str> {
    entry.lines().find_map(|l| l.strip_prefix("Exec=")).map(str::trim)
}

/// The user can switch an entry off without deleting it (`Hidden=true`, or GNOME's own key).
fn is_enabled(entry: &str) -> bool {
    !entry.lines().map(str::trim).any(|l| {
        l.eq_ignore_ascii_case("Hidden=true") || l.eq_ignore_ascii_case("X-GNOME-Autostart-enabled=false")
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn entry(dir: &Path, program: &str) -> XdgEntry {
        XdgEntry { path: Some(dir.join("autostart/io.glyphio.app.desktop")), program: Some(program.into()) }
    }

    #[test]
    fn registering_writes_an_entry_that_reads_back_as_enabled() {
        let dir = tempfile::tempdir().unwrap();
        let mut item = entry(dir.path(), "/opt/Glyphio/Glyphio.AppImage");
        assert_eq!(item.status(), Status::NotRegistered);
        item.register().unwrap();
        assert_eq!(item.status(), Status::Enabled);
        let text = std::fs::read_to_string(dir.path().join("autostart/io.glyphio.app.desktop")).unwrap();
        assert!(text.contains("Exec=\"/opt/Glyphio/Glyphio.AppImage\" --at-login\n"), "{text}");
        item.unregister().unwrap();
        assert_eq!(item.status(), Status::NotRegistered);
        item.unregister().unwrap(); // already gone is fine
    }

    #[test]
    fn an_entry_for_another_copy_or_switched_off_is_not_ours() {
        let dir = tempfile::tempdir().unwrap();
        entry(dir.path(), "/old/Glyphio.AppImage").register().unwrap();
        assert_eq!(entry(dir.path(), "/new/Glyphio.AppImage").status(), Status::NotRegistered);

        let mut ours = entry(dir.path(), "/new/Glyphio.AppImage");
        ours.register().unwrap();
        let path = dir.path().join("autostart/io.glyphio.app.desktop");
        let off = std::fs::read_to_string(&path).unwrap() + "Hidden=true\n";
        std::fs::write(&path, off).unwrap();
        assert_eq!(ours.status(), Status::NotRegistered);
    }

    #[test]
    fn a_copy_that_cannot_be_started_again_is_not_found() {
        let item = XdgEntry { path: Some("/x".into()), program: None };
        assert_eq!(item.status(), Status::NotFound);
        assert!(!launchable(Path::new("/tmp/.mount_GlyphiAb12/usr/bin/glyphio")));
        assert!(launchable(Path::new("/home/a/Apps/Glyphio.AppImage")));
    }

    #[test]
    fn exec_arguments_are_quoted_and_escaped() {
        assert_eq!(quote_exec_argument("/a b/glyphio").unwrap(), "\"/a b/glyphio\"");
        assert_eq!(quote_exec_argument("/x$y`z\"").unwrap(), "\"/x\\\\$y\\\\`z\\\\\"\"");
        assert_eq!(quote_exec_argument("/100%/g").unwrap(), "\"/100%%/g\"");
        assert_eq!(quote_exec_argument("/back\\slash").unwrap(), "\"/back\\\\\\\\slash\"");
        assert!(quote_exec_argument("/a\nb").is_none());
    }
}
