import { DataTable } from "../../components/common/DataTable";
import { Panel } from "../../components/common/Panel";
import { StatCard } from "../../components/common/StatCard";
import { TruthStateNotice } from "../../components/common/TruthStateNotice";
import { formatDateTime } from "../../lib/format";
import { panelTruthRenderState } from "../system/truthRendering";
import type {
  AdmissionCheckSurface,
  AutonomousPaperStatusSurface,
  StrategyDecisionDiagnostics,
  SystemModel,
  WatchlistStatusSurface,
} from "../system/types";

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

      {/* GUI-PAPER-STATUS-VISIBILITY-01: Autonomous paper status — composite read-only
          truth surface (runtime/arm/reconcile/flatten/watchlist/readiness). Read-only
          visibility: this block contains no buttons or invocation paths of any kind.
          Source: GET /api/v1/autonomous/paper-status. */}
      <div className="desk-panel-grid desk-panel-grid-secondary">
        <Panel
          title="Autonomous paper status"
          subtitle="Read-only composite truth surface. Source: GET /api/v1/autonomous/paper-status"
        >
          {model.autonomousPaperStatus.truth_state === "unavailable" ? (
            <div className="unavailable-notice">
              Autonomous paper status is currently unavailable — backend truth could not be retrieved.
            </div>
          ) : model.autonomousPaperStatus.truth_state === "not_applicable" ? (
            <div className="unavailable-notice">
              Autonomous paper status is not applicable for the current deployment mode (paper+alpaca only).
            </div>
          ) : (
            <AutonomousPaperStatusPanel status={model.autonomousPaperStatus} />
          )}
        </Panel>

        <Panel
          title="Flatten availability (visibility only)"
          subtitle="Read-only — does not invoke flatten-paper-positions. Source: autonomous/paper-status.flatten_available / flatten_blockers"
        >
          {model.autonomousPaperStatus.truth_state !== "active" ? (
            <div className="unavailable-notice">
              Flatten availability truth is currently unavailable for this deployment context.
            </div>
          ) : (
            <>
              <div className="metric-list">
                <div>
                  <span>Flatten available</span>
                  <strong className={model.autonomousPaperStatus.flatten_available ? "val-positive" : "val-warn"}>
                    {model.autonomousPaperStatus.flatten_available ? "Yes" : "No"}
                  </strong>
                </div>
              </div>
              {model.autonomousPaperStatus.flatten_blockers.length === 0 ? (
                <div className="empty-state">No flatten blockers reported.</div>
              ) : (
                <ul className="blocker-list">
                  {model.autonomousPaperStatus.flatten_blockers.map((b, i) => (
                    // blockers are a stable ordered list — index is safe as key here
                    // eslint-disable-next-line react/no-array-index-key
                    <li key={i} className="blocker-item">{b}</li>
                  ))}
                </ul>
              )}
              <p className="panel-notice">
                Visibility only — this panel cannot invoke flatten-paper-positions. Operators must use the
                authorized control surface to act on this information.
              </p>
            </>
          )}
        </Panel>
      </div>

      {/* GUI-PAPER-STATUS-VISIBILITY-01: Watchlist status and admission-check (dry-run).
          Both are read-only advisory surfaces — neither gates nor triggers trading behavior. */}
      <div className="desk-panel-grid desk-panel-grid-secondary">
        <Panel
          title="Watchlist status"
          subtitle="Read-only configured-watchlist truth. Source: GET /api/v1/watchlist/status"
        >
          {model.watchlistStatus.truth_state !== "active" ? (
            <div className="unavailable-notice">
              Watchlist status is currently unavailable — backend truth could not be retrieved.
            </div>
          ) : (
            <WatchlistStatusPanel status={model.watchlistStatus} />
          )}
        </Panel>

        <Panel
          title="Watchlist admission-check (dry-run, advisory only)"
          subtitle="Read-only dry-run check — never gates or triggers trading behavior. Source: GET /api/v1/watchlist/admission-check"
        >
          <AdmissionCheckPanel check={model.admissionCheck} />
        </Panel>
      </div>
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

// GUI-PAPER-STATUS-VISIBILITY-01: Read-only render of the autonomous paper status
// composite truth surface. Renders backend-reported values verbatim — never
// substitutes friendly defaults for unknown/null fields.
function AutonomousPaperStatusPanel({ status }: { status: AutonomousPaperStatusSurface }) {
  return (
    <>
      <div className="metric-list">
        <div><span>Readiness classification</span><strong>{status.readiness_classification}</strong></div>
        <div><span>Next operator action</span><strong>{status.next_operator_action ?? "—"}</strong></div>
        <div><span>Mode</span><strong>{status.mode}</strong></div>
        <div>
          <span>Live routing enabled</span>
          <strong className={status.live_routing_enabled ? "val-negative" : "val-positive"}>
            {status.live_routing_enabled ? "Yes" : "No"}
          </strong>
        </div>
        <div><span>Runtime status</span><strong>{status.runtime_status}</strong></div>
        <div><span>Arm state</span><strong>{status.arm_state}</strong></div>
        <div>
          <span>Kill switch active</span>
          <strong className={status.kill_switch_active ? "val-negative" : "val-positive"}>
            {status.kill_switch_active ? "Yes" : "No"}
          </strong>
        </div>
        <div><span>Deadman status</span><strong>{status.deadman_status}</strong></div>
        <div><span>WS continuity</span><strong>{status.ws_continuity}</strong></div>
        <div><span>Reconcile status</span><strong>{status.reconcile_status}</strong></div>
        <div><span>Mismatch count</span><strong>{status.mismatch_count ?? "—"}</strong></div>
        <div><span>Open order count</span><strong>{status.open_order_count ?? "—"}</strong></div>
        <div><span>Position count</span><strong>{status.position_count ?? "—"}</strong></div>
        <div><span>Current symbol</span><strong>{status.current_symbol ?? "—"}</strong></div>
        <div><span>Current position qty</span><strong>{status.current_position_qty ?? "—"}</strong></div>
        <div><span>Target qty</span><strong>{status.target_qty ?? "—"}</strong></div>
        <div><span>Computed delta qty</span><strong>{status.computed_delta_qty ?? "—"}</strong></div>
        <div><span>No-order reason</span><strong>{status.no_order_reason ?? "—"}</strong></div>
        <div><span>Last strategy decision</span><strong>{status.last_strategy_decision ?? "—"}</strong></div>
        <div><span>Watchlist outcome</span><strong>{status.watchlist_outcome}</strong></div>
        <div>
          <span>Watchlist approved</span>
          <strong className={status.watchlist_approved ? "val-positive" : "val-warn"}>
            {status.watchlist_approved ? "Yes" : "No"}
          </strong>
        </div>
        <div><span>Autonomous session state</span><strong>{status.autonomous_session_state}</strong></div>
        <div><span>As of</span><strong>{status.now_utc ? formatDateTime(status.now_utc) : "—"}</strong></div>
        <div><span>Blockers</span><strong>{status.blockers.length}</strong></div>
      </div>
      {status.blockers.length > 0 && (
        <ul className="blocker-list">
          {status.blockers.map((b, i) => (
            // blockers are a stable ordered list — index is safe as key here
            // eslint-disable-next-line react/no-array-index-key
            <li key={i} className="blocker-item">{b}</li>
          ))}
        </ul>
      )}
    </>
  );
}

// GUI-PAPER-STATUS-VISIBILITY-01: Read-only render of the watchlist status truth surface.
// approved_for_live is rendered with negative tone when true — an institutional posture
// where live approval is alarming, not reassuring, until live trading is authorized.
function WatchlistStatusPanel({ status }: { status: WatchlistStatusSurface }) {
  return (
    <>
      <div className="metric-list">
        <div><span>Status</span><strong>{status.status}</strong></div>
        <div><span>Configured path</span><strong>{status.configured_path ?? "—"}</strong></div>
        <div>
          <span>Approved for autonomous paper</span>
          <strong className={status.approved_for_autonomous_paper ? "val-positive" : "val-warn"}>
            {status.approved_for_autonomous_paper ? "Yes" : "No"}
          </strong>
        </div>
        <div>
          <span>Approved for live</span>
          <strong className={status.approved_for_live ? "val-negative" : "val-positive"}>
            {status.approved_for_live ? "Yes" : "No"}
          </strong>
        </div>
        <div><span>Top symbol</span><strong>{status.top_symbol ?? "—"}</strong></div>
        <div><span>Symbols</span><strong>{status.symbols.length > 0 ? status.symbols.join(", ") : "—"}</strong></div>
        <div><span>Strategy assignments</span><strong>{status.strategy_assignment_count ?? "—"}</strong></div>
        <div><span>Max symbols to trade</span><strong>{status.max_symbols_to_trade ?? "—"}</strong></div>
        <div><span>Max concurrent positions</span><strong>{status.max_concurrent_positions ?? "—"}</strong></div>
        <div><span>Checked at</span><strong>{status.checked_at_utc ? formatDateTime(status.checked_at_utc) : "—"}</strong></div>
      </div>
      {status.failure_reasons.length > 0 && (
        <ul className="blocker-list">
          {status.failure_reasons.map((r, i) => (
            // failure reasons are a stable ordered list — index is safe as key here
            // eslint-disable-next-line react/no-array-index-key
            <li key={i} className="blocker-item">{r}</li>
          ))}
        </ul>
      )}
    </>
  );
}

// GUI-PAPER-STATUS-VISIBILITY-01: Read-only render of the watchlist admission-check
// dry-run surface. "not_checked" means no real symbol/strategy pair was available from
// backend truth — this code path never invents a default symbol (e.g. hardcoded "AAPL").
// The result is always rendered as advisory-only and never used to trigger any action.
function AdmissionCheckPanel({ check }: { check: AdmissionCheckSurface }) {
  if (check.state === "not_checked") {
    return (
      <div className="unavailable-notice">
        Admission-check not run — {check.reason_unchecked ?? "symbol/strategy unavailable"}. This panel never
        substitutes a default symbol or strategy to force a check.
      </div>
    );
  }
  if (check.state === "unavailable") {
    return (
      <div className="unavailable-notice">
        Admission-check for {check.symbol ?? "—"} / {check.strategy_id ?? "—"} is currently unavailable —
        backend truth could not be retrieved.
      </div>
    );
  }
  return (
    <>
      <div className="metric-list">
        <div><span>Symbol</span><strong>{check.symbol ?? "—"}</strong></div>
        <div><span>Strategy</span><strong>{check.strategy_id ?? "—"}</strong></div>
        <div>
          <span>Allowed (dry-run)</span>
          <strong className={check.allowed === null ? undefined : check.allowed ? "val-positive" : "val-warn"}>
            {check.allowed === null ? "—" : check.allowed ? "Yes" : "No"}
          </strong>
        </div>
        <div><span>Reason</span><strong>{check.reason_code ?? "—"}</strong></div>
        <div><span>Status</span><strong>{check.status ?? "—"}</strong></div>
        <div>
          <span>Approved for autonomous paper</span>
          <strong className={check.approved_for_autonomous_paper === null ? undefined : check.approved_for_autonomous_paper ? "val-positive" : "val-warn"}>
            {check.approved_for_autonomous_paper === null ? "—" : check.approved_for_autonomous_paper ? "Yes" : "No"}
          </strong>
        </div>
        <div>
          <span>Approved for live</span>
          <strong className={check.approved_for_live === null ? undefined : check.approved_for_live ? "val-negative" : "val-positive"}>
            {check.approved_for_live === null ? "—" : check.approved_for_live ? "Yes" : "No"}
          </strong>
        </div>
        <div><span>Checked at</span><strong>{check.checked_at_utc ? formatDateTime(check.checked_at_utc) : "—"}</strong></div>
      </div>
      <p className="panel-notice">
        {check.note ?? "dry_run_only_not_enforced"} — advisory dry-run only. This result does not gate signal
        admission and must never be used to trigger trading behavior.
      </p>
    </>
  );
}
