import type { TruthRenderState } from "../../features/system/truthRendering";
import { truthStateCopy } from "../../features/system/truthRendering";

export function TruthStateBanner({ state }: { state: TruthRenderState }) {
  const copy = truthStateCopy(state);
  return (
    <div className={`truth-state-banner truth-state-${state}`} role="status" aria-live="polite">
      <strong>{copy.title}</strong>
      <span>{copy.detail}</span>
    </div>
  );
}
