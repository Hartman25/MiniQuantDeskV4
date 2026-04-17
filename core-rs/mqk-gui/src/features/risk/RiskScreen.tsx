import { DataTable } from "../../components/common/DataTable";
import { Panel } from "../../components/common/Panel";
import { StatCard } from "../../components/common/StatCard";
import { TruthStateNotice } from "../../components/common/TruthStateNotice";
import { formatDateTime, formatMoney, formatPercent } from "../../lib/format";
import { panelTruthRenderState } from "../system/truthRendering";
import type { SystemModel } from "../system/types";

export function RiskScreen({ model }: { model: SystemModel }) {
  const r = model.riskSummary;
  const truthState = panelTruthRenderState(model, "risk");
  const activeSuppressions = model.strategySuppressions.filter((row) => row.state === "active");

  // Hard-close on any compromised truth state: stale risk figures (concentration, loss-limit
  // utilization, kill switch) must not render as authoritative. Inline notice is insufficient.
  if (truthState !== null) {
    return <TruthStateNotice state={truthState} />;
  }

  return (
    <div className="screen-grid desk-screen-grid">
      {/* Exposure summary — the four numbers an operator reads first on this screen */}
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

      {/* Breach and halt posture — risk-triggered stops only.
          Armed/disarmed and live routing are on Ops; runtime health is on Dashboard. */}
      <Panel title="Breach and halt posture" subtitle="Risk-triggered hard stops, loss limits, and breach counts. Not system arm state.">
        <div className="metric-list">
          <div><span>Daily PnL</span><strong>{formatMoney(r.daily_pnl)}</strong></div>
          <div><span>Drawdown</span><strong>{formatPercent(r.drawdown_pct)}</strong></div>
          <div><span>Kill switch</span><strong>{r.kill_switch_active ? "Active" : "Inactive"}</strong></div>
          <div><span>Active breaches</span><strong>{r.active_breaches}</strong></div>
          <div><span>Risk halt</span><strong>{model.status.risk_halt_active ? "Active" : "Clear"}</strong></div>
          <div><span>Integrity halt</span><strong>{model.status.integrity_halt_active ? "Active" : "Clear"}</strong></div>
        </div>
      </Panel>

      {/* Denial and suppression tables — what is being blocked and why */}
      <div className="two-column-grid">
        <Panel title="Risk denials" subtitle="Most recent strategy and symbol blocks by the risk layer.">
          <DataTable
            rows={model.riskDenials}
            rowKey={(row) => row.id}
            columns={[
              { key: "at", title: "At", render: (row) => formatDateTime(row.at) },
              { key: "strategy", title: "Strategy", render: (row) => row.strategy_id ?? "—" },
              { key: "symbol", title: "Symbol", render: (row) => row.symbol },
              { key: "rule", title: "Rule", render: (row) => row.rule },
              { key: "message", title: "Message", render: (row) => row.message },
            ]}
          />
        </Panel>

        <Panel title="Active suppressions" subtitle="Strategies currently blocked from signal admission. These are risk admission gates, not configuration flags.">
          {model.strategySuppressionsTruth.truth_state === "not_wired" ? (
            <div className="unavailable-notice">
              Strategy suppression truth is mounted but not wired. Do not read this as no active suppressions.
            </div>
          ) : model.strategySuppressionsTruth.truth_state !== "active" ? (
            <div className="unavailable-notice">
              Strategy suppression truth is currently unavailable. Do not treat the empty row set as authoritative.
            </div>
          ) : activeSuppressions.length > 0 ? (
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
      </div>
    </div>
  );
}
