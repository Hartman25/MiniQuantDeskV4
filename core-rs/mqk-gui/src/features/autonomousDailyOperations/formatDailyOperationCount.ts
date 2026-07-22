// core-rs/mqk-gui/src/features/autonomousDailyOperations/formatDailyOperationCount.ts
//
// AUTONOMOUS-DAILY-PAPER-OPERATIONS-01F1: pure formatter for the three
// full-run-lineage activity counts (strategy_evaluation_count,
// order_activity_count, fill_count). `null` means the lineage/count read was
// unavailable — it must render as an explicit "Unavailable" marker, never as
// "0". A real numeric 0 is authoritative and renders as "0".

export function formatDailyOperationCount(value: number | null): string {
  return value === null ? "Unavailable" : String(value);
}
