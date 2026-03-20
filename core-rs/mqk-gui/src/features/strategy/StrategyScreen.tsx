import { DataTable } from "../../components/common/DataTable";
import { Panel } from "../../components/common/Panel";
import { StatCard } from "../../components/common/StatCard";
import { TruthStateNotice } from "../../components/common/TruthStateNotice";
import { formatDateTime, formatMoney, formatPercent } from "../../lib/format";
import { panelTruthRenderState } from "../system/truthRendering";
import type { SystemModel } from "../system/types";

export function StrategyScreen({ model }: { model: SystemModel }) {
  const armed = model.strategies.filter((s) => s.armed).length;
  const throttled = model.strategies.filter((s) => s.throttle_state !== "normal").length;
  const unhealthy = model.strategies.filter((s) => s.health !== "ok").length;
  const truthState = panelTruthRenderState(model, "strategy");

  if (truthState !== null) {
    return <TruthStateNotice state={truthState} />;
  }

  return (
    <div className="screen-grid desk-screen-grid">
      <div className="summary-grid summary-grid-four">
        <StatCard title="Strategies" value={String(model.strategies.length)} detail="Configured strategy engines" tone="good" />
        <StatCard title="Armed" value={String(armed)} detail="Strategies currently armed" tone={armed > 0 ? "good" : "warn"} />
        <StatCard title="Throttled" value={String(throttled)} detail="Throttle / suppression active" tone={throttled > 0 ? "warn" : "good"} />
        <StatCard title="Warnings" value={String(unhealthy)} detail="Strategies not in ok health" tone={unhealthy > 0 ? "warn" : "good"} />
      </div>

      <Panel title="Strategy engines" subtitle="Monitor strategy runtime health without turning the GUI into manual trading software.">
        {model.strategies.length === 0 ? (
          <div className="empty-state">No strategy summary rows reported.</div>
        ) : (
          <DataTable
            rows={model.strategies}
            rowKey={(row) => row.strategy_id}
            columns={[
              { key: "strategy", title: "Strategy", render: (row) => row.strategy_id },
              { key: "enabled", title: "Enabled", render: (row) => (row.enabled ? "Yes" : "No") },
              { key: "armed", title: "Armed", render: (row) => (row.armed ? "Yes" : "No") },
              { key: "health", title: "Health", render: (row) => row.health },
              { key: "universe", title: "Universe", render: (row) => row.universe },
              { key: "intents", title: "Pending Intents", render: (row) => row.pending_intents },
              { key: "positions", title: "Open Positions", render: (row) => row.open_positions },
              { key: "pnl", title: "Today PnL", render: (row) => formatMoney(row.today_pnl) },
              { key: "drawdown", title: "Drawdown", render: (row) => formatPercent(row.drawdown_pct) },
              { key: "regime", title: "Regime", render: (row) => row.regime },
              { key: "throttle", title: "Throttle", render: (row) => row.throttle_state },
              { key: "last", title: "Last Decision", render: (row) => formatDateTime(row.last_decision_time) },
            ]}
          />
        )}
      </Panel>

      <Panel title="Strategy suppressions" subtitle="Active and historical suppressions affecting strategy output.">
        {model.strategySuppressionsTruth.truth_state === "not_wired" ? (
          <div className="unavailable-notice">
            Strategy suppression truth is mounted but not wired. Empty rows do not mean there are no suppressions.
          </div>
        ) : model.strategySuppressionsTruth.truth_state !== "active" ? (
          <div className="unavailable-notice">
            Strategy suppression truth is currently unavailable. Do not treat the empty row set as authoritative.
          </div>
        ) : model.strategySuppressions.length === 0 ? (
          <div className="empty-state">No strategy suppressions recorded.</div>
        ) : (
          <DataTable
            rows={model.strategySuppressions}
            rowKey={(row) => row.suppression_id}
            columns={[
              { key: "strategy", title: "Strategy", render: (row) => row.strategy_id },
              { key: "state", title: "State", render: (row) => row.state },
              { key: "domain", title: "Trigger Domain", render: (row) => row.trigger_domain },
              { key: "reason", title: "Reason", render: (row) => row.trigger_reason },
              { key: "started", title: "Started", render: (row) => formatDateTime(row.started_at) },
              { key: "cleared", title: "Cleared", render: (row) => row.cleared_at ? formatDateTime(row.cleared_at) : "—" },
              { key: "note", title: "Note", render: (row) => row.note },
            ]}
          />
        )}
      </Panel>
    </div>
  );
}
