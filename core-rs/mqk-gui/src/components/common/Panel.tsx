import type { ReactNode } from "react";

export function Panel({ title, subtitle, children, compact = false }: { title?: string; subtitle?: string; children: ReactNode; compact?: boolean }) {
  return (
    <section className={`panel ${compact ? "panel-compact" : ""}`.trim()}>
      {(title || subtitle) && (
        <div className="panel-header compact">
          <div>
            {title ? <h3>{title}</h3> : null}
            {subtitle ? <p className="panel-subtitle">{subtitle}</p> : null}
          </div>
        </div>
      )}
      {children}
    </section>
  );
}
