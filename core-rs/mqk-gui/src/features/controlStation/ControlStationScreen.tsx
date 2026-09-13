// core-rs/mqk-gui/src/features/controlStation/ControlStationScreen.tsx
//
// GUI-CS-01B: read-only operator-workstation dashboard built entirely on the
// GUI-CS-01A view model. No new backend contracts; existing navigation,
// Panel/StatCard/TruthStateBanner conventions and val-ok/val-warn/val-critical
// tone classes (see DashboardScreen) reused as-is.

import type { ReactNode } from "react";
import { Panel } from "../../components/common/Panel";
import { StatCard } from "../../components/common/StatCard";
import { TruthStateBanner } from "../../components/common/TruthStateBanner";
import { TruthStateNotice } from "../../components/common/TruthStateNotice";
import { formatDateTime, formatLabel } from "../../lib/format";
import type { HealthState, SystemModel } from "../system/types";
import { isTruthHardBlock, panelTruthRenderState, type TruthRenderState } from "../system/truthRendering";
import { buildControlStationViewModel, healthTone, type CsTone } from "./viewModel";

function csToneToStatTone(tone: CsTone): "neutral" | "good" | "warn" | "bad" {
  return tone === "unknown" ? "neutral" : tone;
}

function csToneToClass(tone: CsTone): string {
  switch (tone) {
    case "good":
      return "val-ok";
    case "warn":
      return "val-warn";
    case "bad":
      return "val-critical";
    case "unknown":
      return "val-muted";
  }
}

function healthClass(state: HealthState): string {
  return csToneToClass(healthTone(state));
}

function boolLabel(value: boolean | null, whenTrue: string, whenFalse: string): string {
  if (value === null) return "Unknown";
  return value ? whenTrue : whenFalse;
}

function boolClass(value: boolean, badWhenTrue = true): string {
  if (badWhenTrue) return value ? "val-critical" : "val-ok";
  return value ? "val-ok" : "val-muted";
}

/** Renders a truth banner above section content; hard-block states replace the content entirely. */
function TruthGatedSection({ truth, children }: { truth: TruthRenderState | null; children: ReactNode }) {
  if (truth && isTruthHardBlock(truth)) {
    return <TruthStateBanner state={truth} />;
  }
  return (
    <>
      {truth ? <TruthStateBanner state={truth} /> : null}
      {children}
    </>
  );
}

export function ControlStationScreen({ model }: { model: SystemModel }) {
  // A fully unreachable daemon leaves nothing on this page trustworthy —
  // hard-block the whole station rather than render any section. Any other
  // truth state (stale/degraded/no_snapshot/unimplemented) is handled per
  // section below so a partial degradation stays visibly distinct instead of
  // hiding the whole workstation.
  if (panelTruthRenderState(model, "dashboard") === "unavailable") {
    return <TruthStateNotice state="unavailable" />;
  }

  const vm = buildControlStationViewModel(model);

  return (
    <div className="screen-grid desk-screen-grid">
      <div className="summary-grid summary-grid-four">
        <StatCard
          title="Daemon"
          value={vm.system.daemonOnline ? "Online" : "Offline"}
          detail={`Environment ${formatLabel(vm.system.environment)}`}
          tone={vm.system.daemonOnline ? "good" : "bad"}
        />
        <StatCard
          title="System health"
          value={vm.system.tone === "good" ? "Healthy" : formatLabel(vm.system.tone)}
          detail={`Reconcile ${formatLabel(vm.system.reconcileStatus)}`}
          tone={csToneToStatTone(vm.system.tone)}
        />
        <StatCard
          title="Runtime"
          value={formatLabel(vm.tradingDomain.runtimeStatus)}
          detail={`Mode ${formatLabel(vm.tradingDomain.daemonMode)} / ${formatLabel(vm.tradingDomain.adapterId)}`}
          tone={csToneToStatTone(vm.tradingDomain.tone)}
        />
        <StatCard
          title="Live routing"
          value={boolLabel(vm.tradingDomain.liveRoutingEnabled, "ENABLED", "Disabled")}
          detail={vm.tradingDomain.environment === "live" ? "Live environment" : "Paper environment"}
          tone={vm.tradingDomain.liveRoutingEnabled ? "bad" : "good"}
        />
      </div>

      <Panel title="System" subtitle="Daemon, DB, broker, WS, reconcile, and integrity/risk truth.">
        <div className="metric-list">
          <div><span>Daemon reachable</span><strong className={boolClass(!vm.system.daemonOnline)}>{boolLabel(vm.system.daemonOnline, "Yes", "No")}</strong></div>
          <div><span>DB status</span><strong className={healthClass(vm.system.dbStatus)}>{formatLabel(vm.system.dbStatus)}</strong></div>
          <div><span>Broker status</span><strong className={healthClass(vm.system.brokerStatus)}>{formatLabel(vm.system.brokerStatus)}</strong></div>
          <div><span>Market data health</span><strong className={healthClass(vm.system.marketDataHealth)}>{formatLabel(vm.system.marketDataHealth)}</strong></div>
          <div><span>Reconcile status</span><strong className={healthClass(vm.system.reconcileStatus)}>{formatLabel(vm.system.reconcileStatus)}</strong></div>
          <div><span>Integrity status</span><strong className={healthClass(vm.system.integrityStatus)}>{formatLabel(vm.system.integrityStatus)}</strong></div>
          <div><span>WS continuity</span><strong>{formatLabel(vm.system.wsContinuity)}</strong></div>
          <div><span>Kill switch</span><strong className={boolClass(vm.system.killSwitchActive)}>{boolLabel(vm.system.killSwitchActive, "Active", "Inactive")}</strong></div>
          <div><span>Integrity halt</span><strong className={boolClass(vm.system.integrityHaltActive)}>{boolLabel(vm.system.integrityHaltActive, "Active", "Inactive")}</strong></div>
          <div><span>Risk halt</span><strong className={boolClass(vm.system.riskHaltActive)}>{boolLabel(vm.system.riskHaltActive, "Active", "Inactive")}</strong></div>
          <div><span>Deadman watchdog</span><strong className={vm.system.deadmanStatus === "ok" ? "val-ok" : vm.system.deadmanStatus === "expired" ? "val-warn" : "val-muted"}>{formatLabel(vm.system.deadmanStatus)}</strong></div>
        </div>
      </Panel>

      <div className="two-column-grid">
        <Panel title="Trading domain" subtitle="Paper/Live, armed state, runtime, and session window.">
          <TruthGatedSection truth={vm.tradingDomain.sessionTruth}>
            <div className="metric-list">
              <div><span>Environment</span><strong className={boolClass(vm.tradingDomain.environment === "live")}>{formatLabel(vm.tradingDomain.environment)}</strong></div>
              <div><span>Strategy armed</span><strong>{boolLabel(vm.tradingDomain.strategyArmed, "Armed", "Disarmed")}</strong></div>
              <div><span>Execution armed</span><strong>{boolLabel(vm.tradingDomain.executionArmed, "Armed", "Disarmed")}</strong></div>
              <div><span>Market session</span><strong>{formatLabel(vm.tradingDomain.marketSession)}</strong></div>
              <div><span>Trading window</span><strong>{formatLabel(vm.tradingDomain.tradingWindow)}</strong></div>
            </div>
          </TruthGatedSection>
        </Panel>

        <Panel title="Incidents" subtitle="Open cases and latest halt truth.">
          <TruthGatedSection truth={vm.incidents.incidentsTruth}>
            <div className="metric-list">
              <div><span>Open incidents</span><strong className={vm.incidents.openIncidentCount > 0 ? "val-warn" : "val-ok"}>{vm.incidents.openIncidentCount}</strong></div>
              <div><span>Total incidents</span><strong>{vm.incidents.totalIncidentCount}</strong></div>
              <div><span>Halt status</span><strong className={vm.incidents.haltSeverity === "critical" ? "val-critical" : vm.incidents.haltSeverity === "warning" ? "val-warn" : "val-ok"}>{formatLabel(vm.incidents.haltStatus)}</strong></div>
              <div><span>Halt reason</span><strong>{vm.incidents.haltReason ?? "—"}</strong></div>
            </div>
          </TruthGatedSection>
        </Panel>
      </div>

      <Panel title="Portfolio / open orders" subtitle="Broker-snapshot truth vs active-session execution state — never collapsed together.">
        <div className="two-column-grid">
          <div>
            <p className="panel-subtitle">Broker snapshot (portfolio truth)</p>
            <TruthGatedSection truth={vm.portfolio.portfolioTruth}>
              <div className="metric-list compact-list">
                <div><span>Broker positions</span><strong>{vm.portfolio.brokerPositionCount}</strong></div>
                <div><span>Broker open orders</span><strong>{vm.portfolio.brokerOpenOrderCount}</strong></div>
              </div>
            </TruthGatedSection>
          </div>
          <div>
            <p className="panel-subtitle">Active session (execution truth)</p>
            <TruthGatedSection truth={vm.portfolio.executionTruth}>
              <div className="metric-list compact-list">
                <div><span>Active orders</span><strong>{vm.portfolio.activeSessionOrderCount}</strong></div>
                <div><span>Pending orders</span><strong>{vm.portfolio.pendingSessionOrderCount}</strong></div>
                <div><span>Stuck orders</span><strong className={vm.portfolio.stuckSessionOrderCount > 0 ? "val-warn" : "val-ok"}>{vm.portfolio.stuckSessionOrderCount}</strong></div>
              </div>
            </TruthGatedSection>
          </div>
        </div>
      </Panel>

      <Panel title="Autonomy / readiness" subtitle="Autonomous session state, readiness, and next operator action.">
        {!vm.autonomy.applicable ? (
          <p className="panel-notice">Autonomous readiness is not applicable to the current deployment/adapter.</p>
        ) : (
          <div className="metric-list">
            <div><span>Truth state</span><strong>{formatLabel(vm.autonomy.truthState)}</strong></div>
            <div><span>Arm state</span><strong>{formatLabel(vm.autonomy.armState)}</strong></div>
            <div><span>Readiness classification</span><strong>{formatLabel(vm.autonomy.readinessClassification)}</strong></div>
            <div><span>Next operator action</span><strong>{vm.autonomy.nextOperatorAction ?? "—"}</strong></div>
            <div><span>Daily operation transport</span><strong>{formatLabel(vm.autonomy.dailyOperationTransportState)}</strong></div>
            <div><span>Daily operation truth</span><strong>{formatLabel(vm.autonomy.dailyOperationTruthState)}</strong></div>
            <div><span>Finalization status</span><strong>{formatLabel(vm.autonomy.dailyOperationFinalizationStatus)}</strong></div>
            <div><span>Outcome class</span><strong>{formatLabel(vm.autonomy.dailyOperationOutcomeClass)}</strong></div>
            <div><span>Market date</span><strong>{vm.autonomy.dailyOperationMarketDate ?? "—"}</strong></div>
          </div>
        )}
        {vm.autonomy.blockers.length > 0 && (
          <div className="list-stack compact-list">
            {vm.autonomy.blockers.map((blocker) => (
              <div key={blocker} className="list-row"><span>{blocker}</span></div>
            ))}
          </div>
        )}
      </Panel>

      <Panel title="M1 validation" subtitle="Countable soak-session progress toward the 10-session / 5-clean gate.">
        <TruthStateNotice state="not_wired" />
        <p className="panel-notice">{vm.m1Validation.reason}</p>
      </Panel>

      <p className="panel-notice">Last updated: {formatDateTime(vm.lastUpdatedAt)}</p>
    </div>
  );
}
