"""Strategy Factory: a thin, deterministic coordinator over the existing Research/Backtest owners.

It owns no economics, no trial identity and no Promotion authority. Ideas enter as untrusted data, are typed and
deduplicated without any result, are admitted to an executable path only when one verifiably exists, and are then
handed to the accepted stage-authorized batch pipeline (`ResearchResultStore`, native Rust engine, judge, scanner/review).
"""

SCHEMA_VERSION = "strategy_factory_v1"
