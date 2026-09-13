// OT-MQD-02: React context wiring for the workspace identity model. All
// logic lives in workspaceModel.ts (pure, unit-tested) — this file is a thin
// React binding: local state + a stable setter/reset pair. No API calls, no
// daemon writes.

import { createContext, useCallback, useContext, useMemo, useState, type ReactNode } from "react";
import { EMPTY_WORKSPACE_IDENTITY, mergeWorkspaceIdentity, type WorkspaceIdentity } from "./workspaceModel.ts";

export interface WorkspaceContextValue {
  linked: WorkspaceIdentity;
  setLinked: (patch: Partial<WorkspaceIdentity>) => void;
  resetLinked: () => void;
}

const WorkspaceContext = createContext<WorkspaceContextValue | null>(null);

export function WorkspaceProvider({ children }: { children: ReactNode }) {
  const [linked, setLinkedState] = useState<WorkspaceIdentity>(EMPTY_WORKSPACE_IDENTITY);

  const setLinked = useCallback((patch: Partial<WorkspaceIdentity>) => {
    setLinkedState((prev) => mergeWorkspaceIdentity(prev, patch));
  }, []);

  const resetLinked = useCallback(() => setLinkedState(EMPTY_WORKSPACE_IDENTITY), []);

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
