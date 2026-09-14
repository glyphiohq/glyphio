//! Pure end-to-end capture orchestration.
//!
//! This is the highest seam below Tauri: lifecycle feedback and exact-once delivery sessions
//! move under one lock in production and can be exercised together without a live application.

use super::delivery::{CaptureDeliverySessions, DeliveryRoute, DeliverySessionId};
pub use super::lifecycle::DeliveryOutcome;
use super::lifecycle::{CaptureInProgress, CaptureMode, CaptureToken, Lifecycle, Presentation};

pub struct CaptureOrchestration<T> {
    lifecycle: Lifecycle,
    deliveries: CaptureDeliverySessions<T>,
}

impl<T> Default for CaptureOrchestration<T> {
    fn default() -> Self {
        Self {
            lifecycle: Lifecycle::default(),
            deliveries: CaptureDeliverySessions::default(),
        }
    }
}

impl<T> CaptureOrchestration<T> {
    pub fn begin(&mut self, mode: CaptureMode) -> Result<CaptureToken, CaptureInProgress> {
        self.lifecycle.begin(mode)
    }

    pub fn active_token(&self) -> Option<CaptureToken> {
        self.lifecycle.active_token()
    }

    /// Complete pixel capture and bind its exact-once session to the matching lifecycle token.
    /// The token check and session insertion occur within one mutable orchestration boundary, so
    /// a stale capture cannot leave behind a payload or bind itself to a newer capture.
    pub fn complete(
        &mut self,
        token: CaptureToken,
        route: DeliveryRoute,
        result: T,
    ) -> Option<DeliverySessionId> {
        if self.lifecycle.active_token() != Some(token) {
            return None;
        }
        let session_id = self.deliveries.complete(route, result);
        let bound = self.lifecycle.bind_delivery(token, session_id.as_str());
        debug_assert!(
            bound,
            "active token changed inside an exclusive orchestration borrow"
        );
        Some(session_id)
    }

    pub fn consume(&mut self, id: &DeliverySessionId, route: DeliveryRoute) -> Option<T> {
        self.deliveries.consume(id, route)
    }

    pub fn finish_delivery(&mut self, session_id: &str, outcome: DeliveryOutcome) -> Option<u64> {
        self.lifecycle.finish_delivery(session_id, outcome)
    }

    pub fn fail(&mut self, token: CaptureToken, message: String) -> Option<u64> {
        self.lifecycle.fail(token, message)
    }

    pub fn fail_current(&mut self, message: String) -> Option<u64> {
        self.lifecycle.fail_current(message)
    }

    pub fn cancel_current(&mut self) -> bool {
        self.lifecycle.cancel_current()
    }

    pub fn acknowledge(&mut self) -> Option<u64> {
        self.lifecycle.acknowledge()
    }

    pub fn expire(&mut self, revision: u64) -> bool {
        self.lifecycle.expire(revision)
    }

    pub fn presentation(&self) -> Presentation {
        self.lifecycle.presentation()
    }
}

#[cfg(test)]
mod tests {
    use super::{CaptureOrchestration, DeliveryOutcome};
    use crate::capture::delivery::DeliveryRoute;
    use crate::capture::lifecycle::{Action, CaptureMode, ResultRoute};

    #[test]
    fn visible_delivery_is_acknowledged_once_end_to_end() {
        let mut capture = CaptureOrchestration::default();
        let token = capture.begin(CaptureMode::Visible).unwrap();
        assert_eq!(capture.presentation().menu_text, "Capturing screen…");

        let session = capture
            .complete(token, DeliveryRoute::Editor, "visible payload")
            .unwrap();
        assert_eq!(
            capture.consume(&session, DeliveryRoute::Editor),
            Some("visible payload")
        );
        assert_eq!(capture.consume(&session, DeliveryRoute::Editor), None);

        capture.finish_delivery(
            session.as_str(),
            DeliveryOutcome::Succeeded {
                route: ResultRoute::Editor,
            },
        );
        assert_eq!(
            capture.presentation().action,
            Action::OpenResult(ResultRoute::Editor)
        );
    }

    #[test]
    fn silent_delivery_requires_its_route_and_acknowledges_history() {
        let mut capture = CaptureOrchestration::default();
        let token = capture.begin(CaptureMode::Snip).unwrap();
        let session = capture
            .complete(token, DeliveryRoute::Silent, "silent payload")
            .unwrap();

        assert_eq!(capture.consume(&session, DeliveryRoute::Editor), None);
        assert_eq!(
            capture.consume(&session, DeliveryRoute::Silent),
            Some("silent payload")
        );
        capture.finish_delivery(
            session.as_str(),
            DeliveryOutcome::Succeeded {
                route: ResultRoute::History("capture-1".into()),
            },
        );
        assert_eq!(
            capture.presentation().action,
            Action::OpenResult(ResultRoute::History("capture-1".into()))
        );
    }

    #[test]
    fn delayed_acknowledgement_cannot_settle_a_new_capture() {
        let mut capture = CaptureOrchestration::default();
        let first_token = capture.begin(CaptureMode::Visible).unwrap();
        let first = capture
            .complete(first_token, DeliveryRoute::Editor, "first")
            .unwrap();
        capture.finish_delivery(
            first.as_str(),
            DeliveryOutcome::Succeeded {
                route: ResultRoute::Editor,
            },
        );

        let second_token = capture.begin(CaptureMode::ScrollingPage).unwrap();
        let second = capture
            .complete(second_token, DeliveryRoute::Silent, "second")
            .unwrap();
        assert_eq!(
            capture.finish_delivery(
                first.as_str(),
                DeliveryOutcome::Failed {
                    message: "late failure".into(),
                    recovery: None,
                },
            ),
            None
        );
        assert_eq!(
            capture.consume(&first, DeliveryRoute::Editor),
            Some("first"),
            "a stale page can receive only its own payload"
        );
        assert_eq!(
            capture.consume(&second, DeliveryRoute::Silent),
            Some("second")
        );
        assert_eq!(
            capture.presentation().menu_text,
            "Capturing scrolling page…"
        );
    }

    #[test]
    fn clipboard_failure_exposes_recovery_and_allows_a_clean_retry() {
        let mut capture = CaptureOrchestration::default();
        let token = capture.begin(CaptureMode::Snip).unwrap();
        let failed = capture
            .complete(token, DeliveryRoute::Silent, "failed payload")
            .unwrap();
        assert_eq!(
            capture.consume(&failed, DeliveryRoute::Silent),
            Some("failed payload")
        );
        capture.finish_delivery(
            failed.as_str(),
            DeliveryOutcome::Failed {
                message: "Clipboard copy failed".into(),
                recovery: Some(ResultRoute::History("capture-42".into())),
            },
        );
        assert_eq!(
            capture.presentation().action,
            Action::OpenResult(ResultRoute::History("capture-42".into()))
        );

        let retry = capture.begin(CaptureMode::Snip).unwrap();
        assert!(capture
            .complete(retry, DeliveryRoute::Editor, "retry payload")
            .is_some());
        assert_eq!(capture.presentation().menu_text, "Capturing area…");
    }
}
