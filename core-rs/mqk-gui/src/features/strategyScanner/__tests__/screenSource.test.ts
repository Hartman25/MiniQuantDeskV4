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

// STRATEGY-SCANNER-PROMOTION-01D: review-queue warnings, required whenever
// the review-artifact panel displays a result.
test("screen source contains every required promotion-review warning string", () => {
  assert.ok(screenSource.includes("promotion-ready is not trading-approved."));
  assert.ok(screenSource.includes("paper_candidate is not autonomous trading approval."));
  assert.ok(screenSource.includes("A separate paper-promotion patch is required before any paper trading."));
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

test("screen/api source only calls strategy-scans/strategy-promotions routes, never order/execution/strategy-signal routes", () => {
  const combined = `${screenSource}\n${apiSource}`;
  const forbiddenRoutes = [
    "/api/v1/execution/orders",
    "/api/v1/strategy/signal",
    "/api/v1/ops/action",
    "/v1/run/start",
    "/v1/run/stop",
    "/v1/run/halt",
    "/v1/integrity/arm",
    "/v1/integrity/disarm",
  ];
  for (const route of forbiddenRoutes) {
    assert.ok(
      !combined.includes(route),
      `strategy scanner GUI must never reference forbidden route: '${route}'`,
    );
  }
  assert.ok(apiSource.includes("/api/v1/strategy-scans/jobs"));
  assert.ok(apiSource.includes("/api/v1/strategy-scans/artifact"));
  assert.ok(apiSource.includes("/api/v1/strategy-scans/review-artifact"));
});

// STRATEGY-PROMOTION-REGISTRY-01E: promotion control panel proof.
test("screen/api source references every promotion route and no forbidden trade/live term", () => {
  assert.ok(apiSource.includes("/api/v1/strategy/promotions"));
  assert.ok(apiSource.includes("/api/v1/strategy/promotions/history"));
  assert.ok(apiSource.includes("/api/v1/strategy/promotions/check"));
  assert.ok(apiSource.includes("/api/v1/strategy/promotions/transition"));

  const forbidden = [
    "tradable_live: true",
    "live_trading_enabled",
    "approved_for_live",
    "recommended_for_live",
  ];
  for (const term of forbidden) {
    assert.ok(
      !screenSource.includes(term),
      `screen source must not contain forbidden live-authorization text: '${term}'`,
    );
  }
});

test("screen source contains the paper-vs-live boundary warning", () => {
  assert.ok(
    screenSource.includes("Paper promotion never grants live authorization"),
  );
});

test("screen source requires explicit confirmation for active_paper/demoted/retired transitions", () => {
  assert.ok(screenSource.includes("CONFIRM_REQUIRED_STATES"));
  assert.ok(screenSource.includes('"active_paper", "demoted", "retired"'));
  assert.ok(screenSource.includes("requiresConfirmation"));
});

test("screen source gates initial approval on a selected paper_candidate row", () => {
  assert.ok(screenSource.includes("initialApprovalBlocked"));
  assert.ok(screenSource.includes('review_state === "paper_candidate"'));
});
