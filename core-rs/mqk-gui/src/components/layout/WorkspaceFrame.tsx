import type { ReactNode } from "react";
import { SourceAuthorityBadge } from "../common/SourceAuthorityBadge";
import type { SourceAuthority } from "../../features/system/types";

export function WorkspaceFrame({
  title,
  description,
  authority,
  panelKey,
  children,
}: {
  title: string;
  description: string;
  authority: SourceAuthority;
  panelKey: string;
  children: ReactNode;
}) {
  return (
    <section className="workspace-frame card">
      <div className="panel-head">
        <div>
          <h3>{title}</h3>
          <p className="panel-subtitle">{description}</p>
        </div>
        <SourceAuthorityBadge authority={authority} panelKey={panelKey} />
      </div>

      <div className="workspace-body">
        {children}
      </div>
    </section>
  );
}
