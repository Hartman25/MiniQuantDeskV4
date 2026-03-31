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

export function PortfolioScreen({ model }: { model: SystemModel }) {
  const p = model.portfolioSummary;
  const truthState = panelTruthRenderState(model, "portfolio");

  // Hard-close on any compromised truth state: stale equity and positions are directly
  // dangerous under live conditions. Silent pass-through of stale/degraded is not acceptable.
  if (truthState !== null) {
    return <TruthStateNotice state={truthState} />;
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
              { key: "u", title: "Unrealized", render: (row) => formatMoney(row.unrealized_pnl ?? null) },
              { key: "drift", title: "Drift", render: (row) => (row.drift == null ? "—" : row.drift ? "Yes" : "No") },
            ]}
          />
        </Panel>
      </div>

      <div className="desk-panel-grid desk-panel-grid-secondary">
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
              { key: "side", title: "Side", render: (row) => row.side },
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
                { key: "side", title: "Side", render: (row) => row.side },
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
                { key: "side", title: "Side", render: (row) => row.side },
                { key: "qty", title: "Qty", render: (row) => String(row.qty) },
                { key: "signal", title: "Signal ID", render: (row) => row.signal_id },
              ]}
            />
          )}
        </Panel>
      </div>
    </div>
  );
}
