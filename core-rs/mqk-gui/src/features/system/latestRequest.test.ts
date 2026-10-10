import test from "node:test";
import assert from "node:assert/strict";
import { LatestRequest } from "./latestRequest";
import { createReadClient, fetchJsonCandidate } from "./http";

function deferred<T>() {
  let resolve!: (value: T) => void;
  let reject!: (error: unknown) => void;
  const promise = new Promise<T>((yes, no) => { resolve = yes; reject = no; });
  return { promise, resolve, reject };
}

test("latest observation wins across late success, late error and unmount invalidation", async () => {
  const gate = new LatestRequest();
  const paper = deferred<string>();
  const live = deferred<string>();
  const published: string[] = [];
  const failed: unknown[] = [];
  const old = gate.run(() => paper.promise, (value) => published.push(value), (error) => failed.push(error));
  const next = gate.run(() => live.promise, (value) => published.push(value), (error) => failed.push(error));
  live.resolve("Live unavailable"); await next;
  paper.resolve("Paper ready"); await old;
  assert.deepEqual(published, ["Live unavailable"]);
  const pending = deferred<string>();
  const abandoned = gate.run(() => pending.promise, (value) => published.push(value), (error) => failed.push(error));
  gate.invalidate(); pending.reject(new Error("late failure")); await abandoned;
  assert.deepEqual(failed, []);
  assert.equal(gate.busy, false);
  await gate.run(() => Promise.reject("current failure"), () => assert.fail(), (error) => failed.push(error));
  assert.deepEqual(failed, ["current failure"]);
});

test("read client pins daemon identity and GET requests have bounded timeout", async () => {
  const original = globalThis.fetch;
  const urls: string[] = [];
  globalThis.fetch = (async (input, init) => {
    urls.push(String(input));
    assert.equal(init?.method, "GET");
    if (String(input).endsWith("/hang")) return new Promise((_resolve, reject) => {
      init?.signal?.addEventListener("abort", () => reject(new Error("request timed out")), { once: true });
    });
    return Response.json({ data: "known" });
  }) as typeof fetch;
  try {
    const client = createReadClient("http://paper.example:1234");
    await client.fetchJsonCandidate("/first");
    await client.fetchJsonCandidates(["/second"]);
    assert.deepEqual(urls, ["http://paper.example:1234/first", "http://paper.example:1234/second"]);
    const result = await fetchJsonCandidate("/hang", { baseUrl: "http://paper.example:1234", timeoutMs: 5 });
    assert.equal(result.ok, false);
    assert.match(result.error!, /timed out/);
  } finally { globalThis.fetch = original; }
});
