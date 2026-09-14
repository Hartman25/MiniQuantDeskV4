import { useEffect, useMemo, useRef, useState } from "react";
import {
  WebviewWindow,
  getAllWebviewWindows,
  getCurrentWebviewWindow,
} from "@tauri-apps/api/webviewWindow";
import { getDaemonUrl } from "../config";
import { ActionReceiptBanner } from "../components/common/ActionReceiptBanner";
import { BottomEventRail } from "../components/layout/BottomEventRail";
import { LeftCommandRail } from "../components/layout/LeftCommandRail";
import { RightOpsRail } from "../components/layout/RightOpsRail";
import { RoleCommandStrip } from "../components/layout/RoleCommandStrip";
import { WorkspaceContextStrip } from "../components/layout/WorkspaceContextStrip";
import { WorkspaceToolbar } from "../components/layout/WorkspaceToolbar";
import { PreflightGate } from "../components/preflight/PreflightGate";
import { GlobalStatusBar } from "../components/status/GlobalStatusBar";
import { ROLE_SCREENS, SCREEN_REGISTRY, type ScreenKey } from "../features/screens/screenRegistry";
import { useOperatorModel } from "../features/system/useOperatorModel";
import type { OperatorActionDefinition } from "../features/system/types";
import { confirmAndRunOperatorAction } from "../features/system/runOperatorAction";
import { Workstation, type WorkstationHandle } from "../features/workstation/Workstation";
import { readDetachedPanelBootstrap } from "../features/workstation/detachedWindow";
import { DetachedPanelWindow } from "./DetachedPanelWindow";
import { formatDateTime } from "../lib/format";
import type { DeskMode, DeskRole } from "./shellTypes";

const DESK_MODE_STORAGE_KEY = "mqd.desktop.deskMode";
const LEFT_RAIL_COLLAPSED_STORAGE_KEY = "mqd.workstation.leftRailCollapsed";
const RIGHT_DRAWER_OPEN_STORAGE_KEY = "mqd.workstation.rightDrawerOpen";

function detectDeskRole(): DeskRole {
  try {
    const current = getCurrentWebviewWindow();
    const label = current.label;

    if (label === "execution") return "execution";
    if (label === "oversight") return "oversight";
    return "control";
  } catch {
    return "control";
  }
}

// Derives from ROLE_SCREENS so secondary-window defaults stay in sync with the
// curated role-screen truth in screenRegistry.tsx (diagnostics[0] == "audit").
function defaultScreenForRole(role: DeskRole): ScreenKey {
  switch (role) {
    case "execution":
      return ROLE_SCREENS.execution[0];
    case "oversight":
      return ROLE_SCREENS.oversight[0];
    case "control":
    default:
      return "dashboard";
  }
}

async function getWindowByLabel(label: "execution" | "oversight") {
  const windows = await getAllWebviewWindows();
  return windows.find((w) => w.label === label) ?? null;
}

async function ensureWindow(
  label: "execution" | "oversight",
  title: string,
  width: number,
  height: number,
) {
  const existing = await getWindowByLabel(label);

  if (existing) {
    console.log(`${label} window already exists`);
    await existing.show();
    await existing.setFocus();
    return;
  }

  console.log(`Creating ${label} window`);

  const win = new WebviewWindow(label, {
    title,
    url: "index.html",
    width,
    height,
    minWidth: 1100,
    minHeight: 700,
    resizable: true,
    visible: true,
  });

  win.once("tauri://created", async () => {
    console.log(`Created ${label} window`);
    await win.setFocus();
  });

  win.once("tauri://error", (e) => {
    console.error(`Failed to create ${label} window`, e);
  });
}

async function closeWindow(label: "execution" | "oversight") {
  const existing = await getWindowByLabel(label);

  if (existing) {
    console.log(`Closing ${label} window`);
    await existing.close();
  } else {
    console.log(`No ${label} window found to close`);
  }
}

async function applyDeskMode(mode: DeskMode) {
  console.log("Applying desk mode:", mode);

  if (mode === "single") {
    await closeWindow("execution");
    await closeWindow("oversight");
    console.log("Closed execution + oversight");
    return;
  }

  if (mode === "two") {
    await ensureWindow("execution", "Veritas Ledger — Execution", 1600, 1000);
    await closeWindow("oversight");
    console.log("Ensured execution, closed oversight");
    return;
  }

  if (mode === "three") {
    await ensureWindow("execution", "Veritas Ledger — Execution", 1600, 1000);
    await ensureWindow("oversight", "Veritas Ledger — Oversight", 1500, 960);
    console.log("Ensured execution + oversight");
    return;
  }
}

export function AppShell() {
  // GUI-LAYOUT-03: a window opened by detaching a single panel renders a
  // minimal single-panel shell instead of the full desk-role workstation.
  // Computed once from the immutable window.location.search this window was
  // created with — fails closed to the normal path on any malformed value.
  const detachedBootstrap = useMemo(() => readDetachedPanelBootstrap(window.location.search), []);
  if (detachedBootstrap) {
    return <DetachedPanelWindow bootstrap={detachedBootstrap} />;
  }

  return <ControlWorkstationShell />;
}

function ControlWorkstationShell() {
  const deskRole = useMemo(() => detectDeskRole(), []);
  const [deskMode, setDeskMode] = useState<DeskMode>("single");
  const [activeScreen, setActiveScreen] = useState<ScreenKey>(defaultScreenForRole(deskRole));
  const [leftRailCollapsed, setLeftRailCollapsed] = useState(false);
  const [rightDrawerOpen, setRightDrawerOpen] = useState(false);
  const workstationRef = useRef<WorkstationHandle>(null);

  const {
    model,
    loading,
    refresh,
    selectTimeline,
    timelineLoading,
    actionReceipt,
    runAction,
  } = useOperatorModel();

  const screen = SCREEN_REGISTRY[activeScreen];

  useEffect(() => {
    const stored = window.localStorage.getItem(DESK_MODE_STORAGE_KEY);
    if (stored === "single" || stored === "two" || stored === "three") {
      setDeskMode(stored);
    }
    setLeftRailCollapsed(window.localStorage.getItem(LEFT_RAIL_COLLAPSED_STORAGE_KEY) === "true");
    setRightDrawerOpen(window.localStorage.getItem(RIGHT_DRAWER_OPEN_STORAGE_KEY) === "true");
  }, []);

  useEffect(() => {
    window.localStorage.setItem(DESK_MODE_STORAGE_KEY, deskMode);
  }, [deskMode]);

  useEffect(() => {
    window.localStorage.setItem(LEFT_RAIL_COLLAPSED_STORAGE_KEY, String(leftRailCollapsed));
  }, [leftRailCollapsed]);

  useEffect(() => {
    window.localStorage.setItem(RIGHT_DRAWER_OPEN_STORAGE_KEY, String(rightDrawerOpen));
  }, [rightDrawerOpen]);

  const handleDeskModeChange = async (mode: DeskMode) => {
    setDeskMode(mode);
    window.localStorage.setItem(DESK_MODE_STORAGE_KEY, mode);

    if (deskRole !== "control") return;

    try {
      await applyDeskMode(mode);
      console.log(`Desk mode applied: ${mode}`);
    } catch (error) {
      console.error("Failed to apply desk mode:", error);
    }
  };


  // Opens a Tier-2 panel in the workstation (focusing it if already open —
  // never a second instance) and mirrors it as the "active screen" for
  // toolbar title, PreflightGate gating, and action target_scope.
  const openPanel = (key: ScreenKey) => {
    setActiveScreen(key);
    workstationRef.current?.openPanel(key);
  };

  const handleRunAction = (action: OperatorActionDefinition) =>
    confirmAndRunOperatorAction(action, { status: model.status, runAction, refresh, targetScope: activeScreen });

  const showLeftRail = deskRole === "control";
  const showBottomRail = deskRole !== "oversight";

  // Boot card: shown during the initial fetch (before first daemon response).
  // Replaced by the full layout once loading=false, whether connected or not.
  if (loading) {
    return (
      <div className="app-shell-boot">
        <div className="daemon-boot-card panel">
          <div className="eyebrow">Veritas Ledger</div>
          <h2 className="boot-title">Connecting to daemon&hellip;</h2>
          <p className="boot-detail">
            Waiting for first response from the daemon API. If this persists, verify the daemon process is running.
          </p>
          <p className="boot-endpoint">{getDaemonUrl()}</p>
        </div>
      </div>
    );
  }

  return (
    <div className={`app-shell desk-mode-${deskMode} desk-role-${deskRole} ${leftRailCollapsed ? "left-rail-collapsed" : ""}`}>
      {showLeftRail ? (
        <LeftCommandRail
          activeScreen={activeScreen}
          onSelect={openPanel}
          collapsed={leftRailCollapsed}
          onToggleCollapsed={() => setLeftRailCollapsed((v) => !v)}
        />
      ) : null}

      <div className="main-shell">
        <GlobalStatusBar status={model.status} dataSource={model.dataSource} />

        <div className="workspace-layout">
          <main className="workspace-column">
            <WorkspaceToolbar
              loading={loading}
              connected={model.connected}
              lastUpdatedAtLabel={formatDateTime(model.lastUpdatedAt)}
              screenTitle={screen.title}
              deskRole={deskRole}
              deskMode={deskMode}
              onDeskModeChange={(mode) => void handleDeskModeChange(mode)}
              onRefresh={() => void refresh()}
              rightDrawerOpen={rightDrawerOpen}
              onToggleRightDrawer={() => setRightDrawerOpen((v) => !v)}
              onResetLayout={() => workstationRef.current?.resetLayout()}
            />

            <WorkspaceContextStrip />

            {(deskRole === "execution" || deskRole === "oversight") ? (
              <RoleCommandStrip
                deskRole={deskRole}
                activeScreen={activeScreen}
                onSelect={openPanel}
              />
            ) : null}

            <ActionReceiptBanner receipt={actionReceipt} />
            {!model.connected && (
              <div className="daemon-disconnected-notice" role="status" aria-live="polite">
                <strong className="notice-label">Daemon unreachable</strong>
                <span className="notice-detail">
                  {model.dataSource.message ?? "No connection to daemon established."}
                </span>
                <span className="notice-endpoint">{getDaemonUrl()}</span>
              </div>
            )}
            {activeScreen === "dashboard" && (
              <PreflightGate preflight={model.preflight} runtimeStatus={model.status.runtime_status} />
            )}

            <Workstation
              ref={workstationRef}
              storageKey={`mqd.workstation.layout.${deskRole}`}
              initialPanelId={activeScreen}
              ctx={{
                model,
                selectTimeline: (internalOrderId) => void selectTimeline(internalOrderId),
                timelineLoading,
                runAction: (action) => void handleRunAction(action),
              }}
              onActivePanelChange={(id) => {
                if (id) setActiveScreen(id);
              }}
            />

            {showBottomRail ? <BottomEventRail events={model.feed} /> : null}
          </main>

          {rightDrawerOpen ? (
            <>
              <button
                type="button"
                className="ops-drawer-backdrop"
                aria-label="Close operator context drawer"
                onClick={() => setRightDrawerOpen(false)}
              />
              <div className="ops-drawer">
                <RightOpsRail model={model} />
              </div>
            </>
          ) : null}
        </div>
      </div>
    </div>
  );
}