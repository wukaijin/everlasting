/**
 * D1: 8-color low-saturation palette for session color tags.
 * Index 0-7 stored in DB as `color_tag INTEGER`.
 * NULL = no mark.
 */

export const COLOR_PALETTE = [
  "#8b7355", // 0 warm brown
  "#7a8b6f", // 1 sage green
  "#6b7d8e", // 2 slate blue
  "#8e6b7d", // 3 dusty rose
  "#7d8e6b", // 4 olive
  "#6b7d7a", // 5 teal gray
  "#8e7d6b", // 6 amber brown
  "#6b6b8e", // 7 muted purple
] as const;

export type ColorTagIndex = 0 | 1 | 2 | 3 | 4 | 5 | 6 | 7;

/** Get hex color for a tag index, or null if out of range / null. */
export function colorTagHex(tag: number | null): string | null {
  if (tag === null || tag < 0 || tag >= COLOR_PALETTE.length) return null;
  return COLOR_PALETTE[tag];
}

/** Convert a hex color to rgba with given alpha (0-1). */
export function hexToRgba(hex: string, alpha: number): string {
  const r = parseInt(hex.slice(1, 3), 16);
  const g = parseInt(hex.slice(3, 5), 16);
  const b = parseInt(hex.slice(5, 7), 16);
  return `rgba(${r}, ${g}, ${b}, ${alpha})`;
}
