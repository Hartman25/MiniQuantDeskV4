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
import { EMPTY_WORKSPACE_IDENTITY, mergeWorkspaceIdentity, type WorkspaceIdentity } from "./workspaceModel.ts";
import { WORKSPACE_SYNC_EVENT, isNewerRevision, nextRevision, parseWorkspaceSyncMessage } from "./workspaceSync.ts";

export interface WorkspaceContextValue {
  linked: WorkspaceIdentity;
  setLinked: (patch: Partial<WorkspaceIdentity>) => void;
  resetLinked: () => void;
}

const WorkspaceContext = createContext<WorkspaceContextValue | null>(null);

export function WorkspaceProvider({ children }: { children: ReactNode }) {
  const [linked, setLinkedState] = useState<WorkspaceIdentity>(EMPTY_WORKSPACE_IDENTITY);
  const revisionRef = useRef(0);

  const broadcast = useCallback((identity: WorkspaceIdentity) => {
    const revision = nextRevision(revisionRef.current);
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

  // Receives identity changes broadcast by OTHER windows. A message that is
  // malformed, carries an unrecognized field, or is stale/out-of-order
  // (revision <= this window's own last-applied revision) is dropped
  // outright — never partially applied, never allowed to override a newer
  // local change.
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
