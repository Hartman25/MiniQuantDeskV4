import type { TruthRenderState } from "../../features/system/truthRendering";
import { truthStateCopy } from "../../features/system/truthRendering";

export function TruthStateNotice({ state }: { state: TruthRenderState }) {
  const copy = truthStateCopy(state);

  return (
    <div className={`empty-state truth-state-notice truth-state-${state}`} role="status" aria-live="polite">
      <strong>{copy.title}</strong>
      <div>{copy.detail}</div>
    </div>
  );
}
