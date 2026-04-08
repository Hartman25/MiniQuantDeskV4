import { Panel } from "../../../components/common/Panel";
import { formatDateTime } from "../../../lib/format";
import type { OrderChartResponse, OrderReplayResponse, OrderTraceResponse } from "../../system/types";

function chartUnavailableNotice(chart: OrderChartResponse): string | null {
  switch (chart.truth_state) {
    case "no_bars":
      return chart.comment;
    case "no_order":
      return "Order not found in any current authoritative source. Select an active order to render the execution chart.";
    case "no_db":
      return "No database connection — chart unavailable.";
    default:
      return null;
  }
}

export function ExecutionChartPanel({
  chart,
  replay,
  trace,
  selectedFrameIndex,
  onSelectFrame,
}: {
  chart: OrderChartResponse | null;
  replay: OrderReplayResponse | null;
  trace: OrderTraceResponse | null;
  selectedFrameIndex: number;
  onSelectFrame: (index: number) => void;
}) {
  if (!chart) {
    return <Panel title="Execution chart"><div className="empty-state">Select an order to render price plus execution overlays.</div></Panel>;
  }

  const notice = chartUnavailableNotice(chart);

  // When bars are absent (current always-true case), show the truth notice.
  const bars = chart.bars ?? [];
  if (notice || bars.length === 0) {
    return (
      <Panel title="Execution chart" subtitle={chart.symbol ? `${chart.symbol} · ${chart.order_id}` : chart.order_id}>
        <div className="unavailable-notice">{notice ?? "No bar data available."}</div>
        <div className="timeline-meta-grid">
          <div><span>Truth state</span><strong>{chart.truth_state}</strong></div>
          <div><span>Backend</span><strong>{chart.backend}</strong></div>
          <div><span>Order ID</span><strong>{chart.order_id}</strong></div>
          {chart.symbol ? <div><span>Symbol</span><strong>{chart.symbol}</strong></div> : null}
        </div>
      </Panel>
    );
  }

  // Future: render bars when truth_state === "active" and bars.length > 0.
  const overlays = chart.overlays ?? [];
  const width = 960;
  const height = 320;
  const padding = { top: 20, right: 18, bottom: 38, left: 18 };
  const highs = bars.map((bar) => bar.high);
  const lows = bars.map((bar) => bar.low);
  const minPrice = Math.min(...lows);
  const maxPrice = Math.max(...highs);
  const priceRange = Math.max(maxPrice - minPrice, 0.01);
  const innerWidth = width - padding.left - padding.right;
  const innerHeight = height - padding.top - padding.bottom;
  const step = innerWidth / Math.max(bars.length, 1);

  const xForIndex = (index: number) => padding.left + step * index + step / 2;
  const yForPrice = (price: number) => padding.top + ((maxPrice - price) / priceRange) * innerHeight;
  const indexByTs = new Map(bars.map((bar, index) => [bar.ts, index]));

  const activeFrame = replay?.frames[selectedFrameIndex] ?? null;
  const linkedOverlayIds = new Set(
    overlays
      .filter((overlay) => activeFrame && overlay.linked_frame_id === activeFrame.frame_id)
      .map((overlay) => overlay.overlay_id),
  );

  const fillPathPoints = overlays
    .filter((overlay) => overlay.kind === "partial_fill" || overlay.kind === "fill")
    .map((overlay) => {
      const idx = indexByTs.get(overlay.ts);
      return idx == null ? null : `${xForIndex(idx)},${yForPrice(overlay.price)}`;
    })
    .filter(Boolean)
    .join(" ");

  const overlayLabelByKind: Record<string, string> = {
    signal: "Signal", intent: "Intent", risk_pass: "Risk", order_sent: "Sent",
    broker_ack: "ACK", partial_fill: "Partial", fill: "Fill", replace: "Replace",
    cancel: "Cancel", reconcile: "Recon", portfolio: "Port", expected_price: "Ref",
  };

  return (
    <Panel
      title="Execution chart"
      subtitle={`${chart.symbol ?? chart.order_id} · ${chart.timeframe ?? "1m"} · price with execution overlays`}
    >
      <div className="chart-topbar">
        <div className="chart-topbar-item"><span>Reference</span><strong>{chart.reference_price != null ? `$${chart.reference_price.toFixed(2)}` : "—"}</strong></div>
        <div className="chart-topbar-item"><span>Replay frame</span><strong>{activeFrame ? `${selectedFrameIndex + 1} / ${replay?.frames.length ?? 0}` : "—"}</strong></div>
        <div className="chart-topbar-item"><span>Active event</span><strong>{activeFrame?.event_type ?? "—"}</strong></div>
        <div className="chart-topbar-item"><span>Trace state</span><strong>{trace?.current_status ?? "—"}</strong></div>
      </div>

      <div className="execution-chart-wrap">
        <svg viewBox={`0 0 ${width} ${height}`} className="execution-chart-svg" role="img" aria-label="Execution chart with event overlays">
          <rect x="0" y="0" width={width} height={height} rx="12" className="chart-bg" />

          {[0, 1, 2, 3, 4].map((tick) => {
            const price = minPrice + (priceRange * tick) / 4;
            const y = yForPrice(price);
            return (
              <g key={tick}>
                <line x1={padding.left} x2={width - padding.right} y1={y} y2={y} className="chart-gridline" />
                <text x={width - 2} y={y - 2} className="chart-axis-label" textAnchor="end">{price.toFixed(2)}</text>
              </g>
            );
          })}

          {chart.reference_price != null ? (
            <line x1={padding.left} x2={width - padding.right} y1={yForPrice(chart.reference_price)} y2={yForPrice(chart.reference_price)} className="chart-reference-line" />
          ) : null}

          {bars.map((bar, index) => {
            const x = xForIndex(index);
            const wickTop = yForPrice(bar.high);
            const wickBottom = yForPrice(bar.low);
            const openY = yForPrice(bar.open);
            const closeY = yForPrice(bar.close);
            const bodyTop = Math.min(openY, closeY);
            const bodyHeight = Math.max(Math.abs(closeY - openY), 2);
            const isUp = bar.close >= bar.open;
            return (
              <g key={bar.ts}>
                <line x1={x} x2={x} y1={wickTop} y2={wickBottom} className="chart-wick" />
                <rect x={x - step * 0.22} y={bodyTop} width={step * 0.44} height={bodyHeight} className={isUp ? "chart-body up" : "chart-body down"} />
                {index % 3 === 0 ? <text x={x} y={height - 10} className="chart-axis-label" textAnchor="middle">{new Date(bar.ts).toLocaleTimeString([], { hour: "2-digit", minute: "2-digit" })}</text> : null}
              </g>
            );
          })}

          {fillPathPoints ? <polyline points={fillPathPoints} className="chart-fill-path" /> : null}

          {overlays.map((overlay) => {
            const idx = indexByTs.get(overlay.ts);
            if (idx == null) return null;
            const x = xForIndex(idx);
            const y = yForPrice(overlay.price);
            const isActive = linkedOverlayIds.has(overlay.overlay_id);
            return (
              <g key={overlay.overlay_id} className={`overlay-kind-${overlay.kind} ${isActive ? "is-active" : ""}`}>
                <circle cx={x} cy={y} r={isActive ? 6 : 4.2} className={`chart-overlay-point severity-${overlay.severity}`} />
                <text x={x + 7} y={y - 8} className="chart-overlay-label">{overlayLabelByKind[overlay.kind] ?? overlay.label}</text>
              </g>
            );
          })}
        </svg>
      </div>

      {replay ? (
        <div className="chart-frame-selector">
          {replay.frames.map((frame, index) => (
            <button
              key={frame.frame_id}
              type="button"
              className={`frame-chip ${index === selectedFrameIndex ? "is-active" : ""} ${frame.anomaly_tags.length > 0 ? "is-alert" : ""}`}
              onClick={() => onSelectFrame(index)}
            >
              {index + 1}. {frame.event_type}
            </button>
          ))}
        </div>
      ) : null}

      <div className="overlay-legend-grid">
        <div className="legend-section">
          <strong>Institutional execution overlays</strong>
          <div className="legend-items">
            {["signal", "intent", "risk_pass", "order_sent", "broker_ack", "partial_fill", "reconcile", "portfolio", "expected_price"].map((kind) => (
              <span key={kind} className={`legend-pill overlay-kind-${kind}`}>{overlayLabelByKind[kind]}</span>
            ))}
          </div>
        </div>
        <div className="legend-section">
          <strong>Selected frame</strong>
          <div className="summary-detail">{activeFrame ? `${formatDateTime(activeFrame.timestamp)} · ${activeFrame.message_digest}` : "Replay not loaded"}</div>
          {activeFrame?.anomaly_tags.length ? <div className="summary-detail">Anomalies: {activeFrame.anomaly_tags.join(", ")}</div> : null}
        </div>
      </div>
    </Panel>
  );
}
