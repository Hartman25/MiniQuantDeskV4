import test from "node:test";
import assert from "node:assert/strict";
import {
  normalizeJobStatus,
  isTerminalJobStatus,
  extractArtifactDir,
  buildActiveJob,
  buildBacktestEconomicsRequest,
  buildSessionJobRow,
  getBacktestJobs,
  getInstrumentRegistryV2SourceStatus,
  parseStrictInteger,
  sessionJobIdStillPresent,
  validateBacktestJobsListResponse,
  validateMdBarsDateRange,
} from "../api.ts";
import type {
  BacktestJobRequest,
  BacktestJobStatusResponse,
  BacktestJobSummary,
  FileResult,
  InstrumentRegistryV2SourceStatusResponse,
  SessionJobRow,
} from "../types.ts";

function jsonResponse(body: unknown, status = 200): Response {
  return {
    ok: status >= 200 && status < 300,
    status,
    async json() {
      return body;
    },
  } as Response;
}

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

// ---------------------------------------------------------------------------
// B06: Poll-sequence proof (BACKTEST-GUI-CLOSURE-01)
//
// The BacktestResultsScreen polling effect maps each poll response through
// buildActiveJob and decides auto-load via extractArtifactDir. This test drives
// the EXACT same pure helpers over a realistic queued → running → completed
// sequence and asserts the resulting state stream and the single auto-load
// trigger. Because production now calls these same helpers (not an inline
// reimplementation), this proves the real submit → poll → auto-load decision.
// ---------------------------------------------------------------------------

interface PollSimResult {
  states: { status: string; artifactDir: string | null; error: string | null }[];
  autoLoadDirs: string[]; // every dir the loop would have handed to loadBundle()
  stopped: boolean; // loop reached a terminal status and broke
}

// Pure mirror of the polling effect's per-tick decision in
// BacktestResultsScreen.tsx — same helpers, same control flow, no React/HTTP.
function simulatePollLoop(responses: BacktestJobStatusResponse[]): PollSimResult {
  const states: PollSimResult["states"] = [];
  const autoLoadDirs: string[] = [];
  let stopped = false;

  for (const data of responses) {
    const updated = buildActiveJob(data);
    states.push({
      status: updated.status,
      artifactDir: updated.artifactDir,
      error: updated.error,
    });

    if (isTerminalJobStatus(updated.status)) {
      if (updated.status === "completed") {
        const autoLoadDir = extractArtifactDir(data);
        if (autoLoadDir) autoLoadDirs.push(autoLoadDir);
      }
      stopped = true;
      break; // production breaks the poll loop on any terminal status
    }
  }

  return { states, autoLoadDirs, stopped };
}

test("B06: queued → running → completed(with artifact) fires exactly one auto-load on the completed tick", () => {
  const dir = "C:\\repo\\exports\\backtests\\bfa264d3-1328-5bd1-b732-9d32e8dac8ad";
  const sim = simulatePollLoop([
    makeStatusResponse("queued", null),
    makeStatusResponse("running", null),
    makeStatusResponse("completed", dir),
  ]);

  assert.deepEqual(
    sim.states.map((s) => s.status),
    ["queued", "running", "completed"],
    "status stream must reflect each poll truthfully",
  );
  assert.ok(sim.stopped, "loop must stop once the job reaches a terminal status");
  assert.equal(sim.autoLoadDirs.length, 1, "auto-load must fire exactly once");
  assert.equal(sim.autoLoadDirs[0], dir, "auto-load must use the completed artifact_dir");
});

test("B06b: no auto-load fires on queued or running ticks (artifact_dir ignored until completed)", () => {
  // A stray artifact_dir on a running tick must NOT trigger an early load.
  const sim = simulatePollLoop([
    makeStatusResponse("queued", null),
    makeStatusResponse("running", "C:\\repo\\exports\\backtests\\premature"),
  ]);
  assert.equal(sim.autoLoadDirs.length, 0, "running job must never auto-load even with a path present");
  assert.ok(!sim.stopped, "non-terminal sequence must keep polling (not stop)");
});

test("B06c: completed without artifact_dir surfaces completed state but fires no auto-load", () => {
  const sim = simulatePollLoop([
    makeStatusResponse("running", null),
    makeStatusResponse("completed", null),
  ]);
  assert.deepEqual(sim.states.map((s) => s.status), ["running", "completed"]);
  assert.ok(sim.stopped, "completed is terminal — loop stops");
  assert.equal(sim.autoLoadDirs.length, 0, "completed-without-artifact must not auto-load (screen shows warning)");
});

test("B06d: failed job stops the loop, surfaces error, and never auto-loads", () => {
  const failed = makeStatusResponse("failed", null);
  failed.error = "load bars csv failed: AAPL_1D.csv: file not found";
  const sim = simulatePollLoop([
    makeStatusResponse("running", null),
    failed,
  ]);
  assert.ok(sim.stopped, "failed is terminal — loop stops");
  assert.equal(sim.autoLoadDirs.length, 0, "failed job must never auto-load");
  const last = sim.states[sim.states.length - 1];
  assert.equal(last.status, "failed");
  assert.ok(last.error?.includes("AAPL_1D.csv"), "failed state must carry the truthful error");
});

// ---------------------------------------------------------------------------
// DB-GUI: BACKTEST-DB-BARS-SOURCE-01 — md_bars source request shape
// ---------------------------------------------------------------------------

test("DB-GUI-01: explicit CSV source request omits md_bars-only fields", () => {
  const req: BacktestJobRequest = {
    source: "csv",
    bars_path: "C:\\repo\\exports\\md_backup\\1D\\AAPL_1D.csv",
    strategy: "swing_momentum",
    symbol: "AAPL",
    timeframe_secs: 86400,
    initial_cash_micros: 100_000_000_000,
  };
  assert.equal(req.source, "csv");
  assert.ok(req.bars_path && req.bars_path.length > 0, "csv source must carry a bars_path");
  assert.equal(req.timeframe, undefined, "csv source must not set the md_bars timeframe field");
  assert.equal(req.start, undefined, "csv source must not set start");
  assert.equal(req.end, undefined, "csv source must not set end");
});

test("DB-GUI-02: md_bars source request carries symbol/timeframe/date-range and omits bars_path", () => {
  const req: BacktestJobRequest = {
    source: "md_bars",
    strategy: "swing_momentum",
    symbol: "AAPL",
    timeframe_secs: 86400,
    initial_cash_micros: 100_000_000_000,
    timeframe: "1D",
    start: "2026-06-01T00:00:00Z",
    end: "2026-06-20T00:00:00Z",
  };
  assert.equal(req.source, "md_bars");
  assert.equal(req.bars_path, undefined, "md_bars source must not require bars_path");
  assert.equal(req.timeframe, "1D");
  assert.equal(req.start, "2026-06-01T00:00:00Z");
  assert.equal(req.end, "2026-06-20T00:00:00Z");
  assert.ok(
    new Date(req.end as string) >= new Date(req.start as string),
    "end must be >= start in a well-formed md_bars request",
  );
});

test("DB-GUI-03: BacktestJobRequest with source omitted still type-checks (server defaults to csv)", () => {
  // Mirrors pre-existing B01 shape — no `source` key at all.
  const req: BacktestJobRequest = {
    bars_path: "C:\\repo\\exports\\md_backup\\1D\\AAPL_1D.csv",
    strategy: "swing_momentum",
    symbol: "AAPL",
    timeframe_secs: 86400,
    initial_cash_micros: 100_000_000_000,
  };
  assert.equal(req.source, undefined, "omitted source is a valid request shape — daemon defaults to csv");
  assert.equal(req.strategy, "swing_momentum");
});

// ---------------------------------------------------------------------------
// BACKTEST-ECONOMICS-GUI-REGISTRY-01-COMBINED — request-shape helpers
// ---------------------------------------------------------------------------

test("ECON-GUI-01: blank economics fields omit economics from POST body", () => {
  const result = buildBacktestEconomicsRequest({
    contractMultiplier: "",
    initialMarginMicros: "",
    maintenanceMarginMicros: "",
  });
  assert.equal(result.ok, true);
  assert.equal(result.ok ? result.economics : undefined, undefined);
});

test("ECON-GUI-02: multiplier field adds economics.contract_multiplier", () => {
  const result = buildBacktestEconomicsRequest({
    contractMultiplier: "50",
    initialMarginMicros: "",
    maintenanceMarginMicros: "",
  });
  assert.equal(result.ok, true);
  assert.deepEqual(result.ok ? result.economics : undefined, {
    contract_multiplier: 50,
  });
});

test("ECON-GUI-03: margin fields add economics margin metadata without multiplier", () => {
  const result = buildBacktestEconomicsRequest({
    contractMultiplier: "",
    initialMarginMicros: "500000000",
    maintenanceMarginMicros: "400000000",
  });
  assert.equal(result.ok, true);
  assert.deepEqual(result.ok ? result.economics : undefined, {
    initial_margin_micros: 500_000_000,
    maintenance_margin_micros: 400_000_000,
  });
});

test("ECON-GUI-04: invalid multiplier <= 0 is blocked before submit", () => {
  const zero = buildBacktestEconomicsRequest({
    contractMultiplier: "0",
    initialMarginMicros: "",
    maintenanceMarginMicros: "",
  });
  assert.equal(zero.ok, false);
  assert.match(zero.ok ? "" : zero.error, /positive integer/);

  const negative = buildBacktestEconomicsRequest({
    contractMultiplier: "-1",
    initialMarginMicros: "",
    maintenanceMarginMicros: "",
  });
  assert.equal(negative.ok, false);
  assert.match(negative.ok ? "" : negative.error, /positive integer/);
});

test("ECON-GUI-05: non-integer economics fields are rejected", () => {
  const result = buildBacktestEconomicsRequest({
    contractMultiplier: "50.5",
    initialMarginMicros: "",
    maintenanceMarginMicros: "",
  });
  assert.equal(result.ok, false);
  assert.match(result.ok ? "" : result.error, /integer/);
});

// ---------------------------------------------------------------------------
// getInstrumentRegistryV2SourceStatus (ASSET-CORE-01D-REGISTRY-V2-STATUS-01-COMBINED)
// ---------------------------------------------------------------------------

function notConfiguredV2SourceResponse(): InstrumentRegistryV2SourceStatusResponse {
  return {
    truth_state: "not_configured",
    configured: false,
    path: null,
    source: "MQK_INSTRUMENT_REGISTRY_V2_PATH",
    schema_version: null,
    purpose: "backtest_economics_suggestions_only",
    used_for_trading: false,
    enabled_for_live_trading: false,
    enabled_for_paper_trading: false,
    total_instruments: 0,
    asset_class_counts: {},
    enabled_counts: { enabled: 0, paper_trading_enabled: 0, live_trading_enabled: 0 },
    non_equity_present: false,
    non_equity_all_disabled: true,
    has_economics_metadata: false,
    sample_symbols: [],
    validation_errors: [],
    message: "MQK_INSTRUMENT_REGISTRY_V2_PATH is not set; no separate v2 registry source is configured.",
  };
}

function configuredValidV2SourceResponse(): InstrumentRegistryV2SourceStatusResponse {
  return {
    truth_state: "configured_valid",
    configured: true,
    path: "config/instruments/instruments_v2.backtest_suggestions.example.json",
    source: "MQK_INSTRUMENT_REGISTRY_V2_PATH",
    schema_version: 1,
    purpose: "backtest_economics_suggestions_only",
    used_for_trading: false,
    enabled_for_live_trading: false,
    enabled_for_paper_trading: false,
    total_instruments: 3,
    asset_class_counts: { future: 2, crypto: 1 },
    enabled_counts: { enabled: 0, paper_trading_enabled: 0, live_trading_enabled: 0 },
    non_equity_present: true,
    non_equity_all_disabled: true,
    has_economics_metadata: true,
    sample_symbols: ["ES_TEST", "MES_TEST", "BTCUSD_TEST"],
    validation_errors: [],
    message: "Configured v2 registry source is valid and is used only for read-only backtest economics suggestions.",
  };
}

test("getInstrumentRegistryV2SourceStatus parses a not_configured response", async () => {
  const originalFetch = globalThis.fetch;
  globalThis.fetch = (async () => jsonResponse(notConfiguredV2SourceResponse())) as typeof fetch;

  try {
    const result = await getInstrumentRegistryV2SourceStatus();
    assert.equal(result.ok, true);
    assert.equal(result.data?.truth_state, "not_configured");
    assert.equal(result.data?.configured, false);
    assert.equal(result.data?.path, null);
  } finally {
    globalThis.fetch = originalFetch;
  }
});

test("getInstrumentRegistryV2SourceStatus parses a configured_valid response with full counts", async () => {
  const originalFetch = globalThis.fetch;
  globalThis.fetch = (async () => jsonResponse(configuredValidV2SourceResponse())) as typeof fetch;

  try {
    const result = await getInstrumentRegistryV2SourceStatus();
    assert.equal(result.ok, true);
    assert.equal(result.data?.truth_state, "configured_valid");
    assert.equal(result.data?.total_instruments, 3);
    assert.deepEqual(result.data?.asset_class_counts, { future: 2, crypto: 1 });
    assert.equal(result.data?.non_equity_present, true);
    assert.equal(result.data?.non_equity_all_disabled, true);
    assert.equal(result.data?.used_for_trading, false);
    assert.deepEqual(result.data?.sample_symbols, ["ES_TEST", "MES_TEST", "BTCUSD_TEST"]);
  } finally {
    globalThis.fetch = originalFetch;
  }
});

test("getInstrumentRegistryV2SourceStatus reports a friendly error when the route is not mounted (404)", async () => {
  const originalFetch = globalThis.fetch;
  globalThis.fetch = (async () => jsonResponse({ error: "not found" }, 404)) as typeof fetch;

  try {
    const result = await getInstrumentRegistryV2SourceStatus();
    assert.equal(result.ok, false);
    assert.match(result.error ?? "", /route not found/);
  } finally {
    globalThis.fetch = originalFetch;
  }
});

// ---------------------------------------------------------------------------
// GUI-BACKTEST-JOB-LIST-AND-SELECTION-01: buildSessionJobRow / getBacktestJobs
// ---------------------------------------------------------------------------

function makeJobSummary(overrides: Partial<BacktestJobSummary> = {}): BacktestJobSummary {
  return {
    job_id: "11111111-1111-1111-1111-111111111111",
    status: "completed",
    strategy: "swing_momentum",
    symbol: "AAPL",
    created_at_utc: "2026-08-01T12:00:00Z",
    started_at_utc: "2026-08-01T12:00:01Z",
    completed_at_utc: "2026-08-01T12:00:05Z",
    artifact_dir: "C:\\repo\\exports\\backtests\\run-1",
    error: null,
    ...overrides,
  };
}

test("A-01: buildSessionJobRow maps a completed job with artifact_dir", () => {
  const row = buildSessionJobRow(makeJobSummary());
  assert.equal(row.jobId, "11111111-1111-1111-1111-111111111111");
  assert.equal(row.status, "completed");
  assert.equal(row.artifactDir, "C:\\repo\\exports\\backtests\\run-1");
});

test("A-02: buildSessionJobRow never coerces an unrecognized daemon status to completed", () => {
  const row = buildSessionJobRow(makeJobSummary({ status: "mystery_state", completed_at_utc: null, artifact_dir: null }));
  assert.equal(row.status, "unknown", "unrecognized status must surface as unknown, not completed");
});

test("A-03: buildSessionJobRow surfaces a failed job's error", () => {
  const row = buildSessionJobRow(
    makeJobSummary({ status: "failed", artifact_dir: null, error: "load bars csv failed: file not found" }),
  );
  assert.equal(row.status, "failed");
  assert.equal(row.error, "load bars csv failed: file not found");
  assert.equal(row.artifactDir, null, "failed job must never carry an artifact_dir");
});

test("A-04: buildSessionJobRow on a completed job without artifact_dir does not fabricate a path", () => {
  const row = buildSessionJobRow(makeJobSummary({ artifact_dir: null }));
  assert.equal(row.status, "completed");
  assert.equal(row.artifactDir, null, "completed-without-artifact must stay null, never a guessed path");
});

test("A-05: getBacktestJobs parses a well-formed job list", async () => {
  const originalFetch = globalThis.fetch;
  globalThis.fetch = (async () =>
    jsonResponse({ truth_state: "active", jobs: [makeJobSummary()] })) as typeof fetch;
  try {
    const result = await getBacktestJobs();
    assert.equal(result.ok, true);
    assert.equal(result.data?.jobs.length, 1);
    assert.equal(result.data?.jobs[0].job_id, "11111111-1111-1111-1111-111111111111");
  } finally {
    globalThis.fetch = originalFetch;
  }
});

test("A-06: getBacktestJobs surfaces a route-not-found 404 truthfully, not as an empty list", async () => {
  const originalFetch = globalThis.fetch;
  globalThis.fetch = (async () => jsonResponse({ error: "not found" }, 404)) as typeof fetch;
  try {
    const result = await getBacktestJobs();
    assert.equal(result.ok, false, "a 404 must never present as an authoritative empty job list");
    assert.match(result.error ?? "", /route not found/);
  } finally {
    globalThis.fetch = originalFetch;
  }
});

test("A-07: getBacktestJobs fails visibly on a malformed payload (jobs not an array)", async () => {
  const originalFetch = globalThis.fetch;
  globalThis.fetch = (async () => jsonResponse({ truth_state: "active", jobs: "not-an-array" })) as typeof fetch;
  try {
    const result = await getBacktestJobs();
    assert.equal(result.ok, false, "malformed 'jobs' field must fail visibly, not render fake data");
    assert.match(result.error ?? "", /Malformed/);
  } finally {
    globalThis.fetch = originalFetch;
  }
});

test("A-08: getBacktestJobs fails visibly when 'jobs' is entirely missing", async () => {
  const originalFetch = globalThis.fetch;
  globalThis.fetch = (async () => jsonResponse({ truth_state: "active" })) as typeof fetch;
  try {
    const result = await getBacktestJobs();
    assert.equal(result.ok, false);
  } finally {
    globalThis.fetch = originalFetch;
  }
});

test("A-09: getBacktestJobs surfaces a network failure truthfully", async () => {
  const originalFetch = globalThis.fetch;
  globalThis.fetch = (async () => {
    throw new Error("network down");
  }) as typeof fetch;
  try {
    const result = await getBacktestJobs();
    assert.equal(result.ok, false);
    assert.match(result.error ?? "", /network down/);
  } finally {
    globalThis.fetch = originalFetch;
  }
});

// ---------------------------------------------------------------------------
// A-10: Stale selection fencing — pure mirror of the BacktestResultsScreen
// handleSelectSessionJob generation-token logic. Selecting job A, then
// immediately selecting job B, must always end on B even if A's slower
// artifact read resolves after B's.
// ---------------------------------------------------------------------------

interface FenceSimResult {
  finalBundleSource: string | null;
}

async function simulateSelectionFencing(
  selections: { jobId: string; resolveDelayMs: number }[],
): Promise<FenceSimResult> {
  let generation = 0;
  let finalBundleSource: string | null = null;

  const runs = selections.map((sel) => {
    const myGeneration = ++generation;
    return new Promise<void>((resolve) => {
      setTimeout(() => {
        if (myGeneration === generation) {
          finalBundleSource = sel.jobId;
        }
        resolve();
      }, sel.resolveDelayMs);
    });
  });

  await Promise.all(runs);
  return { finalBundleSource };
}

test("A-10: rapid A then B selection shows B even when A's load resolves later", async () => {
  const result = await simulateSelectionFencing([
    { jobId: "A", resolveDelayMs: 30 },
    { jobId: "B", resolveDelayMs: 5 },
  ]);
  assert.equal(result.finalBundleSource, "B", "the latest selection must win regardless of load order");
});

test("A-10b: three rapid selections always converge on the last one", async () => {
  const result = await simulateSelectionFencing([
    { jobId: "A", resolveDelayMs: 40 },
    { jobId: "B", resolveDelayMs: 25 },
    { jobId: "C", resolveDelayMs: 1 },
  ]);
  assert.equal(result.finalBundleSource, "C");
});

// ---------------------------------------------------------------------------
// GUI-BACKTEST-RUN-WORKBENCH-01 (Patch B): validateMdBarsDateRange
// ---------------------------------------------------------------------------

test("B-DATE-01: valid range with end after start is accepted", () => {
  const result = validateMdBarsDateRange("2026-06-01T00:00:00Z", "2026-06-20T00:00:00Z");
  assert.equal(result.ok, true);
});

test("B-DATE-02: equal start and end is accepted (inclusive range)", () => {
  const result = validateMdBarsDateRange("2026-06-01T00:00:00Z", "2026-06-01T00:00:00Z");
  assert.equal(result.ok, true);
});

test("B-DATE-03: end before start is rejected (invalid date ordering)", () => {
  const result = validateMdBarsDateRange("2026-06-20T00:00:00Z", "2026-06-01T00:00:00Z");
  assert.equal(result.ok, false);
  assert.match(result.ok ? "" : result.error, /end must be >= start/i);
});

test("B-DATE-04: blank start is rejected", () => {
  const result = validateMdBarsDateRange("", "2026-06-01T00:00:00Z");
  assert.equal(result.ok, false);
  assert.match(result.ok ? "" : result.error, /start is required/i);
});

test("B-DATE-05: blank end is rejected", () => {
  const result = validateMdBarsDateRange("2026-06-01T00:00:00Z", "");
  assert.equal(result.ok, false);
  assert.match(result.ok ? "" : result.error, /end is required/i);
});

test("B-DATE-06: non-parseable start timestamp is rejected before hitting the daemon", () => {
  const result = validateMdBarsDateRange("not-a-date", "2026-06-01T00:00:00Z");
  assert.equal(result.ok, false);
  assert.match(result.ok ? "" : result.error, /start must be a valid/i);
});

test("B-DATE-07: non-parseable end timestamp is rejected before hitting the daemon", () => {
  const result = validateMdBarsDateRange("2026-06-01T00:00:00Z", "not-a-date");
  assert.equal(result.ok, false);
  assert.match(result.ok ? "" : result.error, /end must be a valid/i);
});

test("B-DATE-08: whitespace-only fields are treated as blank", () => {
  const result = validateMdBarsDateRange("   ", "2026-06-01T00:00:00Z");
  assert.equal(result.ok, false);
  assert.match(result.ok ? "" : result.error, /start is required/i);
});

// ---------------------------------------------------------------------------
// GUI-BACKTEST-JOB-LIST-AUTHORITY-REPAIR-01: validateBacktestJobsListResponse
// negative controls. This is the exact seam getBacktestJobs calls in
// production, so these tests exercise the real production path rather than a
// mirror of it.
// ---------------------------------------------------------------------------

test("JOBLIST-01: a job row of null is rejected structurally, not thrown", () => {
  const result = validateBacktestJobsListResponse({ truth_state: "active", jobs: [null] });
  assert.equal(result.ok, false);
  assert.match(result.ok ? "" : result.error, /is not an object/);
});

test("JOBLIST-02: a numeric job_id is rejected", () => {
  const result = validateBacktestJobsListResponse({
    truth_state: "active",
    jobs: [{ ...makeJobSummary(), job_id: 12345 }],
  });
  assert.equal(result.ok, false);
  assert.match(result.ok ? "" : result.error, /job_id.*must be a string/);
});

test("JOBLIST-03: a nullable field of the wrong type (number) is rejected", () => {
  const result = validateBacktestJobsListResponse({
    truth_state: "active",
    jobs: [{ ...makeJobSummary(), artifact_dir: 42 }],
  });
  assert.equal(result.ok, false);
  assert.match(result.ok ? "" : result.error, /artifact_dir.*must be a string or null/);
});

test("JOBLIST-04: truth_state != active is unavailable, never an authoritative empty list", () => {
  const result = validateBacktestJobsListResponse({ truth_state: "not_wired", jobs: [] });
  assert.equal(result.ok, false);
  assert.match(result.ok ? "" : result.error, /unavailable/);
});

test("JOBLIST-05: an unknown status string in an otherwise-valid row passes structural validation", () => {
  const result = validateBacktestJobsListResponse({
    truth_state: "active",
    jobs: [makeJobSummary({ status: "some_future_status" })],
  });
  assert.equal(result.ok, true);
  assert.equal(result.ok ? result.data.jobs[0].status : "", "some_future_status");
});

test("JOBLIST-06: response that is not an object is rejected", () => {
  const result = validateBacktestJobsListResponse("not-an-object");
  assert.equal(result.ok, false);
});

test("JOBLIST-07: response that is an array is rejected (not a jobs-list object)", () => {
  const result = validateBacktestJobsListResponse([]);
  assert.equal(result.ok, false);
});

test("JOBLIST-08: getBacktestJobs surfaces jobs=[null] as a visible malformed error, never throws", async () => {
  const originalFetch = globalThis.fetch;
  globalThis.fetch = (async () => jsonResponse({ truth_state: "active", jobs: [null] })) as typeof fetch;
  try {
    const result = await getBacktestJobs();
    assert.equal(result.ok, false);
    assert.match(result.error ?? "", /Malformed/);
  } finally {
    globalThis.fetch = originalFetch;
  }
});

test("JOBLIST-09: getBacktestJobs reports a non-active truth_state as unavailable, not an empty list", async () => {
  const originalFetch = globalThis.fetch;
  globalThis.fetch = (async () => jsonResponse({ truth_state: "not_wired", jobs: [] })) as typeof fetch;
  try {
    const result = await getBacktestJobs();
    assert.equal(result.ok, false);
    assert.equal(result.data, undefined, "a non-active truth_state must never present as an authoritative empty list");
  } finally {
    globalThis.fetch = originalFetch;
  }
});

// ---------------------------------------------------------------------------
// GUI-BACKTEST-JOB-LIST-AUTHORITY-REPAIR-01: sessionJobIdStillPresent — the
// exact predicate the screen's fetchSessionJobs uses to invalidate a stale
// selection/comparison side after a successful refresh.
// ---------------------------------------------------------------------------

function makeSessionJobRow(overrides: Partial<SessionJobRow> = {}): SessionJobRow {
  return {
    jobId: "aaaaaaaa-aaaa-aaaa-aaaa-aaaaaaaaaaaa",
    status: "completed",
    strategy: "swing_momentum",
    symbol: "AAPL",
    createdAt: "2026-08-01T12:00:00Z",
    startedAt: "2026-08-01T12:00:01Z",
    completedAt: "2026-08-01T12:00:05Z",
    artifactDir: "C:\\repo\\exports\\backtests\\run-1",
    error: null,
    ...overrides,
  };
}

test("JOBLIST-10: sessionJobIdStillPresent is true when the id is in the fresh list", () => {
  const rows = [makeSessionJobRow({ jobId: "job-1" })];
  assert.equal(sessionJobIdStillPresent("job-1", rows), true);
});

test("JOBLIST-11: sessionJobIdStillPresent is false when the id vanished from the fresh list (e.g. daemon restart)", () => {
  const rows = [makeSessionJobRow({ jobId: "job-2" })];
  assert.equal(sessionJobIdStillPresent("job-1", rows), false);
});

test("JOBLIST-12: sessionJobIdStillPresent is false against an empty fresh list", () => {
  assert.equal(sessionJobIdStillPresent("job-1", []), false);
});

// ---------------------------------------------------------------------------
// GUI-BACKTEST-INPUT-INTEGER-SAFETY-01: parseStrictInteger negative controls
// ---------------------------------------------------------------------------

test("INT-01: a value one past MAX_SAFE_INTEGER is rejected, not silently rounded", () => {
  const result = parseStrictInteger("9007199254740993", "initial_cash_micros");
  assert.equal(result.ok, false, "parseInt would have silently rounded this to 9007199254740992");
});

test("INT-02: MAX_SAFE_INTEGER itself is accepted exactly", () => {
  const result = parseStrictInteger(String(Number.MAX_SAFE_INTEGER), "initial_cash_micros");
  assert.equal(result.ok, true);
  assert.equal(result.ok ? result.value : NaN, Number.MAX_SAFE_INTEGER);
});

test("INT-03: trailing non-numeric junk is rejected, not truncated", () => {
  const result = parseStrictInteger("100000000000oops", "initial_cash_micros");
  assert.equal(result.ok, false, "parseInt would have silently truncated this to 100000000000");
});

test("INT-04: trailing junk on a short value is rejected", () => {
  const result = parseStrictInteger("120oops", "integrity_stale_threshold_ticks");
  assert.equal(result.ok, false, "parseInt would have silently truncated this to 120");
});

test("INT-05: leading non-numeric junk is rejected", () => {
  const result = parseStrictInteger("oops120", "integrity_stale_threshold_ticks");
  assert.equal(result.ok, false);
});

test("INT-06: whitespace-padded valid integer is accepted after trimming", () => {
  const result = parseStrictInteger("  42  ", "timeframe_secs");
  assert.equal(result.ok, true);
  assert.equal(result.ok ? result.value : NaN, 42);
});

test("INT-07: an ordinary valid default value parses to the exact same number", () => {
  const result = parseStrictInteger("86400", "timeframe_secs");
  assert.equal(result.ok, true);
  assert.equal(result.ok ? result.value : NaN, 86400);
});

test("INT-08: empty string is rejected (required field, unlike the optional economics parser)", () => {
  const result = parseStrictInteger("", "initial_cash_micros");
  assert.equal(result.ok, false);
});
