import type { SourceAuthorityDetail } from "../../features/system/types";
import { sourceAuthorityLabel } from "../../features/system/sourceAuthority";

export function SourceAuthorityBadge({ detail }: { detail: SourceAuthorityDetail }) {
  return (
    <div className={`source-authority source-${detail.authority}`}>
      <span className="source-authority-label">Source of truth</span>
      <strong>{sourceAuthorityLabel(detail.authority)}</strong>
      <span className="source-authority-note">{detail.note}</span>
    </div>
  );
}
