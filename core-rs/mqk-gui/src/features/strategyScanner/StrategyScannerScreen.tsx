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
  getStrategyPromotionCheck,
  getStrategyPromotionHistory,
  getStrategyScanArtifact,
  getStrategyScanJob,
  getStrategyScanReviewArtifact,
  isTerminalStrategyScanJobStatus,
  submitStrategyPromotionTransition,
  submitStrategyScanJob,
} from "./api.ts";
import type {
  ActiveStrategyScanJob,
  PromotionStateKind,
  StrategyPromotionCheckResponse,
  StrategyPromotionRow,
  StrategyScanArtifactResponse,
  StrategyScanJobStatusKind,
  StrategyScanReviewArtifactResponse,
  StrategyScanReviewDecision,
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
//
// STRATEGY-PROMOTION-REGISTRY-01E: an optional `onSelectPaperCandidate`
// callback lets the operator load one `paper_candidate` row's identity into
// the promotion control panel below. Selecting a row never submits,
// promotes, or approves anything by itself -- it only pre-fills the
// initial-approval form, which still requires an explicit separate submit.
// ---------------------------------------------------------------------------

function ReviewArtifactPanel({
  artifact,
  reviewDir,
  onSelectPaperCandidate,
}: {
  artifact: StrategyScanReviewArtifactResponse;
  reviewDir: string;
  onSelectPaperCandidate?: (decision: StrategyScanReviewDecision, reviewDir: string) => void;
}) {
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
              {onSelectPaperCandidate && <th>Promotion</th>}
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
                {onSelectPaperCandidate && (
                  <td>
                    <button
                      type="button"
                      className="action-button"
                      onClick={() => onSelectPaperCandidate(d, reviewDir)}
                    >
                      Select for promotion
                    </button>
                  </td>
                )}
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
// STRATEGY-PROMOTION-REGISTRY-01E: strategy promotion control panel.
//
// Safety invariants:
// - Only calls /api/v1/strategy/promotions* routes. No order/broker/
//   run-start/arm route is called or referenced anywhere in this panel.
// - The mutation route (transition) requires the operator token, same as
//   the scan-job submit control above.
// - Initial approval (target_state = shadow_approved from no prior state)
//   is disabled until a paper_candidate row has been selected from the
//   review-artifact panel above and its identity matches the form fields.
// - active_paper / demoted / retired transitions require an explicit
//   confirmation checkbox before Submit is enabled -- no single click can
//   activate, demote, or retire a strategy.
// - tradable_live is always displayed as false, with a standing warning
//   that paper promotion never grants live authorization.
// ---------------------------------------------------------------------------

const PROMOTION_LIVE_BOUNDARY_WARNING =
  "Paper promotion never grants live authorization. tradable_live is always false on every route in this panel.";

/**
 * Client-side display convenience ONLY -- mirrors the daemon's own
 * `scanner_timeframe_label_to_secs` table (mqk-db/src/strategy_promotion.rs)
 * so the form can pre-fill a numeric `timeframe_secs` when a review row is
 * selected. The daemon route independently re-validates and is the only
 * authoritative source -- this panel never trusts its own conversion.
 */
function timeframeLabelToSecsForDisplay(label: string): number | null {
  switch (label.trim()) {
    case "1m": return 60;
    case "5m": return 300;
    case "15m": return 900;
    case "30m": return 1800;
    case "1H": return 3600;
    case "1D": return 86400;
    default: return null;
  }
}

const CONFIRM_REQUIRED_STATES: PromotionStateKind[] = ["active_paper", "demoted", "retired"];

function promotionStateBadgeLabel(state: string | null): string {
  if (state === null) return "no record";
  return state;
}

function PromotionCheckResult({ result }: { result: StrategyPromotionCheckResponse }) {
  return (
    <div style={{ marginTop: 10 }}>
      <div className="timeline-meta-grid" style={{ marginBottom: 8 }}>
        <div>
          <span>Current state</span>
          <strong>{promotionStateBadgeLabel(result.current_state)}</strong>
        </div>
        <div>
          <span>Paper tradable</span>
          <strong>{result.tradable_paper ? "yes" : "no"}</strong>
        </div>
        <div>
          <span>Live tradable</span>
          <strong style={{ color: "var(--danger, #c0392b)" }}>{result.tradable_live ? "yes" : "no"}</strong>
        </div>
        <div>
          <span>Reason code</span>
          <strong>{result.reason_code}</strong>
        </div>
        <div>
          <span>Config identity</span>
          <strong>{result.config_identity_status ?? "—"}</strong>
        </div>
        <div>
          <span>truth_state</span>
          <strong>{result.truth_state}</strong>
        </div>
      </div>
      {result.blockers.length > 0 && (
        <ul style={{ margin: 0, paddingLeft: 20 }}>
          {result.blockers.map((b, i) => (
            <li key={i}>{b}</li>
          ))}
        </ul>
      )}
    </div>
  );
}

function PromotionHistoryTable({ rows }: { rows: StrategyPromotionRow[] }) {
  if (rows.length === 0) {
    return <div className="unavailable-notice">No transitions recorded for this identity.</div>;
  }
  return (
    <table className="bt-table" style={{ width: "100%", tableLayout: "auto", marginTop: 8 }}>
      <thead>
        <tr>
          <th>Effective</th>
          <th>Previous state</th>
          <th>New state</th>
          <th>Expires</th>
          <th>Evidence review_id</th>
          <th>Initiated by</th>
          <th>Reason</th>
        </tr>
      </thead>
      <tbody>
        {rows.map((r) => (
          <tr key={r.transition_id}>
            <td>{formatDateTime(r.effective_at_utc)}</td>
            <td>{r.previous_state ?? "—"}</td>
            <td><strong>{r.new_state}</strong></td>
            <td>{r.expires_at_utc ? formatDateTime(r.expires_at_utc) : "—"}</td>
            <td style={{ wordBreak: "break-all" }}>{r.evidence_review_id ?? "—"}</td>
            <td>{r.initiated_by}</td>
            <td>{r.reason || "—"}</td>
          </tr>
        ))}
      </tbody>
    </table>
  );
}

interface SelectedPaperCandidate {
  decision: StrategyScanReviewDecision;
  reviewDir: string;
}

function PromotionControlPanel({ selection }: { selection: SelectedPaperCandidate | null }) {
  const [strategyId, setStrategyId] = useState("");
  const [symbol, setSymbol] = useState("");
  const [timeframeSecs, setTimeframeSecs] = useState("");

  const [checkResult, setCheckResult] = useState<StrategyPromotionCheckResponse | null>(null);
  const [checkLoading, setCheckLoading] = useState(false);
  const [checkError, setCheckError] = useState<string | null>(null);

  const [historyRows, setHistoryRows] = useState<StrategyPromotionRow[] | null>(null);
  const [historyLoading, setHistoryLoading] = useState(false);
  const [historyError, setHistoryError] = useState<string | null>(null);

  const [targetState, setTargetState] = useState<PromotionStateKind>("shadow_approved");
  const [reviewDirInput, setReviewDirInput] = useState("");
  const [initiatedBy, setInitiatedBy] = useState("");
  const [reason, setReason] = useState("");
  const [confirmed, setConfirmed] = useState(false);

  const [submitting, setSubmitting] = useState(false);
  const [submitError, setSubmitError] = useState<string | null>(null);
  const [submitResult, setSubmitResult] = useState<{
    accepted: boolean;
    disposition: string;
    blockers: string[];
    transitionId: string | null;
  } | null>(null);

  // Selecting a paper_candidate row in the review-artifact panel above
  // updates the `selection` prop (owned by the parent screen); this panel
  // reacts by pre-filling its identity fields. Selecting a row never
  // submits, promotes, or approves anything by itself.
  useEffect(() => {
    if (!selection) return;
    setStrategyId(selection.decision.strategy_id);
    setSymbol(selection.decision.symbol);
    const secs = timeframeLabelToSecsForDisplay(selection.decision.timeframe);
    setTimeframeSecs(secs !== null ? String(secs) : "");
    setReviewDirInput(selection.reviewDir);
    setTargetState("shadow_approved");
    setConfirmed(false);
    setCheckResult(null);
    setHistoryRows(null);
    setSubmitResult(null);
    setSubmitError(null);
  }, [selection]);

  const parsedTimeframe = Number(timeframeSecs.trim());
  const identityValid =
    strategyId.trim() !== "" && symbol.trim() !== "" && Number.isInteger(parsedTimeframe) && parsedTimeframe > 0;

  const identityMatchesSelection =
    selection !== null &&
    selection.decision.strategy_id === strategyId.trim() &&
    selection.decision.symbol === symbol.trim() &&
    timeframeLabelToSecsForDisplay(selection.decision.timeframe) === parsedTimeframe &&
    selection.decision.review_state === "paper_candidate";

  const isInitialApproval = targetState === "shadow_approved" && checkResult?.current_state == null;
  const initialApprovalBlocked = isInitialApproval && !identityMatchesSelection;

  const requiresConfirmation = CONFIRM_REQUIRED_STATES.includes(targetState);
  const canSubmit =
    identityValid &&
    initiatedBy.trim() !== "" &&
    !initialApprovalBlocked &&
    (!requiresConfirmation || confirmed) &&
    !submitting;

  const handleCheck = useCallback(async () => {
    if (!identityValid) {
      setCheckError("strategy_id, symbol, and a positive timeframe_secs are required.");
      return;
    }
    setCheckLoading(true);
    setCheckError(null);
    const result = await getStrategyPromotionCheck(strategyId, symbol, parsedTimeframe);
    setCheckLoading(false);
    if (!result.ok) {
      setCheckError(result.error ?? "Promotion check failed.");
      setCheckResult(null);
      return;
    }
    setCheckResult(result.data ?? null);
  }, [strategyId, symbol, parsedTimeframe, identityValid]);

  const handleHistory = useCallback(async () => {
    if (!identityValid) {
      setHistoryError("strategy_id, symbol, and a positive timeframe_secs are required.");
      return;
    }
    setHistoryLoading(true);
    setHistoryError(null);
    const result = await getStrategyPromotionHistory(strategyId, symbol, parsedTimeframe);
    setHistoryLoading(false);
    if (!result.ok) {
      setHistoryError(result.error ?? "Promotion history fetch failed.");
      setHistoryRows(null);
      return;
    }
    setHistoryRows(result.data?.rows ?? []);
  }, [strategyId, symbol, parsedTimeframe, identityValid]);

  const handleSubmitTransition = useCallback(async () => {
    if (!canSubmit) return;
    setSubmitting(true);
    setSubmitError(null);
    setSubmitResult(null);

    const result = await submitStrategyPromotionTransition({
      strategy_id: strategyId.trim(),
      symbol: symbol.trim(),
      timeframe_secs: parsedTimeframe,
      target_state: targetState,
      review_dir: reviewDirInput.trim() || null,
      effective_at_utc: new Date().toISOString(),
      expires_at_utc: null,
      initiated_by: initiatedBy.trim(),
      reason: reason.trim(),
    });
    setSubmitting(false);

    if (!result.ok) {
      setSubmitError(result.error ?? "Transition request failed.");
      return;
    }
    const data = result.data!;
    setSubmitResult({
      accepted: data.accepted,
      disposition: data.disposition,
      blockers: data.blockers,
      transitionId: data.transition_id,
    });
    setConfirmed(false);
    // Refresh check + history to reflect the new durable state.
    void handleCheck();
    void handleHistory();
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [canSubmit, strategyId, symbol, parsedTimeframe, targetState, reviewDirInput, initiatedBy, reason]);

  return (
    <Panel
      title="Strategy promotion control"
      subtitle="Operator-authenticated paper-promotion transitions. registered+enabled is never sufficient for paper trading -- only an active_paper promotion is."
    >
      <div
        className="unavailable-notice unavailable-critical"
        style={{ margin: "0 0 10px" }}
      >
        <strong>{PROMOTION_LIVE_BOUNDARY_WARNING}</strong>
      </div>

      {selection && (
        <div className="bt-job-meta" style={{ marginBottom: 8 }}>
          Selected from review artifact: <strong>{selection.decision.symbol}</strong> /{" "}
          {selection.decision.strategy_id} / {selection.decision.timeframe} (
          {selection.decision.review_state})
        </div>
      )}

      <div className="bt-job-form-grid">
        <div className="bt-job-field">
          <label htmlFor="promo-strategy-id">Strategy ID</label>
          <input
            id="promo-strategy-id"
            type="text"
            value={strategyId}
            onChange={(e) => setStrategyId(e.target.value)}
            spellCheck={false}
            autoComplete="off"
          />
        </div>
        <div className="bt-job-field">
          <label htmlFor="promo-symbol">Symbol</label>
          <input
            id="promo-symbol"
            type="text"
            value={symbol}
            onChange={(e) => setSymbol(e.target.value.toUpperCase())}
            spellCheck={false}
            autoComplete="off"
          />
        </div>
        <div className="bt-job-field">
          <label htmlFor="promo-timeframe-secs">Timeframe (seconds)</label>
          <input
            id="promo-timeframe-secs"
            type="text"
            value={timeframeSecs}
            onChange={(e) => setTimeframeSecs(e.target.value)}
            spellCheck={false}
            autoComplete="off"
            placeholder="86400 = 1D, 3600 = 1H, 300 = 5m"
          />
        </div>
      </div>

      <div className="bt-path-row" style={{ marginTop: 8, gap: 8 }}>
        <button type="button" className="action-button" onClick={() => void handleCheck()} disabled={!identityValid || checkLoading}>
          {checkLoading ? "Checking…" : "Check current state"}
        </button>
        <button type="button" className="action-button" onClick={() => void handleHistory()} disabled={!identityValid || historyLoading}>
          {historyLoading ? "Loading…" : "Load history"}
        </button>
      </div>

      {checkError && (
        <div className="unavailable-notice unavailable-critical" style={{ marginTop: 8 }}>
          {checkError}
        </div>
      )}
      {checkResult && <PromotionCheckResult result={checkResult} />}

      {historyError && (
        <div className="unavailable-notice unavailable-critical" style={{ marginTop: 8 }}>
          {historyError}
        </div>
      )}
      {historyRows && <PromotionHistoryTable rows={historyRows} />}

      <h4 style={{ margin: "14px 0 4px" }}>Request a transition</h4>
      <div className="bt-job-form-grid">
        <div className="bt-job-field">
          <label htmlFor="promo-target-state">Target state</label>
          <select
            id="promo-target-state"
            value={targetState}
            onChange={(e) => {
              setTargetState(e.target.value as PromotionStateKind);
              setConfirmed(false);
            }}
          >
            <option value="shadow_approved">shadow_approved</option>
            <option value="paper_approved">paper_approved</option>
            <option value="active_paper">active_paper</option>
            <option value="demoted">demoted</option>
            <option value="retired">retired</option>
            <option value="rejected">rejected</option>
          </select>
        </div>
        <div className="bt-job-field" style={{ gridColumn: "1 / -1" }}>
          <label htmlFor="promo-review-dir">Review artifact directory (required for shadow_approved)</label>
          <input
            id="promo-review-dir"
            type="text"
            value={reviewDirInput}
            onChange={(e) => setReviewDirInput(e.target.value)}
            spellCheck={false}
            autoComplete="off"
            placeholder="exports/strategy_reviews/<review_id>"
          />
        </div>
        <div className="bt-job-field">
          <label htmlFor="promo-initiated-by">Initiated by (operator)</label>
          <input
            id="promo-initiated-by"
            type="text"
            value={initiatedBy}
            onChange={(e) => setInitiatedBy(e.target.value)}
            spellCheck={false}
            autoComplete="off"
          />
        </div>
        <div className="bt-job-field" style={{ gridColumn: "1 / -1" }}>
          <label htmlFor="promo-reason">Reason</label>
          <input
            id="promo-reason"
            type="text"
            value={reason}
            onChange={(e) => setReason(e.target.value)}
            spellCheck={false}
            autoComplete="off"
          />
        </div>
      </div>

      {isInitialApproval && (
        <div className="unavailable-notice" style={{ marginTop: 8 }}>
          Initial approval (no prior state → shadow_approved) requires a <code>paper_candidate</code> row
          selected from the review-artifact panel above, matching this exact identity.
          {initialApprovalBlocked && (
            <strong> No matching selection loaded — Submit is disabled.</strong>
          )}
        </div>
      )}

      {requiresConfirmation && (
        <label className="bt-job-meta" style={{ display: "flex", alignItems: "center", gap: 6, marginTop: 8 }}>
          <input type="checkbox" checked={confirmed} onChange={(e) => setConfirmed(e.target.checked)} />
          I confirm this {targetState} transition. This does not authorize live trading.
        </label>
      )}

      <div className="bt-path-row" style={{ marginTop: 8 }}>
        <button
          type="button"
          className="action-button"
          onClick={() => void handleSubmitTransition()}
          disabled={!canSubmit}
        >
          {submitting ? "Submitting…" : `Submit ${targetState} transition`}
        </button>
      </div>

      <div className="bt-field-hint" style={{ marginTop: 8, fontSize: "0.79rem", color: "var(--text-muted, #888)" }}>
        Submission requires the daemon to be running with <code>MQK_OPERATOR_TOKEN</code> configured.
        This panel never calls an order, broker, run-start, or arm route.
      </div>

      {submitError && (
        <div className="unavailable-notice unavailable-critical" style={{ marginTop: 10 }}>
          <strong>Transition failed:</strong> {submitError}
        </div>
      )}
      {submitResult && (
        <div
          className={submitResult.accepted ? "unavailable-notice" : "unavailable-notice unavailable-critical"}
          style={{ marginTop: 10 }}
        >
          <strong>disposition: {submitResult.disposition}.</strong>{" "}
          {submitResult.transitionId && <span>transition_id: {submitResult.transitionId}. </span>}
          {submitResult.blockers.length > 0 && (
            <ul style={{ margin: "6px 0 0", paddingLeft: 20 }}>
              {submitResult.blockers.map((b, i) => (
                <li key={i}>{b}</li>
              ))}
            </ul>
          )}
        </div>
      )}
    </Panel>
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

  // STRATEGY-PROMOTION-REGISTRY-01E: which paper_candidate row (if any) was
  // last selected for promotion from the review-artifact panel below.
  // Owned here (not inside PromotionControlPanel) so the review panel's
  // "Select for promotion" button can update it directly.
  const [selectedPaperCandidate, setSelectedPaperCandidate] = useState<SelectedPaperCandidate | null>(null);
  const handleSelectPaperCandidate = useCallback(
    (decision: StrategyScanReviewDecision, reviewDirForSelection: string) => {
      setSelectedPaperCandidate({ decision, reviewDir: reviewDirForSelection });
    },
    [],
  );

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
            <ReviewArtifactPanel
              artifact={reviewArtifact}
              reviewDir={reviewDir.trim()}
              onSelectPaperCandidate={handleSelectPaperCandidate}
            />
          </div>
        )}
      </Panel>

      <PromotionControlPanel selection={selectedPaperCandidate} />
    </div>
  );
}
