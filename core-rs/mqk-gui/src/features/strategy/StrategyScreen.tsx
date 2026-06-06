import { DataTable } from "../../components/common/DataTable";
import { Panel } from "../../components/common/Panel";
import { StatCard } from "../../components/common/StatCard";
import { TruthStateNotice } from "../../components/common/TruthStateNotice";
import { formatDateTime } from "../../lib/format";
import { panelTruthRenderState } from "../system/truthRendering";
import type { StrategyDecisionDiagnostics, SystemModel } from "../system/types";

function decisionTone(decision: string): "good" | "warn" | "neutral" {
  if (decision === "signal_long") return "good";
  if (decision === "insufficient_bars") return "warn";
  return "neutral";
}

function formatMicrosAsPrice(micros: number | null): string {
  if (micros == null) return "—";
  return (micros / 1_000_000).toLocaleString(undefined, {
    style: "currency",
    currency: "USD",
    minimumFractionDigits: 2,
    maximumFractionDigits: 4,
  });
}

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

      {/* STRATEGY-DECISION-OBSERVABILITY-01: Last bar signal decision diagnostics.
          Read-only. Source: GET /api/v1/autonomous/readiness → strategy_decision_diagnostics.
          Only rendered when paper+alpaca and at least one bar has been dispatched. */}
      {model.autonomousBarTickCount != null && (
        <div className="desk-panel-grid desk-panel-grid-secondary">
          <Panel
            title="Last bar signal decision"
            subtitle="Read-only diagnostic from the most recent native strategy bar dispatch (autonomous/readiness)."
          >
            {model.strategyDecisionDiagnostics == null ? (
              <div className="empty-state">
                {model.autonomousBarTickCount === 0
                  ? "No bar dispatched yet this session — decision context will appear after the first tick."
                  : "Decision diagnostics unavailable."}
              </div>
            ) : (
              <SignalDecisionPanel
                diag={model.strategyDecisionDiagnostics}
                barTickCount={model.autonomousBarTickCount}
                lastSignalQty={model.autonomousLastSignalQty}
                barContextSource={model.autonomousBarContextSource}
              />
            )}
          </Panel>

          <Panel
            title="Autonomous readiness blockers"
            subtitle="Active blockers from GET /api/v1/autonomous/readiness. Empty when all gates pass."
          >
            {model.autonomousBlockers.length === 0 ? (
              <div className="empty-state">No blockers — all autonomous readiness gates pass.</div>
            ) : (
              <ul className="blocker-list">
                {model.autonomousBlockers.map((b, i) => (
                  // blockers are a stable ordered list — index is safe as key here
                  // eslint-disable-next-line react/no-array-index-key
                  <li key={i} className="blocker-item">{b}</li>
                ))}
              </ul>
            )}
          </Panel>
        </div>
      )}
    </div>
  );
}

function SignalDecisionPanel({
  diag,
  barTickCount,
  lastSignalQty,
  barContextSource,
}: {
  diag: StrategyDecisionDiagnostics;
  barTickCount: number | null;
  lastSignalQty: number | null;
  barContextSource: string | null;
}) {
  return (
    <div className="metric-list">
      <div><span>Strategy</span><strong>{diag.strategy_id}</strong></div>
      <div><span>Symbol</span><strong>{diag.symbol}</strong></div>
      <div><span>Timeframe</span><strong>{diag.timeframe}</strong></div>
      <div>
        <span>Decision</span>
        <strong className={decisionTone(diag.decision) === "good" ? "val-positive" : decisionTone(diag.decision) === "warn" ? "val-warn" : undefined}>
          {diag.decision}
        </strong>
      </div>
      <div><span>Reason</span><strong>{diag.reason}</strong></div>
      <div><span>move_bps</span><strong>{diag.move_bps != null ? `${diag.move_bps} bps` : "—"}</strong></div>
      <div><span>abs_move_bps</span><strong>{diag.abs_move_bps != null ? `${diag.abs_move_bps} bps` : "—"}</strong></div>
      <div><span>threshold_bps</span><strong>{diag.threshold_bps} bps</strong></div>
      <div>
        <span>gap_to_threshold_bps</span>
        <strong className={diag.gap_to_threshold_bps != null && diag.gap_to_threshold_bps <= 0 ? "val-positive" : undefined}>
          {diag.gap_to_threshold_bps != null ? `${diag.gap_to_threshold_bps} bps` : "—"}
        </strong>
      </div>
      <div><span>raw_direction</span><strong>{diag.raw_direction === 1 ? "+1 (bullish)" : diag.raw_direction === -1 ? "-1 (bearish)" : "0 (neutral)"}</strong></div>
      <div><span>lookback_bars</span><strong>{diag.lookback_bars}</strong></div>
      <div><span>latest_close</span><strong>{formatMicrosAsPrice(diag.latest_close_micros)}</strong></div>
      <div><span>lookback_close</span><strong>{formatMicrosAsPrice(diag.lookback_close_micros)}</strong></div>
      <div><span>Bar ticks dispatched</span><strong>{barTickCount ?? "—"}</strong></div>
      <div><span>Last signal qty</span><strong>{lastSignalQty != null ? String(lastSignalQty) : "—"}</strong></div>
      <div><span>Bar context source</span><strong>{barContextSource ?? "—"}</strong></div>
    </div>
  );
}
