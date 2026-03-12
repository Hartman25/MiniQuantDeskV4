import { Panel } from "../../components/common/Panel";
import { SourceAuthorityBadge } from "../../components/common/SourceAuthorityBadge";
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

export function OpsScreen({
  model,
  onRunAction,
  onChangeMode,
}: {
  model: SystemModel;
  onRunAction: (action: OperatorActionDefinition) => void;
  onChangeMode: (targetMode: EnvironmentMode) => void;
}) {
  return (
    <div className="screen-grid desk-screen-grid">
      <SourceAuthorityBadge detail={model.panelSources.ops} />
      <Panel title="System mode transition" subtitle="Mode changes require a controlled daemon restart and configuration reload. This is not a casual runtime toggle.">
        <div className="mode-transition-panel">
          <div className="mode-transition-meta">
            <div><span>Current mode</span><strong>{model.status.environment}</strong></div>
            <div><span>Runtime</span><strong>{model.status.runtime_status}</strong></div>
            <div><span>Live routing</span><strong>{model.status.live_routing_enabled ? "enabled" : "disabled"}</strong></div>
            <div><span>Generation</span><strong>{model.runtimeLeadership.generation_id}</strong></div>
            <div><span>Source state</span><strong>{model.dataSource.state}</strong></div>
          </div>
          <div className="mode-toggle-row">
            {TARGET_MODES.map((mode) => (
              <button
                key={mode}
                type="button"
                className={`mode-toggle ${model.status.environment === mode ? "is-active" : ""} ${mode === "live" ? "is-live" : ""}`}
                onClick={() => onChangeMode(mode)}
                disabled={model.status.environment === mode}
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
      </Panel>
    </div>
  );
}
