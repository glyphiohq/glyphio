/// Argument supplied by the macOS Login Items backend, not a user-facing command-line interface.
///
/// `tauri-plugin-autostart` recognizes `--hidden` for the Login Items implementation; the old
/// custom flag is retained below only to keep an in-flight upgrade silent.
pub const LOGIN_LAUNCH_ARGUMENT: &str = "--hidden";
const LEGACY_LOGIN_LAUNCH_ARGUMENT: &str = "--launch-at-login";

/// Whether startup should present the regular Settings window.
pub fn should_open_settings(args: impl IntoIterator<Item = impl AsRef<str>>) -> bool {
    !args.into_iter().any(|argument| {
        matches!(
            argument.as_ref(),
            LOGIN_LAUNCH_ARGUMENT | LEGACY_LOGIN_LAUNCH_ARGUMENT
        )
    })
}

/// What the already-running copy does when another process is started and exits.
#[derive(Debug, PartialEq, Eq)]
pub enum SecondLaunch {
    /// A user launch (Finder, Dock, direct execution, a link): bring Settings forward.
    ShowSettings,
    /// A login launch while we are already running: stay quiet.
    Ignore,
}

/// Decide how to react to a second launch from its arguments.
pub fn second_launch_action(args: impl IntoIterator<Item = impl AsRef<str>>) -> SecondLaunch {
    if should_open_settings(args) {
        SecondLaunch::ShowSettings
    } else {
        SecondLaunch::Ignore
    }
}

#[cfg(test)]
mod tests {
    use super::{second_launch_action, should_open_settings, SecondLaunch};

    #[test]
    fn a_second_user_launch_surfaces_settings_in_the_running_copy() {
        assert_eq!(
            second_launch_action(["glyphio"]),
            SecondLaunch::ShowSettings
        );
        // A Glyphio link carries its own handling, but the running copy still comes forward.
        assert_eq!(
            second_launch_action(["glyphio", "glyphio://join?code=x"]),
            SecondLaunch::ShowSettings
        );
    }

    #[test]
    fn a_second_login_launch_does_nothing() {
        assert_eq!(
            second_launch_action(["glyphio", "--hidden"]),
            SecondLaunch::Ignore
        );
        assert_eq!(
            second_launch_action(["glyphio", "--launch-at-login"]),
            SecondLaunch::Ignore
        );
    }

    #[test]
    fn a_login_launch_starts_without_the_settings_window() {
        assert!(!should_open_settings(["glyphio", "--hidden"]));
        // A legacy LaunchAgent can start the freshly upgraded app once before the migration
        // removes it, so keep that launch silent too.
        assert!(!should_open_settings(["glyphio", "--launch-at-login"]));
        assert!(should_open_settings(["glyphio"]));
    }
}
