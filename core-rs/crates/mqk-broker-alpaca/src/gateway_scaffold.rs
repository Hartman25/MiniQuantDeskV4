use anyhow::Result;

use crate::{types::SubmitOrderReq, AlpacaBroker};

/// Scaffold adapter to integrate with mqk-execution gateway trait.
/// You will later implement the same trait used by mqk-broker-paper.
pub struct AlpacaGateway {
    broker: AlpacaBroker,
}

impl AlpacaGateway {
    pub fn new(broker: AlpacaBroker) -> Self {
        Self { broker }
    }

    pub async fn submit_order(&self, _req: SubmitOrderReq) -> Result<()> {
        anyhow::bail!("AlpacaGateway::submit_order not implemented (scaffold)")
    }
}
