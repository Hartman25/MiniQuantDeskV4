import test from "node:test";
import assert from "node:assert/strict";
import {
  normalizeJobStatus,
  isTerminalJobStatus,
  extractArtifactDir,
  buildActiveJob,
} from "../api.ts";
import type { BacktestJobStatusResponse } from "../types.ts";

// ---------------------------------------------------------------------------
// normalizeJobStatus
// ---------------------------------------------------------------------------

test("normalizeJobStatus returns queued for 'queued'", () => {
  assert.equal(normalizeJobStatus("queued"), "queued");
});

test("normalizeJobStatus returns running for 'running'", () => {
  assert.equal(normalizeJobStatus("running"), "running");
});

test("normalizeJobStatus returns completed for 'completed'", () => {
  assert.equal(normalizeJobStatus("completed"), "completed");
});

test("normalizeJobStatus returns failed for 'failed'", () => {
  assert.equal(normalizeJobStatus("failed"), "failed");
});

test("normalizeJobStatus returns unknown for an unrecognized string", () => {
  assert.equal(normalizeJobStatus("in_progress"), "unknown");
});

test("normalizeJobStatus returns unknown for empty string", () => {
  assert.equal(normalizeJobStatus(""), "unknown");
});

// ---------------------------------------------------------------------------
// isTerminalJobStatus
// ---------------------------------------------------------------------------

test("isTerminalJobStatus returns true for completed", () => {
  assert.ok(isTerminalJobStatus("completed"));
});

test("isTerminalJobStatus returns true for failed", () => {
  assert.ok(isTerminalJobStatus("failed"));
});

test("isTerminalJobStatus returns false for queued", () => {
  assert.ok(!isTerminalJobStatus("queued"));
});

test("isTerminalJobStatus returns false for running", () => {
  assert.ok(!isTerminalJobStatus("running"));
});

test("isTerminalJobStatus returns false for unknown", () => {
  assert.ok(!isTerminalJobStatus("unknown"));
});

// ---------------------------------------------------------------------------
// extractArtifactDir
// ---------------------------------------------------------------------------

function makeStatusResponse(
  status: string,
  artifact_dir: string | null,
): BacktestJobStatusResponse {
  return {
    truth_state: "active",
    job_id: "00000000-0000-0000-0000-000000000001",
    status,
    strategy: "swing_momentum",
    symbol: "TEST",
    created_at_utc: "2026-05-01T12:00:00Z",
    started_at_utc: null,
    completed_at_utc: null,
    artifact_dir,
    manifest_path: null,
    metrics_path: null,
    error: null,
  };
}

test("extractArtifactDir returns artifact_dir when status is completed and path is present", () => {
  const resp = makeStatusResponse("completed", "/tmp/artifacts/run1");
  assert.equal(extractArtifactDir(resp), "/tmp/artifacts/run1");
});

test("extractArtifactDir returns null when status is completed but artifact_dir is null", () => {
  const resp = makeStatusResponse("completed", null);
  assert.equal(extractArtifactDir(resp), null);
});

test("extractArtifactDir returns null when status is running even if artifact_dir present", () => {
  const resp = makeStatusResponse("running", "/tmp/artifacts/run1");
  assert.equal(extractArtifactDir(resp), null);
});

test("extractArtifactDir returns null when status is failed", () => {
  const resp = makeStatusResponse("failed", "/tmp/artifacts/run1");
  assert.equal(extractArtifactDir(resp), null);
});

// ---------------------------------------------------------------------------
// buildActiveJob
// ---------------------------------------------------------------------------

test("buildActiveJob maps all fields correctly for a running job", () => {
  const resp: BacktestJobStatusResponse = {
    truth_state: "active",
    job_id: "abc-001",
    status: "running",
    strategy: "swing_momentum",
    symbol: "SPY",
    created_at_utc: "2026-05-01T12:00:00Z",
    started_at_utc: "2026-05-01T12:00:01Z",
    completed_at_utc: null,
    artifact_dir: null,
    manifest_path: null,
    metrics_path: null,
    error: null,
  };
  const job = buildActiveJob(resp);
  assert.equal(job.jobId, "abc-001");
  assert.equal(job.status, "running");
  assert.equal(job.strategy, "swing_momentum");
  assert.equal(job.symbol, "SPY");
  assert.equal(job.startedAt, "2026-05-01T12:00:01Z");
  assert.equal(job.completedAt, null);
  assert.equal(job.artifactDir, null);
  assert.equal(job.error, null);
});

test("buildActiveJob maps completed job with artifact_dir", () => {
  const resp: BacktestJobStatusResponse = {
    truth_state: "active",
    job_id: "abc-002",
    status: "completed",
    strategy: "swing_momentum",
    symbol: "TEST",
    created_at_utc: "2026-05-01T12:00:00Z",
    started_at_utc: "2026-05-01T12:00:01Z",
    completed_at_utc: "2026-05-01T12:00:02Z",
    artifact_dir: "C:\\exports\\backtests\\run-abc",
    manifest_path: "C:\\exports\\backtests\\run-abc\\manifest.json",
    metrics_path: "C:\\exports\\backtests\\run-abc\\metrics.json",
    error: null,
  };
  const job = buildActiveJob(resp);
  assert.equal(job.status, "completed");
  assert.equal(job.artifactDir, "C:\\exports\\backtests\\run-abc");
  assert.equal(job.completedAt, "2026-05-01T12:00:02Z");
});

test("buildActiveJob maps failed job with error message", () => {
  const resp: BacktestJobStatusResponse = {
    truth_state: "active",
    job_id: "abc-003",
    status: "failed",
    strategy: "swing_momentum",
    symbol: "TEST",
    created_at_utc: "2026-05-01T12:00:00Z",
    started_at_utc: "2026-05-01T12:00:01Z",
    completed_at_utc: "2026-05-01T12:00:03Z",
    artifact_dir: null,
    manifest_path: null,
    metrics_path: null,
    error: "load bars csv failed: file not found",
  };
  const job = buildActiveJob(resp);
  assert.equal(job.status, "failed");
  assert.equal(job.artifactDir, null);
  assert.equal(job.error, "load bars csv failed: file not found");
});

test("buildActiveJob maps unknown status string to unknown", () => {
  const resp = makeStatusResponse("mystery_state", null);
  const job = buildActiveJob(resp);
  assert.equal(job.status, "unknown");
});
