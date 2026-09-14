import test from "node:test";
import assert from "node:assert/strict";
import {
  EMPTY_WORKSPACE_IDENTITY,
  WORKSPACE_IDENTITY_FIELDS,
  initialPanelLinkState,
  isSymbolCompatible,
  mergeWorkspaceIdentity,
  parsePanelLinkStatePayload,
  parseWorkspaceIdentityPayload,
  pinPanel,
  resolvePanelIdentity,
  unpinPanel,
  type WorkspaceIdentity,
} from "../workspaceModel.ts";

// ---------------------------------------------------------------------------
// A — linked symbol update => compatible (unpinned/LINKED) panel changes.
// ---------------------------------------------------------------------------

test("A: an unpinned panel's resolved identity follows global linked updates", () => {
  const panel = initialPanelLinkState();
  const globalV1 = mergeWorkspaceIdentity(EMPTY_WORKSPACE_IDENTITY, { symbol: "AAPL" });
  const globalV2 = mergeWorkspaceIdentity(globalV1, { symbol: "MSFT" });

  assert.equal(resolvePanelIdentity(globalV1, panel).symbol, "AAPL");
  assert.equal(resolvePanelIdentity(globalV2, panel).symbol, "MSFT");
});

// ---------------------------------------------------------------------------
// B — a pinned panel does NOT change from a global linked update.
// ---------------------------------------------------------------------------

test("B: a pinned panel ignores subsequent global identity changes", () => {
  const globalV1 = mergeWorkspaceIdentity(EMPTY_WORKSPACE_IDENTITY, { symbol: "AAPL", strategyId: "LIQ-01" });
  const pinned = pinPanel(resolvePanelIdentity(globalV1, initialPanelLinkState()));
  assert.equal(pinned.pinned, true);

  const globalV2 = mergeWorkspaceIdentity(globalV1, { symbol: "MSFT", strategyId: "LIQ-02" });
  const resolved = resolvePanelIdentity(globalV2, pinned);
  assert.equal(resolved.symbol, "AAPL");
  assert.equal(resolved.strategyId, "LIQ-01");
});

// ---------------------------------------------------------------------------
// D — context with missing strategy/run ID remains absent/unknown; no
// synthetic identity is fabricated for an omitted field.
// ---------------------------------------------------------------------------

test("D: an identity patch that only sets symbol leaves strategyId/runId null, never fabricated", () => {
  const identity = mergeWorkspaceIdentity(EMPTY_WORKSPACE_IDENTITY, { symbol: "AAPL" });
  assert.equal(identity.symbol, "AAPL");
  assert.equal(identity.strategyId, null);
  assert.equal(identity.runId, null);
  assert.equal(identity.backtestJobId, null);
});

// ---------------------------------------------------------------------------
// E — symbol selection cannot manufacture strategy/deployment authority:
// WorkspaceIdentity's field set is closed and contains no authority field.
// ---------------------------------------------------------------------------

test("E: WorkspaceIdentity's field set is exactly the closed, authority-free list", () => {
  const keys = Object.keys(EMPTY_WORKSPACE_IDENTITY).sort();
  const expected = [...WORKSPACE_IDENTITY_FIELDS].sort();
  assert.deepEqual(keys, expected);
  for (const forbidden of ["armed", "deployed", "promoted", "executed", "approved_for_live", "live_locked"]) {
    assert.ok(!keys.includes(forbidden), `WorkspaceIdentity must never carry '${forbidden}'`);
  }
});

test("E (mutation guard): an unrecognized patch key is silently dropped, not merged in", () => {
  const patch = { symbol: "AAPL", armed: true } as Partial<WorkspaceIdentity> & { armed?: boolean };
  const identity = mergeWorkspaceIdentity(EMPTY_WORKSPACE_IDENTITY, patch);
  assert.equal((identity as unknown as { armed?: boolean }).armed, undefined);
  assert.equal(Object.keys(identity).length, WORKSPACE_IDENTITY_FIELDS.length);
});

// ---------------------------------------------------------------------------
// F — selecting a backtest artifact does not imply Paper/Live deployment.
// ---------------------------------------------------------------------------

test("F: linking a backtest identity keeps executionDomain exactly 'backtest', never widened to paper/live", () => {
  const identity = mergeWorkspaceIdentity(EMPTY_WORKSPACE_IDENTITY, {
    symbol: "AAPL",
    runId: "run-1",
    artifactId: "artifact-1",
    executionDomain: "backtest",
  });
  assert.equal(identity.executionDomain, "backtest");
  assert.notEqual(identity.executionDomain, "paper");
  assert.notEqual(identity.executionDomain, "live");
});

// ---------------------------------------------------------------------------
// G — context reset returns to explicit unset state, not an arbitrary
// default identity.
// ---------------------------------------------------------------------------

test("G: resetting to EMPTY_WORKSPACE_IDENTITY clears every field to null, not a default symbol/strategy", () => {
  const populated = mergeWorkspaceIdentity(EMPTY_WORKSPACE_IDENTITY, {
    symbol: "AAPL",
    strategyId: "LIQ-01",
    runId: "run-1",
    executionDomain: "backtest",
  });
  assert.notDeepEqual(populated, EMPTY_WORKSPACE_IDENTITY);

  const reset = { ...EMPTY_WORKSPACE_IDENTITY };
  for (const field of WORKSPACE_IDENTITY_FIELDS) {
    assert.equal(reset[field], null);
  }
  assert.deepEqual(reset, EMPTY_WORKSPACE_IDENTITY);
});

test("G: unpinning a panel returns it to the unset pinnedIdentity, not the identity it was pinned with", () => {
  const globalV1 = mergeWorkspaceIdentity(EMPTY_WORKSPACE_IDENTITY, { symbol: "AAPL" });
  const pinned = pinPanel(resolvePanelIdentity(globalV1, initialPanelLinkState()));
  assert.equal(pinned.pinnedIdentity.symbol, "AAPL");

  const unpinned = unpinPanel();
  assert.equal(unpinned.pinned, false);
  assert.deepEqual(unpinned.pinnedIdentity, EMPTY_WORKSPACE_IDENTITY);
  // Once unpinned, resolution must follow global again, not the stale pinned value.
  const globalV2 = mergeWorkspaceIdentity(globalV1, { symbol: "MSFT" });
  assert.equal(resolvePanelIdentity(globalV2, unpinned).symbol, "MSFT");
});

// ---------------------------------------------------------------------------
// H — incompatible context: a symbol-keyed panel must fail visibly/neutral,
// never silently show unrelated data as though it were linked.
// ---------------------------------------------------------------------------

test("H: isSymbolCompatible is false for an unset identity, true once a symbol is present", () => {
  assert.equal(isSymbolCompatible(EMPTY_WORKSPACE_IDENTITY), false);
  assert.equal(isSymbolCompatible(mergeWorkspaceIdentity(EMPTY_WORKSPACE_IDENTITY, { symbol: "" })), false);
  assert.equal(isSymbolCompatible(mergeWorkspaceIdentity(EMPTY_WORKSPACE_IDENTITY, { symbol: "   " })), false);
  assert.equal(isSymbolCompatible(mergeWorkspaceIdentity(EMPTY_WORKSPACE_IDENTITY, { symbol: "AAPL" })), true);
});

// ---------------------------------------------------------------------------
// Positive control: a fully populated identity resolves and merges exactly
// as supplied across every field, with no field silently dropped.
// ---------------------------------------------------------------------------

// ---------------------------------------------------------------------------
// I — GUI-LAYOUT-05: cross-window/cross-process payload validation. This is
// the boundary check for any untrusted WorkspaceIdentity-shaped value
// arriving over a Tauri event or a detached-window URL parameter.
// ---------------------------------------------------------------------------

test("I: parseWorkspaceIdentityPayload accepts a well-formed, fully-null identity", () => {
  assert.deepEqual(parseWorkspaceIdentityPayload(EMPTY_WORKSPACE_IDENTITY), EMPTY_WORKSPACE_IDENTITY);
});

test("I: parseWorkspaceIdentityPayload accepts a fully populated identity", () => {
  const full = mergeWorkspaceIdentity(EMPTY_WORKSPACE_IDENTITY, { symbol: "AAPL", executionDomain: "live" });
  assert.deepEqual(parseWorkspaceIdentityPayload(full), full);
});

test("I: parseWorkspaceIdentityPayload rejects a non-object and null", () => {
  assert.equal(parseWorkspaceIdentityPayload(null), null);
  assert.equal(parseWorkspaceIdentityPayload(undefined), null);
  assert.equal(parseWorkspaceIdentityPayload("AAPL"), null);
  assert.equal(parseWorkspaceIdentityPayload(42), null);
  assert.equal(parseWorkspaceIdentityPayload([]), null);
});

test("I: parseWorkspaceIdentityPayload rejects an extra/unrecognized key — no authority field can enter through a message", () => {
  const smuggled = { ...EMPTY_WORKSPACE_IDENTITY, armed: true };
  assert.equal(parseWorkspaceIdentityPayload(smuggled), null);
  const smuggled2 = { ...EMPTY_WORKSPACE_IDENTITY, live_routing_enabled: true };
  assert.equal(parseWorkspaceIdentityPayload(smuggled2), null);
});

test("I: parseWorkspaceIdentityPayload rejects a missing field (not treated as implicitly null)", () => {
  const { symbol: _symbol, ...withoutSymbol } = EMPTY_WORKSPACE_IDENTITY;
  assert.equal(parseWorkspaceIdentityPayload(withoutSymbol), null);
});

test("I: parseWorkspaceIdentityPayload rejects a wrong-typed field value", () => {
  assert.equal(parseWorkspaceIdentityPayload({ ...EMPTY_WORKSPACE_IDENTITY, symbol: 123 }), null);
  assert.equal(parseWorkspaceIdentityPayload({ ...EMPTY_WORKSPACE_IDENTITY, symbol: undefined }), null);
});

test("I: parseWorkspaceIdentityPayload rejects an executionDomain value outside the closed enum", () => {
  assert.equal(parseWorkspaceIdentityPayload({ ...EMPTY_WORKSPACE_IDENTITY, executionDomain: "live_armed" }), null);
  assert.equal(parseWorkspaceIdentityPayload({ ...EMPTY_WORKSPACE_IDENTITY, executionDomain: "" }), null);
  for (const value of ["backtest", "paper", "live", null] as const) {
    assert.ok(parseWorkspaceIdentityPayload({ ...EMPTY_WORKSPACE_IDENTITY, executionDomain: value }) !== null);
  }
});

test("positive: a fully populated identity patch round-trips every field exactly", () => {
  const full: WorkspaceIdentity = {
    symbol: "AAPL",
    timeframe: "5m",
    strategyId: "LIQ-01",
    runId: "run-abc123",
    backtestJobId: "job-1",
    artifactId: "artifact-1",
    evaluationSlice: "oos-fold-3",
    executionDomain: "backtest",
  };
  const identity = mergeWorkspaceIdentity(EMPTY_WORKSPACE_IDENTITY, full);
  assert.deepEqual(identity, full);
});

// ---------------------------------------------------------------------------
// J — parsePanelLinkStatePayload: WAVE-02-FINAL-REPAIR-01 R3 reattach payload
// validation. Unlike parseWorkspaceIdentityPayload, this never returns null:
// any malformed/foreign shape fails closed to initialPanelLinkState()
// (unpinned/Linked) — the safe default a brand-new panel already starts in.
// ---------------------------------------------------------------------------

test("J: parsePanelLinkStatePayload accepts a well-formed pinned payload", () => {
  const identity = mergeWorkspaceIdentity(EMPTY_WORKSPACE_IDENTITY, { symbol: "AAPL" });
  const result = parsePanelLinkStatePayload({ pinned: true, pinnedIdentity: identity });
  assert.deepEqual(result, pinPanel(identity));
});

test("J: parsePanelLinkStatePayload accepts a well-formed unpinned payload", () => {
  const result = parsePanelLinkStatePayload({ pinned: false, pinnedIdentity: EMPTY_WORKSPACE_IDENTITY });
  assert.deepEqual(result, initialPanelLinkState());
});

test("J: parsePanelLinkStatePayload ignores pinnedIdentity when pinned is false, even if it's garbage", () => {
  const result = parsePanelLinkStatePayload({ pinned: false, pinnedIdentity: { armed: true } });
  assert.deepEqual(result, initialPanelLinkState());
});

test("J: parsePanelLinkStatePayload fails closed to unpinned on a non-object, null, or missing/extra key", () => {
  assert.deepEqual(parsePanelLinkStatePayload(null), initialPanelLinkState());
  assert.deepEqual(parsePanelLinkStatePayload(undefined), initialPanelLinkState());
  assert.deepEqual(parsePanelLinkStatePayload("pinned"), initialPanelLinkState());
  assert.deepEqual(parsePanelLinkStatePayload({ pinned: true }), initialPanelLinkState(), "missing pinnedIdentity");
  assert.deepEqual(
    parsePanelLinkStatePayload({ pinned: true, pinnedIdentity: EMPTY_WORKSPACE_IDENTITY, armed: true }),
    initialPanelLinkState(),
    "an extra authority-shaped key must void the whole payload, not just be dropped",
  );
});

test("J: parsePanelLinkStatePayload fails closed to unpinned on a non-boolean pinned or an invalid pinnedIdentity", () => {
  assert.deepEqual(parsePanelLinkStatePayload({ pinned: "true", pinnedIdentity: EMPTY_WORKSPACE_IDENTITY }), initialPanelLinkState());
  assert.deepEqual(
    parsePanelLinkStatePayload({ pinned: true, pinnedIdentity: { ...EMPTY_WORKSPACE_IDENTITY, armed: true } }),
    initialPanelLinkState(),
    "an invalid nested identity must not leave the panel pinned to garbage",
  );
});
