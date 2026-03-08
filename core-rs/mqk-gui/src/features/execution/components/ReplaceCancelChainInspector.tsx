import { DataTable } from "../../../components/common/DataTable";
import { Panel } from "../../../components/common/Panel";
import { formatDateTime } from "../../../lib/format";
import type { ReplaceCancelChainRow } from "../../system/types";

export function ReplaceCancelChainInspector({ chains }: { chains: ReplaceCancelChainRow[] }) {
  return (
    <Panel title="Replace / cancel chain inspector" subtitle="Parent-child order identity and replace/cancel race visibility.">
      <DataTable
        rows={chains}
        rowKey={(row) => row.chain_id}
        columns={[
          { key: "chain", title: "Chain", render: (row) => row.chain_id },
          { key: "root", title: "Root Order", render: (row) => row.root_order_id },
          { key: "current", title: "Current Order", render: (row) => row.current_order_id },
          { key: "action", title: "Action", render: (row) => row.action_type },
          { key: "status", title: "Status", render: (row) => row.status },
          { key: "target", title: "Target", render: (row) => row.target_order_id },
          { key: "req", title: "Requested", render: (row) => formatDateTime(row.request_at) },
          { key: "ack", title: "Ack", render: (row) => formatDateTime(row.ack_at) },
          { key: "notes", title: "Notes", render: (row) => row.notes },
        ]}
      />
    </Panel>
  );
}
