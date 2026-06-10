import { useCallback, useEffect, useRef, useState } from "react";
import { DataTable } from "../../components/common/DataTable";
import { Panel } from "../../components/common/Panel";
import { StatCard } from "../../components/common/StatCard";
import { formatDateTime } from "../../lib/format";
import {
  formatMicrosAsDollars,
  formatNullableNumber,
  formatNullablePercent,
  parseEquityCurve,
  parseFills,
  parseManifest,
  parseMetrics,
  parseOrders,
  parseStrategyFit,
  STRATEGY_FIT_SCHEMA_VERSION,
} from "./parsers.ts";
import { getBacktestJob, isTerminalJobStatus, normalizeJobStatus, submitBacktestJob } from "./api.ts";
import {
  classifyArtifactPathInput,
  buildRepoRelativePath,
  buildMd1DSymbolPath,
  FIXTURE_BARS_SEGMENTS,
  BACKTEST_OUT_SEGMENTS,
  MD_BACKUP_1D_SEGMENTS,
} from "./pathHelpers.ts";
import { getDesktopRepoRoot } from "../../desktop/bootstrap.ts";
import type {
  ActiveBacktestJob,
  ArtifactBundle,
  BacktestManifest,
  BacktestMetrics,
  BacktestJobStatusKind,
  EquityCurveRow,
  FileResult,
  FillRow,
  OrderRow,
  ParsedCsvResult,
  StrategyFitGateFlags,
  StrategyFitParseResult,
} from "./types.ts";

// ---------------------------------------------------------------------------
// Artifact loading
// ---------------------------------------------------------------------------

async function invokeReadArtifactFile(
  folder: string,
  filename: string,
): Promise<string | null> {
  const mod = await import("@tauri-apps/api/core");
  return mod.invoke<string | null>("read_artifact_file", { folder, filename });
}

async function loadFileResult<T>(
  folder: string,
  filename: string,
  parse: (content: string) => T,
): Promise<FileResult<T>> {
  try {
    const content = await invokeReadArtifactFile(folder, filename);
    if (content === null) return { kind: "missing" };
    try {
      return { kind: "ok", data: parse(content) };
    } catch (e) {
      return { kind: "parse_error", message: String(e) };
    }
  } catch (e) {
    return { kind: "read_error", message: String(e) };
  }
}

async function loadBundle(folder: string): Promise<ArtifactBundle> {
  const [manifest, metrics, equityCurve, orders, fills, strategyFit] = await Promise.all([
    loadFileResult(folder, "manifest.json", parseManifest),
    loadFileResult(folder, "metrics.json", parseMetrics),
    loadFileResult(folder, "equity_curve.csv", parseEquityCurve),
    loadFileResult(folder, "orders.csv", parseOrders),
    loadFileResult(folder, "fills.csv", parseFills),
    loadFileResult(folder, "strategy_fit.json", parseStrategyFit),
  ]);
  return { manifest, metrics, equityCurve, orders, fills, strategyFit };
}

// ---------------------------------------------------------------------------
// Shared display components
// ---------------------------------------------------------------------------

function FileStatusNote({ label, result }: { label: string; result: FileResult<unknown> }) {
  if (result.kind === "ok") return null;
  if (result.kind === "idle" || result.kind === "loading") return null;
  if (result.kind === "missing") {
    return (
      <div className="unavailable-notice" style={{ marginBottom: 8 }}>
        <strong>{label} missing</strong> — file not found in artifact folder.
      </div>
    );
  }
  if (result.kind === "parse_error") {
    return (
      <div className="unavailable-notice unavailable-critical" style={{ marginBottom: 8 }}>
        <strong>{label} parse error:</strong> {result.message}
      </div>
    );
  }
  return (
    <div className="unavailable-notice unavailable-critical" style={{ marginBottom: 8 }}>
      <strong>{label} read error:</strong> {result.message}
    </div>
  );
}

function ManifestSection({ manifest }: { manifest: BacktestManifest }) {
  return (
    <Panel title="Run summary" subtitle="Identity and provenance from manifest.json.">
      <div className="timeline-meta-grid">
        <div>
          <span>Run ID</span>
          <strong style={{ fontFamily: "monospace", fontSize: "0.82rem" }}>{manifest.run_id}</strong>
        </div>
        <div>
          <span>Strategy</span>
          <strong>{manifest.strategy_name}</strong>
        </div>
        <div>
          <span>Engine</span>
          <strong>{manifest.engine_id}</strong>
        </div>
        <div>
          <span>Mode</span>
          <strong>{manifest.mode}</strong>
        </div>
        <div>
          <span>Created</span>
          <strong>{formatDateTime(manifest.created_at_utc)}</strong>
        </div>
        <div>
          <span>Git hash</span>
          <strong style={{ fontFamily: "monospace" }}>{manifest.git_hash ?? "—"}</strong>
        </div>
        <div>
          <span>Config hash</span>
          <strong style={{ fontFamily: "monospace", fontSize: "0.78rem" }}>{manifest.config_hash ?? "—"}</strong>
        </div>
        <div>
          <span>Host</span>
          <strong style={{ fontSize: "0.82rem" }}>{manifest.host_fingerprint ?? "—"}</strong>
        </div>
      </div>
    </Panel>
  );
}

function MetricsSection({ m }: { m: BacktestMetrics }) {
  const returnTone =
    m.total_return_micros > 0 ? "good" : m.total_return_micros < 0 ? "bad" : "neutral";
  const ddTone = m.max_drawdown_pct > 10 ? "bad" : m.max_drawdown_pct > 3 ? "warn" : "neutral";
  const haltTone = m.halted ? "bad" : "neutral";

  return (
    <>
      <div className="summary-grid summary-grid-five">
        <StatCard
          title="Starting Equity"
          value={formatMicrosAsDollars(m.starting_equity_micros)}
          detail="Initial portfolio value"
          tone="neutral"
        />
        <StatCard
          title="Ending Equity"
          value={formatMicrosAsDollars(m.ending_equity_micros)}
          detail="Final portfolio value"
          tone={returnTone}
        />
        <StatCard
          title="Total Return"
          value={formatNullablePercent(m.total_return_pct)}
          detail={formatMicrosAsDollars(m.total_return_micros)}
          tone={returnTone}
        />
        <StatCard
          title="Max Drawdown"
          value={formatNullablePercent(m.max_drawdown_pct)}
          detail={formatMicrosAsDollars(m.max_drawdown_micros)}
          tone={ddTone}
        />
        <StatCard
          title="Win Rate"
          value={formatNullablePercent(m.win_rate_pct)}
          detail={`${m.winning_trade_count}W / ${m.losing_trade_count}L / ${m.flat_trade_count}F`}
          tone={m.win_rate_pct != null && m.win_rate_pct >= 50 ? "good" : "neutral"}
        />
      </div>

      <div className="summary-grid summary-grid-five">
        <StatCard
          title="Profit Factor"
          value={formatNullableNumber(m.profit_factor, 2)}
          detail="Gross profit ÷ gross loss"
          tone={m.profit_factor != null && m.profit_factor > 1 ? "good" : "neutral"}
        />
        <StatCard
          title="Expectancy"
          value={formatMicrosAsDollars(m.expectancy_micros)}
          detail="Avg profit per trade"
          tone={
            m.expectancy_micros != null
              ? m.expectancy_micros > 0
                ? "good"
                : "bad"
              : "neutral"
          }
        />
        <StatCard
          title="Sharpe Ratio"
          value={formatNullableNumber(m.sharpe_ratio, 3)}
          detail="Risk-adjusted return"
          tone={m.sharpe_ratio != null && m.sharpe_ratio >= 1 ? "good" : "neutral"}
        />
        <StatCard
          title="Sortino Ratio"
          value={formatNullableNumber(m.sortino_ratio, 3)}
          detail="Downside-risk-adjusted"
          tone={m.sortino_ratio != null && m.sortino_ratio >= 1 ? "good" : "neutral"}
        />
        <StatCard
          title="Exposure"
          value={formatNullablePercent(m.exposure_time_pct)}
          detail={`${m.exposure_bars} / ${m.bars} bars`}
          tone="neutral"
        />
      </div>

      <Panel
        title="Trade statistics"
        subtitle="Execution summary from metrics.json."
      >
        <div className="timeline-meta-grid">
          <div>
            <span>Trade count</span>
            <strong>{m.trade_count}</strong>
          </div>
          <div>
            <span>Orders / Fills</span>
            <strong>
              {m.orders} / {m.fills}
            </strong>
          </div>
          <div>
            <span>Orders rejected</span>
            <strong>{m.orders_rejected}</strong>
          </div>
          <div>
            <span>Total commission</span>
            <strong>{formatMicrosAsDollars(m.total_commission_micros)}</strong>
          </div>
          <div>
            <span>Gross profit</span>
            <strong>{formatMicrosAsDollars(m.gross_profit_micros)}</strong>
          </div>
          <div>
            <span>Gross loss</span>
            <strong>{formatMicrosAsDollars(m.gross_loss_micros)}</strong>
          </div>
          <div>
            <span>Avg win</span>
            <strong>{formatMicrosAsDollars(m.average_win_micros)}</strong>
          </div>
          <div>
            <span>Avg loss</span>
            <strong>{formatMicrosAsDollars(m.average_loss_micros)}</strong>
          </div>
          <div>
            <span>Best trade</span>
            <strong>{formatMicrosAsDollars(m.best_trade_micros)}</strong>
          </div>
          <div>
            <span>Worst trade</span>
            <strong>{formatMicrosAsDollars(m.worst_trade_micros)}</strong>
          </div>
          <div>
            <span>HWM equity</span>
            <strong>{formatMicrosAsDollars(m.equity_high_water_mark_micros)}</strong>
          </div>
          <div>
            <span>Symbols</span>
            <strong>{m.symbols.length > 0 ? m.symbols.join(", ") : "—"}</strong>
          </div>
          <div>
            <span>Halted</span>
            <strong style={{ color: m.halted ? "var(--critical)" : undefined }}>
              {m.halted ? `Yes — ${m.halt_reason ?? "no reason"}` : "No"}
            </strong>
          </div>
          <div>
            <span>Execution blocked</span>
            <strong style={{ color: m.execution_blocked ? "var(--warning)" : undefined }}>
              {m.execution_blocked ? "Yes" : "No"}
            </strong>
          </div>
        </div>
        {m.execution_blocked && (
          <div className="unavailable-notice" style={{ marginTop: 12 }}>
            <strong>Execution blocked by integrity gate.</strong>{" "}
            No orders were placed after the block. Common cause for daily (1D) data:{" "}
            the default stale threshold of 120 s is exceeded by the first 86400 s bar gap.
            Re-run via Workflow B with the stale threshold field left blank (auto-default 172800)
            or set it explicitly to at least 172800.
          </div>
        )}
        {m.halted && (
          <div
            className="unavailable-notice unavailable-critical"
            style={{ marginTop: 12 }}
          >
            <strong>Backtest halted:</strong> {m.halt_reason ?? "unknown reason"}
          </div>
        )}
      </Panel>

      <StatCard
        title="Halt status"
        value={m.halted ? "HALTED" : "Completed"}
        detail={m.halt_reason ?? "Normal completion"}
        tone={haltTone}
      />
    </>
  );
}

// ---------------------------------------------------------------------------
// Strategy fit / promotion gate panel
// ---------------------------------------------------------------------------

const STRATEGY_FIT_MAX_FAILURE_REASONS = 10;

const STRATEGY_FIT_GATE_ROWS: { key: keyof StrategyFitGateFlags; label: string }[] = [
  { key: "profit_factor_failed", label: "Profit factor" },
  { key: "expectancy_failed", label: "Expectancy" },
  { key: "cost_adjusted_edge_failed", label: "Cost-adjusted edge" },
  { key: "out_of_sample_failed", label: "Out-of-sample validation" },
  { key: "sample_quality_failed", label: "Sample quality" },
  { key: "parameter_stability_failed", label: "Parameter stability" },
  { key: "validation_metrics_missing", label: "Validation metrics" },
];

function StrategyFitPanel({ result }: { result: FileResult<StrategyFitParseResult> }) {
  if (result.kind === "idle" || result.kind === "loading") return null;

  return (
    <Panel
      title="Strategy fit / promotion gate"
      subtitle="Backtest gate evaluation from strategy_fit.json (research-py BACKTEST-GATES-01)."
    >
      {result.kind === "missing" && (
        <div className="empty-state">
          Strategy-fit / promotion gate result not found for this artifact folder.
        </div>
      )}
      {result.kind === "read_error" && (
        <div className="unavailable-notice unavailable-critical">
          <strong>strategy_fit.json read error:</strong> {result.message}
        </div>
      )}
      {result.kind === "parse_error" && (
        <div className="unavailable-notice unavailable-critical">
          <strong>strategy_fit.json parse error:</strong> {result.message}
        </div>
      )}
      {result.kind === "ok" && <StrategyFitContent fit={result.data} />}
    </Panel>
  );
}

function StrategyFitContent({ fit }: { fit: StrategyFitParseResult }) {
  if (fit.kind === "unsupported_schema") {
    return (
      <div className="unavailable-notice">
        <strong>Unsupported strategy-fit schema:</strong>{" "}
        {fit.schemaVersion ? `'${fit.schemaVersion}'` : "schema_version missing"} — expected{" "}
        '{STRATEGY_FIT_SCHEMA_VERSION}'.
      </div>
    );
  }

  if (fit.kind === "malformed") {
    return (
      <div className="unavailable-notice unavailable-critical">
        <strong>Invalid strategy_fit.json:</strong> {fit.message}
      </div>
    );
  }

  if (fit.kind === "missing_fields") {
    return (
      <div className="unavailable-notice unavailable-critical">
        <strong>strategy_fit.json missing required fields:</strong> {fit.message}
      </div>
    );
  }

  const a = fit.data;
  const visibleReasons = a.failure_reasons.slice(0, STRATEGY_FIT_MAX_FAILURE_REASONS);
  const hiddenReasonCount = a.failure_reasons.length - visibleReasons.length;

  return (
    <>
      <div className="timeline-meta-grid">
        <div>
          <span>Schema</span>
          <strong style={{ fontFamily: "monospace" }}>{a.schema_version}</strong>
        </div>
        <div>
          <span>Symbol</span>
          <strong>{a.symbol}</strong>
        </div>
        <div>
          <span>Strategy</span>
          <strong>{a.strategy_id}</strong>
        </div>
        <div>
          <span>Timeframe</span>
          <strong>{a.timeframe ?? "—"}</strong>
        </div>
      </div>

      <div className="summary-grid summary-grid-five">
        <StatCard
          title="Recommended for paper"
          value={a.recommended_for_paper ? "YES" : "NO"}
          tone={a.recommended_for_paper ? "good" : "bad"}
        />
        <StatCard
          title="Recommended for live"
          value={a.recommended_for_live ? "YES" : "NO"}
          detail={a.recommended_for_live_present ? undefined : "not reported by artifact"}
          tone={a.recommended_for_live ? "bad" : "neutral"}
        />
        <StatCard title="Trades" value={a.trades != null ? String(a.trades) : "—"} />
        <StatCard title="Profit factor" value={formatNullableNumber(a.profit_factor, 2)} />
        <StatCard title="Expectancy (bps)" value={formatNullableNumber(a.expectancy_bps, 2)} />
      </div>

      <div className="summary-grid summary-grid-five">
        <StatCard
          title="Net expectancy after cost (bps)"
          value={formatNullableNumber(a.net_expectancy_after_cost_bps, 2)}
          tone={
            a.net_expectancy_after_cost_bps != null
              ? a.net_expectancy_after_cost_bps > 0
                ? "good"
                : "bad"
              : "neutral"
          }
        />
      </div>

      {a.recommended_for_live && (
        <div className="unavailable-notice unavailable-critical" style={{ marginTop: 12 }}>
          <strong>Anomaly:</strong> this artifact reports recommended_for_live=true. Live
          recommendations are never authorized by this system — verify the source artifact.
        </div>
      )}

      {visibleReasons.length > 0 && (
        <div style={{ marginTop: 12 }}>
          <span className="eyebrow">Failure reasons ({a.failure_reasons.length})</span>
          <ul>
            {visibleReasons.map((reason) => (
              <li key={reason} style={{ fontFamily: "monospace", fontSize: "0.82rem" }}>
                {reason}
              </li>
            ))}
          </ul>
          {hiddenReasonCount > 0 && (
            <div className="unavailable-notice">
              + {hiddenReasonCount} more reason(s) not shown.
            </div>
          )}
        </div>
      )}

      <div style={{ marginTop: 12 }}>
        <DataTable
          rows={STRATEGY_FIT_GATE_ROWS}
          rowKey={(row) => row.key}
          columns={[
            { key: "gate", title: "Gate", render: (row) => row.label },
            {
              key: "result",
              title: "Result",
              render: (row) => (
                <span style={{ color: a.gateFlags[row.key] ? "var(--critical)" : "var(--success)" }}>
                  {a.gateFlags[row.key] ? "FAIL" : "PASS"}
                </span>
              ),
            },
          ]}
        />
      </div>
    </>
  );
}

function EquityCurveSection({
  result,
}: {
  result: FileResult<ParsedCsvResult<EquityCurveRow>>;
}) {
  return (
    <Panel title="Equity curve" subtitle="Portfolio value over time from equity_curve.csv.">
      <FileStatusNote label="equity_curve.csv" result={result} />
      {result.kind === "ok" && <EquityCurveContent data={result.data} />}
    </Panel>
  );
}

function EquityCurveContent({ data }: { data: ParsedCsvResult<EquityCurveRow> }) {
  const { rows, malformed } = data;

  if (rows.length === 0) {
    return (
      <div className="empty-state">
        {malformed > 0
          ? `Equity curve empty — ${malformed} malformed row(s) skipped.`
          : "No equity curve data — backtest produced no bars."}
      </div>
    );
  }

  const equities = rows.map((r) => r.equity);
  const minEq = Math.min(...equities);
  const maxEq = Math.max(...equities);
  const range = maxEq - minEq;
  const flat = range === 0;

  const width = 600;
  const height = 80;
  const padX = 0;
  const padY = 4;

  const points = rows
    .map((r, i) => {
      const x = rows.length === 1 ? width / 2 : padX + (i / (rows.length - 1)) * (width - padX * 2);
      const y = flat
        ? height / 2
        : padY + ((maxEq - r.equity) / range) * (height - padY * 2);
      return `${x.toFixed(1)},${y.toFixed(1)}`;
    })
    .join(" ");

  const lastEq = equities[equities.length - 1];
  const firstEq = equities[0];
  const curveColor =
    lastEq > firstEq ? "var(--success)" : lastEq < firstEq ? "var(--critical)" : "var(--accent)";

  const formatEq = (v: number) =>
    (v / 1_000_000).toLocaleString(undefined, { style: "currency", currency: "USD", maximumFractionDigits: 0 });

  return (
    <div className="bt-equity-curve">
      <div className="bt-equity-meta">
        <span>
          <span className="eyebrow">bars</span> {rows.length}
        </span>
        <span>
          <span className="eyebrow">min</span> {formatEq(minEq)}
        </span>
        <span>
          <span className="eyebrow">max</span> {formatEq(maxEq)}
        </span>
        {malformed > 0 && (
          <span style={{ color: "var(--warning)" }}>
            {malformed} malformed row(s) skipped
          </span>
        )}
      </div>
      <svg
        className="bt-equity-svg"
        viewBox={`0 0 ${width} ${height}`}
        preserveAspectRatio="none"
        aria-label="Equity curve"
      >
        <polyline
          points={points}
          fill="none"
          stroke={curveColor}
          strokeWidth="1.5"
          vectorEffect="non-scaling-stroke"
        />
      </svg>
    </div>
  );
}

function OrdersSection({ result }: { result: FileResult<ParsedCsvResult<OrderRow>> }) {
  return (
    <Panel title="Orders" subtitle="All simulated orders from orders.csv.">
      <FileStatusNote label="orders.csv" result={result} />
      {result.kind === "ok" && <OrdersContent data={result.data} />}
    </Panel>
  );
}

function OrdersContent({ data }: { data: ParsedCsvResult<OrderRow> }) {
  const { rows, malformed } = data;
  if (rows.length === 0) {
    return (
      <div className="empty-state">
        {malformed > 0
          ? `No valid orders — ${malformed} malformed row(s) skipped.`
          : "No orders — strategy produced no order signals."}
      </div>
    );
  }
  return (
    <>
      {malformed > 0 && (
        <div className="unavailable-notice" style={{ marginBottom: 8 }}>
          {malformed} malformed row(s) skipped.
        </div>
      )}
      <DataTable
        rows={rows}
        rowKey={(r) => r.order_id}
        columns={[
          { key: "ts", title: "Time", render: (r) => r.ts_utc },
          {
            key: "id",
            title: "Order ID",
            render: (r) => (
              <span
                style={{ fontFamily: "monospace", fontSize: "0.78rem" }}
                title={r.order_id}
              >
                {r.order_id.length > 12 ? r.order_id.slice(0, 12) + "…" : r.order_id}
              </span>
            ),
          },
          { key: "sym", title: "Symbol", render: (r) => r.symbol },
          { key: "side", title: "Side", render: (r) => r.side },
          { key: "qty", title: "Qty", render: (r) => r.qty },
          { key: "type", title: "Type", render: (r) => r.order_type || "—" },
          { key: "status", title: "Status", render: (r) => r.status },
        ]}
      />
    </>
  );
}

function FillsSection({ result }: { result: FileResult<ParsedCsvResult<FillRow>> }) {
  return (
    <Panel title="Fills" subtitle="Executed fills from fills.csv.">
      <FileStatusNote label="fills.csv" result={result} />
      {result.kind === "ok" && <FillsContent data={result.data} />}
    </Panel>
  );
}

function FillsContent({ data }: { data: ParsedCsvResult<FillRow> }) {
  const { rows, malformed } = data;
  if (rows.length === 0) {
    return (
      <div className="empty-state">
        {malformed > 0
          ? `No valid fills — ${malformed} malformed row(s) skipped.`
          : "No fills — no orders were executed."}
      </div>
    );
  }
  return (
    <>
      {malformed > 0 && (
        <div className="unavailable-notice" style={{ marginBottom: 8 }}>
          {malformed} malformed row(s) skipped.
        </div>
      )}
      <DataTable
        rows={rows}
        rowKey={(r) => r.fill_id}
        columns={[
          { key: "ts", title: "Time", render: (r) => r.ts_utc },
          {
            key: "fid",
            title: "Fill ID",
            render: (r) => (
              <span
                style={{ fontFamily: "monospace", fontSize: "0.78rem" }}
                title={r.fill_id}
              >
                {r.fill_id.length > 10 ? r.fill_id.slice(0, 10) + "…" : r.fill_id}
              </span>
            ),
          },
          {
            key: "oid",
            title: "Order ID",
            render: (r) => (
              <span
                style={{ fontFamily: "monospace", fontSize: "0.78rem" }}
                title={r.order_id}
              >
                {r.order_id.length > 10 ? r.order_id.slice(0, 10) + "…" : r.order_id}
              </span>
            ),
          },
          { key: "sym", title: "Symbol", render: (r) => r.symbol },
          { key: "side", title: "Side", render: (r) => r.side },
          { key: "qty", title: "Qty", render: (r) => r.qty },
          {
            key: "price",
            title: "Price",
            render: (r) => {
              const n = Number(r.price);
              return Number.isNaN(n)
                ? r.price
                : (n / 1_000_000).toLocaleString(undefined, {
                    style: "currency",
                    currency: "USD",
                    minimumFractionDigits: 2,
                  });
            },
          },
          {
            key: "fee",
            title: "Fee",
            render: (r) => {
              const n = Number(r.fee);
              return Number.isNaN(n)
                ? r.fee || "—"
                : (n / 1_000_000).toLocaleString(undefined, {
                    style: "currency",
                    currency: "USD",
                    minimumFractionDigits: 2,
                  });
            },
          },
        ]}
      />
    </>
  );
}

function ArtifactDisplay({ bundle }: { bundle: ArtifactBundle }) {
  return (
    <>
      <FileStatusNote label="manifest.json" result={bundle.manifest} />
      {bundle.manifest.kind === "ok" && (
        <ManifestSection manifest={bundle.manifest.data} />
      )}

      <FileStatusNote label="metrics.json" result={bundle.metrics} />
      {bundle.metrics.kind === "ok" && (
        <MetricsSection m={bundle.metrics.data} />
      )}

      <StrategyFitPanel result={bundle.strategyFit} />

      <EquityCurveSection result={bundle.equityCurve} />
      <OrdersSection result={bundle.orders} />
      <FillsSection result={bundle.fills} />
    </>
  );
}

// ---------------------------------------------------------------------------
// Job status badge
// ---------------------------------------------------------------------------

function JobStatusBadge({ status }: { status: BacktestJobStatusKind }) {
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
// Portable default path builder — derives defaults from the repo root
// injected by the launcher via MQK_REPO_ROOT. Returns "" when unavailable
// so the operator sees a blank field with placeholder instead of a stale path.
// ---------------------------------------------------------------------------

function deriveDefaultBarsPath(): string {
  return buildRepoRelativePath(getDesktopRepoRoot(), ...FIXTURE_BARS_SEGMENTS) ?? "";
}

function deriveDefaultOutDir(): string {
  return buildRepoRelativePath(getDesktopRepoRoot(), ...BACKTEST_OUT_SEGMENTS) ?? "";
}

function deriveMdBackup1DPath(): string {
  return buildRepoRelativePath(getDesktopRepoRoot(), ...MD_BACKUP_1D_SEGMENTS) ?? "";
}

// ---------------------------------------------------------------------------
// Main screen component
// ---------------------------------------------------------------------------

export function BacktestResultsScreen() {
  // --- Workflow A: manual artifact folder load ---
  const [folderPath, setFolderPath] = useState("");
  const [bundle, setBundle] = useState<ArtifactBundle | null>(null);
  const [loading, setLoading] = useState(false);
  const [loadError, setLoadError] = useState<string | null>(null);

  const handleLoad = useCallback(async () => {
    const trimmed = folderPath.trim();
    if (!trimmed) return;

    const pathKind = classifyArtifactPathInput(trimmed);
    if (pathKind.kind === "csv_file") {
      setLoadError(
        "This is a bars CSV. To view results, select the artifact folder containing manifest.json and metrics.json.",
      );
      return;
    }

    setLoading(true);
    setLoadError(null);
    setBundle(null);
    try {
      const result = await loadBundle(trimmed);
      setBundle(result);
    } catch (e) {
      setLoadError(String(e));
    } finally {
      setLoading(false);
    }
  }, [folderPath]);

  const handleKeyDown = useCallback(
    (e: React.KeyboardEvent<HTMLInputElement>) => {
      if (e.key === "Enter") void handleLoad();
    },
    [handleLoad],
  );

  // --- Workflow B: submit CSV backtest job ---
  const [barsPath, setBarsPath] = useState(() => deriveDefaultBarsPath());
  const [strategy, setStrategy] = useState("swing_momentum");
  const [symbol, setSymbol] = useState("TEST");
  const [timeframeSecs, setTimeframeSecs] = useState("86400");
  const [initialCashMicros, setInitialCashMicros] = useState("100000000000");
  const [outDir, setOutDir] = useState(() => deriveDefaultOutDir());
  // Empty = use daemon's timeframe-aware default (172800 for daily, 120 otherwise).
  const [integrityStaleThresholdTicks, setIntegrityStaleThresholdTicks] = useState("");

  // Refresh portable defaults once the Tauri bootstrap cache is warm (async
  // init may not have finished before first render). Only fills empty fields —
  // any operator edits are preserved.
  useEffect(() => {
    const root = getDesktopRepoRoot();
    if (!root) return;
    setBarsPath((prev) => prev || (buildRepoRelativePath(root, ...FIXTURE_BARS_SEGMENTS) ?? ""));
    setOutDir((prev) => prev || (buildRepoRelativePath(root, ...BACKTEST_OUT_SEGMENTS) ?? ""));
  }, []);

  const [jobSubmitting, setJobSubmitting] = useState(false);
  const [jobSubmitError, setJobSubmitError] = useState<string | null>(null);
  const [activeJob, setActiveJob] = useState<ActiveBacktestJob | null>(null);
  const [jobBundle, setJobBundle] = useState<ArtifactBundle | null>(null);
  const [jobBundleLoading, setJobBundleLoading] = useState(false);
  const [jobBundleError, setJobBundleError] = useState<string | null>(null);

  // Polling: fire when a non-terminal job is active. Bounded cadence: 2s.
  // Cancellation token prevents state updates after unmount or job change.
  const pollingRef = useRef<{ cancelled: boolean }>({ cancelled: false });

  useEffect(() => {
    if (!activeJob) return;
    if (isTerminalJobStatus(activeJob.status)) return;

    const token = { cancelled: false };
    pollingRef.current = token;

    async function poll() {
      while (!token.cancelled) {
        await new Promise<void>((resolve) => setTimeout(resolve, 2000));
        if (token.cancelled) break;

        const result = await getBacktestJob(activeJob!.jobId);
        if (token.cancelled) break;

        if (!result.ok) {
          setActiveJob((prev) =>
            prev?.jobId === activeJob!.jobId
              ? { ...prev, status: "failed" as BacktestJobStatusKind, error: result.error ?? "Poll failed." }
              : prev,
          );
          break;
        }

        const data = result.data!;
        const newStatus = normalizeJobStatus(data.status);

        setActiveJob((prev) =>
          prev?.jobId === activeJob!.jobId
            ? {
                ...prev,
                status: newStatus,
                startedAt: data.started_at_utc ?? null,
                completedAt: data.completed_at_utc ?? null,
                artifactDir: data.artifact_dir ?? null,
                error: data.error ?? null,
              }
            : prev,
        );

        if (isTerminalJobStatus(newStatus)) {
          if (newStatus === "completed") {
            if (data.artifact_dir) {
              if (!token.cancelled) setJobBundleLoading(true);
              loadBundle(data.artifact_dir)
                .then((b) => {
                  if (!token.cancelled) {
                    setJobBundle(b);
                    setJobBundleLoading(false);
                  }
                })
                .catch((e) => {
                  if (!token.cancelled) {
                    setJobBundleError(String(e));
                    setJobBundleLoading(false);
                  }
                });
            }
            // completed but no artifact_dir — leave jobBundle null; screen shows warning
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

  const handleSubmitJob = useCallback(async () => {
    const tf = parseInt(timeframeSecs, 10);
    const cash = parseInt(initialCashMicros, 10);
    const staleThresholdRaw = integrityStaleThresholdTicks.trim();
    const staleThreshold = staleThresholdRaw ? parseInt(staleThresholdRaw, 10) : null;

    if (!barsPath.trim()) { setJobSubmitError("bars_path is required."); return; }
    if (!strategy.trim()) { setJobSubmitError("strategy is required."); return; }
    if (!symbol.trim()) { setJobSubmitError("symbol is required."); return; }
    if (Number.isNaN(tf) || tf <= 0) { setJobSubmitError("timeframe_secs must be a positive integer."); return; }
    if (Number.isNaN(cash) || cash <= 0) { setJobSubmitError("initial_cash_micros must be a positive integer."); return; }
    if (staleThreshold !== null && (Number.isNaN(staleThreshold) || staleThreshold <= 0)) {
      setJobSubmitError("Integrity stale threshold must be a positive integer (or leave blank for auto).");
      return;
    }

    setJobSubmitting(true);
    setJobSubmitError(null);
    setActiveJob(null);
    setJobBundle(null);
    setJobBundleError(null);

    const result = await submitBacktestJob({
      bars_path: barsPath.trim(),
      strategy: strategy.trim(),
      symbol: symbol.trim(),
      timeframe_secs: tf,
      initial_cash_micros: cash,
      out_dir: outDir.trim() || null,
      integrity_stale_threshold_ticks: staleThreshold,
    });

    setJobSubmitting(false);

    if (!result.ok) {
      setJobSubmitError(result.error ?? "Submission failed.");
      return;
    }

    const data = result.data!;

    if (!data.accepted) {
      setJobSubmitError(data.error ?? "Job refused by daemon.");
      return;
    }

    setActiveJob({
      jobId: data.job_id,
      status: normalizeJobStatus(data.status),
      strategy: strategy.trim(),
      symbol: symbol.trim(),
      createdAt: new Date().toISOString(),
      startedAt: null,
      completedAt: null,
      artifactDir: data.artifact_dir ?? null,
      error: null,
    });
  }, [barsPath, strategy, symbol, timeframeSecs, initialCashMicros, outDir, integrityStaleThresholdTicks]);

  const jobIsActive = activeJob !== null && !isTerminalJobStatus(activeJob.status);

  return (
    <div className="screen-grid desk-screen-grid">

      {/* ------------------------------------------------------------------ */}
      {/* Workflow A — Load completed artifact folder                          */}
      {/* ------------------------------------------------------------------ */}

      <Panel
        title="A — Load completed artifact folder"
        subtitle="Paste the path to the folder that directly contains manifest.json, metrics.json, equity_curve.csv, orders.csv, fills.csv."
      >
        <div className="bt-path-hint" style={{ marginBottom: 8, fontSize: "0.82rem", color: "var(--text-muted, #888)" }}>
          The artifact folder is the run-id folder inside <code>exports\backtests\</code>, not a CSV file.
        </div>
        <div className="bt-path-row">
          <input
            className="bt-path-input"
            type="text"
            placeholder="Folder path — e.g. exports\backtests\7273b99b-bbed-58be-9db4-615e9bfaf901"
            value={folderPath}
            onChange={(e) => { setFolderPath(e.target.value); setLoadError(null); }}
            onKeyDown={handleKeyDown}
            spellCheck={false}
            autoComplete="off"
          />
          <button
            type="button"
            className="action-button"
            onClick={() => void handleLoad()}
            disabled={loading || !folderPath.trim()}
          >
            {loading ? "Loading…" : "Load"}
          </button>
        </div>
        {loadError && (
          <div className="unavailable-notice unavailable-critical" style={{ marginTop: 8 }}>
            <strong>Load failed:</strong> {loadError}
          </div>
        )}
      </Panel>

      {!bundle && !loading && !loadError && (
        <div className="empty-state" style={{ padding: "16px 0" }}>
          No artifact folder loaded. Enter a completed backtest run folder path above and click Load.
        </div>
      )}

      {bundle && <ArtifactDisplay bundle={bundle} />}

      {/* ------------------------------------------------------------------ */}
      {/* Workflow B — Run new CSV backtest (offline research only)            */}
      {/* ------------------------------------------------------------------ */}

      <Panel
        title="B — Run new CSV backtest (offline research only)"
        subtitle="Runs an offline backtest against a CSV bars file. No live or paper orders are created. No broker adapter is called."
      >
        <div className="bt-job-form-grid">
          <div className="bt-job-field" style={{ gridColumn: "1 / -1" }}>
            <label htmlFor="bt-bars-path">Bars CSV path</label>
            <input
              id="bt-bars-path"
              type="text"
              value={barsPath}
              onChange={(e) => setBarsPath(e.target.value)}
              spellCheck={false}
              autoComplete="off"
              placeholder={
                getDesktopRepoRoot()
                  ? "Path to .csv bars file (portable default pre-filled above)"
                  : "Absolute path to .csv bars file — repo root not detected, set manually"
              }
            />
            {(() => {
              const md1DDir = deriveMdBackup1DPath();
              const examplePath = buildMd1DSymbolPath(getDesktopRepoRoot(), symbol || "AAPL");
              return (
                <div className="bt-field-hint" style={{ marginTop: 4, fontSize: "0.79rem", color: "var(--text-muted, #888)" }}>
                  {md1DDir
                    ? <>
                        <strong>Real 1D market-data backup:</strong>{" "}
                        <code>{md1DDir}</code> — paste <code>{"<SYMBOL>"}_1D.csv</code> from that folder
                        {examplePath ? <> (e.g. <code>{examplePath.split(/[\\/]/).pop()}</code>)</> : null}.
                        {" "}The pre-filled <code>bkt4_bars.csv</code> fixture has 3 bars — plumbing proof only.
                      </>
                    : <>
                        Set repo root via the launcher to see the portable 1D market-data backup path.
                        Real 1D bars live in <code>exports\md_backup\1D\{"<SYMBOL>"}_1D.csv</code>.
                        The pre-filled <code>bkt4_bars.csv</code> fixture has 3 bars — plumbing proof only.
                      </>
                  }
                </div>
              );
            })()}
          </div>
          <div className="bt-job-field">
            <label htmlFor="bt-strategy">Strategy</label>
            <input
              id="bt-strategy"
              type="text"
              value={strategy}
              onChange={(e) => setStrategy(e.target.value)}
              spellCheck={false}
              autoComplete="off"
              placeholder="e.g. swing_momentum"
            />
          </div>
          <div className="bt-job-field">
            <label htmlFor="bt-symbol">Symbol</label>
            <input
              id="bt-symbol"
              type="text"
              value={symbol}
              onChange={(e) => setSymbol(e.target.value)}
              spellCheck={false}
              autoComplete="off"
              placeholder="e.g. TEST"
            />
          </div>
          <div className="bt-job-field">
            <label htmlFor="bt-timeframe">Timeframe (seconds)</label>
            <input
              id="bt-timeframe"
              type="text"
              value={timeframeSecs}
              onChange={(e) => setTimeframeSecs(e.target.value)}
              spellCheck={false}
              autoComplete="off"
              placeholder="e.g. 86400"
            />
          </div>
          <div className="bt-job-field">
            <label htmlFor="bt-cash">Initial cash (micros)</label>
            <input
              id="bt-cash"
              type="text"
              value={initialCashMicros}
              onChange={(e) => setInitialCashMicros(e.target.value)}
              spellCheck={false}
              autoComplete="off"
              placeholder="e.g. 100000000000"
            />
          </div>
          <div className="bt-job-field" style={{ gridColumn: "1 / -1" }}>
            <label htmlFor="bt-out-dir">Output folder</label>
            <input
              id="bt-out-dir"
              type="text"
              value={outDir}
              onChange={(e) => setOutDir(e.target.value)}
              spellCheck={false}
              autoComplete="off"
              placeholder={
                getDesktopRepoRoot()
                  ? "Output folder (portable default pre-filled above)"
                  : "exports\\backtests — repo root not detected, set manually"
              }
            />
            <div className="bt-field-hint" style={{ marginTop: 4, fontSize: "0.79rem", color: "var(--text-muted, #888)" }}>
              Artifacts are written into a run-id subfolder here. Load that subfolder with Workflow A to view results.
            </div>
          </div>
          <div className="bt-job-field">
            <label htmlFor="bt-stale-threshold">
              Integrity stale threshold (seconds){" "}
              <span style={{ fontWeight: "normal", color: "var(--text-muted, #888)" }}>optional</span>
            </label>
            <input
              id="bt-stale-threshold"
              type="text"
              value={integrityStaleThresholdTicks}
              onChange={(e) => setIntegrityStaleThresholdTicks(e.target.value)}
              spellCheck={false}
              autoComplete="off"
              placeholder={
                parseInt(timeframeSecs, 10) >= 86400
                  ? "auto → 172800 (daily default)"
                  : "auto → 120 (intraday default)"
              }
            />
            <div className="bt-field-hint" style={{ marginTop: 4, fontSize: "0.79rem", color: "var(--text-muted, #888)" }}>
              {parseInt(timeframeSecs, 10) >= 86400
                ? <>
                    <strong>Daily bars detected.</strong>{" "}
                    Leave blank to use the safe default of <strong>172800</strong> (2 days).
                    Daily bar gaps are 86400 s; the intraday default of 120 s would set{" "}
                    <code>execution_blocked=true</code> immediately.
                  </>
                : <>
                    Leave blank to use the intraday default of <strong>120</strong> s.
                    For daily bars (timeframe_secs=86400), use at least <strong>172800</strong>.
                  </>
              }
            </div>
          </div>
        </div>

        <div className="bt-path-row" style={{ marginTop: 4 }}>
          <button
            type="button"
            className="action-button"
            onClick={() => void handleSubmitJob()}
            disabled={jobSubmitting || jobIsActive}
          >
            {jobSubmitting ? "Submitting…" : jobIsActive ? "Job running…" : "Submit backtest"}
          </button>
        </div>

        <div className="bt-field-hint" style={{ marginTop: 8, fontSize: "0.79rem", color: "var(--text-muted, #888)" }}>
          Submission requires the daemon to be running and the operator token to be configured
          (launch via <code>Launch-VeritasLedger.ps1</code> with <code>MQK_OPERATOR_TOKEN</code> set).
        </div>

        {jobSubmitError && (
          <div className="unavailable-notice unavailable-critical" style={{ marginTop: 10 }}>
            <strong>Submit failed:</strong> {jobSubmitError}
          </div>
        )}
      </Panel>

      {/* Job status display */}
      {activeJob && (
        <Panel
          title="Job status"
          subtitle={`Offline backtest job — research only. No broker adapter, no live/paper orders.`}
        >
          <div className="bt-job-status-row">
            <JobStatusBadge status={activeJob.status} />
            <span className="bt-job-meta" title={activeJob.jobId}>
              job {activeJob.jobId.slice(0, 8)}…
            </span>
            <span className="bt-job-meta">
              {activeJob.strategy} / {activeJob.symbol}
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

          {activeJob.status === "failed" && activeJob.error && (
            <div className="unavailable-notice unavailable-critical" style={{ marginTop: 4 }}>
              <strong>Job failed:</strong> {activeJob.error}
            </div>
          )}

          {activeJob.status === "completed" && !activeJob.artifactDir && (
            <div className="unavailable-notice" style={{ marginTop: 4 }}>
              <strong>Completed without artifact path.</strong> Job completed but daemon returned no artifact_dir. Artifacts may still exist on disk — use Workflow A to load manually.
            </div>
          )}

          {activeJob.artifactDir && (
            <div className="bt-job-meta" style={{ marginTop: 6, wordBreak: "break-all" }}>
              <span className="eyebrow">artifact dir</span>{" "}
              <span style={{ color: "var(--text)" }}>{activeJob.artifactDir}</span>
            </div>
          )}

          {(activeJob.status === "queued" || activeJob.status === "running") && (
            <div className="bt-job-meta" style={{ marginTop: 6, color: "var(--accent)" }}>
              Polling for status every 2 seconds…
            </div>
          )}
        </Panel>
      )}

      {/* Job artifact bundle loading state */}
      {jobBundleLoading && (
        <div className="empty-state" style={{ padding: "16px 0" }}>
          Loading artifact results…
        </div>
      )}

      {jobBundleError && (
        <div className="unavailable-notice unavailable-critical" style={{ margin: "8px 0" }}>
          <strong>Job completed but artifact load failed:</strong> {jobBundleError}
        </div>
      )}

      {jobBundle && <ArtifactDisplay bundle={jobBundle} />}
    </div>
  );
}
