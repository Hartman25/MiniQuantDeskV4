import type { ReactNode } from "react";

interface WorkspaceFrameProps {
  title: string;
  description: string;
  children: ReactNode;
}

export function WorkspaceFrame({ title, description, children }: WorkspaceFrameProps) {
  return (
    <div className="workspace-frame">
      <div className="workspace-header panel">
        <div>
          <div className="eyebrow">Primary Workspace</div>
          <h2>{title}</h2>
        </div>
        <p>{description}</p>
      </div>
      <div className="workspace-body">{children}</div>
    </div>
  );
}
