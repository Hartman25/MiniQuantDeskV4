// DAILY-DATA-READINESS-01D-GUI-01: Bounded, read-only Daily Data Readiness
// panel.
//
// Safety invariants:
// - Renders only the supplied GET /api/v1/market-data/readiness response.
// - No ingest submission, provider poll, scheduler, or runtime import or
//   call lives in this file. Refresh and copy-diagnostics are the only
//   operator controls.
// - Never renders "ready" for a response that has not proven every
//   assignment ready — see classifyDailyDataReadinessDisplay.

import { useState } from "react";
import { Panel } from "../../components/common/Panel";
import {
  buildDailyDataReadinessDiagnosticText,
  classifyDailyDataReadinessDisplay,
  formatReadinessNumber,
  formatUnixSecondsDateTime,
  type DailyDataReadinessDisplayState,
} from "./api.ts";
import type { DailyDataReadinessAssignmentResponse, DailyDataReadinessResponse } from "./types.ts";

function displayStateLabel(state: DailyDataReadinessDisplayState): string {
  switch (state) {
    case "ready":
      return "Ready";
    case "blocked":
      return "Blocked";
    case "not_applicable":
      return "Not applicable";
    case "unknown":
    default:
      return "Unknown";
  }
}

function displayStateColor(state: DailyDataReadinessDisplayState): string {
  switch (state) {
    case "ready":
      return "var(--success, #48c78e)";
    case "blocked":
      return "var(--critical, #ff3860)";
    case "not_applicable":
      return "var(--text-muted, #888)";
    case "unknown":
    default:
      return "var(--accent, #f5a623)";
  }
}

function shortUtc(value: string | null | undefined): string {
  if (!value) return "unknown";
  return value.slice(0, 19).replace("T", " ");
}

function StringListBlock({ label, values, emptyText }: { label: string; values: string[]; emptyText: string }) {
  return (
    <div style={{ marginTop: 6 }}>
      <span className="eyebrow">{label}</span>
      {values.length === 0 ? (
        <div style={{ color: "var(--text-muted, #888)", fontSize: "0.85rem" }}>{emptyText}</div>
      ) : (
        <ul style={{ margin: "4px 0 0", paddingLeft: 18, fontSize: "0.85rem" }}>
          {values.map((v, i) => (
            <li key={`${label}-${i}`}>{v}</li>
          ))}
        </ul>
      )}
    </div>
  );
}

function AssignmentBlock({ assignment }: { assignment: DailyDataReadinessAssignmentResponse }) {
  const providerMismatch =
    assignment.expected_provider_id !== null &&
    !assignment.actual_provider_ids.includes(assignment.expected_provider_id);

  return (
    <div
      className="unavailable-notice"
      style={{ marginTop: 10, borderColor: "var(--border, rgba(255,255,255,0.12))" }}
    >
      <div className="bt-job-meta" style={{ marginBottom: 6 }}>
        <span className="eyebrow">symbol</span>{" "}
        <strong>{assignment.assignment_symbol}</strong>{" "}
        <span className="eyebrow">timeframe</span>{" "}
        <strong>{assignment.assignment_timeframe}</strong>{" "}
        <span className="eyebrow">readiness_state</span>{" "}
        <strong>{assignment.readiness_state}</strong>
      </div>

      <div className="timeline-meta-grid" style={{ marginBottom: 6 }}>
        <div>
          <span>Configured strategy</span>
          <strong>{assignment.configured_strategy_id}</strong>
        </div>
        <div>
          <span>Effective strategy</span>
          <strong>{assignment.effective_runtime_strategy_id ?? "unknown"}</strong>
        </div>
        <div>
          <span>Effective target symbol</span>
          <strong>{assignment.effective_runtime_target_symbol ?? "unknown"}</strong>
        </div>
        <div>
          <span>Effective timeframe secs</span>
          <strong>{formatReadinessNumber(assignment.effective_runtime_timeframe_secs, "s")}</strong>
        </div>
        <div>
          <span>Required history bars</span>
          <strong>{formatReadinessNumber(assignment.required_history_bars)}</strong>
        </div>
        <div>
          <span>Loaded completed bars</span>
          <strong>{formatReadinessNumber(assignment.loaded_completed_bars)}</strong>
        </div>
        <div>
          <span>Asset class</span>
          <strong>{assignment.asset_class ?? "unknown"}</strong>
        </div>
        <div>
          <span>Expected latest bar</span>
          <strong>{formatUnixSecondsDateTime(assignment.expected_latest_bar_ts)}</strong>
        </div>
        <div>
          <span>Actual latest bar</span>
          <strong>{formatUnixSecondsDateTime(assignment.actual_latest_bar_ts)}</strong>
        </div>
        <div>
          <span>Continuity state</span>
          <strong>{assignment.continuity_state}</strong>
        </div>
        <div>
          <span>Provenance state</span>
          <strong>{assignment.provenance_state}</strong>
        </div>
        <div>
          <span>Effective grace secs</span>
          <strong>{formatReadinessNumber(assignment.effective_grace_seconds, "s")}</strong>
        </div>
        <div>
          <span>Effective future skew secs</span>
          <strong>{formatReadinessNumber(assignment.effective_future_skew_seconds, "s")}</strong>
        </div>
      </div>

      <div
        className="bt-job-meta"
        style={{
          marginBottom: 6,
          color: providerMismatch ? "var(--critical, #ff3860)" : undefined,
        }}
      >
        <span className="eyebrow">expected provider</span>{" "}
        <strong>
          {assignment.expected_provider_id ?? "unknown"}
          {assignment.expected_provider_symbol ? ` / ${assignment.expected_provider_symbol}` : ""}
        </strong>
        {" "}
        <span className="eyebrow">actual provider(s)</span>{" "}
        <strong>
          {assignment.actual_provider_ids.length > 0
            ? assignment.actual_provider_ids.join(", ")
            : "none observed"}
          {assignment.actual_provider_symbols.length > 0
            ? ` / ${assignment.actual_provider_symbols.join(", ")}`
            : ""}
        </strong>
      </div>

      <StringListBlock label="Blockers" values={assignment.blockers} emptyText="none" />
      <StringListBlock label="Remediation" values={assignment.remediation} emptyText="none" />
    </div>
  );
}

export function DailyDataReadinessPanel({
  response,
  loading,
  error,
  onRefresh,
}: {
  response: DailyDataReadinessResponse | null;
  loading: boolean;
  error: string | null;
  onRefresh: () => void;
}) {
  const [copyNotice, setCopyNotice] = useState<string | null>(null);
  const [copyError, setCopyError] = useState<string | null>(null);

  const overall = classifyDailyDataReadinessDisplay(response, error);

  const handleCopy = async () => {
    if (!response) return;
    setCopyError(null);
    setCopyNotice(null);
    const text = buildDailyDataReadinessDiagnosticText(response);
    try {
      await navigator.clipboard.writeText(text);
      setCopyNotice("Diagnostics copied to clipboard.");
    } catch (e) {
      setCopyError(e instanceof Error ? e.message : "Clipboard copy failed.");
    }
  };

  return (
    <Panel
      title="Daily data readiness"
      subtitle="Read-only projection of GET /api/v1/market-data/readiness. No ingest, no provider poll, no scheduler, no runtime start."
    >
      <div style={{ display: "flex", alignItems: "center", gap: 8, marginBottom: 10, flexWrap: "wrap" }}>
        <button
          type="button"
          className="action-button"
          onClick={onRefresh}
          disabled={loading}
          style={{ padding: "2px 12px" }}
        >
          {loading ? "Loading…" : "Refresh"}
        </button>
        <button
          type="button"
          className="action-button ghost"
          onClick={() => void handleCopy()}
          disabled={!response}
          style={{ padding: "2px 12px" }}
        >
          Copy diagnostics
        </button>
        <span
          style={{
            fontSize: "0.78rem",
            fontWeight: 600,
            letterSpacing: "0.05em",
            color: displayStateColor(overall),
          }}
        >
          {displayStateLabel(overall)}
        </span>
      </div>

      {copyNotice && (
        <div className="unavailable-notice" style={{ marginBottom: 8 }}>
          {copyNotice}
        </div>
      )}
      {copyError && (
        <div className="unavailable-notice unavailable-critical" style={{ marginBottom: 8 }}>
          <strong>Copy failed:</strong> {copyError}
        </div>
      )}

      {loading && response === null && (
        <div className="bt-job-meta" style={{ color: "var(--accent)" }}>
          Loading daily data readiness…
        </div>
      )}

      {error && (
        <div className="unavailable-notice unavailable-critical" style={{ marginBottom: 8 }}>
          <strong>Daily data readiness unavailable:</strong> {error}
        </div>
      )}

      {!loading && !error && response === null && (
        <div className="unavailable-notice" style={{ color: "var(--text-muted, #888)" }}>
          Not loaded yet. Click Refresh or wait for auto-load.
        </div>
      )}

      {response !== null && (
        <>
          {response.binding_scope === "configuration_preview" && (
            <div className="unavailable-notice" style={{ marginBottom: 10, color: "var(--text-muted, #888)" }}>
              This is a configuration preview. Runtime start re-evaluates readiness using the exact
              start-attempt binding.
            </div>
          )}

          <div className="timeline-meta-grid" style={{ marginBottom: 8 }}>
            <div>
              <span>Last evaluated</span>
              <strong>{shortUtc(response.evaluated_at_utc)}</strong>
            </div>
            <div>
              <span>Binding scope</span>
              <strong>{response.binding_scope}</strong>
            </div>
            <div>
              <span>Assignment source</span>
              <strong>{response.assignment_source}</strong>
            </div>
            <div>
              <span>Applicability</span>
              <strong>{response.applicability}</strong>
            </div>
            <div>
              <span>Top-level blocker</span>
              <strong>{response.top_level_blocker ?? "none"}</strong>
            </div>
            <div>
              <span>Calendar source</span>
              <strong>{response.calendar_source ?? "unknown"}</strong>
            </div>
            <div>
              <span>Calendar coverage state</span>
              <strong>{response.calendar_coverage_state}</strong>
            </div>
            <div>
              <span>Market date</span>
              <strong>{response.market_date ?? "unknown"}</strong>
            </div>
            <div>
              <span>Session open</span>
              <strong>{shortUtc(response.session_open_utc)}</strong>
            </div>
            <div>
              <span>Session close</span>
              <strong>{shortUtc(response.session_close_utc)}</strong>
            </div>
            <div>
              <span>Configured grace ceiling</span>
              <strong>{formatReadinessNumber(response.configured_grace_seconds, "s")}</strong>
            </div>
            <div>
              <span>Configured future-skew ceiling</span>
              <strong>{formatReadinessNumber(response.configured_future_skew_seconds, "s")}</strong>
            </div>
          </div>

          {response.applicability === "not_applicable" && (
            <div className="unavailable-notice" style={{ color: "var(--text-muted, #888)" }}>
              Not applicable to the current deployment mode / market-data source. This deployment is not
              autonomous US equity/ETF PAPER operation.
            </div>
          )}

          {response.applicability === "applicable" && response.assignments.length === 0 && (
            <div className="unavailable-notice">
              Applicable deployment reported zero assignments. Overall state cannot be proven ready.
            </div>
          )}

          {response.assignments.length > 0 && (
            <div>
              <span className="eyebrow">assignments ({response.assignments.length})</span>
              {response.assignments.map((a, i) => (
                <AssignmentBlock key={`${a.assignment_symbol}|${a.assignment_timeframe}|${i}`} assignment={a} />
              ))}
            </div>
          )}
        </>
      )}
    </Panel>
  );
}
