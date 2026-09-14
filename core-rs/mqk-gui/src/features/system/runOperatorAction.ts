// GUI-LAYOUT-03: shared operator-action confirmation flow.
//
// Extracted out of AppShell so a detached operator-authority panel (e.g.
// `ops`, opened in its own Tauri window) goes through the exact same
// reason-prompt + confirm dialog as the main workstation window — detaching
// a panel must never bypass its guarded-action UX. Server-side authority
// checks are unaffected either way; this only preserves the client-side
// confirmation step.

import type { OperatorActionDefinition, SystemStatus } from "./types";

export interface RunOperatorActionDeps {
  status: Pick<SystemStatus, "environment" | "live_routing_enabled">;
  runAction: (action: OperatorActionDefinition, args: { reason?: string; target_scope?: string }) => Promise<unknown>;
  refresh: () => Promise<void>;
  targetScope: string;
}

export async function confirmAndRunOperatorAction(
  action: OperatorActionDefinition,
  deps: RunOperatorActionDeps,
): Promise<void> {
  const reason = action.requiresReason
    ? (window.prompt(`Reason required for ${action.label}:`, "Operator review") ?? "")
    : "";
  if (action.requiresReason && !reason.trim()) return;

  const accepted = window.confirm(
    `${action.confirmText}\n\nEnvironment: ${deps.status.environment}\nLive routing: ${
      deps.status.live_routing_enabled ? "enabled" : "disabled"
    }`,
  );
  if (!accepted) return;

  await deps.runAction(action, { reason: reason.trim() || undefined, target_scope: deps.targetScope });
  await deps.refresh();
}
