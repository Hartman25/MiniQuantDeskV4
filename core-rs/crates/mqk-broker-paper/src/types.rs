use mqk_reconcile::{OrderSnapshot, Side};

/// Deterministic broker message ID suitable for inbox de-dupe.
/// Kept as an opaque string newtype.
#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct BrokerMessageId(pub String);

impl BrokerMessageId {
    pub fn new(id: String) -> Self {
        Self(id)
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SubmitOrder {
    pub client_order_id: String,
    pub symbol: String,
    pub side: Side,
    pub qty: i64,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SubmitResponse {
    pub broker_message_id: BrokerMessageId,
    pub broker_order_id: String,
    pub snapshot: OrderSnapshot,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct CancelRequest {
    pub client_order_id: String,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ReplaceRequest {
    pub client_order_id: String,
    pub new_qty: i64,
}
