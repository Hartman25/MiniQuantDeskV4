import { Panel } from "../../components/common/Panel";
import { StatCard } from "../../components/common/StatCard";
import { TruthStateBanner } from "../../components/common/TruthStateBanner";
import { TruthStateNotice } from "../../components/common/TruthStateNotice";
import type { MetricsSection, MetricSeries, SystemModel } from "../system/types";
import { MetricStripChart } from "../execution/components/MetricStripChart";
import { isTruthHardBlock, panelTruthRenderState } from "../system/truthRendering";
import { formatMetricValue } from "../../lib/format";

type DomainTone = "good" | "warn" | "bad";

function domainHealth(section: MetricsSection): {
  tone: DomainTone;
  warnCount: number;
  critCount: number;
} {
  let warnCount = 0;
  let critCount = 0;
  for (const s of section.series) {
    if (s.threshold_critical !== null && s.current_value >= s.threshold_critical) {
      critCount++;
    } else if (s.threshold_warning !== null && s.current_value >= s.threshold_warning) {
      warnCount++;
    }
  }
  const tone: DomainTone = critCount > 0 ? "bad" : warnCount > 0 ? "warn" : "good";
  return { tone, warnCount, critCount };
}

function domainHealthDetail(warnCount: number, critCount: number): string {
  if (critCount === 0 && warnCount === 0) return "All series within bounds";
  const parts: string[] = [];
  if (critCount > 0) parts.push(`${critCount} critical`);
  if (warnCount > 0) parts.push(`${warnCount} warning`);
  return parts.join(", ");
}

type PressureSeries = {
  domainLabel: string;
  series: MetricSeries;
  level: "warning" | "critical";
};

function buildPressureSeries(
  domains: { label: string; section: MetricsSection }[]
): PressureSeries[] {
  const out: PressureSeries[] = [];
  for (const { label, section } of domains) {
    for (const s of section.series) {
      if (s.threshold_critical !== null && s.current_value >= s.threshold_critical) {
        out.push({ domainLabel: label, series: s, level: "critical" });
      } else if (s.threshold_warning !== null && s.current_value >= s.threshold_warning) {
        out.push({ domainLabel: label, series: s, level: "warning" });
      }
    }
  }
  return out;
}

export function MetricsScreen({ model }: { model: SystemModel }) {
  const truthState = panelTruthRenderState(model, "metrics");

  // Hard-block when truth is structurally absent. For stale/degraded, cached telemetry
  // is still useful — show the domain body with a warning banner.
  if (truthState !== null && isTruthHardBlock(truthState)) {
    return <TruthStateNotice state={truthState} />;
  }

  const { metrics } = model;

  const allDomains = [
    { key: "runtime", label: "Runtime", section: metrics.runtime },
    { key: "execution", label: "Execution", section: metrics.execution },
    { key: "fill-quality", label: "Fill Quality", section: metrics.fillQuality },
    { key: "reconciliation", label: "Reconciliation", section: metrics.reconciliation },
    { key: "risk-safety", label: "Risk & Safety", section: metrics.riskSafety },
  ];

  const pressureSeries = buildPressureSeries(allDomains);

  return (
    <div className="screen-grid desk-screen-grid">
      {truthState !== null && <TruthStateBanner state={truthState} />}
      {/* Cross-domain health summary — which domain is degraded at a glance.
          Runtime and execution are included here for comparison even though their
          strip charts live on Dashboard and Execution respectively. */}
      <div className="summary-grid summary-grid-five">
        {allDomains.map(({ key, label, section }) => {
          const { tone, warnCount, critCount } = domainHealth(section);
          const value = tone === "good" ? "OK" : tone === "warn" ? "Warning" : "Critical";
          return (
            <StatCard
              key={key}
              title={label}
              value={value}
              detail={domainHealthDetail(warnCount, critCount)}
              tone={tone}
            />
          );
        })}
      </div>

      {/* Domains under pressure — triage surface across all telemetry domains.
          Lists every series currently crossing a threshold. Not available on any other screen. */}
      <Panel
        title="Domains under pressure"
        subtitle="Series currently crossing warning or critical thresholds across all telemetry domains."
      >
        {pressureSeries.length === 0 ? (
          <div className="empty-state">All telemetry domains within bounds.</div>
        ) : (
          <div className="metric-list">
            {pressureSeries.map(({ domainLabel, series, level }) => (
              <div key={`${domainLabel}-${series.key}`}>
                <span>
                  <span className="eyebrow">{domainLabel}</span>
                  {" — "}
                  {series.label}
                </span>
                <strong style={{ color: level === "critical" ? "var(--critical)" : "var(--warning)" }}>
                  {formatMetricValue(series)}
                </strong>
              </div>
            ))}
          </div>
        )}
      </Panel>

      {/* Telemetry domain detail panels.
          Runtime strip charts are on Dashboard; execution strip charts are on ExecutionScreen.
          MetricsScreen owns the three domains not surfaced inline on other screens. */}
      <div className="desk-panel-grid desk-panel-grid-primary">
        <Panel title={metrics.fillQuality.title} subtitle={metrics.fillQuality.description}>
          <div className="metrics-grid">
            {metrics.fillQuality.series.map((series) => (
              <MetricStripChart key={series.key} series={series} />
            ))}
          </div>
        </Panel>

        <Panel title={metrics.reconciliation.title} subtitle={metrics.reconciliation.description}>
          <div className="metrics-grid">
            {metrics.reconciliation.series.map((series) => (
              <MetricStripChart key={series.key} series={series} />
            ))}
          </div>
        </Panel>
      </div>

      <Panel title={metrics.riskSafety.title} subtitle={metrics.riskSafety.description}>
        <div className="metrics-grid">
          {metrics.riskSafety.series.map((series) => (
            <MetricStripChart key={series.key} series={series} />
          ))}
        </div>
      </Panel>
    </div>
  );
}
