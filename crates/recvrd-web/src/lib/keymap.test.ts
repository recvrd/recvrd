import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest';
import { Keymap, isTypingTarget, type KeymapActions } from './keymap';

function actions(): KeymapActions {
  return {
    move: vi.fn(),
    first: vi.fn(),
    last: vi.fn(),
    halfPage: vi.fn(),
    openSearch: vi.fn(),
    playSelected: vi.fn(),
    togglePlayback: vi.fn(),
    queueSelected: vi.fn(),
    deleteSelected: vi.fn(),
    openHelp: vi.fn(),
    closeOverlays: vi.fn(),
    showLibrary: vi.fn(),
    showQueue: vi.fn(),
    moveFocus: vi.fn(),
    moveQueueItem: vi.fn(),
    requestClearQueue: vi.fn(),
    createPlaylist: vi.fn(),
    renamePlaylist: vi.fn(),
    playSelectedPlaylist: vi.fn(),
    addSelectedToPlaylist: vi.fn(),
    retrySelected: vi.fn(),
    toggleShuffle: vi.fn(),
    cycleRepeat: vi.fn(),
    cycleProvider: vi.fn(),
    adjustVolume: vi.fn(),
    seekRelative: vi.fn(),
    previousTrack: vi.fn(),
    nextTrack: vi.fn(),
  };
}

function key(value: string, init: KeyboardEventInit = {}) {
  return new KeyboardEvent('keydown', { key: value, cancelable: true, ...init });
}

function requireGlobalReceiver(this: unknown): void {
  if (this !== globalThis) throw new TypeError('Illegal invocation');
}

describe('keymap', () => {
  beforeEach(() => {
    vi.useFakeTimers();
  });

  afterEach(() => {
    vi.unstubAllGlobals();
    vi.useRealTimers();
  });

  it('handles vim movement and jumps', () => {
    const handler = actions();
    const keymap = new Keymap(handler);

    expect(keymap.handle(key('j'))).toBe(true);
    expect(handler.move).toHaveBeenLastCalledWith(1);
    expect(keymap.handle(key('k'))).toBe(true);
    expect(handler.move).toHaveBeenLastCalledWith(-1);

    keymap.handle(key('g'));
    keymap.handle(key('g'));
    expect(handler.first).toHaveBeenCalledOnce();
    expect(handler.move).toHaveBeenCalledTimes(2);

    keymap.handle(key('G'));
    expect(handler.last).toHaveBeenCalledOnce();
  });

  it('expires the gg sequence', () => {
    const handler = actions();
    const keymap = new Keymap(handler);
    keymap.handle(key('g'));
    vi.advanceTimersByTime(700);
    keymap.handle(key('g'));
    expect(handler.first).not.toHaveBeenCalled();
  });

  it('handles dd and ignores a single d', () => {
    const handler = actions();
    const keymap = new Keymap(handler);
    keymap.handle(key('d'));
    expect(handler.deleteSelected).not.toHaveBeenCalled();
    keymap.handle(key('d'));
    expect(handler.deleteSelected).toHaveBeenCalledOnce();
  });

  it('handles control shortcuts', () => {
    const handler = actions();
    const keymap = new Keymap(handler);
    keymap.handle(key('d', { ctrlKey: true }));
    expect(handler.halfPage).toHaveBeenLastCalledWith(1);
    keymap.handle(key('u', { ctrlKey: true }));
    expect(handler.halfPage).toHaveBeenLastCalledWith(-1);
    keymap.handle(key('k', { ctrlKey: true }));
    expect(handler.openSearch).toHaveBeenCalledOnce();
  });

  it('handles actions and help', () => {
    const handler = actions();
    const keymap = new Keymap(handler);
    keymap.handle(key('/'));
    keymap.handle(key('?'));
    keymap.handle(key('Enter'));
    keymap.handle(key(' '));
    keymap.handle(key('x'));
    keymap.handle(key('Escape'));

    expect(handler.openSearch).toHaveBeenCalledOnce();
    expect(handler.openHelp).toHaveBeenCalledOnce();
    expect(handler.playSelected).toHaveBeenCalledOnce();
    expect(handler.togglePlayback).toHaveBeenCalledOnce();
    expect(handler.queueSelected).toHaveBeenCalledOnce();
    expect(handler.closeOverlays).toHaveBeenCalledOnce();
  });

  it('handles view keys and focus', () => {
    const handler = actions();
    const keymap = new Keymap(handler);

    expect(keymap.handle(key('1'))).toBe(true);
    expect(handler.showLibrary).toHaveBeenCalledOnce();
    expect(keymap.handle(key('2'))).toBe(true);
    expect(handler.showQueue).toHaveBeenCalledOnce();

    const tab = key('Tab');
    expect(keymap.handle(tab)).toBe(true);
    expect(handler.moveFocus).toHaveBeenCalledOnce();
    expect(tab.defaultPrevented).toBe(true);
  });

  it('handles queue item moves and clear confirmation', () => {
    const handler = actions();
    const keymap = new Keymap(handler);

    expect(keymap.handle(key('J'))).toBe(true);
    expect(handler.moveQueueItem).toHaveBeenLastCalledWith(1);
    expect(keymap.handle(key('K'))).toBe(true);
    expect(handler.moveQueueItem).toHaveBeenLastCalledWith(-1);
    expect(keymap.handle(key('D'))).toBe(true);
    expect(handler.requestClearQueue).toHaveBeenCalledOnce();
  });

  it('handles playlist keys', () => {
    const handler = actions();
    const keymap = new Keymap(handler);

    keymap.handle(key('n'));
    keymap.handle(key('r'));
    keymap.handle(key('p'));
    keymap.handle(key('a'));
    keymap.handle(key('t'));

    expect(handler.createPlaylist).toHaveBeenCalledOnce();
    expect(handler.renamePlaylist).toHaveBeenCalledOnce();
    expect(handler.playSelectedPlaylist).toHaveBeenCalledOnce();
    expect(handler.addSelectedToPlaylist).toHaveBeenCalledOnce();
    expect(handler.retrySelected).toHaveBeenCalledOnce();
  });

  it('handles playback toggles and provider switching', () => {
    const handler = actions();
    const keymap = new Keymap(handler);

    keymap.handle(key('s'));
    keymap.handle(key('R'));
    keymap.handle(key('o'));

    expect(handler.toggleShuffle).toHaveBeenCalledOnce();
    expect(handler.cycleRepeat).toHaveBeenCalledOnce();
    expect(handler.cycleProvider).toHaveBeenCalledOnce();
  });

  it('handles volume keys', () => {
    const handler = actions();
    const keymap = new Keymap(handler);

    keymap.handle(key('+'));
    expect(handler.adjustVolume).toHaveBeenLastCalledWith(5);
    keymap.handle(key('='));
    expect(handler.adjustVolume).toHaveBeenLastCalledWith(5);
    keymap.handle(key('-'));
    expect(handler.adjustVolume).toHaveBeenLastCalledWith(-5);
    keymap.handle(key('_'));
    expect(handler.adjustVolume).toHaveBeenLastCalledWith(-5);
  });

  it('handles seek and track skipping', () => {
    const handler = actions();
    const keymap = new Keymap(handler);

    const left = key('ArrowLeft');
    expect(keymap.handle(left)).toBe(true);
    expect(handler.seekRelative).toHaveBeenLastCalledWith(-5);
    expect(left.defaultPrevented).toBe(true);
    keymap.handle(key('ArrowRight'));
    expect(handler.seekRelative).toHaveBeenLastCalledWith(5);

    keymap.handle(key('ArrowLeft', { ctrlKey: true }));
    expect(handler.previousTrack).toHaveBeenCalledOnce();
    keymap.handle(key('ArrowRight', { ctrlKey: true }));
    expect(handler.nextTrack).toHaveBeenCalledOnce();
  });

  it('leaves Ctrl+r to the browser', () => {
    const handler = actions();
    const keymap = new Keymap(handler);
    expect(keymap.handle(key('r', { ctrlKey: true }))).toBe(false);
    expect(handler.renamePlaylist).not.toHaveBeenCalled();
  });

  it('ignores new bindings while typing or with an overlay open', () => {
    const handler = actions();
    const keymap = new Keymap(handler);

    for (const value of ['1', 'Tab', 'J', 'D', 'n', 'o', '+', 'ArrowLeft']) {
      expect(keymap.handle(key(value), { typing: true })).toBe(false);
      expect(keymap.handle(key(value), { overlayOpen: true })).toBe(false);
    }
    expect(handler.showLibrary).not.toHaveBeenCalled();
    expect(handler.moveFocus).not.toHaveBeenCalled();
    expect(handler.moveQueueItem).not.toHaveBeenCalled();
    expect(handler.requestClearQueue).not.toHaveBeenCalled();
    expect(handler.createPlaylist).not.toHaveBeenCalled();
    expect(handler.cycleProvider).not.toHaveBeenCalled();
    expect(handler.adjustVolume).not.toHaveBeenCalled();
    expect(handler.seekRelative).not.toHaveBeenCalled();
  });

  it('ignores shortcuts while typing', () => {
    const handler = actions();
    const keymap = new Keymap(handler);
    const input = document.createElement('input');

    const handled = keymap.handle(
      new KeyboardEvent('keydown', { key: 'j', bubbles: true, cancelable: true }),
      { typing: true },
    );
    expect(handled).toBe(false);
    expect(handler.move).not.toHaveBeenCalled();

    const event = new KeyboardEvent('keydown', { key: 'j', cancelable: true });
    Object.defineProperty(event, 'target', { value: input });
    expect(keymap.handle(event)).toBe(false);
    expect(isTypingTarget(input)).toBe(true);
  });

  it('leaves overlay keys to the overlay', () => {
    const handler = actions();
    const keymap = new Keymap(handler);
    expect(keymap.handle(key('j'), { overlayOpen: true })).toBe(false);
    expect(handler.move).not.toHaveBeenCalled();
    expect(keymap.handle(key('Escape'), { overlayOpen: true })).toBe(true);
    expect(handler.closeOverlays).toHaveBeenCalledOnce();
  });

  it('uses browser timers with the global receiver', () => {
    const timeout = globalThis.setTimeout;
    const clear = globalThis.clearTimeout;
    vi.stubGlobal('setTimeout', function (this: unknown, handler: () => void, ms?: number) {
      requireGlobalReceiver.call(this);
      return timeout(handler, ms);
    });
    vi.stubGlobal('clearTimeout', function (this: unknown, handle: Parameters<typeof clearTimeout>[0]) {
      requireGlobalReceiver.call(this);
      clear(handle);
    });
    const handler = actions();
    const keymap = new Keymap(handler);

    expect(() => {
      keymap.handle(key('g'));
      keymap.handle(key('g'));
      keymap.handle(key('d'));
      keymap.dispose();
    }).not.toThrow();
    expect(handler.first).toHaveBeenCalledOnce();
  });
});
