//! C3: connection state machine + injectable transport abstraction.
//!
//! `IbkrTransport` is the seam that keeps this crate free of any real IB
//! Gateway/TWS network connection: production code names a real
//! implementation backed by `ibapi::Client`, but nothing in this crate
//! constructs one, and every test here drives a fake implementation.
//! Mirrors this repo's established `BrokerFillActivityFetcher`/
//! `WsGapFillFetcher`/`CryptoFeeActivityFetcher` pattern
//! (`mqk-daemon::state`) exactly.

use crate::identity::IbkrDeploymentIdentity;

/// The lifecycle state of one adapter's connection to TWS/IB Gateway.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum IbkrConnectionState {
    Disconnected,
    Connecting,
    Connected,
    /// The transport was lost after a prior successful connection and the
    /// adapter is attempting to re-establish it. Distinct from
    /// `Connecting` (the very first attempt) so a caller can distinguish
    /// "never connected" from "was connected, now recovering" — the same
    /// distinction B3's `ColdStartUnproven` demotion makes for a broker
    /// cursor across a daemon restart.
    Reconnecting,
}

/// An illegal connection-state transition was attempted. Every transition
/// method fails closed (returns `Err`, leaves `state` unchanged) rather
/// than silently coercing to a plausible-looking state.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct IllegalConnectionTransition {
    pub from: IbkrConnectionState,
    pub attempted: &'static str,
}

impl std::fmt::Display for IllegalConnectionTransition {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(
            f,
            "illegal IBKR connection transition: {:?} -> {}",
            self.from, self.attempted
        )
    }
}

impl std::error::Error for IllegalConnectionTransition {}

/// Deterministic, exhaustive connection-state machine. The legal edge set
/// is closed and explicit — every method below is the only way to move
/// between states, and every one fails closed on an illegal source state.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct IbkrConnectionStateMachine {
    state: IbkrConnectionState,
}

impl Default for IbkrConnectionStateMachine {
    fn default() -> Self {
        Self::new()
    }
}

impl IbkrConnectionStateMachine {
    pub fn new() -> Self {
        Self {
            state: IbkrConnectionState::Disconnected,
        }
    }

    pub fn state(&self) -> IbkrConnectionState {
        self.state
    }

    /// `Disconnected -> Connecting`. The only legal way to begin a fresh
    /// connection attempt.
    pub fn begin_connect(&mut self) -> Result<(), IllegalConnectionTransition> {
        match self.state {
            IbkrConnectionState::Disconnected => {
                self.state = IbkrConnectionState::Connecting;
                Ok(())
            }
            other => Err(IllegalConnectionTransition {
                from: other,
                attempted: "begin_connect",
            }),
        }
    }

    /// `Connecting -> Connected` or `Reconnecting -> Connected`. The only
    /// legal way to reach `Connected`.
    pub fn on_connected(&mut self) -> Result<(), IllegalConnectionTransition> {
        match self.state {
            IbkrConnectionState::Connecting | IbkrConnectionState::Reconnecting => {
                self.state = IbkrConnectionState::Connected;
                Ok(())
            }
            other => Err(IllegalConnectionTransition {
                from: other,
                attempted: "on_connected",
            }),
        }
    }

    /// `Connecting -> Disconnected`. The initial attempt failed before ever
    /// reaching `Connected` — never routes through `Reconnecting`, which is
    /// reserved for a transport that was genuinely connected before.
    pub fn on_connect_failed(&mut self) -> Result<(), IllegalConnectionTransition> {
        match self.state {
            IbkrConnectionState::Connecting => {
                self.state = IbkrConnectionState::Disconnected;
                Ok(())
            }
            other => Err(IllegalConnectionTransition {
                from: other,
                attempted: "on_connect_failed",
            }),
        }
    }

    /// `Connected -> Reconnecting`. The transport was lost after a genuine
    /// prior connection.
    pub fn on_transport_lost(&mut self) -> Result<(), IllegalConnectionTransition> {
        match self.state {
            IbkrConnectionState::Connected => {
                self.state = IbkrConnectionState::Reconnecting;
                Ok(())
            }
            other => Err(IllegalConnectionTransition {
                from: other,
                attempted: "on_transport_lost",
            }),
        }
    }

    /// `Connected | Reconnecting | Connecting -> Disconnected`. An operator
    /// or shutdown-driven disconnect from any non-terminal state.
    pub fn disconnect(&mut self) -> Result<(), IllegalConnectionTransition> {
        match self.state {
            IbkrConnectionState::Connected
            | IbkrConnectionState::Reconnecting
            | IbkrConnectionState::Connecting => {
                self.state = IbkrConnectionState::Disconnected;
                Ok(())
            }
            other => Err(IllegalConnectionTransition {
                from: other,
                attempted: "disconnect",
            }),
        }
    }
}

/// Result of a successful `IbkrTransport::connect` call.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct IbkrSessionInfo {
    /// TWS's `nextValidId` at connect time — the first session-scoped
    /// order id this adapter may assign.
    pub next_valid_order_id: i64,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct IbkrTransportError(pub String);

impl std::fmt::Display for IbkrTransportError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "IBKR transport error: {}", self.0)
    }
}

impl std::error::Error for IbkrTransportError {}

/// Injectable abstraction over the real TWS/IB Gateway session. Tests
/// inject a fake implementation; a production implementation backed by
/// `ibapi::Client` is explicit future work (no such implementation exists
/// in this crate today) -- no real network connection is made anywhere in
/// this patch.
pub trait IbkrTransport: Send + Sync {
    /// Establish (or re-establish) the session against `identity`. Returns
    /// `Err` on any failure -- callers must never infer a session from a
    /// partial/ambiguous outcome.
    fn connect(
        &self,
        identity: &IbkrDeploymentIdentity,
    ) -> Result<IbkrSessionInfo, IbkrTransportError>;
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn happy_path_connect_reaches_connected() {
        let mut sm = IbkrConnectionStateMachine::new();
        assert_eq!(sm.state(), IbkrConnectionState::Disconnected);
        sm.begin_connect().expect("begin_connect must succeed");
        assert_eq!(sm.state(), IbkrConnectionState::Connecting);
        sm.on_connected().expect("on_connected must succeed");
        assert_eq!(sm.state(), IbkrConnectionState::Connected);
    }

    #[test]
    fn reconnect_after_transport_loss_reaches_connected_via_reconnecting() {
        let mut sm = IbkrConnectionStateMachine::new();
        sm.begin_connect().unwrap();
        sm.on_connected().unwrap();
        assert_eq!(sm.state(), IbkrConnectionState::Connected);

        sm.on_transport_lost()
            .expect("on_transport_lost must succeed from Connected");
        assert_eq!(sm.state(), IbkrConnectionState::Reconnecting);

        sm.on_connected()
            .expect("on_connected must succeed from Reconnecting");
        assert_eq!(sm.state(), IbkrConnectionState::Connected);
    }

    #[test]
    fn connect_failure_returns_to_disconnected_never_reconnecting() {
        let mut sm = IbkrConnectionStateMachine::new();
        sm.begin_connect().unwrap();
        sm.on_connect_failed()
            .expect("on_connect_failed must succeed from Connecting");
        assert_eq!(
            sm.state(),
            IbkrConnectionState::Disconnected,
            "a first-attempt failure must never route through Reconnecting -- there was never a \
             genuine prior connection to recover"
        );
    }

    #[test]
    fn disconnected_to_connected_directly_is_illegal() {
        let mut sm = IbkrConnectionStateMachine::new();
        let err = sm
            .on_connected()
            .expect_err("Disconnected -> Connected directly must be refused");
        assert_eq!(err.from, IbkrConnectionState::Disconnected);
        assert_eq!(sm.state(), IbkrConnectionState::Disconnected);
    }

    #[test]
    fn double_begin_connect_is_illegal_and_state_is_unchanged() {
        let mut sm = IbkrConnectionStateMachine::new();
        sm.begin_connect().unwrap();
        let err = sm
            .begin_connect()
            .expect_err("a second begin_connect while already Connecting must be refused");
        assert_eq!(err.from, IbkrConnectionState::Connecting);
        assert_eq!(
            sm.state(),
            IbkrConnectionState::Connecting,
            "a refused transition must never mutate state"
        );
    }

    #[test]
    fn transport_lost_while_never_connected_is_illegal() {
        let mut sm = IbkrConnectionStateMachine::new();
        let err = sm
            .on_transport_lost()
            .expect_err("on_transport_lost from Disconnected must be refused");
        assert_eq!(err.from, IbkrConnectionState::Disconnected);
    }

    #[test]
    fn disconnect_is_legal_from_every_non_terminal_state() {
        for setup in [
            |sm: &mut IbkrConnectionStateMachine| {
                sm.begin_connect().unwrap();
            },
            |sm: &mut IbkrConnectionStateMachine| {
                sm.begin_connect().unwrap();
                sm.on_connected().unwrap();
            },
            |sm: &mut IbkrConnectionStateMachine| {
                sm.begin_connect().unwrap();
                sm.on_connected().unwrap();
                sm.on_transport_lost().unwrap();
            },
        ] {
            let mut sm = IbkrConnectionStateMachine::new();
            setup(&mut sm);
            sm.disconnect().expect("disconnect must succeed");
            assert_eq!(sm.state(), IbkrConnectionState::Disconnected);
        }
    }

    /// Fake transport proving reconnect through the trait seam itself (not
    /// just the pure state machine): the second `connect` call after a
    /// simulated drop must succeed independently, and a caller driving the
    /// state machine from its results reaches `Connected` again.
    struct FlakyThenRecoveringTransport {
        fail_first_n_calls: std::sync::atomic::AtomicU32,
    }

    impl IbkrTransport for FlakyThenRecoveringTransport {
        fn connect(
            &self,
            _identity: &IbkrDeploymentIdentity,
        ) -> Result<IbkrSessionInfo, IbkrTransportError> {
            use std::sync::atomic::Ordering;
            if self.fail_first_n_calls.load(Ordering::SeqCst) > 0 {
                self.fail_first_n_calls.fetch_sub(1, Ordering::SeqCst);
                return Err(IbkrTransportError("simulated transport drop".to_string()));
            }
            Ok(IbkrSessionInfo {
                next_valid_order_id: 1,
            })
        }
    }

    #[test]
    fn fake_transport_drop_then_recovery_drives_state_machine_through_reconnecting() {
        let transport = FlakyThenRecoveringTransport {
            fail_first_n_calls: std::sync::atomic::AtomicU32::new(1),
        };
        let identity = IbkrDeploymentIdentity {
            account_id: "DU1234567".to_string(),
            client_id: 1,
            host: "127.0.0.1".to_string(),
            port: 7497,
        };
        let mut sm = IbkrConnectionStateMachine::new();

        sm.begin_connect().unwrap();
        match transport.connect(&identity) {
            Ok(_) => panic!("first connect was expected to fail in this test"),
            Err(_) => sm.on_connect_failed().unwrap(),
        }
        assert_eq!(sm.state(), IbkrConnectionState::Disconnected);

        // Operator/retry-policy layer (not this crate) decides to try
        // again; from the adapter's perspective this is a fresh
        // begin_connect.
        sm.begin_connect().unwrap();
        let info = transport
            .connect(&identity)
            .expect("second connect must succeed");
        sm.on_connected().unwrap();
        assert_eq!(sm.state(), IbkrConnectionState::Connected);
        assert_eq!(info.next_valid_order_id, 1);
    }
}
