import { formatMetricValue } from "../../../lib/format";
import type { MetricSeries } from "../../system/types";

export function MetricStripChart({ series }: { series: MetricSeries }) {
  const max = Math.max(...series.points.map((point) => point.value), 1);

  return (
    <div className="metric-strip-card">
      <div className="metric-strip-header">
        <div>
          <div className="eyebrow">{series.window}</div>
          <strong>{series.label}</strong>
        </div>
        <span>{formatMetricValue(series)}</span>
      </div>
      <div className="sparkline-bars" aria-hidden="true">
        {series.points.map((point) => (
          <span
            key={`${series.key}-${point.ts}`}
            className={`sparkline-bar ${series.threshold_critical !== null && point.value >= series.threshold_critical ? "critical" : series.threshold_warning !== null && point.value >= series.threshold_warning ? "warning" : "normal"}`}
            style={{ height: `${Math.max(12, (point.value / max) * 100)}%` }}
            title={`${point.value}`}
          />
        ))}
      </div>
    </div>
  );
}
