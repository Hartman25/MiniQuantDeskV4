// DATA-INGEST-GUI-RUNNER-01: CSV market-data ingestion runner.
//
// Safety invariants:
// - Only calls /api/v1/ingest/* routes. No live/paper execution routes.
// - No broker adapter called. No orders created. No provider API credits consumed.
// - submit is privileged (operator token required). poll is public GET.
// - Provider/TwelveData ingestion is explicitly shown as deferred, not clickable.

import { useCallback, useEffect, useRef, useState } from "react";
import { Panel } from "../../components/common/Panel";
import { formatDateTime } from "../../lib/format";
import {
  buildActiveProviderJob,
  buildProviderJobRequest,
  cancelIngestJob,
  classifyCoverageFreshness,
  computeCoverageSummary,
  computeMissingTrackedSymbols,
  coverageTruthLabel,
  fetchIntradayRefreshStatus,
  fetchKrakenOhlcStatus,
  fetchLatestMarkStatus,
  fetchMdBarsCoverage,
  fetchTrackedEquities,
  filterCoverageRows,
  formatEndTs,
  formatUnixSecondsDateTime,
  getIngestJob,
  getMarketDataFeedSchedulerStatus,
  getMarketDataFeedStatus,
  isCoverageActive,
  isIntradayRefreshActive,
  isCancellableIngestStatus,
  isKrakenOhlcEvidenceUnsafe,
  isKrakenOhlcStatusActive,
  isLatestMarkEvidenceUnsafe,
  isLatestMarkStatusActive,
  isMarketDataFeedRealActionAllowed,
  isProviderSyncAllowed,
  isTrackedEquitiesActive,
  isTerminalIngestStatus,
  intradayRefreshTruthLabel,
  krakenOhlcStatusTruthLabel,
  KRAKEN_OHLC_STATUS_WARNING_TEXT,
  latestMarkStatusTruthLabel,
  normalizeIngestJobStatus,
  parseMarketDataFeedSymbols,
  pollMarketDataFeedOnce,
  sortCoverageRows,
  startMarketDataFeedScheduler,
  stopMarketDataFeedScheduler,
  submitIngestJob,
  trackedEquitiesTruthLabel,
  buildMarketDataFeedPollOnceRequest,
  buildMarketDataFeedSchedulerStartRequest,
} from "./api.ts";
import { buildRepoRelativePath, buildMd1DSymbolPath, MD_BACKUP_1D_SEGMENTS, MD_INGEST_SEGMENTS } from "../backtests/pathHelpers.ts";
import { getDesktopRepoRoot } from "../../desktop/bootstrap.ts";
import type { CoverageSortMode } from "./api.ts";
import type { ActiveIngestJob, ActiveProviderJob, IngestJobStatusKind, IntradayRefreshStatusResponse, KrakenOhlcStatusResponse, LatestMarkStatusResponse, MarketDataFeedPollOnceResponse, MarketDataFeedSchedulerStatusResponse, MarketDataFeedStatusResponse, MdBarsCoverageResponse, MdBarsCoverageRow, TrackedEquitiesResponse } from "./types.ts";

// ---------------------------------------------------------------------------
// Status badge
// ---------------------------------------------------------------------------

function IngestJobStatusBadge({ status }: { status: IngestJobStatusKind }) {
  const label =
    status === "queued" ? "Queued" :
    status === "running" ? "Running…" :
    status === "completed" ? "Completed" :
    status === "dry_run_completed" ? "Dry-run completed" :
    status === "partial" ? "Partial" :
    status === "refused" ? "Refused" :
    status === "cancelled" ? "Cancelled" :
    status === "failed" ? "Failed" : "Unknown";

  return (
    <span className={`bt-job-status-badge status-${status}`}>{label}</span>
  );
}

// ---------------------------------------------------------------------------
// Portable default path builders
// ---------------------------------------------------------------------------

function deriveDefaultCsvPath(): string {
  return buildMd1DSymbolPath(getDesktopRepoRoot(), "AAPL") ?? "";
}

function deriveDefaultOutDir(): string {
  return buildRepoRelativePath(getDesktopRepoRoot(), ...MD_INGEST_SEGMENTS) ?? "";
}

function shortUtc(value: string | null | undefined): string {
  return value ? value.slice(0, 19).replace("T", " ") : "—";
}

function numericValue(value: number | null | undefined): string {
  return value === null || value === undefined ? "unknown" : value.toLocaleString();
}

function latestBarFeedResultLabel(result: MarketDataFeedPollOnceResponse | null): string {
  if (!result) return "—";
  const state = result.truth_state || "unknown";
  const error = result.error ? ` (${result.error})` : "";
  return `${state}${error}`;
}

// ---------------------------------------------------------------------------
// Main screen component
// ---------------------------------------------------------------------------

// ---------------------------------------------------------------------------
// Coverage table component
// ---------------------------------------------------------------------------

function CoverageTable({
  coverage,
  rows,
  nowSecs,
}: {
  coverage: MdBarsCoverageResponse;
  rows: MdBarsCoverageRow[];
  nowSecs: number;
}) {
  if (!isCoverageActive(coverage.truth_state)) {
    return (
      <div className="unavailable-notice" style={{ color: "var(--text-muted, #888)" }}>
        <strong>truth_state:</strong> {coverageTruthLabel(coverage.truth_state)}
        {coverage.error ? ` — ${coverage.error}` : ""}
      </div>
    );
  }

  return (
    <table className="bt-table" style={{ width: "100%", tableLayout: "auto" }}>
      <thead>
        <tr>
          <th>Symbol</th>
          <th>Timeframe</th>
          <th style={{ textAlign: "right" }}>Bars</th>
          <th>From</th>
          <th>To</th>
          <th>Freshness</th>
          <th>Last Ingested</th>
        </tr>
      </thead>
      <tbody>
        {rows.map((row) => {
          const freshness = classifyCoverageFreshness(row.max_end_ts, nowSecs, row.timeframe);
          return (
            <tr key={`${row.symbol}|${row.timeframe}`}>
              <td><strong>{row.symbol}</strong></td>
              <td>{row.timeframe}</td>
              <td style={{ textAlign: "right", fontVariantNumeric: "tabular-nums" }}>
                {row.bars.toLocaleString()}
              </td>
              <td style={{ fontVariantNumeric: "tabular-nums" }}>
                {formatEndTs(row.min_end_ts)}
              </td>
              <td style={{ fontVariantNumeric: "tabular-nums" }}>
                {formatEndTs(row.max_end_ts)}
              </td>
              <td style={{
                color: freshness === "fresh"
                  ? "var(--green, #4caf50)"
                  : freshness === "stale"
                  ? "var(--red, #f44336)"
                  : "var(--text-muted, #888)",
                fontSize: "0.82rem",
              }}>
                {freshness}
              </td>
              <td style={{ color: "var(--text-muted, #888)" }}>
                {row.latest_ingested_at
                  ? row.latest_ingested_at.slice(0, 19).replace("T", " ")
                  : "—"}
              </td>
            </tr>
          );
        })}
      </tbody>
    </table>
  );
}

export function IngestScreen() {
  const [csvPath, setCsvPath] = useState(() => deriveDefaultCsvPath());
  const [timeframe, setTimeframe] = useState("1D");
  const [sourceLabel, setSourceLabel] = useState("gui-csv");
  const [outDir, setOutDir] = useState(() => deriveDefaultOutDir());

  // Refresh portable defaults once the Tauri bootstrap cache is warm.
  // Only fills empty fields — operator edits are preserved.
  useEffect(() => {
    const root = getDesktopRepoRoot();
    if (!root) return;
    setCsvPath((prev) => prev || (buildMd1DSymbolPath(root, "AAPL") ?? ""));
    setOutDir((prev) => prev || (buildRepoRelativePath(root, ...MD_INGEST_SEGMENTS) ?? ""));
  }, []);

  const [submitting, setSubmitting] = useState(false);
  const [submitError, setSubmitError] = useState<string | null>(null);
  const [activeJob, setActiveJob] = useState<ActiveIngestJob | null>(null);
  const [csvCancelConfirmed, setCsvCancelConfirmed] = useState(false);
  const [csvCancelling, setCsvCancelling] = useState(false);
  const [csvCancelError, setCsvCancelError] = useState<string | null>(null);
  const [csvCancelNotice, setCsvCancelNotice] = useState<string | null>(null);

  // Coverage state — read-only view of what's in md_bars
  const [coverageFilter, setCoverageFilter] = useState("1D");
  const [coverage, setCoverage] = useState<MdBarsCoverageResponse | null>(null);
  const [coverageLoading, setCoverageLoading] = useState(false);
  const [coverageError, setCoverageError] = useState<string | null>(null);
  const [coverageSymbolSearch, setCoverageSymbolSearch] = useState("");
  const [coverageSortMode, setCoverageSortMode] = useState<CoverageSortMode>("symbol_asc");

  // DATA-INGEST-GUI-SYNC-ALL-01: Tracked-equities registry preview state
  const [trackedEquities, setTrackedEquities] = useState<TrackedEquitiesResponse | null>(null);
  const [trackedEquitiesLoading, setTrackedEquitiesLoading] = useState(false);
  const [trackedEquitiesError, setTrackedEquitiesError] = useState<string | null>(null);

  // INTRADAY-MD-REFRESHER-GUI-01: Intraday refresh status state
  const [intradayRefresh, setIntradayRefresh] = useState<IntradayRefreshStatusResponse | null>(null);
  const [intradayRefreshLoading, setIntradayRefreshLoading] = useState(false);
  const [intradayRefreshError, setIntradayRefreshError] = useState<string | null>(null);

  // CRYPTO-DATA-01Q-R-LATEST-MARK-GUI-SURFACE-BUNDLE-01-COMBINED: Latest-mark evidence status state
  const [latestMarkStatus, setLatestMarkStatus] = useState<LatestMarkStatusResponse | null>(null);
  const [latestMarkStatusLoading, setLatestMarkStatusLoading] = useState(false);
  const [latestMarkStatusError, setLatestMarkStatusError] = useState<string | null>(null);

  // CRYPTO-DATA-01AE-KRAKEN-SYNC-GUI-STATUS-SURFACE-01: Kraken OHLC evidence status state
  const [krakenOhlcStatus, setKrakenOhlcStatus] = useState<KrakenOhlcStatusResponse | null>(null);
  const [krakenOhlcStatusLoading, setKrakenOhlcStatusLoading] = useState(false);
  const [krakenOhlcStatusError, setKrakenOhlcStatusError] = useState<string | null>(null);

  // DATA-PROVIDER-GUI-FEED-SCHEDULER-01: Latest closed-bar feed scheduler state
  const [latestFeedProviderId, setLatestFeedProviderId] = useState("alpaca");
  const [latestFeedTimeframe, setLatestFeedTimeframe] = useState("5m");
  const [latestFeedSymbolsText, setLatestFeedSymbolsText] = useState("AAPL");
  const [latestFeedPollConfirmation, setLatestFeedPollConfirmation] = useState("");
  const [latestFeedStartConfirmation, setLatestFeedStartConfirmation] = useState("");
  const [latestFeedPollImmediately, setLatestFeedPollImmediately] = useState(false);
  const [latestFeedLoading, setLatestFeedLoading] = useState(false);
  const [latestFeedActionPending, setLatestFeedActionPending] = useState(false);
  const [latestFeedStatus, setLatestFeedStatus] = useState<MarketDataFeedStatusResponse | null>(null);
  const [latestFeedSchedulerStatus, setLatestFeedSchedulerStatus] =
    useState<MarketDataFeedSchedulerStatusResponse | null>(null);
  const [latestFeedLastPoll, setLatestFeedLastPoll] = useState<MarketDataFeedPollOnceResponse | null>(null);
  const [latestFeedError, setLatestFeedError] = useState<string | null>(null);
  const [latestFeedNotice, setLatestFeedNotice] = useState<string | null>(null);
  const [latestFeedSymbolsSeeded, setLatestFeedSymbolsSeeded] = useState(false);

  // DATA-INGEST-GUI-PROVIDER-RUNNER-01: Provider sync state
  const [providerStart, setProviderStart] = useState("");
  const [providerEnd, setProviderEnd] = useState("");
  const [providerAllowApiCalls, setProviderAllowApiCalls] = useState(false);
  const [providerSyncConfirmation, setProviderSyncConfirmation] = useState("");
  const [providerApiCreditsPerMin, setProviderApiCreditsPerMin] = useState("");
  const [providerApiCreditsPerDay, setProviderApiCreditsPerDay] = useState("");
  const [providerSubmitting, setProviderSubmitting] = useState(false);
  const [providerSubmitError, setProviderSubmitError] = useState<string | null>(null);
  const [activeProviderJob, setActiveProviderJob] = useState<ActiveProviderJob | null>(null);
  const [providerCancelConfirmed, setProviderCancelConfirmed] = useState(false);
  const [providerCancelling, setProviderCancelling] = useState(false);
  const [providerCancelError, setProviderCancelError] = useState<string | null>(null);
  const [providerCancelNotice, setProviderCancelNotice] = useState<string | null>(null);

  const loadCoverage = useCallback(async () => {
    setCoverageLoading(true);
    setCoverageError(null);
    const tf = coverageFilter.trim() || undefined;
    const result = await fetchMdBarsCoverage(tf);
    setCoverageLoading(false);
    if (!result.ok) {
      setCoverageError(result.error ?? "Coverage fetch failed.");
      return;
    }
    setCoverage(result.data ?? null);
  }, [coverageFilter]);

  // Auto-load coverage on mount
  useEffect(() => {
    void loadCoverage();
  // eslint-disable-next-line react-hooks/exhaustive-deps
  }, []);

  const loadTrackedEquities = useCallback(async () => {
    setTrackedEquitiesLoading(true);
    setTrackedEquitiesError(null);
    const result = await fetchTrackedEquities();
    setTrackedEquitiesLoading(false);
    if (!result.ok) {
      setTrackedEquitiesError(result.error ?? "Tracked-equities fetch failed.");
      return;
    }
    setTrackedEquities(result.data ?? null);
  }, []);

  // Auto-load tracked equities on mount
  useEffect(() => {
    void loadTrackedEquities();
  // eslint-disable-next-line react-hooks/exhaustive-deps
  }, []);

  const loadIntradayRefresh = useCallback(async () => {
    setIntradayRefreshLoading(true);
    setIntradayRefreshError(null);
    const result = await fetchIntradayRefreshStatus();
    setIntradayRefreshLoading(false);
    if (!result.ok) {
      setIntradayRefreshError(result.error ?? "Intraday refresh status fetch failed.");
      return;
    }
    setIntradayRefresh(result.data ?? null);
  }, []);

  // Auto-load intraday refresh status on mount
  useEffect(() => {
    void loadIntradayRefresh();
  // eslint-disable-next-line react-hooks/exhaustive-deps
  }, []);

  const loadLatestMarkStatus = useCallback(async () => {
    setLatestMarkStatusLoading(true);
    setLatestMarkStatusError(null);
    const result = await fetchLatestMarkStatus();
    setLatestMarkStatusLoading(false);
    if (!result.ok) {
      setLatestMarkStatusError(result.error ?? "Latest-mark status fetch failed.");
      return;
    }
    setLatestMarkStatus(result.data ?? null);
  }, []);

  // Auto-load latest-mark evidence status on mount
  useEffect(() => {
    void loadLatestMarkStatus();
  // eslint-disable-next-line react-hooks/exhaustive-deps
  }, []);

  const loadKrakenOhlcStatus = useCallback(async () => {
    setKrakenOhlcStatusLoading(true);
    setKrakenOhlcStatusError(null);
    const result = await fetchKrakenOhlcStatus();
    setKrakenOhlcStatusLoading(false);
    if (!result.ok) {
      setKrakenOhlcStatusError(result.error ?? "Kraken OHLC status fetch failed.");
      return;
    }
    setKrakenOhlcStatus(result.data ?? null);
  }, []);

  // Auto-load Kraken OHLC evidence status on mount
  useEffect(() => {
    void loadKrakenOhlcStatus();
  // eslint-disable-next-line react-hooks/exhaustive-deps
  }, []);

  const loadLatestBarFeedStatus = useCallback(async () => {
    setLatestFeedLoading(true);
    setLatestFeedError(null);
    const [feedResult, schedulerResult] = await Promise.all([
      getMarketDataFeedStatus(),
      getMarketDataFeedSchedulerStatus(),
    ]);
    setLatestFeedLoading(false);

    if (!feedResult.ok) {
      setLatestFeedStatus(null);
      setLatestFeedError(feedResult.error ?? "Feed status fetch failed.");
    } else {
      const data = feedResult.data ?? null;
      setLatestFeedStatus(data);
      setLatestFeedLastPoll(data?.last_poll ?? null);
    }

    if (!schedulerResult.ok) {
      setLatestFeedSchedulerStatus(null);
      setLatestFeedError((prev) =>
        prev
          ? `${prev} Scheduler status fetch failed: ${schedulerResult.error ?? "unknown error"}`
          : schedulerResult.error ?? "Feed scheduler status fetch failed.",
      );
    } else {
      setLatestFeedSchedulerStatus(schedulerResult.data ?? null);
    }
  }, []);

  // Auto-load latest-bar feed status only. This never starts scheduler or polls provider.
  useEffect(() => {
    void loadLatestBarFeedStatus();
  // eslint-disable-next-line react-hooks/exhaustive-deps
  }, []);

  useEffect(() => {
    if (latestFeedSymbolsSeeded) return;
    if (!trackedEquities || !isTrackedEquitiesActive(trackedEquities.truth_state)) return;
    const intradaySymbols = trackedEquities.symbols
      .filter((entry) => entry.timeframes.includes("5m") || entry.timeframes.includes("1m"))
      .slice(0, 10)
      .map((entry) => entry.symbol);
    if (intradaySymbols.length === 0) return;
    setLatestFeedSymbolsText(intradaySymbols.join(", "));
    setLatestFeedSymbolsSeeded(true);
  }, [latestFeedSymbolsSeeded, trackedEquities]);

  const latestFeedSymbols = parseMarketDataFeedSymbols(latestFeedSymbolsText);

  const validateLatestFeedInputs = useCallback((): string | null => {
    if (!latestFeedProviderId.trim()) return "provider id is required.";
    if (!latestFeedTimeframe.trim()) return "timeframe is required.";
    if (latestFeedSymbols.length === 0) return "at least one symbol is required.";
    return null;
  }, [latestFeedProviderId, latestFeedSymbols, latestFeedTimeframe]);

  const handleLatestFeedPoll = useCallback(async (real: boolean) => {
    const validationError = validateLatestFeedInputs();
    if (validationError) {
      setLatestFeedError(validationError);
      return;
    }
    if (real && !isMarketDataFeedRealActionAllowed(true, latestFeedPollConfirmation, "POLL")) {
      setLatestFeedError("Real latest-bar poll requires typing 'POLL' in the confirmation field.");
      return;
    }

    setLatestFeedActionPending(true);
    setLatestFeedError(null);
    setLatestFeedNotice(null);
    const request = buildMarketDataFeedPollOnceRequest({
      providerId: latestFeedProviderId,
      symbols: latestFeedSymbols,
      timeframe: latestFeedTimeframe,
      dryRun: !real,
      allowProviderApiCalls: real,
    });
    const result = await pollMarketDataFeedOnce(request);
    setLatestFeedActionPending(false);

    if (!result.ok) {
      setLatestFeedLastPoll(result.data ?? null);
      setLatestFeedError(result.error ?? "Latest-bar poll failed.");
      return;
    }
    setLatestFeedLastPoll(result.data ?? null);
    setLatestFeedNotice(real ? "Real latest-bar poll completed. Review result counts." : "Dry-run latest-bar poll completed. No provider calls or DB writes.");
    await loadLatestBarFeedStatus();
  }, [
    latestFeedPollConfirmation,
    latestFeedProviderId,
    latestFeedSymbols,
    latestFeedTimeframe,
    loadLatestBarFeedStatus,
    validateLatestFeedInputs,
  ]);

  const handleLatestFeedStart = useCallback(async (real: boolean) => {
    const validationError = validateLatestFeedInputs();
    if (validationError) {
      setLatestFeedError(validationError);
      return;
    }
    if (real && !isMarketDataFeedRealActionAllowed(true, latestFeedStartConfirmation, "START")) {
      setLatestFeedError("Real scheduler start requires typing 'START' in the confirmation field.");
      return;
    }

    setLatestFeedActionPending(true);
    setLatestFeedError(null);
    setLatestFeedNotice(null);
    const request = buildMarketDataFeedSchedulerStartRequest({
      providerId: latestFeedProviderId,
      symbols: latestFeedSymbols,
      timeframe: latestFeedTimeframe,
      dryRun: !real,
      allowProviderApiCalls: real,
      pollImmediately: latestFeedPollImmediately,
    });
    const result = await startMarketDataFeedScheduler(request);
    setLatestFeedActionPending(false);

    if (!result.ok) {
      setLatestFeedSchedulerStatus(result.data ?? null);
      setLatestFeedLastPoll(result.data?.last_result ?? latestFeedLastPoll);
      setLatestFeedError(result.error ?? "Latest-bar scheduler start failed.");
      return;
    }
    setLatestFeedSchedulerStatus(result.data ?? null);
    setLatestFeedLastPoll(result.data?.last_result ?? latestFeedLastPoll);
    setLatestFeedNotice(real ? "Real latest-bar scheduler start accepted. Provider calls are enabled." : "Dry-run latest-bar scheduler start accepted. Provider calls remain disabled.");
    await loadLatestBarFeedStatus();
  }, [
    latestFeedLastPoll,
    latestFeedPollImmediately,
    latestFeedProviderId,
    latestFeedStartConfirmation,
    latestFeedSymbols,
    latestFeedTimeframe,
    loadLatestBarFeedStatus,
    validateLatestFeedInputs,
  ]);

  const handleLatestFeedStop = useCallback(async () => {
    setLatestFeedActionPending(true);
    setLatestFeedError(null);
    setLatestFeedNotice(null);
    const result = await stopMarketDataFeedScheduler();
    setLatestFeedActionPending(false);

    if (!result.ok) {
      setLatestFeedSchedulerStatus(result.data ?? null);
      setLatestFeedError(result.error ?? "Latest-bar scheduler stop failed.");
      return;
    }
    setLatestFeedSchedulerStatus(result.data ?? null);
    setLatestFeedNotice(result.data?.truth_state === "not_running" ? "Scheduler was already stopped." : "Scheduler stopped.");
    await loadLatestBarFeedStatus();
  }, [loadLatestBarFeedStatus]);

  const pollingRef = useRef<{ cancelled: boolean }>({ cancelled: false });
  const providerPollingRef = useRef<{ cancelled: boolean }>({ cancelled: false });

  // Polling: 2-second cadence, stops on terminal status or unmount.
  useEffect(() => {
    if (!activeJob) return;
    if (isTerminalIngestStatus(activeJob.status)) return;

    const token = { cancelled: false };
    pollingRef.current = token;

    async function poll() {
      while (!token.cancelled) {
        await new Promise<void>((resolve) => setTimeout(resolve, 2000));
        if (token.cancelled) break;

        const result = await getIngestJob(activeJob!.jobId);
        if (token.cancelled) break;

        if (!result.ok) {
          setActiveJob((prev) =>
            prev?.jobId === activeJob!.jobId
              ? { ...prev, status: "failed" as IngestJobStatusKind, error: result.error ?? "Poll failed." }
              : prev,
          );
          break;
        }

        const data = result.data!;
        const newStatus = normalizeIngestJobStatus(data.status);

        setActiveJob((prev) =>
          prev?.jobId === activeJob!.jobId
            ? {
                ...prev,
                status: newStatus,
                startedAt: data.started_at_utc ?? null,
                completedAt: data.completed_at_utc ?? null,
                rowsRead: data.rows_read ?? null,
                rowsInserted: data.rows_inserted ?? null,
                rowsRejected: data.rows_rejected ?? null,
                qualityReportPath: data.quality_report_path ?? null,
                error: data.error ?? null,
              }
            : prev,
        );

        if (isTerminalIngestStatus(newStatus)) break;
      }
    }

    void poll();

    return () => {
      token.cancelled = true;
    };
  // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [activeJob?.jobId]);

  const handleSubmit = useCallback(async () => {
    if (!csvPath.trim()) { setSubmitError("csv_path is required."); return; }
    if (!timeframe.trim()) { setSubmitError("timeframe is required (1D, 1m, 5m)."); return; }

    setSubmitting(true);
    setSubmitError(null);
    setActiveJob(null);
    setCsvCancelConfirmed(false);
    setCsvCancelError(null);
    setCsvCancelNotice(null);

    const result = await submitIngestJob({
      source: "csv",
      csv_path: csvPath.trim(),
      timeframe: timeframe.trim(),
      source_label: sourceLabel.trim() || null,
      out_dir: outDir.trim() || null,
    });

    setSubmitting(false);

    if (!result.ok) {
      setSubmitError(result.error ?? "Submission failed.");
      return;
    }

    const data = result.data!;

    if (!data.accepted) {
      setSubmitError(data.error ?? "Job refused by daemon.");
      return;
    }

    setActiveJob({
      jobId: data.job_id,
      source: data.source,
      timeframe: timeframe.trim(),
      csvPath: csvPath.trim(),
      createdAt: new Date().toISOString(),
      startedAt: null,
      completedAt: null,
      status: normalizeIngestJobStatus(data.status),
      rowsRead: null,
      rowsInserted: null,
      rowsRejected: null,
      qualityReportPath: null,
      error: null,
    });
  }, [csvPath, timeframe, sourceLabel, outDir]);

  const handleCsvCancel = useCallback(async () => {
    if (!activeJob || !isCancellableIngestStatus(activeJob.status)) return;
    if (!csvCancelConfirmed) {
      setCsvCancelError("Confirm cancellation before cancelling this ingest job.");
      return;
    }

    setCsvCancelling(true);
    setCsvCancelError(null);
    setCsvCancelNotice(null);

    const cancelResult = await cancelIngestJob(activeJob.jobId);
    if (!cancelResult.ok) {
      setCsvCancelling(false);
      setCsvCancelError(cancelResult.error ?? "Cancel failed.");
      return;
    }

    const returnedStatus = cancelResult.data?.status
      ? normalizeIngestJobStatus(cancelResult.data.status)
      : activeJob.status;
    setCsvCancelNotice(
      cancelResult.data?.accepted
        ? "Cancel accepted. Refreshing job status."
        : `Cancel not accepted: job is ${returnedStatus}.`,
    );

    const statusResult = await getIngestJob(activeJob.jobId);
    setCsvCancelling(false);
    setCsvCancelConfirmed(false);

    if (!statusResult.ok) {
      setCsvCancelError(statusResult.error ?? "Status fetch after cancel failed.");
      setActiveJob((prev) =>
        prev?.jobId === activeJob.jobId
          ? { ...prev, status: returnedStatus, error: cancelResult.data?.error ?? prev.error }
          : prev,
      );
      return;
    }

    setActiveJob((prev) =>
      prev?.jobId === activeJob.jobId
        ? {
            ...prev,
            status: normalizeIngestJobStatus(statusResult.data!.status),
            startedAt: statusResult.data!.started_at_utc ?? null,
            completedAt: statusResult.data!.completed_at_utc ?? null,
            rowsRead: statusResult.data!.rows_read ?? null,
            rowsInserted: statusResult.data!.rows_inserted ?? null,
            rowsRejected: statusResult.data!.rows_rejected ?? null,
            qualityReportPath: statusResult.data!.quality_report_path ?? null,
            error: statusResult.data!.error ?? null,
          }
        : prev,
    );
  }, [activeJob, csvCancelConfirmed]);

  // Provider job polling effect — 2-second cadence, stops on terminal status or unmount.
  useEffect(() => {
    if (!activeProviderJob) return;
    if (isTerminalIngestStatus(activeProviderJob.status)) return;

    const token = { cancelled: false };
    providerPollingRef.current = token;

    async function poll() {
      while (!token.cancelled) {
        await new Promise<void>((resolve) => setTimeout(resolve, 2000));
        if (token.cancelled) break;

        const result = await getIngestJob(activeProviderJob!.jobId);
        if (token.cancelled) break;

        if (!result.ok) {
          setActiveProviderJob((prev) =>
            prev?.jobId === activeProviderJob!.jobId
              ? { ...prev, status: "failed" as IngestJobStatusKind, error: result.error ?? "Poll failed." }
              : prev,
          );
          break;
        }

        const updated = buildActiveProviderJob(result.data!);

        setActiveProviderJob((prev) =>
          prev?.jobId === activeProviderJob!.jobId ? updated : prev,
        );

        if (isTerminalIngestStatus(updated.status)) {
          // Refresh coverage after a completed or partial real sync.
          if (!updated.dryRun && (updated.status === "completed" || updated.status === "partial")) {
            void loadCoverage();
          }
          break;
        }
      }
    }

    void poll();

    return () => {
      token.cancelled = true;
    };
  // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [activeProviderJob?.jobId]);

  const handleDryRunSubmit = useCallback(async () => {
    setProviderSubmitting(true);
    setProviderSubmitError(null);
    setActiveProviderJob(null);
    setProviderCancelConfirmed(false);
    setProviderCancelError(null);
    setProviderCancelNotice(null);

    const req = buildProviderJobRequest({
      dryRun: true,
      allowProviderApiCalls: false,
      start: providerStart.trim() || null,
      end: providerEnd.trim() || null,
    });

    const result = await submitIngestJob(req);
    setProviderSubmitting(false);

    if (!result.ok) {
      setProviderSubmitError(result.error ?? "Submission failed.");
      return;
    }

    const data = result.data!;
    if (!data.accepted) {
      setProviderSubmitError(data.error ?? "Job refused by daemon.");
      return;
    }

    setActiveProviderJob({
      jobId: data.job_id,
      status: normalizeIngestJobStatus(data.status),
      dryRun: true,
      allowProviderApiCalls: false,
      createdAt: new Date().toISOString(),
      startedAt: null,
      completedAt: null,
      error: data.error ?? null,
      apiCallsMade: data.api_calls_made ?? 0,
      symbolsCount: data.symbols_count ?? null,
      symbolsCompleted: null,
      symbolsFailed: null,
      rowsInserted: null,
      rowsRejected: null,
      plannedFirstSymbol: null,
      plannedLastSymbol: null,
    });
  }, [providerStart, providerEnd]);

  const handleRealSyncSubmit = useCallback(async () => {
    if (!isProviderSyncAllowed(true, providerSyncConfirmation)) {
      setProviderSubmitError("Real sync requires typing 'SYNC' in the confirmation field.");
      return;
    }

    setProviderSubmitting(true);
    setProviderSubmitError(null);
    setActiveProviderJob(null);
    setProviderCancelConfirmed(false);
    setProviderCancelError(null);
    setProviderCancelNotice(null);

    const req = buildProviderJobRequest({
      dryRun: false,
      allowProviderApiCalls: true,
      start: providerStart.trim() || null,
      end: providerEnd.trim() || null,
      apiCreditsPerMinute: providerApiCreditsPerMin ? parseInt(providerApiCreditsPerMin, 10) : null,
      apiCreditsPerDay: providerApiCreditsPerDay ? parseInt(providerApiCreditsPerDay, 10) : null,
    });

    const result = await submitIngestJob(req);
    setProviderSubmitting(false);

    if (!result.ok) {
      setProviderSubmitError(result.error ?? "Submission failed.");
      return;
    }

    const data = result.data!;
    if (!data.accepted) {
      setProviderSubmitError(data.error ?? "Job refused by daemon.");
      return;
    }

    setActiveProviderJob({
      jobId: data.job_id,
      status: normalizeIngestJobStatus(data.status),
      dryRun: false,
      allowProviderApiCalls: true,
      createdAt: new Date().toISOString(),
      startedAt: null,
      completedAt: null,
      error: data.error ?? null,
      apiCallsMade: data.api_calls_made ?? 0,
      symbolsCount: data.symbols_count ?? null,
      symbolsCompleted: null,
      symbolsFailed: null,
      rowsInserted: null,
      rowsRejected: null,
      plannedFirstSymbol: null,
      plannedLastSymbol: null,
    });

    setProviderSyncConfirmation("");
  }, [providerSyncConfirmation, providerStart, providerEnd, providerApiCreditsPerMin, providerApiCreditsPerDay]);

  const handleProviderCancel = useCallback(async () => {
    if (!activeProviderJob || !isCancellableIngestStatus(activeProviderJob.status)) return;
    if (!providerCancelConfirmed) {
      setProviderCancelError("Confirm cancellation before cancelling this provider sync job.");
      return;
    }

    setProviderCancelling(true);
    setProviderCancelError(null);
    setProviderCancelNotice(null);

    const cancelResult = await cancelIngestJob(activeProviderJob.jobId);
    if (!cancelResult.ok) {
      setProviderCancelling(false);
      setProviderCancelError(cancelResult.error ?? "Cancel failed.");
      return;
    }

    const returnedStatus = cancelResult.data?.status
      ? normalizeIngestJobStatus(cancelResult.data.status)
      : activeProviderJob.status;
    setProviderCancelNotice(
      cancelResult.data?.accepted
        ? "Cancel accepted. Refreshing job status."
        : `Cancel not accepted: job is ${returnedStatus}.`,
    );

    const statusResult = await getIngestJob(activeProviderJob.jobId);
    setProviderCancelling(false);
    setProviderCancelConfirmed(false);

    if (!statusResult.ok) {
      setProviderCancelError(statusResult.error ?? "Status fetch after cancel failed.");
      setActiveProviderJob((prev) =>
        prev?.jobId === activeProviderJob.jobId
          ? { ...prev, status: returnedStatus, error: cancelResult.data?.error ?? prev.error }
          : prev,
      );
      return;
    }

    const updated = buildActiveProviderJob(statusResult.data!);
    setActiveProviderJob((prev) =>
      prev?.jobId === activeProviderJob.jobId ? updated : prev,
    );
  }, [activeProviderJob, providerCancelConfirmed]);

  const jobIsActive = activeJob !== null && !isTerminalIngestStatus(activeJob.status);
  const providerJobIsActive = activeProviderJob !== null && !isTerminalIngestStatus(activeProviderJob.status);
  const activeJobCanCancel = activeJob !== null && isCancellableIngestStatus(activeJob.status);
  const activeProviderJobCanCancel = activeProviderJob !== null && isCancellableIngestStatus(activeProviderJob.status);

  // Coverage polish computed values — derived inline (small datasets, no useMemo needed)
  const nowSecs = Math.floor(Date.now() / 1000);
  const allCoverageRows = coverage !== null && isCoverageActive(coverage.truth_state) ? coverage.rows : [];
  const coverageFilteredRows = filterCoverageRows(allCoverageRows, coverageSymbolSearch);
  const coverageSortedRows = sortCoverageRows(coverageFilteredRows, coverageSortMode);
  const coverageSummary = computeCoverageSummary(allCoverageRows, coverageSortedRows);
  const missingTrackedSymbols = computeMissingTrackedSymbols(
    trackedEquities !== null && isTrackedEquitiesActive(trackedEquities.truth_state)
      ? trackedEquities.symbols
      : null,
    allCoverageRows,
    coverageFilter.trim() || null,
  );

  const repoRoot = getDesktopRepoRoot();
  const md1DDir = buildRepoRelativePath(repoRoot, ...MD_BACKUP_1D_SEGMENTS);
  const latestFeedSchedulerRunning = latestFeedSchedulerStatus?.running === true;
  const latestFeedStatusLimitation =
    latestFeedSchedulerStatus?.limitation ?? latestFeedStatus?.limitation ?? "process_local_only_not_persisted";
  const latestFeedDisplayResult = latestFeedSchedulerStatus?.last_result ?? latestFeedLastPoll;
  const latestFeedCanRun = !latestFeedActionPending && !latestFeedSchedulerRunning;

  return (
    <div className="screen-grid desk-screen-grid">

      {/* Safety notice — always visible */}
      <div
        className="unavailable-notice"
        style={{ margin: "0 0 4px", borderColor: "var(--accent, #c8a84b)", color: "var(--text)" }}
      >
        <strong>CSV ingestion writes market data to md_bars. It does not submit broker orders.</strong>
        {" "}No live or paper execution routes are called. No broker adapter is invoked.
      </div>

      {/* Submit form */}
      <Panel
        title="Submit CSV ingest job"
        subtitle="Ingest a local CSV bars file into md_bars. Source must be 'csv'. Submission requires the operator token."
      >
        <div className="bt-job-form-grid">
          <div className="bt-job-field" style={{ gridColumn: "1 / -1" }}>
            <label htmlFor="ingest-csv-path">CSV path</label>
            <input
              id="ingest-csv-path"
              type="text"
              value={csvPath}
              onChange={(e) => setCsvPath(e.target.value)}
              spellCheck={false}
              autoComplete="off"
              placeholder={
                repoRoot
                  ? "Absolute path to CSV bars file (portable default pre-filled)"
                  : "Absolute path to CSV bars file — repo root not detected, set manually"
              }
            />
            <div className="bt-field-hint" style={{ marginTop: 4, fontSize: "0.79rem", color: "var(--text-muted, #888)" }}>
              {md1DDir
                ? <>
                    <strong>1D backup directory:</strong>{" "}
                    <code>{md1DDir}</code>{" "}
                    — files named <code>{"<SYMBOL>"}_1D.csv</code> (e.g. <code>AAPL_1D.csv</code>).
                  </>
                : <>
                    Real 1D market-data backup lives in <code>exports\md_backup\1D\{"<SYMBOL>"}_1D.csv</code>.
                    Launch via <code>Launch-VeritasLedger.ps1</code> to auto-fill the portable default.
                  </>
              }
            </div>
          </div>

          <div className="bt-job-field">
            <label htmlFor="ingest-timeframe">Timeframe</label>
            <input
              id="ingest-timeframe"
              type="text"
              value={timeframe}
              onChange={(e) => setTimeframe(e.target.value)}
              spellCheck={false}
              autoComplete="off"
              placeholder="1D | 1m | 5m"
            />
            <div className="bt-field-hint" style={{ marginTop: 4, fontSize: "0.79rem", color: "var(--text-muted, #888)" }}>
              Accepted: <code>1D</code>, <code>1m</code>, <code>5m</code>.
            </div>
          </div>

          <div className="bt-job-field">
            <label htmlFor="ingest-source-label">Source label</label>
            <input
              id="ingest-source-label"
              type="text"
              value={sourceLabel}
              onChange={(e) => setSourceLabel(e.target.value)}
              spellCheck={false}
              autoComplete="off"
              placeholder="e.g. gui-csv"
            />
            <div className="bt-field-hint" style={{ marginTop: 4, fontSize: "0.79rem", color: "var(--text-muted, #888)" }}>
              Stored in the quality report. Defaults to <code>csv</code> if blank.
            </div>
          </div>

          <div className="bt-job-field" style={{ gridColumn: "1 / -1" }}>
            <label htmlFor="ingest-out-dir">Output directory</label>
            <input
              id="ingest-out-dir"
              type="text"
              value={outDir}
              onChange={(e) => setOutDir(e.target.value)}
              spellCheck={false}
              autoComplete="off"
              placeholder={
                repoRoot
                  ? "Output directory (portable default pre-filled)"
                  : "exports\\md_ingest — repo root not detected, set manually"
              }
            />
            <div className="bt-field-hint" style={{ marginTop: 4, fontSize: "0.79rem", color: "var(--text-muted, #888)" }}>
              Quality report (<code>data_quality.json</code>) is written here.
              Defaults to <code>exports/md_ingest</code> relative to daemon working directory if blank.
            </div>
          </div>
        </div>

        <div className="bt-path-row" style={{ marginTop: 8 }}>
          <button
            type="button"
            className="action-button"
            onClick={() => void handleSubmit()}
            disabled={submitting || jobIsActive}
          >
            {submitting ? "Submitting…" : jobIsActive ? "Job running…" : "Submit ingest job"}
          </button>
        </div>

        <div className="bt-field-hint" style={{ marginTop: 8, fontSize: "0.79rem", color: "var(--text-muted, #888)" }}>
          Submission requires the daemon to be running with <code>MQK_OPERATOR_TOKEN</code> configured
          (launch via <code>Launch-VeritasLedger.ps1</code>).
        </div>

        {submitError && (
          <div className="unavailable-notice unavailable-critical" style={{ marginTop: 10 }}>
            <strong>Submit failed:</strong> {submitError}
          </div>
        )}
      </Panel>

      {/* Active job status */}
      {activeJob && (
        <Panel
          title="Job status"
          subtitle="CSV ingest job — writes to md_bars only. No broker adapter. No broker orders."
        >
          <div className="bt-job-status-row">
            <IngestJobStatusBadge status={activeJob.status} />
            <span className="bt-job-meta" title={activeJob.jobId}>
              job {activeJob.jobId.slice(0, 8)}…
            </span>
            <span className="bt-job-meta">
              {activeJob.source} / {activeJob.timeframe}
            </span>
            {activeJob.completedAt && (
              <span className="bt-job-meta">
                completed {formatDateTime(activeJob.completedAt)}
              </span>
            )}
            {!activeJob.completedAt && activeJob.startedAt && (
              <span className="bt-job-meta">
                started {formatDateTime(activeJob.startedAt)}
              </span>
            )}
          </div>

          {(activeJob.status === "queued" || activeJob.status === "running") && (
            <div className="bt-job-meta" style={{ marginTop: 6, color: "var(--accent)" }}>
              Polling for status every 2 seconds…
            </div>
          )}

          {activeJobCanCancel && (
            <div className="unavailable-notice" style={{ marginTop: 8 }}>
              <label style={{ display: "flex", alignItems: "flex-start", gap: 8, cursor: "pointer", fontSize: "0.85rem", marginBottom: 8 }}>
                <input
                  type="checkbox"
                  checked={csvCancelConfirmed}
                  onChange={(e) => {
                    setCsvCancelConfirmed(e.target.checked);
                    if (e.target.checked) setCsvCancelError(null);
                  }}
                  style={{ marginTop: 2 }}
                />
                I understand this cancels only the active CSV ingest job shown here
              </label>
              <button
                type="button"
                className="action-button ghost"
                onClick={() => void handleCsvCancel()}
                disabled={!csvCancelConfirmed || csvCancelling}
                style={{ padding: "2px 12px" }}
              >
                {csvCancelling ? "Cancelling…" : "Cancel job"}
              </button>
            </div>
          )}

          {csvCancelError && (
            <div className="unavailable-notice unavailable-critical" style={{ marginTop: 8 }}>
              <strong>Cancel failed:</strong> {csvCancelError}
            </div>
          )}

          {csvCancelNotice && (
            <div className="unavailable-notice" style={{ marginTop: 8 }}>
              {csvCancelNotice}
            </div>
          )}

          {(activeJob.rowsRead !== null || activeJob.rowsInserted !== null || activeJob.rowsRejected !== null) && (
            <div className="timeline-meta-grid" style={{ marginTop: 8 }}>
              <div>
                <span>Rows read</span>
                <strong>{activeJob.rowsRead ?? "—"}</strong>
              </div>
              <div>
                <span>Rows inserted</span>
                <strong>{activeJob.rowsInserted ?? "—"}</strong>
              </div>
              <div>
                <span>Rows rejected</span>
                <strong>{activeJob.rowsRejected ?? "—"}</strong>
              </div>
            </div>
          )}

          {activeJob.status === "completed" && activeJob.qualityReportPath && (
            <div className="bt-job-meta" style={{ marginTop: 6, wordBreak: "break-all" }}>
              <span className="eyebrow">quality report</span>{" "}
              <span style={{ color: "var(--text)" }}>{activeJob.qualityReportPath}</span>
            </div>
          )}

          {activeJob.status === "completed" && !activeJob.qualityReportPath && (
            <div className="unavailable-notice" style={{ marginTop: 4 }}>
              <strong>Completed without quality report path.</strong> Rows may have been inserted — check db.
            </div>
          )}

          {activeJob.status === "failed" && activeJob.error && (
            <div className="unavailable-notice unavailable-critical" style={{ marginTop: 4 }}>
              <strong>Job failed:</strong> {activeJob.error}
            </div>
          )}

          {activeJob.status === "cancelled" && (
            <div className="unavailable-notice" style={{ marginTop: 8, borderColor: "var(--accent, #c8a84b)" }}>
              <strong>Job cancelled.</strong>{" "}
              {activeJob.error ?? "Daemon marked this ingest job cancelled."}
            </div>
          )}

          {activeJob.status === "refused" && (
            <div className="unavailable-notice unavailable-critical" style={{ marginTop: 8 }}>
              <strong>Job refused:</strong>{" "}
              {activeJob.error ?? "Daemon refused this ingest job."}
            </div>
          )}

          {activeJob.status === "unknown" && (
            <div className="unavailable-notice" style={{ marginTop: 8 }}>
              <strong>Unrecognized status.</strong>{" "}
              Daemon returned an unknown status string — check daemon version.
            </div>
          )}
        </Panel>
      )}

      {/* Local coverage — read-only view of what's in md_bars */}
      <Panel
        title="Local data coverage"
        subtitle="Read-only view of what md_bars data exists locally. No DB writes. No provider calls."
      >
        {/* Timeframe filter + refresh */}
        <div style={{ display: "flex", alignItems: "center", gap: 8, marginBottom: 8, flexWrap: "wrap" }}>
          <label htmlFor="coverage-timeframe" style={{ fontSize: "0.85rem" }}>
            Timeframe
          </label>
          <input
            id="coverage-timeframe"
            type="text"
            value={coverageFilter}
            onChange={(e) => setCoverageFilter(e.target.value)}
            spellCheck={false}
            autoComplete="off"
            placeholder="1D | 1m | 5m | (blank = all)"
            style={{ width: 140 }}
          />
          <button
            type="button"
            className="action-button"
            onClick={() => void loadCoverage()}
            disabled={coverageLoading}
            style={{ padding: "2px 12px" }}
          >
            {coverageLoading ? "Loading…" : "Refresh"}
          </button>
        </div>

        {/* Symbol search + sort */}
        <div style={{ display: "flex", alignItems: "center", gap: 8, marginBottom: 10, flexWrap: "wrap" }}>
          <label htmlFor="coverage-search" style={{ fontSize: "0.85rem" }}>
            Symbol search
          </label>
          <input
            id="coverage-search"
            type="text"
            value={coverageSymbolSearch}
            onChange={(e) => setCoverageSymbolSearch(e.target.value)}
            spellCheck={false}
            autoComplete="off"
            placeholder="filter symbols…"
            style={{ width: 160 }}
          />
          <label htmlFor="coverage-sort" style={{ fontSize: "0.85rem" }}>
            Sort
          </label>
          <select
            id="coverage-sort"
            value={coverageSortMode}
            onChange={(e) => setCoverageSortMode(e.target.value as CoverageSortMode)}
            style={{ fontSize: "0.85rem" }}
          >
            <option value="symbol_asc">Symbol A→Z</option>
            <option value="symbol_desc">Symbol Z→A</option>
            <option value="bars_desc">Bars high→low</option>
            <option value="bars_asc">Bars low→high</option>
            <option value="latest_desc">Latest bar newest</option>
            <option value="latest_asc">Latest bar oldest</option>
          </select>
        </div>

        {coverageError && (
          <div className="unavailable-notice unavailable-critical" style={{ marginBottom: 8 }}>
            <strong>Coverage fetch failed:</strong> {coverageError}
          </div>
        )}

        {coverage === null && !coverageLoading && !coverageError && (
          <div className="unavailable-notice" style={{ color: "var(--text-muted, #888)" }}>
            Coverage not loaded yet. Click Refresh or wait for auto-load.
          </div>
        )}

        {coverage !== null && (
          <>
            <div className="bt-job-meta" style={{ marginBottom: 4 }}>
              <span className="eyebrow">truth_state</span>{" "}
              <strong>{coverageTruthLabel(coverage.truth_state)}</strong>
              {coverage.timeframe && (
                <>
                  {" "}<span className="eyebrow">filter</span>{" "}
                  <strong>{coverage.timeframe}</strong>
                </>
              )}
              {coverage.truth_state === "active" && (
                <>
                  {" "}<span className="eyebrow">total rows</span>{" "}
                  <strong>{coverageSummary.totalDaemonRows}</strong>
                  {coverageSymbolSearch && (
                    <>
                      {" "}<span className="eyebrow">visible</span>{" "}
                      <strong>{coverageSummary.visibleRows}</strong>
                    </>
                  )}
                  {" "}<span className="eyebrow">visible bars</span>{" "}
                  <strong>{coverageSummary.visibleBars.toLocaleString()}</strong>
                </>
              )}
            </div>
            <CoverageTable coverage={coverage} rows={coverageSortedRows} nowSecs={nowSecs} />

            {missingTrackedSymbols === null && (
              <div className="bt-field-hint" style={{ marginTop: 10, fontSize: "0.82rem", color: "var(--text-muted, #888)" }}>
                Registry unavailable — cannot compute missing symbols.
              </div>
            )}
            {missingTrackedSymbols !== null && missingTrackedSymbols.length === 0 && (
              <div className="bt-field-hint" style={{ marginTop: 10, fontSize: "0.82rem", color: "var(--green, #4caf50)" }}>
                All tracked symbols have coverage{coverageFilter.trim() ? ` for ${coverageFilter.trim()}` : ""}.
              </div>
            )}
            {missingTrackedSymbols !== null && missingTrackedSymbols.length > 0 && (
              <div className="unavailable-notice" style={{ marginTop: 10 }}>
                <strong>
                  Missing tracked symbols ({missingTrackedSymbols.length})
                  {coverageFilter.trim() ? ` for ${coverageFilter.trim()}` : ""}:
                </strong>{" "}
                {missingTrackedSymbols.slice(0, 25).join(", ")}
                {missingTrackedSymbols.length > 25 && (
                  <span style={{ color: "var(--text-muted, #888)" }}>
                    {" "}+{missingTrackedSymbols.length - 25} more
                  </span>
                )}
              </div>
            )}
          </>
        )}

        {coverageLoading && (
          <div className="bt-job-meta" style={{ color: "var(--accent)" }}>
            Loading coverage…
          </div>
        )}
      </Panel>

      {/* DATA-INGEST-GUI-SYNC-ALL-01: Tracked equities registry preview */}
      <Panel
        title="Tracked equities"
        subtitle="Registry-backed universe from config/instruments/equities.json. Read-only. No provider calls. No API credits consumed."
      >
        <div style={{ display: "flex", alignItems: "center", gap: 8, marginBottom: 10 }}>
          <button
            type="button"
            className="action-button"
            onClick={() => void loadTrackedEquities()}
            disabled={trackedEquitiesLoading}
            style={{ padding: "2px 12px" }}
          >
            {trackedEquitiesLoading ? "Loading…" : "Refresh"}
          </button>
        </div>

        {trackedEquitiesError && (
          <div className="unavailable-notice unavailable-critical" style={{ marginBottom: 8 }}>
            <strong>Fetch failed:</strong> {trackedEquitiesError}
          </div>
        )}

        {trackedEquities === null && !trackedEquitiesLoading && !trackedEquitiesError && (
          <div className="unavailable-notice" style={{ color: "var(--text-muted, #888)" }}>
            Not loaded yet. Click Refresh or wait for auto-load.
          </div>
        )}

        {trackedEquities !== null && (
          <>
            <div className="bt-job-meta" style={{ marginBottom: 6 }}>
              <span className="eyebrow">truth_state</span>{" "}
              <strong>{trackedEquitiesTruthLabel(trackedEquities.truth_state)}</strong>
              {isTrackedEquitiesActive(trackedEquities.truth_state) && (
                <>
                  {" "}<span className="eyebrow">count</span>{" "}
                  <strong>{trackedEquities.count}</strong>
                  {trackedEquities.first_symbol && (
                    <>
                      {" "}<span className="eyebrow">first</span>{" "}
                      <strong>{trackedEquities.first_symbol}</strong>
                    </>
                  )}
                  {trackedEquities.last_symbol && (
                    <>
                      {" "}<span className="eyebrow">last</span>{" "}
                      <strong>{trackedEquities.last_symbol}</strong>
                    </>
                  )}
                </>
              )}
            </div>

            {isTrackedEquitiesActive(trackedEquities.truth_state) && (
              <div className="bt-field-hint" style={{ marginBottom: 8, fontSize: "0.82rem" }}>
                <span className="eyebrow">registry</span>{" "}
                <code style={{ wordBreak: "break-all" }}>{trackedEquities.registry_path}</code>
              </div>
            )}

            {!isTrackedEquitiesActive(trackedEquities.truth_state) && (
              <div className="unavailable-notice" style={{ color: "var(--text-muted, #888)" }}>
                <strong>truth_state:</strong> {trackedEquitiesTruthLabel(trackedEquities.truth_state)}
                {trackedEquities.error ? ` — ${trackedEquities.error}` : ""}
              </div>
            )}

            <div className="bt-field-hint" style={{ marginTop: 8, fontSize: "0.82rem", color: "var(--text-muted, #888)" }}>
              Provider sync: see the Provider sync panel below.
            </div>
          </>
        )}

        {trackedEquitiesLoading && (
          <div className="bt-job-meta" style={{ color: "var(--accent)" }}>
            Loading tracked equities…
          </div>
        )}
      </Panel>

      {/* DATA-INGEST-GUI-PROVIDER-RUNNER-01: Provider sync runner */}
      <Panel
        title="Provider sync — TwelveData registry sync"
        subtitle="Submits a daemon ingest job to pull 1D bars from TwelveData for all registry symbols. No broker orders. No execution routes."
      >
        {/* Static config display */}
        <div className="bt-job-meta" style={{ marginBottom: 8 }}>
          <span className="eyebrow">source</span>{" "}
          <strong>twelvedata</strong>
          {" "}<span className="eyebrow">mode</span>{" "}
          <strong>sync_provider</strong>
          {" "}<span className="eyebrow">symbols</span>{" "}
          <strong>registry</strong>
          {" "}<span className="eyebrow">asset_class</span>{" "}
          <strong>equity</strong>
          {" "}<span className="eyebrow">timeframe</span>{" "}
          <strong>1D</strong>
        </div>
        <div className="bt-field-hint" style={{ marginBottom: 12, fontSize: "0.82rem" }}>
          <span className="eyebrow">registry</span>{" "}
          <code>config/instruments/equities.json</code>
        </div>

        {/* Optional date range */}
        <div className="bt-job-form-grid">
          <div className="bt-job-field">
            <label htmlFor="provider-start">Start date (optional)</label>
            <input
              id="provider-start"
              type="text"
              value={providerStart}
              onChange={(e) => setProviderStart(e.target.value)}
              placeholder="YYYY-MM-DD"
              spellCheck={false}
              autoComplete="off"
            />
          </div>
          <div className="bt-job-field">
            <label htmlFor="provider-end">End date (optional)</label>
            <input
              id="provider-end"
              type="text"
              value={providerEnd}
              onChange={(e) => setProviderEnd(e.target.value)}
              placeholder="YYYY-MM-DD"
              spellCheck={false}
              autoComplete="off"
            />
          </div>
        </div>

        {/* Dry-run section */}
        <div style={{ marginTop: 12, borderTop: "1px solid var(--border, rgba(255,255,255,0.08))", paddingTop: 12 }}>
          <div className="bt-field-hint" style={{ marginBottom: 8, fontSize: "0.82rem" }}>
            <strong>Dry-run (safe default):</strong> Resolves symbols from registry and validates
            config. Zero provider API calls. Zero DB writes.
          </div>
          <button
            type="button"
            className="action-button"
            onClick={() => void handleDryRunSubmit()}
            disabled={providerSubmitting || providerJobIsActive}
          >
            {providerSubmitting ? "Submitting…" : providerJobIsActive ? "Job running…" : "Run dry-run"}
          </button>
        </div>

        {/* Real sync opt-in section */}
        <div style={{ marginTop: 16, borderTop: "1px solid var(--border, rgba(255,255,255,0.08))", paddingTop: 12 }}>
          <div className="bt-field-hint" style={{ marginBottom: 8, fontSize: "0.82rem", color: "var(--text-muted, #888)" }}>
            <strong style={{ color: "var(--text)" }}>Real sync (explicit opt-in required):</strong>{" "}
            Calls TwelveData API and writes bars to md_bars.
            Provider API credits are consumed. Partial or failed jobs must be reviewed before re-run.
          </div>

          <label style={{ display: "flex", alignItems: "flex-start", gap: 8, cursor: "pointer", fontSize: "0.85rem", marginBottom: 10 }}>
            <input
              type="checkbox"
              checked={providerAllowApiCalls}
              onChange={(e) => {
                setProviderAllowApiCalls(e.target.checked);
                if (!e.target.checked) setProviderSyncConfirmation("");
              }}
              style={{ marginTop: 2 }}
            />
            I understand real sync consumes provider API credits and writes to the database
          </label>

          {providerAllowApiCalls && (
            <>
              <div className="unavailable-notice" style={{ marginBottom: 10, borderColor: "var(--accent, #c8a84b)" }}>
                <strong>Warning:</strong> Real sync will call TwelveData and consume API credits.
                Symbols are loaded from <code>config/instruments/equities.json</code>.
                Failed or partial results are not auto-retried — review job status after completion.
              </div>

              <div className="bt-job-form-grid" style={{ marginBottom: 10 }}>
                <div className="bt-job-field">
                  <label htmlFor="provider-credits-min">API credits / minute (optional guardrail)</label>
                  <input
                    id="provider-credits-min"
                    type="number"
                    value={providerApiCreditsPerMin}
                    onChange={(e) => setProviderApiCreditsPerMin(e.target.value)}
                    placeholder="e.g. 8"
                    min="1"
                    spellCheck={false}
                    autoComplete="off"
                  />
                </div>
                <div className="bt-job-field">
                  <label htmlFor="provider-credits-day">API credits / day (optional guardrail)</label>
                  <input
                    id="provider-credits-day"
                    type="number"
                    value={providerApiCreditsPerDay}
                    onChange={(e) => setProviderApiCreditsPerDay(e.target.value)}
                    placeholder="e.g. 800"
                    min="1"
                    spellCheck={false}
                    autoComplete="off"
                  />
                </div>
              </div>

              <div style={{ display: "flex", alignItems: "center", gap: 8, marginBottom: 10 }}>
                <label htmlFor="provider-sync-confirm" style={{ fontSize: "0.85rem", whiteSpace: "nowrap" }}>
                  Type SYNC to confirm:
                </label>
                <input
                  id="provider-sync-confirm"
                  type="text"
                  value={providerSyncConfirmation}
                  onChange={(e) => setProviderSyncConfirmation(e.target.value)}
                  placeholder="SYNC"
                  spellCheck={false}
                  autoComplete="off"
                  style={{ width: 120 }}
                />
              </div>

              <button
                type="button"
                className="action-button"
                onClick={() => void handleRealSyncSubmit()}
                disabled={
                  !isProviderSyncAllowed(true, providerSyncConfirmation) ||
                  providerSubmitting ||
                  providerJobIsActive
                }
              >
                Run real sync (consumes API credits)
              </button>
            </>
          )}
        </div>

        {providerSubmitError && (
          <div className="unavailable-notice unavailable-critical" style={{ marginTop: 10 }}>
            <strong>Submit failed:</strong> {providerSubmitError}
          </div>
        )}
      </Panel>

      {/* Provider sync job status */}
      {activeProviderJob && (
        <Panel
          title="Provider sync job status"
          subtitle="Daemon-managed TwelveData registry sync. Writes to md_bars only. No broker orders."
        >
          <div className="bt-job-status-row">
            <IngestJobStatusBadge status={activeProviderJob.status} />
            <span className="bt-job-meta" title={activeProviderJob.jobId}>
              job {activeProviderJob.jobId.slice(0, 8)}…
            </span>
            <span className="bt-job-meta">
              {activeProviderJob.dryRun ? "dry-run" : "real sync"}
            </span>
            {activeProviderJob.allowProviderApiCalls && (
              <span className="bt-job-meta" style={{ color: "var(--accent)" }}>
                provider API calls enabled
              </span>
            )}
            {activeProviderJob.completedAt && (
              <span className="bt-job-meta">
                completed {formatDateTime(activeProviderJob.completedAt)}
              </span>
            )}
            {!activeProviderJob.completedAt && activeProviderJob.startedAt && (
              <span className="bt-job-meta">
                started {formatDateTime(activeProviderJob.startedAt)}
              </span>
            )}
          </div>

          {(activeProviderJob.status === "queued" || activeProviderJob.status === "running") && (
            <div className="bt-job-meta" style={{ marginTop: 6, color: "var(--accent)" }}>
              Polling for status every 2 seconds…
            </div>
          )}

          {activeProviderJobCanCancel && (
            <div className="unavailable-notice" style={{ marginTop: 8 }}>
              <label style={{ display: "flex", alignItems: "flex-start", gap: 8, cursor: "pointer", fontSize: "0.85rem", marginBottom: 8 }}>
                <input
                  type="checkbox"
                  checked={providerCancelConfirmed}
                  onChange={(e) => {
                    setProviderCancelConfirmed(e.target.checked);
                    if (e.target.checked) setProviderCancelError(null);
                  }}
                  style={{ marginTop: 2 }}
                />
                I understand this cancels only the active provider sync job shown here
              </label>
              <button
                type="button"
                className="action-button ghost"
                onClick={() => void handleProviderCancel()}
                disabled={!providerCancelConfirmed || providerCancelling}
                style={{ padding: "2px 12px" }}
              >
                {providerCancelling ? "Cancelling…" : "Cancel provider job"}
              </button>
            </div>
          )}

          {providerCancelError && (
            <div className="unavailable-notice unavailable-critical" style={{ marginTop: 8 }}>
              <strong>Cancel failed:</strong> {providerCancelError}
            </div>
          )}

          {providerCancelNotice && (
            <div className="unavailable-notice" style={{ marginTop: 8 }}>
              {providerCancelNotice}
            </div>
          )}

          {/* Progress metrics */}
          <div className="timeline-meta-grid" style={{ marginTop: 8 }}>
            <div>
              <span>API calls made</span>
              <strong>{activeProviderJob.apiCallsMade}</strong>
            </div>
            <div>
              <span>Symbols planned</span>
              <strong>{activeProviderJob.symbolsCount ?? "—"}</strong>
            </div>
            {activeProviderJob.symbolsCompleted !== null && (
              <div>
                <span>Completed</span>
                <strong>{activeProviderJob.symbolsCompleted}</strong>
              </div>
            )}
            {activeProviderJob.symbolsFailed !== null && (
              <div>
                <span>Failed</span>
                <strong style={{ color: activeProviderJob.symbolsFailed > 0 ? "var(--red, #f44336)" : undefined }}>
                  {activeProviderJob.symbolsFailed}
                </strong>
              </div>
            )}
            {activeProviderJob.rowsInserted !== null && (
              <div>
                <span>Rows inserted</span>
                <strong>{activeProviderJob.rowsInserted}</strong>
              </div>
            )}
            {activeProviderJob.rowsRejected !== null && (
              <div>
                <span>Rows rejected</span>
                <strong>{activeProviderJob.rowsRejected}</strong>
              </div>
            )}
          </div>

          {(activeProviderJob.plannedFirstSymbol || activeProviderJob.plannedLastSymbol) && (
            <div className="bt-job-meta" style={{ marginTop: 6, fontSize: "0.82rem" }}>
              {activeProviderJob.plannedFirstSymbol && (
                <>
                  <span className="eyebrow">first symbol</span>{" "}
                  <strong>{activeProviderJob.plannedFirstSymbol}</strong>
                  {" "}
                </>
              )}
              {activeProviderJob.plannedLastSymbol && (
                <>
                  <span className="eyebrow">last symbol</span>{" "}
                  <strong>{activeProviderJob.plannedLastSymbol}</strong>
                </>
              )}
            </div>
          )}

          {/* Terminal state indicators */}
          {activeProviderJob.status === "dry_run_completed" && (
            <div className="unavailable-notice" style={{ marginTop: 8, borderColor: "var(--accent, #c8a84b)" }}>
              <strong>Dry-run completed.</strong> Zero provider API calls consumed. Zero DB writes.
              Registry symbols resolved and validated. Check symbols_count before running real sync.
            </div>
          )}

          {activeProviderJob.status === "partial" && (
            <div className="unavailable-notice" style={{ marginTop: 8, borderColor: "rgba(230,126,34,0.5)" }}>
              <strong>Partial completion — not a full success.</strong>{" "}
              Some symbols succeeded; others failed. Review failed symbols and consider re-running.
            </div>
          )}

          {activeProviderJob.status === "completed" && (
            <div className="unavailable-notice" style={{ marginTop: 8, borderColor: "rgba(76,175,80,0.4)" }}>
              <strong>Sync completed.</strong> All symbols processed successfully.
            </div>
          )}

          {activeProviderJob.status === "failed" && (
            <div className="unavailable-notice unavailable-critical" style={{ marginTop: 8 }}>
              <strong>Job failed:</strong>{" "}
              {activeProviderJob.error ?? "No error message returned. Check daemon logs."}
            </div>
          )}

          {activeProviderJob.status === "cancelled" && (
            <div className="unavailable-notice" style={{ marginTop: 8, borderColor: "var(--accent, #c8a84b)" }}>
              <strong>Job cancelled.</strong>{" "}
              {activeProviderJob.error ?? "Daemon marked this provider sync job cancelled."}
            </div>
          )}

          {activeProviderJob.status === "refused" && (
            <div className="unavailable-notice unavailable-critical" style={{ marginTop: 8 }}>
              <strong>Job refused:</strong>{" "}
              {activeProviderJob.error ?? "Daemon refused this provider sync job."}
            </div>
          )}

          {activeProviderJob.status === "unknown" && (
            <div className="unavailable-notice" style={{ marginTop: 8 }}>
              <strong>Unrecognized status.</strong>{" "}
              Daemon returned an unknown status string — check daemon version.
            </div>
          )}
        </Panel>
      )}

      {/* DATA-PROVIDER-GUI-FEED-SCHEDULER-01: Latest closed-bar feed scheduler */}
      <Panel
        title="Latest Bar Feed"
        subtitle="Operator controls for /api/v1/market-data/feed/*. Dry-run is safe; real provider calls require explicit confirmation."
      >
        <div className="unavailable-notice" style={{ marginBottom: 10 }}>
          <strong>Scheduler state is process-local and not persisted.</strong>{" "}
          Feed status can be lost on daemon restart. Limitation: <code>{latestFeedStatusLimitation}</code>
        </div>

        <div className="bt-job-form-grid">
          <div className="bt-job-field">
            <label htmlFor="latest-feed-provider">Provider id</label>
            <input
              id="latest-feed-provider"
              type="text"
              value={latestFeedProviderId}
              onChange={(e) => setLatestFeedProviderId(e.target.value)}
              spellCheck={false}
              autoComplete="off"
            />
          </div>
          <div className="bt-job-field">
            <label htmlFor="latest-feed-timeframe">Timeframe</label>
            <input
              id="latest-feed-timeframe"
              type="text"
              value={latestFeedTimeframe}
              onChange={(e) => setLatestFeedTimeframe(e.target.value)}
              placeholder="5m"
              spellCheck={false}
              autoComplete="off"
            />
          </div>
          <div className="bt-job-field" style={{ gridColumn: "1 / -1" }}>
            <label htmlFor="latest-feed-symbols">Symbols</label>
            <input
              id="latest-feed-symbols"
              type="text"
              value={latestFeedSymbolsText}
              onChange={(e) => {
                setLatestFeedSymbolsText(e.target.value);
                setLatestFeedSymbolsSeeded(true);
              }}
              placeholder="AAPL, MSFT"
              spellCheck={false}
              autoComplete="off"
            />
          </div>
        </div>

        <div className="timeline-meta-grid" style={{ marginTop: 10 }}>
          <div>
            <span>feed status</span>
            <strong>{latestFeedStatus?.truth_state ?? "unknown"}</strong>
          </div>
          <div>
            <span>scheduler</span>
            <strong>{latestFeedSchedulerStatus?.truth_state ?? "unknown"}</strong>
          </div>
          <div>
            <span>running</span>
            <strong>{latestFeedSchedulerStatus?.running === true ? "yes" : latestFeedSchedulerStatus?.running === false ? "no" : "unknown"}</strong>
          </div>
          <div>
            <span>provider</span>
            <strong>{latestFeedSchedulerStatus?.provider_id ?? latestFeedDisplayResult?.provider_id ?? "unknown"}</strong>
          </div>
          <div>
            <span>timeframe</span>
            <strong>{latestFeedSchedulerStatus?.timeframe ?? latestFeedDisplayResult?.timeframe ?? "unknown"}</strong>
          </div>
          <div>
            <span>symbols</span>
            <strong>{latestFeedSchedulerStatus?.symbols.length ? latestFeedSchedulerStatus.symbols.join(", ") : latestFeedSymbols.join(", ")}</strong>
          </div>
          <div>
            <span>last poll</span>
            <strong>{shortUtc(latestFeedSchedulerStatus?.last_poll_utc ?? latestFeedDisplayResult?.poll_time_utc)}</strong>
          </div>
          <div>
            <span>next poll</span>
            <strong>{shortUtc(latestFeedSchedulerStatus?.next_poll_utc)}</strong>
          </div>
          <div>
            <span>expected closed bar</span>
            <strong>{shortUtc(latestFeedSchedulerStatus?.latest_expected_closed_bar_utc)}</strong>
          </div>
          <div>
            <span>poll count</span>
            <strong>{numericValue(latestFeedSchedulerStatus?.poll_count)}</strong>
          </div>
          <div>
            <span>inserted</span>
            <strong>{numericValue(latestFeedSchedulerStatus?.inserted_count ?? latestFeedDisplayResult?.inserted_count)}</strong>
          </div>
          <div>
            <span>skipped/unchanged</span>
            <strong>{numericValue(latestFeedSchedulerStatus?.unchanged_or_skipped_count ?? latestFeedDisplayResult?.skipped_count)}</strong>
          </div>
          <div>
            <span>errors</span>
            <strong style={{ color: (latestFeedSchedulerStatus?.error_count ?? latestFeedDisplayResult?.error_count ?? 0) > 0 ? "var(--red, #f44336)" : undefined }}>
              {numericValue(latestFeedSchedulerStatus?.error_count ?? latestFeedDisplayResult?.error_count)}
            </strong>
          </div>
          <div>
            <span>last result</span>
            <strong>{latestBarFeedResultLabel(latestFeedDisplayResult)}</strong>
          </div>
        </div>

        {latestFeedSchedulerStatus?.last_error && (
          <div className="unavailable-notice unavailable-critical" style={{ marginTop: 10 }}>
            <strong>Scheduler error:</strong> {latestFeedSchedulerStatus.last_error}
          </div>
        )}

        {latestFeedDisplayResult?.symbols.length ? (
          <table className="bt-table" style={{ width: "100%", tableLayout: "auto", marginTop: 10 }}>
            <thead>
              <tr>
                <th>Symbol</th>
                <th>Status</th>
                <th style={{ textAlign: "right" }}>Inserted</th>
                <th style={{ textAlign: "right" }}>Updated</th>
                <th style={{ textAlign: "right" }}>Skipped</th>
                <th>Error</th>
              </tr>
            </thead>
            <tbody>
              {latestFeedDisplayResult.symbols.map((symbol) => (
                <tr key={`${symbol.symbol}|${symbol.status}`}>
                  <td><strong>{symbol.symbol}</strong></td>
                  <td>{symbol.status}</td>
                  <td style={{ textAlign: "right", fontVariantNumeric: "tabular-nums" }}>{numericValue(symbol.rows_inserted)}</td>
                  <td style={{ textAlign: "right", fontVariantNumeric: "tabular-nums" }}>{numericValue(symbol.rows_updated)}</td>
                  <td style={{ textAlign: "right", fontVariantNumeric: "tabular-nums" }}>{numericValue(symbol.rows_skipped)}</td>
                  <td style={{ color: symbol.error ? "var(--red, #f44336)" : "var(--text-muted, #888)" }}>
                    {symbol.error ?? "—"}
                  </td>
                </tr>
              ))}
            </tbody>
          </table>
        ) : (
          <div className="bt-job-meta" style={{ marginTop: 10, color: "var(--text-muted, #888)" }}>
            No latest-bar poll result loaded.
          </div>
        )}

        <div style={{ display: "flex", flexWrap: "wrap", alignItems: "center", gap: 8, marginTop: 12 }}>
          <button
            type="button"
            className="action-button"
            onClick={() => void loadLatestBarFeedStatus()}
            disabled={latestFeedLoading || latestFeedActionPending}
            style={{ padding: "2px 12px" }}
          >
            {latestFeedLoading ? "Loading…" : "Refresh status"}
          </button>
          <button
            type="button"
            className="action-button"
            onClick={() => void handleLatestFeedPoll(false)}
            disabled={!latestFeedCanRun}
            style={{ padding: "2px 12px" }}
          >
            Poll once dry-run
          </button>
          <button
            type="button"
            className="action-button"
            onClick={() => void handleLatestFeedStart(false)}
            disabled={!latestFeedCanRun}
            style={{ padding: "2px 12px" }}
          >
            Start scheduler dry-run
          </button>
          <button
            type="button"
            className="action-button ghost"
            onClick={() => void handleLatestFeedStop()}
            disabled={latestFeedActionPending}
            style={{ padding: "2px 12px" }}
          >
            Stop scheduler
          </button>
        </div>

        <div style={{ marginTop: 14, borderTop: "1px solid var(--border, rgba(255,255,255,0.08))", paddingTop: 12 }}>
          <div className="bt-field-hint" style={{ marginBottom: 8, fontSize: "0.82rem", color: "var(--text-muted, #888)" }}>
            <strong style={{ color: "var(--text)" }}>Real provider calls:</strong>{" "}
            Real poll/start sends <code>allow_provider_api_calls=true</code> and may write md_bars. No orders are submitted.
          </div>
          <div style={{ display: "flex", flexWrap: "wrap", alignItems: "center", gap: 8 }}>
            <label htmlFor="latest-feed-poll-confirm" style={{ fontSize: "0.85rem" }}>
              Type POLL:
            </label>
            <input
              id="latest-feed-poll-confirm"
              type="text"
              value={latestFeedPollConfirmation}
              onChange={(e) => setLatestFeedPollConfirmation(e.target.value)}
              placeholder="POLL"
              spellCheck={false}
              autoComplete="off"
              style={{ width: 100 }}
            />
            <button
              type="button"
              className="action-button"
              onClick={() => void handleLatestFeedPoll(true)}
              disabled={
                !latestFeedCanRun ||
                !isMarketDataFeedRealActionAllowed(true, latestFeedPollConfirmation, "POLL")
              }
              style={{ padding: "2px 12px" }}
            >
              Poll once real
            </button>
          </div>

          <label style={{ display: "flex", alignItems: "flex-start", gap: 8, cursor: "pointer", fontSize: "0.85rem", marginTop: 10 }}>
            <input
              type="checkbox"
              checked={latestFeedPollImmediately}
              onChange={(e) => setLatestFeedPollImmediately(e.target.checked)}
              style={{ marginTop: 2 }}
            />
            Poll immediately after scheduler start
          </label>

          <div style={{ display: "flex", flexWrap: "wrap", alignItems: "center", gap: 8, marginTop: 10 }}>
            <label htmlFor="latest-feed-start-confirm" style={{ fontSize: "0.85rem" }}>
              Type START:
            </label>
            <input
              id="latest-feed-start-confirm"
              type="text"
              value={latestFeedStartConfirmation}
              onChange={(e) => setLatestFeedStartConfirmation(e.target.value)}
              placeholder="START"
              spellCheck={false}
              autoComplete="off"
              style={{ width: 110 }}
            />
            <button
              type="button"
              className="action-button"
              onClick={() => void handleLatestFeedStart(true)}
              disabled={
                !latestFeedCanRun ||
                !isMarketDataFeedRealActionAllowed(true, latestFeedStartConfirmation, "START")
              }
              style={{ padding: "2px 12px" }}
            >
              Start scheduler real
            </button>
          </div>
        </div>

        {latestFeedNotice && (
          <div className="unavailable-notice" style={{ marginTop: 10 }}>
            {latestFeedNotice}
          </div>
        )}

        {latestFeedError && (
          <div className="unavailable-notice unavailable-critical" style={{ marginTop: 10 }}>
            <strong>Latest-bar feed error:</strong> {latestFeedError}
          </div>
        )}

        {latestFeedActionPending && (
          <div className="bt-job-meta" style={{ marginTop: 8, color: "var(--accent)" }}>
            Latest-bar feed action pending…
          </div>
        )}
      </Panel>

      {/* INTRADAY-MD-REFRESHER-GUI-01: Intraday refresh status */}
      <Panel
        title="Intraday refresh status"
        subtitle="Read-only evidence from Refresh-IntradayMarketData.ps1. No provider calls. No DB writes."
      >
        <div style={{ display: "flex", alignItems: "center", gap: 8, marginBottom: 10 }}>
          <button
            type="button"
            className="action-button"
            onClick={() => void loadIntradayRefresh()}
            disabled={intradayRefreshLoading}
            style={{ padding: "2px 12px" }}
          >
            {intradayRefreshLoading ? "Loading…" : "Refresh"}
          </button>
        </div>

        {intradayRefreshError && (
          <div className="unavailable-notice unavailable-critical" style={{ marginBottom: 8 }}>
            <strong>Fetch failed:</strong> {intradayRefreshError}
          </div>
        )}

        {intradayRefresh === null && !intradayRefreshLoading && !intradayRefreshError && (
          <div className="unavailable-notice" style={{ color: "var(--text-muted, #888)" }}>
            Not loaded yet. Click Refresh or wait for auto-load.
          </div>
        )}

        {intradayRefresh !== null && (
          <>
            <div className="bt-job-meta" style={{ marginBottom: 6 }}>
              <span className="eyebrow">truth_state</span>{" "}
              <strong>{intradayRefreshTruthLabel(intradayRefresh.truth_state)}</strong>
              {intradayRefresh.produced_at_utc && (
                <>
                  {" "}<span className="eyebrow">produced</span>{" "}
                  <strong>{intradayRefresh.produced_at_utc.slice(0, 19).replace("T", " ")}</strong>
                </>
              )}
              {intradayRefresh.mode && (
                <>
                  {" "}<span className="eyebrow">mode</span>{" "}
                  <strong>{intradayRefresh.mode}</strong>
                </>
              )}
              {intradayRefresh.source && (
                <>
                  {" "}<span className="eyebrow">source</span>{" "}
                  <strong>{intradayRefresh.source}</strong>
                </>
              )}
              {intradayRefresh.timeframe && (
                <>
                  {" "}<span className="eyebrow">timeframe</span>{" "}
                  <strong>{intradayRefresh.timeframe}</strong>
                </>
              )}
            </div>

            {intradayRefresh.stale_or_missing_evidence && (
              <div className="unavailable-notice" style={{ marginBottom: 8 }}>
                <strong>Evidence is stale or missing.</strong>{" "}
                Run <code>Refresh-IntradayMarketData.ps1</code> to refresh.
              </div>
            )}

            {isIntradayRefreshActive(intradayRefresh.truth_state) && (
              <>
                <div className="bt-job-meta" style={{ marginBottom: 6 }}>
                  <span className="eyebrow">all_passed</span>{" "}
                  <strong style={{ color: intradayRefresh.all_passed ? "var(--green, #4caf50)" : "var(--red, #f44336)" }}>
                    {intradayRefresh.all_passed === true ? "PASS" : intradayRefresh.all_passed === false ? "FAIL" : "—"}
                  </strong>
                  {intradayRefresh.reason && (
                    <>
                      {" "}<span className="eyebrow">reason</span>{" "}
                      <span>{intradayRefresh.reason}</span>
                    </>
                  )}
                </div>

                {intradayRefresh.symbols.length > 0 && (
                  <table className="bt-table" style={{ width: "100%", tableLayout: "auto" }}>
                    <thead>
                      <tr>
                        <th>Symbol</th>
                        <th>Gate</th>
                        <th style={{ textAlign: "right" }}>Bars</th>
                        <th style={{ textAlign: "right" }}>Stale (min)</th>
                        <th>Fail reasons</th>
                      </tr>
                    </thead>
                    <tbody>
                      {intradayRefresh.symbols.map((sym) => (
                        <tr key={sym.symbol}>
                          <td><strong>{sym.symbol}</strong></td>
                          <td style={{ color: sym.gate === "PASS" ? "var(--green, #4caf50)" : sym.gate === "FAIL" ? "var(--red, #f44336)" : undefined }}>
                            {sym.gate ?? "—"}
                          </td>
                          <td style={{ textAlign: "right", fontVariantNumeric: "tabular-nums" }}>
                            {sym.completed_count ?? "—"}
                          </td>
                          <td style={{ textAlign: "right", fontVariantNumeric: "tabular-nums" }}>
                            {sym.staleness_min ?? "—"}
                          </td>
                          <td style={{ color: "var(--text-muted, #888)", fontSize: "0.82rem" }}>
                            {sym.fail_reasons.length > 0 ? sym.fail_reasons.join("; ") : "—"}
                          </td>
                        </tr>
                      ))}
                    </tbody>
                  </table>
                )}
              </>
            )}

            {!isIntradayRefreshActive(intradayRefresh.truth_state) && (
              <div className="unavailable-notice" style={{ color: "var(--text-muted, #888)" }}>
                <strong>truth_state:</strong> {intradayRefreshTruthLabel(intradayRefresh.truth_state)}
                {intradayRefresh.error ? ` — ${intradayRefresh.error}` : ""}
              </div>
            )}
          </>
        )}

        {intradayRefreshLoading && (
          <div className="bt-job-meta" style={{ color: "var(--accent)" }}>
            Loading intraday refresh status…
          </div>
        )}
      </Panel>

      {/* CRYPTO-DATA-01Q-R-LATEST-MARK-GUI-SURFACE-BUNDLE-01-COMBINED: Crypto latest marks */}
      <Panel
        title="Crypto latest marks"
        subtitle="Read-only evidence from mqk md coinlore-latest-mark. No provider calls. No DB writes."
      >
        <div style={{ display: "flex", alignItems: "center", gap: 8, marginBottom: 10 }}>
          <button
            type="button"
            className="action-button"
            onClick={() => void loadLatestMarkStatus()}
            disabled={latestMarkStatusLoading}
            style={{ padding: "2px 12px" }}
          >
            {latestMarkStatusLoading ? "Loading…" : "Refresh"}
          </button>
        </div>

        <div className="unavailable-notice" style={{ marginBottom: 10, color: "var(--text-muted, #888)" }}>
          <strong>Ticker-only latest marks.</strong> Not OHLCV, not md_bars, not portfolio valuation, and not trading enablement.
        </div>

        {latestMarkStatusError && (
          <div className="unavailable-notice unavailable-critical" style={{ marginBottom: 8 }}>
            <strong>Fetch failed:</strong> {latestMarkStatusError}
          </div>
        )}

        {latestMarkStatus === null && !latestMarkStatusLoading && !latestMarkStatusError && (
          <div className="unavailable-notice" style={{ color: "var(--text-muted, #888)" }}>
            Not loaded yet. Click Refresh or wait for auto-load.
          </div>
        )}

        {latestMarkStatus !== null && (
          <>
            {isLatestMarkEvidenceUnsafe(latestMarkStatus) && (
              <div className="unavailable-notice unavailable-critical" style={{ marginBottom: 8 }}>
                <strong>{latestMarkStatusTruthLabel("unsafe_evidence")}</strong>
                {latestMarkStatus.error ? ` — ${latestMarkStatus.error}` : ""}
              </div>
            )}

            <div className="bt-job-meta" style={{ marginBottom: 6 }}>
              <span className="eyebrow">truth_state</span>{" "}
              <strong>{latestMarkStatusTruthLabel(latestMarkStatus.truth_state)}</strong>
              {latestMarkStatus.provider && (
                <>
                  {" "}<span className="eyebrow">provider</span>{" "}
                  <strong>{latestMarkStatus.provider}</strong>
                </>
              )}
              {latestMarkStatus.produced_at_utc && (
                <>
                  {" "}<span className="eyebrow">produced</span>{" "}
                  <strong>{latestMarkStatus.produced_at_utc.slice(0, 19).replace("T", " ")}</strong>
                </>
              )}
              {" "}<span className="eyebrow">provider_enabled</span>{" "}
              <strong>{latestMarkStatus.provider_enabled === null ? "unknown" : String(latestMarkStatus.provider_enabled)}</strong>
            </div>

            {latestMarkStatus.evidence_path && (
              <div className="bt-field-hint" style={{ marginBottom: 8, fontSize: "0.82rem" }}>
                <span className="eyebrow">evidence</span>{" "}
                <code style={{ wordBreak: "break-all" }}>{latestMarkStatus.evidence_path}</code>
              </div>
            )}

            {latestMarkStatus.stale_or_missing_evidence && (
              <div className="unavailable-notice" style={{ marginBottom: 8 }}>
                <strong>Evidence is stale or missing.</strong>{" "}
                Run <code>mqk md coinlore-latest-mark</code> to refresh.
              </div>
            )}

            <div className="timeline-meta-grid" style={{ marginBottom: 8 }}>
              <div>
                <span>network_call_made</span>
                <strong>{latestMarkStatus.network_call_made === null ? "unknown" : String(latestMarkStatus.network_call_made)}</strong>
              </div>
              <div>
                <span>db_write</span>
                <strong>{latestMarkStatus.db_write === null ? "unknown" : String(latestMarkStatus.db_write)}</strong>
              </div>
              <div>
                <span>md_bars_write</span>
                <strong>{latestMarkStatus.md_bars_write === null ? "unknown" : String(latestMarkStatus.md_bars_write)}</strong>
              </div>
              <div>
                <span>completed_bar_claim</span>
                <strong>{latestMarkStatus.completed_bar_claim === null ? "unknown" : String(latestMarkStatus.completed_bar_claim)}</strong>
              </div>
            </div>

            <div className="bt-job-meta" style={{ marginBottom: 8 }}>
              <span className="eyebrow">symbols requested</span>{" "}
              <strong>{latestMarkStatus.symbols_requested.length > 0 ? latestMarkStatus.symbols_requested.join(", ") : "—"}</strong>
            </div>

            {isLatestMarkStatusActive(latestMarkStatus.truth_state) && !isLatestMarkEvidenceUnsafe(latestMarkStatus) && latestMarkStatus.marks.length > 0 && (
              <table className="bt-table" style={{ width: "100%", tableLayout: "auto" }}>
                <thead>
                  <tr>
                    <th>Symbol</th>
                    <th>Price (USD)</th>
                    <th>Volume 24h (USD)</th>
                    <th>Provider symbol</th>
                    <th>Coin ID</th>
                    <th>As-of (client)</th>
                    <th>Provider ts</th>
                    <th>Kind</th>
                    <th>Truth state</th>
                  </tr>
                </thead>
                <tbody>
                  {latestMarkStatus.marks.map((mark) => (
                    <tr key={mark.canonical_symbol}>
                      <td><strong>{mark.canonical_symbol}</strong></td>
                      <td style={{ fontVariantNumeric: "tabular-nums" }}>{mark.price_usd ?? "—"}</td>
                      <td style={{ fontVariantNumeric: "tabular-nums" }}>{mark.volume24_usd ?? "—"}</td>
                      <td>{mark.provider_symbol ?? "—"}</td>
                      <td>{mark.provider_coin_id ?? "—"}</td>
                      <td style={{ fontVariantNumeric: "tabular-nums" }}>{formatUnixSecondsDateTime(mark.as_of_client_request_ts)}</td>
                      <td style={{ fontVariantNumeric: "tabular-nums" }}>{mark.provider_ts === null ? "none" : formatUnixSecondsDateTime(mark.provider_ts)}</td>
                      <td>{mark.kind ?? "—"}</td>
                      <td>{mark.truth_state ?? "—"}</td>
                    </tr>
                  ))}
                </tbody>
              </table>
            )}

            {!isLatestMarkStatusActive(latestMarkStatus.truth_state) && (
              <div className="unavailable-notice" style={{ color: "var(--text-muted, #888)" }}>
                <strong>truth_state:</strong> {latestMarkStatusTruthLabel(latestMarkStatus.truth_state)}
                {latestMarkStatus.error ? ` — ${latestMarkStatus.error}` : ""}
              </div>
            )}
          </>
        )}

        {latestMarkStatusLoading && (
          <div className="bt-job-meta" style={{ color: "var(--accent)" }}>
            Loading latest-mark status…
          </div>
        )}
      </Panel>

      {/* CRYPTO-DATA-01AE-KRAKEN-SYNC-GUI-STATUS-SURFACE-01: Kraken OHLCV sync status */}
      <Panel
        title="Kraken OHLCV sync status"
        subtitle="Read-only evidence from mqk md kraken-ohlc-ingest / kraken-ohlc-sync. No provider calls. No DB writes."
      >
        <div style={{ display: "flex", alignItems: "center", gap: 8, marginBottom: 10 }}>
          <button
            type="button"
            className="action-button"
            onClick={() => void loadKrakenOhlcStatus()}
            disabled={krakenOhlcStatusLoading}
            style={{ padding: "2px 12px" }}
          >
            {krakenOhlcStatusLoading ? "Loading…" : "Refresh"}
          </button>
        </div>

        <div className="unavailable-notice" style={{ marginBottom: 10, color: "var(--text-muted, #888)" }}>
          <strong>{KRAKEN_OHLC_STATUS_WARNING_TEXT}</strong>
        </div>

        {krakenOhlcStatusError && (
          <div className="unavailable-notice unavailable-critical" style={{ marginBottom: 8 }}>
            <strong>Fetch failed:</strong> {krakenOhlcStatusError}
          </div>
        )}

        {krakenOhlcStatus === null && !krakenOhlcStatusLoading && !krakenOhlcStatusError && (
          <div className="unavailable-notice" style={{ color: "var(--text-muted, #888)" }}>
            Not loaded yet. Click Refresh or wait for auto-load.
          </div>
        )}

        {krakenOhlcStatus !== null && (
          <>
            {isKrakenOhlcEvidenceUnsafe(krakenOhlcStatus) && (
              <div className="unavailable-notice unavailable-critical" style={{ marginBottom: 8 }}>
                <strong>{krakenOhlcStatusTruthLabel("unsafe_evidence")}</strong>
                {krakenOhlcStatus.error ? ` — ${krakenOhlcStatus.error}` : ""}
              </div>
            )}

            <div className="bt-job-meta" style={{ marginBottom: 6 }}>
              <span className="eyebrow">truth_state</span>{" "}
              <strong>{krakenOhlcStatusTruthLabel(krakenOhlcStatus.truth_state)}</strong>
              {krakenOhlcStatus.provider && (
                <>
                  {" "}<span className="eyebrow">provider</span>{" "}
                  <strong>{krakenOhlcStatus.provider}</strong>
                </>
              )}
              {krakenOhlcStatus.latest_mode && (
                <>
                  {" "}<span className="eyebrow">mode</span>{" "}
                  <strong>{krakenOhlcStatus.latest_mode}</strong>
                </>
              )}
              {krakenOhlcStatus.produced_at_utc && (
                <>
                  {" "}<span className="eyebrow">produced</span>{" "}
                  <strong>{krakenOhlcStatus.produced_at_utc.slice(0, 19).replace("T", " ")}</strong>
                </>
              )}
            </div>

            {krakenOhlcStatus.evidence_path && (
              <div className="bt-field-hint" style={{ marginBottom: 8, fontSize: "0.82rem" }}>
                <span className="eyebrow">evidence</span>{" "}
                <code style={{ wordBreak: "break-all" }}>{krakenOhlcStatus.evidence_path}</code>
              </div>
            )}

            {krakenOhlcStatus.stale_or_missing_evidence && (
              <div className="unavailable-notice" style={{ marginBottom: 8 }}>
                <strong>Evidence is stale or missing.</strong>{" "}
                Run <code>mqk md kraken-ohlc-sync</code> to refresh.
              </div>
            )}

            <div className="timeline-meta-grid" style={{ marginBottom: 8 }}>
              <div>
                <span>network_call_made</span>
                <strong>{krakenOhlcStatus.network_call_made === null ? "unknown" : String(krakenOhlcStatus.network_call_made)}</strong>
              </div>
              <div>
                <span>db_write</span>
                <strong>{krakenOhlcStatus.db_write === null ? "unknown" : String(krakenOhlcStatus.db_write)}</strong>
              </div>
              <div>
                <span>md_bars_write</span>
                <strong>{krakenOhlcStatus.md_bars_write === null ? "unknown" : String(krakenOhlcStatus.md_bars_write)}</strong>
              </div>
              <div>
                <span>provider_id</span>
                <strong>{krakenOhlcStatus.provider_id ?? "—"}</strong>
              </div>
              <div>
                <span>provider_source</span>
                <strong>{krakenOhlcStatus.provider_source ?? "—"}</strong>
              </div>
              <div>
                <span>provider_symbol</span>
                <strong>{krakenOhlcStatus.provider_symbol ?? "—"}</strong>
              </div>
              <div>
                <span>ingest_mode</span>
                <strong>{krakenOhlcStatus.ingest_mode ?? "—"}</strong>
              </div>
              <div>
                <span>sync_policy</span>
                <strong>{krakenOhlcStatus.sync_policy ?? "—"}</strong>
              </div>
              <div>
                <span>bars completed</span>
                <strong>{krakenOhlcStatus.bars_completed ?? "—"}</strong>
              </div>
              <div>
                <span>bars excluded forming</span>
                <strong>{krakenOhlcStatus.bars_excluded_forming ?? "—"}</strong>
              </div>
              <div>
                <span>rows inserted</span>
                <strong>{krakenOhlcStatus.rows_inserted ?? "—"}</strong>
              </div>
              <div>
                <span>rows updated</span>
                <strong>{krakenOhlcStatus.rows_updated ?? "—"}</strong>
              </div>
              <div>
                <span>rows skipped unchanged</span>
                <strong>{krakenOhlcStatus.rows_skipped_unchanged ?? "—"}</strong>
              </div>
              <div>
                <span>rows changed</span>
                <strong>{krakenOhlcStatus.rows_changed ?? "—"}</strong>
              </div>
              <div>
                <span>rows changed skipped (no-update-existing)</span>
                <strong>{krakenOhlcStatus.rows_changed_skipped_due_to_no_update_existing ?? "—"}</strong>
              </div>
              <div>
                <span>latest existing end_ts before</span>
                <strong>{krakenOhlcStatus.latest_existing_end_ts_before ?? "—"}</strong>
              </div>
              <div>
                <span>latest completed start_ts</span>
                <strong>{krakenOhlcStatus.latest_completed_start_ts ?? "—"}</strong>
              </div>
              <div>
                <span>latest completed end_ts</span>
                <strong>{krakenOhlcStatus.latest_completed_end_ts ?? "—"}</strong>
              </div>
              <div>
                <span>volume semantics</span>
                <strong>{krakenOhlcStatus.volume_semantics ?? "—"}</strong>
              </div>
              <div>
                <span>volume scale</span>
                <strong>{krakenOhlcStatus.volume_scale ?? "—"}</strong>
              </div>
              <div>
                <span>all_passed</span>
                <strong>{krakenOhlcStatus.all_passed === null ? "unknown" : String(krakenOhlcStatus.all_passed)}</strong>
              </div>
              <div>
                <span>reason_code</span>
                <strong>{krakenOhlcStatus.reason_code ?? "—"}</strong>
              </div>
            </div>

            <div className="bt-job-meta" style={{ marginBottom: 8 }}>
              <span className="eyebrow">symbols requested</span>{" "}
              <strong>{krakenOhlcStatus.symbols_requested.length > 0 ? krakenOhlcStatus.symbols_requested.join(", ") : "—"}</strong>
            </div>

            {krakenOhlcStatus.fail_reasons.length > 0 && (
              <div className="unavailable-notice" style={{ marginBottom: 8 }}>
                <strong>fail_reasons:</strong> {krakenOhlcStatus.fail_reasons.join(", ")}
              </div>
            )}

            {!isKrakenOhlcStatusActive(krakenOhlcStatus.truth_state) && (
              <div className="unavailable-notice" style={{ color: "var(--text-muted, #888)" }}>
                <strong>truth_state:</strong> {krakenOhlcStatusTruthLabel(krakenOhlcStatus.truth_state)}
                {krakenOhlcStatus.error ? ` — ${krakenOhlcStatus.error}` : ""}
              </div>
            )}
          </>
        )}

        {krakenOhlcStatusLoading && (
          <div className="bt-job-meta" style={{ color: "var(--accent)" }}>
            Loading Kraken OHLC status…
          </div>
        )}
      </Panel>

    </div>
  );
}
