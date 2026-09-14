// GUI-LAYOUT-05: cross-window linked-context synchronization — pure,
// unit-tested envelope/ordering logic. The transport (Tauri global
// emit/listen) lives in WorkspaceContext.tsx; this module only decides
// whether an incoming message is well-formed and whether it is newer than
// what this window already has.
//
// UI IDENTITY ONLY: a sync message carries a WorkspaceIdentity (validated via
// parseWorkspaceIdentityPayload's closed field set) and a revision number.
// It is never used to carry broker/DB/runtime state, and never will be —
// see workspaceModel.ts's AUTHORITY BOUNDARY note.

import { parseWorkspaceIdentityPayload, type WorkspaceIdentity } from "./workspaceModel";

export const WORKSPACE_SYNC_EVENT = "mqd:workspace:sync";

export interface WorkspaceSyncMessage {
  revision: number;
  identity: WorkspaceIdentity;
}

/**
 * Validates an untrusted cross-window payload. Rejects anything without a
 * finite numeric `revision` or whose `identity` fails the closed-field-set
 * check — a malformed or foreign message is dropped outright, never
 * partially applied.
 */
export function parseWorkspaceSyncMessage(raw: unknown): WorkspaceSyncMessage | null {
  if (typeof raw !== "object" || raw === null) return null;
  const record = raw as Record<string, unknown>;
  if (typeof record.revision !== "number" || !Number.isFinite(record.revision)) return null;
  const identity = parseWorkspaceIdentityPayload(record.identity);
  if (!identity) return null;
  return { revision: record.revision, identity };
}

/**
 * An incoming update only ever wins on a STRICTLY greater revision — a
 * stale or out-of-order message (equal or lower revision) can never
 * override the current identity, including two updates racing in the same
 * millisecond (the earlier-applied one keeps its state on a tie).
 */
export function isNewerRevision(incomingRevision: number, currentRevision: number): boolean {
  return incomingRevision > currentRevision;
}

/**
 * The next revision to stamp on a LOCAL change. Wall-clock based, but
 * clamped to be strictly greater than the current revision even if the
 * clock hasn't advanced (or moved backward) since the last one — this
 * window's own successive edits are always monotonic regardless of clock
 * resolution.
 */
export function nextRevision(currentRevision: number, now: number = Date.now()): number {
  return Math.max(now, currentRevision + 1);
}
