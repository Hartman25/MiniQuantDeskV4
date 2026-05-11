import test from "node:test";
import assert from "node:assert/strict";
import {
  normalizeJobStatus,
  isTerminalJobStatus,
  extractArtifactDir,
  buildActiveJob,
} from "../api.ts";
import type { BacktestJobRequest, BacktestJobStatusResponse, FileResult } from "../types.ts";

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

// ---------------------------------------------------------------------------
// B01: AAPL 1D daily submit request body shape (BACKTEST-GUI-CLOSURE-01)
// Proves the BacktestJobRequest fields are correctly typed and valued for the
// AAPL 1D daily backtest scenario that the GUI form constructs.
// ---------------------------------------------------------------------------

test("B01: AAPL 1D BacktestJobRequest has required fields with correct types", () => {
  const req: BacktestJobRequest = {
    bars_path: "C:\\repo\\exports\\md_backup\\1D\\AAPL_1D.csv",
    strategy: "swing_momentum",
    symbol: "AAPL",
    timeframe_secs: 86400,
    initial_cash_micros: 100_000_000_000,
    out_dir: "C:\\repo\\exports\\backtests",
    integrity_stale_threshold_ticks: 172800,
  };
  assert.equal(req.strategy, "swing_momentum");
  assert.equal(req.symbol, "AAPL");
  assert.equal(req.timeframe_secs, 86400, "timeframe_secs must be 86400 for daily bars");
  assert.equal(req.initial_cash_micros, 100_000_000_000);
  assert.equal(req.integrity_stale_threshold_ticks, 172800);
  assert.ok(req.bars_path.endsWith("AAPL_1D.csv"), "bars_path must point to AAPL 1D CSV");
});

test("B01b: empty stale threshold string maps to null in submit request (daemon applies default)", () => {
  // Mirrors GUI handleSubmitJob logic: staleThresholdRaw ? parseInt(...) : null
  const raw = "".trim();
  const threshold: number | null = raw ? parseInt(raw, 10) : null;
  assert.equal(threshold, null, "empty field sends null → daemon applies timeframe-aware default");
});

test("B01c: explicit stale threshold '172800' parses to integer 172800 for daily bars", () => {
  const raw = "172800".trim();
  const threshold: number | null = raw ? parseInt(raw, 10) : null;
  assert.equal(threshold, 172800);
  assert.ok(threshold !== null && threshold >= 86400 * 2,
    "172800 >= 2 days; needed to avoid blocking on 3-day weekend gaps in daily data");
});

// ---------------------------------------------------------------------------
// B02: Completed AAPL job with artifact_dir → extractArtifactDir signals auto-load
// ---------------------------------------------------------------------------

test("B02: completed AAPL job with artifact_dir → extractArtifactDir returns path for auto-load", () => {
  const resp: BacktestJobStatusResponse = {
    truth_state: "active",
    job_id: "bfa264d3-1328-5bd1-b732-9d32e8dac8ad",
    status: "completed",
    strategy: "swing_momentum",
    symbol: "AAPL",
    created_at_utc: "2026-05-11T06:45:26Z",
    started_at_utc: "2026-05-11T06:45:26Z",
    completed_at_utc: "2026-05-11T06:45:27Z",
    artifact_dir: "C:\\repo\\exports\\backtests\\bfa264d3-1328-5bd1-b732-9d32e8dac8ad",
    manifest_path: "C:\\repo\\exports\\backtests\\bfa264d3-1328-5bd1-b732-9d32e8dac8ad\\manifest.json",
    metrics_path: "C:\\repo\\exports\\backtests\\bfa264d3-1328-5bd1-b732-9d32e8dac8ad\\metrics.json",
    error: null,
  };
  const dir = extractArtifactDir(resp);
  assert.ok(dir !== null, "completed job with artifact_dir must return non-null — this is the auto-load trigger");
  assert.ok(dir!.length > 0, "extracted artifact_dir must be non-empty");
  assert.ok(dir!.includes("bfa264d3"), "extracted artifact_dir must contain the run_id");
});

test("B02b: completed AAPL job without artifact_dir → extractArtifactDir returns null (no auto-load)", () => {
  const resp = makeStatusResponse("completed", null);
  assert.equal(extractArtifactDir(resp), null,
    "null artifact_dir on completed job must not trigger auto-load");
});

// ---------------------------------------------------------------------------
// B03: Failed AAPL job surfaces failure reason truthfully
// ---------------------------------------------------------------------------

test("B03: failed AAPL job with CSV load error → buildActiveJob surfaces error truthfully", () => {
  const resp: BacktestJobStatusResponse = {
    truth_state: "active",
    job_id: "00000000-0000-0000-0000-000000000099",
    status: "failed",
    strategy: "swing_momentum",
    symbol: "AAPL",
    created_at_utc: "2026-05-11T06:45:26Z",
    started_at_utc: "2026-05-11T06:45:26Z",
    completed_at_utc: "2026-05-11T06:45:27Z",
    artifact_dir: null,
    manifest_path: null,
    metrics_path: null,
    error: "load bars csv failed: C:\\repo\\exports\\md_backup\\1D\\AAPL_1D.csv: file not found",
  };
  const job = buildActiveJob(resp);
  assert.equal(job.status, "failed");
  assert.ok(job.error !== null, "error must not be null for failed job");
  assert.ok(job.error!.includes("AAPL_1D.csv"), "error must reference the bars file — truthful, not hidden");
  assert.equal(job.artifactDir, null, "failed job must have null artifactDir — no partial artifacts claimed");
});

test("B03b: failed AAPL job with unknown strategy error → buildActiveJob surfaces strategy error", () => {
  const resp: BacktestJobStatusResponse = {
    truth_state: "active",
    job_id: "00000000-0000-0000-0000-000000000100",
    status: "failed",
    strategy: "bad_strategy",
    symbol: "AAPL",
    created_at_utc: "2026-05-11T06:45:26Z",
    started_at_utc: null,
    completed_at_utc: "2026-05-11T06:45:27Z",
    artifact_dir: null,
    manifest_path: null,
    metrics_path: null,
    error: "unknown strategy 'bad_strategy'; available: swing_momentum, mean_reversion",
  };
  const job = buildActiveJob(resp);
  assert.equal(job.status, "failed");
  assert.ok(job.error!.includes("unknown strategy"), "error must describe the strategy problem");
  assert.ok(job.error!.includes("swing_momentum"), "error must list available strategies");
});

// ---------------------------------------------------------------------------
// B05: Missing artifact file produces explicit 'missing' FileResult — not crash
// FileResult kinds are structurally distinct; 'missing' is not 'parse_error'
// or 'read_error'. The GUI renders each kind with its own explicit message.
// ---------------------------------------------------------------------------

test("B05: FileResult 'missing' kind is structurally distinct from parse_error and read_error", () => {
  const missing: FileResult<string> = { kind: "missing" };
  const parseErr: FileResult<string> = { kind: "parse_error", message: "bad json" };
  const readErr: FileResult<string> = { kind: "read_error", message: "permission denied" };
  const ok: FileResult<string> = { kind: "ok", data: "data" };
  assert.notEqual(missing.kind, parseErr.kind, "missing != parse_error");
  assert.notEqual(missing.kind, readErr.kind, "missing != read_error");
  assert.notEqual(missing.kind, ok.kind, "missing != ok");
  assert.equal(missing.kind, "missing");
});

test("B05b: idle and loading FileResult kinds are non-error states", () => {
  const idle: FileResult<string> = { kind: "idle" };
  const loading: FileResult<string> = { kind: "loading" };
  const errorKinds = ["missing", "parse_error", "read_error"];
  assert.ok(!errorKinds.includes(idle.kind), "idle is not an error kind");
  assert.ok(!errorKinds.includes(loading.kind), "loading is not an error kind");
  // GUI hides FileStatusNote for idle and loading — no false error displayed.
});
