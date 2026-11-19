import type { Event as ApiEvent } from '@api/Event';
import type { EventKind } from '@api/EventKind';

export type ConnectionState = 'connecting' | 'online' | 'reconnecting' | 'offline';

export interface EventSourceLike {
  onopen: ((event: Event) => void) | null;
  onerror: ((event: Event) => void) | null;
  onmessage: ((event: MessageEvent) => void) | null;
  addEventListener(type: string, listener: (event: MessageEvent) => void): void;
  close(): void;
}

export interface EventStreamOptions {
  url?: string;
  create?: (url: string) => EventSourceLike;
  onEvent: (event: EventKind) => void;
  onState: (state: ConnectionState) => void;
  onParseError?: (error: unknown) => void;
  /** Called when a later connection opens after an earlier one was online. */
  onReconnect?: () => void;
  offlineAfterMs?: number;
  retryBaseMs?: number;
  retryMaxMs?: number;
  /** How often the watchdog checks for silence. Defaults to 5000. */
  watchdogIntervalMs?: number;
  /** Silence that counts as a dead stream. Defaults to 45000. */
  silenceMs?: number;
  setTimeoutFn?: typeof setTimeout;
  clearTimeoutFn?: typeof clearTimeout;
  setIntervalFn?: typeof setInterval;
  clearIntervalFn?: typeof clearInterval;
  randomFn?: () => number;
}

interface ResolvedOptions {
  url: string;
  create: (url: string) => EventSourceLike;
  onEvent: (event: EventKind) => void;
  onState: (state: ConnectionState) => void;
  onParseError?: (error: unknown) => void;
  onReconnect?: () => void;
  offlineAfterMs: number;
  retryBaseMs: number;
  retryMaxMs: number;
  watchdogIntervalMs: number;
  silenceMs: number;
  setTimeoutFn: typeof setTimeout;
  clearTimeoutFn: typeof clearTimeout;
  setIntervalFn: typeof setInterval;
  clearIntervalFn: typeof clearInterval;
  randomFn: () => number;
}

function defaultCreate(url: string): EventSourceLike {
  return new EventSource(url) as unknown as EventSourceLike;
}

/**
 * EventSource wrapper with explicit exponential backoff. The last event id is
 * sent as a query parameter, because a new EventSource cannot set headers.
 * The daemon accepts both the query parameter and the standard header.
 *
 * Suspend recovery: the daemon sends a named `heartbeat` event every 15 s
 * (and EventSource stays silent on a dead TCP connection, so no error fires
 * until data moves). This wrapper tracks the last open, event, or heartbeat
 * and reconnects when the stream has been silent for `silenceMs`; it also
 * reconnects on `online` / `visibilitychange` / `pageshow` when the page
 * becomes visible and the stream is offline or stale.
 */
export class EventStream {
  private readonly options: ResolvedOptions;
  private source: EventSourceLike | null = null;
  private offlineTimer: ReturnType<typeof setTimeout> | null = null;
  private retryTimer: ReturnType<typeof setTimeout> | null = null;
  private watchdogTimer: ReturnType<typeof setInterval> | null = null;
  private attempts = 0;
  private lastEventId: number | null = null;
  private lastActivityMs = 0;
  private closed = false;
  private online = false;
  private wasOnline = false;
  private wakeInstalled = false;

  constructor(options: EventStreamOptions) {
    this.options = {
      url: options.url ?? '/api/events',
      create: options.create ?? defaultCreate,
      onEvent: options.onEvent,
      onState: options.onState,
      onParseError: options.onParseError,
      onReconnect: options.onReconnect,
      offlineAfterMs: options.offlineAfterMs ?? 5000,
      retryBaseMs: options.retryBaseMs ?? 1000,
      retryMaxMs: options.retryMaxMs ?? 30000,
      watchdogIntervalMs: options.watchdogIntervalMs ?? 5000,
      silenceMs: options.silenceMs ?? 45000,
      setTimeoutFn: options.setTimeoutFn ?? setTimeout,
      clearTimeoutFn: options.clearTimeoutFn ?? clearTimeout,
      setIntervalFn: options.setIntervalFn ?? setInterval,
      clearIntervalFn: options.clearIntervalFn ?? clearInterval,
      randomFn: options.randomFn ?? Math.random,
    };
  }

  connect(): void {
    this.closed = false;
    this.attempts = 0;
    // A pending retry/offline timer from an earlier attempt must not
    // survive a manual (re)connect: it would open a second source. The
    // timers are cleared here, not in open(): a scheduled reconnect must
    // leave the offline timer running, or the UI would never report
    // 'offline' while reconnect attempts keep coming.
    this.clearRetryTimer();
    this.clearOfflineTimer();
    this.installWakeHandlers();
    this.noteActivity();
    this.startWatchdog();
    this.open(false);
  }

  retry(): void {
    this.connect();
  }

  close(): void {
    this.closed = true;
    this.clearOfflineTimer();
    this.clearRetryTimer();
    this.clearWatchdogTimer();
    this.removeWakeHandlers();
    this.disposeSource();
  }

  private open(reconnecting: boolean): void {
    this.disposeSource();
    this.options.onState(reconnecting ? 'reconnecting' : 'connecting');

    const url =
      this.lastEventId === null
        ? this.options.url
        : `${this.options.url}?last_event_id=${this.lastEventId}`;
    const source = this.options.create(url);
    this.source = source;

    source.onopen = () => {
      this.attempts = 0;
      this.noteActivity();
      this.clearOfflineTimer();
      this.clearRetryTimer();
      this.online = true;
      this.options.onState('online');
      if (this.wasOnline) this.options.onReconnect?.();
      this.wasOnline = true;
    };

    source.onerror = () => {
      if (this.closed) return;
      this.online = false;
      this.options.onState('reconnecting');
      this.disposeSource();
      this.startOfflineTimer();
      this.scheduleReconnect();
    };

    source.addEventListener('snapshot', (event) => this.handleMessage(event));
    source.addEventListener('heartbeat', () => this.noteActivity());
    source.onmessage = (event) => this.handleMessage(event);
  }

  private scheduleReconnect(): void {
    this.clearRetryTimer();
    const capped = Math.min(
      this.options.retryBaseMs * 2 ** this.attempts,
      this.options.retryMaxMs,
    );
    // Jitter between 50% and 100% of the capped delay, so many clients do
    // not all retry at the same instant after a shared outage.
    const delay = capped * (0.5 + this.options.randomFn() * 0.5);
    this.attempts += 1;
    this.retryTimer = this.options.setTimeoutFn(() => {
      this.retryTimer = null;
      if (!this.closed) this.open(true);
    }, delay);
  }

  private startOfflineTimer(): void {
    if (this.offlineTimer !== null) return;
    this.offlineTimer = this.options.setTimeoutFn(() => {
      this.offlineTimer = null;
      if (!this.closed) this.options.onState('offline');
    }, this.options.offlineAfterMs);
  }

  private startWatchdog(): void {
    if (this.watchdogTimer !== null) return;
    this.watchdogTimer = this.options.setIntervalFn(() => {
      if (this.closed || this.source === null) return;
      if (Date.now() - this.lastActivityMs >= this.options.silenceMs) {
        // Dead TCP connection: no error will fire until data moves, so
        // reconnect directly instead of waiting for the next retry.
        this.clearRetryTimer();
        this.clearOfflineTimer();
        this.noteActivity();
        this.open(true);
      }
    }, this.options.watchdogIntervalMs);
  }

  private noteActivity(): void {
    this.lastActivityMs = Date.now();
  }

  private disposeSource(): void {
    if (this.source) {
      this.source.onopen = null;
      this.source.onerror = null;
      this.source.onmessage = null;
      this.source.close();
      this.source = null;
    }
  }

  private clearOfflineTimer(): void {
    if (this.offlineTimer !== null) {
      this.options.clearTimeoutFn(this.offlineTimer);
      this.offlineTimer = null;
    }
  }

  private clearRetryTimer(): void {
    if (this.retryTimer !== null) {
      this.options.clearTimeoutFn(this.retryTimer);
      this.retryTimer = null;
    }
  }

  private clearWatchdogTimer(): void {
    if (this.watchdogTimer !== null) {
      this.options.clearIntervalFn(this.watchdogTimer);
      this.watchdogTimer = null;
    }
  }

  private handleMessage(message: MessageEvent): void {
    this.noteActivity();
    try {
      const envelope = JSON.parse(String(message.data)) as ApiEvent;
      if (typeof envelope?.id === 'number') this.lastEventId = envelope.id;
      if (envelope?.event) this.options.onEvent(envelope.event);
    } catch (error) {
      this.options.onParseError?.(error);
    }
  }

  private readonly onWakeOnline = (): void => {
    this.maybeWakeReconnect();
  };

  private readonly onWakeVisibility = (): void => {
    if (typeof document !== 'undefined' && document.visibilityState === 'visible') {
      this.maybeWakeReconnect();
    }
  };

  private readonly onWakePageshow = (): void => {
    this.maybeWakeReconnect();
  };

  private maybeWakeReconnect(): void {
    // Only reconnect a stream that was online before: before the first
    // successful open there is nothing to recover, and a pageshow at page
    // load must not open a second source.
    if (this.closed || !this.wasOnline) return;
    const stale = Date.now() - this.lastActivityMs >= this.options.silenceMs;
    if (this.online && !stale) return;
    // Like a manual retry: drop any pending retry/offline timers so the
    // wake reconnect is the only attempt in flight.
    this.clearRetryTimer();
    this.clearOfflineTimer();
    this.noteActivity();
    this.open(true);
  }

  private installWakeHandlers(): void {
    if (this.wakeInstalled) return;
    this.wakeInstalled = true;
    if (typeof window !== 'undefined') {
      window.addEventListener('online', this.onWakeOnline);
      window.addEventListener('pageshow', this.onWakePageshow);
    }
    if (typeof document !== 'undefined') {
      document.addEventListener('visibilitychange', this.onWakeVisibility);
    }
  }

  private removeWakeHandlers(): void {
    if (!this.wakeInstalled) return;
    this.wakeInstalled = false;
    if (typeof window !== 'undefined') {
      window.removeEventListener('online', this.onWakeOnline);
      window.removeEventListener('pageshow', this.onWakePageshow);
    }
    if (typeof document !== 'undefined') {
      document.removeEventListener('visibilitychange', this.onWakeVisibility);
    }
  }
}
