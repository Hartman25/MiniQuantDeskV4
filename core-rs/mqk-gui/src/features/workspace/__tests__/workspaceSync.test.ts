import test from "node:test";
import assert from "node:assert/strict";
import {
  INITIAL_SYNC_REVISION,
  WORKSPACE_SYNC_EVENT,
  WORKSPACE_SYNC_REQUEST_EVENT,
  installWorkspaceSyncHandshake,
  isNewerRevision,
  nextLocalRevision,
  parseWorkspaceSyncMessage,
  parseWorkspaceSyncRequest,
  type WorkspaceSyncHandshakeState,
  type WorkspaceSyncRevision,
  type WorkspaceSyncTransport,
} from "../workspaceSync.ts";
import {
  EMPTY_WORKSPACE_IDENTITY,
  initialPanelLinkState,
  mergeWorkspaceIdentity,
  pinPanel,
  resolvePanelIdentity,
  unpinPanel,
  type WorkspaceIdentity,
} from "../workspaceModel.ts";

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

// ---------------------------------------------------------------------------
// R1 — GUI-WORKSPACE-LATE-JOIN-SYNC-01: request schema.
// ---------------------------------------------------------------------------

test("parseWorkspaceSyncRequest accepts a well-formed {requester} request", () => {
  assert.deepEqual(parseWorkspaceSyncRequest({ requester: "panel-marketData" }), { requester: "panel-marketData" });
});

test("parseWorkspaceSyncRequest fails closed on non-object, missing/empty/non-string requester, or any extra key", () => {
  assert.equal(parseWorkspaceSyncRequest(null), null);
  assert.equal(parseWorkspaceSyncRequest("not an object"), null);
  assert.equal(parseWorkspaceSyncRequest({}), null, "missing requester");
  assert.equal(parseWorkspaceSyncRequest({ requester: "" }), null, "empty requester");
  assert.equal(parseWorkspaceSyncRequest({ requester: 42 }), null, "non-string requester");
  assert.equal(parseWorkspaceSyncRequest({ requester: "B", armed: true }), null, "an extra authority-shaped key must void the whole request");
  assert.equal(
    parseWorkspaceSyncRequest({ requester: "B", identity: EMPTY_WORKSPACE_IDENTITY }),
    null,
    "identity must never be embedded in a request",
  );
});

// ---------------------------------------------------------------------------
// R1 — GUI-WORKSPACE-LATE-JOIN-SYNC-01: installWorkspaceSyncHandshake,
// exercised through a fake in-memory event bus standing in for the real
// Tauri global-event transport. Each `installWorkspaceSyncHandshake(...)`
// call below models one window's WorkspaceProvider mount.
// ---------------------------------------------------------------------------

function createFakeBus(): { transport: () => WorkspaceSyncTransport } {
  const listeners = new Map<string, Set<(payload: unknown) => void>>();
  return {
    transport(): WorkspaceSyncTransport {
      return {
        listen: async (event, handler) => {
          let set = listeners.get(event);
          if (!set) {
            set = new Set();
            listeners.set(event, set);
          }
          set.add(handler);
          return () => {
            set?.delete(handler);
          };
        },
        emit: async (event, payload) => {
          const set = listeners.get(event);
          if (!set) return;
          for (const handler of [...set]) handler(payload);
        },
      };
    },
  };
}

interface FakeWindowState {
  revision: WorkspaceSyncRevision;
  identity: WorkspaceIdentity;
}

function installFakeWindow(
  bus: { transport: () => WorkspaceSyncTransport },
  origin: string,
  state: FakeWindowState,
  onRemoteState?: WorkspaceSyncHandshakeState["onRemoteState"],
): Promise<() => void> {
  return installWorkspaceSyncHandshake(bus.transport(), origin, {
    getRevision: () => state.revision,
    getIdentity: () => state.identity,
    onRemoteState:
      onRemoteState ??
      ((revision, identity) => {
        state.revision = revision;
        state.identity = identity;
      }),
  });
}

test("R1-LJ-01: a window that joins AFTER the current workspace identity was established converges without another operator change", async () => {
  const bus = createFakeBus();
  const identityAAPL = mergeWorkspaceIdentity(EMPTY_WORKSPACE_IDENTITY, { symbol: "AAPL", runId: "run-1", timeframe: "5m" });
  const aState: FakeWindowState = { revision: { counter: 3, origin: "A" }, identity: identityAAPL };
  await installFakeWindow(bus, "A", aState, () => {
    throw new Error("A must not receive any remote state in this scenario — it is the only window with prior identity");
  });

  const bState: FakeWindowState = { revision: INITIAL_SYNC_REVISION, identity: EMPTY_WORKSPACE_IDENTITY };
  await installFakeWindow(bus, "B", bState);

  assert.deepEqual(bState.identity, identityAAPL, "B converges to A's identity without A ever changing workspace context again");
  assert.deepEqual(bState.revision, aState.revision);
});

test("R1-LJ-02: multiple responders — requester converges to the highest (counter, origin) revision independent of registration/delivery order", async () => {
  async function scenario(order: "A-then-C" | "C-then-A") {
    const bus = createFakeBus();
    const aState: FakeWindowState = { revision: { counter: 5, origin: "A" }, identity: mergeWorkspaceIdentity(EMPTY_WORKSPACE_IDENTITY, { symbol: "AAPL" }) };
    const cState: FakeWindowState = { revision: { counter: 9, origin: "C" }, identity: mergeWorkspaceIdentity(EMPTY_WORKSPACE_IDENTITY, { symbol: "MSFT" }) };
    if (order === "A-then-C") {
      await installFakeWindow(bus, "A", aState, () => {});
      await installFakeWindow(bus, "C", cState, () => {});
    } else {
      await installFakeWindow(bus, "C", cState, () => {});
      await installFakeWindow(bus, "A", aState, () => {});
    }
    const bState: FakeWindowState = { revision: INITIAL_SYNC_REVISION, identity: EMPTY_WORKSPACE_IDENTITY };
    await installFakeWindow(bus, "B", bState);
    return { bState, cState };
  }

  const first = await scenario("A-then-C");
  assert.deepEqual(first.bState.identity, first.cState.identity, "C's revision (9) beats A's (5) regardless of arrival order");
  assert.deepEqual(first.bState.revision, first.cState.revision);

  const second = await scenario("C-then-A");
  assert.deepEqual(second.bState.identity, second.cState.identity);
  assert.deepEqual(second.bState.revision, second.cState.revision);
});

test("R1-LJ-03: a newer local write beats a stale late-arriving snapshot", async () => {
  const bus = createFakeBus();
  const bState: FakeWindowState = { revision: INITIAL_SYNC_REVISION, identity: EMPTY_WORKSPACE_IDENTITY };
  await installFakeWindow(bus, "B", bState);

  // B makes its own newer local write, exactly as WorkspaceContext.tsx's
  // setLinked/broadcast would (bump past the observed max counter).
  const newerIdentity = mergeWorkspaceIdentity(EMPTY_WORKSPACE_IDENTITY, { symbol: "TSLA" });
  bState.revision = nextLocalRevision(bState.revision.counter, "B");
  bState.identity = newerIdentity;

  // A stale snapshot (e.g. a slow responder answering an old request) arrives with a lower counter than B now has.
  await bus.transport().emit(WORKSPACE_SYNC_EVENT, {
    revision: { counter: 0, origin: "A" },
    identity: mergeWorkspaceIdentity(EMPTY_WORKSPACE_IDENTITY, { symbol: "STALE" }),
  });

  assert.deepEqual(bState.identity, newerIdentity, "the stale snapshot must not overwrite B's newer local write");
});

test("R1-LJ-04: a malformed sync-request is rejected and produces no response", async () => {
  const bus = createFakeBus();
  const aState: FakeWindowState = { revision: { counter: 3, origin: "A" }, identity: mergeWorkspaceIdentity(EMPTY_WORKSPACE_IDENTITY, { symbol: "AAPL" }) };
  await installFakeWindow(bus, "A", aState, () => {
    throw new Error("A must not apply anything from this scenario");
  });

  let responseCount = 0;
  await bus.transport().listen(WORKSPACE_SYNC_EVENT, () => {
    responseCount += 1;
  });

  const malformedRequests: unknown[] = [null, "not an object", {}, { requester: "" }, { requester: 42 }, { requester: "B", armed: true }];
  for (const payload of malformedRequests) {
    await bus.transport().emit(WORKSPACE_SYNC_REQUEST_EVENT, payload);
  }

  assert.equal(responseCount, 0, "no malformed request may trigger a response");
});

test("R1-LJ-05: a malformed WORKSPACE_SYNC_EVENT delivered through the handshake's own listener is rejected (existing fail-closed behavior stays intact)", async () => {
  const bus = createFakeBus();
  const bState: FakeWindowState = { revision: INITIAL_SYNC_REVISION, identity: EMPTY_WORKSPACE_IDENTITY };
  await installFakeWindow(bus, "B", bState, () => {
    throw new Error("a malformed message must never reach onRemoteState");
  });

  await bus.transport().emit(WORKSPACE_SYNC_EVENT, { revision: { counter: 1, origin: "A", armed: true }, identity: EMPTY_WORKSPACE_IDENTITY });
  await bus.transport().emit(WORKSPACE_SYNC_EVENT, "not an object");
  await bus.transport().emit(WORKSPACE_SYNC_EVENT, { revision: { counter: 1, origin: "A" }, identity: { ...EMPTY_WORKSPACE_IDENTITY, armed: true } });

  assert.deepEqual(bState.identity, EMPTY_WORKSPACE_IDENTITY, "state must remain untouched by any malformed message");
});

test("R1-LJ-06: a window never responds to its own request, and a request never triggers another request (no loop)", async () => {
  const bus = createFakeBus();
  let responseCount = 0;
  let requestCount = 0;
  await bus.transport().listen(WORKSPACE_SYNC_EVENT, () => {
    responseCount += 1;
  });
  await bus.transport().listen(WORKSPACE_SYNC_REQUEST_EVENT, () => {
    requestCount += 1;
  });

  const aState: FakeWindowState = { revision: { counter: 1, origin: "A" }, identity: EMPTY_WORKSPACE_IDENTITY };
  await installFakeWindow(bus, "A", aState, () => {});

  assert.equal(requestCount, 1, "exactly one request was emitted at mount — no request-response loop generated more");
  assert.equal(responseCount, 0, "A must not answer its own request (the only listener present is A's own)");
});

test("R1-LJ-07: a late-joining UNPINNED detached panel resolves to the synchronized global identity", async () => {
  const bus = createFakeBus();
  const globalIdentity = mergeWorkspaceIdentity(EMPTY_WORKSPACE_IDENTITY, { symbol: "AAPL", timeframe: "5m" });
  const controlState: FakeWindowState = { revision: { counter: 2, origin: "control" }, identity: globalIdentity };
  await installFakeWindow(bus, "control", controlState, () => {});

  // App.tsx wraps every entry point — docked or detached — in the same
  // WorkspaceProvider, so a detached panel window's provider runs the exact
  // same mount-time handshake as any other window.
  const panelState: FakeWindowState = { revision: INITIAL_SYNC_REVISION, identity: EMPTY_WORKSPACE_IDENTITY };
  await installFakeWindow(bus, "panel-marketData", panelState);

  const resolved = resolvePanelIdentity(panelState.identity, initialPanelLinkState());
  assert.deepEqual(resolved, globalIdentity, "an unpinned panel always mirrors the (now-synchronized) global identity");
});

test("R1-LJ-08: a PINNED detached panel stays frozen while its provider synchronizes in the background, then resolves to the synced identity on unpin", async () => {
  const bus = createFakeBus();
  const globalIdentity = mergeWorkspaceIdentity(EMPTY_WORKSPACE_IDENTITY, { symbol: "MSFT", runId: "run-42" });
  const controlState: FakeWindowState = { revision: { counter: 4, origin: "control" }, identity: globalIdentity };
  await installFakeWindow(bus, "control", controlState, () => {});

  const panelState: FakeWindowState = { revision: INITIAL_SYNC_REVISION, identity: EMPTY_WORKSPACE_IDENTITY };
  await installFakeWindow(bus, "panel-backtestResults", panelState);

  // The provider's underlying linked identity DID converge...
  assert.deepEqual(panelState.identity, globalIdentity);

  // ...but a pinned panel ignores it and keeps showing its frozen identity.
  const frozenAtDetachTime = mergeWorkspaceIdentity(EMPTY_WORKSPACE_IDENTITY, { symbol: "OLD", runId: "run-1" });
  let linkState = pinPanel(frozenAtDetachTime);
  assert.deepEqual(resolvePanelIdentity(panelState.identity, linkState), frozenAtDetachTime, "pinned display stays frozen even though the provider already synchronized");

  // Unpinning immediately resolves to the already-synchronized global identity — no further sync round trip required.
  linkState = unpinPanel();
  assert.deepEqual(resolvePanelIdentity(panelState.identity, linkState), globalIdentity);
});

test("R1-LJ-09: a failing transport (simulating no Tauri context) rejects cleanly instead of hanging or half-installing", async () => {
  const failingTransport: WorkspaceSyncTransport = {
    listen: async () => {
      throw new Error("not in a Tauri context");
    },
    emit: async () => {
      throw new Error("not in a Tauri context");
    },
  };
  await assert.rejects(
    () =>
      installWorkspaceSyncHandshake(failingTransport, "browser-preview", {
        getRevision: () => INITIAL_SYNC_REVISION,
        getIdentity: () => EMPTY_WORKSPACE_IDENTITY,
        onRemoteState: () => {
          throw new Error("must never be called");
        },
      }),
    /not in a Tauri context/,
    "the handshake propagates the failure to its caller (WorkspaceContext.tsx's own try/catch), matching the existing browser-preview safety pattern — no false synchronization authority is manufactured",
  );
});
