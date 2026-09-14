// OT-MQD-02 / GUI-LAYOUT-05: React context wiring for the workspace identity
// model. Core logic lives in workspaceModel.ts / workspaceSync.ts (pure,
// unit-tested) — this file is a thin React binding: local state + a stable
// setter/reset pair, now also broadcasting/receiving changes across Tauri
// windows. No API calls, no daemon writes.
//
// Sync is UI identity only and best-effort: broadcasting/listening is
// wrapped in try/catch and silently no-ops outside Tauri (browser-only
// preview stays single-window, exactly as before this patch).

import {
  createContext,
  useCallback,
  useContext,
  useEffect,
  useMemo,
  useRef,
  useState,
  type ReactNode,
} from "react";
import { getCurrentWebviewWindow } from "@tauri-apps/api/webviewWindow";
import { EMPTY_WORKSPACE_IDENTITY, mergeWorkspaceIdentity, type WorkspaceIdentity } from "./workspaceModel.ts";
import {
  INITIAL_SYNC_REVISION,
  WORKSPACE_SYNC_EVENT,
  isNewerRevision,
  nextLocalRevision,
  parseWorkspaceSyncMessage,
  type WorkspaceSyncRevision,
} from "./workspaceSync.ts";

/**
 * WAVE-02-FINAL-REPAIR-01 R5: this window's own identifier for the Lamport
 * tie-break — carries no authority, used only so two windows racing to the
 * same counter resolve to the same winner everywhere. Synchronous, same
 * try/catch-fails-closed pattern as Workstation.tsx's
 * currentWindowLabelOrDefault(); falls back to a fixed label outside Tauri,
 * where there is no other window to race against anyway.
 */
function currentWindowOrigin(): string {
  try {
    return getCurrentWebviewWindow().label;
  } catch {
    return "single-window";
  }
}

export interface WorkspaceContextValue {
  linked: WorkspaceIdentity;
  setLinked: (patch: Partial<WorkspaceIdentity>) => void;
  resetLinked: () => void;
}

const WorkspaceContext = createContext<WorkspaceContextValue | null>(null);

export function WorkspaceProvider({ children }: { children: ReactNode }) {
  const [linked, setLinkedState] = useState<WorkspaceIdentity>(EMPTY_WORKSPACE_IDENTITY);
  // The revision of the identity CURRENTLY applied in this window — whether
  // that came from this window's own last local write or from the last
  // incoming message that won its comparison. Doubles as this window's
  // Lamport clock: nextLocalRevision always stamps strictly past
  // revisionRef.current.counter, and a REJECTED incoming message never
  // carries a counter greater than revisionRef.current.counter already
  // (that is exactly why it was rejected) — so revisionRef.current.counter
  // is always the max counter this window has observed, accepted or not,
  // with no separate clock variable needed.
  const revisionRef = useRef<WorkspaceSyncRevision>(INITIAL_SYNC_REVISION);

  const broadcast = useCallback((identity: WorkspaceIdentity) => {
    const revision = nextLocalRevision(revisionRef.current.counter, currentWindowOrigin());
    revisionRef.current = revision;
    void (async () => {
      try {
        const { emit } = await import("@tauri-apps/api/event");
        await emit(WORKSPACE_SYNC_EVENT, { revision, identity });
      } catch {
        // Not in a Tauri context (or no other windows to reach) — this
        // window's own state is already updated regardless.
      }
    })();
  }, []);

  const setLinked = useCallback(
    (patch: Partial<WorkspaceIdentity>) => {
      setLinkedState((prev) => {
        const next = mergeWorkspaceIdentity(prev, patch);
        broadcast(next);
        return next;
      });
    },
    [broadcast],
  );

  const resetLinked = useCallback(() => {
    setLinkedState(EMPTY_WORKSPACE_IDENTITY);
    broadcast(EMPTY_WORKSPACE_IDENTITY);
  }, [broadcast]);

  // Receives identity changes broadcast by OTHER windows (including this
  // window's own echo of its last broadcast — isNewerRevision correctly
  // rejects that as a tie on both counter and origin). A message that is
  // malformed, carries an unrecognized field, or loses the (counter, origin)
  // comparison against revisionRef.current is dropped outright — never
  // partially applied. Whether accepted or rejected, revisionRef.current
  // already reflects the max counter observed (see the ref's own comment
  // above), so no separate Lamport-clock-advance step is needed here.
  useEffect(() => {
    let unlisten: (() => void) | null = null;
    let cancelled = false;
    (async () => {
      try {
        const { listen } = await import("@tauri-apps/api/event");
        const stop = await listen<unknown>(WORKSPACE_SYNC_EVENT, (event) => {
          const msg = parseWorkspaceSyncMessage(event.payload);
          if (!msg || !isNewerRevision(msg.revision, revisionRef.current)) return;
          revisionRef.current = msg.revision;
          setLinkedState(msg.identity);
        });
        if (cancelled) stop();
        else unlisten = stop;
      } catch {
        // Not in a Tauri context — single-window, nothing to listen for.
      }
    })();
    return () => {
      cancelled = true;
      unlisten?.();
    };
  }, []);

  const value = useMemo<WorkspaceContextValue>(
    () => ({ linked, setLinked, resetLinked }),
    [linked, setLinked, resetLinked],
  );

  return <WorkspaceContext.Provider value={value}>{children}</WorkspaceContext.Provider>;
}

export function useWorkspaceContext(): WorkspaceContextValue {
  const ctx = useContext(WorkspaceContext);
  if (!ctx) {
    throw new Error("useWorkspaceContext must be used within a WorkspaceProvider");
  }
  return ctx;
}

/**
 * GUI-LAYOUT-05: overrides `linked` for a subtree (a pinned panel, or a
 * detached window restoring a frozen identity) while still delegating
 * writes (`setLinked`/`resetLinked`) to the real global provider — pinning a
 * panel's DISPLAY must never disconnect it from the one true write path.
 */
export function WorkspaceScopeProvider({
  value,
  children,
}: {
  value: WorkspaceContextValue;
  children: ReactNode;
}) {
  return <WorkspaceContext.Provider value={value}>{children}</WorkspaceContext.Provider>;
}
