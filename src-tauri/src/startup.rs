/// Argument supplied by the macOS login item, not a user-facing command-line interface.
pub const LOGIN_LAUNCH_ARGUMENT: &str = "--launch-at-login";

/// Whether startup should present the regular Settings window.
pub fn should_open_settings(args: impl IntoIterator<Item = impl AsRef<str>>) -> bool {
    !args
        .into_iter()
        .any(|argument| argument.as_ref() == LOGIN_LAUNCH_ARGUMENT)
}

#[cfg(test)]
mod tests {
    use super::should_open_settings;

    #[test]
    fn a_login_launch_starts_without_the_settings_window() {
        assert!(!should_open_settings(["glyphio", "--launch-at-login"]));
        assert!(should_open_settings(["glyphio"]));
    }
}
