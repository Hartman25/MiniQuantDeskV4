import { useEffect, useState } from "react";
import { Panel } from "../../components/common/Panel";
import { getDynamicSelectionPlanDetail, getDynamicSelectionStatus } from "./api";
import type { DynamicSelectionPlanDetail, DynamicSelectionStatus } from "./types/dynamicSelectionEvidence";

// DYNAMIC-STRATEGY-SYMBOL-SELECTION-01 Phase 7C Part 5: read-only durable
// dynamic-selection evidence panel.
//
// Read-only: no mode/live/start/order control anywhere in this component.
// evidence_validation_state governs what renders as trustworthy — an
// unrecognized/malformed response (parseDynamicSelectionStatus /
// parseDynamicSelectionPlanDetail) always downgrades to an unavailable
// sentinel before it reaches this component. This panel must never show
// green/ready for missing or invalid evidence.
export function DynamicSelectionEvidencePanel() {
  const [status, setStatus] = useState<DynamicSelectionStatus | null>(null);
  const [plan, setPlan] = useState<DynamicSelectionPlanDetail | null>(null);
  const [error, setError] = useState<string | null>(null);
  const [loading, setLoading] = useState(true);

  useEffect(() => {
    let cancelled = false;
    setLoading(true);
    void getDynamicSelectionStatus().then(async (statusResult) => {
      if (cancelled) return;
      if (!statusResult.ok) {
        setError(statusResult.error ?? "Dynamic-selection status unavailable.");
        setLoading(false);
        return;
      }
      setError(null);
      setStatus(statusResult.data);

      if (statusResult.data.committed_plan_id) {
        const planResult = await getDynamicSelectionPlanDetail(statusResult.data.committed_plan_id);
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

  const lifecycleLabel = (lifecycle: string): string => {
    switch (lifecycle) {
      case "running":
        return "Running";
      case "starting":
        return "Starting";
      case "degraded":
        return "Degraded";
      default:
        return "Idle";
    }
  };

  const dispositionLabel = (disposition: string | null): string => {
    switch (disposition) {
      case "paper_enforced_allowed":
        return "Paper-enforced — allowed (selected-host dispatch authoritative)";
      case "paper_enforced_refused":
        return "Paper-enforced — refused (run start blocked)";
      case "shadow_allowed":
        return "Shadow — allowed (evidence only, no dispatch effect)";
      case "shadow_invalid":
        return "Shadow — invalid (evidence only, no dispatch effect)";
      default:
        return "Off (no committed truth)";
    }
  };

  const validationLabel = (state: string | null): string => {
    switch (state) {
      case "valid":
        return "Valid";
      case "missing":
        return "Missing";
      case null:
        return "— (no committed plan)";
      default:
        return `Invalid (${state})`;
    }
  };

  const isValidationHealthy = status?.evidence_validation_state === "valid";

  return (
    <Panel
      title="Dynamic strategy/symbol selection evidence"
      subtitle="Read-only. Durable plan evidence for paper-only dynamic strategy/symbol selection — never live-capital authority, never a mutation control."
    >
      {loading && <div className="unavailable-notice">Checking dynamic-selection status…</div>}
      {!loading && error && (
        <div className="unavailable-notice unavailable-critical">
          <strong>Dynamic-selection status unavailable:</strong> {error}
        </div>
      )}
      {!loading && !error && status && (
        <div className="metric-list">
          <div>
            <span>Lifecycle</span>
            <strong>{lifecycleLabel(status.lifecycle)}</strong>
          </div>
          <div>
            <span>Active run</span>
            <strong>{status.active_run_id ?? "— (none)"}</strong>
          </div>
          <div>
            <span>Mode (committed / preview)</span>
            <strong>
              {status.committed_effective_mode ?? "—"} / {status.preview_effective_mode}
            </strong>
          </div>
          <div>
            <span>Committed disposition</span>
            <strong>{dispositionLabel(status.disposition)}</strong>
          </div>
          <div>
            <span>Approved for live</span>
            <strong>{status.approved_for_live ? "true (should never happen)" : "false"}</strong>
          </div>
          {status.committed_live_lock_applied && (
            <div className="unavailable-notice unavailable-critical">
              <strong>Live lock applied:</strong> deployment is not paper+Alpaca — the committed
              run's dynamic selection has zero dispatch effect regardless of configuration.
            </div>
          )}
          <div>
            <span>Committed plan</span>
            <strong>{status.committed_plan_id ?? "— (none committed)"}</strong>
          </div>
          <div>
            <span>Evidence persisted</span>
            <strong>{status.evidence_persisted ? "yes" : "no"}</strong>
          </div>
          <div>
            <span>Evidence validation</span>
            <strong className={isValidationHealthy ? undefined : "unavailable-critical"}>
              {validationLabel(status.evidence_validation_state)}
            </strong>
          </div>
          {status.evidence_validation_detail && (
            <div className="unavailable-notice unavailable-critical">
              {status.evidence_validation_detail}
            </div>
          )}
          {status.validation_blockers.length > 0 && (
            <div className="unavailable-notice unavailable-critical">
              <strong>Validation blockers:</strong>
              <ul>
                {status.validation_blockers.map((b, i) => (
                  <li key={i}>{b}</li>
                ))}
              </ul>
            </div>
          )}
          <div>
            <span>Host pool</span>
            <strong>
              {status.host_pool_present ? `present (${status.host_pool_count} hosts)` : "absent"}
            </strong>
          </div>
          <div>
            <span>Source</span>
            <strong>
              {status.committed_source_kind ?? "—"}
              {status.committed_source_identity ? ` (${status.committed_source_identity})` : ""}
            </strong>
          </div>
        </div>
      )}
      {!loading && !error && status && (status.selected.length > 0 || status.refused.length > 0) && (
        <table className="data-table">
          <caption>Committed plan symbol dispositions</caption>
          <thead>
            <tr>
              <th>Symbol</th>
              <th>Selected strategy</th>
              <th>Disposition</th>
              <th>Reason</th>
            </tr>
          </thead>
          <tbody>
            {[...status.selected, ...status.refused].map((s, i) => (
              <tr key={`${s.symbol}-${i}`}>
                <td>{s.symbol}</td>
                <td>{s.selected_strategy_id ?? "—"}</td>
                <td>{s.disposition}</td>
                <td>{s.reason_code}</td>
              </tr>
            ))}
          </tbody>
        </table>
      )}
      {!loading && !error && plan && plan.found && plan.candidates.length > 0 && (
        <table className="data-table">
          <caption>Committed plan candidates</caption>
          <thead>
            <tr>
              <th>Symbol</th>
              <th>Strategy</th>
              <th>Timeframe (s)</th>
              <th>Score</th>
              <th>Rank</th>
              <th>Selected</th>
              <th>Disposition</th>
              <th>Reason</th>
            </tr>
          </thead>
          <tbody>
            {plan.candidates.map((c, i) => (
              <tr key={`${c.symbol}-${c.strategy_id}-${c.timeframe_secs}-${i}`}>
                <td>{c.symbol}</td>
                <td>{c.strategy_id}</td>
                <td>{c.timeframe_secs}</td>
                <td>{c.canonical_score_decimal ?? "—"}</td>
                <td>{c.scanner_rank ?? "—"}</td>
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
