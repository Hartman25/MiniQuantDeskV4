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
  coverageTruthLabel,
  fetchMdBarsCoverage,
  formatEndTs,
  getIngestJob,
  isCoverageActive,
  isTerminalIngestStatus,
  normalizeIngestJobStatus,
  submitIngestJob,
} from "./api.ts";
import { buildRepoRelativePath, buildMd1DSymbolPath, MD_BACKUP_1D_SEGMENTS, MD_INGEST_SEGMENTS } from "../backtests/pathHelpers.ts";
import { getDesktopRepoRoot } from "../../desktop/bootstrap.ts";
import type { ActiveIngestJob, IngestJobStatusKind, MdBarsCoverageResponse } from "./types.ts";

// ---------------------------------------------------------------------------
// Status badge
// ---------------------------------------------------------------------------

function IngestJobStatusBadge({ status }: { status: IngestJobStatusKind }) {
  const label =
    status === "queued" ? "Queued" :
    status === "running" ? "Running…" :
    status === "completed" ? "Completed" :
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

  const pollingRef = useRef<{ cancelled: boolean }>({ cancelled: false });

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

  const jobIsActive = activeJob !== null && !isTerminalIngestStatus(activeJob.status);

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

      {/* Provider ingestion — deferred notice */}
      <Panel
        title="Provider ingestion (deferred)"
        subtitle="TwelveData and other market-data providers are not implemented in this GUI."
      >
        <div className="unavailable-notice" style={{ color: "var(--text-muted, #888)" }}>
          Provider-based ingestion (TwelveData, etc.) is explicitly deferred.
          Only CSV ingestion is available. No provider API credentials are read or API credits consumed by this GUI.
          To ingest from a provider, use the CLI (<code>mqk-cli</code>) directly.
        </div>
      </Panel>

    </div>
  );
}
