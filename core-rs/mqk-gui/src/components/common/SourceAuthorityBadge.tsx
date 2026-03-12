import type { SourceAuthority } from "../../features/system/types";

const AUTHORITY_LABELS: Record<SourceAuthority, string> = {
  db_truth: "DB truth",
  runtime_memory: "Runtime memory",
  broker_snapshot: "Broker snapshot",
  placeholder: "Placeholder",
  mixed: "Mixed",
  unknown: "Unknown",
};

export function SourceAuthorityBadge({ authority, panelKey }: { authority: SourceAuthority; panelKey: string }) {
  const label = AUTHORITY_LABELS[authority];

  return (
    <span
      className={`source-authority-badge source-authority-${authority}`}
      data-testid={`source-authority-${panelKey}`}
      aria-label={`Source authority for ${panelKey}: ${label}`}
      title={`Source authority: ${label}`}
    >
      Source: {label}
    </span>
  );
}
