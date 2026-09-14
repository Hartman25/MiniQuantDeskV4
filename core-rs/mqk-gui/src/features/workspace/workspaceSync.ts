// GUI-LAYOUT-05: cross-window linked-context synchronization — pure,
// unit-tested envelope/ordering logic. The transport (Tauri global
// emit/listen) lives in WorkspaceContext.tsx; this module only decides
// whether an incoming message is well-formed and whether it is newer than
// what this window already has.
//
// UI IDENTITY ONLY: a sync message carries a WorkspaceIdentity (validated via
// parseWorkspaceIdentityPayload's closed field set) and a revision. It is
// never used to carry broker/DB/runtime state, and never will be — see
// workspaceModel.ts's AUTHORITY BOUNDARY note.
//
// WAVE-02-FINAL-REPAIR-01 R5: revision is a Lamport-style (counter, origin)
// tuple, not a wall-clock timestamp. A single `Math.max(Date.now(), n+1)`
// counter lets two windows that write in the same millisecond compute the
// identical next revision — each then rejects the other's equal-revision
// broadcast (isNewerRevision required STRICTLY greater), leaving the two
// windows' linked context permanently diverged until one of them happens to
// write again. The (counter, origin) tuple is a strict total order: when
// counters tie, `origin` (this window's own Tauri label — unique per window,
// carries no authority, used only to break the tie) decides the winner, and
// every window applies the exact same comparison — so two windows that
// raced to the same counter always converge to the identical result,
// regardless of which message each window happened to receive first.

import { parseWorkspaceIdentityPayload, type WorkspaceIdentity } from "./workspaceModel";

export const WORKSPACE_SYNC_EVENT = "mqd:workspace:sync";

/**
 * A Lamport logical clock stamp. `counter` orders events; `origin` (a
 * window identifier) is a pure tie-break with no authority meaning of its
 * own — it is never read for anything except this comparison.
 */
export interface WorkspaceSyncRevision {
  counter: number;
  origin: string;
}

/** Every window starts here — origin "" sorts below any real window label, so the very first real write from any window is unambiguously newer. */
export const INITIAL_SYNC_REVISION: WorkspaceSyncRevision = { counter: 0, origin: "" };

export interface WorkspaceSyncMessage {
  revision: WorkspaceSyncRevision;
  identity: WorkspaceIdentity;
}

const REVISION_FIELDS = ["counter", "origin"] as const;

function parseSyncRevision(value: unknown): WorkspaceSyncRevision | null {
  if (typeof value !== "object" || value === null) return null;
  const record = value as Record<string, unknown>;
  const keys = Object.keys(record);
  if (keys.length !== REVISION_FIELDS.length) return null;
  for (const key of keys) {
    if (!(REVISION_FIELDS as readonly string[]).includes(key)) return null;
  }
  const { counter, origin } = record;
  if (typeof counter !== "number" || !Number.isFinite(counter)) return null;
  if (typeof origin !== "string" || origin.length === 0) return null;
  return { counter, origin };
}

/**
 * Validates an untrusted cross-window payload. Rejects anything whose
 * `revision` isn't a well-formed {counter, origin} tuple or whose `identity`
 * fails the closed-field-set check — a malformed or foreign message
 * (including a smuggled extra key on the revision itself) is dropped
 * outright, never partially applied.
 */
export function parseWorkspaceSyncMessage(raw: unknown): WorkspaceSyncMessage | null {
  if (typeof raw !== "object" || raw === null) return null;
  const record = raw as Record<string, unknown>;
  const revision = parseSyncRevision(record.revision);
  if (!revision) return null;
  const identity = parseWorkspaceIdentityPayload(record.identity);
  if (!identity) return null;
  return { revision, identity };
}

/**
 * Total order over WorkspaceSyncRevision: `counter` first; on a tie,
 * `origin` compared lexicographically. This is a strict total order (no two
 * distinct windows share an origin, so a tie on both fields only ever
 * happens when comparing a revision against itself) — the same comparison
 * evaluated in any window, in any arrival order, always picks the same
 * winner. A stale/lower counter (or an equal counter with a
 * lexicographically-losing origin) can never override.
 */
export function isNewerRevision(incoming: WorkspaceSyncRevision, current: WorkspaceSyncRevision): boolean {
  if (incoming.counter !== current.counter) return incoming.counter > current.counter;
  return incoming.origin > current.origin;
}

/**
 * The next revision to stamp on a LOCAL change, given the highest counter
 * this window has observed so far (its own last-stamped counter, and every
 * counter received from another window's broadcast — see
 * WorkspaceContext.tsx's Lamport-clock advance on receive). Always strictly
 * greater than every counter observed, so a local write is deterministically
 * ordered after everything this window currently knows about — including a
 * second write from a window that just lost a tie, which advances past the
 * counter it lost on rather than repeating it.
 */
export function nextLocalRevision(observedMaxCounter: number, origin: string): WorkspaceSyncRevision {
  return { counter: observedMaxCounter + 1, origin };
}
