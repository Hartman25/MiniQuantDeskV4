import type { ReactNode } from "react";

export function WorkspaceFrame({
  title,
  description,
  children,
}: {
  title: string;
  description: string;
  children: ReactNode;
}) {
  return (
    <section className="workspace-frame panel">
      <div className="panel-head">
        <div>
          <div className="eyebrow">Workspace</div>
          <h3>{title}</h3>
          <p className="panel-subtitle">{description}</p>
        </div>
      </div>

      <div className="workspace-body">
        {children}
      </div>
    </section>
  );
}
