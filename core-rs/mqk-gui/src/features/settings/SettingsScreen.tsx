import { clearSavedDaemonUrl, defaultDaemonUrl, getSavedDaemonUrl, setSavedDaemonUrl } from "../../config";
import { Panel } from "../../components/common/Panel";
import { formatLabel } from "../../lib/format";
import { AssetCapabilityMatrixPanel } from "../system/AssetCapabilityMatrixPanel";
import { InstrumentRegistryV2SourcePanel } from "../system/InstrumentRegistryV2SourcePanel";
import { systemStatusScreenIncludes } from "../system/systemStatusSections";
import type { SystemModel } from "../system/types";

export function SettingsScreen({ model }: { model: SystemModel }) {
  const saved = getSavedDaemonUrl();
  const current = saved ?? defaultDaemonUrl();
  const session = model.sessionState;
  const profileOpen = session.session_profile_is_open;
  const supportedProfiles = session.supported_session_profiles ?? [];

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
      <Panel title="Session profile">
        <div className="metric-list">
          <div><span>Profile</span><strong>{formatLabel(session.session_profile ?? "unavailable")}</strong></div>
          <div><span>Authority</span><strong>{formatLabel(session.session_authority ?? "unavailable")}</strong></div>
          <div>
            <span>Open</span>
            <strong>{profileOpen == null ? "—" : profileOpen ? "Yes" : "No"}</strong>
          </div>
          <div><span>Reason</span><strong>{formatLabel(session.session_profile_reason_code ?? "unavailable")}</strong></div>
          <div><span>Message</span><strong>{session.session_profile_message ?? "—"}</strong></div>
          <div>
            <span>Supported profiles</span>
            <strong>{supportedProfiles.length > 0 ? supportedProfiles.map(formatLabel).join(", ") : "—"}</strong>
          </div>
        </div>
      </Panel>
      {systemStatusScreenIncludes("instrument-registry-v2-source") && <InstrumentRegistryV2SourcePanel />}
      {systemStatusScreenIncludes("asset-capability-matrix") && <AssetCapabilityMatrixPanel />}
    </div>
  );
}
