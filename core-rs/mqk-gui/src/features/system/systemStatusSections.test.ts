import test from "node:test";
import assert from "node:assert/strict";
import {
  SYSTEM_STATUS_SECTION_IDS,
  systemStatusScreenIncludes,
} from "./systemStatusSections.ts";

test("System/Status sections include registry-v2 source status and asset capability matrix", () => {
  assert.ok(systemStatusScreenIncludes("instrument-registry-v2-source"));
  assert.ok(systemStatusScreenIncludes("asset-capability-matrix"));
  assert.ok(systemStatusScreenIncludes("runtime-opportunity-allocation"));
  assert.ok(systemStatusScreenIncludes("strategy-conflict-policy"));
  assert.ok(systemStatusScreenIncludes("dynamic-selection-evidence"));
  assert.deepEqual(
    [...SYSTEM_STATUS_SECTION_IDS],
    [
      "operations-metadata",
      "instrument-registry-v2-source",
      "asset-capability-matrix",
      "runtime-opportunity-allocation",
      "strategy-conflict-policy",
      "dynamic-selection-evidence",
    ],
  );
});
