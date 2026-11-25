/** The space between a menu and the viewport edge, in pixels. */
export const MENU_EDGE_MARGIN = 8;

export interface MenuPosition {
  left: number;
  top: number;
}

/**
 * Keep a fixed-position menu inside the viewport.
 *
 * A menu that fits near the right or bottom edge moves left or up. A menu
 * that would leave the viewport at the top or left moves back to the
 * margin.
 */
export function clampMenuPosition(
  x: number,
  y: number,
  width: number,
  height: number,
  viewportWidth: number,
  viewportHeight: number,
  margin: number = MENU_EDGE_MARGIN,
): MenuPosition {
  const maxLeft = Math.max(margin, viewportWidth - width - margin);
  const maxTop = Math.max(margin, viewportHeight - height - margin);

  return {
    left: Math.min(Math.max(x, margin), maxLeft),
    top: Math.min(Math.max(y, margin), maxTop),
  };
}
