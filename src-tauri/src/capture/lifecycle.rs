//! Pure capture lifecycle state.
//!
//! The native tray is only an adapter for this model. Keeping the transition rules here means
//! capture behaviour can be verified without ScreenCaptureKit, a running application, or a live
//! menu-bar icon.

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct CaptureToken(u64);

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ResultRoute {
    Editor,
    History(String),
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Phase {
    Idle,
    Active {
        token: CaptureToken,
        mode: String,
        delivery_session: Option<String>,
    },
    Succeeded {
        revision: u64,
        route: ResultRoute,
    },
    Failed {
        revision: u64,
        message: String,
        recovery: Option<ResultRoute>,
    },
    Acknowledged {
        revision: u64,
    },
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct CaptureInProgress {
    pub mode: String,
}

impl std::fmt::Display for CaptureInProgress {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(
            f,
            "A {} capture is already in progress.",
            mode_label(&self.mode)
        )
    }
}

impl std::error::Error for CaptureInProgress {}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Action {
    None,
    OpenResult(ResultRoute),
    ShowError(String),
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Presentation {
    pub title: &'static str,
    pub tooltip: String,
    pub menu_text: String,
    pub menu_enabled: bool,
    pub action: Action,
}

#[derive(Debug)]
pub struct Lifecycle {
    next_revision: u64,
    phase: Phase,
}

impl Default for Lifecycle {
    fn default() -> Self {
        Self {
            next_revision: 1,
            phase: Phase::Idle,
        }
    }
}

impl Lifecycle {
    pub fn begin(&mut self, mode: &str) -> Result<CaptureToken, CaptureInProgress> {
        if let Phase::Active { mode, .. } = &self.phase {
            return Err(CaptureInProgress { mode: mode.clone() });
        }
        let token = CaptureToken(self.take_revision());
        self.phase = Phase::Active {
            token,
            mode: mode.to_string(),
            delivery_session: None,
        };
        Ok(token)
    }

    pub fn active_token(&self) -> Option<CaptureToken> {
        match self.phase {
            Phase::Active { token, .. } => Some(token),
            _ => None,
        }
    }

    /// Attach the exact-once editor/worker session to the capture that produced it.
    pub fn bind_delivery(&mut self, token: CaptureToken, session_id: &str) -> bool {
        match &mut self.phase {
            Phase::Active {
                token: active,
                delivery_session,
                ..
            } if *active == token => {
                *delivery_session = Some(session_id.to_string());
                true
            }
            _ => false,
        }
    }

    /// Settle only the capture whose delivery page is reporting. A delayed old page cannot
    /// acknowledge whichever capture happens to be active now.
    pub fn finish_delivery(
        &mut self,
        session_id: &str,
        route: ResultRoute,
        error: Option<String>,
        recovery: Option<ResultRoute>,
    ) -> Option<u64> {
        let matches = matches!(
            &self.phase,
            Phase::Active {
                delivery_session: Some(active),
                ..
            } if active == session_id
        );
        if !matches {
            return None;
        }
        let revision = self.take_revision();
        self.phase = match error {
            Some(message) => Phase::Failed {
                revision,
                message,
                recovery,
            },
            None => Phase::Succeeded { revision, route },
        };
        Some(revision)
    }

    pub fn fail(&mut self, token: CaptureToken, message: String) -> Option<u64> {
        if self.active_token() != Some(token) {
            return None;
        }
        let revision = self.take_revision();
        self.phase = Phase::Failed {
            revision,
            message,
            recovery: None,
        };
        Some(revision)
    }

    pub fn fail_current(&mut self, message: String) -> Option<u64> {
        let token = self.active_token()?;
        self.fail(token, message)
    }

    pub fn cancel_current(&mut self) -> bool {
        if !matches!(self.phase, Phase::Active { .. }) {
            return false;
        }
        self.take_revision();
        self.phase = Phase::Idle;
        true
    }

    /// Show the existing lightweight acknowledgement through the same serialized feedback
    /// channel, so its delayed reset cannot erase a capture that starts in the meantime.
    pub fn acknowledge(&mut self) -> Option<u64> {
        if matches!(self.phase, Phase::Active { .. }) {
            return None;
        }
        let revision = self.take_revision();
        self.phase = Phase::Acknowledged { revision };
        Some(revision)
    }

    pub fn expire(&mut self, revision: u64) -> bool {
        let is_current = match self.phase {
            Phase::Succeeded { revision: r, .. }
            | Phase::Failed { revision: r, .. }
            | Phase::Acknowledged { revision: r } => r == revision,
            _ => false,
        };
        if is_current {
            self.phase = Phase::Idle;
        }
        is_current
    }

    pub fn presentation(&self) -> Presentation {
        match &self.phase {
            Phase::Idle => Presentation {
                title: "",
                tooltip: "Glyphio".into(),
                menu_text: "Capture status: Ready".into(),
                menu_enabled: false,
                action: Action::None,
            },
            Phase::Active { mode, .. } => Presentation {
                title: "●",
                tooltip: format!("Glyphio — Capturing {}…", mode_label(mode)),
                menu_text: format!("Capturing {}…", mode_label(mode)),
                menu_enabled: false,
                action: Action::None,
            },
            Phase::Succeeded { route, .. } => Presentation {
                title: "✓",
                tooltip: "Glyphio — Capture ready. Click for the result.".into(),
                menu_text: "Capture ready — Open result".into(),
                menu_enabled: true,
                action: Action::OpenResult(route.clone()),
            },
            Phase::Failed {
                message,
                recovery: Some(route),
                ..
            } => Presentation {
                title: "!",
                tooltip: format!(
                    "Glyphio — Capture delivery failed: {message}. Click for the saved capture."
                ),
                menu_text: "Clipboard copy failed — Open saved capture".into(),
                menu_enabled: true,
                action: Action::OpenResult(route.clone()),
            },
            Phase::Failed { message, .. } => Presentation {
                title: "!",
                tooltip: format!("Glyphio — Capture failed: {message}"),
                menu_text: "Capture failed — Show details".into(),
                menu_enabled: true,
                action: Action::ShowError(message.clone()),
            },
            Phase::Acknowledged { .. } => Presentation {
                title: "✓",
                tooltip: "Glyphio — Done".into(),
                menu_text: "Capture status: Ready".into(),
                menu_enabled: false,
                action: Action::None,
            },
        }
    }

    fn take_revision(&mut self) -> u64 {
        let revision = self.next_revision;
        self.next_revision = self.next_revision.wrapping_add(1);
        revision
    }
}

fn mode_label(mode: &str) -> &str {
    match mode {
        "visible" => "screen",
        "snip" => "area",
        "fullWindow" => "window",
        "frontWindow" => "front window",
        "pageOnly" => "browser page",
        "scrolling" => "scrolling area",
        "scrollingPage" => "scrolling page",
        _ => "image",
    }
}

#[cfg(test)]
mod tests {
    use super::{Action, Lifecycle, Phase, ResultRoute};

    #[test]
    fn exposes_the_full_observable_capture_lifecycle() {
        let mut lifecycle = Lifecycle::default();
        let token = lifecycle.begin("scrollingPage").unwrap();

        let active = lifecycle.presentation();
        assert_eq!(active.title, "●");
        assert_eq!(active.tooltip, "Glyphio — Capturing scrolling page…");
        assert!(!active.menu_enabled);

        assert!(lifecycle.bind_delivery(token, "delivery-1"));
        let revision = lifecycle
            .finish_delivery(
                "delivery-1",
                ResultRoute::History("capture-1".into()),
                None,
                None,
            )
            .unwrap();
        let succeeded = lifecycle.presentation();
        assert_eq!(succeeded.title, "✓");
        assert_eq!(
            succeeded.action,
            Action::OpenResult(ResultRoute::History("capture-1".into()))
        );

        assert!(lifecycle.expire(revision));
        assert_eq!(lifecycle.phase, Phase::Idle);
        assert_eq!(lifecycle.presentation().title, "");
    }

    #[test]
    fn reports_failure_without_losing_its_details() {
        let mut lifecycle = Lifecycle::default();
        let token = lifecycle.begin("snip").unwrap();
        assert!(lifecycle.bind_delivery(token, "failed-delivery"));
        let revision = lifecycle
            .finish_delivery(
                "failed-delivery",
                ResultRoute::Editor,
                Some("permission denied".into()),
                None,
            )
            .unwrap();

        let failed = lifecycle.presentation();
        assert_eq!(failed.title, "!");
        assert!(failed.tooltip.contains("permission denied"));
        assert_eq!(failed.action, Action::ShowError("permission denied".into()));
        assert!(lifecycle.expire(revision));
    }

    #[test]
    fn duplicate_request_does_not_disturb_an_active_scrolling_capture() {
        let mut lifecycle = Lifecycle::default();
        let first = lifecycle.begin("scrolling").unwrap();

        let error = lifecycle.begin("scrolling").unwrap_err();

        assert_eq!(error.mode, "scrolling");
        assert_eq!(lifecycle.active_token(), Some(first));
        assert_eq!(lifecycle.presentation().title, "●");
    }

    #[test]
    fn stale_delivery_and_timeout_cannot_settle_a_new_capture() {
        let mut lifecycle = Lifecycle::default();
        let first = lifecycle.begin("visible").unwrap();
        assert!(lifecycle.bind_delivery(first, "old"));
        let old_revision = lifecycle
            .finish_delivery("old", ResultRoute::Editor, None, None)
            .unwrap();
        let second = lifecycle.begin("snip").unwrap();
        assert!(lifecycle.bind_delivery(second, "new"));

        assert_eq!(
            lifecycle.finish_delivery("old", ResultRoute::Editor, None, None),
            None
        );
        assert!(!lifecycle.expire(old_revision));
        assert_eq!(lifecycle.active_token(), Some(second));
    }

    #[test]
    fn clipboard_failure_opens_the_exact_saved_capture_for_recovery() {
        let mut lifecycle = Lifecycle::default();
        let token = lifecycle.begin("snip").unwrap();
        assert!(lifecycle.bind_delivery(token, "delivery-1"));

        lifecycle.finish_delivery(
            "delivery-1",
            ResultRoute::Editor,
            Some("Clipboard copy failed: pasteboard unavailable".into()),
            Some(ResultRoute::History("capture-42".into())),
        );

        let failed = lifecycle.presentation();
        assert_eq!(failed.title, "!");
        assert_eq!(failed.menu_text, "Clipboard copy failed — Open saved capture");
        assert_eq!(
            failed.action,
            Action::OpenResult(ResultRoute::History("capture-42".into()))
        );
    }

    #[test]
    fn escape_cancellation_returns_the_indicator_to_idle() {
        let mut lifecycle = Lifecycle::default();
        lifecycle.begin("scrolling").unwrap();

        assert!(lifecycle.cancel_current());
        assert_eq!(lifecycle.presentation().title, "");
        assert!(!lifecycle.cancel_current());
    }
}
