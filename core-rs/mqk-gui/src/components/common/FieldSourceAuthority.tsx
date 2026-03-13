import { SourceAuthorityBadge } from "./SourceAuthorityBadge";
import type { SourceAuthority } from "../../features/system/types";

export function FieldSourceAuthority({ fieldKey, authority }: { fieldKey: string; authority: SourceAuthority }) {
  return (
    <span className="field-source-authority" data-testid={`field-source-${fieldKey}`}>
      <SourceAuthorityBadge authority={authority} panelKey={`field-${fieldKey}`} />
    </span>
  );
}
