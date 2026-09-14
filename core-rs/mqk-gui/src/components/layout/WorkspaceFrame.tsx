import type { ReactNode } from "react";
import { SourceAuthorityBadge } from "../common/SourceAuthorityBadge";
import type { SourceAuthority } from "../../features/system/types";

export function WorkspaceFrame({
  title,
  description,
  authority,
  panelKey,
  onDetach,
  children,
}: {
  title: string;
  description: string;
  authority: SourceAuthority;
  panelKey: string;
  /** GUI-LAYOUT-03: present only for a detachable Tier-2 panel with a live workstation action to move it into its own window. */
  onDetach?: () => void;
  children: ReactNode;
}) {
  return (
    <section className="workspace-frame card">
      <div className="panel-head">
        <div>
          <h3>{title}</h3>
          <p className="panel-subtitle">{description}</p>
        </div>
        <div className="panel-head-actions">
          {onDetach ? (
            <button type="button" className="action-button ghost panel-detach-button" onClick={onDetach} title="Move to a new window">
              Detach
            </button>
          ) : null}
          <SourceAuthorityBadge authority={authority} panelKey={panelKey} />
        </div>
      </div>

      <div className="workspace-body">
        {children}
      </div>
    </section>
  );
}
