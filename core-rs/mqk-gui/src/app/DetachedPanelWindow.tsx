// GUI-LAYOUT-03: the render path for a Tauri window opened by detaching a
// single Tier-2 panel out of a workstation. Tier-0 truth (GlobalStatusBar)
// is rendered unconditionally, exactly as in the main desk-role windows —
// detaching a panel never removes safety truth from view.

import { ScreenErrorBoundary } from "../components/common/ScreenErrorBoundary";
import { GlobalStatusBar } from "../components/status/GlobalStatusBar";
import { WorkspaceFrame } from "../components/layout/WorkspaceFrame";
import { SCREEN_REGISTRY } from "../features/screens/screenRegistry";
import { confirmAndRunOperatorAction } from "../features/system/runOperatorAction";
import { useOperatorModel } from "../features/system/useOperatorModel";
import type { DetachedPanelBootstrap } from "../features/workstation/detachedWindow";
import { reattachAndClose } from "../features/workstation/detachedWindow";

export function DetachedPanelWindow({ bootstrap }: { bootstrap: DetachedPanelBootstrap }) {
  const { model, refresh, selectTimeline, timelineLoading, runAction } = useOperatorModel();
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
                onClick={() => void reattachAndClose(bootstrap)}
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
            >
              <ScreenErrorBoundary screenKey={bootstrap.panelId}>
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
              </ScreenErrorBoundary>
            </WorkspaceFrame>
          </main>
        </div>
      </div>
    </div>
  );
}
