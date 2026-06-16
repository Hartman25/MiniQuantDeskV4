import test from "node:test";
import assert from "node:assert/strict";
import {
  normalizeIngestJobStatus,
  isTerminalIngestStatus,
  extractIngestRowCounts,
  buildActiveIngestJob,
  isProviderSyncAllowed,
  buildProviderJobRequest,
  buildActiveProviderJob,
  formatEndTs,
  coverageTruthLabel,
  isCoverageActive,
  isIntradayRefreshActive,
  intradayRefreshTruthLabel,
  isTrackedEquitiesActive,
  trackedEquitiesTruthLabel,
} from "../api.ts";
import type { IngestJobStatusResponse, IntradayRefreshStatusResponse, MdBarsCoverageResponse, TrackedEquitiesResponse } from "../types.ts";

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

test("IngestJobStatusResponse with symbols_completed and symbols_failed fields is valid", () => {
  const resp = makeProviderStatusResponse("partial", {
    symbols_completed: 50,
    symbols_failed: 10,
  });
  assert.equal(resp.symbols_completed, 50);
  assert.equal(resp.symbols_failed, 10);
  assert.ok(resp.truth_state === "active");
});
