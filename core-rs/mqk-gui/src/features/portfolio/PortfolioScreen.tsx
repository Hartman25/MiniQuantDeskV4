import { DataTable } from "../../components/common/DataTable";
import { Panel } from "../../components/common/Panel";
import { StatCard } from "../../components/common/StatCard";
import { TruthStateNotice } from "../../components/common/TruthStateNotice";
import { formatDateTime, formatMoney } from "../../lib/format";
import { paperJournalLaneNotice } from "../system/legacy";
import { panelTruthRenderState } from "../system/truthRendering";
import type { SystemModel } from "../system/types";

function formatMicros(micros: number): string {
  return (micros / 1_000_000).toLocaleString(undefined, { style: "currency", currency: "USD", minimumFractionDigits: 2, maximumFractionDigits: 4 });
}

// DURABLE-PAPER-PORTFOLIO-AND-PNL-01F: `null` always means unavailable/
// unproven and must render as the word "Unavailable" — never silently
// collapsed into the same "—" glyph the broker-snapshot panel above uses
// for its own, pre-existing null convention. A true numeric zero (e.g. a
// flat position, or exactly-zero P&L) always renders as the literal `0`.
function formatDurableMoney(value: number | null) {
  if (value == null) return <span className="val-unavailable">Unavailable</span>;
  return formatMoney(value);
}

function formatDurableCount(value: number | null) {
  if (value == null) return <span className="val-unavailable">Unavailable</span>;
  return String(value);
}

export function PortfolioScreen({ model }: { model: SystemModel }) {
  const p = model.portfolioSummary;
  const durable = model.durablePortfolioSummary;
  const truthState = panelTruthRenderState(model, "portfolio");

  // DURABLE-PAPER-PORTFOLIO-AND-PNL-01F: the durable, restart-surviving
  // section renders unconditionally, below the in-memory hard-close notice
  // when one fires. This is deliberate — durable truth is exactly what
  // remains visible when in-memory broker-snapshot truth is degraded or
  // absent (e.g. immediately after a daemon restart), so gating it behind
  // the same check would hide it precisely when it is most useful. The two
  // truths are never collapsed into one: this section only ever reads
  // model.durablePortfolioSummary/Positions/Snapshots, never
  // model.portfolioSummary/positions.
  const durableSection = <DurablePortfolioSection model={model} durable={durable} />;

  // Hard-close on any compromised truth state: stale equity and positions are directly
  // dangerous under live conditions. Silent pass-through of stale/degraded is not acceptable.
  if (truthState !== null) {
    return (
      <div className="screen-grid desk-screen-grid">
        <TruthStateNotice state={truthState} />
        {durableSection}
      </div>
    );
  }

  return (
    <div className="screen-grid desk-screen-grid">
      <div className="summary-grid summary-grid-four">
        <StatCard title="Equity" value={formatMoney(p.account_equity)} detail="Account equity" tone="good" />
        <StatCard title="Cash" value={formatMoney(p.cash)} detail="Available cash" tone="neutral" />
        <StatCard title="Long Market Value" value={formatMoney(p.long_market_value)} detail="Long exposure" tone="neutral" />
        <StatCard title="Daily PnL" value={formatMoney(p.daily_pnl)} detail="Realized + unrealized" tone={p.daily_pnl < 0 ? "bad" : "good"} />
      </div>

      <div className="desk-panel-grid desk-panel-grid-primary">
        <Panel title="Portfolio summary">
          <div className="metric-list">
            <div><span>Buying power</span><strong>{formatMoney(p.buying_power)}</strong></div>
            <div><span>Short market value</span><strong>{formatMoney(p.short_market_value)}</strong></div>
            <div><span>Source state</span><strong>{model.dataSource.state}</strong></div>
            <div><span>Connected</span><strong>{model.connected ? "Yes" : "No"}</strong></div>
          </div>
        </Panel>

        <Panel title="Positions">
          <DataTable
            rows={model.positions}
            rowKey={(row) => row.symbol}
            columns={[
              { key: "symbol", title: "Symbol", render: (row) => row.symbol },
              { key: "strategy", title: "Strategy", render: (row) => row.strategy_id },
              { key: "qty", title: "Qty", render: (row) => row.qty },
              { key: "avg", title: "Avg", render: (row) => formatMoney(row.avg_price) },
              { key: "mark", title: "Mark", render: (row) => formatMoney(row.mark_price ?? null) },
              { key: "u", title: "Unrealized", render: (row) => {
                const v = row.unrealized_pnl ?? null;
                const s = formatMoney(v);
                if (v == null) return s;
                if (v < 0) return <span className="val-negative">{s}</span>;
                if (v > 0) return <span className="val-positive">{s}</span>;
                return s;
              }},
              { key: "drift", title: "Drift", render: (row) => {
                if (row.drift == null) return "—";
                return row.drift ? <span className="val-critical">Yes</span> : <span className="val-ok">No</span>;
              }},
            ]}
          />
        </Panel>
      </div>

      <div className="desk-panel-grid desk-panel-grid-thirds">
        <Panel title="Open Orders">
          <DataTable
            rows={model.openOrders}
            rowKey={(row) => row.internal_order_id}
            columns={[
              { key: "id", title: "Order", render: (row) => row.internal_order_id },
              { key: "symbol", title: "Symbol", render: (row) => row.symbol },
              { key: "status", title: "Status", render: (row) => row.status },
              { key: "qty", title: "Qty", render: (row) => row.filled_qty != null ? `${row.filled_qty}/${row.requested_qty}` : `—/${row.requested_qty}` },
            ]}
          />
        </Panel>

        <Panel title="Recent Fills">
          <DataTable
            rows={model.fills}
            rowKey={(row) => row.fill_id}
            columns={[
              { key: "at", title: "At", render: (row) => formatDateTime(row.at) },
              { key: "symbol", title: "Symbol", render: (row) => row.symbol },
              { key: "side", title: "Side", render: (row) => <span className={row.side === "buy" ? "side-buy" : "side-sell"}>{row.side}</span> },
              { key: "qty", title: "Qty", render: (row) => row.qty },
              { key: "price", title: "Price", render: (row) => formatMoney(row.price) },
            ]}
          />
        </Panel>

        <Panel title="Portfolio notes" compact>
          <div className="metric-list compact-list">
            <div><span>Positions</span><strong>{model.positions.length}</strong></div>
            <div><span>Open orders</span><strong>{model.openOrders.length}</strong></div>
            <div><span>Recent fills</span><strong>{model.fills.length}</strong></div>
            <div><span>Mock sections</span><strong>{model.dataSource.mockSections.length}</strong></div>
          </div>
        </Panel>
      </div>

      {/* GUI-OPS-01: Paper journal — durable fills + signal admissions evidence surface. */}
      <div className="desk-panel-grid desk-panel-grid-secondary">
        <Panel
          title="Paper journal — fills"
          subtitle={
            model.paperJournal.fills_truth_state === "active"
              ? `Run ${model.paperJournal.run_id ?? "unknown"} — postgres.fill_quality_telemetry`
              : "Fill evidence (paper journal)"
          }
        >
          {paperJournalLaneNotice(model.paperJournal.fills_truth_state) ? (
            <div className="unavailable-notice">{paperJournalLaneNotice(model.paperJournal.fills_truth_state)}</div>
          ) : model.paperJournal.fills.length === 0 ? (
            <div className="empty-state">No fills recorded yet this run.</div>
          ) : (
            <DataTable
              rows={model.paperJournal.fills}
              rowKey={(row) => row.telemetry_id}
              columns={[
                { key: "symbol", title: "Symbol", render: (row) => row.symbol },
                { key: "side", title: "Side", render: (row) => <span className={row.side === "buy" ? "side-buy" : "side-sell"}>{row.side}</span> },
                { key: "kind", title: "Kind", render: (row) => row.fill_kind },
                { key: "qty", title: "Fill/Ordered", render: (row) => `${row.fill_qty}/${row.ordered_qty}` },
                { key: "price", title: "Fill Price", render: (row) => formatMicros(row.fill_price_micros) },
                { key: "slippage", title: "Slippage", render: (row) => row.slippage_bps != null ? `${row.slippage_bps} bps` : "—" },
                { key: "at", title: "Fill Received", render: (row) => formatDateTime(row.fill_received_at_utc) },
              ]}
            />
          )}
        </Panel>

        <Panel
          title="Paper journal — signal admissions"
          subtitle={
            model.paperJournal.admissions_truth_state === "active"
              ? "postgres.audit_events[topic=signal_ingestion]"
              : "Signal admission history (paper journal)"
          }
        >
          {paperJournalLaneNotice(model.paperJournal.admissions_truth_state) ? (
            <div className="unavailable-notice">{paperJournalLaneNotice(model.paperJournal.admissions_truth_state)}</div>
          ) : model.paperJournal.admissions.length === 0 ? (
            <div className="empty-state">No signals admitted yet this run.</div>
          ) : (
            <DataTable
              rows={model.paperJournal.admissions}
              rowKey={(row) => row.event_id}
              columns={[
                { key: "at", title: "At", render: (row) => formatDateTime(row.ts_utc) },
                { key: "strategy", title: "Strategy", render: (row) => row.strategy_id },
                { key: "symbol", title: "Symbol", render: (row) => row.symbol },
                { key: "side", title: "Side", render: (row) => <span className={row.side === "buy" ? "side-buy" : "side-sell"}>{row.side}</span> },
                { key: "qty", title: "Qty", render: (row) => String(row.qty) },
                { key: "signal", title: "Signal ID", render: (row) => row.signal_id },
              ]}
            />
          )}
        </Panel>
      </div>

      {durableSection}
    </div>
  );
}

// ---------------------------------------------------------------------------
// DURABLE-PAPER-PORTFOLIO-AND-PNL-01F
// ---------------------------------------------------------------------------

/**
 * Restart-surviving durable portfolio/P&L truth, rendered as its own
 * section rather than merged into the broker-snapshot panels above — the
 * two must never be collapsed into one truth. Renders its own truth-state
 * notices inline (never hard-blocks the whole screen) and contains no
 * capture/retry/order/arm/flatten control of any kind — read-only display
 * only, sourced from model.durablePortfolioSummary/Positions/Snapshots.
 */
function DurablePortfolioSection({
  model,
  durable,
}: {
  model: SystemModel;
  durable: SystemModel["durablePortfolioSummary"];
}) {
  return (
    <>
      <div className="desk-panel-grid desk-panel-grid-primary">
        <Panel
          title="Durable Portfolio (restart-surviving)"
          subtitle={
            durable.snapshot_truth_state === "active"
              ? `Snapshot ${durable.snapshot_id ?? "unknown"} — ${durable.source ?? "unknown source"} @ ${formatDateTime(durable.captured_at_utc)}`
              : `Durable snapshot: ${durable.snapshot_truth_state}`
          }
        >
          {durable.snapshot_truth_state === "snapshot_stale" && (
            <div className="unavailable-notice">
              Durable snapshot is stale — the last known authoritative capture is older than the
              refresh interval. Do not treat these values as current.
            </div>
          )}
          {durable.snapshot_truth_state !== "active" && durable.snapshot_truth_state !== "snapshot_stale" && (
            <div className="unavailable-notice">
              Durable portfolio truth unavailable ({durable.snapshot_truth_state}). This is
              independent of the in-memory broker-snapshot panels above.
            </div>
          )}
          <div className="metric-list">
            <div><span>Equity</span><strong>{formatDurableMoney(durable.account_equity)}</strong></div>
            <div><span>Cash</span><strong>{formatDurableMoney(durable.cash)}</strong></div>
            <div><span>Currency</span><strong>{durable.currency ?? <span className="val-unavailable">Unavailable</span>}</strong></div>
            <div><span>Run</span><strong>{durable.run_id ?? <span className="val-unavailable">Unavailable</span>}</strong></div>
            <div><span>Accounting completeness</span><strong>
              {durable.accounting_epoch == null
                ? <span className="val-unavailable">Unavailable</span>
                : durable.accounting_epoch === "complete"
                  ? <span className="val-ok">Complete</span>
                  : <span className="val-critical">Incomplete</span>}
            </strong></div>
            <div><span>Realized PnL</span><strong>{formatDurableMoney(durable.realized_pnl)}</strong></div>
            <div><span>Unrealized PnL</span><strong>{formatDurableMoney(durable.unrealized_pnl)}</strong></div>
            <div><span>Daily PnL</span><strong>{formatDurableMoney(durable.daily_pnl)}</strong></div>
            <div><span>Fees</span><strong>{formatDurableMoney(durable.fees)}</strong></div>
            <div><span>Cumulative cash movement</span><strong>{formatDurableMoney(durable.cumulative_cash_movement)}</strong></div>
          </div>
          {durable.accounting_epoch_reason && (
            <div className="unavailable-notice">Accounting incomplete: {durable.accounting_epoch_reason}</div>
          )}
          {durable.blockers.length > 0 && (
            <div className="unavailable-notice">
              {durable.blockers.map((b) => <div key={b}>{b}</div>)}
            </div>
          )}
        </Panel>

        <Panel title="Durable positions" subtitle="Restart-surviving — from the durable snapshot, not the in-memory broker snapshot">
          {model.durablePortfolioPositions.truth_state !== "active" ? (
            <div className="unavailable-notice">
              Durable positions unavailable ({model.durablePortfolioPositions.truth_state}).
            </div>
          ) : model.durablePortfolioPositions.positions.length === 0 ? (
            <div className="empty-state">Flat — no durable positions.</div>
          ) : (
            <DataTable
              rows={model.durablePortfolioPositions.positions}
              rowKey={(row) => row.symbol}
              columns={[
                { key: "symbol", title: "Symbol", render: (row) => row.symbol },
                { key: "qty", title: "Qty", render: (row) => formatDurableCount(row.qty_signed) },
                { key: "avg", title: "Avg entry", render: (row) => formatDurableMoney(row.avg_entry_price) },
                { key: "provenance", title: "Provenance", render: (row) => row.provenance },
              ]}
            />
          )}
        </Panel>
      </div>

      <div className="desk-panel-grid desk-panel-grid-primary">
        <Panel title="Recent durable snapshots" subtitle="Durable snapshot history — restart-surviving">
          {model.durablePortfolioSnapshots.truth_state !== "active" ? (
            <div className="unavailable-notice">
              Durable snapshot history unavailable ({model.durablePortfolioSnapshots.truth_state}).
            </div>
          ) : model.durablePortfolioSnapshots.snapshots.length === 0 ? (
            <div className="empty-state">No durable snapshots captured yet.</div>
          ) : (
            <DataTable
              rows={model.durablePortfolioSnapshots.snapshots}
              rowKey={(row) => row.snapshot_id}
              columns={[
                { key: "at", title: "Captured", render: (row) => formatDateTime(row.captured_at_utc) },
                { key: "source", title: "Source", render: (row) => row.source },
                { key: "equity", title: "Equity", render: (row) => formatMoney(row.equity) },
                { key: "cash", title: "Cash", render: (row) => formatMoney(row.cash) },
                { key: "run", title: "Run", render: (row) => row.run_id ?? "—" },
              ]}
            />
          )}
        </Panel>
      </div>
    </>
  );
}
