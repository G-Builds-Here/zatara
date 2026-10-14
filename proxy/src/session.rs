//! Session state and the relay-sink seam.
//!
//! These are the extension points the build-orchestration (LOC-0109) and
//! output-pumping (LOC-0111) stories plug into — the relay itself never
//! depends on them; it calls them.

use serde_json::Value;

/// Where the session stands in the DAP handshake.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum SessionPhase {
    /// Spawned; no `initialize` yet.
    #[default]
    Started,
    /// An `initialize` request has been seen.
    Initialized,
    /// A `launch` request passed zatara-types validation.
    Launched,
    /// A `disconnect` was requested or the session ended.
    Terminated,
}

/// Observation hooks the relay calls at the wire boundaries. LOC-0111 (test
/// output pumping) consumes `on_child_message` and synthesizes through the
/// relay; LOC-0109 (build orchestration) consumes `on_client_message` on the
/// pre-launch path. The sink observes and annotates; it never blocks or
/// rewrites the relayed stream.
pub trait RelaySink: Send {
    fn on_client_message(&mut self, _message: &Value) {}
    fn on_child_message(&mut self, _message: &Value) {}
    fn on_child_exit(&mut self, _exit_code: Option<i32>) {}
}

/// Default sink for the shipping binary: the stdio relay *is* the output.
#[derive(Default)]
pub struct NullSink;

impl RelaySink for NullSink {}

#[derive(Debug, Default)]
pub struct Session {
    phase: SessionPhase,
}

impl Session {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn phase(&self) -> SessionPhase {
        self.phase
    }

    /// Advance the phase on a client request. `launch` is deliberately not
    /// handled here: the phase machine only sees the raw request, and
    /// `Launched` means the launch *passed validation* — a transition only
    /// the relay, which runs the validation, may grant via
    /// [`Session::accept_launch`].
    pub fn advance_on_client_message(&mut self, message: &Value) {
        if message.get("type").and_then(Value::as_str) != Some("request") {
            return;
        }
        match message.get("command").and_then(Value::as_str) {
            Some("initialize") => self.phase = SessionPhase::Initialized,
            Some("disconnect") => self.phase = SessionPhase::Terminated,
            _ => {}
        }
    }

    /// The relay calls this once a `launch` request has passed zatara-types
    /// validation — the only path to [`SessionPhase::Launched`]. A launch
    /// rejected by validation must leave the phase where it stands, so
    /// consumers of the phase (LOC-0109's pre-launch path) never observe a
    /// launch that was never accepted.
    pub fn accept_launch(&mut self) {
        if matches!(self.phase, SessionPhase::Started | SessionPhase::Initialized) {
            self.phase = SessionPhase::Launched;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn session_walks_started_initialized_launched_terminated() {
        let mut session = Session::new();
        assert_eq!(session.phase(), SessionPhase::Started);
        session.advance_on_client_message(&json!({"seq":1,"type":"request","command":"initialize"}));
        assert_eq!(session.phase(), SessionPhase::Initialized);
        session.advance_on_client_message(&json!({"seq":2,"type":"request","command":"launch"}));
        session.accept_launch();
        assert_eq!(session.phase(), SessionPhase::Launched);
        session.advance_on_client_message(&json!({"seq":3,"type":"request","command":"disconnect"}));
        assert_eq!(session.phase(), SessionPhase::Terminated);
    }

    #[test]
    fn a_launch_seen_before_validation_leaves_the_phase_at_initialized() {
        // Launched means "passed zatara-types validation". The phase machine
        // only ever sees the raw request — so seeing a launch must not, by
        // itself, advance past Initialized; the relay grants that transition
        // separately, once validation returns Ok.
        let mut session = Session::new();
        session.advance_on_client_message(&json!({"seq":1,"type":"request","command":"initialize"}));
        session.advance_on_client_message(&json!({"seq":2,"type":"request","command":"launch"}));
        assert_eq!(
            session.phase(),
            SessionPhase::Initialized,
            "a launch that validation has not accepted must not read as launched"
        );
        session.accept_launch();
        assert_eq!(
            session.phase(),
            SessionPhase::Launched,
            "the accept — not the request — is what launches the session"
        );
    }

    #[test]
    fn non_requests_and_plain_stepping_do_not_move_the_phase() {
        let mut session = Session::new();
        session.advance_on_client_message(&json!({"type":"event","event":"output"}));
        session.advance_on_client_message(&json!({"seq":1,"type":"request","command":"next"}));
        assert_eq!(session.phase(), SessionPhase::Started);
    }

    #[test]
    fn the_sink_seam_stays_default_noop_over_a_trait_object() {
        let mut sink: Box<dyn RelaySink> = Box::new(NullSink);
        sink.on_client_message(&json!({}));
        sink.on_child_message(&json!({}));
        sink.on_child_exit(Some(0));
    }
}
