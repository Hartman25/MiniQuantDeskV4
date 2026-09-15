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

// ---------------------------------------------------------------------------
// R1 — GUI-WORKSPACE-LATE-JOIN-SYNC-01: mount-time handshake so a window that
// joins AFTER the current workspace identity was already established
// converges without requiring another operator context change. UI IDENTITY
// ONLY, same as the rest of this module — see the file header.
// ---------------------------------------------------------------------------

export const WORKSPACE_SYNC_REQUEST_EVENT = "mqd:workspace:sync-request";

/** Closed request schema: exactly {requester}, a non-empty string identifying the asking window. Carries no identity/authority — a request never smuggles the answer it's asking for. */
export interface WorkspaceSyncRequest {
  requester: string;
}

const SYNC_REQUEST_FIELDS = ["requester"] as const;

/**
 * Validates an untrusted cross-window sync-request payload against the
 * CLOSED {requester} schema. Same discipline as parseWorkspaceSyncMessage:
 * wrong shape, wrong type, empty string, or any extra key (including an
 * authority-shaped one) voids the whole payload rather than partially
 * accepting it.
 */
export function parseWorkspaceSyncRequest(raw: unknown): WorkspaceSyncRequest | null {
  if (typeof raw !== "object" || raw === null) return null;
  const record = raw as Record<string, unknown>;
  const keys = Object.keys(record);
  if (keys.length !== SYNC_REQUEST_FIELDS.length) return null;
  for (const key of keys) {
    if (!(SYNC_REQUEST_FIELDS as readonly string[]).includes(key)) return null;
  }
  const { requester } = record;
  if (typeof requester !== "string" || requester.length === 0) return null;
  return { requester };
}

/** Transport-agnostic view of the cross-window event bus, so the handshake below can be exercised by tests with a fake in-memory bus instead of the real Tauri event API. */
export interface WorkspaceSyncTransport {
  listen: (event: string, handler: (payload: unknown) => void) => Promise<() => void>;
  emit: (event: string, payload: unknown) => Promise<void>;
}

/** The caller's (WorkspaceContext.tsx's) current state, read fresh each time the handshake needs it — never captured once, since a local write can happen between handshake install and a request/response round trip. */
export interface WorkspaceSyncHandshakeState {
  getRevision: () => WorkspaceSyncRevision;
  getIdentity: () => WorkspaceIdentity;
  /** Called when a remote WORKSPACE_SYNC_EVENT (an ordinary broadcast, or a late-join response) beats the current revision. Must apply both revision and identity atomically from the caller's side. */
  onRemoteState: (revision: WorkspaceSyncRevision, identity: WorkspaceIdentity) => void;
}

/**
 * Installs the full R1 mount-time handshake on `transport` and returns a
 * cleanup function. Required lifecycle, enforced by sequential awaits (not
 * by effect-declaration order, which the real Tauri event API gives no
 * ordering guarantee across):
 *
 *   1. install the ordinary workspace-state listener (existing behavior —
 *      future broadcasts still apply via isNewerRevision);
 *   2. install the sync-request listener;
 *   3. only once both listeners are ready, emit ONE sync-request.
 *
 * A window receiving another origin's request responds with its OWN current
 * {revision, identity} on the existing WORKSPACE_SYNC_EVENT — never a new
 * Lamport-stamped write (no nextLocalRevision call, no mutation of the
 * responder's own state), so a requester that made a newer local write
 * before the response arrives keeps winning via the ordinary
 * isNewerRevision comparison (R1-LJ-03). A window never responds to its own
 * request (matched by `origin`), and a request only ever yields state
 * responses, never another request — so no request/response loop is
 * possible structurally.
 */
export async function installWorkspaceSyncHandshake(
  transport: WorkspaceSyncTransport,
  origin: string,
  state: WorkspaceSyncHandshakeState,
): Promise<() => void> {
  const unlistenState = await transport.listen(WORKSPACE_SYNC_EVENT, (payload) => {
    const msg = parseWorkspaceSyncMessage(payload);
    if (!msg || !isNewerRevision(msg.revision, state.getRevision())) return;
    state.onRemoteState(msg.revision, msg.identity);
  });

  const unlistenRequest = await transport.listen(WORKSPACE_SYNC_REQUEST_EVENT, (payload) => {
    const req = parseWorkspaceSyncRequest(payload);
    if (!req || req.requester === origin) return;
    void transport.emit(WORKSPACE_SYNC_EVENT, { revision: state.getRevision(), identity: state.getIdentity() });
  });

  await transport.emit(WORKSPACE_SYNC_REQUEST_EVENT, { requester: origin });

  return () => {
    unlistenState();
    unlistenRequest();
  };
}
