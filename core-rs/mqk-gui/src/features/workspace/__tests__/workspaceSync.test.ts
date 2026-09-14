import test from "node:test";
import assert from "node:assert/strict";
import { isNewerRevision, nextRevision, parseWorkspaceSyncMessage } from "../workspaceSync.ts";
import { EMPTY_WORKSPACE_IDENTITY, mergeWorkspaceIdentity } from "../workspaceModel.ts";

test("parseWorkspaceSyncMessage accepts a well-formed message", () => {
  const identity = mergeWorkspaceIdentity(EMPTY_WORKSPACE_IDENTITY, { symbol: "AAPL" });
  const msg = parseWorkspaceSyncMessage({ revision: 42, identity });
  assert.deepEqual(msg, { revision: 42, identity });
});

test("parseWorkspaceSyncMessage fails closed on a non-object, missing/non-numeric revision, or bad identity", () => {
  assert.equal(parseWorkspaceSyncMessage(null), null);
  assert.equal(parseWorkspaceSyncMessage("not an object"), null);
  assert.equal(parseWorkspaceSyncMessage({ identity: EMPTY_WORKSPACE_IDENTITY }), null, "missing revision");
  assert.equal(parseWorkspaceSyncMessage({ revision: "42", identity: EMPTY_WORKSPACE_IDENTITY }), null, "string revision");
  assert.equal(parseWorkspaceSyncMessage({ revision: NaN, identity: EMPTY_WORKSPACE_IDENTITY }), null, "NaN revision");
  assert.equal(parseWorkspaceSyncMessage({ revision: 1, identity: null }), null, "null identity");
  assert.equal(
    parseWorkspaceSyncMessage({ revision: 1, identity: { ...EMPTY_WORKSPACE_IDENTITY, armed: true } }),
    null,
    "identity with a smuggled authority-shaped key",
  );
});

test("isNewerRevision: a strictly greater revision wins; equal or lower never overrides (stale/out-of-order protection)", () => {
  assert.equal(isNewerRevision(5, 4), true);
  assert.equal(isNewerRevision(5, 5), false, "a tie must not override — the currently-applied update keeps its state");
  assert.equal(isNewerRevision(4, 5), false, "a lower/stale revision must never override a newer one");
});

test("nextRevision is strictly greater than the current revision even if the wall clock hasn't advanced", () => {
  assert.equal(nextRevision(100, 50), 101, "clock behind current revision must still advance by at least 1");
  assert.equal(nextRevision(100, 100), 101, "clock equal to current revision must still advance by at least 1");
  assert.equal(nextRevision(100, 200), 200, "clock ahead of current revision is used directly");
});

test("nextRevision defaults to Date.now() when no explicit time is given, and always exceeds the current revision", () => {
  const rev = nextRevision(0);
  assert.ok(rev > 0);
  assert.ok(Number.isFinite(rev));
});
