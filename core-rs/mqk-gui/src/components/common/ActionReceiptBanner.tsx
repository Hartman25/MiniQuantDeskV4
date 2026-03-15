import type { OperatorActionReceipt } from "../../features/system/types";

export function ActionReceiptBanner({ receipt }: { receipt: OperatorActionReceipt | null }) {
  if (!receipt) return null;

  return (
    <div className={`action-receipt ${receipt.ok ? "ok" : "fail"}`}>
      <strong>{receipt.action_key}</strong>
      <span>{receipt.result_state}</span>
      <span>Audit: {receipt.audit_reference}</span>
      <span>Env: {receipt.environment}</span>
            {receipt.warnings.length > 0 ? <span>Warnings: {receipt.warnings.join(" | ")}</span> : null}
      {receipt.blocking_failures.length > 0 ? <span>Blocked: {receipt.blocking_failures.join(" | ")}</span> : null}
    </div>
  );
}
