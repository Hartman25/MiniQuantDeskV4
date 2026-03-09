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
import { WorkspaceFrame } from "../components/layout/WorkspaceFrame";
import { PreflightGate } from "../components/preflight/PreflightGate";
import { GlobalStatusBar } from "../components/status/GlobalStatusBar";
import { SCREEN_REGISTRY, type ScreenKey } from "../features/screens/screenRegistry";
import { useOperatorModel } from "../features/system/useOperatorModel";
import type { EnvironmentMode, OperatorActionDefinition } from "../features/system/types";
import { formatDateTime } from "../lib/format";

type DeskMode = "single" | "two" | "three";
type DeskRole = "control" | "execution" | "oversight";

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

function defaultScreenForRole(role: DeskRole): ScreenKey {
  switch (role) {
    case "execution":
      return "execution";
    case "oversight":
      return "risk";
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
    await ensureWindow("execution", "MiniQuantDesk - Execution", 1600, 1000);
    await closeWindow("oversight");
    console.log("Ensured execution, closed oversight");
    return;
  }

  if (mode === "three") {
    await ensureWindow("execution", "MiniQuantDesk - Execution", 1600, 1000);
    await ensureWindow("oversight", "MiniQuantDesk - Oversight", 1500, 960);
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
    requestModeChange,
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

  const handleChangeMode = async (targetMode: EnvironmentMode) => {
    if (targetMode === model.status.environment) return;

    const typed = window.prompt(
      [
        `Change system mode from ${model.status.environment.toUpperCase()} to ${targetMode.toUpperCase()}.`,
        "This will request a controlled daemon restart and configuration reload.",
        `Type ${targetMode.toUpperCase()} to confirm.`,
      ].join("\\n\\n"),
      "",
    );
    if ((typed ?? "").trim().toUpperCase() !== targetMode.toUpperCase()) return;

    const reason =
      window.prompt(
        `Reason required for mode transition to ${targetMode.toUpperCase()}:`,
        "Controlled environment change",
      ) ?? "";
    if (!reason.trim()) return;

    await requestModeChange(targetMode, reason.trim());
    await refresh();
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
            <div className="workspace-toolbar panel">
              <div>
                <div className="eyebrow">Operator Session</div>
                <h2>{screen.title}</h2>
              </div>

              <div className="toolbar-metrics">
                <span>{loading ? "Loading…" : model.connected ? "Connected" : "Disconnected"}</span>
                <span>Last refresh: {formatDateTime(model.lastUpdatedAt)}</span>

                {deskRole === "control" ? (
                  <div className="desk-mode-toggle" role="group" aria-label="Desk mode">
                    <button
                      type="button"
                      className={`action-button ghost ${deskMode === "single" ? "is-selected" : ""}`}
                      onClick={() => void handleDeskModeChange("single")}
                    >
                      1 window
                    </button>
                    <button
                      type="button"
                      className={`action-button ghost ${deskMode === "two" ? "is-selected" : ""}`}
                      onClick={() => void handleDeskModeChange("two")}
                    >
                      2 monitors
                    </button>
                    <button
                      type="button"
                      className={`action-button ghost ${deskMode === "three" ? "is-selected" : ""}`}
                      onClick={() => void handleDeskModeChange("three")}
                    >
                      3 monitors
                    </button>
                  </div>
                ) : null}

                <button className="action-button ghost" onClick={() => void refresh()}>
                  Refresh
                </button>
              </div>
            </div>

            <ActionReceiptBanner receipt={actionReceipt} />
            <PreflightGate preflight={model.preflight} />

            <WorkspaceFrame title={screen.title} description={screen.description}>
              {screen.render({
                model,
                selectTimeline: (internalOrderId) => void selectTimeline(internalOrderId),
                timelineLoading,
                runAction: (action) => void handleRunAction(action),
                changeMode: (targetMode) => void handleChangeMode(targetMode),
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