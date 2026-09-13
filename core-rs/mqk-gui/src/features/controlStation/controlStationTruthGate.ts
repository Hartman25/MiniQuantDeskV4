// core-rs/mqk-gui/src/features/controlStation/controlStationTruthGate.ts
//
// GUI-CS-01C: Control Station's own top-level truth disposition. This is the
// single source ControlStationScreen consumes to decide whether to hard-block
// the whole page, surface a compromised-but-present banner, or render
// normally. It does not duplicate truthRendering.ts classification — it only
// interprets the one TruthRenderState panelTruthRenderState("controlStation")
// already produces.

import type { SystemModel } from "../system/types";
import { isTruthHardBlock, panelTruthRenderState, type TruthRenderState } from "../system/truthRendering";

export type ControlStationDisposition =
  | { kind: "healthy" }
  | { kind: "compromised"; state: TruthRenderState }
  | { kind: "hard_block"; state: TruthRenderState };

export function controlStationDisposition(model: SystemModel): ControlStationDisposition {
  const state = panelTruthRenderState(model, "controlStation");
  if (state === null) return { kind: "healthy" };
  if (isTruthHardBlock(state)) return { kind: "hard_block", state };
  return { kind: "compromised", state };
}
