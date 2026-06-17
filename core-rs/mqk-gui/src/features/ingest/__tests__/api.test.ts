import test from "node:test";
import assert from "node:assert/strict";
import {
  normalizeIngestJobStatus,
  isTerminalIngestStatus,
  isCancellableIngestStatus,
  extractIngestRowCounts,
  buildActiveIngestJob,
  cancelIngestJob,
  isProviderSyncAllowed,
  buildProviderJobRequest,
  buildActiveProviderJob,
  classifyCoverageFreshness,
  computeCoverageSummary,
  computeMissingTrackedSymbols,
  coverageFreshnessThresholdSecs,
  coverageTruthLabel,
  filterCoverageRows,
  formatEndTs,
  isCoverageActive,
  isIntradayRefreshActive,
  intradayRefreshTruthLabel,
  isTrackedEquitiesActive,
  sortCoverageRows,
  trackedEquitiesTruthLabel,
  COVERAGE_FRESHNESS_THRESHOLD_1D_SECS,
  COVERAGE_FRESHNESS_THRESHOLD_INTRADAY_SECS,
  buildMarketDataFeedPollOnceRequest,
  buildMarketDataFeedSchedulerStartRequest,
  getMarketDataFeedSchedulerStatus,
  isMarketDataFeedRealActionAllowed,
  normalizeMarketDataFeedSchedulerStatusResponse,
  normalizeMarketDataFeedStatusResponse,
  parseMarketDataFeedSymbols,
  pollMarketDataFeedOnce,
  startMarketDataFeedScheduler,
  stopMarketDataFeedScheduler,
} from "../api.ts";
import type { IngestJobStatusResponse, IntradayRefreshStatusResponse, MdBarsCoverageResponse, MdBarsCoverageRow, TrackedEquitiesResponse } from "../types.ts";

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

test("normalizeIngestJobStatus returns refused for 'refused'", () => {
  assert.equal(normalizeIngestJobStatus("refused"), "refused");
});

test("normalizeIngestJobStatus returns cancelled for 'cancelled'", () => {
  assert.equal(normalizeIngestJobStatus("cancelled"), "cancelled");
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

test("isTerminalIngestStatus returns true for refused", () => {
  assert.ok(isTerminalIngestStatus("refused"));
});

test("isTerminalIngestStatus returns true for cancelled", () => {
  assert.ok(isTerminalIngestStatus("cancelled"));
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
// isCancellableIngestStatus
// ---------------------------------------------------------------------------

test("isCancellableIngestStatus returns true for queued and running", () => {
  assert.ok(isCancellableIngestStatus("queued"));
  assert.ok(isCancellableIngestStatus("running"));
});

test("isCancellableIngestStatus returns false for terminal and unknown statuses", () => {
  for (const status of [
    "completed",
    "failed",
    "partial",
    "refused",
    "dry_run_completed",
    "cancelled",
    "unknown",
  ] as const) {
    assert.equal(isCancellableIngestStatus(status), false);
  }
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

// ---------------------------------------------------------------------------
// DATA-INGEST-GUI-SYNC-ALL-01: tracked-equities helpers
// ---------------------------------------------------------------------------

// isTrackedEquitiesActive

test("isTrackedEquitiesActive returns true for active", () => {
  assert.ok(isTrackedEquitiesActive("active"));
});

test("isTrackedEquitiesActive returns false for registry_unavailable", () => {
  assert.ok(!isTrackedEquitiesActive("registry_unavailable"));
});

test("isTrackedEquitiesActive returns false for registry_invalid", () => {
  assert.ok(!isTrackedEquitiesActive("registry_invalid"));
});

test("isTrackedEquitiesActive returns false for unknown string", () => {
  assert.ok(!isTrackedEquitiesActive(""));
});

// trackedEquitiesTruthLabel

test("trackedEquitiesTruthLabel active → active", () => {
  assert.equal(trackedEquitiesTruthLabel("active"), "active");
});

test("trackedEquitiesTruthLabel registry_unavailable → registry unavailable", () => {
  assert.equal(trackedEquitiesTruthLabel("registry_unavailable"), "registry unavailable");
});

test("trackedEquitiesTruthLabel registry_invalid → registry invalid", () => {
  assert.equal(trackedEquitiesTruthLabel("registry_invalid"), "registry invalid");
});

test("trackedEquitiesTruthLabel unknown string passes through", () => {
  assert.equal(trackedEquitiesTruthLabel("some_other_state"), "some_other_state");
});

// TrackedEquitiesResponse shape

test("TrackedEquitiesResponse active shape has count and symbols", () => {
  const resp: TrackedEquitiesResponse = {
    canonical_route: "/api/v1/ingest/tracked-equities",
    truth_state: "active",
    registry_path: "config/instruments/equities.json",
    count: 88,
    symbols: [
      { symbol: "AAPL", instrument_id: "equity:US:AAPL", provider: "twelvedata", venue: "NASDAQ", timeframes: ["1D"] },
      { symbol: "SPY", instrument_id: "equity:US:SPY", provider: "twelvedata", venue: "NYSEARCA", timeframes: ["1D"] },
    ],
    first_symbol: "AAPL",
    last_symbol: "SPY",
    error: null,
  };
  assert.ok(isTrackedEquitiesActive(resp.truth_state));
  assert.equal(resp.count, 88);
  assert.equal(resp.symbols.length, 2);
  assert.equal(resp.symbols[0].symbol, "AAPL");
  assert.equal(resp.first_symbol, "AAPL");
  assert.equal(resp.last_symbol, "SPY");
  assert.equal(resp.error, null);
});

test("TrackedEquitiesResponse registry_unavailable shape is honest", () => {
  const resp: TrackedEquitiesResponse = {
    canonical_route: "/api/v1/ingest/tracked-equities",
    truth_state: "registry_unavailable",
    registry_path: "/nonexistent/equities.json",
    count: 0,
    symbols: [],
    first_symbol: null,
    last_symbol: null,
    error: "registry file not found: /nonexistent/equities.json",
  };
  assert.ok(!isTrackedEquitiesActive(resp.truth_state));
  assert.equal(resp.count, 0);
  assert.equal(resp.symbols.length, 0);
  assert.ok(resp.error !== null);
});

// ---------------------------------------------------------------------------
// INTRADAY-MD-REFRESHER-GUI-01: Intraday refresh helpers
// ---------------------------------------------------------------------------

// isIntradayRefreshActive

test("isIntradayRefreshActive returns true for active", () => {
  assert.ok(isIntradayRefreshActive("active"));
});

test("isIntradayRefreshActive returns false for no_evidence", () => {
  assert.ok(!isIntradayRefreshActive("no_evidence"));
});

test("isIntradayRefreshActive returns false for parse_error", () => {
  assert.ok(!isIntradayRefreshActive("parse_error"));
});

test("isIntradayRefreshActive returns false for backend_unavailable", () => {
  assert.ok(!isIntradayRefreshActive("backend_unavailable"));
});

test("isIntradayRefreshActive returns false for empty string", () => {
  assert.ok(!isIntradayRefreshActive(""));
});

// intradayRefreshTruthLabel

test("intradayRefreshTruthLabel active → active", () => {
  assert.equal(intradayRefreshTruthLabel("active"), "active");
});

test("intradayRefreshTruthLabel no_evidence → no evidence", () => {
  assert.equal(intradayRefreshTruthLabel("no_evidence"), "no evidence");
});

test("intradayRefreshTruthLabel parse_error → parse error", () => {
  assert.equal(intradayRefreshTruthLabel("parse_error"), "parse error");
});

test("intradayRefreshTruthLabel backend_unavailable → unavailable", () => {
  assert.equal(intradayRefreshTruthLabel("backend_unavailable"), "unavailable");
});

test("intradayRefreshTruthLabel unknown string passes through", () => {
  assert.equal(intradayRefreshTruthLabel("some_other_state"), "some_other_state");
});

// IntradayRefreshStatusResponse shape

test("IntradayRefreshStatusResponse active shape has symbols and all_passed", () => {
  const resp: IntradayRefreshStatusResponse = {
    canonical_route: "/api/v1/market-data/intraday-refresh/status",
    truth_state: "active",
    evidence_path: "exports/market_data/intraday_refresh_20260615_093000.json",
    stale_or_missing_evidence: false,
    schema_version: "intraday-refresh-v1",
    produced_at_utc: "2026-06-15T09:30:00Z",
    mode: "once",
    source: "alpaca",
    timeframe: "1m",
    all_passed: true,
    reason: "all symbols passed",
    symbols: [
      {
        symbol: "AAPL",
        gate: "PASS",
        completed_count: 390,
        latest_completed_bar_ts: "2026-06-15T16:00:00Z",
        staleness_min: 2,
        provider_source: "alpaca",
        provider_configured: true,
        provider_attempted: true,
        provider_success: true,
        rows_inserted: 5,
        rows_updated: 385,
        rows_filtered_incomplete: 0,
        rows_filtered_in_progress: 1,
        fail_reasons: [],
      },
    ],
    error: null,
  };
  assert.ok(isIntradayRefreshActive(resp.truth_state));
  assert.equal(resp.all_passed, true);
  assert.equal(resp.symbols.length, 1);
  assert.equal(resp.symbols[0].symbol, "AAPL");
  assert.equal(resp.symbols[0].gate, "PASS");
  assert.equal(resp.symbols[0].completed_count, 390);
  assert.equal(resp.symbols[0].fail_reasons.length, 0);
  assert.equal(resp.stale_or_missing_evidence, false);
});

test("IntradayRefreshStatusResponse no_evidence shape is honest", () => {
  const resp: IntradayRefreshStatusResponse = {
    canonical_route: "/api/v1/market-data/intraday-refresh/status",
    truth_state: "no_evidence",
    evidence_path: null,
    stale_or_missing_evidence: true,
    schema_version: null,
    produced_at_utc: null,
    mode: null,
    source: null,
    timeframe: null,
    all_passed: null,
    reason: null,
    symbols: [],
    error: null,
  };
  assert.ok(!isIntradayRefreshActive(resp.truth_state));
  assert.equal(resp.all_passed, null);
  assert.equal(resp.symbols.length, 0);
  assert.equal(resp.stale_or_missing_evidence, true);
  assert.equal(resp.evidence_path, null);
});

test("IntradayRefreshStatusResponse stale evidence has stale_or_missing_evidence=true", () => {
  const resp: IntradayRefreshStatusResponse = {
    canonical_route: "/api/v1/market-data/intraday-refresh/status",
    truth_state: "active",
    evidence_path: "exports/market_data/intraday_refresh_20260614_093000.json",
    stale_or_missing_evidence: true,
    schema_version: "intraday-refresh-v1",
    produced_at_utc: "2026-06-14T09:30:00Z",
    mode: "once",
    source: "alpaca",
    timeframe: "1m",
    all_passed: true,
    reason: "all symbols passed",
    symbols: [],
    error: null,
  };
  assert.ok(isIntradayRefreshActive(resp.truth_state));
  assert.equal(resp.stale_or_missing_evidence, true);
});

test("IntradayRefreshStatusResponse parse_error shape has error field", () => {
  const resp: IntradayRefreshStatusResponse = {
    canonical_route: "/api/v1/market-data/intraday-refresh/status",
    truth_state: "parse_error",
    evidence_path: "exports/market_data/intraday_refresh_20260615_093000.json",
    stale_or_missing_evidence: true,
    schema_version: null,
    produced_at_utc: null,
    mode: null,
    source: null,
    timeframe: null,
    all_passed: null,
    reason: null,
    symbols: [],
    error: "unsupported schema_version: unknown-v99",
  };
  assert.ok(!isIntradayRefreshActive(resp.truth_state));
  assert.equal(resp.error, "unsupported schema_version: unknown-v99");
  assert.equal(resp.symbols.length, 0);
});

// ---------------------------------------------------------------------------
// DATA-INGEST-GUI-COVERAGE-POLISH-01: coverage freshness, sort, filter, summary, missing
// ---------------------------------------------------------------------------

// Test fixture helpers
function makeRow(
  symbol: string,
  timeframe: string,
  bars: number,
  maxEndTs: number,
): MdBarsCoverageRow {
  return {
    symbol,
    timeframe,
    bars,
    min_end_ts: maxEndTs - bars * 86400,
    max_end_ts: maxEndTs,
    latest_ingested_at: null,
  };
}

const NOW = 1_718_000_000; // fixed epoch for deterministic freshness tests

// coverageFreshnessThresholdSecs

test("coverageFreshnessThresholdSecs 1D returns 345600", () => {
  assert.equal(coverageFreshnessThresholdSecs("1D"), COVERAGE_FRESHNESS_THRESHOLD_1D_SECS);
  assert.equal(coverageFreshnessThresholdSecs("1D"), 345600);
});

test("coverageFreshnessThresholdSecs 1m returns 900", () => {
  assert.equal(coverageFreshnessThresholdSecs("1m"), COVERAGE_FRESHNESS_THRESHOLD_INTRADAY_SECS);
  assert.equal(coverageFreshnessThresholdSecs("1m"), 900);
});

test("coverageFreshnessThresholdSecs 5m returns 900", () => {
  assert.equal(coverageFreshnessThresholdSecs("5m"), 900);
});

test("coverageFreshnessThresholdSecs unknown timeframe returns null", () => {
  assert.equal(coverageFreshnessThresholdSecs("4h"), null);
  assert.equal(coverageFreshnessThresholdSecs(""), null);
});

// classifyCoverageFreshness

test("classifyCoverageFreshness 1D fresh — age at exactly threshold is fresh", () => {
  const maxEndTs = NOW - COVERAGE_FRESHNESS_THRESHOLD_1D_SECS;
  assert.equal(classifyCoverageFreshness(maxEndTs, NOW, "1D"), "fresh");
});

test("classifyCoverageFreshness 1D stale — age one second past threshold", () => {
  const maxEndTs = NOW - COVERAGE_FRESHNESS_THRESHOLD_1D_SECS - 1;
  assert.equal(classifyCoverageFreshness(maxEndTs, NOW, "1D"), "stale");
});

test("classifyCoverageFreshness 5m fresh — age well within 900s", () => {
  const maxEndTs = NOW - 300;
  assert.equal(classifyCoverageFreshness(maxEndTs, NOW, "5m"), "fresh");
});

test("classifyCoverageFreshness 5m stale — age exceeds 900s", () => {
  const maxEndTs = NOW - 2000;
  assert.equal(classifyCoverageFreshness(maxEndTs, NOW, "5m"), "stale");
});

test("classifyCoverageFreshness unknown timeframe → unknown", () => {
  assert.equal(classifyCoverageFreshness(NOW - 100, NOW, "4h"), "unknown");
});

test("classifyCoverageFreshness null maxEndTs → unknown", () => {
  assert.equal(classifyCoverageFreshness(null, NOW, "1D"), "unknown");
});

test("classifyCoverageFreshness zero maxEndTs → unknown", () => {
  assert.equal(classifyCoverageFreshness(0, NOW, "1D"), "unknown");
});

// filterCoverageRows

test("filterCoverageRows empty query returns all rows", () => {
  const rows = [makeRow("AAPL", "1D", 100, NOW), makeRow("MSFT", "1D", 200, NOW)];
  assert.equal(filterCoverageRows(rows, "").length, 2);
});

test("filterCoverageRows whitespace-only query returns all rows", () => {
  const rows = [makeRow("AAPL", "1D", 100, NOW), makeRow("MSFT", "1D", 200, NOW)];
  assert.equal(filterCoverageRows(rows, "   ").length, 2);
});

test("filterCoverageRows matches case-insensitively", () => {
  const rows = [makeRow("AAPL", "1D", 100, NOW), makeRow("MSFT", "1D", 200, NOW)];
  const result = filterCoverageRows(rows, "aapl");
  assert.equal(result.length, 1);
  assert.equal(result[0].symbol, "AAPL");
});

test("filterCoverageRows uppercase query matches lowercase stored symbol", () => {
  const rows = [makeRow("aapl", "1D", 100, NOW)];
  const result = filterCoverageRows(rows, "AAPL");
  assert.equal(result.length, 1);
});

test("filterCoverageRows returns empty when no match", () => {
  const rows = [makeRow("AAPL", "1D", 100, NOW), makeRow("MSFT", "1D", 200, NOW)];
  assert.equal(filterCoverageRows(rows, "GOOG").length, 0);
});

test("filterCoverageRows partial substring match", () => {
  const rows = [makeRow("AAPL", "1D", 100, NOW), makeRow("MSFT", "1D", 200, NOW), makeRow("AMZN", "1D", 50, NOW)];
  const result = filterCoverageRows(rows, "A");
  assert.equal(result.length, 2); // AAPL and AMZN
});

// sortCoverageRows

test("sortCoverageRows symbol_asc sorts A before Z", () => {
  const rows = [makeRow("MSFT", "1D", 100, NOW), makeRow("AAPL", "1D", 100, NOW)];
  const sorted = sortCoverageRows(rows, "symbol_asc");
  assert.equal(sorted[0].symbol, "AAPL");
  assert.equal(sorted[1].symbol, "MSFT");
});

test("sortCoverageRows symbol_desc sorts Z before A", () => {
  const rows = [makeRow("AAPL", "1D", 100, NOW), makeRow("MSFT", "1D", 100, NOW)];
  const sorted = sortCoverageRows(rows, "symbol_desc");
  assert.equal(sorted[0].symbol, "MSFT");
  assert.equal(sorted[1].symbol, "AAPL");
});

test("sortCoverageRows bars_desc puts highest bar count first", () => {
  const rows = [makeRow("AAPL", "1D", 50, NOW), makeRow("MSFT", "1D", 200, NOW)];
  const sorted = sortCoverageRows(rows, "bars_desc");
  assert.equal(sorted[0].symbol, "MSFT");
});

test("sortCoverageRows bars_asc puts lowest bar count first", () => {
  const rows = [makeRow("AAPL", "1D", 50, NOW), makeRow("MSFT", "1D", 200, NOW)];
  const sorted = sortCoverageRows(rows, "bars_asc");
  assert.equal(sorted[0].symbol, "AAPL");
});

test("sortCoverageRows bars_desc ties broken by symbol ascending", () => {
  const rows = [makeRow("MSFT", "1D", 100, NOW), makeRow("AAPL", "1D", 100, NOW)];
  const sorted = sortCoverageRows(rows, "bars_desc");
  assert.equal(sorted[0].symbol, "AAPL"); // tie broken alphabetically
});

test("sortCoverageRows latest_desc puts newest max_end_ts first", () => {
  const rows = [makeRow("AAPL", "1D", 10, NOW - 1000), makeRow("MSFT", "1D", 10, NOW)];
  const sorted = sortCoverageRows(rows, "latest_desc");
  assert.equal(sorted[0].symbol, "MSFT");
});

test("sortCoverageRows latest_asc puts oldest max_end_ts first", () => {
  const rows = [makeRow("AAPL", "1D", 10, NOW - 1000), makeRow("MSFT", "1D", 10, NOW)];
  const sorted = sortCoverageRows(rows, "latest_asc");
  assert.equal(sorted[0].symbol, "AAPL");
});

test("sortCoverageRows does not mutate the input array", () => {
  const rows = [makeRow("MSFT", "1D", 100, NOW), makeRow("AAPL", "1D", 100, NOW)];
  sortCoverageRows(rows, "symbol_asc");
  assert.equal(rows[0].symbol, "MSFT"); // original order unchanged
});

// computeCoverageSummary

test("computeCoverageSummary counts totalDaemonRows from allRows", () => {
  const allRows = [makeRow("AAPL", "1D", 100, NOW), makeRow("MSFT", "1D", 200, NOW)];
  const filtered = [allRows[0]];
  const summary = computeCoverageSummary(allRows, filtered);
  assert.equal(summary.totalDaemonRows, 2);
});

test("computeCoverageSummary counts visibleRows from filtered", () => {
  const allRows = [makeRow("AAPL", "1D", 100, NOW), makeRow("MSFT", "1D", 200, NOW)];
  const filtered = [allRows[0]];
  const summary = computeCoverageSummary(allRows, filtered);
  assert.equal(summary.visibleRows, 1);
});

test("computeCoverageSummary sums visibleBars from filtered rows", () => {
  const allRows = [makeRow("AAPL", "1D", 100, NOW), makeRow("MSFT", "1D", 200, NOW)];
  const filtered = allRows;
  const summary = computeCoverageSummary(allRows, filtered);
  assert.equal(summary.visibleBars, 300);
});

test("computeCoverageSummary empty filtered has zero visibleBars", () => {
  const allRows = [makeRow("AAPL", "1D", 100, NOW)];
  const summary = computeCoverageSummary(allRows, []);
  assert.equal(summary.visibleRows, 0);
  assert.equal(summary.visibleBars, 0);
  assert.equal(summary.totalDaemonRows, 1);
});

// computeMissingTrackedSymbols

test("computeMissingTrackedSymbols returns null when trackedSymbols is null (registry unavailable)", () => {
  const result = computeMissingTrackedSymbols(null, [makeRow("AAPL", "1D", 100, NOW)], "1D");
  assert.equal(result, null); // NOT an empty array
});

test("computeMissingTrackedSymbols returns empty array when all tracked symbols have coverage", () => {
  const tracked = [{ symbol: "AAPL", timeframes: ["1D"] }];
  const coverage = [makeRow("AAPL", "1D", 100, NOW)];
  const result = computeMissingTrackedSymbols(tracked, coverage, "1D");
  assert.deepEqual(result, []);
});

test("computeMissingTrackedSymbols returns missing symbols sorted", () => {
  const tracked = [
    { symbol: "MSFT", timeframes: ["1D"] },
    { symbol: "AAPL", timeframes: ["1D"] },
    { symbol: "GOOG", timeframes: ["1D"] },
  ];
  const coverage = [makeRow("AAPL", "1D", 100, NOW)];
  const result = computeMissingTrackedSymbols(tracked, coverage, "1D");
  assert.deepEqual(result, ["GOOG", "MSFT"]); // sorted, AAPL covered
});

test("computeMissingTrackedSymbols with empty coverage returns all tracked (sorted)", () => {
  const tracked = [
    { symbol: "MSFT", timeframes: ["1D"] },
    { symbol: "AAPL", timeframes: ["1D"] },
  ];
  const result = computeMissingTrackedSymbols(tracked, [], "1D");
  assert.deepEqual(result, ["AAPL", "MSFT"]);
});

test("computeMissingTrackedSymbols timeframe filter skips tracked symbols not in filter", () => {
  const tracked = [
    { symbol: "AAPL", timeframes: ["1D"] },    // matches filter
    { symbol: "MSFT", timeframes: ["1m"] },    // does NOT match 1D filter
  ];
  const coverage: MdBarsCoverageRow[] = []; // empty coverage
  const result = computeMissingTrackedSymbols(tracked, coverage, "1D");
  // Only AAPL should be checked (has 1D in timeframes). MSFT is excluded.
  assert.deepEqual(result, ["AAPL"]);
});

test("computeMissingTrackedSymbols null timeframeFilter checks all tracked symbols", () => {
  const tracked = [
    { symbol: "AAPL", timeframes: ["1D"] },
    { symbol: "MSFT", timeframes: ["1m"] },
  ];
  const coverage = [makeRow("AAPL", "1D", 100, NOW)];
  const result = computeMissingTrackedSymbols(tracked, coverage, null);
  // timeframeFilter=null → all tracked symbols are relevant; coverage row for AAPL 1D covers AAPL
  assert.deepEqual(result, ["MSFT"]);
});

test("COVERAGE_FRESHNESS_THRESHOLD_1D_SECS is 345600 (4 days)", () => {
  assert.equal(COVERAGE_FRESHNESS_THRESHOLD_1D_SECS, 345600);
});

test("COVERAGE_FRESHNESS_THRESHOLD_INTRADAY_SECS is 900 (15 min)", () => {
  assert.equal(COVERAGE_FRESHNESS_THRESHOLD_INTRADAY_SECS, 900);
});

test("IntradayRefreshSymbolStatus fail case has fail_reasons", () => {
  const resp: IntradayRefreshStatusResponse = {
    canonical_route: "/api/v1/market-data/intraday-refresh/status",
    truth_state: "active",
    evidence_path: "exports/market_data/intraday_refresh_20260615_093000.json",
    stale_or_missing_evidence: false,
    schema_version: "intraday-refresh-v1",
    produced_at_utc: "2026-06-15T09:30:00Z",
    mode: "once",
    source: "alpaca",
    timeframe: "1m",
    all_passed: false,
    reason: "TSLA failed gate",
    symbols: [
      {
        symbol: "TSLA",
        gate: "FAIL",
        completed_count: 12,
        latest_completed_bar_ts: "2026-06-15T10:00:00Z",
        staleness_min: 380,
        provider_source: "alpaca",
        provider_configured: true,
        provider_attempted: true,
        provider_success: false,
        rows_inserted: 0,
        rows_updated: 0,
        rows_filtered_incomplete: 0,
        rows_filtered_in_progress: 0,
        fail_reasons: ["too_few_bars: 12 < 30", "provider_failed"],
      },
    ],
    error: null,
  };
  assert.equal(resp.symbols[0].gate, "FAIL");
  assert.equal(resp.symbols[0].fail_reasons.length, 2);
  assert.equal(resp.symbols[0].fail_reasons[0], "too_few_bars: 12 < 30");
  assert.equal(resp.all_passed, false);
});

// provider sync NOT active — this panel is preview-only

test("TrackedEquitiesResponse does not contain provider sync fields", () => {
  const resp: TrackedEquitiesResponse = {
    canonical_route: "/api/v1/ingest/tracked-equities",
    truth_state: "active",
    registry_path: "config/instruments/equities.json",
    count: 1,
    symbols: [{ symbol: "AAPL", instrument_id: "equity:US:AAPL", provider: "twelvedata", venue: "NASDAQ", timeframes: ["1D"] }],
    first_symbol: "AAPL",
    last_symbol: "AAPL",
    error: null,
  };
  // No provider job, no dry_run, no api_credits fields exist on this type.
  assert.ok(!("provider_job" in resp));
  assert.ok(!("dry_run" in resp));
  assert.ok(!("api_credits" in resp));
});

// ---------------------------------------------------------------------------
// DATA-INGEST-GUI-PROVIDER-RUNNER-01: provider-specific status helpers
// ---------------------------------------------------------------------------

test("normalizeIngestJobStatus returns dry_run_completed for 'dry_run_completed'", () => {
  assert.equal(normalizeIngestJobStatus("dry_run_completed"), "dry_run_completed");
});

test("normalizeIngestJobStatus returns partial for 'partial'", () => {
  assert.equal(normalizeIngestJobStatus("partial"), "partial");
});

test("isTerminalIngestStatus returns true for dry_run_completed", () => {
  assert.ok(isTerminalIngestStatus("dry_run_completed"));
});

test("isTerminalIngestStatus returns true for partial", () => {
  assert.ok(isTerminalIngestStatus("partial"));
});

// ---------------------------------------------------------------------------
// cancelIngestJob
// ---------------------------------------------------------------------------

test("cancelIngestJob POSTs to the canonical cancel endpoint", async () => {
  const calls: Array<{ url: string; init?: RequestInit }> = [];
  const originalFetch = globalThis.fetch;
  globalThis.fetch = (async (url: RequestInfo | URL, init?: RequestInit) => {
    calls.push({ url: String(url), init });
    return new Response(
      JSON.stringify({
        canonical_route: "/api/v1/ingest/jobs/:job_id/cancel",
        truth_state: "cancel_accepted",
        accepted: true,
        job_id: "job-123",
        status: "cancelled",
        error: null,
      }),
      {
        status: 202,
        headers: { "content-type": "application/json" },
      },
    );
  }) as typeof fetch;

  try {
    const result = await cancelIngestJob("job-123");
    assert.equal(result.ok, true);
    assert.equal(result.status, 202);
    assert.equal(result.data?.status, "cancelled");
    assert.equal(calls.length, 1);
    assert.equal(calls[0].url, "http://127.0.0.1:8899/api/v1/ingest/jobs/job-123/cancel");
    assert.equal(calls[0].init?.method, "POST");
    assert.equal(calls[0].init?.body, "{}");
  } finally {
    globalThis.fetch = originalFetch;
  }
});

test("cancelIngestJob maps HTTP 404 to notFound", async () => {
  const originalFetch = globalThis.fetch;
  globalThis.fetch = (async () =>
    new Response(
      JSON.stringify({
        canonical_route: "/api/v1/ingest/jobs/:job_id/cancel",
        truth_state: "not_found",
        accepted: false,
        job_id: "missing-job",
        error: "job_id missing-job not found",
      }),
      {
        status: 404,
        headers: { "content-type": "application/json" },
      },
    )) as typeof fetch;

  try {
    const result = await cancelIngestJob("missing-job");
    assert.equal(result.ok, false);
    assert.equal(result.status, 404);
    assert.equal(result.notFound, true);
    assert.equal(result.error, "Ingest job not found.");
  } finally {
    globalThis.fetch = originalFetch;
  }
});

// ---------------------------------------------------------------------------
// isProviderSyncAllowed
// ---------------------------------------------------------------------------

test("isProviderSyncAllowed: dry-run (allowProviderApiCalls=false) is always allowed", () => {
  assert.ok(isProviderSyncAllowed(false, ""));
  assert.ok(isProviderSyncAllowed(false, "anything"));
  assert.ok(isProviderSyncAllowed(false, "SYNC"));
});

test("isProviderSyncAllowed: real sync (allowProviderApiCalls=true) requires SYNC confirmation", () => {
  assert.ok(!isProviderSyncAllowed(true, ""));
  assert.ok(!isProviderSyncAllowed(true, "sync"));
  assert.ok(!isProviderSyncAllowed(true, "Sync"));
  assert.ok(isProviderSyncAllowed(true, "SYNC"));
  // trim() is applied so leading/trailing whitespace is accepted
  assert.ok(isProviderSyncAllowed(true, " SYNC "));
  assert.ok(isProviderSyncAllowed(true, "SYNC "));
});

// ---------------------------------------------------------------------------
// buildProviderJobRequest
// ---------------------------------------------------------------------------

test("buildProviderJobRequest dry-run default has correct safe payload", () => {
  const req = buildProviderJobRequest({ dryRun: true, allowProviderApiCalls: false });
  assert.equal(req.source, "twelvedata");
  assert.equal(req.mode, "sync_provider");
  assert.equal(req.timeframe, "1D");
  assert.equal(req.symbols_source, "registry");
  assert.equal(req.registry_path, "config/instruments/equities.json");
  assert.equal(req.asset_class, "equity");
  assert.equal(req.dry_run, true);
  assert.equal(req.allow_provider_api_calls, false);
  assert.equal(req.start, null);
  assert.equal(req.end, null);
  assert.equal(req.api_credits_per_minute, null);
  assert.equal(req.api_credits_per_day, null);
});

test("buildProviderJobRequest real sync payload has dry_run=false and allow_provider_api_calls=true", () => {
  const req = buildProviderJobRequest({ dryRun: false, allowProviderApiCalls: true });
  assert.equal(req.dry_run, false);
  assert.equal(req.allow_provider_api_calls, true);
  assert.equal(req.source, "twelvedata");
});

test("buildProviderJobRequest passes optional date range and credit limits", () => {
  const req = buildProviderJobRequest({
    dryRun: true,
    allowProviderApiCalls: false,
    start: "2025-01-01",
    end: "2025-12-31",
    apiCreditsPerMinute: 8,
    apiCreditsPerDay: 800,
  });
  assert.equal(req.start, "2025-01-01");
  assert.equal(req.end, "2025-12-31");
  assert.equal(req.api_credits_per_minute, 8);
  assert.equal(req.api_credits_per_day, 800);
});

test("buildProviderJobRequest omits optional fields when not provided", () => {
  const req = buildProviderJobRequest({ dryRun: true, allowProviderApiCalls: false });
  assert.equal(req.api_credits_per_minute, null);
  assert.equal(req.api_credits_per_day, null);
});

// ---------------------------------------------------------------------------
// buildActiveProviderJob
// ---------------------------------------------------------------------------

function makeProviderStatusResponse(
  status: string,
  overrides: Partial<IngestJobStatusResponse> = {},
): IngestJobStatusResponse {
  return {
    truth_state: "active",
    job_id: "prov-001",
    status,
    source: "twelvedata",
    mode: "sync_provider",
    timeframe: "1D",
    csv_path: null,
    created_at_utc: "2026-06-15T10:00:00Z",
    started_at_utc: null,
    completed_at_utc: null,
    rows_read: null,
    rows_inserted: null,
    rows_rejected: null,
    quality_report_path: null,
    error: null,
    dry_run: true,
    provider_api_calls_allowed: false,
    api_calls_made: 0,
    symbols_source: "registry",
    registry_path_used: "config/instruments/equities.json",
    symbols_count: 88,
    planned_first_symbol: "AAPL",
    planned_last_symbol: "XOM",
    asset_class: "equity",
    provider_enabled: true,
    provider_verification_status: "verified",
    symbols_completed: null,
    symbols_failed: null,
    ...overrides,
  };
}

test("buildActiveProviderJob maps dry_run_completed job correctly", () => {
  const resp = makeProviderStatusResponse("dry_run_completed");
  const job = buildActiveProviderJob(resp);
  assert.equal(job.jobId, "prov-001");
  assert.equal(job.status, "dry_run_completed");
  assert.equal(job.dryRun, true);
  assert.equal(job.allowProviderApiCalls, false);
  assert.equal(job.symbolsCount, 88);
  assert.equal(job.plannedFirstSymbol, "AAPL");
  assert.equal(job.plannedLastSymbol, "XOM");
  assert.equal(job.apiCallsMade, 0);
  assert.equal(job.symbolsCompleted, null);
  assert.equal(job.symbolsFailed, null);
});

test("buildActiveProviderJob maps partial job with symbols_completed and symbols_failed", () => {
  const resp = makeProviderStatusResponse("partial", {
    dry_run: false,
    provider_api_calls_allowed: true,
    api_calls_made: 70,
    symbols_completed: 62,
    symbols_failed: 8,
    rows_inserted: 124600,
    rows_rejected: 0,
  });
  const job = buildActiveProviderJob(resp);
  assert.equal(job.status, "partial");
  assert.equal(job.dryRun, false);
  assert.equal(job.allowProviderApiCalls, true);
  assert.equal(job.apiCallsMade, 70);
  assert.equal(job.symbolsCompleted, 62);
  assert.equal(job.symbolsFailed, 8);
  assert.equal(job.rowsInserted, 124600);
});

test("buildActiveProviderJob maps completed job with all symbol counts", () => {
  const resp = makeProviderStatusResponse("completed", {
    dry_run: false,
    provider_api_calls_allowed: true,
    api_calls_made: 88,
    symbols_completed: 88,
    symbols_failed: 0,
    rows_inserted: 176000,
    rows_rejected: 0,
    completed_at_utc: "2026-06-15T10:45:00Z",
  });
  const job = buildActiveProviderJob(resp);
  assert.equal(job.status, "completed");
  assert.equal(job.symbolsCompleted, 88);
  assert.equal(job.symbolsFailed, 0);
  assert.equal(job.rowsInserted, 176000);
  assert.equal(job.completedAt, "2026-06-15T10:45:00Z");
});

test("buildActiveProviderJob maps failed job with error", () => {
  const resp = makeProviderStatusResponse("failed", {
    error: "registry_unavailable: config/instruments/equities.json not found",
  });
  const job = buildActiveProviderJob(resp);
  assert.equal(job.status, "failed");
  assert.equal(job.error, "registry_unavailable: config/instruments/equities.json not found");
});

test("buildActiveProviderJob maps unknown daemon status to unknown", () => {
  const resp = makeProviderStatusResponse("some_future_state");
  const job = buildActiveProviderJob(resp);
  assert.equal(job.status, "unknown");
});

test("buildActiveProviderJob maps cancelled job without dropping progress counters", () => {
  const resp = makeProviderStatusResponse("cancelled", {
    dry_run: false,
    provider_api_calls_allowed: true,
    api_calls_made: 4,
    symbols_completed: 3,
    symbols_failed: 1,
    rows_inserted: 150,
    rows_rejected: 2,
    completed_at_utc: "2026-06-15T10:12:00Z",
    error: "cancelled by operator",
  });
  const job = buildActiveProviderJob(resp);
  assert.equal(job.status, "cancelled");
  assert.equal(isTerminalIngestStatus(job.status), true);
  assert.equal(job.apiCallsMade, 4);
  assert.equal(job.symbolsCompleted, 3);
  assert.equal(job.symbolsFailed, 1);
  assert.equal(job.rowsInserted, 150);
  assert.equal(job.rowsRejected, 2);
  assert.equal(job.error, "cancelled by operator");
});

test("IngestJobStatusResponse with symbols_completed and symbols_failed fields is valid", () => {
  const resp = makeProviderStatusResponse("partial", {
    symbols_completed: 50,
    symbols_failed: 10,
  });
  assert.equal(resp.symbols_completed, 50);
  assert.equal(resp.symbols_failed, 10);
  assert.ok(resp.truth_state === "active");
});

// ---------------------------------------------------------------------------
// DATA-PROVIDER-GUI-FEED-SCHEDULER-01: latest-bar feed scheduler helpers
// ---------------------------------------------------------------------------

test("parseMarketDataFeedSymbols normalizes comma and whitespace separated symbols", () => {
  assert.deepEqual(parseMarketDataFeedSymbols("aapl, msft\nAAPL  spy"), ["AAPL", "MSFT", "SPY"]);
});

test("isMarketDataFeedRealActionAllowed requires exact real-action confirmation", () => {
  assert.equal(isMarketDataFeedRealActionAllowed(false, "", "POLL"), true);
  assert.equal(isMarketDataFeedRealActionAllowed(true, "", "POLL"), false);
  assert.equal(isMarketDataFeedRealActionAllowed(true, "poll", "POLL"), false);
  assert.equal(isMarketDataFeedRealActionAllowed(true, " POLL ", "POLL"), true);
  assert.equal(isMarketDataFeedRealActionAllowed(true, " START ", "START"), true);
});

test("buildMarketDataFeedPollOnceRequest dry-run omits provider-call allowance", () => {
  const req = buildMarketDataFeedPollOnceRequest({
    providerId: "alpaca",
    symbols: ["AAPL"],
    timeframe: "5m",
    dryRun: true,
    allowProviderApiCalls: false,
  });
  assert.equal(req.provider_id, "alpaca");
  assert.deepEqual(req.symbols, ["AAPL"]);
  assert.equal(req.timeframe, "5m");
  assert.equal(req.dry_run, true);
  assert.equal("allow_provider_api_calls" in req, false);
});

test("buildMarketDataFeedPollOnceRequest real poll sends provider-call allowance", () => {
  const req = buildMarketDataFeedPollOnceRequest({
    providerId: "alpaca",
    symbols: ["AAPL"],
    timeframe: "5m",
    dryRun: false,
    allowProviderApiCalls: true,
  });
  assert.equal(req.dry_run, false);
  assert.equal(req.allow_provider_api_calls, true);
});

test("buildMarketDataFeedSchedulerStartRequest dry-run omits provider-call allowance", () => {
  const req = buildMarketDataFeedSchedulerStartRequest({
    providerId: "alpaca",
    symbols: ["AAPL"],
    timeframe: "5m",
    dryRun: true,
    allowProviderApiCalls: false,
    pollImmediately: true,
  });
  assert.equal(req.dry_run, true);
  assert.equal(req.poll_immediately, true);
  assert.equal("allow_provider_api_calls" in req, false);
});

test("buildMarketDataFeedSchedulerStartRequest confirmed real start sends provider-call allowance", () => {
  const req = buildMarketDataFeedSchedulerStartRequest({
    providerId: "alpaca",
    symbols: ["AAPL"],
    timeframe: "5m",
    dryRun: false,
    allowProviderApiCalls: true,
    pollImmediately: false,
  });
  assert.equal(req.dry_run, false);
  assert.equal(req.allow_provider_api_calls, true);
  assert.equal(req.poll_immediately, false);
});

test("normalizeMarketDataFeedSchedulerStatusResponse parses running scheduler status", () => {
  const status = normalizeMarketDataFeedSchedulerStatusResponse({
    canonical_route: "/api/v1/market-data/feed/scheduler/status",
    truth_state: "running",
    limitation: "process_local_only_not_persisted",
    running: true,
    provider_id: "alpaca",
    timeframe: "5m",
    symbols: ["AAPL"],
    last_poll_utc: "2024-01-01T00:10:30+00:00",
    next_poll_utc: "2024-01-01T00:15:00+00:00",
    latest_expected_closed_bar_utc: "2024-01-01T00:05:00+00:00",
    last_result: {
      canonical_route: "/api/v1/market-data/feed/poll-once",
      truth_state: "dry_run",
      provider_id: "alpaca",
      timeframe: "5m",
      dry_run: true,
      provider_api_calls_allowed: false,
      symbols_count: 1,
      poll_time_utc: "2024-01-01T00:10:30+00:00",
      latest_expected_closed_bar_ts: 1704067500,
      next_poll_ts: 1704068100,
      inserted_count: 0,
      updated_count: 0,
      skipped_count: 0,
      error_count: 0,
      api_calls_made: 0,
      symbols: [],
      error: null,
    },
    last_error: null,
    started_at_utc: "2024-01-01T00:10:30+00:00",
    stopped_at_utc: null,
    poll_count: 1,
    inserted_count: 0,
    unchanged_or_skipped_count: 0,
    error_count: 0,
  });
  assert.equal(status.truth_state, "running");
  assert.equal(status.running, true);
  assert.equal(status.provider_id, "alpaca");
  assert.equal(status.last_result?.truth_state, "dry_run");
});

test("normalizeMarketDataFeedStatusResponse treats missing backend fields as unknown or null", () => {
  const status = normalizeMarketDataFeedStatusResponse({});
  assert.equal(status.truth_state, "unknown");
  assert.equal(status.limitation, null);
  assert.equal(status.last_poll, null);
});

test("getMarketDataFeedSchedulerStatus GET parses scheduler status", async () => {
  const originalFetch = globalThis.fetch;
  globalThis.fetch = (async (url: RequestInfo | URL, init?: RequestInit) => {
    assert.equal(String(url), "http://127.0.0.1:8899/api/v1/market-data/feed/scheduler/status");
    assert.equal(init?.method, "GET");
    return new Response(
      JSON.stringify({
        canonical_route: "/api/v1/market-data/feed/scheduler/status",
        truth_state: "not_started",
        limitation: "process_local_only_not_persisted",
        running: false,
        provider_id: null,
        timeframe: null,
        symbols: [],
        last_poll_utc: null,
        next_poll_utc: null,
        latest_expected_closed_bar_utc: null,
        last_result: null,
        last_error: null,
        started_at_utc: null,
        stopped_at_utc: null,
        poll_count: 0,
        inserted_count: 0,
        unchanged_or_skipped_count: 0,
        error_count: 0,
      }),
      { status: 200, headers: { "content-type": "application/json" } },
    );
  }) as typeof fetch;

  try {
    const result = await getMarketDataFeedSchedulerStatus();
    assert.equal(result.ok, true);
    assert.equal(result.data?.truth_state, "not_started");
    assert.equal(result.data?.running, false);
  } finally {
    globalThis.fetch = originalFetch;
  }
});

test("pollMarketDataFeedOnce sends dry-run request without provider-call allowance", async () => {
  const calls: Array<{ url: string; init?: RequestInit }> = [];
  const originalFetch = globalThis.fetch;
  globalThis.fetch = (async (url: RequestInfo | URL, init?: RequestInit) => {
    calls.push({ url: String(url), init });
    return new Response(
      JSON.stringify({
        canonical_route: "/api/v1/market-data/feed/poll-once",
        truth_state: "dry_run",
        provider_id: "alpaca",
        timeframe: "5m",
        dry_run: true,
        provider_api_calls_allowed: false,
        symbols_count: 1,
        poll_time_utc: "2024-01-01T00:10:30+00:00",
        latest_expected_closed_bar_ts: 1704067500,
        next_poll_ts: 1704068100,
        inserted_count: 0,
        updated_count: 0,
        skipped_count: 0,
        error_count: 0,
        api_calls_made: 0,
        symbols: [],
        error: null,
      }),
      { status: 200, headers: { "content-type": "application/json" } },
    );
  }) as typeof fetch;

  try {
    const req = buildMarketDataFeedPollOnceRequest({
      providerId: "alpaca",
      symbols: ["AAPL"],
      timeframe: "5m",
      dryRun: true,
      allowProviderApiCalls: false,
    });
    const result = await pollMarketDataFeedOnce(req);
    const body = JSON.parse(String(calls[0].init?.body));
    assert.equal(result.ok, true);
    assert.equal(calls[0].url, "http://127.0.0.1:8899/api/v1/market-data/feed/poll-once");
    assert.equal(calls[0].init?.method, "POST");
    assert.equal(body.dry_run, true);
    assert.equal("allow_provider_api_calls" in body, false);
  } finally {
    globalThis.fetch = originalFetch;
  }
});

test("pollMarketDataFeedOnce sends confirmed real request with provider-call allowance", async () => {
  const calls: Array<{ url: string; init?: RequestInit }> = [];
  const originalFetch = globalThis.fetch;
  globalThis.fetch = (async (url: RequestInfo | URL, init?: RequestInit) => {
    calls.push({ url: String(url), init });
    return new Response(
      JSON.stringify({
        canonical_route: "/api/v1/market-data/feed/poll-once",
        truth_state: "completed",
        provider_id: "alpaca",
        timeframe: "5m",
        dry_run: false,
        provider_api_calls_allowed: true,
        symbols_count: 1,
        poll_time_utc: "2024-01-01T00:10:30+00:00",
        latest_expected_closed_bar_ts: 1704067500,
        next_poll_ts: 1704068100,
        inserted_count: 1,
        updated_count: 0,
        skipped_count: 0,
        error_count: 0,
        api_calls_made: 1,
        symbols: [],
        error: null,
      }),
      { status: 200, headers: { "content-type": "application/json" } },
    );
  }) as typeof fetch;

  try {
    const req = buildMarketDataFeedPollOnceRequest({
      providerId: "alpaca",
      symbols: ["AAPL"],
      timeframe: "5m",
      dryRun: false,
      allowProviderApiCalls: true,
    });
    const result = await pollMarketDataFeedOnce(req);
    const body = JSON.parse(String(calls[0].init?.body));
    assert.equal(result.ok, true);
    assert.equal(body.dry_run, false);
    assert.equal(body.allow_provider_api_calls, true);
  } finally {
    globalThis.fetch = originalFetch;
  }
});

test("startMarketDataFeedScheduler dry-run request does not send provider-call allowance", async () => {
  const calls: Array<{ url: string; init?: RequestInit }> = [];
  const originalFetch = globalThis.fetch;
  globalThis.fetch = (async (url: RequestInfo | URL, init?: RequestInit) => {
    calls.push({ url: String(url), init });
    return new Response(
      JSON.stringify({
        canonical_route: "/api/v1/market-data/feed/scheduler/start",
        truth_state: "started",
        limitation: "process_local_only_not_persisted",
        running: true,
        provider_id: "alpaca",
        timeframe: "5m",
        symbols: ["AAPL"],
        last_poll_utc: null,
        next_poll_utc: "2024-01-01T00:15:00+00:00",
        latest_expected_closed_bar_utc: "2024-01-01T00:05:00+00:00",
        last_result: null,
        last_error: null,
        started_at_utc: "2024-01-01T00:10:30+00:00",
        stopped_at_utc: null,
        poll_count: 0,
        inserted_count: 0,
        unchanged_or_skipped_count: 0,
        error_count: 0,
      }),
      { status: 200, headers: { "content-type": "application/json" } },
    );
  }) as typeof fetch;

  try {
    const req = buildMarketDataFeedSchedulerStartRequest({
      providerId: "alpaca",
      symbols: ["AAPL"],
      timeframe: "5m",
      dryRun: true,
      allowProviderApiCalls: false,
      pollImmediately: false,
    });
    const result = await startMarketDataFeedScheduler(req);
    const body = JSON.parse(String(calls[0].init?.body));
    assert.equal(result.ok, true);
    assert.equal(calls[0].url, "http://127.0.0.1:8899/api/v1/market-data/feed/scheduler/start");
    assert.equal(body.dry_run, true);
    assert.equal(body.poll_immediately, false);
    assert.equal("allow_provider_api_calls" in body, false);
  } finally {
    globalThis.fetch = originalFetch;
  }
});

test("startMarketDataFeedScheduler confirmed real request sends provider-call allowance", async () => {
  const calls: Array<{ url: string; init?: RequestInit }> = [];
  const originalFetch = globalThis.fetch;
  globalThis.fetch = (async (url: RequestInfo | URL, init?: RequestInit) => {
    calls.push({ url: String(url), init });
    return new Response(
      JSON.stringify({
        canonical_route: "/api/v1/market-data/feed/scheduler/start",
        truth_state: "started",
        limitation: "process_local_only_not_persisted",
        running: true,
        provider_id: "alpaca",
        timeframe: "5m",
        symbols: ["AAPL"],
        last_poll_utc: null,
        next_poll_utc: "2024-01-01T00:15:00+00:00",
        latest_expected_closed_bar_utc: "2024-01-01T00:05:00+00:00",
        last_result: null,
        last_error: null,
        started_at_utc: "2024-01-01T00:10:30+00:00",
        stopped_at_utc: null,
        poll_count: 0,
        inserted_count: 0,
        unchanged_or_skipped_count: 0,
        error_count: 0,
      }),
      { status: 200, headers: { "content-type": "application/json" } },
    );
  }) as typeof fetch;

  try {
    const req = buildMarketDataFeedSchedulerStartRequest({
      providerId: "alpaca",
      symbols: ["AAPL"],
      timeframe: "5m",
      dryRun: false,
      allowProviderApiCalls: true,
      pollImmediately: true,
    });
    const result = await startMarketDataFeedScheduler(req);
    const body = JSON.parse(String(calls[0].init?.body));
    assert.equal(result.ok, true);
    assert.equal(body.dry_run, false);
    assert.equal(body.allow_provider_api_calls, true);
    assert.equal(body.poll_immediately, true);
  } finally {
    globalThis.fetch = originalFetch;
  }
});

test("stopMarketDataFeedScheduler POSTs an empty body to canonical stop endpoint", async () => {
  const calls: Array<{ url: string; init?: RequestInit }> = [];
  const originalFetch = globalThis.fetch;
  globalThis.fetch = (async (url: RequestInfo | URL, init?: RequestInit) => {
    calls.push({ url: String(url), init });
    return new Response(
      JSON.stringify({
        canonical_route: "/api/v1/market-data/feed/scheduler/stop",
        truth_state: "not_running",
        limitation: "process_local_only_not_persisted",
        running: false,
        provider_id: null,
        timeframe: null,
        symbols: [],
        last_poll_utc: null,
        next_poll_utc: null,
        latest_expected_closed_bar_utc: null,
        last_result: null,
        last_error: null,
        started_at_utc: null,
        stopped_at_utc: null,
        poll_count: 0,
        inserted_count: 0,
        unchanged_or_skipped_count: 0,
        error_count: 0,
      }),
      { status: 200, headers: { "content-type": "application/json" } },
    );
  }) as typeof fetch;

  try {
    const result = await stopMarketDataFeedScheduler();
    assert.equal(result.ok, true);
    assert.equal(calls[0].url, "http://127.0.0.1:8899/api/v1/market-data/feed/scheduler/stop");
    assert.equal(calls[0].init?.method, "POST");
    assert.equal(calls[0].init?.body, "{}");
  } finally {
    globalThis.fetch = originalFetch;
  }
});

test("startMarketDataFeedScheduler normalizes already-running error response", async () => {
  const originalFetch = globalThis.fetch;
  globalThis.fetch = (async () =>
    new Response(
      JSON.stringify({
        canonical_route: "/api/v1/market-data/feed/scheduler/start",
        truth_state: "already_running",
        last_error: "latest-bar scheduler is already running",
      }),
      { status: 409, headers: { "content-type": "application/json" } },
    )) as typeof fetch;

  try {
    const req = buildMarketDataFeedSchedulerStartRequest({
      providerId: "alpaca",
      symbols: ["AAPL"],
      timeframe: "5m",
      dryRun: true,
      allowProviderApiCalls: false,
      pollImmediately: false,
    });
    const result = await startMarketDataFeedScheduler(req);
    assert.equal(result.ok, false);
    assert.equal(result.status, 409);
    assert.equal(result.error, "latest-bar scheduler is already running");
    assert.equal(result.data?.truth_state, "already_running");
    assert.equal(result.data?.last_error, "latest-bar scheduler is already running");
  } finally {
    globalThis.fetch = originalFetch;
  }
});
