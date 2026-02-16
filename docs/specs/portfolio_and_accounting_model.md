# Portfolio and Accounting Model (V4)

Fill-driven ledger is the source of truth.
Broker positions are for reconciliation only.

v0:
- FIFO lots
- realized/unrealized PnL
- cash ledger for fees/dividends
- equity = cash + unrealized
- drawdown tracked off peak equity

Exposure metrics:
- gross/net exposure, leverage

Determinism: ledger recompute must match incremental updates.
