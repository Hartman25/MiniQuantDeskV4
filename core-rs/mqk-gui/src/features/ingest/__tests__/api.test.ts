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
  fetchLatestMarkStatus,
  formatUnixSecondsDateTime,
  isLatestMarkEvidenceUnsafe,
  isLatestMarkStatusActive,
  latestMarkStatusTruthLabel,
  fetchKrakenOhlcStatus,
  isKrakenOhlcEvidenceUnsafe,
  isKrakenOhlcStatusActive,
  krakenOhlcStatusTruthLabel,
  KRAKEN_OHLC_STATUS_WARNING_TEXT,
  fetchCryptoRegistryReadiness,
  isCryptoRegistryReadinessActive,
  isCryptoRegistryReadinessUnsafe,
  cryptoRegistryReadinessTruthLabel,
  CRYPTO_REGISTRY_READINESS_WARNING_TEXT,
  fetchKrakenSchedulerReadiness,
  isKrakenSchedulerReadinessActive,
  isKrakenSchedulerReadinessUnsafe,
  krakenSchedulerReadinessTruthLabel,
  KRAKEN_SCHEDULER_READINESS_WARNING_TEXT,
  fetchKrakenSchedulerTaskStatus,
  isKrakenSchedulerTaskEvidenceUnsafe,
  isKrakenSchedulerTaskStatusActive,
  krakenSchedulerTaskStatusTruthLabel,
  KRAKEN_SCHEDULER_TASK_STATUS_WARNING_TEXT,
  buildDailyDataReadinessDiagnosticText,
  classifyDailyDataReadinessDisplay,
  DAILY_DATA_READINESS_ROUTE,
  fetchDailyDataReadiness,
  formatReadinessNumber,
  normalizeDailyDataReadinessResponse,
} from "../api.ts";
import type { CryptoRegistryReadinessResponse, DailyDataReadinessAssignmentResponse, DailyDataReadinessResponse, IngestJobStatusResponse, IntradayRefreshStatusResponse, KrakenOhlcStatusResponse, KrakenSchedulerReadinessResponse, KrakenSchedulerTaskStatusResponse, LatestMarkStatusResponse, MdBarsCoverageResponse, MdBarsCoverageRow, TrackedEquitiesResponse } from "../types.ts";

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

// ---------------------------------------------------------------------------
// CRYPTO-DATA-01Q-R-LATEST-MARK-GUI-SURFACE-BUNDLE-01-COMBINED:
// Latest-mark evidence status helpers
// ---------------------------------------------------------------------------

function makeActiveLatestMarkStatusResponse(
  overrides: Partial<LatestMarkStatusResponse> = {},
): LatestMarkStatusResponse {
  return {
    canonical_route: "/api/v1/market-data/latest-marks/status",
    truth_state: "active",
    provider: "coinlore",
    produced_at_utc: "2026-07-04T12:00:00Z",
    evidence_path: "exports/market_data/coinlore_latest_mark_20260704_120000.json",
    stale_or_missing_evidence: false,
    max_evidence_age_secs: 86400,
    network_call_made: false,
    db_write: false,
    md_bars_write: false,
    completed_bar_claim: false,
    provider_enabled: false,
    symbols_requested: ["BTC/USD", "ETH/USD"],
    marks: [
      {
        canonical_symbol: "BTC/USD",
        provider_id: "coinlore",
        provider_symbol: "BTC",
        provider_coin_id: "90",
        price_usd: "62906.61",
        volume24_usd: "16261421089.448309",
        as_of_client_request_ts: 1783264281,
        provider_ts: null,
        truth_state: "client_observed_only",
        kind: "ticker",
      },
      {
        canonical_symbol: "ETH/USD",
        provider_id: "coinlore",
        provider_symbol: "ETH",
        provider_coin_id: "80",
        price_usd: "1777.74",
        volume24_usd: "9252669028.469555",
        as_of_client_request_ts: 1783264281,
        provider_ts: null,
        truth_state: "client_observed_only",
        kind: "ticker",
      },
    ],
    all_passed: true,
    reason_code: "latest_mark_evidence_generated",
    fail_reasons: [],
    error: null,
    ...overrides,
  };
}

// isLatestMarkStatusActive

test("isLatestMarkStatusActive returns true for active", () => {
  assert.ok(isLatestMarkStatusActive("active"));
});

test("isLatestMarkStatusActive returns false for stale", () => {
  assert.ok(!isLatestMarkStatusActive("stale"));
});

test("isLatestMarkStatusActive returns false for no_evidence", () => {
  assert.ok(!isLatestMarkStatusActive("no_evidence"));
});

test("isLatestMarkStatusActive returns false for parse_error", () => {
  assert.ok(!isLatestMarkStatusActive("parse_error"));
});

test("isLatestMarkStatusActive returns false for unsafe_evidence", () => {
  assert.ok(!isLatestMarkStatusActive("unsafe_evidence"));
});

test("isLatestMarkStatusActive returns false for backend_unavailable", () => {
  assert.ok(!isLatestMarkStatusActive("backend_unavailable"));
});

// latestMarkStatusTruthLabel

test("latestMarkStatusTruthLabel active -> active", () => {
  assert.equal(latestMarkStatusTruthLabel("active"), "active");
});

test("latestMarkStatusTruthLabel stale -> stale", () => {
  assert.equal(latestMarkStatusTruthLabel("stale"), "stale");
});

test("latestMarkStatusTruthLabel no_evidence -> no evidence", () => {
  assert.equal(latestMarkStatusTruthLabel("no_evidence"), "no evidence");
});

test("latestMarkStatusTruthLabel parse_error -> parse error", () => {
  assert.equal(latestMarkStatusTruthLabel("parse_error"), "parse error");
});

test("latestMarkStatusTruthLabel unsafe_evidence is worded as a severe fail-closed state", () => {
  const label = latestMarkStatusTruthLabel("unsafe_evidence");
  assert.match(label, /unsafe/i);
  assert.notEqual(label, "unsafe_evidence"); // must not be a bare passthrough
});

test("latestMarkStatusTruthLabel backend_unavailable -> backend unavailable", () => {
  assert.equal(latestMarkStatusTruthLabel("backend_unavailable"), "backend unavailable");
});

test("latestMarkStatusTruthLabel unknown string passes through", () => {
  assert.equal(latestMarkStatusTruthLabel("some_other_state"), "some_other_state");
});

// isLatestMarkEvidenceUnsafe (defense-in-depth check)

test("isLatestMarkEvidenceUnsafe returns true when truth_state is unsafe_evidence", () => {
  const resp = makeActiveLatestMarkStatusResponse({ truth_state: "unsafe_evidence" });
  assert.ok(isLatestMarkEvidenceUnsafe(resp));
});

test("isLatestMarkEvidenceUnsafe returns true when db_write=true even if truth_state says active", () => {
  const resp = makeActiveLatestMarkStatusResponse({ db_write: true });
  assert.ok(isLatestMarkEvidenceUnsafe(resp));
});

test("isLatestMarkEvidenceUnsafe returns true when md_bars_write=true even if truth_state says active", () => {
  const resp = makeActiveLatestMarkStatusResponse({ md_bars_write: true });
  assert.ok(isLatestMarkEvidenceUnsafe(resp));
});

test("isLatestMarkEvidenceUnsafe returns true when completed_bar_claim=true even if truth_state says active", () => {
  const resp = makeActiveLatestMarkStatusResponse({ completed_bar_claim: true });
  assert.ok(isLatestMarkEvidenceUnsafe(resp));
});

test("isLatestMarkEvidenceUnsafe returns false for a genuinely safe active response", () => {
  const resp = makeActiveLatestMarkStatusResponse();
  assert.ok(!isLatestMarkEvidenceUnsafe(resp));
});

test("isLatestMarkEvidenceUnsafe returns false for provider_enabled=false alone (not an error by itself)", () => {
  const resp = makeActiveLatestMarkStatusResponse({ provider_enabled: false });
  assert.ok(!isLatestMarkEvidenceUnsafe(resp));
});

// formatUnixSecondsDateTime

test("formatUnixSecondsDateTime formats a known unix timestamp", () => {
  assert.equal(formatUnixSecondsDateTime(1783264281), "2026-07-05 15:11:21");
});

test("formatUnixSecondsDateTime returns em dash for null", () => {
  assert.equal(formatUnixSecondsDateTime(null), "—");
});

test("formatUnixSecondsDateTime returns em dash for undefined", () => {
  assert.equal(formatUnixSecondsDateTime(undefined), "—");
});

test("formatUnixSecondsDateTime returns em dash for zero", () => {
  assert.equal(formatUnixSecondsDateTime(0), "—");
});

// LatestMarkStatusResponse shape — mirrors backend truth_state contract

test("LatestMarkStatusResponse active shape has BTC/USD and ETH/USD marks", () => {
  const resp = makeActiveLatestMarkStatusResponse();
  assert.ok(isLatestMarkStatusActive(resp.truth_state));
  assert.ok(!isLatestMarkEvidenceUnsafe(resp));
  assert.equal(resp.marks.length, 2);
  assert.equal(resp.marks[0].canonical_symbol, "BTC/USD");
  assert.equal(resp.marks[0].price_usd, "62906.61");
  assert.equal(resp.marks[1].canonical_symbol, "ETH/USD");
  assert.equal(resp.marks[1].price_usd, "1777.74");
  assert.equal(resp.stale_or_missing_evidence, false);
  assert.equal(resp.provider_enabled, false);
  assert.equal(resp.db_write, false);
  assert.equal(resp.md_bars_write, false);
  assert.equal(resp.completed_bar_claim, false);
});

test("LatestMarkStatusResponse no_evidence shape is honest (empty marks, path null)", () => {
  const resp: LatestMarkStatusResponse = {
    canonical_route: "/api/v1/market-data/latest-marks/status",
    truth_state: "no_evidence",
    provider: null,
    produced_at_utc: null,
    evidence_path: null,
    stale_or_missing_evidence: true,
    max_evidence_age_secs: 86400,
    network_call_made: null,
    db_write: null,
    md_bars_write: null,
    completed_bar_claim: null,
    provider_enabled: null,
    symbols_requested: [],
    marks: [],
    all_passed: null,
    reason_code: null,
    fail_reasons: [],
    error: null,
  };
  assert.ok(!isLatestMarkStatusActive(resp.truth_state));
  assert.equal(resp.marks.length, 0);
  assert.equal(resp.evidence_path, null);
  assert.equal(resp.stale_or_missing_evidence, true);
});

test("LatestMarkStatusResponse stale shape has stale_or_missing_evidence=true", () => {
  const resp = makeActiveLatestMarkStatusResponse({
    truth_state: "stale",
    stale_or_missing_evidence: true,
  });
  assert.ok(!isLatestMarkStatusActive(resp.truth_state));
  assert.equal(resp.stale_or_missing_evidence, true);
});

test("LatestMarkStatusResponse parse_error shape has error field and empty marks", () => {
  const resp: LatestMarkStatusResponse = {
    canonical_route: "/api/v1/market-data/latest-marks/status",
    truth_state: "parse_error",
    provider: null,
    produced_at_utc: null,
    evidence_path: "exports/market_data/coinlore_latest_mark_20260704_120000.json",
    stale_or_missing_evidence: true,
    max_evidence_age_secs: 86400,
    network_call_made: null,
    db_write: null,
    md_bars_write: null,
    completed_bar_claim: null,
    provider_enabled: null,
    symbols_requested: [],
    marks: [],
    all_passed: null,
    reason_code: null,
    fail_reasons: [],
    error: "unsupported schema_version: unknown-v99",
  };
  assert.ok(!isLatestMarkStatusActive(resp.truth_state));
  assert.equal(resp.error, "unsupported schema_version: unknown-v99");
  assert.equal(resp.marks.length, 0);
});

test("LatestMarkStatusResponse unsafe_evidence shape is never active and flagged unsafe", () => {
  const resp = makeActiveLatestMarkStatusResponse({
    truth_state: "unsafe_evidence",
    db_write: true,
    error: "evidence claims db_write=true",
  });
  assert.ok(!isLatestMarkStatusActive(resp.truth_state));
  assert.ok(isLatestMarkEvidenceUnsafe(resp));
  assert.equal(resp.error, "evidence claims db_write=true");
});

test("LatestMarkStatusResponse backend_unavailable shape is honest", () => {
  const resp: LatestMarkStatusResponse = {
    canonical_route: "/api/v1/market-data/latest-marks/status",
    truth_state: "backend_unavailable",
    provider: null,
    produced_at_utc: null,
    evidence_path: null,
    stale_or_missing_evidence: true,
    max_evidence_age_secs: 86400,
    network_call_made: null,
    db_write: null,
    md_bars_write: null,
    completed_bar_claim: null,
    provider_enabled: null,
    symbols_requested: [],
    marks: [],
    all_passed: null,
    reason_code: null,
    fail_reasons: [],
    error: "evidence directory could not be read",
  };
  assert.ok(!isLatestMarkStatusActive(resp.truth_state));
  assert.equal(resp.error, "evidence directory could not be read");
});

// fetchLatestMarkStatus

test("fetchLatestMarkStatus GETs the canonical latest-marks status route", async () => {
  const calls: Array<{ url: string; init?: RequestInit }> = [];
  const originalFetch = globalThis.fetch;
  globalThis.fetch = (async (url: RequestInfo | URL, init?: RequestInit) => {
    calls.push({ url: String(url), init });
    return new Response(
      JSON.stringify(makeActiveLatestMarkStatusResponse()),
      { status: 200, headers: { "content-type": "application/json" } },
    );
  }) as typeof fetch;

  try {
    const result = await fetchLatestMarkStatus();
    assert.equal(result.ok, true);
    assert.equal(calls.length, 1);
    assert.equal(calls[0].url, "http://127.0.0.1:8899/api/v1/market-data/latest-marks/status");
    assert.equal(calls[0].init?.method ?? "GET", "GET");
    assert.equal(result.data?.truth_state, "active");
    assert.equal(result.data?.marks.length, 2);
  } finally {
    globalThis.fetch = originalFetch;
  }
});

test("fetchLatestMarkStatus surfaces backend_unavailable without throwing", async () => {
  const originalFetch = globalThis.fetch;
  globalThis.fetch = (async () =>
    new Response(
      JSON.stringify({
        canonical_route: "/api/v1/market-data/latest-marks/status",
        truth_state: "backend_unavailable",
        provider: null,
        produced_at_utc: null,
        evidence_path: null,
        stale_or_missing_evidence: true,
        max_evidence_age_secs: 86400,
        network_call_made: null,
        db_write: null,
        md_bars_write: null,
        completed_bar_claim: null,
        provider_enabled: null,
        symbols_requested: [],
        marks: [],
        all_passed: null,
        reason_code: null,
        fail_reasons: [],
        error: "evidence directory could not be read",
      }),
      { status: 200, headers: { "content-type": "application/json" } },
    )) as typeof fetch;

  try {
    const result = await fetchLatestMarkStatus();
    assert.equal(result.ok, true);
    assert.equal(result.data?.truth_state, "backend_unavailable");
    assert.ok(!isLatestMarkStatusActive(result.data!.truth_state));
  } finally {
    globalThis.fetch = originalFetch;
  }
});

test("fetchLatestMarkStatus maps a network/transport failure to ok:false", async () => {
  const originalFetch = globalThis.fetch;
  globalThis.fetch = (async () => {
    throw new Error("network unreachable");
  }) as typeof fetch;

  try {
    const result = await fetchLatestMarkStatus();
    assert.equal(result.ok, false);
    assert.ok(result.error);
  } finally {
    globalThis.fetch = originalFetch;
  }
});

// ---------------------------------------------------------------------------
// CRYPTO-DATA-01AE-KRAKEN-SYNC-GUI-STATUS-SURFACE-01:
// Kraken OHLC ingest/sync evidence status helpers
// ---------------------------------------------------------------------------

function makeActiveKrakenOhlcStatusResponse(
  overrides: Partial<KrakenOhlcStatusResponse> = {},
): KrakenOhlcStatusResponse {
  return {
    canonical_route: "/api/v1/market-data/kraken-ohlc/status",
    truth_state: "active",
    provider: "kraken",
    latest_mode: "sync",
    latest_schema_version: "kraken-ohlc-sync-v2",
    produced_at_utc: "2026-07-05T12:00:00Z",
    evidence_path: "exports/market_data/kraken_ohlc_sync_1783300000.json",
    stale_or_missing_evidence: false,
    max_evidence_age_secs: 86400,
    network_call_made: false,
    db_write: true,
    md_bars_write: true,
    provider_id: "kraken",
    provider_source: "kraken",
    provider_symbol: "XXBTZUSD",
    ingest_mode: "provider_sync",
    sync_policy: "content_diff_skip_unchanged_update_changed_insert_missing",
    no_update_existing: false,
    symbols_requested: ["BTC/USD", "ETH/USD"],
    bars_completed: 2,
    bars_excluded_forming: 1,
    bars_considered_for_sync: 2,
    bars_missing_new: 1,
    bars_existing_candidate: 1,
    rows_changed: 1,
    rows_skipped_unchanged: 0,
    rows_changed_skipped_due_to_no_update_existing: 0,
    rows_inserted: 1,
    rows_updated: 1,
    rows_skipped_if_known: 0,
    latest_existing_end_ts_before: 1783123200,
    latest_completed_start_ts: 1783123200,
    latest_completed_end_ts: 1783209600,
    volume_semantics: "kraken_base_asset_volume_scaled_by_1e8_not_whole_coins_not_usd",
    volume_scale: 100000000,
    all_passed: true,
    reason_code: "kraken_sync_evidence_generated",
    fail_reasons: [],
    error: null,
    ...overrides,
  };
}

// isKrakenOhlcStatusActive

test("isKrakenOhlcStatusActive returns true for active", () => {
  assert.ok(isKrakenOhlcStatusActive("active"));
});

test("isKrakenOhlcStatusActive returns false for stale", () => {
  assert.ok(!isKrakenOhlcStatusActive("stale"));
});

test("isKrakenOhlcStatusActive returns false for no_evidence", () => {
  assert.ok(!isKrakenOhlcStatusActive("no_evidence"));
});

test("isKrakenOhlcStatusActive returns false for parse_error", () => {
  assert.ok(!isKrakenOhlcStatusActive("parse_error"));
});

test("isKrakenOhlcStatusActive returns false for unsafe_evidence", () => {
  assert.ok(!isKrakenOhlcStatusActive("unsafe_evidence"));
});

test("isKrakenOhlcStatusActive returns false for backend_unavailable", () => {
  assert.ok(!isKrakenOhlcStatusActive("backend_unavailable"));
});

// krakenOhlcStatusTruthLabel

test("krakenOhlcStatusTruthLabel active -> active", () => {
  assert.equal(krakenOhlcStatusTruthLabel("active"), "active");
});

test("krakenOhlcStatusTruthLabel stale -> stale", () => {
  assert.equal(krakenOhlcStatusTruthLabel("stale"), "stale");
});

test("krakenOhlcStatusTruthLabel no_evidence -> no evidence", () => {
  assert.equal(krakenOhlcStatusTruthLabel("no_evidence"), "no evidence");
});

test("krakenOhlcStatusTruthLabel parse_error -> parse error", () => {
  assert.equal(krakenOhlcStatusTruthLabel("parse_error"), "parse error");
});

test("krakenOhlcStatusTruthLabel unsafe_evidence is worded as a severe fail-closed state", () => {
  const label = krakenOhlcStatusTruthLabel("unsafe_evidence");
  assert.match(label, /unsafe/i);
  assert.notEqual(label, "unsafe_evidence"); // must not be a bare passthrough
});

test("krakenOhlcStatusTruthLabel backend_unavailable -> backend unavailable", () => {
  assert.equal(krakenOhlcStatusTruthLabel("backend_unavailable"), "backend unavailable");
});

test("krakenOhlcStatusTruthLabel unknown string passes through", () => {
  assert.equal(krakenOhlcStatusTruthLabel("some_other_state"), "some_other_state");
});

// isKrakenOhlcEvidenceUnsafe (defense-in-depth check)

test("isKrakenOhlcEvidenceUnsafe returns true when truth_state is unsafe_evidence", () => {
  const resp = makeActiveKrakenOhlcStatusResponse({ truth_state: "unsafe_evidence" });
  assert.ok(isKrakenOhlcEvidenceUnsafe(resp));
});

test("isKrakenOhlcEvidenceUnsafe returns true when provider is not kraken", () => {
  const resp = makeActiveKrakenOhlcStatusResponse({ provider: "coinlore" });
  assert.ok(isKrakenOhlcEvidenceUnsafe(resp));
});

test("isKrakenOhlcEvidenceUnsafe returns true when db_write=false but rows_inserted > 0", () => {
  const resp = makeActiveKrakenOhlcStatusResponse({ db_write: false, rows_inserted: 1 });
  assert.ok(isKrakenOhlcEvidenceUnsafe(resp));
});

test("isKrakenOhlcEvidenceUnsafe returns true when db_write=false but rows_updated > 0", () => {
  const resp = makeActiveKrakenOhlcStatusResponse({ db_write: false, rows_updated: 2 });
  assert.ok(isKrakenOhlcEvidenceUnsafe(resp));
});

test("isKrakenOhlcEvidenceUnsafe returns false for a genuinely safe active response", () => {
  const resp = makeActiveKrakenOhlcStatusResponse();
  assert.ok(!isKrakenOhlcEvidenceUnsafe(resp));
});

test("isKrakenOhlcEvidenceUnsafe returns false when db_write=false and no rows were written", () => {
  const resp = makeActiveKrakenOhlcStatusResponse({
    db_write: false,
    rows_inserted: 0,
    rows_updated: 0,
    md_bars_write: false,
  });
  assert.ok(!isKrakenOhlcEvidenceUnsafe(resp));
});

// KrakenOhlcStatusResponse shape — mirrors backend truth_state contract,
// including the 01AB-AC content-diff fields.

test("KrakenOhlcStatusResponse active sync shape has BTC/USD and ETH/USD requested symbols", () => {
  const resp = makeActiveKrakenOhlcStatusResponse();
  assert.ok(isKrakenOhlcStatusActive(resp.truth_state));
  assert.ok(!isKrakenOhlcEvidenceUnsafe(resp));
  assert.deepEqual(resp.symbols_requested, ["BTC/USD", "ETH/USD"]);
  assert.equal(resp.latest_mode, "sync");
});

test("KrakenOhlcStatusResponse content-diff fields are present and typed as numbers for sync evidence", () => {
  const resp = makeActiveKrakenOhlcStatusResponse();
  assert.equal(typeof resp.rows_changed, "number");
  assert.equal(typeof resp.rows_skipped_unchanged, "number");
  assert.equal(typeof resp.rows_changed_skipped_due_to_no_update_existing, "number");
});

test("KrakenOhlcStatusResponse ingest-mode shape has sync-only fields null, never fabricated", () => {
  const resp = makeActiveKrakenOhlcStatusResponse({
    latest_mode: "ingest",
    latest_schema_version: "kraken-ohlc-ingest-v1",
    ingest_mode: "provider_ingest",
    sync_policy: null,
    no_update_existing: null,
    bars_considered_for_sync: null,
    bars_missing_new: null,
    bars_existing_candidate: null,
    rows_changed: null,
    rows_skipped_unchanged: null,
    rows_changed_skipped_due_to_no_update_existing: null,
    rows_skipped_if_known: null,
    latest_existing_end_ts_before: null,
  });
  assert.ok(isKrakenOhlcStatusActive(resp.truth_state));
  assert.equal(resp.latest_mode, "ingest");
  assert.equal(resp.sync_policy, null);
  assert.equal(resp.rows_changed, null);
});

test("KrakenOhlcStatusResponse no_evidence shape is honest (empty symbols, path null)", () => {
  const resp: KrakenOhlcStatusResponse = {
    canonical_route: "/api/v1/market-data/kraken-ohlc/status",
    truth_state: "no_evidence",
    provider: null,
    latest_mode: null,
    latest_schema_version: null,
    produced_at_utc: null,
    evidence_path: null,
    stale_or_missing_evidence: true,
    max_evidence_age_secs: 86400,
    network_call_made: null,
    db_write: null,
    md_bars_write: null,
    provider_id: null,
    provider_source: null,
    provider_symbol: null,
    ingest_mode: null,
    sync_policy: null,
    no_update_existing: null,
    symbols_requested: [],
    bars_completed: null,
    bars_excluded_forming: null,
    bars_considered_for_sync: null,
    bars_missing_new: null,
    bars_existing_candidate: null,
    rows_changed: null,
    rows_skipped_unchanged: null,
    rows_changed_skipped_due_to_no_update_existing: null,
    rows_inserted: null,
    rows_updated: null,
    rows_skipped_if_known: null,
    latest_existing_end_ts_before: null,
    latest_completed_start_ts: null,
    latest_completed_end_ts: null,
    volume_semantics: null,
    volume_scale: null,
    all_passed: null,
    reason_code: null,
    fail_reasons: [],
    error: null,
  };
  assert.ok(!isKrakenOhlcStatusActive(resp.truth_state));
  assert.equal(resp.evidence_path, null);
  assert.equal(resp.symbols_requested.length, 0);
});

test("KrakenOhlcStatusResponse unsafe_evidence shape is never active and flagged unsafe", () => {
  const resp = makeActiveKrakenOhlcStatusResponse({
    truth_state: "unsafe_evidence",
    error: "evidence provider field is not 'kraken'",
  });
  assert.ok(!isKrakenOhlcStatusActive(resp.truth_state));
  assert.ok(isKrakenOhlcEvidenceUnsafe(resp));
});

// KRAKEN_OHLC_STATUS_WARNING_TEXT

test("KRAKEN_OHLC_STATUS_WARNING_TEXT names data-ingestion visibility, not trading enablement", () => {
  assert.match(KRAKEN_OHLC_STATUS_WARNING_TEXT, /data-ingestion visibility only/i);
  assert.match(KRAKEN_OHLC_STATUS_WARNING_TEXT, /not.*crypto trading enablement/i);
});

// fetchKrakenOhlcStatus

test("fetchKrakenOhlcStatus GETs the canonical kraken-ohlc status route", async () => {
  const calls: Array<{ url: string; init?: RequestInit }> = [];
  const originalFetch = globalThis.fetch;
  globalThis.fetch = (async (url: RequestInfo | URL, init?: RequestInit) => {
    calls.push({ url: String(url), init });
    return new Response(
      JSON.stringify(makeActiveKrakenOhlcStatusResponse()),
      { status: 200, headers: { "content-type": "application/json" } },
    );
  }) as typeof fetch;

  try {
    const result = await fetchKrakenOhlcStatus();
    assert.equal(result.ok, true);
    assert.equal(calls.length, 1);
    assert.equal(calls[0].url, "http://127.0.0.1:8899/api/v1/market-data/kraken-ohlc/status");
    assert.equal(calls[0].init?.method ?? "GET", "GET");
    assert.equal(result.data?.truth_state, "active");
    assert.equal(result.data?.rows_changed, 1);
  } finally {
    globalThis.fetch = originalFetch;
  }
});

test("fetchKrakenOhlcStatus surfaces backend_unavailable without throwing", async () => {
  const originalFetch = globalThis.fetch;
  globalThis.fetch = (async () =>
    new Response(
      JSON.stringify({
        canonical_route: "/api/v1/market-data/kraken-ohlc/status",
        truth_state: "backend_unavailable",
        provider: null,
        latest_mode: null,
        latest_schema_version: null,
        produced_at_utc: null,
        evidence_path: null,
        stale_or_missing_evidence: true,
        max_evidence_age_secs: 86400,
        network_call_made: null,
        db_write: null,
        md_bars_write: null,
        provider_id: null,
        provider_source: null,
        provider_symbol: null,
        ingest_mode: null,
        sync_policy: null,
        no_update_existing: null,
        symbols_requested: [],
        bars_completed: null,
        bars_excluded_forming: null,
        bars_considered_for_sync: null,
        bars_missing_new: null,
        bars_existing_candidate: null,
        rows_changed: null,
        rows_skipped_unchanged: null,
        rows_changed_skipped_due_to_no_update_existing: null,
        rows_inserted: null,
        rows_updated: null,
        rows_skipped_if_known: null,
        latest_existing_end_ts_before: null,
        latest_completed_start_ts: null,
        latest_completed_end_ts: null,
        volume_semantics: null,
        volume_scale: null,
        all_passed: null,
        reason_code: null,
        fail_reasons: [],
        error: "evidence directory could not be read",
      }),
      { status: 200, headers: { "content-type": "application/json" } },
    )) as typeof fetch;

  try {
    const result = await fetchKrakenOhlcStatus();
    assert.equal(result.ok, true);
    assert.equal(result.data?.truth_state, "backend_unavailable");
    assert.ok(!isKrakenOhlcStatusActive(result.data!.truth_state));
  } finally {
    globalThis.fetch = originalFetch;
  }
});

test("fetchKrakenOhlcStatus maps a network/transport failure to ok:false", async () => {
  const originalFetch = globalThis.fetch;
  globalThis.fetch = (async () => {
    throw new Error("network unreachable");
  }) as typeof fetch;

  try {
    const result = await fetchKrakenOhlcStatus();
    assert.equal(result.ok, false);
    assert.ok(result.error);
  } finally {
    globalThis.fetch = originalFetch;
  }
});

// ---------------------------------------------------------------------------
// CRYPTO-REGISTRY-04-KRAKEN-DATA-ONLY-REGISTRY-STATUS-SURFACE-01:
// Crypto registry readiness helpers
// ---------------------------------------------------------------------------

function makeActiveCryptoRegistryReadinessResponse(
  overrides: Partial<CryptoRegistryReadinessResponse> = {},
): CryptoRegistryReadinessResponse {
  return {
    canonical_route: "/api/v1/market-data/crypto-registry/readiness",
    truth_state: "active",
    data_readiness_state: "data_ready_manual_only",
    trading_readiness_state: "disabled",
    scheduler_readiness_state: "absent",
    provider: "kraken",
    provider_enabled: false,
    api_key_required: false,
    provider_asset_classes: ["crypto"],
    provider_implementation_status: "ohlcv_adapter_fixture_proven_network_opt_in_only",
    registry_path: "config/instruments/instruments_v2.crypto_local_marks.example.json",
    providers_path: "config/providers/providers.json",
    symbols: [
      {
        symbol: "BTC/USD",
        found: true,
        asset_class_ok: true,
        kraken_pair: "XBTUSD",
        kraken_result_key: "XXBTZUSD",
        alias_ok: true,
        enabled: false,
        paper_trading_enabled: false,
        live_trading_enabled: false,
        trading_flags_safe: true,
        passed: true,
      },
      {
        symbol: "ETH/USD",
        found: true,
        asset_class_ok: true,
        kraken_pair: "ETHUSD",
        kraken_result_key: "XETHZUSD",
        alias_ok: true,
        enabled: false,
        paper_trading_enabled: false,
        live_trading_enabled: false,
        trading_flags_safe: true,
        passed: true,
      },
    ],
    all_passed: true,
    reason_code: "crypto_registry_readiness_data_ready_manual_only",
    fail_reasons: [],
    safety: {
      no_scheduler: true,
      no_db_connection: true,
      no_network_call: true,
      no_trading_enabled: true,
      no_config_file_mutated: true,
    },
    ...overrides,
  };
}

// isCryptoRegistryReadinessActive

test("isCryptoRegistryReadinessActive returns true for active", () => {
  assert.ok(isCryptoRegistryReadinessActive("active"));
});

test("isCryptoRegistryReadinessActive returns false for missing_provider", () => {
  assert.ok(!isCryptoRegistryReadinessActive("missing_provider"));
});

test("isCryptoRegistryReadinessActive returns false for missing_symbol", () => {
  assert.ok(!isCryptoRegistryReadinessActive("missing_symbol"));
});

test("isCryptoRegistryReadinessActive returns false for missing_alias", () => {
  assert.ok(!isCryptoRegistryReadinessActive("missing_alias"));
});

test("isCryptoRegistryReadinessActive returns false for unsafe_trading_enabled", () => {
  assert.ok(!isCryptoRegistryReadinessActive("unsafe_trading_enabled"));
});

test("isCryptoRegistryReadinessActive returns false for unsafe_provider_enabled", () => {
  assert.ok(!isCryptoRegistryReadinessActive("unsafe_provider_enabled"));
});

test("isCryptoRegistryReadinessActive returns false for parse_error", () => {
  assert.ok(!isCryptoRegistryReadinessActive("parse_error"));
});

// cryptoRegistryReadinessTruthLabel

test("cryptoRegistryReadinessTruthLabel active -> active", () => {
  assert.equal(cryptoRegistryReadinessTruthLabel("active"), "active");
});

test("cryptoRegistryReadinessTruthLabel missing_provider -> provider not found", () => {
  assert.equal(cryptoRegistryReadinessTruthLabel("missing_provider"), "provider not found");
});

test("cryptoRegistryReadinessTruthLabel missing_symbol -> symbol not found or wrong asset class", () => {
  assert.equal(
    cryptoRegistryReadinessTruthLabel("missing_symbol"),
    "symbol not found or wrong asset class",
  );
});

test("cryptoRegistryReadinessTruthLabel missing_alias -> Kraken alias incomplete", () => {
  assert.equal(cryptoRegistryReadinessTruthLabel("missing_alias"), "Kraken alias incomplete");
});

test("cryptoRegistryReadinessTruthLabel unsafe_trading_enabled is worded as a severe fail-closed state", () => {
  const label = cryptoRegistryReadinessTruthLabel("unsafe_trading_enabled");
  assert.ok(label.includes("UNSAFE"));
});

test("cryptoRegistryReadinessTruthLabel unsafe_provider_enabled is worded as a severe fail-closed state", () => {
  const label = cryptoRegistryReadinessTruthLabel("unsafe_provider_enabled");
  assert.ok(label.includes("UNSAFE"));
});

test("cryptoRegistryReadinessTruthLabel parse_error -> parse error", () => {
  assert.equal(cryptoRegistryReadinessTruthLabel("parse_error"), "parse error");
});

test("cryptoRegistryReadinessTruthLabel passes through an unknown truth_state verbatim", () => {
  assert.equal(cryptoRegistryReadinessTruthLabel("something_new"), "something_new");
});

// isCryptoRegistryReadinessUnsafe

test("isCryptoRegistryReadinessUnsafe is false for a genuinely safe response", () => {
  assert.ok(!isCryptoRegistryReadinessUnsafe(makeActiveCryptoRegistryReadinessResponse()));
});

test("isCryptoRegistryReadinessUnsafe is true for truth_state=unsafe_trading_enabled", () => {
  const resp = makeActiveCryptoRegistryReadinessResponse({ truth_state: "unsafe_trading_enabled" });
  assert.ok(isCryptoRegistryReadinessUnsafe(resp));
});

test("isCryptoRegistryReadinessUnsafe is true for truth_state=unsafe_provider_enabled", () => {
  const resp = makeActiveCryptoRegistryReadinessResponse({ truth_state: "unsafe_provider_enabled" });
  assert.ok(isCryptoRegistryReadinessUnsafe(resp));
});

test("isCryptoRegistryReadinessUnsafe is true when provider_enabled is unexpectedly true", () => {
  const resp = makeActiveCryptoRegistryReadinessResponse({ provider_enabled: true });
  assert.ok(isCryptoRegistryReadinessUnsafe(resp));
});

test("isCryptoRegistryReadinessUnsafe is true when a symbol carries paper_trading_enabled=true", () => {
  const base = makeActiveCryptoRegistryReadinessResponse();
  const resp: CryptoRegistryReadinessResponse = {
    ...base,
    symbols: [{ ...base.symbols[0], paper_trading_enabled: true }, base.symbols[1]],
  };
  assert.ok(isCryptoRegistryReadinessUnsafe(resp));
});

test("isCryptoRegistryReadinessUnsafe is true when a symbol carries live_trading_enabled=true", () => {
  const base = makeActiveCryptoRegistryReadinessResponse();
  const resp: CryptoRegistryReadinessResponse = {
    ...base,
    symbols: [base.symbols[0], { ...base.symbols[1], live_trading_enabled: true }],
  };
  assert.ok(isCryptoRegistryReadinessUnsafe(resp));
});

// CRYPTO_REGISTRY_READINESS_WARNING_TEXT

test("CRYPTO_REGISTRY_READINESS_WARNING_TEXT states data-pipeline-visibility-only scope", () => {
  assert.ok(CRYPTO_REGISTRY_READINESS_WARNING_TEXT.includes("data-pipeline visibility only"));
  assert.ok(CRYPTO_REGISTRY_READINESS_WARNING_TEXT.includes("does not enable crypto trading"));
});

// fetchCryptoRegistryReadiness

test("fetchCryptoRegistryReadiness GETs the canonical crypto-registry readiness route", async () => {
  const calls: Array<{ url: string; init?: RequestInit }> = [];
  const originalFetch = globalThis.fetch;
  globalThis.fetch = (async (url: RequestInfo | URL, init?: RequestInit) => {
    calls.push({ url: String(url), init });
    return new Response(
      JSON.stringify(makeActiveCryptoRegistryReadinessResponse()),
      { status: 200, headers: { "content-type": "application/json" } },
    );
  }) as typeof fetch;

  try {
    const result = await fetchCryptoRegistryReadiness();
    assert.equal(result.ok, true);
    assert.equal(calls.length, 1);
    assert.equal(
      calls[0].url,
      "http://127.0.0.1:8899/api/v1/market-data/crypto-registry/readiness",
    );
    assert.equal(calls[0].init?.method ?? "GET", "GET");
    assert.equal(result.data?.truth_state, "active");
    assert.equal(result.data?.data_readiness_state, "data_ready_manual_only");
    assert.equal(result.data?.symbols.length, 2);
  } finally {
    globalThis.fetch = originalFetch;
  }
});

test("fetchCryptoRegistryReadiness surfaces missing_provider without throwing", async () => {
  const originalFetch = globalThis.fetch;
  globalThis.fetch = (async () =>
    new Response(
      JSON.stringify(
        makeActiveCryptoRegistryReadinessResponse({
          truth_state: "missing_provider",
          data_readiness_state: "blocked",
          provider_enabled: false,
          all_passed: false,
          reason_code: "provider_not_found",
          fail_reasons: ["provider 'kraken' not found in config/providers/providers.json"],
        }),
      ),
      { status: 200, headers: { "content-type": "application/json" } },
    )) as typeof fetch;

  try {
    const result = await fetchCryptoRegistryReadiness();
    assert.equal(result.ok, true);
    assert.equal(result.data?.truth_state, "missing_provider");
    assert.ok(!isCryptoRegistryReadinessActive(result.data!.truth_state));
    assert.equal(result.data?.fail_reasons.length, 1);
  } finally {
    globalThis.fetch = originalFetch;
  }
});

test("fetchCryptoRegistryReadiness maps a network/transport failure to ok:false", async () => {
  const originalFetch = globalThis.fetch;
  globalThis.fetch = (async () => {
    throw new Error("network unreachable");
  }) as typeof fetch;

  try {
    const result = await fetchCryptoRegistryReadiness();
    assert.equal(result.ok, false);
    assert.ok(result.error);
  } finally {
    globalThis.fetch = originalFetch;
  }
});

// ---------------------------------------------------------------------------
// CRYPTO-DATA-02C-KRAKEN-SCHEDULER-READINESS-STATUS-SURFACE-01:
// Kraken scheduler readiness helpers
// ---------------------------------------------------------------------------

function makeActiveKrakenSchedulerReadinessResponse(
  overrides: Partial<KrakenSchedulerReadinessResponse> = {},
): KrakenSchedulerReadinessResponse {
  return {
    canonical_route: "/api/v1/market-data/kraken-scheduler/readiness",
    truth_state: "active",
    scheduler_readiness_state: "scheduler_ready_manual_registration_blocked",
    rate_limit_policy_state: "valid",
    registry_readiness_state: "ready",
    provider_readiness_state: "ready",
    evidence_readiness_state: "not_required",
    provider: "kraken",
    provider_enabled: false,
    symbols: [
      {
        symbol: "BTC/USD",
        found: true,
        asset_class_ok: true,
        alias_ok: true,
        enabled: false,
        paper_trading_enabled: false,
        live_trading_enabled: false,
        trading_flags_safe: true,
      },
      {
        symbol: "ETH/USD",
        found: true,
        asset_class_ok: true,
        alias_ok: true,
        enabled: false,
        paper_trading_enabled: false,
        live_trading_enabled: false,
        trading_flags_safe: true,
      },
    ],
    recommended_default_cadence: "daily",
    min_seconds_between_pair_calls: 2,
    min_seconds_between_scheduled_runs: 86400,
    max_ohlc_calls_per_run: 2,
    max_total_network_calls_per_run: 2,
    concurrency: "sequential_only",
    scheduler_registration_status: "not_registered",
    daemon_job_status: "absent",
    network_call_made: false,
    db_write: false,
    trading_enabled: false,
    policy_path: "docs/specs/crypto_data_02a_kraken_scheduler_rate_limit_decision.json",
    registry_path: "config/instruments/instruments_v2.crypto_local_marks.example.json",
    providers_path: "config/providers/providers.json",
    all_passed: true,
    reason_code: "kraken_scheduler_prerequisites_satisfied_manual_registration_blocked",
    fail_reasons: [],
    warnings: [],
    safety: {
      no_scheduled_task_registered: true,
      no_daemon_job_added: true,
      no_network_call_made: true,
      no_db_connection: true,
      no_trading_enabled: true,
      no_config_file_mutated: true,
    },
    ...overrides,
  };
}

// isKrakenSchedulerReadinessActive

test("isKrakenSchedulerReadinessActive returns true for active", () => {
  assert.ok(isKrakenSchedulerReadinessActive("active"));
});

for (const truthState of [
  "policy_missing",
  "policy_invalid",
  "registry_unsafe",
  "provider_unsafe",
  "trading_flags_unsafe",
  "scheduler_already_registered",
  "evidence_unsafe",
  "parse_error",
  "backend_unavailable",
]) {
  test(`isKrakenSchedulerReadinessActive returns false for ${truthState}`, () => {
    assert.ok(!isKrakenSchedulerReadinessActive(truthState));
  });
}

// krakenSchedulerReadinessTruthLabel

test("krakenSchedulerReadinessTruthLabel active -> active", () => {
  assert.equal(krakenSchedulerReadinessTruthLabel("active"), "active");
});

for (const truthState of [
  "policy_missing",
  "policy_invalid",
  "registry_unsafe",
  "provider_unsafe",
  "trading_flags_unsafe",
  "scheduler_already_registered",
  "evidence_unsafe",
]) {
  test(`krakenSchedulerReadinessTruthLabel ${truthState} is worded as a severe fail-closed state`, () => {
    assert.ok(krakenSchedulerReadinessTruthLabel(truthState).includes("UNSAFE"));
  });
}

test("krakenSchedulerReadinessTruthLabel parse_error -> parse error", () => {
  assert.equal(krakenSchedulerReadinessTruthLabel("parse_error"), "parse error");
});

test("krakenSchedulerReadinessTruthLabel backend_unavailable -> registry/providers file unreadable", () => {
  assert.equal(
    krakenSchedulerReadinessTruthLabel("backend_unavailable"),
    "registry/providers file unreadable",
  );
});

test("krakenSchedulerReadinessTruthLabel passes through an unknown truth_state verbatim", () => {
  assert.equal(krakenSchedulerReadinessTruthLabel("something_new"), "something_new");
});

// isKrakenSchedulerReadinessUnsafe

test("isKrakenSchedulerReadinessUnsafe is false for a genuinely safe, active response", () => {
  assert.ok(!isKrakenSchedulerReadinessUnsafe(makeActiveKrakenSchedulerReadinessResponse()));
});

test("isKrakenSchedulerReadinessUnsafe is true when truth_state is not active", () => {
  const resp = makeActiveKrakenSchedulerReadinessResponse({ truth_state: "policy_invalid" });
  assert.ok(isKrakenSchedulerReadinessUnsafe(resp));
});

test("isKrakenSchedulerReadinessUnsafe is true when provider_enabled is unexpectedly true", () => {
  const resp = makeActiveKrakenSchedulerReadinessResponse({ provider_enabled: true });
  assert.ok(isKrakenSchedulerReadinessUnsafe(resp));
});

test("isKrakenSchedulerReadinessUnsafe is true when scheduler_registration_status is not not_registered", () => {
  const resp = makeActiveKrakenSchedulerReadinessResponse({
    scheduler_registration_status: "registered",
  });
  assert.ok(isKrakenSchedulerReadinessUnsafe(resp));
});

test("isKrakenSchedulerReadinessUnsafe is true when network_call_made is unexpectedly true", () => {
  const resp = makeActiveKrakenSchedulerReadinessResponse({ network_call_made: true });
  assert.ok(isKrakenSchedulerReadinessUnsafe(resp));
});

test("isKrakenSchedulerReadinessUnsafe is true when db_write is unexpectedly true", () => {
  const resp = makeActiveKrakenSchedulerReadinessResponse({ db_write: true });
  assert.ok(isKrakenSchedulerReadinessUnsafe(resp));
});

test("isKrakenSchedulerReadinessUnsafe is true when trading_enabled is unexpectedly true", () => {
  const resp = makeActiveKrakenSchedulerReadinessResponse({ trading_enabled: true });
  assert.ok(isKrakenSchedulerReadinessUnsafe(resp));
});

test("isKrakenSchedulerReadinessUnsafe is true when a symbol carries paper_trading_enabled=true", () => {
  const base = makeActiveKrakenSchedulerReadinessResponse();
  const resp: KrakenSchedulerReadinessResponse = {
    ...base,
    symbols: [{ ...base.symbols[0], paper_trading_enabled: true }, base.symbols[1]],
  };
  assert.ok(isKrakenSchedulerReadinessUnsafe(resp));
});

test("isKrakenSchedulerReadinessUnsafe is true when a symbol carries live_trading_enabled=true", () => {
  const base = makeActiveKrakenSchedulerReadinessResponse();
  const resp: KrakenSchedulerReadinessResponse = {
    ...base,
    symbols: [base.symbols[0], { ...base.symbols[1], live_trading_enabled: true }],
  };
  assert.ok(isKrakenSchedulerReadinessUnsafe(resp));
});

// KRAKEN_SCHEDULER_READINESS_WARNING_TEXT

test("KRAKEN_SCHEDULER_READINESS_WARNING_TEXT states data-pipeline-visibility-only scope", () => {
  assert.ok(KRAKEN_SCHEDULER_READINESS_WARNING_TEXT.includes("data-pipeline visibility only"));
  assert.ok(KRAKEN_SCHEDULER_READINESS_WARNING_TEXT.includes("does not register a scheduled task"));
  assert.ok(KRAKEN_SCHEDULER_READINESS_WARNING_TEXT.includes("enable crypto trading"));
});

// fetchKrakenSchedulerReadiness

test("fetchKrakenSchedulerReadiness GETs the canonical kraken-scheduler readiness route", async () => {
  const calls: Array<{ url: string; init?: RequestInit }> = [];
  const originalFetch = globalThis.fetch;
  globalThis.fetch = (async (url: RequestInfo | URL, init?: RequestInit) => {
    calls.push({ url: String(url), init });
    return new Response(
      JSON.stringify(makeActiveKrakenSchedulerReadinessResponse()),
      { status: 200, headers: { "content-type": "application/json" } },
    );
  }) as typeof fetch;

  try {
    const result = await fetchKrakenSchedulerReadiness();
    assert.equal(result.ok, true);
    assert.equal(calls.length, 1);
    assert.equal(
      calls[0].url,
      "http://127.0.0.1:8899/api/v1/market-data/kraken-scheduler/readiness",
    );
    assert.equal(calls[0].init?.method ?? "GET", "GET");
    assert.equal(result.data?.truth_state, "active");
    assert.equal(
      result.data?.scheduler_readiness_state,
      "scheduler_ready_manual_registration_blocked",
    );
    assert.equal(result.data?.scheduler_registration_status, "not_registered");
    assert.equal(result.data?.symbols.length, 2);
  } finally {
    globalThis.fetch = originalFetch;
  }
});

test("fetchKrakenSchedulerReadiness surfaces policy_missing without throwing", async () => {
  const originalFetch = globalThis.fetch;
  globalThis.fetch = (async () =>
    new Response(
      JSON.stringify(
        makeActiveKrakenSchedulerReadinessResponse({
          truth_state: "policy_missing",
          scheduler_readiness_state: "policy_blocked",
          rate_limit_policy_state: "missing",
          all_passed: false,
          reason_code: "scheduler_policy_file_missing",
          fail_reasons: ["policy file not found: docs/specs/crypto_data_02a_kraken_scheduler_rate_limit_decision.json"],
        }),
      ),
      { status: 200, headers: { "content-type": "application/json" } },
    )) as typeof fetch;

  try {
    const result = await fetchKrakenSchedulerReadiness();
    assert.equal(result.ok, true);
    assert.equal(result.data?.truth_state, "policy_missing");
    assert.ok(!isKrakenSchedulerReadinessActive(result.data!.truth_state));
    assert.equal(result.data?.fail_reasons.length, 1);
  } finally {
    globalThis.fetch = originalFetch;
  }
});

test("fetchKrakenSchedulerReadiness maps a network/transport failure to ok:false", async () => {
  const originalFetch = globalThis.fetch;
  globalThis.fetch = (async () => {
    throw new Error("network unreachable");
  }) as typeof fetch;

  try {
    const result = await fetchKrakenSchedulerReadiness();
    assert.equal(result.ok, false);
    assert.ok(result.error);
  } finally {
    globalThis.fetch = originalFetch;
  }
});

// ---------------------------------------------------------------------------
// CRYPTO-DATA-03C-KRAKEN-SCHEDULER-TASK-STATUS-SURFACE-01:
// Kraken scheduled task status helpers
// ---------------------------------------------------------------------------

function makeActiveKrakenSchedulerTaskStatusResponse(
  overrides: Partial<KrakenSchedulerTaskStatusResponse> = {},
): KrakenSchedulerTaskStatusResponse {
  return {
    canonical_route: "/api/v1/market-data/kraken-scheduler/task-status",
    truth_state: "active",
    schema_version: "kraken-ohlc-task-registration-v1",
    produced_at_utc: "2026-07-04T14:00:00.000Z",
    mode: "check_only",
    task_name: "MiniQuantDesk-KrakenOhlcSync",
    task_exists_before: false,
    task_exists_after: false,
    registered: false,
    unregistered: false,
    check_only: true,
    task_action:
      "-ExecutionPolicy Bypass -NonInteractive -WindowStyle Hidden -Command \"& Run-KrakenOhlcSync.ps1 -CheckOnly:$false\"",
    runner_path: "C:\\repo\\scripts\\windows\\Run-KrakenOhlcSync.ps1",
    policy_path: "docs/specs/crypto_data_02a_kraken_scheduler_rate_limit_decision.json",
    registry_path: "config/instruments/instruments_v2.crypto_local_marks.example.json",
    providers_path: "config/providers/providers.json",
    symbols: ["BTC/USD", "ETH/USD"],
    timeframe: "1D",
    at: "07:45",
    scheduled_task_mutation: false,
    network_call_made: false,
    db_write: false,
    md_bars_write: false,
    env_vars_embedded: [],
    env_vars_required: ["MQK_DATABASE_URL", "MQK_ALLOW_KRAKEN_SCHEDULED_SYNC"],
    all_passed: true,
    reason_code: "task_registration_preview_generated",
    fail_reasons: [],
    warnings: [],
    safety: {
      calls_runner_script_only: true,
      no_daemon_runtime_broker_provider_order_references: true,
      no_env_vars_embedded_in_task_action: true,
      no_env_local_read: true,
      task_never_started_by_this_script: true,
    },
    evidence_path: "exports/market_data/kraken_ohlc_task_registration.json",
    error: null,
    ...overrides,
  };
}

// isKrakenSchedulerTaskStatusActive

test("isKrakenSchedulerTaskStatusActive returns true for active", () => {
  assert.ok(isKrakenSchedulerTaskStatusActive("active"));
});

for (const truthState of [
  "no_evidence",
  "parse_error",
  "unsafe_evidence",
  "backend_unavailable",
]) {
  test(`isKrakenSchedulerTaskStatusActive returns false for ${truthState}`, () => {
    assert.ok(!isKrakenSchedulerTaskStatusActive(truthState));
  });
}

// krakenSchedulerTaskStatusTruthLabel

test("krakenSchedulerTaskStatusTruthLabel active -> active", () => {
  assert.equal(krakenSchedulerTaskStatusTruthLabel("active"), "active");
});

test("krakenSchedulerTaskStatusTruthLabel no_evidence -> no evidence", () => {
  assert.equal(krakenSchedulerTaskStatusTruthLabel("no_evidence"), "no evidence");
});

test("krakenSchedulerTaskStatusTruthLabel parse_error -> parse error", () => {
  assert.equal(krakenSchedulerTaskStatusTruthLabel("parse_error"), "parse error");
});

test("krakenSchedulerTaskStatusTruthLabel unsafe_evidence is worded as a severe fail-closed state", () => {
  assert.ok(krakenSchedulerTaskStatusTruthLabel("unsafe_evidence").includes("UNSAFE"));
});

test("krakenSchedulerTaskStatusTruthLabel backend_unavailable -> backend unavailable", () => {
  assert.equal(krakenSchedulerTaskStatusTruthLabel("backend_unavailable"), "backend unavailable");
});

// isKrakenSchedulerTaskEvidenceUnsafe

test("isKrakenSchedulerTaskEvidenceUnsafe is false for a genuinely safe, active response", () => {
  assert.ok(!isKrakenSchedulerTaskEvidenceUnsafe(makeActiveKrakenSchedulerTaskStatusResponse()));
});

test("isKrakenSchedulerTaskEvidenceUnsafe is true when truth_state is unsafe_evidence", () => {
  const resp = makeActiveKrakenSchedulerTaskStatusResponse({ truth_state: "unsafe_evidence" });
  assert.ok(isKrakenSchedulerTaskEvidenceUnsafe(resp));
});

test("isKrakenSchedulerTaskEvidenceUnsafe catches network_call_made=true", () => {
  const resp = makeActiveKrakenSchedulerTaskStatusResponse({ network_call_made: true });
  assert.ok(isKrakenSchedulerTaskEvidenceUnsafe(resp));
});

test("isKrakenSchedulerTaskEvidenceUnsafe catches db_write=true", () => {
  const resp = makeActiveKrakenSchedulerTaskStatusResponse({ db_write: true });
  assert.ok(isKrakenSchedulerTaskEvidenceUnsafe(resp));
});

test("isKrakenSchedulerTaskEvidenceUnsafe catches md_bars_write=true", () => {
  const resp = makeActiveKrakenSchedulerTaskStatusResponse({ md_bars_write: true });
  assert.ok(isKrakenSchedulerTaskEvidenceUnsafe(resp));
});

test("isKrakenSchedulerTaskEvidenceUnsafe catches env_vars_embedded non-empty", () => {
  const resp = makeActiveKrakenSchedulerTaskStatusResponse({
    env_vars_embedded: ["MQK_DATABASE_URL"],
  });
  assert.ok(isKrakenSchedulerTaskEvidenceUnsafe(resp));
});

test("isKrakenSchedulerTaskEvidenceUnsafe catches check_only claiming scheduled_task_mutation=true", () => {
  const resp = makeActiveKrakenSchedulerTaskStatusResponse({
    mode: "check_only",
    scheduled_task_mutation: true,
  });
  assert.ok(isKrakenSchedulerTaskEvidenceUnsafe(resp));
});

test("isKrakenSchedulerTaskEvidenceUnsafe catches check_only claiming registered=true", () => {
  const resp = makeActiveKrakenSchedulerTaskStatusResponse({
    mode: "check_only",
    registered: true,
  });
  assert.ok(isKrakenSchedulerTaskEvidenceUnsafe(resp));
});

test("isKrakenSchedulerTaskEvidenceUnsafe preserves registered=true for a genuine register-mode response", () => {
  const resp = makeActiveKrakenSchedulerTaskStatusResponse({
    mode: "register",
    check_only: false,
    registered: true,
    task_exists_after: true,
    scheduled_task_mutation: true,
  });
  assert.ok(!isKrakenSchedulerTaskEvidenceUnsafe(resp));
  assert.equal(resp.registered, true);
});

// KRAKEN_SCHEDULER_TASK_STATUS_WARNING_TEXT

test("KRAKEN_SCHEDULER_TASK_STATUS_WARNING_TEXT states evidence-visibility-only scope", () => {
  assert.ok(KRAKEN_SCHEDULER_TASK_STATUS_WARNING_TEXT.includes("evidence visibility only"));
  assert.ok(KRAKEN_SCHEDULER_TASK_STATUS_WARNING_TEXT.includes("cannot register"));
  assert.ok(KRAKEN_SCHEDULER_TASK_STATUS_WARNING_TEXT.includes("enable crypto trading"));
});

// fetchKrakenSchedulerTaskStatus

test("fetchKrakenSchedulerTaskStatus GETs the canonical task-status route", async () => {
  const calls: Array<{ url: string; init?: RequestInit }> = [];
  const originalFetch = globalThis.fetch;
  globalThis.fetch = (async (url: RequestInfo | URL, init?: RequestInit) => {
    calls.push({ url: String(url), init });
    return new Response(
      JSON.stringify(makeActiveKrakenSchedulerTaskStatusResponse()),
      { status: 200, headers: { "content-type": "application/json" } },
    );
  }) as typeof fetch;

  try {
    const result = await fetchKrakenSchedulerTaskStatus();
    assert.equal(result.ok, true);
    assert.equal(calls.length, 1);
    assert.equal(
      calls[0].url,
      "http://127.0.0.1:8899/api/v1/market-data/kraken-scheduler/task-status",
    );
    assert.equal(calls[0].init?.method ?? "GET", "GET");
    assert.equal(result.data?.truth_state, "active");
    assert.equal(result.data?.task_name, "MiniQuantDesk-KrakenOhlcSync");
    assert.equal(result.data?.registered, false);
  } finally {
    globalThis.fetch = originalFetch;
  }
});

test("fetchKrakenSchedulerTaskStatus preserves registered=true from a register-mode response", async () => {
  const originalFetch = globalThis.fetch;
  globalThis.fetch = (async () =>
    new Response(
      JSON.stringify(
        makeActiveKrakenSchedulerTaskStatusResponse({
          mode: "register",
          check_only: false,
          registered: true,
          task_exists_before: false,
          task_exists_after: true,
          scheduled_task_mutation: true,
          reason_code: "task_registered",
        }),
      ),
      { status: 200, headers: { "content-type": "application/json" } },
    )) as typeof fetch;

  try {
    const result = await fetchKrakenSchedulerTaskStatus();
    assert.equal(result.ok, true);
    assert.equal(result.data?.truth_state, "active");
    assert.equal(result.data?.registered, true);
    assert.equal(result.data?.task_exists_after, true);
  } finally {
    globalThis.fetch = originalFetch;
  }
});

test("fetchKrakenSchedulerTaskStatus surfaces no_evidence without throwing", async () => {
  const originalFetch = globalThis.fetch;
  globalThis.fetch = (async () =>
    new Response(
      JSON.stringify({
        canonical_route: "/api/v1/market-data/kraken-scheduler/task-status",
        truth_state: "no_evidence",
        schema_version: null,
        produced_at_utc: null,
        mode: null,
        task_name: null,
        task_exists_before: null,
        task_exists_after: null,
        registered: null,
        unregistered: null,
        check_only: null,
        task_action: null,
        runner_path: null,
        policy_path: null,
        registry_path: null,
        providers_path: null,
        symbols: [],
        timeframe: null,
        at: null,
        scheduled_task_mutation: null,
        network_call_made: null,
        db_write: null,
        md_bars_write: null,
        env_vars_embedded: [],
        env_vars_required: [],
        all_passed: null,
        reason_code: null,
        fail_reasons: [],
        warnings: [],
        safety: null,
        evidence_path: null,
        error: null,
      }),
      { status: 200, headers: { "content-type": "application/json" } },
    )) as typeof fetch;

  try {
    const result = await fetchKrakenSchedulerTaskStatus();
    assert.equal(result.ok, true);
    assert.equal(result.data?.truth_state, "no_evidence");
    assert.ok(!isKrakenSchedulerTaskStatusActive(result.data!.truth_state));
    assert.deepEqual(result.data?.fail_reasons, []);
    assert.deepEqual(result.data?.warnings, []);
  } finally {
    globalThis.fetch = originalFetch;
  }
});

test("fetchKrakenSchedulerTaskStatus maps a network/transport failure to ok:false", async () => {
  const originalFetch = globalThis.fetch;
  globalThis.fetch = (async () => {
    throw new Error("network unreachable");
  }) as typeof fetch;

  try {
    const result = await fetchKrakenSchedulerTaskStatus();
    assert.equal(result.ok, false);
    assert.ok(result.error);
  } finally {
    globalThis.fetch = originalFetch;
  }
});

// ---------------------------------------------------------------------------
// DAILY-DATA-READINESS-01D-GUI-01: Daily data readiness helpers
// ---------------------------------------------------------------------------

function makeDailyDataReadinessAssignment(
  overrides: Partial<DailyDataReadinessAssignmentResponse> = {},
): DailyDataReadinessAssignmentResponse {
  return {
    assignment_symbol: "AAPL",
    assignment_timeframe: "1D",
    configured_strategy_id: "strat-1",
    effective_runtime_strategy_id: "strat-1",
    effective_runtime_target_symbol: "AAPL",
    effective_runtime_timeframe_secs: 86400,
    required_history_bars: 200,
    asset_class: "equity",
    expected_provider_id: "alpaca",
    expected_provider_symbol: "AAPL",
    actual_provider_ids: ["alpaca"],
    actual_provider_symbols: ["AAPL"],
    loaded_completed_bars: 250,
    expected_latest_bar_ts: 1718000000,
    actual_latest_bar_ts: 1718000000,
    continuity_state: "ok",
    provenance_state: "ok",
    readiness_state: "ready",
    blockers: [],
    remediation: [],
    configured_grace_seconds: 60,
    effective_grace_seconds: 60,
    configured_future_skew_seconds: 5,
    effective_future_skew_seconds: 5,
    ...overrides,
  };
}

function makeDailyDataReadinessResponse(
  overrides: Partial<DailyDataReadinessResponse> = {},
): DailyDataReadinessResponse {
  return {
    canonical_route: DAILY_DATA_READINESS_ROUTE,
    schema_version: "daily-data-readiness-v1",
    evaluated_at_utc: "2026-07-15T09:30:00Z",
    binding_scope: "configuration_preview",
    assignment_source: "watchlist_v2",
    applicability: "applicable",
    start_allowed: true,
    top_level_blocker: null,
    configured_grace_seconds: 60,
    configured_future_skew_seconds: 5,
    calendar_source: "exchange_calendar",
    calendar_coverage_state: "active",
    market_date: "2026-07-15",
    session_open_utc: "2026-07-15T13:30:00Z",
    session_close_utc: "2026-07-15T20:00:00Z",
    assignments: [makeDailyDataReadinessAssignment()],
    ...overrides,
  };
}

// --- normalizeDailyDataReadinessResponse ---

test("normalizeDailyDataReadinessResponse: complete ready response normalizes correctly", () => {
  const resp = makeDailyDataReadinessResponse();
  const normalized = normalizeDailyDataReadinessResponse(resp as unknown);
  assert.deepEqual(normalized, resp);
});

test("normalizeDailyDataReadinessResponse: complete blocked response preserves all blockers", () => {
  const blocked = makeDailyDataReadinessAssignment({
    readiness_state: "blocked",
    blockers: ["insufficient_history", "market_data_missing", "duplicate_timestamp"],
    remediation: ["backfill history", "run ingest", "dedupe rows"],
  });
  const resp = makeDailyDataReadinessResponse({ start_allowed: false, assignments: [blocked] });
  const normalized = normalizeDailyDataReadinessResponse(resp as unknown);
  assert.deepEqual(normalized.assignments[0].blockers, [
    "insufficient_history",
    "market_data_missing",
    "duplicate_timestamp",
  ]);
  assert.equal(normalized.assignments[0].blockers.length, 3);
});

test("normalizeDailyDataReadinessResponse: null numeric evidence remains null", () => {
  const raw = makeDailyDataReadinessAssignment({
    loaded_completed_bars: null,
    expected_latest_bar_ts: null,
    actual_latest_bar_ts: null,
    required_history_bars: null,
    effective_runtime_timeframe_secs: null,
  });
  const resp = makeDailyDataReadinessResponse({ assignments: [raw] });
  const normalized = normalizeDailyDataReadinessResponse(resp as unknown);
  const a = normalized.assignments[0];
  assert.equal(a.loaded_completed_bars, null);
  assert.equal(a.expected_latest_bar_ts, null);
  assert.equal(a.actual_latest_bar_ts, null);
  assert.equal(a.required_history_bars, null);
  assert.equal(a.effective_runtime_timeframe_secs, null);
});

test("normalizeDailyDataReadinessResponse: missing numbers do not become zero", () => {
  const raw = { assignment_symbol: "AAPL", assignment_timeframe: "1D" }; // no numeric fields at all
  const resp = { applicability: "applicable", assignments: [raw] };
  const normalized = normalizeDailyDataReadinessResponse(resp as unknown);
  const a = normalized.assignments[0];
  assert.equal(a.loaded_completed_bars, null);
  assert.notEqual(a.loaded_completed_bars, 0);
  assert.equal(a.expected_latest_bar_ts, null);
  assert.equal(a.actual_latest_bar_ts, null);
  assert.equal(a.required_history_bars, null);
});

test("normalizeDailyDataReadinessResponse: missing start_allowed normalizes to null", () => {
  const raw = { applicability: "applicable", assignments: [] };
  const normalized = normalizeDailyDataReadinessResponse(raw as unknown);
  assert.equal(normalized.start_allowed, null);
  assert.notEqual(normalized.start_allowed, false);
});

test("normalizeDailyDataReadinessResponse: string 'true' start_allowed normalizes to null, not false or true", () => {
  const raw = { applicability: "applicable", start_allowed: "true", assignments: [] };
  const normalized = normalizeDailyDataReadinessResponse(raw as unknown);
  assert.equal(normalized.start_allowed, null);
});

test("normalizeDailyDataReadinessResponse: string 'false' start_allowed normalizes to null, not false", () => {
  const raw = { applicability: "applicable", start_allowed: "false", assignments: [] };
  const normalized = normalizeDailyDataReadinessResponse(raw as unknown);
  assert.equal(normalized.start_allowed, null);
  assert.notEqual(normalized.start_allowed, false);
});

test("normalizeDailyDataReadinessResponse: numeric 0 start_allowed normalizes to null, not false", () => {
  const raw = { applicability: "applicable", start_allowed: 0, assignments: [] };
  const normalized = normalizeDailyDataReadinessResponse(raw as unknown);
  assert.equal(normalized.start_allowed, null);
  assert.notEqual(normalized.start_allowed, false);
});

test("normalizeDailyDataReadinessResponse: explicit boolean false start_allowed remains false", () => {
  const raw = { applicability: "applicable", start_allowed: false, assignments: [] };
  const normalized = normalizeDailyDataReadinessResponse(raw as unknown);
  assert.equal(normalized.start_allowed, false);
});

test("normalizeDailyDataReadinessResponse: explicit boolean true start_allowed remains true", () => {
  const raw = { applicability: "applicable", start_allowed: true, assignments: [] };
  const normalized = normalizeDailyDataReadinessResponse(raw as unknown);
  assert.equal(normalized.start_allowed, true);
});

test("normalizeDailyDataReadinessResponse: missing applicability becomes unknown", () => {
  const raw = { start_allowed: true, assignments: [] };
  const normalized = normalizeDailyDataReadinessResponse(raw as unknown);
  assert.equal(normalized.applicability, "unknown");
});

test("normalizeDailyDataReadinessResponse: malformed arrays become empty arrays", () => {
  const raw = makeDailyDataReadinessAssignment();
  const malformed = {
    ...raw,
    actual_provider_ids: "not-an-array",
    actual_provider_symbols: 42,
    blockers: null,
    remediation: { not: "an array" },
  };
  const resp = { applicability: "applicable", assignments: [malformed] };
  const normalized = normalizeDailyDataReadinessResponse(resp as unknown);
  const a = normalized.assignments[0];
  assert.deepEqual(a.actual_provider_ids, []);
  assert.deepEqual(a.actual_provider_symbols, []);
  assert.deepEqual(a.blockers, []);
  assert.deepEqual(a.remediation, []);
});

test("normalizeDailyDataReadinessResponse: top-level assignments non-array becomes empty array", () => {
  const raw = { applicability: "applicable", assignments: "not-an-array" };
  const normalized = normalizeDailyDataReadinessResponse(raw as unknown);
  assert.deepEqual(normalized.assignments, []);
});

test("normalizeDailyDataReadinessResponse: malformed assignment entries do not throw", () => {
  const raw = {
    applicability: "applicable",
    assignments: [null, undefined, "a string entry", 42, [], {}],
  };
  assert.doesNotThrow(() => normalizeDailyDataReadinessResponse(raw as unknown));
  const normalized = normalizeDailyDataReadinessResponse(raw as unknown);
  assert.equal(normalized.assignments.length, 6);
  for (const a of normalized.assignments) {
    assert.equal(a.assignment_symbol, "unknown");
    assert.equal(a.readiness_state, "unknown");
    assert.deepEqual(a.blockers, []);
  }
});

test("normalizeDailyDataReadinessResponse: entirely malformed top-level input does not throw", () => {
  assert.doesNotThrow(() => normalizeDailyDataReadinessResponse("not an object" as unknown));
  assert.doesNotThrow(() => normalizeDailyDataReadinessResponse(null as unknown));
  assert.doesNotThrow(() => normalizeDailyDataReadinessResponse(undefined as unknown));
  const normalized = normalizeDailyDataReadinessResponse(null as unknown);
  assert.equal(normalized.applicability, "unknown");
  assert.equal(normalized.start_allowed, null);
  assert.deepEqual(normalized.assignments, []);
});

test("normalizeDailyDataReadinessResponse: unknown blocker codes are preserved", () => {
  const raw = makeDailyDataReadinessAssignment({
    blockers: ["totally_new_blocker_code_v7", "provider_timestamp_convention_unverified"],
  });
  const resp = makeDailyDataReadinessResponse({ assignments: [raw] });
  const normalized = normalizeDailyDataReadinessResponse(resp as unknown);
  assert.deepEqual(normalized.assignments[0].blockers, [
    "totally_new_blocker_code_v7",
    "provider_timestamp_convention_unverified",
  ]);
});

test("normalizeDailyDataReadinessResponse: expected and actual provider arrays remain distinct", () => {
  const raw = makeDailyDataReadinessAssignment({
    expected_provider_id: "alpaca",
    expected_provider_symbol: "AAPL",
    actual_provider_ids: ["twelvedata"],
    actual_provider_symbols: ["AAPL.US"],
  });
  const resp = makeDailyDataReadinessResponse({ assignments: [raw] });
  const normalized = normalizeDailyDataReadinessResponse(resp as unknown);
  const a = normalized.assignments[0];
  assert.equal(a.expected_provider_id, "alpaca");
  assert.deepEqual(a.actual_provider_ids, ["twelvedata"]);
  assert.notDeepEqual(a.actual_provider_ids, [a.expected_provider_id]);
  assert.equal(a.expected_provider_symbol, "AAPL");
  assert.deepEqual(a.actual_provider_symbols, ["AAPL.US"]);
});

// --- classifyDailyDataReadinessDisplay ---

test("classifyDailyDataReadinessDisplay: fully proven applicable response becomes ready", () => {
  const resp = makeDailyDataReadinessResponse({
    applicability: "applicable",
    start_allowed: true,
    assignments: [makeDailyDataReadinessAssignment({ readiness_state: "ready", blockers: [] })],
  });
  assert.equal(classifyDailyDataReadinessDisplay(resp, null), "ready");
});

test("classifyDailyDataReadinessDisplay: start_allowed=false becomes blocked", () => {
  const resp = makeDailyDataReadinessResponse({ applicability: "applicable", start_allowed: false });
  assert.equal(classifyDailyDataReadinessDisplay(resp, null), "blocked");
});

test("classifyDailyDataReadinessDisplay: not-applicable response becomes not_applicable", () => {
  const resp = makeDailyDataReadinessResponse({ applicability: "not_applicable", start_allowed: false, assignments: [] });
  assert.equal(classifyDailyDataReadinessDisplay(resp, null), "not_applicable");
});

test("classifyDailyDataReadinessDisplay: fetch failure becomes unknown", () => {
  const resp = makeDailyDataReadinessResponse();
  assert.equal(classifyDailyDataReadinessDisplay(resp, "network error"), "unknown");
});

test("classifyDailyDataReadinessDisplay: missing response becomes unknown", () => {
  assert.equal(classifyDailyDataReadinessDisplay(null, null), "unknown");
});

test("classifyDailyDataReadinessDisplay: applicable response with no assignments becomes unknown", () => {
  const resp = makeDailyDataReadinessResponse({ applicability: "applicable", start_allowed: true, assignments: [] });
  assert.equal(classifyDailyDataReadinessDisplay(resp, null), "unknown");
});

test("classifyDailyDataReadinessDisplay: start_allowed=true plus blocked assignment becomes unknown, never ready", () => {
  const resp = makeDailyDataReadinessResponse({
    applicability: "applicable",
    start_allowed: true,
    assignments: [makeDailyDataReadinessAssignment({ readiness_state: "blocked", blockers: ["interior_gap"] })],
  });
  const result = classifyDailyDataReadinessDisplay(resp, null);
  assert.equal(result, "unknown");
  assert.notEqual(result, "ready");
});

test("classifyDailyDataReadinessDisplay: unknown assignment readiness becomes unknown", () => {
  const resp = makeDailyDataReadinessResponse({
    applicability: "applicable",
    start_allowed: true,
    assignments: [makeDailyDataReadinessAssignment({ readiness_state: "unknown" })],
  });
  assert.equal(classifyDailyDataReadinessDisplay(resp, null), "unknown");
});

test("classifyDailyDataReadinessDisplay: multiple all-ready assignments can become ready", () => {
  const resp = makeDailyDataReadinessResponse({
    applicability: "applicable",
    start_allowed: true,
    assignments: [
      makeDailyDataReadinessAssignment({ assignment_symbol: "AAPL", readiness_state: "ready", blockers: [] }),
      makeDailyDataReadinessAssignment({ assignment_symbol: "MSFT", readiness_state: "ready", blockers: [] }),
    ],
  });
  assert.equal(classifyDailyDataReadinessDisplay(resp, null), "ready");
});

test("classifyDailyDataReadinessDisplay: one blocked assignment prevents ready", () => {
  const resp = makeDailyDataReadinessResponse({
    applicability: "applicable",
    start_allowed: true,
    assignments: [
      makeDailyDataReadinessAssignment({ assignment_symbol: "AAPL", readiness_state: "ready", blockers: [] }),
      makeDailyDataReadinessAssignment({ assignment_symbol: "MSFT", readiness_state: "blocked", blockers: ["gap_detected"] }),
    ],
  });
  assert.notEqual(classifyDailyDataReadinessDisplay(resp, null), "ready");
});

test("classifyDailyDataReadinessDisplay: applicable response with start_allowed=null classifies unknown, never blocked", () => {
  const resp = makeDailyDataReadinessResponse({
    applicability: "applicable",
    start_allowed: null,
  });
  const result = classifyDailyDataReadinessDisplay(resp, null);
  assert.equal(result, "unknown");
  assert.notEqual(result, "blocked");
});

test("classifyDailyDataReadinessDisplay: fully valid explicit false response remains blocked", () => {
  const resp = makeDailyDataReadinessResponse({
    applicability: "applicable",
    start_allowed: false,
    assignments: [],
  });
  assert.equal(classifyDailyDataReadinessDisplay(resp, null), "blocked");
});

test("classifyDailyDataReadinessDisplay: fully valid explicit true/all-ready response remains ready", () => {
  const resp = makeDailyDataReadinessResponse({
    applicability: "applicable",
    start_allowed: true,
    assignments: [
      makeDailyDataReadinessAssignment({ readiness_state: "ready", blockers: [] }),
    ],
  });
  assert.equal(classifyDailyDataReadinessDisplay(resp, null), "ready");
});

// --- buildDailyDataReadinessDiagnosticText ---

test("buildDailyDataReadinessDiagnosticText: includes assignment identity", () => {
  const resp = makeDailyDataReadinessResponse({
    assignments: [makeDailyDataReadinessAssignment({ assignment_symbol: "TSLA", assignment_timeframe: "5m" })],
  });
  const text = buildDailyDataReadinessDiagnosticText(resp);
  assert.ok(text.includes("TSLA"));
  assert.ok(text.includes("5m"));
});

test("buildDailyDataReadinessDiagnosticText: includes provider expected/actual identity", () => {
  const resp = makeDailyDataReadinessResponse({
    assignments: [
      makeDailyDataReadinessAssignment({
        expected_provider_id: "alpaca",
        actual_provider_ids: ["twelvedata"],
      }),
    ],
  });
  const text = buildDailyDataReadinessDiagnosticText(resp);
  assert.ok(text.includes("alpaca"));
  assert.ok(text.includes("twelvedata"));
});

test("buildDailyDataReadinessDiagnosticText: includes every blocker", () => {
  const resp = makeDailyDataReadinessResponse({
    assignments: [
      makeDailyDataReadinessAssignment({
        blockers: ["insufficient_history", "duplicate_timestamp", "gap_detected"],
      }),
    ],
  });
  const text = buildDailyDataReadinessDiagnosticText(resp);
  assert.ok(text.includes("insufficient_history"));
  assert.ok(text.includes("duplicate_timestamp"));
  assert.ok(text.includes("gap_detected"));
});

test("buildDailyDataReadinessDiagnosticText: includes remediation", () => {
  const resp = makeDailyDataReadinessResponse({
    assignments: [makeDailyDataReadinessAssignment({ remediation: ["backfill 200 bars via CSV ingest"] })],
  });
  const text = buildDailyDataReadinessDiagnosticText(resp);
  assert.ok(text.includes("backfill 200 bars via CSV ingest"));
});

test("buildDailyDataReadinessDiagnosticText: includes binding-scope warning", () => {
  const resp = makeDailyDataReadinessResponse({ binding_scope: "configuration_preview" });
  const text = buildDailyDataReadinessDiagnosticText(resp);
  assert.ok(text.toLowerCase().includes("configuration preview"));
  assert.ok(text.toLowerCase().includes("start-attempt binding"));
});

test("buildDailyDataReadinessDiagnosticText: does not output 'undefined'", () => {
  const resp = makeDailyDataReadinessResponse({
    top_level_blocker: null,
    calendar_source: null,
    market_date: null,
    session_open_utc: null,
    session_close_utc: null,
    assignments: [
      makeDailyDataReadinessAssignment({
        effective_runtime_strategy_id: null,
        effective_runtime_target_symbol: null,
        expected_provider_id: null,
        expected_provider_symbol: null,
        actual_provider_ids: [],
        actual_provider_symbols: [],
        blockers: [],
        remediation: [],
      }),
    ],
  });
  const text = buildDailyDataReadinessDiagnosticText(resp);
  assert.ok(!text.includes("undefined"));
});

test("buildDailyDataReadinessDiagnosticText: does not contain operator tokens or credential names", () => {
  const resp = makeDailyDataReadinessResponse();
  const text = buildDailyDataReadinessDiagnosticText(resp);
  const lower = text.toLowerCase();
  assert.ok(!lower.includes("mqk_operator_token"));
  assert.ok(!lower.includes("bearer "));
  assert.ok(!lower.includes("password"));
  assert.ok(!lower.includes("api_key"));
  assert.ok(!lower.includes("secret"));
});

test("buildDailyDataReadinessDiagnosticText: null start_allowed prints unknown, not false", () => {
  const resp = makeDailyDataReadinessResponse({ start_allowed: null });
  const text = buildDailyDataReadinessDiagnosticText(resp);
  assert.ok(text.includes("start_allowed: unknown"));
  assert.ok(!text.includes("start_allowed: false"));
  assert.ok(!text.includes("start_allowed: null"));
});

// --- formatReadinessNumber ---

test("formatReadinessNumber: null renders unknown", () => {
  assert.equal(formatReadinessNumber(null), "unknown");
});

test("formatReadinessNumber: undefined renders unknown", () => {
  assert.equal(formatReadinessNumber(undefined), "unknown");
});

test("formatReadinessNumber: NaN renders unknown", () => {
  assert.equal(formatReadinessNumber(NaN), "unknown");
});

test("formatReadinessNumber: Infinity renders unknown", () => {
  assert.equal(formatReadinessNumber(Infinity), "unknown");
  assert.equal(formatReadinessNumber(-Infinity), "unknown");
});

test("formatReadinessNumber: zero renders 0, not unknown", () => {
  assert.equal(formatReadinessNumber(0), "0");
});

test("formatReadinessNumber: seconds zero renders 0s", () => {
  assert.equal(formatReadinessNumber(0, "s"), "0s");
});

test("formatReadinessNumber: seconds null renders unknown, not a bare suffix", () => {
  assert.equal(formatReadinessNumber(null, "s"), "unknown");
  assert.notEqual(formatReadinessNumber(null, "s"), "s");
});

test("formatReadinessNumber: valid positive number renders with suffix", () => {
  assert.equal(formatReadinessNumber(300, "s"), "300s");
});

test("formatReadinessNumber: valid positive number renders without suffix when omitted", () => {
  assert.equal(formatReadinessNumber(250), "250");
});

// --- fetchDailyDataReadiness ---

test("fetchDailyDataReadiness GETs the canonical route and normalizes the body", async () => {
  const calls: Array<{ url: string; init?: RequestInit }> = [];
  const originalFetch = globalThis.fetch;
  const body = makeDailyDataReadinessResponse();
  globalThis.fetch = (async (url: RequestInfo | URL, init?: RequestInit) => {
    calls.push({ url: String(url), init });
    return new Response(JSON.stringify(body), {
      status: 200,
      headers: { "content-type": "application/json" },
    });
  }) as typeof fetch;

  try {
    const result = await fetchDailyDataReadiness();
    assert.equal(result.ok, true);
    assert.deepEqual(result.data, body);
    assert.equal(calls.length, 1);
    assert.equal(calls[0].url, `http://127.0.0.1:8899${DAILY_DATA_READINESS_ROUTE}`);
    assert.equal(calls[0].init?.method, "GET");
  } finally {
    globalThis.fetch = originalFetch;
  }
});

test("fetchDailyDataReadiness maps a network/transport failure to ok:false", async () => {
  const originalFetch = globalThis.fetch;
  globalThis.fetch = (async () => {
    throw new Error("network unreachable");
  }) as typeof fetch;

  try {
    const result = await fetchDailyDataReadiness();
    assert.equal(result.ok, false);
    assert.ok(result.error);
  } finally {
    globalThis.fetch = originalFetch;
  }
});
