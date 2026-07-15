// DAILY-DATA-READINESS-01D-GUI-01: static source-text proof for
// DailyDataReadinessPanel.tsx and its wiring in IngestScreen.tsx.
//
// This repo's GUI test harness (tsx --test) does not render React
// components (no jsdom/testing-library configured) — the same constraint
// already applies to every other GUI feature test in this repo (see
// strategyScanner/__tests__/screenSource.test.ts). This file greps source
// text for the required safety strings and the absence of any mutating
// route or submission-function reference in the readiness panel.

import test from "node:test";
import assert from "node:assert/strict";
import { readFileSync } from "node:fs";
import { fileURLToPath } from "node:url";
import { dirname, join } from "node:path";

const __dirname = dirname(fileURLToPath(import.meta.url));
const panelSource = readFileSync(join(__dirname, "..", "DailyDataReadinessPanel.tsx"), "utf-8");
const screenSource = readFileSync(join(__dirname, "..", "IngestScreen.tsx"), "utf-8");
const apiSource = readFileSync(join(__dirname, "..", "api.ts"), "utf-8");

test("api source declares the dedicated daily data readiness route", () => {
  assert.ok(apiSource.includes('DAILY_DATA_READINESS_ROUTE = "/api/v1/market-data/readiness"'));
});

test("fetchDailyDataReadiness uses the GET helper, not postJson", () => {
  const fnStart = apiSource.indexOf("export async function fetchDailyDataReadiness");
  assert.ok(fnStart >= 0, "fetchDailyDataReadiness must exist in api.ts");
  const fnEnd = apiSource.indexOf("\n}", fnStart);
  const fnBody = apiSource.slice(fnStart, fnEnd);
  assert.ok(fnBody.includes("fetchJsonCandidate"));
  assert.ok(!fnBody.includes("postJson"));
});

test("panel contains Refresh and Copy diagnostics controls", () => {
  assert.ok(panelSource.includes("Refresh"));
  assert.ok(panelSource.includes("Copy diagnostics"));
});

test("panel contains the configuration-preview warning verbatim", () => {
  assert.ok(
    panelSource.includes(
      "This is a configuration preview. Runtime start re-evaluates readiness using the exact",
    ),
  );
  assert.ok(panelSource.includes("start-attempt binding."));
});

test("panel does not import postJson or any ingest/scheduler/runtime submission function", () => {
  const forbiddenImports = [
    "postJson",
    "submitIngestJob",
    "cancelIngestJob",
    "pollMarketDataFeedOnce",
    "startMarketDataFeedScheduler",
    "stopMarketDataFeedScheduler",
  ];
  for (const term of forbiddenImports) {
    assert.ok(
      !panelSource.includes(term),
      `DailyDataReadinessPanel.tsx must not reference '${term}'`,
    );
  }
});

test("panel does not reference any mutating or execution route", () => {
  const forbiddenRoutes = [
    "/v1/run/start",
    "/v1/run/stop",
    "/v1/run/halt",
    "/v1/integrity/arm",
    "/v1/integrity/disarm",
    "/api/v1/strategy/signal",
    "/api/v1/execution/orders",
    "/api/v1/ingest/jobs",
    "/api/v1/market-data/feed/poll-once",
    "/api/v1/market-data/feed/scheduler/start",
    "/api/v1/market-data/feed/scheduler/stop",
  ];
  for (const route of forbiddenRoutes) {
    assert.ok(
      !panelSource.includes(route),
      `DailyDataReadinessPanel.tsx must not reference forbidden route: '${route}'`,
    );
  }
});

test("panel never calls fetch/postJson directly — it only renders data passed in as props", () => {
  assert.ok(!panelSource.includes("fetch("));
  assert.ok(!panelSource.includes("await fetch"));
});

test("IngestScreen wires the readiness refresh callback to fetchDailyDataReadiness only, no ingest submission", () => {
  const loaderStart = screenSource.indexOf("const loadDailyDataReadiness = useCallback");
  assert.ok(loaderStart >= 0, "loadDailyDataReadiness must exist in IngestScreen.tsx");
  const loaderEnd = screenSource.indexOf("}, []);", loaderStart);
  const loaderBody = screenSource.slice(loaderStart, loaderEnd);
  assert.ok(loaderBody.includes("fetchDailyDataReadiness"));
  assert.ok(!loaderBody.includes("submitIngestJob"));
  assert.ok(!loaderBody.includes("postJson"));
  assert.ok(!loaderBody.includes("pollMarketDataFeedOnce"));
  assert.ok(!loaderBody.includes("startMarketDataFeedScheduler"));
});

test("IngestScreen mounts DailyDataReadinessPanel with a read-only onRefresh prop only", () => {
  assert.ok(screenSource.includes("<DailyDataReadinessPanel"));
  const mountStart = screenSource.indexOf("<DailyDataReadinessPanel");
  const mountEnd = screenSource.indexOf("/>", mountStart);
  const mountBlock = screenSource.slice(mountStart, mountEnd);
  assert.ok(mountBlock.includes("onRefresh={() => void loadDailyDataReadiness()}"));
});

test("no automatic mutation is tied to readiness state: panel has no useEffect", () => {
  assert.ok(!panelSource.includes("useEffect"));
});

test("panel does not directly concatenate a nullable numeric field with a bare 's' suffix", () => {
  const forbiddenPatterns = [
    /\{response\.configured_grace_seconds\}s/,
    /\{response\.configured_future_skew_seconds\}s/,
    /\{assignment\.effective_grace_seconds\}s/,
    /\{assignment\.effective_future_skew_seconds\}s/,
    /\{assignment\.effective_runtime_timeframe_secs\s*\?\?\s*"unknown"\}/,
    /\{assignment\.required_history_bars\s*\?\?\s*"unknown"\}/,
    /\{assignment\.loaded_completed_bars\s*\?\?\s*"unknown"\}/,
  ];
  for (const pattern of forbiddenPatterns) {
    assert.ok(
      !pattern.test(panelSource),
      `DailyDataReadinessPanel.tsx must not render a nullable numeric field with pattern: ${pattern}`,
    );
  }
});

test("Daily Data Readiness normalizer never fabricates a zero for missing/malformed numeric evidence", () => {
  const sectionStart = apiSource.indexOf(
    "// DAILY-DATA-READINESS-01D-GUI-01: Daily data readiness (read-only,",
  );
  assert.ok(sectionStart >= 0, "Daily Data Readiness normalization section must exist in api.ts");
  const sectionEnd = apiSource.indexOf(
    "export function formatReadinessNumber",
    sectionStart,
  );
  assert.ok(sectionEnd >= 0, "formatReadinessNumber must exist after the Daily Data Readiness normalization functions");
  const section = apiSource.slice(sectionStart, sectionEnd);
  const fabricatedZeroPattern = /normalizeNullableNumber\([^)]*\)\s*\?\?\s*0/;
  assert.ok(
    !fabricatedZeroPattern.test(section),
    "Daily Data Readiness normalization must not fall back missing/malformed numeric evidence to 0",
  );
});

test("panel renders nullable numeric readiness fields through the shared formatter", () => {
  assert.ok(
    panelSource.includes("formatReadinessNumber"),
    "DailyDataReadinessPanel.tsx must import and use formatReadinessNumber for nullable numeric display",
  );
  assert.ok(panelSource.includes('import {') && /formatReadinessNumber/.test(panelSource));
});
