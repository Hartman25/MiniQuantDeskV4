import { clearSavedDaemonUrl, defaultDaemonUrl, getSavedDaemonUrl, setSavedDaemonUrl } from "../../config";
import { Panel } from "../../components/common/Panel";
import { formatLabel } from "../../lib/format";
import type { SystemModel } from "../system/types";

export function SettingsScreen({ model }: { model: SystemModel }) {
  const saved = getSavedDaemonUrl();
  const current = saved ?? defaultDaemonUrl();

  const handleUseDefault = () => {
    clearSavedDaemonUrl();
    window.location.reload();
  };

  const handlePrompt = () => {
    const next = window.prompt("Enter daemon base URL", current);
    if (!next) return;
    const result = setSavedDaemonUrl(next);
    if (!result.ok) {
      window.alert(result.error ?? "Invalid URL");
      return;
    }
    window.location.reload();
  };

  return (
    <div className="screen-grid">
      <Panel title="Daemon endpoint">
        <div className="settings-stack">
          <div className="setting-row"><span>Current endpoint</span><strong>{current}</strong></div>
          <div className="button-row">
            <button type="button" className="action-button" onClick={handlePrompt}>Change endpoint</button>
            <button type="button" className="action-button ghost" onClick={handleUseDefault}>Use default</button>
          </div>
        </div>
      </Panel>
      <Panel title="Operations metadata">
        <div className="metric-list">
          <div><span>Build version</span><strong>{model.metadata.build_version}</strong></div>
          <div><span>API version</span><strong>{model.metadata.api_version}</strong></div>
          <div><span>Broker adapter</span><strong>{model.metadata.broker_adapter}</strong></div>
          <div><span>Endpoint status</span><strong>{formatLabel(model.metadata.endpoint_status)}</strong></div>
          <div><span>Environment</span><strong>{model.status.environment}</strong></div>
          <div><span>Config profile</span><strong>{model.status.config_profile ?? "—"}</strong></div>
        </div>
      </Panel>
    </div>
  );
}
