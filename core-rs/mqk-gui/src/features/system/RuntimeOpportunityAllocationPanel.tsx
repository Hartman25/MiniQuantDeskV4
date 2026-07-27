import { useEffect, useState } from "react";
import { Panel } from "../../components/common/Panel";
import { getAllocationPlanDetail, getAllocationStatus } from "./api";
import type { AllocationPlanDetail, AllocationStatus } from "./types/allocation";

// RUNTIME-OPPORTUNITY-ALLOCATION-01 Phase H: read-only allocation panel.
//
// Read-only: no mode switch, no mutation control anywhere in this component.
// truth_state governs what renders — an unrecognized/malformed response
// (parseAllocationStatus/parseAllocationPlanDetail) always downgrades to an
// unavailable sentinel before it reaches this component, so this file only
// has to render the closed vocabulary those parsers guarantee.
export function RuntimeOpportunityAllocationPanel() {
  const [status, setStatus] = useState<AllocationStatus | null>(null);
  const [plan, setPlan] = useState<AllocationPlanDetail | null>(null);
  const [error, setError] = useState<string | null>(null);
  const [loading, setLoading] = useState(true);

  useEffect(() => {
    let cancelled = false;
    setLoading(true);
    void getAllocationStatus().then(async (statusResult) => {
      if (cancelled) return;
      if (!statusResult.ok) {
        setError(statusResult.error ?? "Allocation status unavailable.");
        setLoading(false);
        return;
      }
      setError(null);
      setStatus(statusResult.data);

      if (statusResult.data.latest_plan_id) {
        const planResult = await getAllocationPlanDetail(statusResult.data.latest_plan_id);
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

  const influenceLabel = (influence: string): string => {
    switch (influence) {
      case "paper_enforced":
        return "Paper-enforced (clamping/refusing buys)";
      case "shadow":
        return "Shadow (evidence only — no trading effect)";
      default:
        return "None (allocator not consulted)";
    }
  };

  return (
    <Panel
      title="Runtime opportunity allocation"
      subtitle="Read-only. Paper-only, long-only, pre-decision buy constraint layer — never live-capital authority, never AI-derived."
    >
      {loading && <div className="unavailable-notice">Checking runtime opportunity allocation status…</div>}
      {!loading && error && (
        <div className="unavailable-notice unavailable-critical">
          <strong>Allocation status unavailable:</strong> {error}
        </div>
      )}
      {!loading && !error && status && (
        <div className="metric-list">
          <div>
            <span>Runtime influence</span>
            <strong>{influenceLabel(status.runtime_influence)}</strong>
          </div>
          <div>
            <span>Mode (configured / effective)</span>
            <strong>
              {status.mode_configured} / {status.mode_effective}
            </strong>
          </div>
          {status.invalid_configuration && (
            <div className="unavailable-notice unavailable-critical">
              <strong>Invalid configuration:</strong> MQK_RUNTIME_OPPORTUNITY_ALLOCATION_MODE=
              {status.invalid_configuration} is not recognized — running as off.
            </div>
          )}
          {status.live_lock_applied && (
            <div className="unavailable-notice unavailable-critical">
              <strong>Live lock applied:</strong> deployment is not paper+Alpaca — allocation has zero effect
              regardless of configuration.
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
            <div className="unavailable-notice">No run resolved for allocation status.</div>
          )}
          {status.truth_state === "query_failed" && (
            <div className="unavailable-notice unavailable-critical">
              Allocation query failed — durable evidence could not be read.
            </div>
          )}
          <div>
            <span>Latest plan</span>
            <strong>{status.latest_plan_id ?? "— (none this run)"}</strong>
          </div>
          {status.latest_plan_candidate_count !== null && (
            <div>
              <span>Candidates (allowed / total)</span>
              <strong>
                {status.latest_plan_allowed_count} / {status.latest_plan_candidate_count}
              </strong>
            </div>
          )}
        </div>
      )}
      {!loading && !error && plan && plan.truth_state === "active" && plan.plan && (
        <table className="data-table">
          <caption>Latest plan candidates ({plan.plan.mode})</caption>
          <thead>
            <tr>
              <th>Symbol</th>
              <th>Strategy</th>
              <th>Score</th>
              <th>Weight</th>
              <th>Current</th>
              <th>Strategy target</th>
              <th>Final target</th>
              <th>Disposition</th>
              <th>Reason</th>
            </tr>
          </thead>
          <tbody>
            {plan.candidates.map((c) => (
              <tr key={c.symbol}>
                <td>{c.symbol}</td>
                <td>{c.strategy_id}</td>
                <td>{c.input_score.toFixed(6)}</td>
                <td>{c.target_weight.toFixed(6)}</td>
                <td>{c.current_qty}</td>
                <td>{c.strategy_target_qty}</td>
                <td>{c.final_target_qty}</td>
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
