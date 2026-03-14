use anyhow::{Context, Result};
use chrono::{DateTime, Utc};
use mqk_schemas::{Bar, BrokerSnapshot};
use serde_json::json;
use std::fs;

pub fn load_bars_csv(path: &str) -> Result<Vec<Bar>> {
    let mut rdr = csv::Reader::from_path(path).with_context(|| format!("open bars csv: {path}"))?;
    let mut out = Vec::new();

    for rec in rdr.records() {
        let rec = rec?;
        let ts: DateTime<Utc> = rec[0].parse().context("parse ts_close_utc")?;
        let bar = Bar {
            ts_close_utc: ts,
            open: rec[1].to_string(),
            high: rec[2].to_string(),
            low: rec[3].to_string(),
            close: rec[4].to_string(),
            volume: rec[5].to_string(),
        };
        out.push(bar);
    }

    // Minimal structural checks (expand later)
    for w in out.windows(2) {
        if w[0].ts_close_utc >= w[1].ts_close_utc {
            anyhow::bail!("bars not strictly increasing");
        }
    }

    Ok(out)
}

pub fn load_broker_snapshot_json(path: &str) -> Result<BrokerSnapshot> {
    let s = fs::read_to_string(path).with_context(|| format!("read snapshot: {path}"))?;
    let snap: BrokerSnapshot = serde_json::from_str(&s).context("parse snapshot json")?;
    Ok(snap)
}

/// Deterministic parity scenario runner.
///
/// This intentionally stays narrow: a single-share buy-and-hold replay over
/// the provided bar stream, producing stable artifacts that can be compared
/// across runs/restarts.
pub struct ScenarioRunResult {
    pub orders_csv: String,
    pub fills_csv: String,
    pub equity_curve_csv: String,
    pub metrics_json: String,
    pub audit_jsonl: String,
}

pub fn run_parity_scenario_stub(bars: &[Bar]) -> Result<ScenarioRunResult> {
    if bars.is_empty() {
        anyhow::bail!("parity runner requires at least one bar");
    }

    // Structural parity check: input must be strictly increasing in time.
    for w in bars.windows(2) {
        if w[0].ts_close_utc >= w[1].ts_close_utc {
            anyhow::bail!("bars not strictly increasing");
        }
    }

    let first = &bars[0];
    let first_close_micros = decimal_str_to_micros(&first.close)
        .with_context(|| format!("parse first close as decimal: {}", first.close))?;

    let mut equity_rows = Vec::with_capacity(bars.len());
    let mut max_equity_micros = first_close_micros;
    let mut max_drawdown_micros = 0_i64;
    let mut audit_lines = Vec::with_capacity(bars.len());

    for b in bars {
        let close_micros = decimal_str_to_micros(&b.close)
            .with_context(|| format!("parse close as decimal: {}", b.close))?;
        let equity_micros = close_micros;
        max_equity_micros = max_equity_micros.max(equity_micros);
        max_drawdown_micros = max_drawdown_micros.max(max_equity_micros - equity_micros);

        equity_rows.push(format!("{},{equity_micros}", b.ts_close_utc.to_rfc3339()));
        audit_lines.push(
            json!({
                "event": "bar_replay",
                "ts_close_utc": b.ts_close_utc,
                "close_micros": close_micros,
                "equity_micros": equity_micros,
            })
            .to_string(),
        );
    }

    let last_close_micros =
        decimal_str_to_micros(&bars[bars.len() - 1].close).with_context(|| {
            format!(
                "parse last close as decimal: {}",
                bars[bars.len() - 1].close
            )
        })?;
    let gross_return_bps = if first_close_micros == 0 {
        0_i64
    } else {
        ((last_close_micros - first_close_micros) * 10_000) / first_close_micros
    };

    let orders_csv = format!(
        "ts_close_utc,order_id,symbol,side,qty,limit_price_micros\n{},parity-entry,SYNTH,LONG,1,{first_close_micros}\n",
        first.ts_close_utc.to_rfc3339()
    );
    let fills_csv = format!(
        "ts_close_utc,order_id,filled_qty,fill_price_micros\n{},parity-entry,1,{first_close_micros}\n",
        first.ts_close_utc.to_rfc3339()
    );
    let equity_curve_csv = format!("ts_close_utc,equity_micros\n{}\n", equity_rows.join("\n"));
    let metrics_json = json!({
        "bars_seen": bars.len(),
        "entry_price_micros": first_close_micros,
        "final_equity_micros": last_close_micros,
        "gross_return_bps": gross_return_bps,
        "max_drawdown_micros": max_drawdown_micros,
    })
    .to_string();
    let audit_jsonl = format!("{}\n", audit_lines.join("\n"));

    Ok(ScenarioRunResult {
        orders_csv,
        fills_csv,
        equity_curve_csv,
        metrics_json,
        audit_jsonl,
    })
}

fn decimal_str_to_micros(s: &str) -> Result<i64> {
    // Input bars use decimal strings at this boundary.
    // Keep conversion deterministic for test/proof artifacts.
    let parsed: f64 = s.parse().with_context(|| format!("invalid decimal: {s}"))?;
    Ok((parsed * 1_000_000.0).round() as i64)
}

mod recovery;

pub use recovery::{recover_outbox_against_broker, FakeBroker, RecoveryReport};

// I9-1: capital conservation invariant helper.
pub mod conservation;
pub use conservation::assert_capital_conservation;

pub mod orchestrator;

pub use orchestrator::{
    Orchestrator, OrchestratorBar, OrchestratorConfig, OrchestratorReport, OrchestratorRunMeta,
};
