import test from "node:test";
import assert from "node:assert/strict";
import { formatDailyOperationCount } from "../formatDailyOperationCount.ts";

// F1.7 / F1.11 item 11: null must never render as "0" or any other implied-zero copy.
test("formatDailyOperationCount renders null as Unavailable, never as zero", () => {
  assert.equal(formatDailyOperationCount(null), "Unavailable");
});

// F1.11 item 12: a real numeric zero is authoritative and renders as "0".
test("formatDailyOperationCount renders a real zero as 0", () => {
  assert.equal(formatDailyOperationCount(0), "0");
});

test("formatDailyOperationCount renders a positive count verbatim", () => {
  assert.equal(formatDailyOperationCount(7), "7");
});
