import { Panel } from "../../components/common/Panel";
import { StatCard } from "../../components/common/StatCard";
import type { SystemModel } from "../system/types";
import { formatDateTime, formatLatency } from "../../lib/format";
import { MetricStripChart } from "../execution/components/MetricStripChart";

export function DashboardScreen({ model }: { model: SystemModel }) {
  const {
    status,
    alerts,
    preflight,
    executionSummary,
    riskSummary,
    reconcileSummary,
    positions,
    openOrders,
    fills,
  } = model;

  return (
    <div className="screen-grid desk-screen-grid">
      <div className="summary-grid summary-grid-four">
        <StatCard
          title="Runtime"
          value={status.runtime_status}
          detail={`Heartbeat ${formatDateTime(status.last_heartbeat)}`}
          tone={status.has_critical ? "bad" : status.has_warning ? "warn" : "good"}
        />
        <StatCard
          title="Daemon"
          value={model.connected ? "Connected" : "Disconnected"}
          detail={`Loop latency ${formatLatency(status.loop_latency_ms)}`}
          tone={model.connected ? "good" : "bad"}
        />
        <StatCard
          title="Open Alerts"
          value={String(alerts.length)}
          detail={`${alerts.filter((a) => a.severity === "critical").length} critical`}
          tone={alerts.some((a) => a.severity === "critical") ? "bad" : alerts.length > 0 ? "warn" : "good"}
        />
        <StatCard
          title="Preflight"
          value={preflight.blockers.length > 0 ? "Blocked" : "Clear"}
          detail={`${preflight.warnings.length} warnings`}
          tone={preflight.blockers.length > 0 ? "bad" : preflight.warnings.length > 0 ? "warn" : "good"}
        />
      </div>

      <div className="metrics-grid desk-panel-row">
        {model.metrics.runtime.series.map((series) => (
          <MetricStripChart key={series.key} series={series} />
        ))}
      </div>

      <div className="desk-panel-grid desk-panel-grid-primary">
        <Panel title="Session posture" compact>
          <div className="metric-list compact-list">
            <div><span>Market session</span><strong>{model.sessionState.market_session}</strong></div>
            <div><span>Trading window</span><strong>{model.sessionState.system_trading_window}</strong></div>
            <div><span>Next change</span><strong>{formatDateTime(model.sessionState.next_session_change_at)}</strong></div>
          </div>
        </Panel>

        <Panel title="Config fingerprint" compact>
          <div className="metric-list compact-list">
            <div><span>Config hash</span><strong>{model.configFingerprint.config_hash}</strong></div>
            <div><span>Runtime generation</span><strong>{model.configFingerprint.runtime_generation_id}</strong></div>
            <div><span>Risk policy</span><strong>{model.configFingerprint.risk_policy_version}</strong></div>
          </div>
        </Panel>

        <Panel title="Execution pipeline summary" subtitle="System status and order-flow posture.">
          <div className="metric-list">
            <div><span>Strategy state</span><strong>{status.strategy_armed ? "Armed" : "Disarmed"}</strong></div>
            <div><span>Execution state</span><strong>{status.execution_armed ? "Armed" : "Disarmed"}</strong></div>
            <div><span>Live routing</span><strong>{status.live_routing_enabled ? "Enabled" : "Disabled"}</strong></div>
            <div><span>Active orders</span><strong>{executionSummary.active_orders}</strong></div>
            <div><span>Pending orders</span><strong>{executionSummary.pending_orders}</strong></div>
            <div><span>Stuck orders</span><strong>{executionSummary.stuck_orders}</strong></div>
          </div>
        </Panel>

        <Panel title="Risk / reconcile summary" subtitle="Hard-stop posture and drift visibility.">
          <div className="metric-list">
            <div><span>Loss-limit utilization</span><strong>{riskSummary.loss_limit_utilization_pct.toFixed(1)}%</strong></div>
            <div><span>Drawdown</span><strong>{riskSummary.drawdown_pct.toFixed(2)}%</strong></div>
            <div><span>Reconcile status</span><strong>{reconcileSummary.status}</strong></div>
            <div><span>Mismatched orders</span><strong>{reconcileSummary.mismatched_orders}</strong></div>
            <div><span>Unmatched broker events</span><strong>{reconcileSummary.unmatched_broker_events}</strong></div>
            <div><span>Kill switch</span><strong>{status.kill_switch_active ? "Active" : "Inactive"}</strong></div>
          </div>
        </Panel>
      </div>

      <Panel
        title="Desk split recommendation"
        subtitle="Two-monitor mode favors control + execution. Three-monitor mode gives risk / reconcile / audit their own dedicated view."
        compact
      >
        <div className="desk-monitor-strip">
          <div className="desk-monitor-card">
            <strong>Monitor 1</strong>
            <p>Runtime, global health, operator actions, alerts, and event supervision.</p>
          </div>
          <div className="desk-monitor-card">
            <strong>Monitor 2</strong>
            <p>Execution timelines, OMS state, traces, replay, open orders, and broker messages.</p>
          </div>
          <div className="desk-monitor-card optional-monitor">
            <strong>Monitor 3</strong>
            <p>Risk, portfolio, reconcile, incidents, audit evidence, and deeper forensics.</p>
          </div>
        </div>
      </Panel>

      <div className="desk-panel-grid desk-panel-grid-secondary">
        <Panel title="Positions snapshot">
          <div className="list-stack compact-list">
            {positions.slice(0, 5).map((position) => (
              <div key={`${position.strategy_id}-${position.symbol}`} className="list-row">
                <strong>{position.symbol}</strong>
                <span>{position.strategy_id}</span>
                <span>{position.qty} sh</span>
              </div>
            ))}
          </div>
        </Panel>

        <Panel title="Open orders snapshot">
          <div className="list-stack compact-list">
            {openOrders.slice(0, 5).map((order) => (
              <div key={order.internal_order_id} className="list-row">
                <strong>{order.symbol}</strong>
                <span>{order.status}</span>
                <span>{order.filled_qty}/{order.requested_qty}</span>
              </div>
            ))}
          </div>
        </Panel>

        <Panel title="Recent fills snapshot">
          <div className="list-stack compact-list">
            {fills.slice(0, 5).map((fill) => (
              <div key={fill.fill_id} className="list-row">
                <strong>{fill.symbol}</strong>
                <span>{fill.qty} @ {fill.price}</span>
                <span>{formatDateTime(fill.at)}</span>
              </div>
            ))}
          </div>
        </Panel>
      </div>
    </div>
  );
}
