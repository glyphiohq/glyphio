//! Exactly-once handoff for completed captures.
//!
//! Window pages are delivery adapters: they name the session they were opened for and may
//! consume only that result through this module. A stale page therefore cannot take a newer
//! capture just because it happens to ask after the newer capture completed.

use std::collections::HashMap;

use uuid::Uuid;

/// The adapter that is allowed to consume a completed capture.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum DeliveryRoute {
    Editor,
    Silent,
}

/// Stable identity assigned when a capture completes, before a window consumes it.
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub struct DeliverySessionId(String);

impl DeliverySessionId {
    pub fn as_str(&self) -> &str {
        &self.0
    }

    pub fn parse(value: String) -> Option<Self> {
        Uuid::parse_str(&value).ok().map(|_| Self(value))
    }
}

struct DeliverySession<T> {
    route: DeliveryRoute,
    result: T,
}

/// Holds completed results until the delivery adapter acknowledged by `consume` receives one.
pub struct CaptureDeliverySessions<T> {
    pending: HashMap<DeliverySessionId, DeliverySession<T>>,
}

impl<T> CaptureDeliverySessions<T> {
    pub fn new() -> Self {
        Self {
            pending: HashMap::new(),
        }
    }

    /// Register a completed result and return the identity that must be presented to consume it.
    pub fn complete(&mut self, route: DeliveryRoute, result: T) -> DeliverySessionId {
        let id = DeliverySessionId(Uuid::new_v4().to_string());
        self.pending
            .insert(id.clone(), DeliverySession { route, result });
        id
    }

    /// Acknowledge and return a result only when this route owns the named session.
    ///
    /// A route mismatch deliberately leaves the session pending, so an interrupted worker can
    /// still recover its own result without letting another window consume it.
    pub fn consume(&mut self, id: &DeliverySessionId, route: DeliveryRoute) -> Option<T> {
        let owns_session = self
            .pending
            .get(id)
            .is_some_and(|session| session.route == route);
        owns_session.then(|| {
            self.pending
                .remove(id)
                .expect("checked pending session")
                .result
        })
    }
}

impl<T> Default for CaptureDeliverySessions<T> {
    fn default() -> Self {
        Self::new()
    }
}

impl DeliveryRoute {
    pub fn from_silent(silent: bool) -> Self {
        if silent {
            Self::Silent
        } else {
            Self::Editor
        }
    }
}

#[cfg(test)]
mod tests {
    use super::{CaptureDeliverySessions, DeliveryRoute};

    #[test]
    fn delivers_a_visible_result_only_to_its_own_session() {
        let mut sessions = CaptureDeliverySessions::new();
        let first = sessions.complete(DeliveryRoute::Editor, "first");
        let second = sessions.complete(DeliveryRoute::Editor, "second");

        assert_eq!(
            sessions.consume(&first, DeliveryRoute::Editor),
            Some("first")
        );
        assert_eq!(
            sessions.consume(&second, DeliveryRoute::Editor),
            Some("second")
        );
    }

    #[test]
    fn does_not_consume_a_session_from_the_wrong_delivery_route() {
        let mut sessions = CaptureDeliverySessions::new();
        let session = sessions.complete(DeliveryRoute::Silent, "capture");

        assert_eq!(sessions.consume(&session, DeliveryRoute::Editor), None);
        assert_eq!(
            sessions.consume(&session, DeliveryRoute::Silent),
            Some("capture")
        );
    }

    #[test]
    fn consumes_a_completed_result_exactly_once() {
        let mut sessions = CaptureDeliverySessions::new();
        let session = sessions.complete(DeliveryRoute::Editor, "capture");

        assert_eq!(
            sessions.consume(&session, DeliveryRoute::Editor),
            Some("capture")
        );
        assert_eq!(sessions.consume(&session, DeliveryRoute::Editor), None);
    }
}
