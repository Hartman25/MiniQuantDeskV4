export function StatCard({ title, value, detail, tone = "neutral" }: { title: string; value: string; detail?: string; tone?: "neutral" | "good" | "warn" | "bad" }) {
  return (
    <div className={`summary-card panel stat-${tone}`.trim()}>
      <div className="eyebrow">{title}</div>
      <div className="summary-value">{value}</div>
      {detail ? <div className="summary-detail">{detail}</div> : null}
    </div>
  );
}
