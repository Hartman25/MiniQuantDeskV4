// OT-MQD-02: Linked Identity / Workspace Context — pure model.
//
// WorkspaceIdentity is presentation-only state describing what the operator
// is currently looking at. It carries no authority: there is no
// armed/deployed/promoted/executed field on this type, and no function in
// this module calls any API, mutates any artifact, or performs an
// operator action. Selecting a symbol/strategy/run here can only ever
// change what a panel requests/displays — never broker, risk, or promotion
// state. See AUTHORITY BOUNDARY in the OT-MQD-02 mission for the full list
// of things workspace selection MUST NOT do.

export type WorkspaceExecutionDomain = "backtest" | "paper" | "live" | null;

export interface WorkspaceIdentity {
  symbol: string | null;
  timeframe: string | null;
  strategyId: string | null;
  runId: string | null;
  backtestJobId: string | null;
  artifactId: string | null;
  evaluationSlice: string | null;
  executionDomain: WorkspaceExecutionDomain;
}

/**
 * The closed set of fields WorkspaceIdentity may ever carry. Enumerated
 * explicitly (rather than relying on `keyof WorkspaceIdentity` alone) so a
 * runtime test can assert the identity object never grows an authority
 * field without a deliberate, reviewed change to this list.
 */
export const WORKSPACE_IDENTITY_FIELDS = [
  "symbol",
  "timeframe",
  "strategyId",
  "runId",
  "backtestJobId",
  "artifactId",
  "evaluationSlice",
  "executionDomain",
] as const;

export const EMPTY_WORKSPACE_IDENTITY: WorkspaceIdentity = {
  symbol: null,
  timeframe: null,
  strategyId: null,
  runId: null,
  backtestJobId: null,
  artifactId: null,
  evaluationSlice: null,
  executionDomain: null,
};

/**
 * Applies a partial identity patch. A field omitted from `patch` is left
 * unchanged; a field explicitly present in `patch` (including explicit
 * `null`/`undefined`) is set to its patch value or null — never invented.
 * Only fields in WORKSPACE_IDENTITY_FIELDS are ever read from `patch`, so an
 * accidental extra key (e.g. a caller trying to smuggle `armed: true`) is
 * silently dropped rather than merged in.
 */
export function mergeWorkspaceIdentity(
  base: WorkspaceIdentity,
  patch: Partial<WorkspaceIdentity>,
): WorkspaceIdentity {
  const next: WorkspaceIdentity = { ...base };
  for (const key of WORKSPACE_IDENTITY_FIELDS) {
    if (key in patch) {
      next[key] = (patch[key] ?? null) as never;
    }
  }
  return next;
}

// ---------------------------------------------------------------------------
// Panel link state — LINKED vs PINNED
// ---------------------------------------------------------------------------

export interface PanelLinkState {
  pinned: boolean;
  pinnedIdentity: WorkspaceIdentity;
}

export function initialPanelLinkState(): PanelLinkState {
  return { pinned: false, pinnedIdentity: EMPTY_WORKSPACE_IDENTITY };
}

/**
 * What identity should a panel display right now? A LINKED panel (pinned
 * === false) always mirrors the current global identity; a PINNED panel
 * ignores global updates entirely and keeps showing its own frozen identity.
 */
export function resolvePanelIdentity(global: WorkspaceIdentity, panel: PanelLinkState): WorkspaceIdentity {
  return panel.pinned ? panel.pinnedIdentity : global;
}

/** Freezes the panel's currently-resolved identity so global updates stop propagating to it. */
export function pinPanel(currentResolved: WorkspaceIdentity): PanelLinkState {
  return { pinned: true, pinnedIdentity: currentResolved };
}

/** Returns to following the global linked identity. */
export function unpinPanel(): PanelLinkState {
  return { pinned: false, pinnedIdentity: EMPTY_WORKSPACE_IDENTITY };
}

/**
 * A panel that only makes sense keyed by symbol (e.g. Market Data) must
 * treat a missing symbol as incompatible — never silently fall back to
 * showing its full unfiltered content as though it were linked.
 */
export function isSymbolCompatible(identity: WorkspaceIdentity): boolean {
  return typeof identity.symbol === "string" && identity.symbol.trim() !== "";
}

// ---------------------------------------------------------------------------
// Cross-window transport validation — GUI-LAYOUT-05
// ---------------------------------------------------------------------------

const EXECUTION_DOMAIN_VALUES: ReadonlySet<string | null> = new Set(["backtest", "paper", "live", null]);

function isStringOrNull(value: unknown): value is string | null {
  return typeof value === "string" || value === null;
}

/**
 * Validates an untrusted value (e.g. from a cross-window event payload or a
 * detached-window URL parameter) as a WorkspaceIdentity. The check is a
 * CLOSED set: the value must have exactly WORKSPACE_IDENTITY_FIELDS.length
 * keys, every key must be one of WORKSPACE_IDENTITY_FIELDS, and every value
 * must be the right type — an extra key (e.g. a smuggled `armed` or
 * `live_routing_enabled`) or a wrong-typed value fails the whole payload
 * rather than being silently dropped or coerced. This is what keeps a
 * cross-window message from ever introducing an authority field: the type
 * system only exists on WorkspaceIdentity, but this closed-set runtime check
 * is what enforces it on data crossing a process boundary.
 */
export function parseWorkspaceIdentityPayload(value: unknown): WorkspaceIdentity | null {
  if (typeof value !== "object" || value === null) return null;
  const record = value as Record<string, unknown>;
  const keys = Object.keys(record);
  if (keys.length !== WORKSPACE_IDENTITY_FIELDS.length) return null;
  for (const key of keys) {
    if (!(WORKSPACE_IDENTITY_FIELDS as readonly string[]).includes(key)) return null;
  }
  for (const field of WORKSPACE_IDENTITY_FIELDS) {
    const fieldValue = record[field];
    if (field === "executionDomain") {
      if (!EXECUTION_DOMAIN_VALUES.has(fieldValue as string | null)) return null;
    } else if (!isStringOrNull(fieldValue)) {
      return null;
    }
  }
  return record as unknown as WorkspaceIdentity;
}

const PANEL_LINK_STATE_FIELDS = ["pinned", "pinnedIdentity"] as const;

/**
 * Validates an untrusted value (e.g. a cross-window reattach event payload)
 * as a PanelLinkState. Same closed-set discipline as
 * parseWorkspaceIdentityPayload: the value must have exactly {pinned,
 * pinnedIdentity} and nothing else — a smuggled extra key (e.g. an
 * authority-shaped field) fails the whole payload. Never throws and never
 * returns null: any malformed shape, a non-boolean `pinned`, or an invalid
 * `pinnedIdentity` fails closed to initialPanelLinkState() (unpinned/Linked)
 * — the same safe default a brand-new panel starts in — rather than half
 * applying a corrupted pin.
 */
export function parsePanelLinkStatePayload(value: unknown): PanelLinkState {
  if (typeof value !== "object" || value === null) return initialPanelLinkState();
  const record = value as Record<string, unknown>;
  const keys = Object.keys(record);
  if (keys.length !== PANEL_LINK_STATE_FIELDS.length) return initialPanelLinkState();
  for (const key of keys) {
    if (!(PANEL_LINK_STATE_FIELDS as readonly string[]).includes(key)) return initialPanelLinkState();
  }
  if (typeof record.pinned !== "boolean") return initialPanelLinkState();
  if (!record.pinned) return initialPanelLinkState();
  const identity = parseWorkspaceIdentityPayload(record.pinnedIdentity);
  return identity === null ? initialPanelLinkState() : pinPanel(identity);
}
