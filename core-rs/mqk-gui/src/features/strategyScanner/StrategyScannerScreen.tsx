// STRATEGY-SCANNER-JOBS-GUI-01D: Strategy scanner operator review screen.
//
// Safety invariants:
// - Only calls /api/v1/strategy-scans/* routes. No live/paper execution routes.
// - Submit is privileged (operator token required). Status/artifact reads are public.
// - No provider/broker call. No order/promote/approve control anywhere on this screen.
// - Every result view carries the fixed research-only warning set.

import { useCallback, useEffect, useRef, useState } from "react";
import { Panel } from "../../components/common/Panel";
import { formatDateTime } from "../../lib/format";
import { buildRepoRelativePath } from "../backtests/pathHelpers.ts";
import { getDesktopRepoRoot } from "../../desktop/bootstrap";
import {
  buildActiveStrategyScanJob,
  buildStrategyScanJobRequest,
  getStrategyScanArtifact,
  getStrategyScanJob,
  getStrategyScanReviewArtifact,
  isTerminalStrategyScanJobStatus,
  submitStrategyScanJob,
} from "./api.ts";
import type {
  ActiveStrategyScanJob,
  StrategyScanArtifactResponse,
  StrategyScanJobStatusKind,
  StrategyScanReviewArtifactResponse,
} from "./types.ts";

const REGISTRY_SEGMENTS = ["config", "instruments", "equities.json"];
const BARS_ROOT_SEGMENTS = ["exports", "md_backup"];
const OUT_DIR_SEGMENTS = ["exports", "strategy_scans"];

// ---------------------------------------------------------------------------
// Fixed research-only warnings — always shown when any scan result is displayed.
// Must match the daemon's fixed warning text (routes/strategy_scans.rs).
// ---------------------------------------------------------------------------

const RESEARCH_WARNINGS: string[] = [
  "Scanner ranking is research evidence only.",
  "Scanner output is not autonomous trading approval.",
  "Candidates can rank well while still having negative absolute returns.",
];

function ResearchOnlyWarningBanner() {
  return (
    <div
      className="unavailable-notice"
      style={{ margin: "0 0 4px", borderColor: "var(--accent, #c8a84b)", color: "var(--text)" }}
    >
      <strong>Research review only.</strong>{" "}
      {RESEARCH_WARNINGS.join(" ")}
    </div>
  );
}

// ---------------------------------------------------------------------------
// STRATEGY-SCANNER-PROMOTION-01D: review-artifact warnings — always shown
// when any review artifact result is displayed. `paper_candidate` is a
// research-review label only, never trading approval.
// ---------------------------------------------------------------------------

const REVIEW_WARNINGS: string[] = [
  "promotion-ready is not trading-approved.",
  "paper_candidate is not autonomous trading approval.",
  "A separate paper-promotion patch is required before any paper trading.",
];

function ReviewOnlyWarningBanner() {
  return (
    <div
      className="unavailable-notice"
      style={{ margin: "0 0 4px", borderColor: "var(--accent, #c8a84b)", color: "var(--text)" }}
    >
      <strong>Review queue only.</strong>{" "}
      {REVIEW_WARNINGS.join(" ")}
    </div>
  );
}

function StrategyScanJobStatusBadge({ status }: { status: StrategyScanJobStatusKind }) {
  const label =
    status === "queued" ? "Queued" :
    status === "running" ? "Running…" :
    status === "completed" ? "Completed" :
    status === "failed" ? "Failed" : "Unknown";

  return <span className={`bt-job-status-badge status-${status}`}>{label}</span>;
}

function deriveDefaultRegistryPath(): string {
  return buildRepoRelativePath(getDesktopRepoRoot(), ...REGISTRY_SEGMENTS) ?? "";
}
function deriveDefaultBarsRoot(): string {
  return buildRepoRelativePath(getDesktopRepoRoot(), ...BARS_ROOT_SEGMENTS) ?? "";
}
function deriveDefaultOutDir(): string {
  return buildRepoRelativePath(getDesktopRepoRoot(), ...OUT_DIR_SEGMENTS) ?? "";
}

// ---------------------------------------------------------------------------
// Artifact review panel
// ---------------------------------------------------------------------------

function ArtifactReviewPanel({ artifact }: { artifact: StrategyScanArtifactResponse }) {
  if (artifact.truth_state !== "active") {
    const label =
      artifact.truth_state === "missing_artifact" ? "Artifact directory not found." :
      artifact.truth_state === "invalid_artifact" ? "Artifact files could not be parsed." :
      artifact.truth_state === "path_rejected" ? "Artifact path was rejected (outside the configured scan artifact root)." :
      "Artifact could not be read.";
    return (
      <div className="unavailable-notice unavailable-critical">
        <strong>truth_state: {artifact.truth_state}.</strong> {label}
        {artifact.error ? ` (${artifact.error})` : ""}
      </div>
    );
  }

  return (
    <>
      <div className="timeline-meta-grid" style={{ marginBottom: 8 }}>
        <div>
          <span>Universe</span>
          <strong>{artifact.summary?.universe_count ?? "—"}</strong>
        </div>
        <div>
          <span>Ranked</span>
          <strong>{artifact.summary?.ranked_count ?? "—"}</strong>
        </div>
        <div>
          <span>Skipped</span>
          <strong>{artifact.summary?.skipped_count ?? "—"}</strong>
        </div>
      </div>

      <h4 style={{ margin: "8px 0 4px" }}>Top candidates</h4>
      {artifact.top_candidates.length === 0 ? (
        <div className="unavailable-notice">No ranked candidates in this scan.</div>
      ) : (
        <table className="bt-table" style={{ width: "100%", tableLayout: "auto" }}>
          <thead>
            <tr>
              <th>Rank</th>
              <th>Symbol</th>
              <th>Strategy</th>
              <th>Timeframe</th>
              <th style={{ textAlign: "right" }}>Score</th>
              <th style={{ textAlign: "right" }}>Total return %</th>
              <th style={{ textAlign: "right" }}>Alpha %</th>
              <th style={{ textAlign: "right" }}>Max DD %</th>
              <th style={{ textAlign: "right" }}>Trades</th>
              <th>Truth state</th>
              <th>Reason code</th>
            </tr>
          </thead>
          <tbody>
            {artifact.top_candidates.map((c) => (
              <tr key={`${c.symbol}|${c.timeframe}|${c.strategy_id}`}>
                <td>{c.rank ?? "—"}</td>
                <td><strong>{c.symbol}</strong></td>
                <td>{c.strategy_id}</td>
                <td>{c.timeframe}</td>
                <td style={{ textAlign: "right", fontVariantNumeric: "tabular-nums" }}>
                  {c.score !== null ? c.score.toFixed(2) : "—"}
                </td>
                <td style={{ textAlign: "right", fontVariantNumeric: "tabular-nums" }}>
                  {c.metrics.total_return_pct !== null ? c.metrics.total_return_pct.toFixed(2) : "—"}
                </td>
                <td style={{ textAlign: "right", fontVariantNumeric: "tabular-nums" }}>
                  {c.metrics.alpha_pct !== null ? c.metrics.alpha_pct.toFixed(2) : "—"}
                </td>
                <td style={{ textAlign: "right", fontVariantNumeric: "tabular-nums" }}>
                  {c.metrics.max_drawdown_pct !== null ? c.metrics.max_drawdown_pct.toFixed(2) : "—"}
                </td>
                <td style={{ textAlign: "right", fontVariantNumeric: "tabular-nums" }}>
                  {c.metrics.trade_count ?? "—"}
                </td>
                <td>{c.truth_state}</td>
                <td>{c.reason_code}</td>
              </tr>
            ))}
          </tbody>
        </table>
      )}

      <h4 style={{ margin: "12px 0 4px" }}>Skipped / missing data</h4>
      {artifact.skip_reasons.length === 0 ? (
        <div className="unavailable-notice">No skipped candidates in this scan.</div>
      ) : (
        <ul style={{ margin: 0, paddingLeft: 20 }}>
          {artifact.skip_reasons.map((r) => (
            <li key={r.reason_code}>
              <code>{r.reason_code}</code>: {r.count}
            </li>
          ))}
        </ul>
      )}
    </>
  );
}

// ---------------------------------------------------------------------------
// STRATEGY-SCANNER-PROMOTION-01D: review-artifact display panel.
//
// Display only -- no button on this panel submits, promotes, or approves
// anything. `paper_candidate` means "eligible for a later, separately
// authorized paper-promotion patch to consider", not trading approval.
// ---------------------------------------------------------------------------

function ReviewArtifactPanel({ artifact }: { artifact: StrategyScanReviewArtifactResponse }) {
  if (artifact.truth_state !== "active") {
    const label =
      artifact.truth_state === "missing_artifact" ? "Review artifact directory not found." :
      artifact.truth_state === "invalid_artifact" ? "Review artifact files could not be parsed." :
      artifact.truth_state === "path_rejected" ? "Review artifact path was rejected (outside the configured review artifact root)." :
      "Review artifact could not be read.";
    return (
      <div className="unavailable-notice unavailable-critical">
        <strong>truth_state: {artifact.truth_state}.</strong> {label}
        {artifact.error ? ` (${artifact.error})` : ""}
      </div>
    );
  }

  return (
    <>
      <div className="timeline-meta-grid" style={{ marginBottom: 8 }}>
        <div>
          <span>Candidates</span>
          <strong>{artifact.summary?.candidate_count ?? "—"}</strong>
        </div>
        <div>
          <span>Paper candidate</span>
          <strong>{artifact.summary?.paper_candidate_count ?? "—"}</strong>
        </div>
        <div>
          <span>Watchlist</span>
          <strong>{artifact.summary?.watchlist_candidate_count ?? "—"}</strong>
        </div>
        <div>
          <span>Needs review</span>
          <strong>{artifact.summary?.needs_review_count ?? "—"}</strong>
        </div>
        <div>
          <span>Blocked</span>
          <strong>{artifact.summary?.blocked_count ?? "—"}</strong>
        </div>
        <div>
          <span>Rejected</span>
          <strong>{artifact.summary?.rejected_count ?? "—"}</strong>
        </div>
      </div>

      <h4 style={{ margin: "8px 0 4px" }}>Paper candidates (research review only)</h4>
      {artifact.top_paper_candidates.length === 0 ? (
        <div className="unavailable-notice">No paper_candidate rows in this review.</div>
      ) : (
        <table className="bt-table" style={{ width: "100%", tableLayout: "auto" }}>
          <thead>
            <tr>
              <th>Symbol</th>
              <th>Strategy</th>
              <th>Timeframe</th>
              <th style={{ textAlign: "right" }}>Scanner rank</th>
              <th style={{ textAlign: "right" }}>Scanner score</th>
              <th>Review state</th>
              <th>Reason codes</th>
            </tr>
          </thead>
          <tbody>
            {artifact.top_paper_candidates.map((d) => (
              <tr key={`${d.symbol}|${d.timeframe}|${d.strategy_id}`}>
                <td><strong>{d.symbol}</strong></td>
                <td>{d.strategy_id}</td>
                <td>{d.timeframe}</td>
                <td style={{ textAlign: "right", fontVariantNumeric: "tabular-nums" }}>
                  {d.scanner_rank ?? "—"}
                </td>
                <td style={{ textAlign: "right", fontVariantNumeric: "tabular-nums" }}>
                  {d.scanner_score !== null ? d.scanner_score.toFixed(2) : "—"}
                </td>
                <td>{d.review_state}</td>
                <td>{d.reason_codes.join(", ")}</td>
              </tr>
            ))}
          </tbody>
        </table>
      )}

      <h4 style={{ margin: "12px 0 4px" }}>Watchlist candidates</h4>
      {artifact.top_watchlist_candidates.length === 0 ? (
        <div className="unavailable-notice">No watchlist_candidate rows in this review.</div>
      ) : (
        <table className="bt-table" style={{ width: "100%", tableLayout: "auto" }}>
          <thead>
            <tr>
              <th>Symbol</th>
              <th>Strategy</th>
              <th>Timeframe</th>
              <th>Reason codes</th>
            </tr>
          </thead>
          <tbody>
            {artifact.top_watchlist_candidates.map((d) => (
              <tr key={`${d.symbol}|${d.timeframe}|${d.strategy_id}`}>
                <td><strong>{d.symbol}</strong></td>
                <td>{d.strategy_id}</td>
                <td>{d.timeframe}</td>
                <td>{d.reason_codes.join(", ")}</td>
              </tr>
            ))}
          </tbody>
        </table>
      )}

      {artifact.blockers.length > 0 && (
        <>
          <h4 style={{ margin: "12px 0 4px" }}>Blockers</h4>
          <ul style={{ margin: 0, paddingLeft: 20 }}>
            {artifact.blockers.map((b, i) => (
              <li key={i}>{b}</li>
            ))}
          </ul>
        </>
      )}
    </>
  );
}

// ---------------------------------------------------------------------------
// Main screen component
// ---------------------------------------------------------------------------

export function StrategyScannerScreen() {
  const [registryPath, setRegistryPath] = useState(() => deriveDefaultRegistryPath());
  const [barsRoot, setBarsRoot] = useState(() => deriveDefaultBarsRoot());
  const [timeframe, setTimeframe] = useState("1D");
  const [strategy, setStrategy] = useState("swing_momentum");
  const [top, setTop] = useState("20");
  const [limitSymbols, setLimitSymbols] = useState("");
  const [outDir, setOutDir] = useState(() => deriveDefaultOutDir());

  useEffect(() => {
    const root = getDesktopRepoRoot();
    if (!root) return;
    setRegistryPath((prev) => prev || (buildRepoRelativePath(root, ...REGISTRY_SEGMENTS) ?? ""));
    setBarsRoot((prev) => prev || (buildRepoRelativePath(root, ...BARS_ROOT_SEGMENTS) ?? ""));
    setOutDir((prev) => prev || (buildRepoRelativePath(root, ...OUT_DIR_SEGMENTS) ?? ""));
  }, []);

  const [submitting, setSubmitting] = useState(false);
  const [submitError, setSubmitError] = useState<string | null>(null);
  const [activeJob, setActiveJob] = useState<ActiveStrategyScanJob | null>(null);

  const [artifact, setArtifact] = useState<StrategyScanArtifactResponse | null>(null);
  const [artifactLoading, setArtifactLoading] = useState(false);
  const [artifactError, setArtifactError] = useState<string | null>(null);

  const [reviewDir, setReviewDir] = useState("");
  const [reviewArtifact, setReviewArtifact] = useState<StrategyScanReviewArtifactResponse | null>(null);
  const [reviewArtifactLoading, setReviewArtifactLoading] = useState(false);
  const [reviewArtifactError, setReviewArtifactError] = useState<string | null>(null);

  const handleLoadReviewArtifact = useCallback(async () => {
    const trimmed = reviewDir.trim();
    if (!trimmed) {
      setReviewArtifactError("review_dir is required.");
      return;
    }
    setReviewArtifactLoading(true);
    setReviewArtifactError(null);
    const result = await getStrategyScanReviewArtifact(trimmed);
    setReviewArtifactLoading(false);
    if (!result.ok) {
      setReviewArtifactError(result.error ?? "Review artifact fetch failed.");
      setReviewArtifact(null);
      return;
    }
    setReviewArtifact(result.data ?? null);
  }, [reviewDir]);

  const pollingRef = useRef<{ cancelled: boolean }>({ cancelled: false });

  const loadArtifact = useCallback(async (artifactDir: string) => {
    setArtifactLoading(true);
    setArtifactError(null);
    const result = await getStrategyScanArtifact(artifactDir);
    setArtifactLoading(false);
    if (!result.ok) {
      setArtifactError(result.error ?? "Artifact fetch failed.");
      setArtifact(null);
      return;
    }
    setArtifact(result.data ?? null);
  }, []);

  // Polling: 2-second cadence, stops on terminal status or unmount.
  useEffect(() => {
    if (!activeJob) return;
    if (isTerminalStrategyScanJobStatus(activeJob.status)) return;

    const token = { cancelled: false };
    pollingRef.current = token;

    async function poll() {
      while (!token.cancelled) {
        await new Promise<void>((resolve) => setTimeout(resolve, 2000));
        if (token.cancelled) break;

        const result = await getStrategyScanJob(activeJob!.jobId);
        if (token.cancelled) break;

        if (!result.ok) {
          setActiveJob((prev) =>
            prev?.jobId === activeJob!.jobId
              ? { ...prev, status: "failed", error: result.error ?? "Poll failed." }
              : prev,
          );
          break;
        }

        const updated = buildActiveStrategyScanJob(result.data!);
        setActiveJob((prev) => (prev?.jobId === activeJob!.jobId ? updated : prev));

        if (isTerminalStrategyScanJobStatus(updated.status)) {
          if (updated.status === "completed" && updated.artifactDir) {
            void loadArtifact(updated.artifactDir);
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
  }, [activeJob?.jobId]);

  const handleSubmit = useCallback(async () => {
    const built = buildStrategyScanJobRequest({
      registryPath,
      barsRoot,
      timeframe,
      strategy,
      top,
      limitSymbols,
      outDir,
    });
    if (!built.ok) {
      setSubmitError(built.error);
      return;
    }

    setSubmitting(true);
    setSubmitError(null);
    setActiveJob(null);
    setArtifact(null);
    setArtifactError(null);

    const result = await submitStrategyScanJob(built.request);
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
      status: "queued",
      timeframe,
      strategy,
      createdAt: new Date().toISOString(),
      startedAt: null,
      completedAt: null,
      artifactDir: null,
      summary: null,
      warnings: [],
      error: null,
    });
  }, [registryPath, barsRoot, timeframe, strategy, top, limitSymbols, outDir]);

  const jobIsActive = activeJob !== null && !isTerminalStrategyScanJobStatus(activeJob.status);

  return (
    <div className="screen-grid desk-screen-grid">
      <ResearchOnlyWarningBanner />

      <Panel
        title="Submit local-data strategy scan"
        subtitle="Bounded local-data scan. No provider/broker call. No live or paper order. Submission requires the operator token."
      >
        <div className="bt-job-form-grid">
          <div className="bt-job-field" style={{ gridColumn: "1 / -1" }}>
            <label htmlFor="scan-registry-path">Registry path</label>
            <input
              id="scan-registry-path"
              type="text"
              value={registryPath}
              onChange={(e) => setRegistryPath(e.target.value)}
              spellCheck={false}
              autoComplete="off"
              placeholder="config/instruments/equities.json"
            />
          </div>

          <div className="bt-job-field" style={{ gridColumn: "1 / -1" }}>
            <label htmlFor="scan-bars-root">Bars root</label>
            <input
              id="scan-bars-root"
              type="text"
              value={barsRoot}
              onChange={(e) => setBarsRoot(e.target.value)}
              spellCheck={false}
              autoComplete="off"
              placeholder="exports/md_backup"
            />
          </div>

          <div className="bt-job-field">
            <label htmlFor="scan-timeframe">Timeframe</label>
            <input
              id="scan-timeframe"
              type="text"
              value={timeframe}
              onChange={(e) => setTimeframe(e.target.value)}
              spellCheck={false}
              autoComplete="off"
              placeholder="1D | 5m | 1H"
            />
          </div>

          <div className="bt-job-field">
            <label htmlFor="scan-strategy">Strategy</label>
            <input
              id="scan-strategy"
              type="text"
              value={strategy}
              onChange={(e) => setStrategy(e.target.value)}
              spellCheck={false}
              autoComplete="off"
              placeholder="swing_momentum"
            />
          </div>

          <div className="bt-job-field">
            <label htmlFor="scan-top">Top N</label>
            <input
              id="scan-top"
              type="text"
              value={top}
              onChange={(e) => setTop(e.target.value)}
              spellCheck={false}
              autoComplete="off"
              placeholder="20 (max 100)"
            />
          </div>

          <div className="bt-job-field">
            <label htmlFor="scan-limit-symbols">Limit symbols</label>
            <input
              id="scan-limit-symbols"
              type="text"
              value={limitSymbols}
              onChange={(e) => setLimitSymbols(e.target.value)}
              spellCheck={false}
              autoComplete="off"
              placeholder="blank = full registry universe (max 200)"
            />
          </div>

          <div className="bt-job-field" style={{ gridColumn: "1 / -1" }}>
            <label htmlFor="scan-out-dir">Output directory</label>
            <input
              id="scan-out-dir"
              type="text"
              value={outDir}
              onChange={(e) => setOutDir(e.target.value)}
              spellCheck={false}
              autoComplete="off"
              placeholder="exports/strategy_scans"
            />
          </div>
        </div>

        <div className="bt-path-row" style={{ marginTop: 8 }}>
          <button
            type="button"
            className="action-button"
            onClick={() => void handleSubmit()}
            disabled={submitting || jobIsActive}
          >
            {submitting ? "Submitting…" : jobIsActive ? "Scan running…" : "Submit scan job"}
          </button>
        </div>

        <div className="bt-field-hint" style={{ marginTop: 8, fontSize: "0.79rem", color: "var(--text-muted, #888)" }}>
          Submission requires the daemon to be running with <code>MQK_OPERATOR_TOKEN</code> configured.
          Job history is daemon-lifetime only — a daemon restart clears the job list (the artifact files
          on disk are unaffected).
        </div>

        {submitError && (
          <div className="unavailable-notice unavailable-critical" style={{ marginTop: 10 }}>
            <strong>Submit failed:</strong> {submitError}
          </div>
        )}
      </Panel>

      {activeJob && (
        <Panel
          title="Job status"
          subtitle="Local-data scan job — writes a scan artifact directory only. No broker adapter. No orders."
        >
          <div className="bt-job-status-row">
            <StrategyScanJobStatusBadge status={activeJob.status} />
            <span className="bt-job-meta" title={activeJob.jobId}>
              job {activeJob.jobId.slice(0, 8)}…
            </span>
            <span className="bt-job-meta">
              {activeJob.timeframe} / {activeJob.strategy}
            </span>
            {activeJob.completedAt && (
              <span className="bt-job-meta">completed {formatDateTime(activeJob.completedAt)}</span>
            )}
          </div>

          {(activeJob.status === "queued" || activeJob.status === "running") && (
            <div className="bt-job-meta" style={{ marginTop: 6, color: "var(--accent)" }}>
              Polling for status every 2 seconds…
            </div>
          )}

          {activeJob.status === "completed" && activeJob.artifactDir && (
            <div className="bt-job-meta" style={{ marginTop: 6, wordBreak: "break-all" }}>
              <span className="eyebrow">artifact directory</span>{" "}
              <span style={{ color: "var(--text)" }}>{activeJob.artifactDir}</span>
            </div>
          )}

          {activeJob.status === "failed" && (
            <div className="unavailable-notice unavailable-critical" style={{ marginTop: 8 }}>
              <strong>Job failed:</strong> {activeJob.error ?? "Daemon reported a failure with no message."}
            </div>
          )}

          {activeJob.status === "unknown" && (
            <div className="unavailable-notice" style={{ marginTop: 8 }}>
              <strong>Unrecognized status.</strong> Daemon returned an unknown status string — check daemon version.
            </div>
          )}
        </Panel>
      )}

      {(artifactLoading || artifact || artifactError) && (
        <Panel
          title="Scan artifact review"
          subtitle="Ranked candidates and skip reasons read directly from the scan's artifact directory."
        >
          <ResearchOnlyWarningBanner />
          {artifactLoading && <div className="bt-job-meta">Loading artifact…</div>}
          {artifactError && (
            <div className="unavailable-notice unavailable-critical">
              <strong>Artifact load failed:</strong> {artifactError}
            </div>
          )}
          {artifact && <ArtifactReviewPanel artifact={artifact} />}
        </Panel>
      )}

      <Panel
        title="Research-review queue (promotion evidence)"
        subtitle="Loads a review artifact written by 'mqk backtest review-scan'. Display only -- no promote/approve/trade action anywhere on this panel."
      >
        <ReviewOnlyWarningBanner />
        <div className="bt-job-form-grid">
          <div className="bt-job-field" style={{ gridColumn: "1 / -1" }}>
            <label htmlFor="review-dir">Review artifact directory</label>
            <input
              id="review-dir"
              type="text"
              value={reviewDir}
              onChange={(e) => setReviewDir(e.target.value)}
              spellCheck={false}
              autoComplete="off"
              placeholder="exports/strategy_reviews/<review_id>"
            />
          </div>
        </div>
        <div className="bt-path-row" style={{ marginTop: 8 }}>
          <button
            type="button"
            className="action-button"
            onClick={() => void handleLoadReviewArtifact()}
            disabled={reviewArtifactLoading}
          >
            {reviewArtifactLoading ? "Loading…" : "Load review artifact"}
          </button>
        </div>

        {reviewArtifactError && (
          <div className="unavailable-notice unavailable-critical" style={{ marginTop: 10 }}>
            <strong>Review artifact load failed:</strong> {reviewArtifactError}
          </div>
        )}
        {reviewArtifact && (
          <div style={{ marginTop: 10 }}>
            <ReviewArtifactPanel artifact={reviewArtifact} />
          </div>
        )}
      </Panel>
    </div>
  );
}
