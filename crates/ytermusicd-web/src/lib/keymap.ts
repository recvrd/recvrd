export interface KeymapActions {
  move(delta: number): void;
  first(): void;
  last(): void;
  halfPage(direction: 1 | -1): void;
  openSearch(): void;
  playSelected(): void;
  togglePlayback(): void;
  queueSelected(): void;
  deleteSelected(): void;
  openHelp(): void;
  closeOverlays(): void;
  showLibrary(): void;
  showQueue(): void;
  moveFocus(): void;
  moveQueueItem(delta: number): void;
  requestClearQueue(): void;
  createPlaylist(): void;
  renamePlaylist(): void;
  playSelectedPlaylist(): void;
  addSelectedToPlaylist(): void;
  retrySelected(): void;
  toggleShuffle(): void;
  cycleRepeat(): void;
  cycleProvider(): void;
  adjustVolume(delta: number): void;
  seekRelative(deltaSecs: number): void;
  previousTrack(): void;
  nextTrack(): void;
}

export interface KeymapContext {
  typing?: boolean;
  overlayOpen?: boolean;
}

export interface KeymapOptions {
  sequenceTimeoutMs?: number;
  setTimeoutFn?: typeof setTimeout;
  clearTimeoutFn?: typeof clearTimeout;
}

export function isTypingTarget(target: EventTarget | null): boolean {
  if (!(target instanceof HTMLElement)) return false;
  const tag = target.tagName.toLowerCase();
  return (
    tag === 'input' ||
    tag === 'textarea' ||
    tag === 'select' ||
    target.isContentEditable
  );
}

/**
 * Central mode-aware key handler. Multi-key sequences (`gg`, `dd`) expire
 * after a short timeout, and shortcuts are ignored while an input is focused.
 *
 * Deliberately unbound: Ctrl+r, which the browser owns (page reload).
 */
export class Keymap {
  private readonly actions: KeymapActions;
  private readonly sequenceTimeoutMs: number;
  private readonly setTimeoutFn: typeof setTimeout;
  private readonly clearTimeoutFn: typeof clearTimeout;
  private pending: 'g' | 'd' | null = null;
  private pendingTimer: ReturnType<typeof setTimeout> | null = null;

  constructor(actions: KeymapActions, options: KeymapOptions = {}) {
    this.actions = actions;
    this.sequenceTimeoutMs = options.sequenceTimeoutMs ?? 650;
    // Browser timer functions require the global receiver when called through
    // this class instance.
    this.setTimeoutFn = options.setTimeoutFn ?? setTimeout.bind(globalThis);
    this.clearTimeoutFn = options.clearTimeoutFn ?? clearTimeout.bind(globalThis);
  }

  handle(event: KeyboardEvent, context: KeymapContext = {}): boolean {
    if (context.typing || isTypingTarget(event.target)) {
      if (event.key === 'Escape') return this.run(event, () => this.actions.closeOverlays());
      return false;
    }

    if (event.key === 'Escape') {
      return this.run(event, () => this.actions.closeOverlays());
    }

    if (context.overlayOpen) return false;

    if (event.ctrlKey && !event.altKey && !event.metaKey) {
      if (event.key === 'd') return this.run(event, () => this.actions.halfPage(1));
      if (event.key === 'u') return this.run(event, () => this.actions.halfPage(-1));
      if (event.key === 'k') return this.run(event, () => this.actions.openSearch());
      if (event.key === 'ArrowLeft') return this.run(event, () => this.actions.previousTrack());
      if (event.key === 'ArrowRight') return this.run(event, () => this.actions.nextTrack());
      return false;
    }

    if (event.metaKey || event.altKey) return false;

    if (this.pending !== null) {
      if (this.pending === 'g' && event.key === 'g') {
        this.clearPending();
        return this.run(event, () => this.actions.first());
      }
      if (this.pending === 'd' && event.key === 'd') {
        this.clearPending();
        return this.run(event, () => this.actions.deleteSelected());
      }
      this.clearPending();
    }

    switch (event.key) {
      case 'j':
      case 'ArrowDown':
        return this.run(event, () => this.actions.move(1));
      case 'k':
      case 'ArrowUp':
        return this.run(event, () => this.actions.move(-1));
      case 'Home':
        return this.run(event, () => this.actions.first());
      case 'End':
        return this.run(event, () => this.actions.last());
      case 'G':
        return this.run(event, () => this.actions.last());
      case 'g':
        this.startSequence('g');
        event.preventDefault();
        return true;
      case 'd':
        this.startSequence('d');
        event.preventDefault();
        return true;
      case '/':
        return this.run(event, () => this.actions.openSearch());
      case '?':
        return this.run(event, () => this.actions.openHelp());
      case 'Enter':
        return this.run(event, () => this.actions.playSelected());
      case ' ':
        return this.run(event, () => this.actions.togglePlayback());
      case 'x':
        return this.run(event, () => this.actions.queueSelected());
      case '1':
        return this.run(event, () => this.actions.showLibrary());
      case '2':
        return this.run(event, () => this.actions.showQueue());
      case 'Tab':
        return this.run(event, () => this.actions.moveFocus());
      case 'J':
        return this.run(event, () => this.actions.moveQueueItem(1));
      case 'K':
        return this.run(event, () => this.actions.moveQueueItem(-1));
      case 'D':
        return this.run(event, () => this.actions.requestClearQueue());
      case 'n':
        return this.run(event, () => this.actions.createPlaylist());
      case 'r':
        return this.run(event, () => this.actions.renamePlaylist());
      case 'p':
        return this.run(event, () => this.actions.playSelectedPlaylist());
      case 'a':
        return this.run(event, () => this.actions.addSelectedToPlaylist());
      case 't':
        return this.run(event, () => this.actions.retrySelected());
      case 's':
        return this.run(event, () => this.actions.toggleShuffle());
      case 'R':
        return this.run(event, () => this.actions.cycleRepeat());
      case 'o':
        return this.run(event, () => this.actions.cycleProvider());
      case '+':
      case '=':
        return this.run(event, () => this.actions.adjustVolume(5));
      case '-':
      case '_':
        return this.run(event, () => this.actions.adjustVolume(-5));
      case 'ArrowLeft':
        return this.run(event, () => this.actions.seekRelative(-5));
      case 'ArrowRight':
        return this.run(event, () => this.actions.seekRelative(5));
      default:
        return false;
    }
  }

  dispose(): void {
    this.clearPending();
  }

  private run(event: KeyboardEvent, action: () => void): boolean {
    event.preventDefault();
    action();
    return true;
  }

  private startSequence(key: 'g' | 'd'): void {
    this.clearPending();
    this.pending = key;
    this.pendingTimer = this.setTimeoutFn(() => {
      this.pending = null;
      this.pendingTimer = null;
    }, this.sequenceTimeoutMs);
  }

  private clearPending(): void {
    if (this.pendingTimer !== null) {
      this.clearTimeoutFn(this.pendingTimer);
      this.pendingTimer = null;
    }
    this.pending = null;
  }
}
