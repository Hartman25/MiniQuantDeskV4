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
  coverageTruthLabel,
  fetchIntradayRefreshStatus,
  fetchMdBarsCoverage,
  fetchTrackedEquities,
  formatEndTs,
  getIngestJob,
  isCoverageActive,
  isIntradayRefreshActive,
  isProviderSyncAllowed,
  isTrackedEquitiesActive,
  isTerminalIngestStatus,
  intradayRefreshTruthLabel,
  normalizeIngestJobStatus,
  submitIngestJob,
  trackedEquitiesTruthLabel,
} from "./api.ts";
import { buildRepoRelativePath, buildMd1DSymbolPath, MD_BACKUP_1D_SEGMENTS, MD_INGEST_SEGMENTS } from "../backtests/pathHelpers.ts";
import { getDesktopRepoRoot } from "../../desktop/bootstrap.ts";
import type { ActiveIngestJob, ActiveProviderJob, IngestJobStatusKind, IntradayRefreshStatusResponse, MdBarsCoverageResponse, TrackedEquitiesResponse } from "./types.ts";

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

// ---------------------------------------------------------------------------
// Main screen component
// ---------------------------------------------------------------------------

// ---------------------------------------------------------------------------
// Coverage table component
// ---------------------------------------------------------------------------

function CoverageTable({ coverage }: { coverage: MdBarsCoverageResponse }) {
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
          <th>Last Ingested</th>
        </tr>
      </thead>
      <tbody>
        {coverage.rows.map((row) => (
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
            <td style={{ color: "var(--text-muted, #888)" }}>
              {row.latest_ingested_at
                ? row.latest_ingested_at.slice(0, 19).replace("T", " ")
                : "—"}
            </td>
          </tr>
        ))}
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

  // Coverage state — read-only view of what's in md_bars
  const [coverageFilter, setCoverageFilter] = useState("1D");
  const [coverage, setCoverage] = useState<MdBarsCoverageResponse | null>(null);
  const [coverageLoading, setCoverageLoading] = useState(false);
  const [coverageError, setCoverageError] = useState<string | null>(null);

  // DATA-INGEST-GUI-SYNC-ALL-01: Tracked-equities registry preview state
  const [trackedEquities, setTrackedEquities] = useState<TrackedEquitiesResponse | null>(null);
  const [trackedEquitiesLoading, setTrackedEquitiesLoading] = useState(false);
  const [trackedEquitiesError, setTrackedEquitiesError] = useState<string | null>(null);

  // INTRADAY-MD-REFRESHER-GUI-01: Intraday refresh status state
  const [intradayRefresh, setIntradayRefresh] = useState<IntradayRefreshStatusResponse | null>(null);
  const [intradayRefreshLoading, setIntradayRefreshLoading] = useState(false);
  const [intradayRefreshError, setIntradayRefreshError] = useState<string | null>(null);

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

  const jobIsActive = activeJob !== null && !isTerminalIngestStatus(activeJob.status);
  const providerJobIsActive = activeProviderJob !== null && !isTerminalIngestStatus(activeProviderJob.status);

  const repoRoot = getDesktopRepoRoot();
  const md1DDir = buildRepoRelativePath(repoRoot, ...MD_BACKUP_1D_SEGMENTS);

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

          {activeJob.status === "completed" && (
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
        </Panel>
      )}

      {/* Local coverage — read-only view of what's in md_bars */}
      <Panel
        title="Local data coverage"
        subtitle="Read-only view of what md_bars data exists locally. No DB writes. No provider calls."
      >
        <div style={{ display: "flex", alignItems: "center", gap: 8, marginBottom: 10 }}>
          <label htmlFor="coverage-timeframe" style={{ fontSize: "0.85rem" }}>
            Timeframe filter
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
            <div className="bt-job-meta" style={{ marginBottom: 6 }}>
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
                  {" "}<span className="eyebrow">groups</span>{" "}
                  <strong>{coverage.rows.length}</strong>
                </>
              )}
            </div>
            <CoverageTable coverage={coverage} />
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

          {activeProviderJob.status === "unknown" && (
            <div className="unavailable-notice" style={{ marginTop: 8 }}>
              <strong>Unrecognized status.</strong>{" "}
              Daemon returned an unknown status string — check daemon version.
            </div>
          )}
        </Panel>
      )}

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

    </div>
  );
}
