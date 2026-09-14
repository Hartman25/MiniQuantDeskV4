import test from "node:test";
import assert from "node:assert/strict";
import {
  INITIAL_SYNC_REVISION,
  isNewerRevision,
  nextLocalRevision,
  parseWorkspaceSyncMessage,
  type WorkspaceSyncRevision,
} from "../workspaceSync.ts";
import { EMPTY_WORKSPACE_IDENTITY, mergeWorkspaceIdentity } from "../workspaceModel.ts";

test("parseWorkspaceSyncMessage accepts a well-formed message", () => {
  const identity = mergeWorkspaceIdentity(EMPTY_WORKSPACE_IDENTITY, { symbol: "AAPL" });
  const revision: WorkspaceSyncRevision = { counter: 1, origin: "control" };
  const msg = parseWorkspaceSyncMessage({ revision, identity });
  assert.deepEqual(msg, { revision, identity });
});

test("parseWorkspaceSyncMessage fails closed on a non-object, missing/malformed revision, or bad identity", () => {
  assert.equal(parseWorkspaceSyncMessage(null), null);
  assert.equal(parseWorkspaceSyncMessage("not an object"), null);
  assert.equal(parseWorkspaceSyncMessage({ identity: EMPTY_WORKSPACE_IDENTITY }), null, "missing revision");
  assert.equal(parseWorkspaceSyncMessage({ revision: 42, identity: EMPTY_WORKSPACE_IDENTITY }), null, "revision is a bare number, not a {counter, origin} tuple");
  assert.equal(parseWorkspaceSyncMessage({ revision: { counter: "1", origin: "control" }, identity: EMPTY_WORKSPACE_IDENTITY }), null, "string counter");
  assert.equal(parseWorkspaceSyncMessage({ revision: { counter: NaN, origin: "control" }, identity: EMPTY_WORKSPACE_IDENTITY }), null, "NaN counter");
  assert.equal(parseWorkspaceSyncMessage({ revision: { counter: 1, origin: 42 }, identity: EMPTY_WORKSPACE_IDENTITY }), null, "non-string origin");
  assert.equal(parseWorkspaceSyncMessage({ revision: { counter: 1, origin: "" }, identity: EMPTY_WORKSPACE_IDENTITY }), null, "empty-string origin");
  assert.equal(parseWorkspaceSyncMessage({ revision: { counter: 1 }, identity: EMPTY_WORKSPACE_IDENTITY }), null, "missing origin");
  assert.equal(
    parseWorkspaceSyncMessage({ revision: { counter: 1, origin: "control", armed: true }, identity: EMPTY_WORKSPACE_IDENTITY }),
    null,
    "an extra authority-shaped key on the revision itself must void the whole message",
  );
  assert.equal(parseWorkspaceSyncMessage({ revision: { counter: 1, origin: "control" }, identity: null }), null, "null identity");
  assert.equal(
    parseWorkspaceSyncMessage({ revision: { counter: 1, origin: "control" }, identity: { ...EMPTY_WORKSPACE_IDENTITY, armed: true } }),
    null,
    "identity with a smuggled authority-shaped key",
  );
});

// ---------------------------------------------------------------------------
// WAVE-02-FINAL-REPAIR-01 R5: total order over (counter, origin).
// ---------------------------------------------------------------------------

test("isNewerRevision: a strictly greater counter always wins regardless of origin", () => {
  assert.equal(isNewerRevision({ counter: 5, origin: "a" }, { counter: 4, origin: "z" }), true);
  assert.equal(isNewerRevision({ counter: 4, origin: "z" }, { counter: 5, origin: "a" }), false, "a lower counter must never override a higher one, even with a lexicographically-winning origin");
});

test("isNewerRevision: on a counter tie, origin breaks it lexicographically", () => {
  assert.equal(isNewerRevision({ counter: 5, origin: "b" }, { counter: 5, origin: "a" }), true, "'b' > 'a'");
  assert.equal(isNewerRevision({ counter: 5, origin: "a" }, { counter: 5, origin: "b" }), false, "'a' < 'b'");
});

test("isNewerRevision: an identical revision (same counter AND origin) is never newer than itself — self-echo is a no-op", () => {
  assert.equal(isNewerRevision({ counter: 5, origin: "control" }, { counter: 5, origin: "control" }), false);
});

test("nextLocalRevision always exceeds the observed max counter by exactly 1, stamped with the given origin", () => {
  assert.deepEqual(nextLocalRevision(0, "control"), { counter: 1, origin: "control" });
  assert.deepEqual(nextLocalRevision(100, "execution"), { counter: 101, origin: "execution" });
});

test("INITIAL_SYNC_REVISION's origin sorts below any real window label, so the first real write from any window is unambiguously newer", () => {
  assert.equal(isNewerRevision({ counter: 0, origin: "control" }, INITIAL_SYNC_REVISION), true);
});

// ---------------------------------------------------------------------------
// Negative controls from the mission: simulate two windows (A, B) applying
// the SAME comparator independently and prove they converge — this is what
// isNewerRevision/nextLocalRevision must guarantee once wired into
// WorkspaceContext.tsx's single revisionRef per window.
// ---------------------------------------------------------------------------

function applyIfNewer(
  current: { revision: WorkspaceSyncRevision; value: string },
  incoming: { revision: WorkspaceSyncRevision; value: string },
): { revision: WorkspaceSyncRevision; value: string } {
  return isNewerRevision(incoming.revision, current.revision) ? incoming : current;
}

test("A and B start at the same revision and write concurrently with the same counter — both converge to the identical deterministic winner", () => {
  const start: WorkspaceSyncRevision = { counter: 5, origin: "" };
  let a = { revision: start, value: "start" };
  let b = { revision: start, value: "start" };

  // A writes AAPL, B writes MSFT — both compute the same next counter (6)
  // independently, before either has seen the other's message.
  const aWrite = { revision: nextLocalRevision(a.revision.counter, "control"), value: "AAPL" };
  const bWrite = { revision: nextLocalRevision(b.revision.counter, "execution"), value: "MSFT" };
  a = aWrite;
  b = bWrite;
  assert.equal(a.revision.counter, b.revision.counter, "both windows independently landed on the same counter");
  assert.notEqual(a.value, b.value, "precondition: they actually diverged locally before exchanging messages");

  // Exchange messages: each window evaluates the other's broadcast against its own current state.
  const aAfterExchange = applyIfNewer(a, bWrite);
  const bAfterExchange = applyIfNewer(b, aWrite);

  assert.equal(aAfterExchange.value, bAfterExchange.value, "both windows must converge to the identical value");
  assert.deepEqual(aAfterExchange.revision, bAfterExchange.revision);
  // "execution" > "control" lexicographically, so MSFT (B's write) is the deterministic winner.
  assert.equal(aAfterExchange.value, "MSFT");

  // Then the loser window (A, whose AAPL lost) writes again.
  const aSecondWrite = { revision: nextLocalRevision(aAfterExchange.revision.counter, "control"), value: "TSLA" };
  assert.ok(aSecondWrite.revision.counter > aAfterExchange.revision.counter, "the loser's next write advances strictly past the observed max counter");

  const bAfterSecondWrite = applyIfNewer(bAfterExchange, aSecondWrite);
  const aAfterSecondWrite = applyIfNewer(aAfterExchange, aSecondWrite);
  assert.equal(bAfterSecondWrite.value, "TSLA");
  assert.equal(aAfterSecondWrite.value, "TSLA");
  assert.deepEqual(bAfterSecondWrite.revision, aAfterSecondWrite.revision, "both windows converge to the later value with an identical revision");
});

test("a stale/lower revision can never override a newer value, even after exchange", () => {
  const winner = { revision: { counter: 10, origin: "control" }, value: "WINNER" };
  const stale = { revision: { counter: 3, origin: "execution" }, value: "STALE" };
  assert.deepEqual(applyIfNewer(winner, stale), winner);
});
