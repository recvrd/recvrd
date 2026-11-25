import { render, screen, waitFor } from '@testing-library/svelte';
import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest';
import ContextMenu from './ContextMenu.svelte';

const VIEWPORT_WIDTH = 1024;
const VIEWPORT_HEIGHT = 768;
const MENU_WIDTH = 200;
const MENU_HEIGHT = 300;

let originalWidth: number;
let originalHeight: number;

function renderMenu(x: number, y: number) {
  return render(ContextMenu, {
    props: {
      x,
      y,
      items: [{ label: 'Play now', action: vi.fn() }],
      onclose: vi.fn(),
    },
  });
}

describe('ContextMenu', () => {
  beforeEach(() => {
    originalWidth = window.innerWidth;
    originalHeight = window.innerHeight;
    Object.defineProperty(window, 'innerWidth', { configurable: true, value: VIEWPORT_WIDTH });
    Object.defineProperty(window, 'innerHeight', { configurable: true, value: VIEWPORT_HEIGHT });
    vi.spyOn(HTMLElement.prototype, 'getBoundingClientRect').mockReturnValue({
      x: 0,
      y: 0,
      width: MENU_WIDTH,
      height: MENU_HEIGHT,
      top: 0,
      left: 0,
      right: MENU_WIDTH,
      bottom: MENU_HEIGHT,
      toJSON: () => ({}),
    } as DOMRect);
  });

  afterEach(() => {
    Object.defineProperty(window, 'innerWidth', { configurable: true, value: originalWidth });
    Object.defineProperty(window, 'innerHeight', { configurable: true, value: originalHeight });
  });

  it('stays inside the viewport near the right and bottom edges', async () => {
    renderMenu(1000, 700);

    const menu = screen.getByRole('menu');
    await waitFor(() => {
      expect(menu.style.left).toBe(`${VIEWPORT_WIDTH - MENU_WIDTH - 8}px`);
      expect(menu.style.top).toBe(`${VIEWPORT_HEIGHT - MENU_HEIGHT - 8}px`);
    });
  });

  it('keeps a menu that fits at the requested position', async () => {
    renderMenu(100, 100);

    const menu = screen.getByRole('menu');
    await waitFor(() => {
      expect(menu.style.left).toBe('100px');
      expect(menu.style.top).toBe('100px');
    });
  });
});
