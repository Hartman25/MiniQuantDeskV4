import { useEffect, useState } from "react";
import { FieldSourceAuthority } from "../../components/common/FieldSourceAuthority";
import { Panel } from "../../components/common/Panel";
import { TruthStateNotice } from "../../components/common/TruthStateNotice";
import { fetchModeChangeGuidance } from "../system/api";
import { normalizeModeChangeGuidance } from "../system/legacy";
import { classifyFieldSource, type FieldEvidenceHints } from "../system/sourceAuthority";
import { panelTruthRenderState } from "../system/truthRendering";
import type { ModeChangeGuidanceResponse, OperatorActionDefinition, SystemModel } from "../system/types";

function levelLabel(level: OperatorActionDefinition["level"]): string {
  switch (level) {
    case 0:
      return "Level 0";
    case 1:
      return "Level 1";
    case 2:
      return "Level 2";
    case 3:
      return "Level 3";
    default:
      return "Level ?";
  }
}

const MODE_FIELD_HINTS: Record<"environment" | "runtime" | "liveRouting" | "generation" | "sourceState", FieldEvidenceHints> = {
  // daemon_mode / environment comes from system/status and config-fingerprint — both runtime memory.
  environment: { db: [], runtime: ["/system/status", "/system/config-fingerprint"], broker: [], placeholder: ["status", "configFingerprint"] },
  // runtime_status is pure daemon runtime state.
  runtime: { db: [], runtime: ["/system/status"], broker: [], placeholder: ["status"] },
  // live_routing_enabled is derived from system/status — runtime memory.
  liveRouting: { db: [], runtime: ["/system/status"], broker: [], placeholder: ["status"] },
  // generation_id comes from runtime-leadership — runtime memory, no DB backing in current arch.
  generation: { db: [], runtime: ["/system/runtime-leadership"], broker: [], placeholder: ["runtimeLeadership"] },
  sourceState: { db: [], runtime: [], broker: [], placeholder: ["all", "status", "runtimeLeadership"] },
};

export function OpsScreen({
  model,
  onRunAction,
}: {
  model: SystemModel;
  onRunAction: (action: OperatorActionDefinition) => void;
}) {
  const truthState = panelTruthRenderState(model, "ops");
  const [guidance, setGuidance] = useState<ModeChangeGuidanceResponse | null>(null);
  const [guidanceLoading, setGuidanceLoading] = useState(true);

  useEffect(() => {
    void fetchModeChangeGuidance().then((raw) => {
      setGuidance(normalizeModeChangeGuidance(raw));
      setGuidanceLoading(false);
    });
  }, []);

  // Hard-close on any compromised truth state: ops is the mode-change and action surface.
  // An operator must not be able to ARM, change mode, or execute actions under stale or
  // degraded system truth. Every non-null truth state is a hard stop here.
  if (truthState !== null) {
    return <TruthStateNotice state={truthState} />;
  }

  return (
    <div className="screen-grid desk-screen-grid">
      <Panel title="System mode transition" subtitle="Mode changes require a controlled daemon restart and configuration reload. This is not a casual runtime toggle.">
        <div className="mode-transition-panel">
          <div className="mode-transition-meta">
            <div>
              <span>Current mode</span>
              <strong>{model.status.environment}</strong>
              <FieldSourceAuthority
                fieldKey="ops-current-mode"
                authority={classifyFieldSource(model.dataSource, model.connected, MODE_FIELD_HINTS.environment)}
              />
            </div>
            <div>
              <span>Runtime</span>
              <strong>{model.status.runtime_status}</strong>
              <FieldSourceAuthority
                fieldKey="ops-runtime-status"
                authority={classifyFieldSource(model.dataSource, model.connected, MODE_FIELD_HINTS.runtime)}
              />
            </div>
            <div>
              <span>Live routing</span>
              <strong>{model.status.live_routing_enabled ? "enabled" : "disabled"}</strong>
              <FieldSourceAuthority
                fieldKey="ops-live-routing"
                authority={classifyFieldSource(model.dataSource, model.connected, MODE_FIELD_HINTS.liveRouting)}
              />
            </div>
            <div>
              <span>Generation</span>
              <strong>{model.runtimeLeadership.generation_id}</strong>
              <FieldSourceAuthority
                fieldKey="ops-generation"
                authority={classifyFieldSource(model.dataSource, model.connected, MODE_FIELD_HINTS.generation)}
              />
            </div>
            <div>
              <span>Source state</span>
              <strong>{model.dataSource.state}</strong>
              <FieldSourceAuthority
                fieldKey="ops-source-state"
                authority={classifyFieldSource(model.dataSource, model.connected, MODE_FIELD_HINTS.sourceState)}
              />
            </div>
          </div>
          {/* Mode-change buttons are disabled: mode transitions require a controlled daemon
              restart with configuration reload — no hot switching is permitted.
              Buttons remain visible so the operator knows the surface exists. */}
          {guidanceLoading ? (
            <p className="panel-notice">Loading mode-change guidance from daemon…</p>
          ) : guidance === null ? (
            <p className="panel-notice panel-notice-warn">
              Mode-change guidance unavailable — /api/v1/ops/mode-change-guidance not reachable.
              A controlled daemon restart with configuration reload is required for any mode change.
            </p>
          ) : (
            <p className="panel-notice panel-notice-warn">
              {guidance.transition_refused_reason}
            </p>
          )}
          {guidance !== null && (
            <div className="mode-toggle-row">
              {guidance.transition_verdicts.map((entry) => {
                const isActive = entry.verdict === "same_mode";
                const verdictLabel = isActive ? "current mode" : entry.verdict.replace(/_/g, " ");
                const isLive = entry.target_mode.startsWith("live");
                return (
                  <button
                    key={entry.target_mode}
                    type="button"
                    className={`mode-toggle ${isActive ? "is-active" : ""} ${isLive ? "is-live" : ""}`}
                    disabled
                    aria-disabled="true"
                    title={`${entry.verdict}: ${entry.reason}`}
                  >
                    <span>{entry.target_mode.toUpperCase()}</span>
                    <small>{verdictLabel}</small>
                  </button>
                );
              })}
            </div>
          )}
          {guidance !== null && guidance.preconditions.length > 0 && (
            <div>
              <strong className="check-list-heading">Required preconditions</strong>
              <ul className="check-list compact">
                {guidance.preconditions.map((p, i) => <li key={i}>{p}</li>)}
              </ul>
            </div>
          )}
          {guidance !== null && guidance.operator_next_steps.length > 0 && (
            <div>
              <strong className="check-list-heading">Operator next steps</strong>
              <ol className="check-list compact">
                {guidance.operator_next_steps.map((step, i) => <li key={i}>{step}</li>)}
              </ol>
            </div>
          )}
          {guidance !== null && (
            <div className="metric-list compact-list">
              <div><span>Parity evidence</span><strong>{guidance.parity_evidence_state}</strong></div>
              <div><span>Live trust complete</span><strong>{guidance.live_trust_complete === null ? "—" : String(guidance.live_trust_complete)}</strong></div>
            </div>
          )}
          {guidance !== null && (
            <div>
              <strong className="check-list-heading">Restart workflow</strong>
              <div className="metric-list compact-list">
                <div><span>State</span><strong>{guidance.restart_workflow.truth_state}</strong></div>
                <div>
                  <span>Pending intent</span>
                  <strong>{guidance.restart_workflow.pending_intent !== null ? "yes" : "none"}</strong>
                </div>
              </div>
              {guidance.restart_workflow.pending_intent !== null && (
                <div className="metric-list compact-list">
                  <div><span>From</span><strong>{guidance.restart_workflow.pending_intent.from_mode}</strong></div>
                  <div><span>To</span><strong>{guidance.restart_workflow.pending_intent.to_mode}</strong></div>
                  <div><span>Verdict</span><strong>{guidance.restart_workflow.pending_intent.transition_verdict}</strong></div>
                  <div><span>Initiated by</span><strong>{guidance.restart_workflow.pending_intent.initiated_by}</strong></div>
                  <div><span>At</span><strong>{guidance.restart_workflow.pending_intent.initiated_at_utc}</strong></div>
                  {guidance.restart_workflow.pending_intent.note && (
                    <div><span>Note</span><strong>{guidance.restart_workflow.pending_intent.note}</strong></div>
                  )}
                </div>
              )}
            </div>
          )}
          {guidance === null && !guidanceLoading && (
            <ul className="check-list compact">
              <li>Strategies should be disarmed before changing environment.</li>
              <li>Execution should be disabled before restart.</li>
              <li>Transport queues should be reviewed for backlog or orphaned claims.</li>
              <li>Runtime generation and policy fingerprint will advance after successful transition.</li>
            </ul>
          )}
        </div>
      </Panel>

      <Panel title="Operator action catalog" subtitle="Every dangerous action should stay explicit, confirmed, and audited.">
        {model.actionCatalog.length === 0 ? (
          <p className="empty-state">Action catalog unavailable: backend truth did not provide operator actions.</p>
        ) : (
          <div className="action-catalog-grid">
            {model.actionCatalog.map((action) => (
              <div key={action.action_key} className={`action-card action-level-${action.level}`}>
                <div className="action-card-head">
                  <strong>{action.label}</strong>
                  <span>{levelLabel(action.level)}</span>
                </div>
                <p>{action.description}</p>
                <div className="button-row">
                  <button className="action-button" disabled={action.disabled} onClick={() => onRunAction(action)}>
                    Run
                  </button>
                </div>
              </div>
            ))}
          </div>
        )}
      </Panel>
    </div>
  );
}
