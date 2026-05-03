import test from "node:test";
import assert from "node:assert/strict";
import {
  normalizeIngestJobStatus,
  isTerminalIngestStatus,
  extractIngestRowCounts,
  buildActiveIngestJob,
  formatEndTs,
  coverageTruthLabel,
  isCoverageActive,
} from "../api.ts";
import type { IngestJobStatusResponse, MdBarsCoverageResponse } from "../types.ts";

// ---------------------------------------------------------------------------
// normalizeIngestJobStatus
// ---------------------------------------------------------------------------

test("normalizeIngestJobStatus returns queued for 'queued'", () => {
  assert.equal(normalizeIngestJobStatus("queued"), "queued");
});

test("normalizeIngestJobStatus returns running for 'running'", () => {
  assert.equal(normalizeIngestJobStatus("running"), "running");
});

test("normalizeIngestJobStatus returns completed for 'completed'", () => {
  assert.equal(normalizeIngestJobStatus("completed"), "completed");
});

test("normalizeIngestJobStatus returns failed for 'failed'", () => {
  assert.equal(normalizeIngestJobStatus("failed"), "failed");
});

test("normalizeIngestJobStatus returns unknown for unrecognized string", () => {
  assert.equal(normalizeIngestJobStatus("in_progress"), "unknown");
});

test("normalizeIngestJobStatus returns unknown for empty string", () => {
  assert.equal(normalizeIngestJobStatus(""), "unknown");
});

// ---------------------------------------------------------------------------
// isTerminalIngestStatus
// ---------------------------------------------------------------------------

test("isTerminalIngestStatus returns true for completed", () => {
  assert.ok(isTerminalIngestStatus("completed"));
});

test("isTerminalIngestStatus returns true for failed", () => {
  assert.ok(isTerminalIngestStatus("failed"));
});

test("isTerminalIngestStatus returns false for queued", () => {
  assert.ok(!isTerminalIngestStatus("queued"));
});

test("isTerminalIngestStatus returns false for running", () => {
  assert.ok(!isTerminalIngestStatus("running"));
});

test("isTerminalIngestStatus returns false for unknown", () => {
  assert.ok(!isTerminalIngestStatus("unknown"));
});

// ---------------------------------------------------------------------------
// extractIngestRowCounts
// ---------------------------------------------------------------------------

function makeStatusResponse(
  status: string,
  rows_read: number | null,
  rows_inserted: number | null,
  rows_rejected: number | null,
): IngestJobStatusResponse {
  return {
    truth_state: "active",
    job_id: "00000000-0000-0000-0000-000000000001",
    status,
    source: "csv",
    timeframe: "1D",
    csv_path: "/data/AAPL_1D.csv",
    created_at_utc: "2026-05-01T12:00:00Z",
    started_at_utc: null,
    completed_at_utc: null,
    rows_read,
    rows_inserted,
    rows_rejected,
    quality_report_path: null,
    error: null,
  };
}

test("extractIngestRowCounts returns counts when status is completed", () => {
  const resp = makeStatusResponse("completed", 8375, 8375, 0);
  const counts = extractIngestRowCounts(resp);
  assert.ok(counts !== null);
  assert.equal(counts!.rowsRead, 8375);
  assert.equal(counts!.rowsInserted, 8375);
  assert.equal(counts!.rowsRejected, 0);
});

test("extractIngestRowCounts returns null when status is running", () => {
  const resp = makeStatusResponse("running", null, null, null);
  assert.equal(extractIngestRowCounts(resp), null);
});

test("extractIngestRowCounts returns null when status is failed", () => {
  const resp = makeStatusResponse("failed", null, null, null);
  assert.equal(extractIngestRowCounts(resp), null);
});

test("extractIngestRowCounts handles null row counts on completed job", () => {
  const resp = makeStatusResponse("completed", null, null, null);
  const counts = extractIngestRowCounts(resp);
  assert.ok(counts !== null);
  assert.equal(counts!.rowsRead, null);
  assert.equal(counts!.rowsInserted, null);
  assert.equal(counts!.rowsRejected, null);
});

// ---------------------------------------------------------------------------
// buildActiveIngestJob
// ---------------------------------------------------------------------------

test("buildActiveIngestJob maps all fields correctly for a running job", () => {
  const resp: IngestJobStatusResponse = {
    truth_state: "active",
    job_id: "abc-001",
    status: "running",
    source: "csv",
    timeframe: "1D",
    csv_path: "C:\\exports\\md_backup\\1D\\AAPL_1D.csv",
    created_at_utc: "2026-05-01T12:00:00Z",
    started_at_utc: "2026-05-01T12:00:01Z",
    completed_at_utc: null,
    rows_read: null,
    rows_inserted: null,
    rows_rejected: null,
    quality_report_path: null,
    error: null,
  };
  const job = buildActiveIngestJob(resp);
  assert.equal(job.jobId, "abc-001");
  assert.equal(job.status, "running");
  assert.equal(job.source, "csv");
  assert.equal(job.timeframe, "1D");
  assert.equal(job.csvPath, "C:\\exports\\md_backup\\1D\\AAPL_1D.csv");
  assert.equal(job.startedAt, "2026-05-01T12:00:01Z");
  assert.equal(job.completedAt, null);
  assert.equal(job.rowsRead, null);
  assert.equal(job.error, null);
});

test("buildActiveIngestJob maps completed job with row counts and quality report", () => {
  const resp: IngestJobStatusResponse = {
    truth_state: "active",
    job_id: "abc-002",
    status: "completed",
    source: "csv",
    timeframe: "1D",
    csv_path: "C:\\exports\\md_backup\\1D\\AAPL_1D.csv",
    created_at_utc: "2026-05-01T12:00:00Z",
    started_at_utc: "2026-05-01T12:00:01Z",
    completed_at_utc: "2026-05-01T12:00:05Z",
    rows_read: 8375,
    rows_inserted: 8375,
    rows_rejected: 0,
    quality_report_path: "C:\\exports\\md_ingest\\data_quality.json",
    error: null,
  };
  const job = buildActiveIngestJob(resp);
  assert.equal(job.status, "completed");
  assert.equal(job.rowsRead, 8375);
  assert.equal(job.rowsInserted, 8375);
  assert.equal(job.rowsRejected, 0);
  assert.equal(job.qualityReportPath, "C:\\exports\\md_ingest\\data_quality.json");
  assert.equal(job.error, null);
});

test("buildActiveIngestJob maps failed job with error message", () => {
  const resp: IngestJobStatusResponse = {
    truth_state: "active",
    job_id: "abc-003",
    status: "failed",
    source: "csv",
    timeframe: "1D",
    csv_path: "C:\\missing.csv",
    created_at_utc: "2026-05-01T12:00:00Z",
    started_at_utc: null,
    completed_at_utc: "2026-05-01T12:00:02Z",
    rows_read: null,
    rows_inserted: null,
    rows_rejected: null,
    quality_report_path: null,
    error: "csv_path not found: C:\\missing.csv",
  };
  const job = buildActiveIngestJob(resp);
  assert.equal(job.status, "failed");
  assert.equal(job.error, "csv_path not found: C:\\missing.csv");
  assert.equal(job.rowsInserted, null);
});

test("buildActiveIngestJob maps unknown status string to unknown", () => {
  const resp = makeStatusResponse("mystery_state", null, null, null);
  const job = buildActiveIngestJob(resp);
  assert.equal(job.status, "unknown");
});

// ---------------------------------------------------------------------------
// DATA-INGEST-GUI-RESULTS-01: coverage helpers
// ---------------------------------------------------------------------------

// formatEndTs

test("formatEndTs returns YYYY-MM-DD for known unix timestamp", () => {
  // 2024-02-16T00:00:00Z → 1708041600
  const result = formatEndTs(1708041600);
  assert.equal(result, "2024-02-16");
});

test("formatEndTs returns — for null", () => {
  assert.equal(formatEndTs(null), "—");
});

test("formatEndTs returns — for undefined", () => {
  assert.equal(formatEndTs(undefined), "—");
});

test("formatEndTs returns — for zero", () => {
  assert.equal(formatEndTs(0), "—");
});

// coverageTruthLabel

test("coverageTruthLabel active → active", () => {
  assert.equal(coverageTruthLabel("active"), "active");
});

test("coverageTruthLabel empty → no data", () => {
  assert.equal(coverageTruthLabel("empty"), "no data");
});

test("coverageTruthLabel db_unavailable → db unavailable", () => {
  assert.equal(coverageTruthLabel("db_unavailable"), "db unavailable");
});

test("coverageTruthLabel unavailable → unavailable", () => {
  assert.equal(coverageTruthLabel("unavailable"), "unavailable");
});

test("coverageTruthLabel unknown string passes through", () => {
  assert.equal(coverageTruthLabel("some_other_state"), "some_other_state");
});

// isCoverageActive

test("isCoverageActive returns true for active", () => {
  assert.ok(isCoverageActive("active"));
});

test("isCoverageActive returns false for empty", () => {
  assert.ok(!isCoverageActive("empty"));
});

test("isCoverageActive returns false for db_unavailable", () => {
  assert.ok(!isCoverageActive("db_unavailable"));
});

test("isCoverageActive returns false for unavailable", () => {
  assert.ok(!isCoverageActive("unavailable"));
});

// coverage response shape

test("MdBarsCoverageResponse with active rows has correct structure", () => {
  const resp: MdBarsCoverageResponse = {
    canonical_route: "/api/v1/market-data/coverage",
    truth_state: "active",
    timeframe: "1D",
    rows: [
      {
        symbol: "AAPL",
        timeframe: "1D",
        bars: 8375,
        min_end_ts: 726105600,
        max_end_ts: 1745625600,
        latest_ingested_at: "2026-04-19T12:21:42Z",
      },
    ],
    error: null,
  };
  assert.ok(isCoverageActive(resp.truth_state));
  assert.equal(resp.rows.length, 1);
  assert.equal(resp.rows[0].symbol, "AAPL");
  assert.equal(resp.rows[0].bars, 8375);
  assert.equal(formatEndTs(resp.rows[0].min_end_ts), "1993-01-04");
  assert.equal(resp.rows[0].latest_ingested_at, "2026-04-19T12:21:42Z");
});

test("MdBarsCoverageResponse with db_unavailable has empty rows", () => {
  const resp: MdBarsCoverageResponse = {
    canonical_route: "/api/v1/market-data/coverage",
    truth_state: "db_unavailable",
    timeframe: null,
    rows: [],
    error: "database pool not configured",
  };
  assert.ok(!isCoverageActive(resp.truth_state));
  assert.equal(resp.rows.length, 0);
  assert.ok(resp.error !== null);
});
