// STRATEGY-SCANNER-JOBS-GUI-01D: pure-helper tests for the strategy scanner
// GUI feature. No HTTP calls — only tests functions that do not touch the
// network (submit/poll HTTP wrappers are exercised manually/by daemon
// scenario tests, matching the existing ingest/backtests GUI test pattern).

import test from "node:test";
import assert from "node:assert/strict";
import {
  artifactTruthLabel,
  buildActiveStrategyScanJob,
  buildStrategyScanJobRequest,
  isArtifactActive,
  isTerminalStrategyScanJobStatus,
  normalizeStrategyScanJobStatus,
} from "../api.ts";
import type { StrategyScanJobStatusResponse } from "../types.ts";

// 1. normalizeStrategyScanJobStatus maps known values and falls back to "unknown".
test("normalizeStrategyScanJobStatus maps known statuses", () => {
  assert.equal(normalizeStrategyScanJobStatus("queued"), "queued");
  assert.equal(normalizeStrategyScanJobStatus("running"), "running");
  assert.equal(normalizeStrategyScanJobStatus("completed"), "completed");
  assert.equal(normalizeStrategyScanJobStatus("failed"), "failed");
  assert.equal(normalizeStrategyScanJobStatus("something_else"), "unknown");
});

// 2. isTerminalStrategyScanJobStatus.
test("isTerminalStrategyScanJobStatus is true only for completed/failed", () => {
  assert.equal(isTerminalStrategyScanJobStatus("queued"), false);
  assert.equal(isTerminalStrategyScanJobStatus("running"), false);
  assert.equal(isTerminalStrategyScanJobStatus("completed"), true);
  assert.equal(isTerminalStrategyScanJobStatus("failed"), true);
  assert.equal(isTerminalStrategyScanJobStatus("unknown"), false);
});

// 3. isArtifactActive / artifactTruthLabel cover every documented truth_state.
test("isArtifactActive is true only for 'active'", () => {
  assert.equal(isArtifactActive("active"), true);
  assert.equal(isArtifactActive("missing_artifact"), false);
  assert.equal(isArtifactActive("invalid_artifact"), false);
  assert.equal(isArtifactActive("path_rejected"), false);
  assert.equal(isArtifactActive("read_failed"), false);
});

test("artifactTruthLabel gives a human label for every documented truth_state", () => {
  assert.equal(artifactTruthLabel("active"), "Active");
  assert.equal(artifactTruthLabel("missing_artifact"), "Missing artifact");
  assert.equal(artifactTruthLabel("invalid_artifact"), "Invalid artifact");
  assert.equal(artifactTruthLabel("path_rejected"), "Path rejected");
  assert.equal(artifactTruthLabel("read_failed"), "Read failed");
  assert.ok(artifactTruthLabel("something_new").includes("something_new"));
});

// 4. buildStrategyScanJobRequest — validation and defaulting.
test("buildStrategyScanJobRequest rejects blank timeframe", () => {
  const result = buildStrategyScanJobRequest({
    registryPath: "", barsRoot: "", timeframe: "   ", strategy: "swing_momentum",
    top: "20", limitSymbols: "", outDir: "",
  });
  assert.equal(result.ok, false);
});

test("buildStrategyScanJobRequest rejects blank strategy", () => {
  const result = buildStrategyScanJobRequest({
    registryPath: "", barsRoot: "", timeframe: "1D", strategy: "  ",
    top: "20", limitSymbols: "", outDir: "",
  });
  assert.equal(result.ok, false);
});

test("buildStrategyScanJobRequest rejects top out of bounds", () => {
  for (const top of ["0", "101", "abc"]) {
    const result = buildStrategyScanJobRequest({
      registryPath: "", barsRoot: "", timeframe: "1D", strategy: "swing_momentum",
      top, limitSymbols: "", outDir: "",
    });
    assert.equal(result.ok, false, `top=${top} should be rejected`);
  }
});

test("buildStrategyScanJobRequest rejects limit_symbols out of bounds", () => {
  for (const limitSymbols of ["0", "201"]) {
    const result = buildStrategyScanJobRequest({
      registryPath: "", barsRoot: "", timeframe: "1D", strategy: "swing_momentum",
      top: "20", limitSymbols, outDir: "",
    });
    assert.equal(result.ok, false, `limit_symbols=${limitSymbols} should be rejected`);
  }
});

test("buildStrategyScanJobRequest defaults top to 20 and omits blank optional fields", () => {
  const result = buildStrategyScanJobRequest({
    registryPath: "  ", barsRoot: "  ", timeframe: "1D", strategy: "swing_momentum",
    top: "", limitSymbols: "", outDir: "  ",
  });
  assert.equal(result.ok, true);
  if (result.ok) {
    assert.equal(result.request.top, 20);
    assert.equal(result.request.registry_path, null);
    assert.equal(result.request.bars_root, null);
    assert.equal(result.request.out_dir, null);
    assert.equal(result.request.limit_symbols, null);
  }
});

test("buildStrategyScanJobRequest carries through valid fields", () => {
  const result = buildStrategyScanJobRequest({
    registryPath: "config/instruments/equities.json",
    barsRoot: "exports/md_backup",
    timeframe: "1D",
    strategy: "swing_momentum",
    top: "10",
    limitSymbols: "5",
    outDir: "exports/strategy_scans",
  });
  assert.equal(result.ok, true);
  if (result.ok) {
    assert.equal(result.request.registry_path, "config/instruments/equities.json");
    assert.equal(result.request.bars_root, "exports/md_backup");
    assert.equal(result.request.timeframe, "1D");
    assert.equal(result.request.strategy, "swing_momentum");
    assert.equal(result.request.top, 10);
    assert.equal(result.request.limit_symbols, 5);
    assert.equal(result.request.out_dir, "exports/strategy_scans");
  }
});

// 5. buildActiveStrategyScanJob maps a job status response to the GUI's active-job shape.
test("buildActiveStrategyScanJob maps a completed status response", () => {
  const response: StrategyScanJobStatusResponse = {
    truth_state: "active",
    job_id: "11111111-1111-1111-1111-111111111111",
    status: "completed",
    submitted_at_utc: "2026-07-12T00:00:00Z",
    completed_at_utc: "2026-07-12T00:01:00Z",
    request: {
      registry_path: "config/instruments/equities.json",
      bars_root: "exports/md_backup",
      timeframe: "1D",
      strategy: "swing_momentum",
      top: 20,
      limit_symbols: null,
      out_dir: "exports/strategy_scans",
    },
    artifact_dir: "exports/strategy_scans/abc",
    summary: {
      scan_id: "abc",
      universe_count: 3,
      ranked_count: 2,
      skipped_count: 1,
      top_ranked: [],
      top_skip_reasons: [],
    },
    blockers: [],
    warnings: ["Scanner ranking is research evidence only."],
    error: null,
  };

  const active = buildActiveStrategyScanJob(response);
  assert.equal(active.jobId, response.job_id);
  assert.equal(active.status, "completed");
  assert.equal(active.timeframe, "1D");
  assert.equal(active.strategy, "swing_momentum");
  assert.equal(active.artifactDir, "exports/strategy_scans/abc");
  assert.equal(active.summary?.ranked_count, 2);
  assert.deepEqual(active.warnings, ["Scanner ranking is research evidence only."]);
});
