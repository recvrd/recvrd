import { describe, expect, it } from 'vitest';
import { MENU_EDGE_MARGIN, clampMenuPosition } from './menu-position';

const VIEWPORT_WIDTH = 1024;
const VIEWPORT_HEIGHT = 768;

function clamp(x: number, y: number, width = 200, height = 300) {
  return clampMenuPosition(x, y, width, height, VIEWPORT_WIDTH, VIEWPORT_HEIGHT);
}

describe('clampMenuPosition', () => {
  it('keeps a menu that fits at the requested position', () => {
    expect(clamp(100, 100)).toEqual({ left: 100, top: 100 });
  });

  it('moves a menu left at the right edge', () => {
    expect(clamp(1000, 100)).toEqual({
      left: VIEWPORT_WIDTH - 200 - MENU_EDGE_MARGIN,
      top: 100,
    });
  });

  it('moves a menu up at the bottom edge', () => {
    expect(clamp(100, 700)).toEqual({
      left: 100,
      top: VIEWPORT_HEIGHT - 300 - MENU_EDGE_MARGIN,
    });
  });

  it('moves a menu back to the margin at the top and left edges', () => {
    expect(clamp(-20, -10)).toEqual({
      left: MENU_EDGE_MARGIN,
      top: MENU_EDGE_MARGIN,
    });
  });

  it('uses the margin when the menu is wider than the viewport', () => {
    const position = clamp(100, 100, VIEWPORT_WIDTH + 50, VIEWPORT_HEIGHT + 50);

    expect(position).toEqual({
      left: MENU_EDGE_MARGIN,
      top: MENU_EDGE_MARGIN,
    });
  });
});
