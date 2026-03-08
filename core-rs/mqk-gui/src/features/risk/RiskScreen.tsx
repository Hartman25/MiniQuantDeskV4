import { DataTable } from "../../components/common/DataTable";
import { Panel } from "../../components/common/Panel";
import { StatCard } from "../../components/common/StatCard";
import { formatDateTime, formatMoney, formatPercent } from "../../lib/format";
import type { SystemModel } from "../system/types";

export function RiskScreen({ model }: { model: SystemModel }) {
  const r = model.riskSummary;
  return (
    <div className="screen-grid">
      <div className="summary-grid summary-grid-four">
        <StatCard title="Gross Exposure" value={formatMoney(r.gross_exposure)} detail="Current deployed gross capital" tone="neutral" />
        <StatCard title="Net Exposure" value={formatMoney(r.net_exposure)} detail="Directional net capital" tone="neutral" />
        <StatCard title="Concentration" value={formatPercent(r.concentration_pct)} detail="Largest symbol concentration" tone={r.concentration_pct > 50 ? "bad" : r.concentration_pct > 35 ? "warn" : "good"} />
        <StatCard title="Loss Limit Utilization" value={formatPercent(r.loss_limit_utilization_pct)} detail="Daily loss budget used" tone={r.loss_limit_utilization_pct > 80 ? "bad" : r.loss_limit_utilization_pct > 60 ? "warn" : "good"} />
      </div>
      <Panel title="Risk posture">
        <div className="metric-list two-up">
          <div><span>Daily PnL</span><strong>{formatMoney(r.daily_pnl)}</strong></div>
          <div><span>Drawdown</span><strong>{formatPercent(r.drawdown_pct)}</strong></div>
          <div><span>Kill switch</span><strong>{r.kill_switch_active ? "Active" : "Inactive"}</strong></div>
          <div><span>Active breaches</span><strong>{r.active_breaches}</strong></div>
        </div>
      </Panel>
      <Panel title="Risk denials">
        <DataTable
          rows={model.riskDenials}
          rowKey={(row) => row.id}
          columns={[
            { key: "at", title: "At", render: (row) => formatDateTime(row.at) },
            { key: "strategy", title: "Strategy", render: (row) => row.strategy_id },
            { key: "symbol", title: "Symbol", render: (row) => row.symbol },
            { key: "rule", title: "Rule", render: (row) => row.rule },
            { key: "message", title: "Message", render: (row) => row.message },
          ]}
        />
      </Panel>
    </div>
  );
}
