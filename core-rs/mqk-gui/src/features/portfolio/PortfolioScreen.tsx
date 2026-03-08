import { DataTable } from "../../components/common/DataTable";
import { Panel } from "../../components/common/Panel";
import { StatCard } from "../../components/common/StatCard";
import { formatDateTime, formatMoney } from "../../lib/format";
import type { SystemModel } from "../system/types";

export function PortfolioScreen({ model }: { model: SystemModel }) {
  const p = model.portfolioSummary;
  return (
    <div className="screen-grid">
      <div className="summary-grid summary-grid-four">
        <StatCard title="Equity" value={formatMoney(p.account_equity)} detail="Account equity" tone="good" />
        <StatCard title="Cash" value={formatMoney(p.cash)} detail="Available cash" tone="neutral" />
        <StatCard title="Long Market Value" value={formatMoney(p.long_market_value)} detail="Long exposure" tone="neutral" />
        <StatCard title="Daily PnL" value={formatMoney(p.daily_pnl)} detail="Realized + unrealized" tone={p.daily_pnl < 0 ? "bad" : "good"} />
      </div>
      <Panel title="Positions">
        <DataTable
          rows={model.positions}
          rowKey={(row) => `${row.strategy_id}-${row.symbol}`}
          columns={[
            { key: "symbol", title: "Symbol", render: (row) => row.symbol },
            { key: "strategy", title: "Strategy", render: (row) => row.strategy_id },
            { key: "qty", title: "Qty", render: (row) => row.qty },
            { key: "avg", title: "Avg", render: (row) => formatMoney(row.avg_price) },
            { key: "mark", title: "Mark", render: (row) => formatMoney(row.mark_price) },
            { key: "u", title: "Unrealized", render: (row) => formatMoney(row.unrealized_pnl) },
            { key: "drift", title: "Drift", render: (row) => (row.drift ? "Yes" : "No") },
          ]}
        />
      </Panel>
      <div className="two-column-grid">
        <Panel title="Open Orders">
          <DataTable
            rows={model.openOrders}
            rowKey={(row) => row.internal_order_id}
            columns={[
              { key: "id", title: "Order", render: (row) => row.internal_order_id },
              { key: "symbol", title: "Symbol", render: (row) => row.symbol },
              { key: "status", title: "Status", render: (row) => row.status },
              { key: "qty", title: "Qty", render: (row) => `${row.filled_qty}/${row.requested_qty}` },
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
      </div>
    </div>
  );
}
