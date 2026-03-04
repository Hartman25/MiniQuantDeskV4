use serde::{Deserialize, Serialize};

/// Runtime armed state (what it is doing now).
#[derive(Debug, Clone, Copy, Serialize, Deserialize)]
pub enum RuntimeArmedState {
    Armed,
    Disarmed,
}

/// A periodic status snapshot that the daemon/GUI can display.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RuntimeStatusSnapshot {
    pub node_id: String,
    pub epoch: i64,
    pub armed_state: RuntimeArmedState,
    pub lease_expires_at_utc: String,
}

/// Scaffold only: the runtime should publish this snapshot periodically,
/// either in-memory for the daemon, or persisted to DB.
pub trait RuntimeStatusPublisher {
    fn publish(&self, snapshot: RuntimeStatusSnapshot);
}
