import { useEffect, useState } from "react";
import { Panel } from "../../components/common/Panel";
import { getConflictPlanDetail, getConflictStatus } from "./api";
import type { ConflictPlanDetail, ConflictStatus } from "./types/strategyConflict";

// MULTI-STRATEGY-CONFLICT-POLICY-01 Phase D: read-only conflict-policy panel.
//
// Read-only: no mode switch, no mutation control anywhere in this component.
// truth_state governs what renders — an unrecognized/malformed response
// (parseConflictStatus/parseConflictPlanDetail) always downgrades to an
// unavailable sentinel before it reaches this component, so this file only
// has to render the closed vocabulary those parsers guarantee.
export function StrategyConflictPolicyPanel() {
  const [status, setStatus] = useState<ConflictStatus | null>(null);
  const [plan, setPlan] = useState<ConflictPlanDetail | null>(null);
  const [error, setError] = useState<string | null>(null);
  const [loading, setLoading] = useState(true);

  useEffect(() => {
    let cancelled = false;
    setLoading(true);
    void getConflictStatus().then(async (statusResult) => {
      if (cancelled) return;
      if (!statusResult.ok) {
        setError(statusResult.error ?? "Conflict policy status unavailable.");
        setLoading(false);
        return;
      }
      setError(null);
      setStatus(statusResult.data);

      if (statusResult.data.latest_plan_id) {
        const planResult = await getConflictPlanDetail(statusResult.data.latest_plan_id);
        if (!cancelled && planResult.ok) {
          setPlan(planResult.data);
        }
      }
      if (!cancelled) setLoading(false);
    });
    return () => {
      cancelled = true;
    };
  }, []);

  const modeLabel = (mode: string): string => {
    switch (mode) {
      case "paper_enforced":
        return "Paper-enforced (narrows same-symbol conflicts to one candidate)";
      case "shadow":
        return "Shadow (evidence only — no trading effect)";
      default:
        return "Off (conflict policy not consulted)";
    }
  };

  return (
    <Panel
      title="Multi-strategy conflict policy"
      subtitle="Read-only. Paper-only, pre-decision conflict-resolution layer, ahead of runtime opportunity allocation — never live-capital authority, never AI-derived, never a strategy ranking."
    >
      {loading && <div className="unavailable-notice">Checking conflict policy status…</div>}
      {!loading && error && (
        <div className="unavailable-notice unavailable-critical">
          <strong>Conflict policy status unavailable:</strong> {error}
        </div>
      )}
      {!loading && !error && status && (
        <div className="metric-list">
          <div>
            <span>Mode (configured / effective)</span>
            <strong>
              {status.mode_configured} / {status.mode_effective}
            </strong>
          </div>
          <div>
            <span>Effective behavior</span>
            <strong>{modeLabel(status.mode_effective)}</strong>
          </div>
          {status.invalid_configuration && (
            <div className="unavailable-notice unavailable-critical">
              <strong>Invalid configuration:</strong> MQK_STRATEGY_CONFLICT_POLICY_MODE=
              {status.invalid_configuration} is not recognized — running as off.
            </div>
          )}
          {status.live_lock_applied && (
            <div className="unavailable-notice unavailable-critical">
              <strong>Live lock applied:</strong> deployment is not paper+Alpaca — conflict policy
              has zero effect regardless of configuration.
            </div>
          )}
          <div>
            <span>Approved for live</span>
            <strong>{status.approved_for_live ? "true (should never happen)" : "false"}</strong>
          </div>
          <div>
            <span>Truth state</span>
            <strong>{status.truth_state}</strong>
          </div>
          {status.truth_state === "db_unavailable" && (
            <div className="unavailable-notice">Durable evidence unavailable — no database connection.</div>
          )}
          {status.truth_state === "not_found" && (
            <div className="unavailable-notice">No run resolved for conflict policy status.</div>
          )}
          {status.truth_state === "query_failed" && (
            <div className="unavailable-notice unavailable-critical">
              Conflict policy query failed — durable evidence could not be read.
            </div>
          )}
          <div>
            <span>Latest plan</span>
            <strong>{status.latest_plan_id ?? "— (none this run)"}</strong>
          </div>
          {status.latest_plan_candidate_count !== null && (
            <div>
              <span>Candidates (selected / total)</span>
              <strong>
                {status.latest_plan_selected_count} / {status.latest_plan_candidate_count}
              </strong>
            </div>
          )}
          <div className="unavailable-notice">{status.current_runtime_limitation}</div>
        </div>
      )}
      {!loading && !error && plan && plan.truth_state === "active" && plan.plan && (
        <table className="data-table">
          <caption>Latest plan candidates ({plan.plan.mode})</caption>
          <thead>
            <tr>
              <th>Symbol</th>
              <th>Strategy</th>
              <th>Side</th>
              <th>Qty</th>
              <th>Current</th>
              <th>Proposed target</th>
              <th>Selected</th>
              <th>Disposition</th>
              <th>Reason</th>
            </tr>
          </thead>
          <tbody>
            {plan.candidates.map((c, i) => (
              <tr key={`${c.symbol}-${c.strategy_id}-${i}`}>
                <td>{c.symbol}</td>
                <td>{c.strategy_id}</td>
                <td>{c.side}</td>
                <td>{c.qty}</td>
                <td>{c.current_qty}</td>
                <td>{c.proposed_target_qty ?? "—"}</td>
                <td>{c.selected ? "yes" : "no"}</td>
                <td>{c.disposition}</td>
                <td>{c.reason_code}</td>
              </tr>
            ))}
          </tbody>
        </table>
      )}
    </Panel>
  );
}
