import { useEffect, useMemo, useState } from "react";
import {
  WebviewWindow,
  getAllWebviewWindows,
  getCurrentWebviewWindow,
} from "@tauri-apps/api/webviewWindow";
import { ActionReceiptBanner } from "../components/common/ActionReceiptBanner";
import { BottomEventRail } from "../components/layout/BottomEventRail";
import { LeftCommandRail } from "../components/layout/LeftCommandRail";
import { RightOpsRail } from "../components/layout/RightOpsRail";
import { RoleCommandStrip } from "../components/layout/RoleCommandStrip";
import { WorkspaceFrame } from "../components/layout/WorkspaceFrame";
import { WorkspaceToolbar } from "../components/layout/WorkspaceToolbar";
import { PreflightGate } from "../components/preflight/PreflightGate";
import { GlobalStatusBar } from "../components/status/GlobalStatusBar";
import { ROLE_SCREENS, SCREEN_REGISTRY, type ScreenKey } from "../features/screens/screenRegistry";
import { useOperatorModel } from "../features/system/useOperatorModel";
import type { OperatorActionDefinition } from "../features/system/types";
import { formatDateTime } from "../lib/format";
import type { DeskMode, DeskRole } from "./shellTypes";

const DESK_MODE_STORAGE_KEY = "mqd.desktop.deskMode";

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
  const deskRole = useMemo(() => detectDeskRole(), []);
  const [deskMode, setDeskMode] = useState<DeskMode>("single");
  const [activeScreen, setActiveScreen] = useState<ScreenKey>(defaultScreenForRole(deskRole));

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
  }, []);

  useEffect(() => {
    window.localStorage.setItem(DESK_MODE_STORAGE_KEY, deskMode);
  }, [deskMode]);

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


  const handleRunAction = async (action: OperatorActionDefinition) => {
    const reason = action.requiresReason
      ? window.prompt(`Reason required for ${action.label}:`, "Operator review") ?? ""
      : "";
    if (action.requiresReason && !reason.trim()) return;

    const accepted = window.confirm(
      `${action.confirmText}\\n\\nEnvironment: ${model.status.environment}\\nLive routing: ${
        model.status.live_routing_enabled ? "enabled" : "disabled"
      }`,
    );
    if (!accepted) return;

    await runAction(action, {
      reason: reason.trim() || undefined,
      target_scope: activeScreen,
    });
    await refresh();
  };

  const showLeftRail = deskRole === "control";
  const showBottomRail = deskRole !== "oversight";
  const showRightRail = true;

  return (
    <div className={`app-shell desk-mode-${deskMode} desk-role-${deskRole}`}>
      {showLeftRail ? (
        <LeftCommandRail activeScreen={activeScreen} onSelect={setActiveScreen} />
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
            />

            {(deskRole === "execution" || deskRole === "oversight") ? (
              <RoleCommandStrip
                deskRole={deskRole}
                activeScreen={activeScreen}
                onSelect={setActiveScreen}
              />
            ) : null}

            <ActionReceiptBanner receipt={actionReceipt} />
            <PreflightGate preflight={model.preflight} />

            <WorkspaceFrame
              title={screen.title}
              description={screen.description}
              panelKey={activeScreen}
              authority={model.panelSources[activeScreen]}
            >
              {screen.render({
                model,
                selectTimeline: (internalOrderId) => void selectTimeline(internalOrderId),
                timelineLoading,
                runAction: (action) => void handleRunAction(action),
              })}
            </WorkspaceFrame>

            {showBottomRail ? <BottomEventRail events={model.feed} /> : null}
          </main>

          {showRightRail ? <RightOpsRail model={model} /> : null}
        </div>
      </div>
    </div>
  );
}