//! Pure capture lifecycle state.
//!
//! The native tray is only an adapter for this model. Keeping the transition rules here means
//! capture behaviour can be verified without ScreenCaptureKit, a running application, or a live
//! menu-bar icon.

use std::time::{Duration, Instant};

/// How long a capture may sit in each phase before the watchdog gives up on it. Without a
/// deadline one lost acknowledgement leaves the lifecycle `Active` and rejects every later
/// capture until the app restarts.
const SELECTION_BUDGET: Duration = Duration::from_secs(150);
const PIXEL_BUDGET: Duration = Duration::from_secs(30);
const DELIVERY_BUDGET: Duration = Duration::from_secs(20);
/// Each scrolling frame pushes the deadline this far ahead of now...
const HEARTBEAT_EXTENSION: Duration = Duration::from_secs(20);
/// ...but never past this long after the capture began.
const SCROLLING_CAP: Duration = Duration::from_secs(300);

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct CaptureToken(u64);

/// A validated capture kind. Strings enter through Tauri commands and generated shortcut
/// configuration, but the orchestration core should never have to reinterpret them.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum CaptureMode {
    Visible,
    Snip,
    FullWindow,
    FrontWindow,
    PageOnly,
    Scrolling,
    ScrollingPage,
}

impl CaptureMode {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Visible => "visible",
            Self::Snip => "snip",
            Self::FullWindow => "fullWindow",
            Self::FrontWindow => "frontWindow",
            Self::PageOnly => "pageOnly",
            Self::Scrolling => "scrolling",
            Self::ScrollingPage => "scrollingPage",
        }
    }

    fn label(self) -> &'static str {
        match self {
            Self::Visible => "screen",
            Self::Snip => "area",
            Self::FullWindow => "window",
            Self::FrontWindow => "front window",
            Self::PageOnly => "browser page",
            Self::Scrolling => "scrolling area",
            Self::ScrollingPage => "scrolling page",
        }
    }
}

impl TryFrom<&str> for CaptureMode {
    type Error = String;

    fn try_from(value: &str) -> Result<Self, Self::Error> {
        match value {
            "visible" => Ok(Self::Visible),
            "snip" => Ok(Self::Snip),
            "fullWindow" => Ok(Self::FullWindow),
            "frontWindow" => Ok(Self::FrontWindow),
            "pageOnly" => Ok(Self::PageOnly),
            "scrolling" => Ok(Self::Scrolling),
            "scrollingPage" => Ok(Self::ScrollingPage),
            _ => Err(format!("unknown capture mode: {value}")),
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ResultRoute {
    Editor,
    History(String),
}

/// The terminal report from the editor or silent worker. Keeping its related values together
/// prevents callers from constructing contradictory route/error/recovery combinations.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum DeliveryOutcome {
    Succeeded {
        route: ResultRoute,
    },
    Failed {
        message: String,
        recovery: Option<ResultRoute>,
    },
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Phase {
    Idle,
    Active {
        token: CaptureToken,
        mode: CaptureMode,
        delivery_session: Option<String>,
        notice: Option<String>,
        deadline: Instant,
        /// A region-drag capture is waiting on its selection overlay; set until the overlay
        /// reports a rectangle.
        selecting: bool,
        /// Latest the deadline may be extended to by heartbeats (scrolling captures only).
        cap: Option<Instant>,
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
    pub mode: CaptureMode,
}

impl std::fmt::Display for CaptureInProgress {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "A {} capture is already in progress.", self.mode.label())
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

/// The watchdog's view of the active capture at a given moment.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum DeadlineCheck {
    /// The token no longer names the active capture (it ended, or a newer one began).
    Stale,
    /// Still within its deadline; check again after this long.
    Pending(Duration),
    /// Past its deadline: the capture is now `Failed` under `revision`.
    Expired {
        revision: u64,
        mode: CaptureMode,
        phase: &'static str,
    },
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
    pub fn begin(&mut self, requested: CaptureMode) -> Result<CaptureToken, CaptureInProgress> {
        self.begin_at(requested, Instant::now())
    }

    pub fn begin_at(
        &mut self,
        requested: CaptureMode,
        now: Instant,
    ) -> Result<CaptureToken, CaptureInProgress> {
        if let Phase::Active { mode, notice, .. } = &mut self.phase {
            *notice = Some(format!(
                "Already capturing {} — the new request was ignored.",
                mode.label()
            ));
            return Err(CaptureInProgress { mode: *mode });
        }
        let token = CaptureToken(self.take_revision());
        self.phase = Phase::Active {
            token,
            mode: requested,
            delivery_session: None,
            notice: None,
            deadline: now + initial_budget(requested),
            selecting: requested == CaptureMode::Scrolling,
            cap: matches!(requested, CaptureMode::Scrolling | CaptureMode::ScrollingPage)
                .then(|| now + SCROLLING_CAP),
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
        self.bind_delivery_at(token, session_id, Instant::now())
    }

    pub fn bind_delivery_at(&mut self, token: CaptureToken, session_id: &str, now: Instant) -> bool {
        match &mut self.phase {
            Phase::Active {
                token: active,
                delivery_session,
                deadline,
                cap,
                ..
            } if *active == token => {
                *delivery_session = Some(session_id.to_string());
                // Pixels exist: the selection or scroll budget is spent, delivery has its own.
                *deadline = now + DELIVERY_BUDGET;
                *cap = None;
                true
            }
            _ => false,
        }
    }

    /// Settle only the capture whose delivery page is reporting. A delayed old page cannot
    /// acknowledge whichever capture happens to be active now.
    pub fn finish_delivery(&mut self, session_id: &str, outcome: DeliveryOutcome) -> Option<u64> {
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
        self.phase = match outcome {
            DeliveryOutcome::Failed { message, recovery } => Phase::Failed {
                revision,
                message,
                recovery,
            },
            DeliveryOutcome::Succeeded { route } => Phase::Succeeded { revision, route },
        };
        Some(revision)
    }

    /// A scrolling capture is still making progress: push its deadline out, up to the cap.
    pub fn heartbeat(&mut self) {
        self.heartbeat_at(Instant::now());
    }

    pub fn heartbeat_at(&mut self, now: Instant) {
        if let Phase::Active {
            deadline,
            cap: Some(cap),
            ..
        } = &mut self.phase
        {
            // Never shorten a budget (selection is longer than one extension).
            *deadline = (*deadline).max((now + HEARTBEAT_EXTENSION).min(*cap));
        }
    }

    /// Ask whether the capture `token` has run out of time, failing it if so. The watchdog
    /// calls this when its sleep ends; a stale token (the capture finished or a newer one is
    /// running) changes nothing.
    pub fn check_deadline(&mut self, token: CaptureToken, now: Instant) -> DeadlineCheck {
        let Phase::Active {
            token: active,
            mode,
            delivery_session,
            deadline,
            ..
        } = &self.phase
        else {
            return DeadlineCheck::Stale;
        };
        if *active != token {
            return DeadlineCheck::Stale;
        }
        if now < *deadline {
            return DeadlineCheck::Pending(*deadline - now);
        }
        let mode = *mode;
        let phase = if delivery_session.is_some() {
            "delivering"
        } else {
            "capturing"
        };
        let revision = self.take_revision();
        self.phase = Phase::Failed {
            revision,
            message: format!("The {} capture timed out while {phase}.", mode.label()),
            recovery: None,
        };
        DeadlineCheck::Expired {
            revision,
            mode,
            phase,
        }
    }

    /// Return to idle whatever the state, e.g. from the tray's Reload. Revisions are spent so
    /// nothing scheduled for the discarded capture can act on a later one.
    pub fn reset(&mut self) {
        self.take_revision();
        self.phase = Phase::Idle;
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

    /// The user backed out of the capture `token` (for example dismissed the picker). Unlike a
    /// failure this shows nothing, and a late cancel cannot touch a newer capture.
    pub fn cancel(&mut self, token: CaptureToken) -> bool {
        if self.active_token() != Some(token) {
            return false;
        }
        self.cancel_current()
    }

    /// The selection overlay produced a rectangle: closing it now starts the capture rather
    /// than abandoning it.
    pub fn selection_made(&mut self, token: CaptureToken) {
        if let Phase::Active {
            token: active,
            selecting,
            ..
        } = &mut self.phase
        {
            if *active == token {
                *selecting = false;
            }
        }
    }

    /// The selection overlay went away without a rectangle, by any route. Cancels only a
    /// capture that is still waiting on it.
    pub fn cancel_selection(&mut self) -> bool {
        if matches!(self.phase, Phase::Active { selecting: true, .. }) {
            return self.cancel_current();
        }
        false
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
            Phase::Active {
                mode,
                notice: Some(notice),
                ..
            } => Presentation {
                title: "●",
                tooltip: format!("Glyphio — {notice} Capturing {}…", mode.label()),
                menu_text: notice.clone(),
                menu_enabled: false,
                action: Action::None,
            },
            Phase::Active { mode, .. } => Presentation {
                title: "●",
                tooltip: format!("Glyphio — Capturing {}…", mode.label()),
                menu_text: format!("Capturing {}…", mode.label()),
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

fn initial_budget(mode: CaptureMode) -> Duration {
    match mode {
        // Includes the time the user spends choosing a region or window.
        CaptureMode::Snip | CaptureMode::FullWindow | CaptureMode::Scrolling => SELECTION_BUDGET,
        // Scrolling frames extend this through `heartbeat`.
        CaptureMode::Visible
        | CaptureMode::FrontWindow
        | CaptureMode::PageOnly
        | CaptureMode::ScrollingPage => PIXEL_BUDGET,
    }
}

#[cfg(test)]
mod tests {
    use super::{
        Action, CaptureMode, DeadlineCheck, DeliveryOutcome, Lifecycle, Phase, ResultRoute,
    };
    use std::time::{Duration, Instant};

    fn secs(n: u64) -> Duration {
        Duration::from_secs(n)
    }

    #[test]
    fn a_capture_past_its_deadline_fails_and_a_new_capture_can_start() {
        let t0 = Instant::now();
        let mut lifecycle = Lifecycle::default();
        let token = lifecycle.begin_at(CaptureMode::Visible, t0).unwrap();

        assert_eq!(
            lifecycle.check_deadline(token, t0 + secs(29)),
            DeadlineCheck::Pending(secs(1))
        );
        let DeadlineCheck::Expired { revision, mode, phase } =
            lifecycle.check_deadline(token, t0 + secs(30))
        else {
            panic!("the capture should have expired");
        };
        assert_eq!((mode, phase), (CaptureMode::Visible, "capturing"));
        assert_eq!(lifecycle.presentation().title, "!");
        assert!(lifecycle.presentation().tooltip.contains("timed out"));
        assert!(lifecycle.expire(revision));

        assert!(lifecycle.begin_at(CaptureMode::Snip, t0 + secs(31)).is_ok());
    }

    #[test]
    fn selection_gets_longer_than_pixel_capture_and_delivery_gets_its_own_budget() {
        let t0 = Instant::now();
        let mut lifecycle = Lifecycle::default();
        let token = lifecycle.begin_at(CaptureMode::Snip, t0).unwrap();
        assert_eq!(
            lifecycle.check_deadline(token, t0 + secs(100)),
            DeadlineCheck::Pending(secs(50))
        );

        assert!(lifecycle.bind_delivery_at(token, "s1", t0 + secs(100)));
        assert_eq!(
            lifecycle.check_deadline(token, t0 + secs(119)),
            DeadlineCheck::Pending(secs(1))
        );
        let DeadlineCheck::Expired { phase, .. } = lifecycle.check_deadline(token, t0 + secs(120))
        else {
            panic!("delivery should have expired");
        };
        assert_eq!(phase, "delivering");
    }

    #[test]
    fn a_stale_expiry_cannot_fail_a_newer_capture() {
        let t0 = Instant::now();
        let mut lifecycle = Lifecycle::default();
        let first = lifecycle.begin_at(CaptureMode::Visible, t0).unwrap();
        lifecycle.cancel_current();
        let second = lifecycle.begin_at(CaptureMode::Visible, t0 + secs(10)).unwrap();

        assert_eq!(
            lifecycle.check_deadline(first, t0 + secs(35)),
            DeadlineCheck::Stale
        );
        assert_eq!(lifecycle.active_token(), Some(second));
        assert_eq!(
            lifecycle.check_deadline(second, t0 + secs(35)),
            DeadlineCheck::Pending(secs(5))
        );
    }

    #[test]
    fn a_scrolling_heartbeat_extends_the_deadline_up_to_the_cap() {
        let t0 = Instant::now();
        let mut lifecycle = Lifecycle::default();
        let token = lifecycle.begin_at(CaptureMode::ScrollingPage, t0).unwrap();

        // Frames keep arriving, so the 30 s initial budget never runs out.
        for second in (10..=290).step_by(10) {
            lifecycle.heartbeat_at(t0 + secs(second));
            assert!(matches!(
                lifecycle.check_deadline(token, t0 + secs(second)),
                DeadlineCheck::Pending(_)
            ));
        }
        // The last beat cannot push past five minutes in total.
        lifecycle.heartbeat_at(t0 + secs(295));
        assert_eq!(
            lifecycle.check_deadline(token, t0 + secs(299)),
            DeadlineCheck::Pending(secs(1))
        );
        assert!(matches!(
            lifecycle.check_deadline(token, t0 + secs(300)),
            DeadlineCheck::Expired { .. }
        ));
    }

    #[test]
    fn without_a_heartbeat_a_scrolling_capture_expires() {
        let t0 = Instant::now();
        let mut lifecycle = Lifecycle::default();
        let token = lifecycle.begin_at(CaptureMode::ScrollingPage, t0).unwrap();
        lifecycle.heartbeat_at(t0 + secs(10));
        assert!(matches!(
            lifecycle.check_deadline(token, t0 + secs(31)),
            DeadlineCheck::Expired { .. }
        ));
    }

    #[test]
    fn cancelling_a_capture_clears_the_indicator_without_a_failure() {
        let mut lifecycle = Lifecycle::default();
        let token = lifecycle.begin(CaptureMode::Snip).unwrap();

        assert!(lifecycle.cancel(token));

        assert_eq!(lifecycle.phase, Phase::Idle);
        assert_eq!(lifecycle.presentation().title, "");
        assert!(lifecycle.begin(CaptureMode::Snip).is_ok());
    }

    #[test]
    fn a_late_cancel_cannot_cancel_a_newer_capture() {
        let mut lifecycle = Lifecycle::default();
        let old = lifecycle.begin(CaptureMode::Snip).unwrap();
        lifecycle.cancel(old);
        let newer = lifecycle.begin(CaptureMode::Visible).unwrap();

        assert!(!lifecycle.cancel(old));
        assert_eq!(lifecycle.active_token(), Some(newer));
    }

    #[test]
    fn abandoning_the_selection_overlay_cancels_a_capture_still_selecting() {
        let mut lifecycle = Lifecycle::default();
        lifecycle.begin(CaptureMode::Scrolling).unwrap();

        assert!(lifecycle.cancel_selection());

        assert_eq!(lifecycle.presentation().title, "");
        assert!(!lifecycle.cancel_selection());
    }

    #[test]
    fn closing_the_overlay_to_start_the_capture_does_not_cancel_it() {
        let mut lifecycle = Lifecycle::default();
        let token = lifecycle.begin(CaptureMode::Scrolling).unwrap();

        lifecycle.selection_made(token);

        assert!(!lifecycle.cancel_selection());
        assert_eq!(lifecycle.active_token(), Some(token));
    }

    #[test]
    fn abandoning_a_selection_never_touches_a_capture_that_has_no_overlay() {
        let mut lifecycle = Lifecycle::default();
        let token = lifecycle.begin(CaptureMode::Visible).unwrap();

        assert!(!lifecycle.cancel_selection());
        assert_eq!(lifecycle.active_token(), Some(token));
    }

    #[test]
    fn reset_returns_a_stuck_capture_to_idle_and_voids_its_deadline() {
        let t0 = Instant::now();
        let mut lifecycle = Lifecycle::default();
        let stuck = lifecycle.begin_at(CaptureMode::Visible, t0).unwrap();

        lifecycle.reset();

        assert_eq!(lifecycle.phase, Phase::Idle);
        assert_eq!(
            lifecycle.check_deadline(stuck, t0 + secs(60)),
            DeadlineCheck::Stale
        );
        assert!(lifecycle.begin_at(CaptureMode::Visible, t0 + secs(1)).is_ok());
    }

    #[test]
    fn capture_mode_is_validated_once_at_the_string_boundary() {
        assert_eq!(CaptureMode::try_from("frontWindow"), Ok(CaptureMode::FrontWindow));
        assert_eq!(
            CaptureMode::try_from("front-window"),
            Err("unknown capture mode: front-window".into())
        );
    }

    #[test]
    fn exposes_the_full_observable_capture_lifecycle() {
        let mut lifecycle = Lifecycle::default();
        let token = lifecycle.begin(CaptureMode::ScrollingPage).unwrap();

        let active = lifecycle.presentation();
        assert_eq!(active.title, "●");
        assert_eq!(active.tooltip, "Glyphio — Capturing scrolling page…");
        assert!(!active.menu_enabled);

        assert!(lifecycle.bind_delivery(token, "delivery-1"));
        let revision = lifecycle
            .finish_delivery(
                "delivery-1",
                DeliveryOutcome::Succeeded {
                    route: ResultRoute::History("capture-1".into()),
                },
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
        let token = lifecycle.begin(CaptureMode::Snip).unwrap();
        assert!(lifecycle.bind_delivery(token, "failed-delivery"));
        let revision = lifecycle
            .finish_delivery(
                "failed-delivery",
                DeliveryOutcome::Failed {
                    message: "permission denied".into(),
                    recovery: None,
                },
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
        let first = lifecycle.begin(CaptureMode::Scrolling).unwrap();

        let error = lifecycle.begin(CaptureMode::Scrolling).unwrap_err();

        assert_eq!(error.mode, CaptureMode::Scrolling);
        assert_eq!(lifecycle.active_token(), Some(first));
        assert_eq!(lifecycle.presentation().title, "●");
        assert_eq!(
            lifecycle.presentation().menu_text,
            "Already capturing scrolling area — the new request was ignored."
        );
    }

    #[test]
    fn stale_delivery_and_timeout_cannot_settle_a_new_capture() {
        let mut lifecycle = Lifecycle::default();
        let first = lifecycle.begin(CaptureMode::Visible).unwrap();
        assert!(lifecycle.bind_delivery(first, "old"));
        let old_revision = lifecycle
            .finish_delivery(
                "old",
                DeliveryOutcome::Succeeded {
                    route: ResultRoute::Editor,
                },
            )
            .unwrap();
        let second = lifecycle.begin(CaptureMode::Snip).unwrap();
        assert!(lifecycle.bind_delivery(second, "new"));

        assert_eq!(
            lifecycle.finish_delivery(
                "old",
                DeliveryOutcome::Succeeded {
                    route: ResultRoute::Editor,
                },
            ),
            None
        );
        assert!(!lifecycle.expire(old_revision));
        assert_eq!(lifecycle.active_token(), Some(second));
    }

    #[test]
    fn clipboard_failure_opens_the_exact_saved_capture_for_recovery() {
        let mut lifecycle = Lifecycle::default();
        let token = lifecycle.begin(CaptureMode::Snip).unwrap();
        assert!(lifecycle.bind_delivery(token, "delivery-1"));

        lifecycle.finish_delivery(
            "delivery-1",
            DeliveryOutcome::Failed {
                message: "Clipboard copy failed: pasteboard unavailable".into(),
                recovery: Some(ResultRoute::History("capture-42".into())),
            },
        );

        let failed = lifecycle.presentation();
        assert_eq!(failed.title, "!");
        assert_eq!(
            failed.menu_text,
            "Clipboard copy failed — Open saved capture"
        );
        assert_eq!(
            failed.action,
            Action::OpenResult(ResultRoute::History("capture-42".into()))
        );
    }

    #[test]
    fn escape_cancellation_returns_the_indicator_to_idle() {
        let mut lifecycle = Lifecycle::default();
        lifecycle.begin(CaptureMode::Scrolling).unwrap();

        assert!(lifecycle.cancel_current());
        assert_eq!(lifecycle.presentation().title, "");
        assert!(!lifecycle.cancel_current());
    }
}
