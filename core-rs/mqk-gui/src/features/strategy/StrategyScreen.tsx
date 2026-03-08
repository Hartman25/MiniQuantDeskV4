import { DataTable } from "../../components/common/DataTable";
import { Panel } from "../../components/common/Panel";
import { formatDateTime, formatLabel, formatMoney, formatPercent } from "../../lib/format";
import type { SystemModel } from "../system/types";

export function StrategyScreen({ model }: { model: SystemModel }) {
  return (
    <div className="screen-grid">
      <Panel title="Strategy matrix" subtitle="Monitor strategy engines without manual trading controls.">
        <DataTable
          rows={model.strategies}
          rowKey={(row) => row.strategy_id}
          columns={[
            { key: "strategy", title: "Strategy", render: (row) => row.strategy_id },
            { key: "enabled", title: "Enabled", render: (row) => (row.enabled ? "Yes" : "No") },
            { key: "armed", title: "Armed", render: (row) => (row.armed ? "Yes" : "No") },
            { key: "health", title: "Health", render: (row) => row.health },
            { key: "universe", title: "Universe", render: (row) => row.universe },
            { key: "pending", title: "Pending Intents", render: (row) => row.pending_intents },
            { key: "positions", title: "Open Positions", render: (row) => row.open_positions },
            { key: "pnl", title: "Today PnL", render: (row) => formatMoney(row.today_pnl) },
            { key: "dd", title: "Drawdown", render: (row) => formatPercent(row.drawdown_pct) },
            { key: "regime", title: "Regime", render: (row) => row.regime },
            { key: "throttle", title: "Throttle", render: (row) => row.throttle_state },
            { key: "decision", title: "Last Decision", render: (row) => formatDateTime(row.last_decision_time) },
          ]}
        />
      </Panel>

      <Panel title="Suppression lineage" subtitle="Every blocked or throttled strategy needs a reason and a timeline.">
        <DataTable
          rows={model.strategySuppressions}
          rowKey={(row) => row.suppression_id}
          columns={[
            { key: "id", title: "Suppression", render: (row) => row.suppression_id },
            { key: "strategy", title: "Strategy", render: (row) => row.strategy_id },
            { key: "state", title: "State", render: (row) => formatLabel(row.state) },
            { key: "domain", title: "Trigger Domain", render: (row) => formatLabel(row.trigger_domain) },
            { key: "reason", title: "Trigger Reason", render: (row) => row.trigger_reason },
            { key: "started", title: "Started", render: (row) => formatDateTime(row.started_at) },
            { key: "cleared", title: "Cleared", render: (row) => formatDateTime(row.cleared_at) },
            { key: "note", title: "Note", render: (row) => row.note },
          ]}
        />
      </Panel>
    </div>
  );
}
