import { FieldSourceAuthority } from "../../components/common/FieldSourceAuthority";
import { Panel } from "../../components/common/Panel";
import { TruthStateNotice } from "../../components/common/TruthStateNotice";
import { classifyFieldSource, type FieldEvidenceHints } from "../system/sourceAuthority";
import { panelTruthRenderState } from "../system/truthRendering";
import type { EnvironmentMode, OperatorActionDefinition, SystemModel } from "../system/types";

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

const TARGET_MODES: EnvironmentMode[] = ["backtest", "paper", "live"];

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
  onChangeMode,
}: {
  model: SystemModel;
  onRunAction: (action: OperatorActionDefinition) => void;
  onChangeMode: (targetMode: EnvironmentMode) => void;
}) {
  const truthState = panelTruthRenderState(model, "ops");

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
          {/* Mode-change buttons are disabled: /api/v1/ops/change-mode is not yet mounted on the
              daemon. Mode transitions require a controlled restart with configuration reload — this
              cannot be done via API in the current architecture. Buttons remain visible so the
              operator knows the surface exists but cannot be misled into believing a click works. */}
          <p className="panel-notice panel-notice-warn">
            Mode transition is not yet available via the console. A controlled daemon restart with configuration reload is required. This route is not mounted.
          </p>
          <div className="mode-toggle-row">
            {TARGET_MODES.map((mode) => (
              <button
                key={mode}
                type="button"
                className={`mode-toggle ${model.status.environment === mode ? "is-active" : ""} ${mode === "live" ? "is-live" : ""}`}
                disabled
                aria-disabled="true"
                title="Mode transition not available — daemon restart required"
              >
                <span>{mode.toUpperCase()}</span>
                <small>{mode === "live" ? "controlled restart required" : "safe environment transition"}</small>
              </button>
            ))}
          </div>
          <ul className="check-list compact">
            <li>Strategies should be disarmed before changing environment.</li>
            <li>Execution should be disabled before restart.</li>
            <li>Transport queues should be reviewed for backlog or orphaned claims.</li>
            <li>Runtime generation and policy fingerprint will advance after successful transition.</li>
          </ul>
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
