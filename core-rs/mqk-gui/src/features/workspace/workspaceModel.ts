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
