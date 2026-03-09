import { useEffect, useState } from "react";
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

type DeskMode = "two" | "three";

const DESK_MODE_STORAGE_KEY = "mqd.deskMode";

export function AppShell() {
  const [activeScreen, setActiveScreen] = useState<ScreenKey>("dashboard");
  const [deskMode, setDeskMode] = useState<DeskMode>("two");

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
    if (stored === "two" || stored === "three") {
      setDeskMode(stored);
    }
  }, []);

  useEffect(() => {
    window.localStorage.setItem(DESK_MODE_STORAGE_KEY, deskMode);
  }, [deskMode]);

  const handleChangeMode = async (targetMode: EnvironmentMode) => {
    if (targetMode === model.status.environment) return;

    const typed = window.prompt(
      [
        `Change system mode from ${model.status.environment.toUpperCase()} to ${targetMode.toUpperCase()}.`,
        "This will request a controlled daemon restart and configuration reload.",
        `Type ${targetMode.toUpperCase()} to confirm.`,
      ].join("\n\n"),
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
      `${action.confirmText}\n\nEnvironment: ${model.status.environment}\nLive routing: ${
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

  return (
    <div className={`app-shell desk-mode-${deskMode}`}>
      <LeftCommandRail activeScreen={activeScreen} onSelect={setActiveScreen} />

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

                <div className="desk-mode-toggle" role="group" aria-label="Desk mode">
                  <button
                    type="button"
                    className={`action-button ghost ${deskMode === "two" ? "is-selected" : ""}`}
                    onClick={() => setDeskMode("two")}
                  >
                    2 monitors
                  </button>
                  <button
                    type="button"
                    className={`action-button ghost ${deskMode === "three" ? "is-selected" : ""}`}
                    onClick={() => setDeskMode("three")}
                  >
                    3 monitors
                  </button>
                </div>

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

            <BottomEventRail events={model.feed} />
          </main>

          <RightOpsRail model={model} />
        </div>
      </div>
    </div>
  );
}
