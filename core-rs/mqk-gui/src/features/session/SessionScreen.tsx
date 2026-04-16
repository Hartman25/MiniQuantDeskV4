import { Panel } from "../../components/common/Panel";
import { StatCard } from "../../components/common/StatCard";
import { TruthStateBanner } from "../../components/common/TruthStateBanner";
import { TruthStateNotice } from "../../components/common/TruthStateNotice";
import { formatDateTime } from "../../lib/format";
import { isTruthHardBlock, panelTruthRenderState } from "../system/truthRendering";
import type { SystemModel } from "../system/types";

export function SessionScreen({ model }: { model: SystemModel }) {
  const s = model.sessionState;
  const pf = model.preflight;
  const truthState = panelTruthRenderState(model, "session");

  // Hard-block when truth is structurally absent (unavailable, no_snapshot, unimplemented,
  // not_wired). For stale/degraded, cached session state is still operator context —
  // show the domain body with a warning banner rather than a blank screen.
  if (truthState !== null && isTruthHardBlock(truthState)) {
    return <TruthStateNotice state={truthState} />;
  }

  const wsContLabel = (() => {
    switch (model.status.alpaca_ws_continuity) {
      case "live": return "Live";
      case "cold_start_unproven": return "Cold start — unproven";
      case "gap_detected": return "Gap detected";
      case "not_applicable": return "Not applicable";
      default: return String(model.status.alpaca_ws_continuity);
    }
  })();

  const wsContTone: "good" | "warn" | "bad" | "neutral" = (() => {
    switch (model.status.alpaca_ws_continuity) {
      case "live": return "good";
      case "cold_start_unproven": return "warn";
      case "gap_detected": return "bad";
      default: return "neutral";
    }
  })();

  return (
    <div className="screen-grid desk-screen-grid">
      {truthState !== null && <TruthStateBanner state={truthState} />}
      {/* Orientation strip — compact, not repeated on Dashboard */}
      <div className="summary-grid summary-grid-four">
        <StatCard
          title="Market Session"
          value={s.market_session}
          detail={`Calendar: ${s.exchange_calendar_state}`}
          tone={s.market_session === "regular" ? "good" : "neutral"}
        />
        <StatCard
          title="Trading Window"
          value={s.system_trading_window}
          detail={`Strategy: ${s.strategy_allowed ? "allowed" : "blocked"}`}
          tone={s.system_trading_window === "enabled" ? "good" : s.system_trading_window === "exit_only" ? "warn" : "bad"}
        />
        <StatCard
          title="Next Transition"
          value={s.next_session_change_at ? formatDateTime(s.next_session_change_at) : "—"}
          detail="Next scheduled session change"
          tone="neutral"
        />
        <StatCard
          title="WS Continuity"
          value={wsContLabel}
          detail="Alpaca trade-update feed state"
          tone={wsContTone}
        />
      </div>

      {/* Core body — session gate state and deployment context. Both panels are
          specific to this screen; they do not appear on Dashboard in any tab. */}
      <div className="two-column-grid">
        <Panel
          title="Trading session gate"
          subtitle="Conditions that control whether execution is currently permitted."
        >
          <div className="metric-list">
            <div>
              <span>Exchange calendar</span>
              <strong>{s.exchange_calendar_state}</strong>
            </div>
            <div>
              <span>Calendar spec</span>
              <strong>{s.calendar_spec_id ?? "—"}</strong>
            </div>
            <div>
              <span>System trading window</span>
              <strong>{s.system_trading_window}</strong>
            </div>
            <div>
              <span>Strategy signal allowed</span>
              <strong>{s.strategy_allowed ? "Yes" : "No"}</strong>
            </div>
            {pf.autonomous_readiness_applicable && (
              <>
                <div>
                  <span>Session in window</span>
                  <strong>
                    {pf.session_in_window == null ? "—" : pf.session_in_window ? "Yes" : "No"}
                  </strong>
                </div>
                <div>
                  <span>WS continuity ready</span>
                  <strong>
                    {pf.ws_continuity_ready == null ? "—" : pf.ws_continuity_ready ? "Yes" : "No"}
                  </strong>
                </div>
                <div>
                  <span>Autonomous arm state</span>
                  <strong>{pf.autonomous_arm_state ?? "—"}</strong>
                </div>
              </>
            )}
          </div>
          {pf.autonomous_readiness_applicable &&
            pf.autonomous_blockers != null &&
            pf.autonomous_blockers.length > 0 && (
              <div className="list-stack" style={{ marginTop: 12 }}>
                {pf.autonomous_blockers.map((blocker) => (
                  <div key={blocker} className="list-row">
                    <strong style={{ color: "var(--critical)" }}>Blocker</strong>
                    <span>{blocker}</span>
                  </div>
                ))}
              </div>
            )}
        </Panel>

        <Panel
          title="Deployment context"
          subtitle="Mode, adapter, and authority state for this session."
        >
          <div className="metric-list">
            <div>
              <span>Deployment mode</span>
              <strong>{s.daemon_mode ?? model.status.daemon_mode}</strong>
            </div>
            <div>
              <span>Broker adapter</span>
              <strong>{s.adapter_id ?? model.status.adapter_id}</strong>
            </div>
            <div>
              <span>Start allowed</span>
              <strong>
                {(s.deployment_start_allowed ?? model.status.deployment_start_allowed) ? "Yes" : "No"}
              </strong>
            </div>
            {s.deployment_blocker && (
              <div>
                <span>Start blocker</span>
                <strong style={{ color: "var(--warning)" }}>{s.deployment_blocker}</strong>
              </div>
            )}
            <div>
              <span>Operator auth mode</span>
              <strong>{s.operator_auth_mode ?? "—"}</strong>
            </div>
            <div>
              <span>Parity evidence</span>
              <strong>{model.status.parity_evidence_state}</strong>
            </div>
            <div>
              <span>Live trust complete</span>
              <strong>
                {model.status.live_trust_complete == null
                  ? "—"
                  : model.status.live_trust_complete
                    ? "Yes"
                    : "No"}
              </strong>
            </div>
          </div>
        </Panel>
      </div>

      {/* Session notes — operator context and upcoming transition warnings */}
      {s.notes.length > 0 && (
        <Panel title="Session notes" subtitle="Operator session context and upcoming transitions.">
          <div className="list-stack">
            {s.notes.map((note) => (
              <div key={note} className="list-row">
                <strong>Note</strong>
                <span>{note}</span>
              </div>
            ))}
          </div>
        </Panel>
      )}
    </div>
  );
}
