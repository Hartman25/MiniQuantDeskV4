import type { DeskMode, DeskRole } from "../../app/shellTypes";

type WorkspaceToolbarProps = {
  loading: boolean;
  connected: boolean;
  lastUpdatedAtLabel: string;
  screenTitle: string;
  deskRole: DeskRole;
  deskMode: DeskMode;
  onDeskModeChange: (mode: DeskMode) => void;
  onRefresh: () => void;
  rightDrawerOpen: boolean;
  onToggleRightDrawer: () => void;
  onResetLayout: () => void;
};

export function WorkspaceToolbar({
  loading,
  connected,
  lastUpdatedAtLabel,
  screenTitle,
  deskRole,
  deskMode,
  onDeskModeChange,
  onRefresh,
  rightDrawerOpen,
  onToggleRightDrawer,
  onResetLayout,
}: WorkspaceToolbarProps) {
  return (
    <div className="workspace-toolbar panel">
      <div>
        <div className="eyebrow">Operator Session</div>
        <h2>{screenTitle}</h2>
      </div>

      <div className="toolbar-metrics">
        <span>{loading ? "Loading…" : connected ? "Connected" : "Disconnected"}</span>
        <span>Last refresh: {lastUpdatedAtLabel}</span>

        {deskRole === "control" ? (
          <div className="desk-mode-toggle" role="group" aria-label="Desk mode">
            <button
              type="button"
              className={`action-button ghost ${deskMode === "single" ? "is-selected" : ""}`}
              onClick={() => onDeskModeChange("single")}
            >
              1 window
            </button>
            <button
              type="button"
              className={`action-button ghost ${deskMode === "two" ? "is-selected" : ""}`}
              onClick={() => onDeskModeChange("two")}
            >
              2 monitors
            </button>
            <button
              type="button"
              className={`action-button ghost ${deskMode === "three" ? "is-selected" : ""}`}
              onClick={() => onDeskModeChange("three")}
            >
              3 monitors
            </button>
          </div>
        ) : null}

        <button className="action-button ghost" onClick={onRefresh}>
          Refresh
        </button>

        <button
          type="button"
          className={`action-button ghost ${rightDrawerOpen ? "is-selected" : ""}`}
          onClick={onToggleRightDrawer}
          aria-pressed={rightDrawerOpen}
        >
          Context
        </button>

        <button type="button" className="action-button ghost" onClick={onResetLayout}>
          Reset layout
        </button>
      </div>
    </div>
  );
}
