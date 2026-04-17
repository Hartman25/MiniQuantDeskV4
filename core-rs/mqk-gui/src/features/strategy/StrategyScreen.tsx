import { DataTable } from "../../components/common/DataTable";
import { Panel } from "../../components/common/Panel";
import { StatCard } from "../../components/common/StatCard";
import { TruthStateNotice } from "../../components/common/TruthStateNotice";
import { formatDateTime } from "../../lib/format";
import { panelTruthRenderState } from "../system/truthRendering";
import type { SystemModel } from "../system/types";

export function StrategyScreen({ model }: { model: SystemModel }) {
  const armed = model.strategies.filter((s) => s.armed).length;
  // Enabled but not armed: the arming gap the operator must close before execution begins.
  const notArmed = model.strategies.filter((s) => s.enabled && !s.armed).length;
  const throttled = model.strategies.filter((s) => s.throttle_state === "day_limit_reached").length;
  const suppressionsTruthActive = model.strategySuppressionsTruth.truth_state === "active";
  // Active suppression count is only authoritative when truth_state is "active".
  const activeSuppressionsCount = suppressionsTruthActive
    ? model.strategySuppressions.filter((s) => s.state === "active").length
    : null;

  const truthState = panelTruthRenderState(model, "strategy");

  if (truthState !== null) {
    return <TruthStateNotice state={truthState} />;
  }

  // Sort by urgency: degraded engines with open exposure first, then arming gaps,
  // then throttled, then armed-and-healthy, then disabled.
  const sortedEngines = [...model.strategies].sort((a, b) => {
    const urgency = (s: typeof a) => {
      if (s.health !== "ok" && (s.open_positions > 0 || s.pending_intents > 0)) return 0;
      if (s.enabled && !s.armed) return 1;
      if (s.throttle_state === "day_limit_reached") return 2;
      if (s.armed && s.health === "ok") return 3;
      return 4;
    };
    return urgency(a) - urgency(b);
  });

  return (
    <div className="screen-grid desk-screen-grid">
      {/* Posture summary — arming gap and suppression pressure lead */}
      <div className="summary-grid summary-grid-four">
        <StatCard
          title="Armed"
          value={String(armed)}
          detail="Strategies currently armed"
          tone={armed > 0 ? "good" : "warn"}
        />
        <StatCard
          title="Not Armed"
          value={String(notArmed)}
          detail="Enabled but not yet armed"
          tone={notArmed > 0 ? "warn" : "good"}
        />
        <StatCard
          title="Throttled"
          value={String(throttled)}
          detail="Day limit reached"
          tone={throttled > 0 ? "warn" : "good"}
        />
        <StatCard
          title="Active Suppressions"
          value={activeSuppressionsCount !== null ? String(activeSuppressionsCount) : "—"}
          detail={suppressionsTruthActive ? "Active suppression entries" : "Suppression truth unavailable"}
          tone={
            activeSuppressionsCount !== null && activeSuppressionsCount > 0
              ? "bad"
              : suppressionsTruthActive
                ? "good"
                : "neutral"
          }
        />
      </div>

      {/* Engine posture — arm state, admission, throttle, and open exposure.
          Posture-only columns: analytics (pnl, drawdown, regime, universe) belong on Metrics/Portfolio. */}
      <Panel
        title="Engine posture"
        subtitle="Arm state, admission, throttle, and open exposure per engine. Degraded engines with open exposure sorted first."
      >
        {model.strategies.length === 0 ? (
          <div className="empty-state">No strategy engines reported.</div>
        ) : (
          <DataTable
            rows={sortedEngines}
            rowKey={(row) => row.strategy_id}
            columns={[
              { key: "strategy", title: "Strategy", render: (row) => row.strategy_id },
              { key: "armed", title: "Armed", render: (row) => (row.armed ? "Yes" : "No") },
              { key: "enabled", title: "Enabled", render: (row) => (row.enabled ? "Yes" : "No") },
              { key: "admission", title: "Admission", render: (row) => row.admission_state },
              { key: "health", title: "Health", render: (row) => row.health },
              { key: "throttle", title: "Throttle", render: (row) => row.throttle_state ?? "—" },
              { key: "intents", title: "Pending Intents", render: (row) => row.pending_intents },
              { key: "positions", title: "Open Positions", render: (row) => row.open_positions },
              { key: "last", title: "Last Decision", render: (row) => formatDateTime(row.last_decision_time) },
            ]}
          />
        )}
      </Panel>

      {/* Suppression ledger — full lifecycle (active + cleared).
          Active suppressions also appear as admission gates on the Risk screen.
          This panel is the durable record; Risk's active-only view is incident context. */}
      <Panel
        title="Suppression ledger"
        subtitle="Full suppression record — active and cleared. Active suppressions also appear as admission gates on the Risk screen."
      >
        {model.strategySuppressionsTruth.truth_state === "not_wired" ? (
          <div className="unavailable-notice">
            Strategy suppression truth is mounted but not wired. Empty rows do not mean there are no suppressions.
          </div>
        ) : model.strategySuppressionsTruth.truth_state !== "active" ? (
          <div className="unavailable-notice">
            Strategy suppression truth is currently unavailable. Do not treat the empty row set as authoritative.
          </div>
        ) : model.strategySuppressions.length === 0 ? (
          <div className="empty-state">No suppression entries recorded.</div>
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
              { key: "cleared", title: "Cleared", render: (row) => (row.cleared_at ? formatDateTime(row.cleared_at) : "—") },
              { key: "note", title: "Note", render: (row) => row.note },
            ]}
          />
        )}
      </Panel>
    </div>
  );
}
