// STRATEGY-SCANNER-JOBS-GUI-01D: static source-text proof for
// StrategyScannerScreen.tsx.
//
// This repo's GUI test harness (tsx --test) does not render React
// components (no jsdom/testing-library configured) — the same constraint
// already applies to every other GUI feature test in this repo. Mirroring
// the Rust scanner's own source-level safety proof
// (`scanner_source_does_not_import_broker_or_oms_write_types` in
// mqk-backtest), this file greps the screen's source text for the required
// warning strings and the absence of any trade/promote/approve control or
// order-submission route.

import test from "node:test";
import assert from "node:assert/strict";
import { readFileSync } from "node:fs";
import { fileURLToPath } from "node:url";
import { dirname, join } from "node:path";

const __dirname = dirname(fileURLToPath(import.meta.url));
const screenSource = readFileSync(
  join(__dirname, "..", "StrategyScannerScreen.tsx"),
  "utf-8",
);
const apiSource = readFileSync(join(__dirname, "..", "api.ts"), "utf-8");

test("screen source contains every required research-only warning string", () => {
  assert.ok(screenSource.includes("Scanner ranking is research evidence only."));
  assert.ok(screenSource.includes("Scanner output is not autonomous trading approval."));
  assert.ok(screenSource.includes("Candidates can rank well while still having negative absolute returns."));
});

test("screen source has no trade/promote/approve control", () => {
  const forbidden = [
    "Promote",
    "Approve",
    "Submit Order",
    "Place Order",
    "Buy ",
    "Sell ",
    "Trade Now",
    "recommended_for_live",
    "approved_for_live",
  ];
  for (const term of forbidden) {
    assert.ok(
      !screenSource.includes(term),
      `screen source must not contain forbidden trade-control text: '${term}'`,
    );
  }
});

test("screen/api source only calls strategy-scans routes, never order/execution/strategy-signal routes", () => {
  const combined = `${screenSource}\n${apiSource}`;
  const forbiddenRoutes = [
    "/api/v1/execution/orders",
    "/api/v1/strategy/signal",
    "/api/v1/ops/action",
    "/v1/run/start",
  ];
  for (const route of forbiddenRoutes) {
    assert.ok(
      !combined.includes(route),
      `strategy scanner GUI must never reference forbidden route: '${route}'`,
    );
  }
  assert.ok(apiSource.includes("/api/v1/strategy-scans/jobs"));
  assert.ok(apiSource.includes("/api/v1/strategy-scans/artifact"));
});
