import { DataTable } from "../../components/common/DataTable";
import { Panel } from "../../components/common/Panel";
import { StatCard } from "../../components/common/StatCard";
import { TruthStateNotice } from "../../components/common/TruthStateNotice";
import { formatDateTime } from "../../lib/format";
import { panelTruthRenderState } from "../system/truthRendering";
import type { SystemModel } from "../system/types";

function resultStateClass(result: string): string {
  if (result === "ok" || result === "success") return "state-ok";
  if (result === "blocked" || result === "denied" || result === "rejected" || result === "error" || result === "failed") return "state-critical";
  if (result === "partial" || result === "warn") return "state-warning";
  return "val-muted";
}

function feedSeverityClass(severity: string): string {
  if (severity === "critical") return "state-critical";
  if (severity === "warning") return "state-warning";
  return "";
}

export function AuditScreen({ model }: { model: SystemModel }) {
  const truthState = panelTruthRenderState(model, "audit");

  if (truthState !== null) {
    return <TruthStateNotice state={truthState} />;
  }

  return (
    <div className="screen-grid desk-screen-grid">
      <div className="summary-grid summary-grid-four">
        <StatCard title="Audit Actions" value={String(model.auditActions.length)} detail="Recent operator receipts" tone="good" />
        <StatCard title="Feed Events" value={String(model.feed.length)} detail="Recent structured system events" tone="neutral" />
        <StatCard title="Source State" value={model.dataSource.state} detail="Truth model state" tone={model.dataSource.state === "real" ? "good" : model.dataSource.state === "partial" ? "warn" : "bad"} />
        <StatCard title="Connected" value={model.connected ? "Yes" : "No"} detail="Daemon reachability" tone={model.connected ? "good" : "bad"} />
      </div>

      <Panel title="Audit actions" subtitle="Structured operator actions and receipts.">
        <DataTable
          rows={model.auditActions}
          rowKey={(row) => row.audit_ref}
          columns={[
            { key: "at", title: "At", render: (row) => formatDateTime(row.at) },
            { key: "actor", title: "Actor", render: (row) => row.actor ?? "—" },
            { key: "action", title: "Action", render: (row) => row.action_key },
            { key: "environment", title: "Env", render: (row) => row.environment ?? "—" },
            { key: "scope", title: "Scope", render: (row) => row.target_scope ?? "—" },
            { key: "result", title: "Result", render: (row) => <span className={resultStateClass(row.result_state)}>{row.result_state}</span> },
            { key: "warnings", title: "Warnings", render: (row) => row.warnings.length > 0 ? <span className="state-warning">{row.warnings.join(", ")}</span> : "—" },
          ]}
        />
      </Panel>

      <Panel title="Event feed" subtitle="Recent structured events suitable for operator forensics.">
        <DataTable
          rows={model.feed}
          rowKey={(row) => row.id}
          columns={[
            { key: "at", title: "At", render: (row) => formatDateTime(row.at) },
            { key: "severity", title: "Sev", render: (row) => <span className={feedSeverityClass(row.severity)}>{row.severity}</span> },
            { key: "source", title: "Source", render: (row) => row.source },
            { key: "text", title: "Text", render: (row) => row.text },
          ]}
        />
      </Panel>
    </div>
  );
}
