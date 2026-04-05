import type { PreflightStatus } from "../../features/system/types";

interface PreflightGateProps {
  preflight: PreflightStatus;
}

const CHECKS: Array<{ key: keyof PreflightStatus; label: string }> = [
  { key: "daemon_reachable", label: "Daemon reachable" },
  { key: "db_reachable", label: "Database reachable" },
  { key: "broker_config_present", label: "Broker config present" },
  { key: "market_data_config_present", label: "Market data config present" },
  { key: "audit_writer_ready", label: "Audit writer ready" },
  { key: "runtime_idle", label: "Runtime idle" },
  { key: "strategy_disarmed", label: "Strategy disarmed" },
  { key: "execution_disarmed", label: "Execution disarmed" },
  { key: "live_routing_disabled", label: "Live routing disabled" },
];

// AUTON-GUI-01: Autonomous-paper checks shown only when paper+alpaca.
// Consuming daemon truth directly — never approximated.
const AUTONOMOUS_CHECKS: Array<{ key: keyof PreflightStatus; label: string }> = [
  { key: "ws_continuity_ready", label: "WS continuity proven (live)" },
  { key: "reconcile_ready", label: "Reconcile clean (not dirty/stale)" },
  { key: "session_in_window", label: "Session window open" },
];

function armStateLabel(state: string | undefined): string {
  switch (state) {
    case "armed": return "Armed";
    case "arm_pending": return "Arm pending (DB check on next tick)";
    case "halted": return "Halted — operator must arm";
    case "not_applicable": return "N/A";
    default: return state ?? "Unknown";
  }
}

export function PreflightGate({ preflight }: PreflightGateProps) {
  const showAutonomous = Boolean(preflight.autonomous_readiness_applicable);

  return (
    <section className="panel preflight-panel">
      <div className="panel-header">
        <div>
          <div className="eyebrow">Startup Safety Model</div>
          <h2>Preflight gate</h2>
        </div>
      </div>
      <div className="checklist-grid">
        {CHECKS.map((check) => {
          const ok = Boolean(preflight[check.key]);
          return (
            <div key={check.key} className={`check-card ${ok ? "is-ok" : "is-blocked"}`}>
              <span className="check-icon">{ok ? "✓" : "!"}</span>
              <div>
                <strong>{check.label}</strong>
                <p>{ok ? "Ready" : "Review required"}</p>
              </div>
            </div>
          );
        })}
      </div>

      {showAutonomous && (
        <>
          <div className="panel-header" style={{ marginTop: "1rem" }}>
            <div>
              <div className="eyebrow">Paper + Alpaca Autonomous Path</div>
              <h3>Autonomous readiness</h3>
            </div>
          </div>
          <div className="checklist-grid">
            {AUTONOMOUS_CHECKS.map((check) => {
              const val = preflight[check.key];
              const ok = val === true;
              const unknown = val == null;
              return (
                <div
                  key={check.key}
                  className={`check-card ${ok ? "is-ok" : unknown ? "is-unknown" : "is-blocked"}`}
                >
                  <span className="check-icon">{ok ? "✓" : unknown ? "?" : "!"}</span>
                  <div>
                    <strong>{check.label}</strong>
                    <p>{ok ? "Ready" : unknown ? "Unknown" : "Blocking"}</p>
                  </div>
                </div>
              );
            })}
            <div
              className={`check-card ${
                preflight.autonomous_arm_state === "armed" ? "is-ok" :
                preflight.autonomous_arm_state === "arm_pending" ? "is-warning" : "is-blocked"
              }`}
            >
              <span className="check-icon">
                {preflight.autonomous_arm_state === "armed" ? "✓" :
                 preflight.autonomous_arm_state === "arm_pending" ? "~" : "!"}
              </span>
              <div>
                <strong>Autonomous arm state</strong>
                <p>{armStateLabel(preflight.autonomous_arm_state)}</p>
              </div>
            </div>
          </div>
        </>
      )}

      <div className="preflight-notes">
        <div>
          <h3>Warnings</h3>
          {preflight.warnings.length > 0 ? (
            <ul>
              {preflight.warnings.map((warning) => (
                <li key={warning}>{warning}</li>
              ))}
            </ul>
          ) : (
            <p>No warnings.</p>
          )}
        </div>
        <div>
          <h3>Blockers</h3>
          {preflight.blockers.length > 0 ? (
            <ul>
              {preflight.blockers.map((blocker) => (
                <li key={blocker}>{blocker}</li>
              ))}
            </ul>
          ) : (
            <p>No blockers.</p>
          )}
        </div>
      </div>
    </section>
  );
}
