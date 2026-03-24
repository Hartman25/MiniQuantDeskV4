//! PT-TOOL-01: Testkit-only deterministic paper validation harness.
//!
//! **THIS BINARY IS NOT THE REAL DAEMON PAPER PATH.**
//!
//! This binary exists to run deterministic paper loops against a pre-seeded
//! database for scenario validation and research harness use.  It uses the
//! in-process `LockedPaperBroker` (synthetic fills, no Alpaca connectivity)
//! and is gated behind `--features testkit`.
//!
//! The real broker-backed daemon paper path is `paper+alpaca`:
//!   `MQK_DAEMON_DEPLOYMENT_MODE=paper MQK_DAEMON_ADAPTER_ID=alpaca`
//!
//! This binary uses synthetic fills from `LockedPaperBroker` and requires
//! an external `on_bar()` caller to produce fills; without it, orders are
//! accepted but never filled.  This is NOT an honest paper trading route.
//!
//! Usage (testkit/validation only):
//! ```
//! cargo run -p mqk-cli --features testkit --bin mqk_paper_loop -- \
//!   --run-id <uuid> --ticks 10 --sleep-ms 1000 --initial-cash-usd 100000
//! ```
#![forbid(unsafe_code)]

use std::time::Duration;

use anyhow::Context;
use uuid::Uuid;

use mqk_execution::BrokerOrderMap;
use mqk_portfolio::PortfolioState;

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    let args = Args::parse()?;

    let pool = mqk_db::connect_from_env().await?;
    let run = mqk_db::fetch_run(&pool, args.run_id).await?;
    let initial_equity_micros = args.initial_cash_usd * 1_000_000;

    // PT-TOOL-01: testkit gateway — LockedPaperBroker with real risk evaluation.
    // PassGate bypasses integrity and reconcile gates (testkit harness, not daemon).
    // Do NOT use this wiring in production; see file-level doc comment above.
    let gateway = mqk_runtime::wiring_paper::paper_gateway_for_testkit_validation(
        &run.config_json,
        initial_equity_micros,
    );

    let mut order_map = BrokerOrderMap::new();
    let existing = mqk_db::broker_map_load(&pool).await?;
    for (internal_id, broker_id) in existing {
        order_map.register(&internal_id, &broker_id);
    }

    // A2: resume from last-persisted cursor so events are not re-fetched from
    // the beginning after a restart.  Fails closed: None = start from beginning.
    let broker_cursor = mqk_db::load_broker_cursor(&pool, "paper").await?;

    let mut orchestrator = mqk_runtime::orchestrator::ExecutionOrchestrator::new(
        pool,
        gateway,
        order_map,
        std::collections::BTreeMap::new(),
        PortfolioState::new(initial_equity_micros),
        args.run_id,
        "mqk-paper-loop",
        "paper",
        broker_cursor,
        mqk_runtime::orchestrator::WallClock,
        Box::new(mqk_reconcile::LocalSnapshot::empty),
        Box::new(mqk_reconcile::BrokerSnapshot::empty),
    );

    for _ in 0..args.ticks {
        orchestrator.tick().await.context("tick failed")?;
        tokio::time::sleep(Duration::from_millis(args.sleep_ms)).await;
    }

    orchestrator
        .release_runtime_leadership()
        .await
        .context("release runtime leadership failed")?;

    println!(
        "paper_loop_ok=true run_id={} ticks={}",
        args.run_id, args.ticks
    );
    Ok(())
}

struct Args {
    run_id: Uuid,
    ticks: u64,
    sleep_ms: u64,
    initial_cash_usd: i64,
}

impl Args {
    fn parse() -> anyhow::Result<Self> {
        let mut run_id: Option<Uuid> = None;
        let mut ticks: u64 = 1;
        let mut sleep_ms: u64 = 1000;
        let mut initial_cash_usd: i64 = 100_000;

        let mut it = std::env::args().skip(1);
        while let Some(a) = it.next() {
            match a.as_str() {
                "--run-id" => {
                    let v = it.next().context("missing --run-id value")?;
                    run_id = Some(Uuid::parse_str(&v).context("invalid run_id uuid")?);
                }
                "--ticks" => {
                    let v = it.next().context("missing --ticks value")?;
                    ticks = v.parse::<u64>().context("invalid --ticks")?;
                }
                "--sleep-ms" => {
                    let v = it.next().context("missing --sleep-ms value")?;
                    sleep_ms = v.parse::<u64>().context("invalid --sleep-ms")?;
                }
                "--initial-cash-usd" => {
                    let v = it.next().context("missing --initial-cash-usd value")?;
                    initial_cash_usd = v.parse::<i64>().context("invalid --initial-cash-usd")?;
                }
                _ => anyhow::bail!("unknown arg: {}", a),
            }
        }

        Ok(Self {
            run_id: run_id.context("--run-id is required")?,
            ticks,
            sleep_ms,
            initial_cash_usd,
        })
    }
}
