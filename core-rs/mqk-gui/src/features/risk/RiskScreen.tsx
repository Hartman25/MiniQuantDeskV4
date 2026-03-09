import { DataTable } from "../../components/common/DataTable";
import { Panel } from "../../components/common/Panel";
import { StatCard } from "../../components/common/StatCard";
import { formatDateTime, formatMoney, formatPercent } from "../../lib/format";
import type { SystemModel } from "../system/types";

export function RiskScreen({ model }: { model: SystemModel }) {
  const r = model.riskSummary;
  const activeSuppressions = model.strategySuppressions.filter((row) => row.state === "active");

  return (
    <div className="screen-grid desk-screen-grid">
      <div className="summary-grid summary-grid-four">
        <StatCard title="Gross Exposure" value={formatMoney(r.gross_exposure)} detail="Current deployed gross capital" tone="neutral" />
        <StatCard title="Net Exposure" value={formatMoney(r.net_exposure)} detail="Directional net capital" tone="neutral" />
        <StatCard
          title="Concentration"
          value={formatPercent(r.concentration_pct)}
          detail="Largest symbol concentration"
          tone={r.concentration_pct > 50 ? "bad" : r.concentration_pct > 35 ? "warn" : "good"}
        />
        <StatCard
          title="Loss Limit Utilization"
          value={formatPercent(r.loss_limit_utilization_pct)}
          detail="Daily loss budget used"
          tone={r.loss_limit_utilization_pct > 80 ? "bad" : r.loss_limit_utilization_pct > 60 ? "warn" : "good"}
        />
      </div>

      <div className="desk-panel-grid desk-panel-grid-primary">
        <Panel title="Risk posture">
          <div className="metric-list">
            <div><span>Daily PnL</span><strong>{formatMoney(r.daily_pnl)}</strong></div>
            <div><span>Drawdown</span><strong>{formatPercent(r.drawdown_pct)}</strong></div>
            <div><span>Kill switch</span><strong>{r.kill_switch_active ? "Active" : "Inactive"}</strong></div>
            <div><span>Active breaches</span><strong>{r.active_breaches}</strong></div>
          </div>
        </Panel>

        <Panel title="System safety state">
          <div className="metric-list">
            <div><span>Strategy armed</span><strong>{model.status.strategy_armed ? "Yes" : "No"}</strong></div>
            <div><span>Execution armed</span><strong>{model.status.execution_armed ? "Yes" : "No"}</strong></div>
            <div><span>Risk halt</span><strong>{model.status.risk_halt_active ? "Active" : "Clear"}</strong></div>
            <div><span>Integrity halt</span><strong>{model.status.integrity_halt_active ? "Active" : "Clear"}</strong></div>
            <div><span>Live routing</span><strong>{model.status.live_routing_enabled ? "Enabled" : "Disabled"}</strong></div>
            <div><span>Open alerts</span><strong>{model.alerts.length}</strong></div>
          </div>
        </Panel>
      </div>

      <div className="desk-panel-grid desk-panel-grid-secondary">
        <Panel title="Risk denials" subtitle="Most recent strategy and symbol blocks.">
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

        <Panel title="Strategy suppressions" subtitle="Active trading blocks that matter right now.">
          {activeSuppressions.length > 0 ? (
            <DataTable
              rows={activeSuppressions}
              rowKey={(row) => row.suppression_id}
              columns={[
                { key: "strategy", title: "Strategy", render: (row) => row.strategy_id },
                { key: "domain", title: "Domain", render: (row) => row.trigger_domain },
                { key: "reason", title: "Reason", render: (row) => row.trigger_reason },
                { key: "started", title: "Started", render: (row) => formatDateTime(row.started_at) },
              ]}
            />
          ) : (
            <div className="empty-state">No active suppressions.</div>
          )}
        </Panel>

        <Panel title="Operator context" compact>
          <div className="metric-list compact-list">
            <div><span>Source state</span><strong>{model.dataSource.state}</strong></div>
            <div><span>Mock sections</span><strong>{model.dataSource.mockSections.length}</strong></div>
            <div><span>Warnings</span><strong>{model.preflight.warnings.length}</strong></div>
            <div><span>Blockers</span><strong>{model.preflight.blockers.length}</strong></div>
          </div>
        </Panel>
      </div>
    </div>
  );
}
