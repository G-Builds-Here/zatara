use serde_json::Value;
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum SessionPhase {
    #[default]
    Started,
    Initialized,
    Launched,
    Terminated,
}
pub trait RelaySink: Send {
    fn on_client_message(&mut self, _message: &Value) {}
    fn on_child_message(&mut self, _message: &Value) {}
    fn on_child_exit(&mut self, _exit_code: Option<i32>) {}
}
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
