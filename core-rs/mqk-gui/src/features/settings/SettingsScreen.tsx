import { useState } from "react";
import { clearSavedDaemonUrl, getDaemonUrl, setSavedDaemonUrl } from "../../config";
import { getDesktopDaemonUrl } from "../../desktop/bootstrap";
import { Panel } from "../../components/common/Panel";
import { formatLabel } from "../../lib/format";
import { AssetCapabilityMatrixPanel } from "../system/AssetCapabilityMatrixPanel";
import { DynamicSelectionEvidencePanel } from "../system/DynamicSelectionEvidencePanel";
import { InstrumentRegistryV2SourcePanel } from "../system/InstrumentRegistryV2SourcePanel";
import { RuntimeOpportunityAllocationPanel } from "../system/RuntimeOpportunityAllocationPanel";
import { StrategyConflictPolicyPanel } from "../system/StrategyConflictPolicyPanel";
import { systemStatusScreenIncludes } from "../system/systemStatusSections";
import type { SystemModel } from "../system/types";

export function SettingsScreen({ model }: { model: SystemModel }) {
  const current = getDaemonUrl();
  const [draftUrl, setDraftUrl] = useState(current);
  const [endpointError, setEndpointError] = useState<string | null>(null);
  const launcherManaged = getDesktopDaemonUrl() !== null;
  const session = model.sessionState;
  const profileOpen = session.session_profile_is_open;
  const supportedProfiles = session.supported_session_profiles ?? [];

  const handleUseDefault = () => {
    clearSavedDaemonUrl();
    window.location.reload();
  };

  const handleApplyEndpoint = () => {
    if (launcherManaged) return;
    const result = setSavedDaemonUrl(draftUrl);
    if (!result.ok) {
      setEndpointError(result.error ?? "Invalid URL");
      return;
    }
    window.location.reload();
  };

  return (
    <div className="screen-grid">
      <Panel title="Daemon endpoint">
        <div className="settings-stack">
          <div className="setting-row"><span>Current endpoint</span><strong>{current}</strong></div>
          <form onSubmit={(event) => { event.preventDefault(); handleApplyEndpoint(); }}>
            <label>Daemon base URL <input value={draftUrl} disabled={launcherManaged} onChange={(event) => { setDraftUrl(event.target.value); setEndpointError(null); }} /></label>
            <div className="button-row">
              <button type="submit" className="action-button" disabled={launcherManaged}>Apply endpoint</button>
              <button type="button" className="action-button ghost" disabled={launcherManaged} onClick={handleUseDefault}>Use default</button>
            </div>
          </form>
          {launcherManaged && <p>Desktop endpoint is managed by the launcher. Restart with the configured launcher endpoint to change it.</p>}
          {endpointError && <div role="alert">{endpointError}</div>}
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
      {systemStatusScreenIncludes("runtime-opportunity-allocation") && <RuntimeOpportunityAllocationPanel />}
      {systemStatusScreenIncludes("strategy-conflict-policy") && <StrategyConflictPolicyPanel />}
      {systemStatusScreenIncludes("dynamic-selection-evidence") && <DynamicSelectionEvidencePanel />}
    </div>
  );
}
