import { formatLabel } from "../../lib/format";
import type { Severity } from "../../features/system/types";

interface StatusPillProps {
  label: string;
  value: string | null | undefined;
  tone: Severity;
  emphasis?: "normal" | "loud";
}

export function StatusPill({ label, value, tone, emphasis = "normal" }: StatusPillProps) {
  return (
    <div className={`status-pill tone-${tone} emphasis-${emphasis}`}>
      <span className="status-pill-label">{label}</span>
      <span className="status-pill-value">{formatLabel(value)}</span>
    </div>
  );
}