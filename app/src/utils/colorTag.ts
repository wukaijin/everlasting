/**
 * D1: 8-color medium-saturation palette for session color tags.
 * Index 0-7 stored in DB as `color_tag INTEGER`.
 * NULL = no mark.
 */

export const COLOR_PALETTE = [
  "#d4826a", // 0 warm orange-brown
  "#6a9e7e", // 1 forest green
  "#6a82b5", // 2 sky blue
  "#b56a9e", // 3 plum pink
  "#8eb56a", // 4 apple green
  "#6ab5ae", // 5 teal
  "#b5a06a", // 6 amber gold
  "#9e6ab5", // 7 lavender purple
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
