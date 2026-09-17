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

#[cfg(test)]
mod tests {
    use super::should_open_settings;

    #[test]
    fn a_login_launch_starts_without_the_settings_window() {
        assert!(!should_open_settings(["glyphio", "--hidden"]));
        // A legacy LaunchAgent can start the freshly upgraded app once before the migration
        // removes it, so keep that launch silent too.
        assert!(!should_open_settings(["glyphio", "--launch-at-login"]));
        assert!(should_open_settings(["glyphio"]));
    }
}
