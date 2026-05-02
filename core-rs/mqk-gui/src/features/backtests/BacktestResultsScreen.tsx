import { useCallback, useState } from "react";
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
} from "./parsers.ts";
import type {
  ArtifactBundle,
  BacktestManifest,
  BacktestMetrics,
  EquityCurveRow,
  FileResult,
  FillRow,
  OrderRow,
  ParsedCsvResult,
} from "./types.ts";

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
  const [manifest, metrics, equityCurve, orders, fills] = await Promise.all([
    loadFileResult(folder, "manifest.json", parseManifest),
    loadFileResult(folder, "metrics.json", parseMetrics),
    loadFileResult(folder, "equity_curve.csv", parseEquityCurve),
    loadFileResult(folder, "orders.csv", parseOrders),
    loadFileResult(folder, "fills.csv", parseFills),
  ]);
  return { manifest, metrics, equityCurve, orders, fills };
}

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

export function BacktestResultsScreen() {
  const [folderPath, setFolderPath] = useState("");
  const [bundle, setBundle] = useState<ArtifactBundle | null>(null);
  const [loading, setLoading] = useState(false);
  const [loadError, setLoadError] = useState<string | null>(null);

  const handleLoad = useCallback(async () => {
    const trimmed = folderPath.trim();
    if (!trimmed) return;
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

  return (
    <div className="screen-grid desk-screen-grid">
      <Panel title="Artifact folder" subtitle="Paste the path to a backtest artifact folder (the folder containing manifest.json, metrics.json, etc.).">
        <div className="bt-path-row">
          <input
            className="bt-path-input"
            type="text"
            placeholder="e.g. C:\Users\…\exports\backtests\run-name\<run-id>"
            value={folderPath}
            onChange={(e) => setFolderPath(e.target.value)}
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
        <div className="empty-state" style={{ padding: "32px 0" }}>
          No artifact folder loaded. Enter a path above and click Load.
        </div>
      )}

      {bundle && (
        <>
          <FileStatusNote label="manifest.json" result={bundle.manifest} />
          {bundle.manifest.kind === "ok" && (
            <ManifestSection manifest={bundle.manifest.data} />
          )}

          <FileStatusNote label="metrics.json" result={bundle.metrics} />
          {bundle.metrics.kind === "ok" && (
            <MetricsSection m={bundle.metrics.data} />
          )}

          <EquityCurveSection result={bundle.equityCurve} />
          <OrdersSection result={bundle.orders} />
          <FillsSection result={bundle.fills} />
        </>
      )}
    </div>
  );
}
