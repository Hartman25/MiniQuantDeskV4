// GUI-LAYOUT-04: preset switcher (control window only, orchestrates sibling
// windows) + per-window saved custom layouts (available in every desk role,
// operating only on this window's own workstation).

import { useEffect, useState, type RefObject } from "react";
import type { DeskRole } from "../../app/shellTypes";
import type { WorkstationHandle } from "../../features/workstation/Workstation";
import {
  BUILTIN_PRESETS,
  customLayoutsStorageKey,
  parseCustomLayouts,
  serializeCustomLayouts,
  withRenamedCustomLayout,
  withSavedCustomLayout,
  withoutCustomLayout,
  type CustomLayout,
  type WorkstationPreset,
} from "../../features/workstation/presets";

function readCustomLayouts(role: DeskRole): CustomLayout[] {
  try {
    return parseCustomLayouts(window.localStorage.getItem(customLayoutsStorageKey(role)));
  } catch {
    return [];
  }
}

function persistCustomLayouts(role: DeskRole, layouts: CustomLayout[]): void {
  try {
    window.localStorage.setItem(customLayoutsStorageKey(role), serializeCustomLayouts(layouts));
  } catch {
    // Best-effort persistence only — never block the UI on a failed write.
  }
}

export function WorkstationPresetBar({
  deskRole,
  workstationRef,
  onApplyPreset,
}: {
  deskRole: DeskRole;
  workstationRef: RefObject<WorkstationHandle | null>;
  onApplyPreset?: (preset: WorkstationPreset) => void;
}) {
  const [customLayouts, setCustomLayouts] = useState<CustomLayout[]>([]);
  const [selectedId, setSelectedId] = useState("");

  useEffect(() => {
    setCustomLayouts(readCustomLayouts(deskRole));
  }, [deskRole]);

  const handleSaveAs = () => {
    const doc = workstationRef.current?.getCurrentLayoutDoc();
    if (!doc) return;
    const name = window.prompt("Save current layout as:", "My Layout");
    if (!name || !name.trim()) return;
    const updated = withSavedCustomLayout(customLayouts, name, doc);
    persistCustomLayouts(deskRole, updated);
    setCustomLayouts(updated);
  };

  const handleApply = () => {
    const entry = customLayouts.find((l) => l.id === selectedId);
    if (!entry) return;
    const applied = workstationRef.current?.applyLayoutDoc(entry.layout);
    if (applied === false) {
      window.alert(`"${entry.name}" could not be applied — it no longer resolves to any known panel.`);
    }
  };

  const handleRename = () => {
    const entry = customLayouts.find((l) => l.id === selectedId);
    if (!entry) return;
    const name = window.prompt("Rename saved layout:", entry.name);
    if (!name || !name.trim()) return;
    const updated = withRenamedCustomLayout(customLayouts, entry.id, name);
    persistCustomLayouts(deskRole, updated);
    setCustomLayouts(updated);
  };

  const handleDelete = () => {
    const entry = customLayouts.find((l) => l.id === selectedId);
    if (!entry) return;
    if (!window.confirm(`Delete saved layout "${entry.name}"?`)) return;
    const updated = withoutCustomLayout(customLayouts, entry.id);
    persistCustomLayouts(deskRole, updated);
    setCustomLayouts(updated);
    setSelectedId("");
  };

  return (
    <div className="workstation-preset-bar panel panel-compact">
      {deskRole === "control" && onApplyPreset ? (
        <div className="preset-bar-group" role="group" aria-label="Workstation presets">
          <span className="preset-bar-label">Preset</span>
          {Object.values(BUILTIN_PRESETS).map((preset) => (
            <button
              key={preset.id}
              type="button"
              className="action-button ghost"
              onClick={() => onApplyPreset(preset)}
              title={`Starting arrangement: ${preset.windows.map((w) => w.role).join(" + ")}`}
            >
              {preset.label}
            </button>
          ))}
        </div>
      ) : null}

      <div className="preset-bar-group" role="group" aria-label="Saved layouts">
        <span className="preset-bar-label">Saved layouts</span>
        <select
          className="preset-bar-select"
          value={selectedId}
          onChange={(e) => setSelectedId(e.target.value)}
          aria-label="Saved layout"
        >
          <option value="">— none selected —</option>
          {customLayouts.map((l) => (
            <option key={l.id} value={l.id}>
              {l.name}
            </option>
          ))}
        </select>
        <button type="button" className="action-button ghost" onClick={handleApply} disabled={!selectedId}>
          Apply
        </button>
        <button type="button" className="action-button ghost" onClick={handleRename} disabled={!selectedId}>
          Rename
        </button>
        <button type="button" className="action-button ghost" onClick={handleDelete} disabled={!selectedId}>
          Delete
        </button>
        <button type="button" className="action-button ghost" onClick={handleSaveAs}>
          Save current as…
        </button>
      </div>
    </div>
  );
}
