// GUI-LAYOUT-03: the render path for a Tauri window opened by detaching a
// single Tier-2 panel out of a workstation. Tier-0 truth (GlobalStatusBar)
// is rendered unconditionally, exactly as in the main desk-role windows —
// detaching a panel never removes safety truth from view.

import { useEffect, useState } from "react";
import { ScreenErrorBoundary } from "../components/common/ScreenErrorBoundary";
import { GlobalStatusBar } from "../components/status/GlobalStatusBar";
import { WorkspaceFrame } from "../components/layout/WorkspaceFrame";
import { SCREEN_REGISTRY } from "../features/screens/screenRegistry";
import { confirmAndRunOperatorAction } from "../features/system/runOperatorAction";
import { useOperatorModel } from "../features/system/useOperatorModel";
import { useWorkspaceContext, WorkspaceScopeProvider } from "../features/workspace/WorkspaceContext.tsx";
import { initialPanelLinkState, pinPanel, resolvePanelIdentity, unpinPanel, type PanelLinkState } from "../features/workspace/workspaceModel.ts";
import type { DetachedPanelBootstrap } from "../features/workstation/detachedWindow";
import { reattachAndClose } from "../features/workstation/detachedWindow";
import { recoverCurrentWindowIfStranded } from "../features/workstation/windowRecovery";

export function DetachedPanelWindow({ bootstrap }: { bootstrap: DetachedPanelBootstrap }) {
  const { model, refresh, selectTimeline, timelineLoading, runAction } = useOperatorModel();
  const workspace = useWorkspaceContext();

  // GUI-LAYOUT-06: a detached window is exactly as susceptible to monitor
  // loss as any other — validate its geometry once at startup too.
  useEffect(() => {
    void recoverCurrentWindowIfStranded();
  }, []);
  // GUI-LAYOUT-05: seeded pinned if the panel was pinned at detach time —
  // same PanelLinkState model PanelHost uses, so pin/unpin behaves
  // identically whether the panel is docked or detached.
  const [linkState, setLinkState] = useState<PanelLinkState>(() =>
    bootstrap.pinnedIdentity ? pinPanel(bootstrap.pinnedIdentity) : initialPanelLinkState(),
  );
  const resolved = resolvePanelIdentity(workspace.linked, linkState);
  const togglePin = () => setLinkState((prev) => (prev.pinned ? unpinPanel() : pinPanel(resolved)));
  const screen = SCREEN_REGISTRY[bootstrap.panelId];

  return (
    <div className="app-shell detached-panel-window">
      <div className="main-shell">
        <GlobalStatusBar status={model.status} dataSource={model.dataSource} />

        <div className="workspace-layout">
          <main className="workspace-column">
            <div className="workspace-toolbar panel">
              <div>
                <div className="eyebrow">Detached panel</div>
                <h2>{screen.title}</h2>
              </div>
              <button
                type="button"
                className="action-button ghost"
                onClick={() => void reattachAndClose(bootstrap, linkState)}
                title="Move this panel back to its originating window"
              >
                Reattach
              </button>
            </div>

            <WorkspaceFrame
              title={screen.title}
              description={screen.description}
              panelKey={bootstrap.panelId}
              authority={model.panelSources[bootstrap.panelId]}
              pinState={{ pinned: linkState.pinned, onToggle: togglePin }}
            >
              <ScreenErrorBoundary screenKey={bootstrap.panelId}>
                <WorkspaceScopeProvider value={{ ...workspace, linked: resolved }}>
                  {screen.render({
                    model,
                    selectTimeline: (internalOrderId) => void selectTimeline(internalOrderId),
                    timelineLoading,
                    runAction: (action) =>
                      void confirmAndRunOperatorAction(action, {
                        status: model.status,
                        runAction,
                        refresh,
                        targetScope: bootstrap.panelId,
                      }),
                  })}
                </WorkspaceScopeProvider>
              </ScreenErrorBoundary>
            </WorkspaceFrame>
          </main>
        </div>
      </div>
    </div>
  );
}
