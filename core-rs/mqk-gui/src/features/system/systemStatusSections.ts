export const SYSTEM_STATUS_SECTION_IDS = [
  "operations-metadata",
  "instrument-registry-v2-source",
  "asset-capability-matrix",
] as const;

export type SystemStatusSectionId = (typeof SYSTEM_STATUS_SECTION_IDS)[number];

export function systemStatusScreenIncludes(section: SystemStatusSectionId): boolean {
  return SYSTEM_STATUS_SECTION_IDS.includes(section);
}
